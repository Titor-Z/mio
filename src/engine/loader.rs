//! 转译型模块加载器（Boa `ModuleLoader` 实现）。
//!
//! 与 Boa 自带的 `SimpleModuleLoader` 相比，本实现在读取文件与解析模块之间插入一次
//! TypeScript 转译，从而让 `import` 语义直接落到 `.ts` 源文件上；同时把已解析的模块按
//! 绝对路径缓存，实现「同一文件多次说明符命中同一实例」与循环依赖复用。
//!
//! 路径解析委托给 [`crate::resolver::ModuleResolver`]：Import Maps 重写、相对/绝对判定、
//! 扩展名补全与目录 `index.*` 命中都在其中。本文件只承担「拿到路径→读→转译→解析→缓存」。
//!
//! 另一类是 `@std/*` 内建模块：不读文件，而是把 [`crate::stdlib`] 里的纯 Rust 实现（引擎无关）
//! 装配为 Boa 合成模块（synthetic module）。这是引擎侧的包装职责，`boa_*` 因此仍只在本模块出现。

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use boa_engine::module::{ModuleLoader, ModuleRequest, Referrer, SyntheticModuleInitializer};
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, Module, NativeFunction, Source, js_string,
};

use crate::loader::RemoteCache;
use crate::resolver::ModuleResolver;
use crate::sourcemap::{SourceMapEntry, SourceMapRegistry, remap_stack};
use crate::transpiler::{TranspileConfig, Transpiler};

/// 携带转译步骤与路径缓存的模块加载器。
pub struct TsModuleLoader {
    cache: RefCell<rustc_hash::FxHashMap<PathBuf, Module>>,
    resolver: ModuleResolver,
    transpile: TranspileConfig,
    registry: RefCell<SourceMapRegistry>,
    /// 远程下载缓存；`None` 表示本次运行不允许远程导入。
    remote: RefCell<Option<Rc<RemoteCache>>>,
    /// 远程导入是否被显式禁用（`--no-remote`），用于给出准确的报错。
    remote_disabled: Cell<bool>,
}

impl Default for TsModuleLoader {
    fn default() -> Self {
        Self::new(ModuleResolver::default(), TranspileConfig::default())
    }
}

impl TsModuleLoader {
    /// 用指定解析器与转译参数构造加载器（解析器携带 Import Maps 与基准目录）。
    pub fn new(resolver: ModuleResolver, transpile: TranspileConfig) -> Self {
        Self {
            cache: RefCell::new(rustc_hash::FxHashMap::default()),
            resolver,
            transpile,
            registry: RefCell::new(SourceMapRegistry::default()),
            remote: RefCell::new(None),
            remote_disabled: Cell::new(false),
        }
    }

    /// 挂上远程下载缓存（构造后再挂，避开在引擎构造里多开一个参数）。
    pub fn attach_remote(&self, remote: Rc<RemoteCache>) {
        *self.remote.borrow_mut() = Some(remote);
    }

    /// 显式禁用远程导入（`--no-remote`）。
    pub fn disable_remote(&self) {
        self.remote_disabled.set(true);
    }

    /// 登记一份产物对应的源码映射（入口文件由引擎侧转译，映射在此汇入同一注册表）。
    pub fn register_source_map(&self, path: &Path, mapping: String) {
        self.registry.borrow_mut().register(
            path,
            SourceMapEntry {
                source_path: path.to_path_buf(),
                mapping,
            },
        );
    }

    /// 把错误文本中命中已登记映射的栈帧坐标改写回 TypeScript 行列。
    pub fn remap_error_text(&self, text: &str) -> String {
        match remap_stack(text, &self.registry.borrow()) {
            std::borrow::Cow::Owned(rewritten) => rewritten,
            std::borrow::Cow::Borrowed(original) => original.to_string(),
        }
    }

    /// 把已经解析好的模块预填入缓存，用于入口模块：让后续 import 命中同一实例，
    /// 也让入口文件被自己间接 import 时避免二次转译。
    pub fn register(&self, path: PathBuf, module: Module) {
        self.cache.borrow_mut().insert(path, module);
    }
}

impl ModuleLoader for TsModuleLoader {
    fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: ModuleRequest,
        context: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<Module>> {
        // 全部工作同步完成，用 `async { result }` 满足 trait 的 Future 契约。
        let result: JsResult<Module> = (|| {
            let specifier = request.specifier().to_std_string_escaped();

            // 先过 Import Maps：若重写后仍是 `@std/` 前缀，说明用户未把该内建模块
            // 改指向本地路径，按合成模块装配；否则继续走文件解析。
            let rewritten = self.resolver.rewrite(&specifier);
            if let Some(builtin) = rewritten.strip_prefix("@std/") {
                return self.load_builtin(builtin, context);
            }

            // 远程导入：绝对 URL，或引用者本身是远程模块时的相对说明符。
            // Import Maps 把别名映射到 URL 的情形也在此处命中（rewritten 已是 URL）。
            let remote = self.remote.borrow().clone();
            if let Some((cache, url)) = remote.as_ref().and_then(|cache| {
                cache
                    .locate(&rewritten, referrer.path())
                    .map(|url| (cache, url))
            }) {
                let path = cache
                    .materialize(&url)
                    .map_err(|message| JsNativeError::typ().with_message(message))?;
                return self.load_file_at(&path, Some(&url), context);
            }

            // 未挂缓存但说明符就是 URL：区分「显式禁用」与「无法处理」两种原因。
            if remote.is_none()
                && let Some(url) = crate::loader::remote::remote_url(&rewritten)
            {
                return Err(if self.remote_disabled.get() {
                    JsNativeError::typ()
                        .with_message(format!("远程导入已被 `--no-remote` 禁用：{url}"))
                        .into()
                } else {
                    JsNativeError::typ()
                        .with_message(format!("无法加载远程模块：{url}"))
                        .into()
                });
            }

            self.load_file(&specifier, referrer.path(), context)
        })();

        async { result }
    }
}

impl TsModuleLoader {
    /// 加载一个文件模块：解析→读→转译→解析为模块→按绝对路径缓存。
    fn load_file(
        &self,
        specifier: &str,
        referrer: Option<&std::path::Path>,
        context: &RefCell<&mut Context>,
    ) -> JsResult<Module> {
        let path = self
            .resolver
            .resolve(specifier, referrer)
            .map_err(|error| {
                JsNativeError::typ().with_message(format!("模块解析失败 `{specifier}`：{error}"))
            })?;
        self.load_file_at(&path, None, context)
    }

    /// 加载指定路径的模块；`url` 为 `Some` 时它是远程模块，报错与来源标记用 URL 代替缓存路径。
    fn load_file_at(
        &self,
        path: &Path,
        url: Option<&str>,
        context: &RefCell<&mut Context>,
    ) -> JsResult<Module> {
        if let Some(existing) = self.cache.borrow().get(path) {
            return Ok(existing.clone());
        }

        // 远程模块的报错里不出现本地缓存路径，换成用户认得的 URL。
        let label = url
            .map(str::to_string)
            .unwrap_or_else(|| path.display().to_string());

        let raw = std::fs::read_to_string(path).map_err(|error| {
            JsNativeError::typ().with_message(format!("无法读取 {label}：{error}"))
        })?;

        let output = Transpiler::transpile(&raw, path, &self.transpile).map_err(|diagnostics| {
            JsNativeError::syntax()
                .with_message(format!("转译 {label} 失败：{}", diagnostics.join("\n")))
        })?;

        if let Some(mapping) = output.sourcemap {
            self.register_source_map(path, mapping);
        }

        let module = Module::parse(
            Source::from_bytes(&output.code).with_path(path),
            None,
            &mut context.borrow_mut(),
        )
        .map_err(|error| {
            JsNativeError::syntax().with_message(format!("解析 {label} 失败：{error}"))
        })?;

        self.cache
            .borrow_mut()
            .insert(path.to_path_buf(), module.clone());
        Ok(module)
    }

    /// 加载一个 `@std/<name>` 内建模块：以 `@std/<name>` 为缓存键去重，未命中时装配合成模块。
    fn load_builtin(&self, name: &str, context: &RefCell<&mut Context>) -> JsResult<Module> {
        let key = PathBuf::from(format!("@std/{name}"));
        if let Some(existing) = self.cache.borrow().get(&key) {
            return Ok(existing.clone());
        }

        let module = build_builtin(name, &mut context.borrow_mut())?;
        self.cache.borrow_mut().insert(key, module.clone());
        Ok(module)
    }
}

/// 把 `@std/<name>` 的描述表装配为 Boa 合成模块。
///
/// 初始化闭包只捕获 `&'static` 切片（均为 `Copy`），故可用 `from_copy_closure`；
/// 真正的 `NativeFunction` 在求值时按 `crate::stdlib` 的纯函数现场构造。
fn build_builtin(name: &str, context: &mut Context) -> JsResult<Module> {
    // `fs` 返回 Promise / 对象 / 二进制，无法用「字符串进、字符串出」的通用装配，
    // 由 engine 侧专用构建器负责。
    if name == "fs" {
        return crate::engine::fs::build_fs_module(context);
    }
    // 同理：`os` 返回数值、`env` 返回 `undefined`/对象、`encoding` 返回 `Uint8Array`、
    // `log` 返回 `undefined` 且要写 stderr，均走各自的专用构建器。
    match name {
        "os" => return super::os::build_os_module(context),
        "env" => return super::env::build_env_module(context),
        "encoding" => return super::encoding::build_encoding_module(context),
        "log" => return super::log::build_log_module(context),
        _ => {}
    }

    let builtin = crate::stdlib::lookup(name).ok_or_else(|| {
        JsNativeError::typ().with_message(format!("未知的内建模块 `@std/{name}`"))
    })?;

    let functions = builtin.functions;
    let constants = builtin.string_constants;

    let export_names: Vec<JsString> = functions
        .iter()
        .map(|function| js_string!(function.name))
        .chain(constants.iter().map(|(key, _)| js_string!(*key)))
        .collect();

    let initializer = SyntheticModuleInitializer::from_copy_closure(move |synthetic, ctx| {
        for function in functions {
            let call = function.call;
            let export_name = function.name;
            let native = NativeFunction::from_copy_closure(move |_this, args, _ctx| {
                let mut owned: Vec<String> = Vec::with_capacity(args.len());
                for arg in args {
                    let Some(value) = arg.as_string() else {
                        return Err(JsNativeError::typ()
                            .with_message(format!("内建函数 `{export_name}` 只接受字符串参数"))
                            .into());
                    };
                    owned.push(value.to_std_string_escaped());
                }
                let references: Vec<&str> = owned.iter().map(String::as_str).collect();
                match call(&references) {
                    Ok(result) => Ok(JsValue::from(js_string!(result))),
                    Err(message) => Err(JsNativeError::typ().with_message(message).into()),
                }
            });

            let value = FunctionObjectBuilder::new(ctx.realm(), native)
                .name(js_string!(export_name))
                .build();
            synthetic.set_export(&js_string!(export_name), value.into())?;
        }

        for (key, value) in constants {
            synthetic.set_export(&js_string!(*key), JsValue::from(js_string!(*value)))?;
        }

        Ok(())
    });

    Ok(Module::synthetic(
        &export_names,
        initializer,
        Some(PathBuf::from(format!("@std/{name}"))),
        None,
        context,
    ))
}

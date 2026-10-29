//! 转译型模块加载器（Boa `ModuleLoader` 实现）。
//!
//! 与 Boa 自带的 `SimpleModuleLoader` 相比，本实现在读取文件与解析模块之间插入一次
//! TypeScript 转译，从而让 `import` 语义直接落到 `.ts` 源文件上；同时把已解析的模块按
//! 绝对路径缓存，实现「同一文件多次说明符命中同一实例」与循环依赖复用。
//!
//! 路径解析委托给 [`crate::resolver::ModuleResolver`]：Import Maps 重写、相对/绝对判定、
//! 扩展名补全与目录 `index.*` 命中都在其中。本文件只承担「拿到路径→读→转译→解析→缓存」。

use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;

use boa_engine::module::{ModuleLoader, ModuleRequest, Referrer};
use boa_engine::{Context, JsNativeError, JsResult, Module, Source};

use crate::resolver::ModuleResolver;
use crate::transpiler::{TranspileConfig, Transpiler};

/// 携带转译步骤与路径缓存的模块加载器。
#[derive(Default)]
pub struct TsModuleLoader {
    cache: RefCell<rustc_hash::FxHashMap<PathBuf, Module>>,
    resolver: ModuleResolver,
}

impl TsModuleLoader {
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
            let path = self
                .resolver
                .resolve(&specifier, referrer.path())
                .map_err(|error| {
                    JsNativeError::typ()
                        .with_message(format!("模块解析失败 `{specifier}`：{error}"))
                })?;

            if let Some(existing) = self.cache.borrow().get(&path) {
                return Ok(existing.clone());
            }

            let raw = std::fs::read_to_string(&path).map_err(|error| {
                JsNativeError::typ().with_message(format!("无法读取 {}：{error}", path.display()))
            })?;

            let output = Transpiler::transpile(&raw, &path, &TranspileConfig::default()).map_err(
                |diagnostics| {
                    JsNativeError::syntax().with_message(format!(
                        "转译 {} 失败：{}",
                        path.display(),
                        diagnostics.join("\n")
                    ))
                },
            )?;

            let module = Module::parse(
                Source::from_bytes(&output.code).with_path(&path),
                None,
                &mut context.borrow_mut(),
            )
            .map_err(|error| {
                JsNativeError::syntax()
                    .with_message(format!("解析 {} 失败：{error}", path.display()))
            })?;

            self.cache.borrow_mut().insert(path, module.clone());
            Ok(module)
        })();

        async { result }
    }
}

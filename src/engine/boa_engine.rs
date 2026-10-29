//! 基于 Boa 的执行引擎实现（阶段 A）。
//!
//! 这是阶段 A 与阶段 B 之间第一个接缝的具体实现：`boa_*` 只允许出现在本模块，
//! 上层只通过 [`Engine`] 抽象与本文件交互。替换阶段 B 的自研虚拟机时，只需新增
//! 一个实现 [`Engine`] 的类型，上游命令行与加载逻辑不受影响。
//!
//! 当前实现覆盖里程碑 1 的最小垂直切片：读取单文件 → 转译擦除类型 → 以 ES Module
//! 语义求值 → 驱动微任务。多文件模块图、`@std/*` 与 `fetch` 在此之上分层补齐。

use std::path::Path;

use boa_engine::builtins::promise::PromiseState;
use boa_engine::object::FunctionObjectBuilder;
use boa_engine::property::{Attribute, PropertyDescriptor};
use boa_engine::{Context, JsError, JsObject, JsValue, NativeFunction, Source, js_string};

use super::{Engine, EngineError};
use crate::transpiler::{TranspileConfig, Transpiler};

/// Boa 引擎包装。持有唯一的活动 [`Context`]。
pub struct BoaEngine {
    context: Context,
}

impl Default for BoaEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl BoaEngine {
    /// 构造引擎并注入最小可观测全局（`console.log`）。
    pub fn new() -> Self {
        let mut context = Context::default();
        install_console(&mut context);
        Self { context }
    }
}

impl Engine for BoaEngine {
    fn eval_module_file(&mut self, path: &Path) -> Result<String, EngineError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|error| EngineError::new("读取", format!("{}：{error}", path.display())))?;

        let output = Transpiler::transpile(&raw, path, &TranspileConfig::default())
            .map_err(|diagnostics| EngineError::new("转译", diagnostics.join("\n")))?;

        self.eval_module_source(&output.code, Some(path))
    }

    fn eval_module_source(
        &mut self,
        source: &str,
        hint: Option<&Path>,
    ) -> Result<String, EngineError> {
        let module = match hint {
            Some(path) => boa_engine::Module::parse(
                Source::from_bytes(source).with_path(path),
                None,
                &mut self.context,
            ),
            None => boa_engine::Module::parse(Source::from_bytes(source), None, &mut self.context),
        }
        .map_err(|error| EngineError::new("解析", error.to_string()))?;

        // load_link_evaluate 走 加载→链接→求值 全生命周期，错误以 rejected Promise 形式返回；
        // 先驱动微任务，再检查最终状态以向上暴露运行期异常。
        let promise = module.load_link_evaluate(&mut self.context);
        self.run_jobs()?;
        if let PromiseState::Rejected(reason) = promise.state() {
            return Err(EngineError::new(
                "运行期",
                JsError::from_opaque(reason).to_string(),
            ));
        }

        Ok(String::new())
    }

    fn eval_script(&mut self, source: &str) -> Result<String, EngineError> {
        let value = self
            .context
            .eval(Source::from_bytes(source))
            .map_err(|error| EngineError::new("运行期", error.to_string()))?;
        Ok(display_value(&value))
    }

    fn run_jobs(&mut self) -> Result<(), EngineError> {
        self.context
            .run_jobs()
            .map_err(|error| EngineError::new("微任务", error.to_string()))
    }
}

/// 注入 `console` 全局对象，当前仅实现 `log`。
///
/// 完整版应落在 `web` 层；在依赖隔离约束下（`boa_*` 仅限 `src/engine/`）先由本文件提供，
/// 阶段 B 引擎替换时一并迁移为与引擎无关的全局描述符。
fn install_console(context: &mut Context) {
    let log = NativeFunction::from_copy_closure(|_this, args, _ctx| {
        let line = args
            .iter()
            .map(format_for_log)
            .collect::<Vec<_>>()
            .join(" ");
        println!("{line}");
        Ok(JsValue::undefined())
    });

    let console = JsObject::default(context.intrinsics());
    let function = FunctionObjectBuilder::new(context.realm(), log)
        .name(js_string!("log"))
        .build();
    let descriptor = PropertyDescriptor::builder()
        .value(function)
        .writable(true)
        .enumerable(true)
        .configurable(true)
        .build();
    console.insert_property(js_string!("log"), descriptor);

    context
        .register_global_property(js_string!("console"), console, Attribute::all())
        .expect("安装 console 全局失败");
}

/// 把求值结果转成面向人类的文本；未定义值渲染为空串。
fn display_value(value: &JsValue) -> String {
    if value.is_undefined() {
        String::new()
    } else {
        value.display().to_string()
    }
}

/// `console.log` 的实参格式化：字符串按原样输出（不带引号），其余走调试展示。
fn format_for_log(value: &JsValue) -> String {
    match value.as_string() {
        Some(string) => string.to_std_string_escaped(),
        None => value.display().to_string(),
    }
}

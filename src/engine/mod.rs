//! 执行引擎层。
//!
//! 这一层是阶段 A 与阶段 B 之间的第一个接缝：上层（命令行、运行时装配）
//! 只认识 [`Engine`] 抽象，不认识任何具体引擎。
//!
//! 阶段 A 提供基于 Boa 的实现；阶段 B 将提供自研字节码虚拟机的实现。
//! 两个实现之间的切换不应引起上游任何改动。
//!
//! 依赖约束：`boa_*` 只允许出现在本模块内部，其余模块一律不得直接引用。

mod boa_engine;
mod error;

pub use boa_engine::BoaEngine;
pub use error::EngineError;

use std::path::Path;

/// 执行引擎抽象。
///
/// 所有方法以「模块」为单位，而非以字符串为单位，
/// 因为类型擦除后的代码需要按文件路径参与模块解析与缓存。
pub trait Engine {
    /// 以 ES Module 语义执行一个文件，返回其求值结果的可读描述。
    fn eval_module_file(&mut self, path: &Path) -> Result<String, EngineError>;

    /// 以 ES Module 语义执行一段内联源码，`hint` 用于相对导入的基准路径。
    fn eval_module_source(
        &mut self,
        source: &str,
        hint: Option<&Path>,
    ) -> Result<String, EngineError>;

    /// 以 Script 语义执行一段源码（不具备导入导出能力）。
    fn eval_script(&mut self, source: &str) -> Result<String, EngineError>;

    /// 驱动一次微任务检查点，用于兑现 Promise 回调。
    fn run_jobs(&mut self) -> Result<(), EngineError>;
}

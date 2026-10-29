//! 引擎层错误类型。

use std::fmt;

/// 执行引擎在编译或运行期产生的错误。
///
/// 保留 `phase` 字段而非拆成多个枚举变体，
/// 是因为上层处理方式一致：格式化后打印并退出。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    /// 出错阶段的中文描述，例如「解析」「转译」「运行期」。
    pub phase: String,
    /// 面向使用者的错误说明。
    pub message: String,
}

impl EngineError {
    /// 构造一个引擎错误。
    pub fn new(phase: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            phase: phase.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}：{}", self.phase, self.message)
    }
}

impl std::error::Error for EngineError {}

//! 配置层：`mio.json` 的解析、发现与优先级叠加。
//!
//! 配置来源按优先级从低到高叠加：
//! 内建默认 → `mio.json` → `MIO_*` 环境变量 → 命令行选项。
//!
//! 每个字段都必须在文档注释中标注生效状态，共三档：
//! - 生效：本版本真实影响运行时行为
//! - 仅提示：本版本只记录并告警，不改变行为
//! - 预留：字段被解析但完全不参与运行时

mod overrides;

pub use overrides::ConfigOverride;

/// 配置覆盖项：命令行或环境变量中对配置的单点修改。
///
/// 与完整配置分离，是为了让「用户写了什么」与「最终生效什么」可以分别断言。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MioConfig {
    /// 详见各子配置模块。
    pub language: LanguageConfig,
}

/// 语言相关配置。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LanguageConfig {
    /// ECMAScript 版本闸门，例如 `es2025`。当前仅提示，不做语法拦截。
    pub version: Option<String>,
}

//! 命令行与环境变量带来的配置覆盖项。

/// 需要在运行时覆盖配置的少量选项。
///
/// 字段使用 `Option` 语义：`None` 表示「不覆盖，沿用配置文件」。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigOverride {
    /// 类型检查开关，取值 `tsgo` 或 `off`。
    pub typecheck: Option<String>,
}

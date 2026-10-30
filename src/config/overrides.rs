//! 命令行与环境变量带来的配置覆盖项。
//!
//! 与完整配置分离，是为了让「用户写了什么」与「最终生效什么」可以分别断言，
//! 也让优先级叠加的每一层都能独立测试。

/// 需要覆盖配置的少量选项。
///
/// 字段使用 `Option` 语义：`None` 表示「不覆盖，沿用下层结果」。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigOverride {
    /// 类型检查开关，取值 `on` 或 `off`。对应 `--typecheck` / `--no-typecheck`。
    pub typecheck: Option<String>,

    /// 事件循环超时毫秒数。对应 `--timeout <ms>`。
    pub timeout_ms: Option<u64>,

    /// 诊断级别。对应 `--log-level <level>`。
    pub log_level: Option<String>,
}

impl ConfigOverride {
    /// 判定是否存在任何覆盖项。
    ///
    /// 用于在诊断输出中区分「命令行改过」与「仅来自配置文件」两种情况。
    pub fn is_empty(&self) -> bool {
        self.typecheck.is_none() && self.timeout_ms.is_none() && self.log_level.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_override_is_empty() {
        assert!(ConfigOverride::default().is_empty());
    }

    #[test]
    fn setting_typecheck_makes_override_non_empty() {
        let overrides = ConfigOverride {
            typecheck: Some(String::from("on")),
            ..ConfigOverride::default()
        };
        assert!(!overrides.is_empty());
    }
}

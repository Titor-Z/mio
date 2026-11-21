//! `@std/log` 内建模块的纯 Rust 实现（引擎无关）。
//!
//! 只负责两件事：级别的解析/比较，以及一行的文本或 JSON 渲染。
//! 「写到 stderr」与「级别的运行时可变」属于引擎绑定层（`engine/log.rs`）。
//!
//! 级别与 `mio.json` 的 `log.level` 是同一套取值：`silent` / `error` / `warn` / `info` / `debug`。

/// 日志级别，按严重程度从高到低排列（`Silent` 关闭全部输出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// 静默：不输出任何日志。
    Silent,
    /// 只输出 error。
    Error,
    /// 输出 error / warn。
    Warn,
    /// 输出 error / warn / info。
    Info,
    /// 输出全部。
    Debug,
}

impl Level {
    /// 解析级别文本；不区分大小写，未知取值返回 `None`。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "silent" => Some(Self::Silent),
            "error" => Some(Self::Error),
            "warn" => Some(Self::Warn),
            "info" => Some(Self::Info),
            "debug" => Some(Self::Debug),
            _ => None,
        }
    }

    /// 级别的脚本可见文本。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Silent => "silent",
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
            Self::Debug => "debug",
        }
    }

    /// 当前阈值下是否输出该级别的消息：级别数值越大越「啰嗦」，
    /// 因此消息级别不高于阈值即输出（阈值 `silent` 关闭一切）。
    pub fn allows(self, message: Self) -> bool {
        self != Self::Silent && message <= self
    }
}

/// 渲染一行日志：非 JSON 为 `[level] message`，JSON 为单行对象。
///
/// 时间戳沿用约定 2（毫秒浮点数），由调用方传入，便于测试固定时间。
pub fn render(level: Level, message: &str, json: bool, timestamp_ms: f64) -> String {
    if !json {
        return format!("[{}] {message}", level.as_str());
    }
    format!(
        "{{\"level\":\"{}\",\"msg\":\"{}\",\"ts\":{timestamp_ms}}}",
        level.as_str(),
        escape_json(message)
    )
}

/// JSON 字符串转义：只处理必需的引号、反斜杠与控制字符。
fn escape_json(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            control if (control as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", control as u32));
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_levels_case_insensitively() {
        for (text, level) in [
            ("silent", Level::Silent),
            ("ERROR", Level::Error),
            ("Warn", Level::Warn),
            ("info", Level::Info),
            (" debug ", Level::Debug),
        ] {
            assert_eq!(Level::parse(text), Some(level));
        }
        assert_eq!(Level::parse("trace"), None);
    }

    #[test]
    fn silent_suppresses_everything() {
        assert!(!Level::Silent.allows(Level::Error));
    }

    #[test]
    fn thresholds_follow_severity() {
        assert!(Level::Warn.allows(Level::Error));
        assert!(Level::Warn.allows(Level::Warn));
        assert!(!Level::Warn.allows(Level::Info));
        assert!(Level::Debug.allows(Level::Debug));
    }

    #[test]
    fn renders_text_and_json() {
        assert_eq!(render(Level::Info, "hello", false, 0.0), "[info] hello");
        assert_eq!(
            render(Level::Error, "a\"b\n", true, 12.5),
            "{\"level\":\"error\",\"msg\":\"a\\\"b\\n\",\"ts\":12.5}"
        );
    }
}

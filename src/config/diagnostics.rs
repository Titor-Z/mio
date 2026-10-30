//! 配置诊断：未知字段检测与相近键建议。
//!
//! 启用 `serde` 的 `deny_unknown_fields` 会让配置文件在字段拼写错误时直接失败，
//! 而新增字段又会让旧版本报错，两种情况都不合适。
//!
//! 采用「宽容解析 + 主动告警」：字段拼错时仍能运行，但打印
//! 「未知字段 + 最接近的合法字段」提示，把错误发现时间从运行期提前到启动期。

use serde_json::Value;

/// 一条配置诊断。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// 出现问题的字段路径，例如 `compilerOptions.targett`。
    pub path: String,
    /// 诊断级别，本版本只产出 `warn`。
    pub level: DiagnosticLevel,
    /// 面向使用者的说明文本。
    pub message: String,
}

/// 诊断级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticLevel {
    /// 提示性告警：不影响运行，但用户很可能写错了。
    Warn,
}

impl Diagnostic {
    /// 构造一条告警。
    pub fn warn(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            level: DiagnosticLevel::Warn,
            message: message.into(),
        }
    }

    /// 渲染为单行输出。
    pub fn render(&self) -> String {
        format!("{}: {}：{}", crate::LANG_TAG, self.path, self.message)
    }
}

/// 已知的顶层字段名集合。新增顶层字段时必须同步维护此表。
pub const KNOWN_TOP_LEVEL_KEYS: &[&str] = &[
    "$schema",
    "extends",
    "compilerOptions",
    "imports",
    "scopes",
    "include",
    "exclude",
    "files",
    "stdlib",
    "module",
    "check",
    "run",
    "build",
    "log",
    "tasks",
    "test",
];

/// 已知的 `compilerOptions` 字段名集合。
pub const KNOWN_COMPILER_OPTION_KEYS: &[&str] = &[
    "target",
    "module",
    "moduleResolution",
    "jsx",
    "jsxFactory",
    "useDefineForClassFields",
    "lib",
    "strict",
    "sourceMap",
    "paths",
    "baseUrl",
];

/// 检查顶层字段，产出未知字段告警。
pub fn check_top_level(value: &Value) -> Vec<Diagnostic> {
    let Some(object) = value.as_object() else {
        return vec![Diagnostic::warn("<root>", "配置文件根节点必须是对象")];
    };

    object
        .keys()
        .filter(|key| !KNOWN_TOP_LEVEL_KEYS.contains(&key.as_str()))
        .map(|key| {
            let suggestion = closest_key(key, KNOWN_TOP_LEVEL_KEYS);
            Diagnostic::warn(key, unknown_message("顶层字段", suggestion.as_deref()))
        })
        .collect()
}

/// 检查 `compilerOptions` 字段，产出未知字段告警。
pub fn check_compiler_options(value: &Value) -> Vec<Diagnostic> {
    let Some(options) = value.get("compilerOptions").and_then(Value::as_object) else {
        return Vec::new();
    };

    options
        .keys()
        .filter(|key| !KNOWN_COMPILER_OPTION_KEYS.contains(&key.as_str()))
        .map(|key| {
            let suggestion = closest_key(key, KNOWN_COMPILER_OPTION_KEYS);
            let path = format!("compilerOptions.{key}");
            Diagnostic::warn(path, unknown_message("编译选项", suggestion.as_deref()))
        })
        .collect()
}

/// 组装未知字段的说明文本。
fn unknown_message(kind: &str, suggestion: Option<&str>) -> String {
    match suggestion {
        Some(candidate) => format!("{kind}拼写可能有误，是否想写 {candidate}"),
        None => format!("{kind}不受支持，将被忽略"),
    }
}

/// 在候选集合中查找编辑距离最近且不超过阈值的键。
fn closest_key(input: &str, candidates: &[&str]) -> Option<String> {
    let mut best: Option<(usize, &str)> = None;

    for candidate in candidates {
        let distance = edit_distance(&input.to_lowercase(), &candidate.to_lowercase());
        let closer = best.is_none_or(|(current, _)| distance < current);
        if closer {
            best = Some((distance, candidate));
        }
    }

    let threshold = 3;
    best.filter(|(distance, _)| *distance <= threshold)
        .map(|(_, candidate)| candidate.to_string())
}

/// 计算两个字符串的编辑距离（只使用插入、删除、替换三种操作）。
fn edit_distance(left: &str, right: &str) -> usize {
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();

    let mut previous: Vec<usize> = (0..=right_chars.len()).collect();
    let mut current = vec![0usize; right_chars.len() + 1];

    for (row, left_char) in left_chars.iter().enumerate() {
        current[0] = row + 1;
        for (column, right_char) in right_chars.iter().enumerate() {
            let substitution = previous[column] + usize::from(left_char != right_char);
            let deletion = previous[column + 1] + 1;
            let insertion = current[column] + 1;
            current[column + 1] = substitution.min(deletion).min(insertion);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[right_chars.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_unknown_top_level_key_with_suggestion() {
        let value = serde_json::json!({ "task": {}, "include": [] });
        let diagnostics = check_top_level(&value);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].message.contains("tasks"));
    }

    #[test]
    fn accepts_known_keys() {
        let value = serde_json::json!({ "$schema": "", "extends": [], "imports": {} });
        assert!(check_top_level(&value).is_empty());
    }

    #[test]
    fn flags_unknown_compiler_option() {
        let value = serde_json::json!({ "compilerOptions": { "targett": "es2025" } });
        let diagnostics = check_compiler_options(&value);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].path, "compilerOptions.targett");
        assert!(diagnostics[0].message.contains("target"));
    }

    #[test]
    fn reports_root_type_error() {
        let value = serde_json::json!([1, 2]);
        let diagnostics = check_top_level(&value);
        assert_eq!(diagnostics[0].path, "<root>");
    }

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("target", "target"), 0);
        assert_eq!(edit_distance("targett", "target"), 1);
        assert_eq!(edit_distance("", "abc"), 3);
    }
}

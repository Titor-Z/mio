//! 错误文本中的栈帧坐标重写：把 `at … (path:line:col)` 从 JS 坐标改写为 TS 坐标。
//!
//! 纯字符串处理，不依赖引擎类型：引擎层在构造错误消息前把文本交进来即可。
//! 任何解析失败或映射未命中的行都原样保留，保证最坏情况等于现状。

use std::borrow::Cow;
use std::path::Path;

use super::registry::SourceMapRegistry;

/// 对多行错误文本逐行尝试重写栈帧坐标；无帧命中时返回原文本借用。
pub fn remap_stack<'a>(text: &'a str, registry: &SourceMapRegistry) -> Cow<'a, str> {
    if registry.is_empty() || !text.contains("at ") {
        return Cow::Borrowed(text);
    }
    let mut any = false;
    let lines: Vec<String> = text
        .lines()
        .map(|line| match remap_frame(line, registry) {
            Some(rewritten) => {
                any = true;
                rewritten
            }
            None => line.to_string(),
        })
        .collect();
    if any {
        Cow::Owned(lines.join("\n"))
    } else {
        Cow::Borrowed(text)
    }
}

/// 尝试重写单行栈帧；不是可重写的帧时返回 `None`（行原样保留）。
///
/// 支持两种形态（Boa 的实际产出为前者，后者按 V8 惯例容许）：
/// `at <name> (<path>:<line>:<col>)` 与 `at <path>:<line>:<col>`。
/// 坐标一律从右侧切分，路径含冒号（如 Windows 盘符）不受影响。
fn remap_frame(line: &str, registry: &SourceMapRegistry) -> Option<String> {
    let indent = &line[..line.len() - line.trim_start().len()];
    let rest = line.trim_start().strip_prefix("at ")?;

    let (name, location, wrapped) = match rest.strip_suffix(')') {
        Some(inner) => {
            let open = inner.find('(')?;
            (&inner[..open], &inner[open + 1..], true)
        }
        None => ("", rest, false),
    };

    // 右侧两段必须是纯数字行列，否则视为 native/未知帧。
    let (path_and_line, column_text) = location.rsplit_once(':')?;
    let (path_text, line_text) = path_and_line.rsplit_once(':')?;
    let column: u32 = column_text.parse().ok()?;
    let number: u32 = line_text.parse().ok()?;

    let (ts_line, ts_column) = registry.remap(Path::new(path_text), number, column)?;
    let name = name.trim_end();
    Some(if wrapped {
        format!("{indent}at {name} ({path_text}:{ts_line}:{ts_column})")
    } else {
        format!("{indent}at {path_text}:{ts_line}:{ts_column}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sourcemap::registry::SourceMapEntry;
    use std::path::PathBuf;

    fn register(registry: &mut SourceMapRegistry, path: &str, mappings: &str) {
        registry.register(
            Path::new(path),
            SourceMapEntry {
                source_path: PathBuf::from(path),
                mapping: format!(r#"{{ "mappings": "{mappings}" }}"#),
            },
        );
    }

    #[test]
    fn rewrites_named_frame_with_parentheses() {
        // 映射 `;AAAA`：第 2 行任意列都回到源 (0,0) → 1-based (1,1)。
        let mut registry = SourceMapRegistry::default();
        register(&mut registry, "/app/a.ts", ";AAAA");
        let frame = "    at boom (/app/a.ts:2:5)";
        assert_eq!(
            remap_stack(frame, &registry).as_ref(),
            "    at boom (/app/a.ts:1:1)"
        );
    }

    #[test]
    fn leaves_native_and_unknown_frames_untouched() {
        let mut registry = SourceMapRegistry::default();
        register(&mut registry, "/app/a.ts", ";;AAAA");
        let text = "Error: kaboom (native)\n    at  (native)\n    at f (/app/other.ts:3:8)\n    at g (/app/a.ts:3:2)";
        let remapped = remap_stack(text, &registry);
        assert!(
            remapped.starts_with("Error: kaboom (native)"),
            "首行不应变化：{remapped}"
        );
        assert!(
            remapped.contains("at  (native)"),
            "native 帧原样：{remapped}"
        );
        assert!(
            remapped.contains("/app/other.ts:3:8"),
            "未登记路径原样：{remapped}"
        );
        assert!(
            remapped.contains("at g (/app/a.ts:1:1)"),
            "命中帧应改写为 TS 坐标：{remapped}"
        );
    }

    #[test]
    fn empty_registry_returns_borrowed_text() {
        let registry = SourceMapRegistry::default();
        let text = "    at boom (/app/a.ts:3:8)";
        assert!(matches!(remap_stack(text, &registry), Cow::Borrowed(_)));
    }

    #[test]
    fn handles_drive_colon_and_bare_location() {
        let mut registry = SourceMapRegistry::default();
        register(&mut registry, "C:\\app\\a.ts", "AAAA");
        // 无括号形态 + 盘符冒号：坐标仍从右侧切分，路径完整保留。
        let frame = "    at C:\\app\\a.ts:1:3";
        assert_eq!(
            remap_stack(frame, &registry).as_ref(),
            "    at C:\\app\\a.ts:1:1"
        );
    }
}

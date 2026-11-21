//! 构建错误：带阶段、位置与源码上下文的结构化错误。
//!
//! 诊断精度分两档（有意为之，见 docs/Taolun.md TL20261122T022000）：
//! - lower 阶段：错误发生在 AST 转换期，手里有 OXC 的 `Span`，可精确到行列；
//! - check 阶段：TIR 不带 span（不建独立 IR 是明确记录的临时偏离），错误定位到
//!   「函数声明行」（`Func.line`）；codegen/emit/link 期错误无位置信息。
//!
//! 渲染为 rustc 风格：首行消息，次行 ` --> path:line:col`，随后源码行与插入符。

use std::fmt;

/// 出错阶段。用户看到它可以立刻判断「是我的源码问题」还是「工具链问题」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// AST → TIR 降低期（语法、不支持的形态、类型标注缺失）。
    Lower,
    /// 静态类型检查期（无隐式转换、`as` 白名单、签名匹配）。
    Check,
    /// TIR → Cranelift IR 生成期（变量重复声明、break/continue 语境等）。
    Codegen,
    /// 目标文件写盘期。
    Emit,
    /// `cc` 链接期（多见于宿主环境问题）。
    Link,
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Lower => "降低",
            Self::Check => "类型检查",
            Self::Codegen => "代码生成",
            Self::Emit => "写盘",
            Self::Link => "链接",
        };
        write!(f, "{name}")
    }
}

/// 一次构建的失败描述。
///
/// `line` / `column` 为 1 起始；两者均为 0 表示「无位置信息」（codegen/emit/link 期错误）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildError {
    /// 出错阶段。
    pub stage: Stage,
    /// 入口源码路径（渲染用）。
    pub path: String,
    /// 1 起始的行号；0 表示无位置。
    pub line: u32,
    /// 1 起始的列号（按字符计）；0 表示无位置。
    pub column: u32,
    /// 面向用户的消息（不含位置信息，渲染时拼装）。
    pub message: String,
}

impl BuildError {
    /// 无位置信息的错误（codegen/emit/link 期，或下游传来的字符串错误）。
    #[must_use]
    pub fn new(stage: Stage, path: &str, message: String) -> Self {
        Self {
            stage,
            path: String::from(path),
            line: 0,
            column: 0,
            message,
        }
    }

    /// 由字节偏移换算行列（1 起始；列按字符计，中文源不会错位）。
    #[must_use]
    pub fn at_offset(stage: Stage, path: &str, src: &str, offset: usize, message: String) -> Self {
        let (line, column) = line_col(src, offset);
        Self {
            stage,
            path: String::from(path),
            line,
            column,
            message,
        }
    }

    /// 渲染为 rustc 风格的多行诊断块；`src` 为 `None` 或无位置信息时省略源码行。
    ///
    /// 位置块示例：
    /// ```text
    /// error: 变量 s 必须有类型标注
    ///  --> app.ts:3:14
    ///   |
    /// 3 |   const s = "hi";
    ///   |              ^
    /// ```
    #[must_use]
    pub fn render(&self, src: Option<&str>) -> String {
        let mut out = format!("error: {}\n", self.message);
        if self.line > 0 {
            out.push_str(&format!(
                " --> {}:{}:{}\n",
                self.path, self.line, self.column
            ));
            if let Some(source_line) = src.and_then(|s| line_at(s, self.line)) {
                let gutter = format!("{}", self.line);
                let pad = " ".repeat(gutter.len());
                out.push_str(&format!("{pad} |\n"));
                out.push_str(&format!("{gutter} | {source_line}\n"));
                let caret_pad = " ".repeat(pad.len() + 3 + self.column.saturating_sub(1) as usize);
                out.push_str(&format!("{caret_pad}^\n"));
            }
        }
        // 去掉渲染块末尾换行，交给调用方决定如何收尾。
        out.trim_end().to_string()
    }
}

impl fmt::Display for BuildError {
    /// 单行形态（日志与紧凑输出）：`[阶段] message (path:line:col)`。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.line > 0 {
            write!(
                f,
                "[{}] {} ({}:{}:{})",
                self.stage, self.message, self.path, self.line, self.column
            )
        } else {
            write!(f, "[{}] {}", self.stage, self.message)
        }
    }
}

impl std::error::Error for BuildError {}

/// 把字节偏移换算为 1 起始的 (行, 列)；列按字符计数。
fn line_col(src: &str, offset: usize) -> (u32, u32) {
    let offset = offset.min(src.len());
    let mut line = 1u32;
    let mut line_start = 0usize;
    for (i, ch) in src.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            line_start = i + 1;
        }
    }
    let column = src[line_start..offset].chars().count() as u32 + 1;
    (line, column)
}

/// 取第 `line` 行（1 起始）的文本（不含换行符）；越界返回 `None`。
fn line_at(src: &str, line: u32) -> Option<&str> {
    src.lines().nth(line.saturating_sub(1) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_col_counts_from_one_and_handles_multibyte() {
        let src = "fn() {\n  let 中文 = 1;\n}\n";
        // 「中」前有 13 个字节（`fn() {\n` 7 字节 + `  let ` 6 字节），位于第 2 行第 7 个字符。
        assert_eq!(line_col(src, 13), (2, 7));
        assert_eq!(line_col(src, 0), (1, 1));
        // 偏移在「中」前一格（第 2 行第 6 字符）。
        assert_eq!(line_col(src, 12), (2, 6));
    }

    #[test]
    fn render_shows_source_line_and_caret() {
        let err = BuildError {
            stage: Stage::Lower,
            path: String::from("app.ts"),
            line: 2,
            column: 11,
            message: String::from("变量 x 必须有类型标注"),
        };
        let src = "function f(): void {\n  let x = 1;\n}\n";
        let out = err.render(Some(src));
        assert!(out.contains("error: 变量 x 必须有类型标注"), "{out}");
        assert!(out.contains(" --> app.ts:2:11"), "{out}");
        assert!(out.contains("2 |   let x = 1;"), "{out}");
        // 插入符对齐第 11 列。
        assert!(out.contains("           ^"), "{out}");
    }

    #[test]
    fn render_without_position_omits_location_block() {
        let err = BuildError::new(Stage::Link, "app.ts", String::from("cc 链接失败"));
        assert_eq!(err.render(None), "error: cc 链接失败");
    }
}

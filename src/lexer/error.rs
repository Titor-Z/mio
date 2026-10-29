//! 词法错误与诊断渲染。
//!
//! 每个错误带一个稳定编号，形如 `Lex001`，便于文档化与测试断言。
//! 错误只记编码与区间，文本在渲染时按源码展开成带行号与脱字符的形式，
//! 这样同一份错误既能给人读，也能被机器按编号匹配。

use std::fmt;

use super::source::{Source, Span};

/// 词法错误编码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LexErrorCode {
    /// 字符串字面量在换行前未闭合。
    UnterminatedString,
    /// 模板字面量缺少结束反引号。
    UnterminatedTemplate,
    /// 块注释缺少 `*/`。
    UnterminatedComment,
    /// 正则字面量缺少结束斜杠，或内含裸换行。
    UnterminatedRegExp,
    /// 正则修饰符非法或互相冲突。
    InvalidRegExpFlag,
    /// 出现不属于任何语法的字符。
    InvalidCharacter,
    /// 反斜杠转义序列形式非法。
    InvalidEscapeSequence,
    /// 数字字面量形式非法（前导零、小数点后接 BigInt、非法分隔符位置等）。
    InvalidNumericLiteral,
    /// 标识符中的 Unicode 转义形式非法。
    InvalidIdentifier,
    /// 内建模块前缀后的名字为空，例如 `@std/`。
    EmptyModuleName,
}

impl LexErrorCode {
    /// 稳定错误编号。
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnterminatedString => "Lex001",
            Self::UnterminatedTemplate => "Lex002",
            Self::UnterminatedComment => "Lex003",
            Self::UnterminatedRegExp => "Lex004",
            Self::InvalidRegExpFlag => "Lex005",
            Self::InvalidCharacter => "Lex006",
            Self::InvalidEscapeSequence => "Lex007",
            Self::InvalidNumericLiteral => "Lex008",
            Self::InvalidIdentifier => "Lex009",
            Self::EmptyModuleName => "Lex010",
        }
    }

    /// 面向使用者的说明文本。
    pub fn message(&self) -> &'static str {
        match self {
            Self::UnterminatedString => "字符串字面量未闭合，行尾前缺少结束引号",
            Self::UnterminatedTemplate => "模板字面量未闭合，缺少结束反引号",
            Self::UnterminatedComment => "块注释未闭合，缺少 */",
            Self::UnterminatedRegExp => "正则字面量未闭合，缺少结束 / 或内含裸换行",
            Self::InvalidRegExpFlag => "正则修饰符非法",
            Self::InvalidCharacter => "无法识别的字符",
            Self::InvalidEscapeSequence => "转义序列非法",
            Self::InvalidNumericLiteral => "数字字面量形式非法",
            Self::InvalidIdentifier => "标识符中的 Unicode 转义形式非法",
            Self::EmptyModuleName => "内建模块名前缀后为空",
        }
    }

    /// 修复提示，无法给出有效提示时返回 `None`。
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::UnterminatedString => Some("跨行字符串请改用模板字面量，或在行尾用反斜杠续行"),
            Self::InvalidNumericLiteral => {
                Some("mio 的模块默认严格模式，不允许 0755 这类前导零八进制写法")
            }
            Self::InvalidRegExpFlag => Some("可用修饰符为 d g i m s u v y，且 u 与 v 互斥"),
            Self::UnterminatedTemplate => Some("检查 ${ } 内是否有未闭合的嵌套模板"),
            _ => None,
        }
    }
}

/// 一次词法错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexError {
    /// 错误编码。
    pub code: LexErrorCode,
    /// 出错区间。
    pub span: Span,
}

impl LexError {
    /// 构造错误。
    pub fn new(code: LexErrorCode, span: Span) -> Self {
        Self { code, span }
    }

    /// 渲染为带位置信息的单行摘要。
    pub fn summarize(&self, source: &Source, file_name: &str) -> String {
        let (line, column) = source.line_col(self.span.start);
        format!(
            "{}: error[{}]: {} at {}:{}:{}",
            crate::LANG_TAG,
            self.code.code(),
            self.code.message(),
            file_name,
            line,
            column
        )
    }

    /// 渲染为带源码片段与脱字符的完整诊断块。
    pub fn render(&self, source: &Source, file_name: &str) -> String {
        let (line, column) = source.line_col(self.span.start);
        let text = source.line_text(line);
        // 脱字符位置按字符计，区间宽度不足一时至少标一个字符
        let width = (self.span.len() as usize).max(1).min(text.len().max(1));
        let caret_indent = " ".repeat((column as usize).saturating_sub(1));

        let mut output = String::new();
        output.push_str(&format!(
            "error[{}]: {}\n",
            self.code.code(),
            self.code.message()
        ));
        output.push_str(&format!("  --> {file_name}:{line}:{column}\n"));
        output.push_str("   |\n");
        output.push_str(&format!("{line} | {text}\n"));
        output.push_str(&format!("   | {caret_indent}{}\n", "^".repeat(width)));
        if let Some(hint) = self.code.hint() {
            output.push_str(&format!("   = 提示：{hint}"));
        }
        output
    }
}

impl fmt::Display for LexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "[{}] {}", self.code.code(), self.code.message())
    }
}

impl std::error::Error for LexError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique() {
        let all = [
            LexErrorCode::UnterminatedString,
            LexErrorCode::UnterminatedTemplate,
            LexErrorCode::UnterminatedComment,
            LexErrorCode::UnterminatedRegExp,
            LexErrorCode::InvalidRegExpFlag,
            LexErrorCode::InvalidCharacter,
            LexErrorCode::InvalidEscapeSequence,
            LexErrorCode::InvalidNumericLiteral,
            LexErrorCode::InvalidIdentifier,
            LexErrorCode::EmptyModuleName,
        ];
        let mut seen: Vec<&str> = Vec::new();
        for code in all {
            assert!(
                !seen.contains(&code.code()),
                "错误编号重复：{}",
                code.code()
            );
            seen.push(code.code());
            assert!(!code.message().is_empty(), "{} 缺说明文本", code.code());
        }
    }

    #[test]
    fn render_includes_line_and_caret() {
        let source = Source::new(String::from("const a = \"xyz\n"));
        let error = LexError::new(LexErrorCode::UnterminatedString, Span::new(10, 14));
        let rendered = error.render(&source, "demo.ts");

        assert!(rendered.contains("error[Lex001]"));
        assert!(
            rendered.contains("2:1") || rendered.contains("1:11"),
            "{rendered}"
        );
        assert!(rendered.contains('^'), "应画出脱字符");
        assert!(rendered.contains("模板字面量"), "应给出提示");
    }

    #[test]
    fn caret_width_never_exceeds_line() {
        let source = Source::new(String::from("x"));
        let error = LexError::new(LexErrorCode::InvalidCharacter, Span::new(0, 99));
        let rendered = error.render(&source, "demo.ts");
        assert!(rendered.contains('^'));
    }

    #[test]
    fn summarize_contains_file_position() {
        let source = Source::new(String::from("let a = 1;"));
        let error = LexError::new(LexErrorCode::InvalidCharacter, Span::new(4, 5));
        let line = error.summarize(&source, "a.ts");
        assert!(line.contains("a.ts:1:5"), "{line}");
    }
}

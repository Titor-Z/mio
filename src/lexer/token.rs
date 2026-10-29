//! token 分类体系。
//!
//! 词法单元分为十类。设计要点有三条。
//!
//! 一、TypeScript 不引入第二个词法模式。`type`、`as`、`satisfies`、`declare` 这类
//! TypeScript 关键字按 ECMAScript 规范仍是合法标识符，因此全部按普通标识符产出，
//! 由语法分析器在合适的位置按文本识别；只有真正的保留字才产出关键字 token。
//! 这是「JavaScript 是 TypeScript 真子集、单条文法路径」在词法层的落点。
//!
//! 二、换行信息不产 token。自动分号插入需要知道某个 token 之前是否有换行，
//! 用 `newline_before` 标记比产出独立换行 token 更省，也不会干扰取数窗口。
//!
//! 三、注释不产 token。注释作为源码映射与文档信息的素材单独收集。

use std::fmt;

use super::source::Span;

/// 词法单元类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    /// 标识符，同时容纳 TypeScript 软关键字。
    Identifier,
    /// `#` 开头的私有字段名。
    PrivateName,
    /// 数字字面量。
    Number {
        /// 是否带 `n` 后缀。
        bigint: bool,
    },
    /// 单引号或双引号字符串。
    String,
    /// 正则字面量，文本形如 `/ab+c/gi`。
    RegularExpression,
    /// 模板字面量的文本段。
    TemplateText,
    /// 模板字面量的分隔符。
    Template(TemplateDelimiter),
    /// ECMAScript 保留字。
    Keyword(Keyword),
    /// 运算符与界符。
    Punctuator(Punctuator),
    /// 输入结束。
    EndOfFile,
}

impl TokenKind {
    /// 该类别是否可充当名字（属性名、解构键、方法名等宽松位置）。
    pub fn is_identifier_like(&self) -> bool {
        matches!(self, Self::Identifier | Self::Keyword(_))
    }

    /// 该类别出现后，其后的 `/` 应视为除号而非正则起始。
    pub fn ends_expression(&self) -> bool {
        match self {
            Self::Identifier
            | Self::Number { .. }
            | Self::String
            | Self::RegularExpression
            | Self::PrivateName
            | Self::Template(TemplateDelimiter::Tail) => true,
            Self::Keyword(keyword) => keyword.terminates_expression(),
            Self::Punctuator(punctuator) => {
                matches!(
                    punctuator,
                    Punctuator::RightParen | Punctuator::RightBracket
                ) || punctuator.is_postfix()
            }
            _ => false,
        }
    }

    /// 产出人类可读类别名，用于「期望 A，实际 B」式报错。
    pub fn describe(&self) -> String {
        match self {
            Self::Identifier => String::from("标识符"),
            Self::PrivateName => String::from("私有名称"),
            Self::Number { bigint } => {
                if *bigint {
                    String::from("BigInt 字面量")
                } else {
                    String::from("数字字面量")
                }
            }
            Self::String => String::from("字符串字面量"),
            Self::RegularExpression => String::from("正则字面量"),
            Self::TemplateText => String::from("模板文本"),
            Self::Template(delimiter) => format!("模板分隔符 {}", delimiter.text()),
            Self::Keyword(keyword) => format!("关键字 {keyword}"),
            Self::Punctuator(punctuator) => format!("运算符 {}", punctuator.text()),
            Self::EndOfFile => String::from("文件结束"),
        }
    }
}

/// 模板字面量的四种分隔位置。
///
/// 拆成四个独立 token 而不是让语法分析器数花括号，是为了让 `${}` 的嵌套
/// 在 token 流里本身可判定：表达式部分正常递归，遇到 `Resume` 即知表达式结束。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateDelimiter {
    /// 起始反引号。
    Head,
    /// 表达式开启符 `${`。
    Substitution,
    /// 表达式回接符 `}`，仅当模板模式栈非空时产出。
    Resume,
    /// 结束反引号。
    Tail,
}

impl TemplateDelimiter {
    /// 对应的源文本。
    pub fn text(&self) -> &'static str {
        match self {
            Self::Head | Self::Tail => "`",
            Self::Substitution => "${",
            Self::Resume => "}",
        }
    }
}

/// ECMAScript 保留字，含严格模式与模块码保留字。
///
/// `enum`、`interface` 一类词在规范里属于「未来保留字」，严格模式下不可作标识符，
/// 而 mio 的一切代码都是模块（默认严格模式），故一并归入关键字。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    Break,
    Case,
    Catch,
    Class,
    Const,
    Continue,
    Debugger,
    Default,
    Delete,
    Do,
    Else,
    Enum,
    Export,
    Extends,
    False,
    Finally,
    For,
    Function,
    If,
    Import,
    In,
    Instanceof,
    New,
    Null,
    Return,
    Super,
    Switch,
    This,
    Throw,
    True,
    Try,
    Typeof,
    Var,
    Void,
    While,
    With,
    /// 严格模式保留。
    Let,
    /// 严格模式保留。
    Static,
    /// 严格模式保留。
    Yield,
    /// 模块码保留。
    Await,
}

impl Keyword {
    /// 全部保留字文本，供测试与文档生成使用。
    pub const WORDS: &[&str] = &[
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "let",
        "new",
        "null",
        "return",
        "static",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "var",
        "void",
        "await",
        "while",
        "with",
        "yield",
    ];

    /// 由单词文本查保留字，非保留字返回 `None`。
    pub fn from_word(word: &str) -> Option<Self> {
        let keyword = match word {
            "break" => Self::Break,
            "case" => Self::Case,
            "catch" => Self::Catch,
            "class" => Self::Class,
            "const" => Self::Const,
            "continue" => Self::Continue,
            "debugger" => Self::Debugger,
            "default" => Self::Default,
            "delete" => Self::Delete,
            "do" => Self::Do,
            "else" => Self::Else,
            "enum" => Self::Enum,
            "export" => Self::Export,
            "extends" => Self::Extends,
            "false" => Self::False,
            "finally" => Self::Finally,
            "for" => Self::For,
            "function" => Self::Function,
            "if" => Self::If,
            "import" => Self::Import,
            "in" => Self::In,
            "instanceof" => Self::Instanceof,
            "new" => Self::New,
            "null" => Self::Null,
            "return" => Self::Return,
            "super" => Self::Super,
            "switch" => Self::Switch,
            "this" => Self::This,
            "throw" => Self::Throw,
            "true" => Self::True,
            "try" => Self::Try,
            "typeof" => Self::Typeof,
            "var" => Self::Var,
            "void" => Self::Void,
            "while" => Self::While,
            "with" => Self::With,
            "let" => Self::Let,
            "static" => Self::Static,
            "yield" => Self::Yield,
            "await" => Self::Await,
            _ => return None,
        };
        Some(keyword)
    }

    /// 该关键字是否为字面量型表达式终点（`true`、`false`、`null`、`this`、`super`）。
    ///
    /// 这些词出现在表达式末尾，其后紧跟的 `/` 是除号而非正则起始。
    pub fn terminates_expression(&self) -> bool {
        matches!(
            self,
            Self::False | Self::Null | Self::Super | Self::This | Self::True
        )
    }

    /// 该关键字是否开启受限生产（其后不许有换行）。
    ///
    /// 自动分号插入规则里，`return`、`throw`、`break`、`continue` 的标签受限。
    pub fn is_restricted(&self) -> bool {
        matches!(
            self,
            Self::Break | Self::Continue | Self::Return | Self::Throw
        )
    }
}

impl fmt::Display for Keyword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let word = match self {
            Self::Break => "break",
            Self::Case => "case",
            Self::Catch => "catch",
            Self::Class => "class",
            Self::Const => "const",
            Self::Continue => "continue",
            Self::Debugger => "debugger",
            Self::Default => "default",
            Self::Delete => "delete",
            Self::Do => "do",
            Self::Else => "else",
            Self::Enum => "enum",
            Self::Export => "export",
            Self::Extends => "extends",
            Self::False => "false",
            Self::Finally => "finally",
            Self::For => "for",
            Self::Function => "function",
            Self::If => "if",
            Self::Import => "import",
            Self::In => "in",
            Self::Instanceof => "instanceof",
            Self::New => "new",
            Self::Null => "null",
            Self::Return => "return",
            Self::Super => "super",
            Self::Switch => "switch",
            Self::This => "this",
            Self::Throw => "throw",
            Self::True => "true",
            Self::Try => "try",
            Self::Typeof => "typeof",
            Self::Var => "var",
            Self::Void => "void",
            Self::While => "while",
            Self::With => "with",
            Self::Let => "let",
            Self::Static => "static",
            Self::Yield => "yield",
            Self::Await => "await",
        };
        formatter.write_str(word)
    }
}

/// 运算符与界符。
///
/// 变体按源文本含义命名，不按运算语义命名：词法层不知道 `*` 是乘法还是通配符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Punctuator {
    LeftParen,
    RightParen,
    LeftBrace,
    RightBrace,
    LeftBracket,
    RightBracket,
    Semicolon,
    Comma,
    Dot,
    /// `...` 展开或剩余参数。
    Ellipsis,
    Question,
    /// `?.` 可选链。
    OptionalChain,
    Colon,
    /// `=>` 箭头函数。
    Arrow,
    Plus,
    Minus,
    Star,
    /// `**` 幂运算。
    DoubleStar,
    Slash,
    Percent,
    PlusPlus,
    MinusMinus,
    BitAnd,
    DoubleAnd,
    BitOr,
    DoubleOr,
    BitXor,
    BitNot,
    Not,
    Equal,
    StrictEqual,
    NotEqual,
    StrictNotEqual,
    LessThan,
    GreaterThan,
    LessOrEqual,
    GreaterOrEqual,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    DoubleStarAssign,
    BitAndAssign,
    BitOrAssign,
    BitXorAssign,
    ShiftLeft,
    ShiftRight,
    UnsignedShiftRight,
    ShiftLeftAssign,
    ShiftRightAssign,
    UnsignedShiftRightAssign,
    DoubleAndAssign,
    DoubleOrAssign,
    /// `??` 空值合并。
    Coalesce,
    CoalesceAssign,
    /// `@` 装饰器前缀。
    At,
    /// 单独的 `#`，出现在 `#field in obj` 语法中。
    Hash,
}

impl Punctuator {
    /// 运算符文本表。条目顺序无关紧要，匹配逻辑按长度从长到短逐档尝试。
    pub const ORDERED: &[(&str, Self)] = &[
        (">>>=", Self::UnsignedShiftRightAssign),
        ("...", Self::Ellipsis),
        ("===", Self::StrictEqual),
        ("!==", Self::StrictNotEqual),
        ("**=", Self::DoubleStarAssign),
        ("<<=", Self::ShiftLeftAssign),
        (">>=", Self::ShiftRightAssign),
        ("&&=", Self::DoubleAndAssign),
        ("||=", Self::DoubleOrAssign),
        ("??=", Self::CoalesceAssign),
        (">>>", Self::UnsignedShiftRight),
        ("=>", Self::Arrow),
        ("==", Self::Equal),
        ("!=", Self::NotEqual),
        ("<=", Self::LessOrEqual),
        (">=", Self::GreaterOrEqual),
        ("&&", Self::DoubleAnd),
        ("||", Self::DoubleOr),
        ("??", Self::Coalesce),
        ("?.", Self::OptionalChain),
        ("++", Self::PlusPlus),
        ("--", Self::MinusMinus),
        ("+=", Self::PlusAssign),
        ("-=", Self::MinusAssign),
        ("*=", Self::StarAssign),
        ("/=", Self::SlashAssign),
        ("%=", Self::PercentAssign),
        ("&=", Self::BitAndAssign),
        ("|=", Self::BitOrAssign),
        ("^=", Self::BitXorAssign),
        ("**", Self::DoubleStar),
        ("<<", Self::ShiftLeft),
        (">>", Self::ShiftRight),
        ("(", Self::LeftParen),
        (")", Self::RightParen),
        ("{", Self::LeftBrace),
        ("}", Self::RightBrace),
        ("[", Self::LeftBracket),
        ("]", Self::RightBracket),
        (";", Self::Semicolon),
        (",", Self::Comma),
        (".", Self::Dot),
        (":", Self::Colon),
        ("?", Self::Question),
        ("+", Self::Plus),
        ("-", Self::Minus),
        ("*", Self::Star),
        ("/", Self::Slash),
        ("%", Self::Percent),
        ("&", Self::BitAnd),
        ("|", Self::BitOr),
        ("^", Self::BitXor),
        ("~", Self::BitNot),
        ("!", Self::Not),
        ("<", Self::LessThan),
        (">", Self::GreaterThan),
        ("=", Self::Assign),
        ("@", Self::At),
        ("#", Self::Hash),
    ];

    /// 表中最长运算符的字节长度，决定尝试次数上限。
    pub const MAX_LENGTH: usize = 4;

    /// 在文本前缀上做最长匹配，返回运算符类别与其字节长度。
    ///
    /// 从长到短逐档尝试，而不是依赖 `ORDERED` 的声明顺序。
    /// 靠顺序保证正确性太脆弱：表的自然演进方式是追加，
    /// 而追加者不会记住「`>>>=` 必须排在 `>>>` 之前」这类隐式约束。
    pub fn match_longest(text: &str) -> Option<(Self, usize)> {
        let mut length = Self::MAX_LENGTH.min(text.len());
        while length >= 1 {
            let hit = Self::ORDERED
                .iter()
                .find(|(candidate, _)| candidate.len() == length && text.starts_with(candidate));
            let matched = hit.map(|(_, punctuator)| (*punctuator, length));
            if matched.is_some() {
                return matched;
            }
            length -= 1;
        }
        None
    }

    /// 由运算符文本查类别。
    pub fn from_text(text: &str) -> Option<Self> {
        Self::ORDERED
            .iter()
            .find(|(candidate, _)| *candidate == text)
            .map(|(_, punctuator)| *punctuator)
    }

    /// 对应的源文本。
    pub fn text(&self) -> &'static str {
        match self {
            Self::LeftParen => "(",
            Self::RightParen => ")",
            Self::LeftBrace => "{",
            Self::RightBrace => "}",
            Self::LeftBracket => "[",
            Self::RightBracket => "]",
            Self::Semicolon => ";",
            Self::Comma => ",",
            Self::Dot => ".",
            Self::Ellipsis => "...",
            Self::Question => "?",
            Self::OptionalChain => "?.",
            Self::Colon => ":",
            Self::Arrow => "=>",
            Self::Plus => "+",
            Self::Minus => "-",
            Self::Star => "*",
            Self::DoubleStar => "**",
            Self::Slash => "/",
            Self::Percent => "%",
            Self::PlusPlus => "++",
            Self::MinusMinus => "--",
            Self::BitAnd => "&",
            Self::DoubleAnd => "&&",
            Self::BitOr => "|",
            Self::DoubleOr => "||",
            Self::BitXor => "^",
            Self::BitNot => "~",
            Self::Not => "!",
            Self::Equal => "==",
            Self::StrictEqual => "===",
            Self::NotEqual => "!=",
            Self::StrictNotEqual => "!==",
            Self::LessThan => "<",
            Self::GreaterThan => ">",
            Self::LessOrEqual => "<=",
            Self::GreaterOrEqual => ">=",
            Self::Assign => "=",
            Self::PlusAssign => "+=",
            Self::MinusAssign => "-=",
            Self::StarAssign => "*=",
            Self::SlashAssign => "/=",
            Self::PercentAssign => "%=",
            Self::DoubleStarAssign => "**=",
            Self::BitAndAssign => "&=",
            Self::BitOrAssign => "|=",
            Self::BitXorAssign => "^=",
            Self::ShiftLeft => "<<",
            Self::ShiftRight => ">>",
            Self::UnsignedShiftRight => ">>>",
            Self::ShiftLeftAssign => "<<=",
            Self::ShiftRightAssign => ">>=",
            Self::UnsignedShiftRightAssign => ">>>=",
            Self::DoubleAndAssign => "&&=",
            Self::DoubleOrAssign => "||=",
            Self::Coalesce => "??",
            Self::CoalesceAssign => "??=",
            Self::At => "@",
            Self::Hash => "#",
        }
    }

    /// 是否为后缀自增/自减，其后同样不许再接正则。
    pub fn is_postfix(&self) -> bool {
        matches!(self, Self::PlusPlus | Self::MinusMinus)
    }
}

impl fmt::Display for Punctuator {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.text())
    }
}

/// 一个词法单元。
///
/// `text` 是指向源码的切片，不做拷贝；需要数值或字符串真值的场合，
/// 由语法分析器按需调用转换函数，避免在 token 流里堆积临时对象。
#[derive(Debug, Clone, Copy)]
pub struct Token<'a> {
    /// token 类别。
    pub kind: TokenKind,
    /// token 在源码中的位置。
    pub span: Span,
    /// token 的原始文本。
    pub text: &'a str,
    /// token 之前是否出现过换行，自动分号插入的判据。
    pub newline_before: bool,
}

impl<'a> Token<'a> {
    /// 构造 token。
    pub fn new(kind: TokenKind, span: Span, text: &'a str, newline_before: bool) -> Self {
        Self {
            kind,
            span,
            text,
            newline_before,
        }
    }

    /// 是否为指定运算符。
    pub fn is(&self, punctuator: Punctuator) -> bool {
        self.kind == TokenKind::Punctuator(punctuator)
    }

    /// 是否为指定关键字。
    pub fn is_keyword(&self, keyword: Keyword) -> bool {
        self.kind == TokenKind::Keyword(keyword)
    }

    /// 取可作名字使用的文本。
    pub fn name(&self) -> Option<&'a str> {
        match self.kind {
            TokenKind::Identifier | TokenKind::Keyword(_) | TokenKind::PrivateName => {
                Some(self.text)
            }
            _ => None,
        }
    }

    /// 产出一行人类可读描述，供语法错误信息引用。
    pub fn describe(&self) -> String {
        format!("{}「{}」", self.kind.describe(), self.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_reserved_words() {
        assert_eq!(Keyword::from_word("function"), Some(Keyword::Function));
        assert_eq!(Keyword::from_word("let"), Some(Keyword::Let));
        assert_eq!(Keyword::from_word("await"), Some(Keyword::Await));
        assert_eq!(Keyword::from_word("functionx"), None);
    }

    #[test]
    fn every_listed_word_round_trips() {
        for word in Keyword::WORDS {
            let found = Keyword::from_word(word).unwrap_or_else(|| panic!("{word} 未登记"));
            assert_eq!(found.to_string(), *word, "{word} 的 Display 与文本不一致");
        }
    }

    #[test]
    fn leaves_typescript_soft_keywords_as_identifiers() {
        // 这些词按规范仍是合法标识符，不该进保留字表
        for word in [
            "type",
            "as",
            "satisfies",
            "declare",
            "namespace",
            "readonly",
            "infer",
        ] {
            assert_eq!(Keyword::from_word(word), None, "{word} 不该是保留字");
        }
    }

    #[test]
    fn operator_text_round_trips() {
        for (text, punctuator) in Punctuator::ORDERED {
            assert_eq!(punctuator.text(), *text, "{punctuator:?} 文本不一致");
            assert_eq!(Punctuator::from_text(text), Some(*punctuator));
        }
    }

    #[test]
    fn table_has_no_duplicate_operator() {
        let mut seen: Vec<&str> = Vec::new();
        for (text, _) in Punctuator::ORDERED {
            assert!(!seen.contains(text), "运算符 {text} 重复登记");
            seen.push(text);
        }
    }

    #[test]
    fn longest_match_is_independent_of_table_order() {
        assert_eq!(
            Punctuator::match_longest(">>>=1"),
            Some((Punctuator::UnsignedShiftRightAssign, 4))
        );
        assert_eq!(
            Punctuator::match_longest(">>>x"),
            Some((Punctuator::UnsignedShiftRight, 3))
        );
        assert_eq!(
            Punctuator::match_longest(">>x"),
            Some((Punctuator::ShiftRight, 2))
        );
        assert_eq!(
            Punctuator::match_longest("?.5"),
            Some((Punctuator::OptionalChain, 2))
        );
        // `<>` 整体不是运算符，但最长前缀 `<` 命中 LessThan
        assert_eq!(
            Punctuator::match_longest("<>"),
            Some((Punctuator::LessThan, 1))
        );
        // 只剩一个字符时也必须命中
        assert_eq!(
            Punctuator::match_longest(")"),
            Some((Punctuator::RightParen, 1))
        );
    }

    #[test]
    fn expression_ending_tokens_mark_division_context() {
        assert!(TokenKind::Number { bigint: false }.ends_expression());
        assert!(TokenKind::Keyword(Keyword::This).ends_expression());
        assert!(TokenKind::Punctuator(Punctuator::RightParen).ends_expression());
        assert!(TokenKind::Punctuator(Punctuator::PlusPlus).ends_expression());
        // 标识符是表达式终点，其后的 `/` 是除号：`a / b / c` 应判为两次除法
        assert!(TokenKind::Identifier.ends_expression());
        // 运算符之后期待表达式，因此 `/` 是正则起始
        assert!(!TokenKind::Punctuator(Punctuator::Assign).ends_expression());
        assert!(!TokenKind::Keyword(Keyword::Return).ends_expression());
    }

    #[test]
    fn restricted_keywords_cover_asi_cases() {
        assert!(Keyword::Return.is_restricted());
        assert!(!Keyword::Const.is_restricted());
    }

    #[test]
    fn template_delimiters_report_source_text() {
        assert_eq!(TemplateDelimiter::Head.text(), "`");
        assert_eq!(TemplateDelimiter::Substitution.text(), "${");
        assert_eq!(TemplateDelimiter::Resume.text(), "}");
    }

    #[test]
    fn token_name_accepts_keywords_in_loose_positions() {
        let token = Token::new(
            TokenKind::Keyword(Keyword::Let),
            Span::new(0, 3),
            "let",
            false,
        );
        assert_eq!(token.name(), Some("let"));
        assert!(token.kind.is_identifier_like());
    }
}

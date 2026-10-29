//! 词法分析器（阶段 B：自研前端第一层）。
//!
//! 一次线性扫描把源码切分为 token 流，不做二次遍历。子模块各司其职：
//! - [`source`]：源码文本与行列索引，token 文本以其切片形式零拷贝引用。
//! - [`token`]：token 分类体系与保留字、运算符表。
//! - [`literal`]：字面量真值转换（转义、数字、正则修饰符）。
//! - [`error`]：词法错误编码与带脱字符的诊断渲染。
//! - [`scanner`]：扫描器主体与注释收集。
//!
//! 设计上不引入第二个词法模式：TypeScript 软关键字（`type`、`as` 等）按规范仍是
//! 合法标识符，一律产出 [`TokenKind::Identifier`]，由语法分析器在合适位置按文本识别，
//! 这是「JavaScript 是 TypeScript 真子集、单条文法路径」在词法层的落点。

mod error;
mod literal;
mod scanner;
mod source;
mod token;

pub use error::{LexError, LexErrorCode};
pub use literal::{
    LiteralError, check_regex_flags, cook_string, cook_template_text, decode_identifier,
    number_value,
};
pub use scanner::{Comment, CommentKind, Lexer};
pub use source::{Source, Span};
pub use token::{Keyword, Punctuator, TemplateDelimiter, Token, TokenKind};

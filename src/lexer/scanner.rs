//! 词法分析器。
//!
//! 手写扫描器，一次线性扫描产出全部 token，不做二次遍历。三条主要难点在实现里各有交代：
//!
//! 一、除号与正则的歧义。ECMAScript 的词法依赖语法上下文：`a / b / c` 是两次除法，
//! 而 `if (x) /ab/.test(y)` 里第一个 `/` 开启正则。这里用「前一个 token 是否为表达式终点」
//! 做启发式判定，并暴露 `set_regex_allowed` 供语法分析器在右括号后回退重扫。
//!
//! 二、模板字面量的嵌套。`${}` 内部可以出现对象字面量、函数体，甚至嵌套另一层模板，
//! 因此用模式栈而不是括号计数器，`{` 与 `}` 在替换模式里累加深度，深度归零的那个 `}`
//! 才作为模板回接符产出。
//!
//! 三、注释与换行不占用 token 流。注释单独收集供源码映射与文档取用，
//! 换行以 `newline_before` 标记附着在 token 上，供自动分号插入使用。

use super::error::{LexError, LexErrorCode};
use super::source::{Source, Span};
use super::token::{Keyword, Punctuator, TemplateDelimiter, Token, TokenKind};

/// 扫描模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 普通表达式模式。
    Expression,
    /// 位于模板字面量的文本段中。
    Template,
    /// 位于 `${` 与对应 `}` 之间的表达式中，`depth` 为该区间内未闭合的 `{` 数量。
    Substitution { depth: usize },
}

/// 注释形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommentKind {
    /// `//` 到行尾。
    Line,
    /// `/*` 到 `*/`。
    Block,
}

/// 一条注释。
#[derive(Debug, Clone, Copy)]
pub struct Comment<'a> {
    /// 注释形态。
    pub kind: CommentKind,
    /// 注释在源码中的完整区间，含标记本身。
    pub span: Span,
    /// 去掉标记后的注释正文。
    pub text: &'a str,
}

impl Comment<'_> {
    /// 是否为文档注释 `/** ... */`。
    ///
    /// 阶段 B 的语义分析会用它承载导出符号说明，因此这里就把它识别出来。
    pub fn is_doc(&self) -> bool {
        self.kind == CommentKind::Block && self.text.starts_with('*') && self.text != "*"
    }
}

/// 词法分析器。
pub struct Lexer<'a> {
    /// 待分析源码。
    source: &'a Source,
    /// 当前扫描位置，字节偏移。
    position: usize,
    /// 模式栈，栈底恒为表达式模式。
    modes: Vec<Mode>,
    /// 上一个产出的 token 类别，用于除号/正则判定。
    previous: Option<TokenKind>,
    /// 已扫过换行但尚未附着到 token 的状态。
    newline_pending: bool,
    /// 语法分析器对下一个 token 是否为正则的强制覆盖，只对紧随其后的一个 token 生效。
    regex_override: Option<bool>,
    /// 收集到的注释。
    comments: Vec<Comment<'a>>,
}

impl<'a> Lexer<'a> {
    /// 从源码构造扫描器。
    pub fn new(source: &'a Source) -> Self {
        Self {
            source,
            position: 0,
            modes: vec![Mode::Expression],
            previous: None,
            newline_pending: false,
            regex_override: None,
            comments: Vec::new(),
        }
    }

    /// 已收集的注释。
    pub fn comments(&self) -> &[Comment<'a>] {
        &self.comments
    }

    /// 强制下一个 token 按正则（`true`）或按除号（`false`）扫描。
    ///
    /// 存在的原因是启发式无法独自解决右括号歧义：
    /// `(a) / b` 里 `/` 是除号，`if (x) /re/.test(y)` 里 `/` 开启正则。
    /// 语法分析器知道自己此刻是否处在表达式起始位置，故由它来裁定。
    pub fn set_regex_allowed(&mut self, allowed: bool) {
        self.regex_override = Some(allowed);
    }

    /// 取下一个 token。
    pub fn next_token(&mut self) -> Result<Token<'a>, LexError> {
        self.skip_trivia()?;

        let newline_before = self.newline_pending;
        self.newline_pending = false;
        let start = self.position;

        let Some(ch) = self.peek() else {
            return Ok(self.make(TokenKind::EndOfFile, start, start, newline_before));
        };

        let token = self.scan(ch, start, newline_before)?;
        self.previous = Some(token.kind);
        self.regex_override = None;
        Ok(token)
    }

    /// 一次性扫描到输入结束，返回全部 token 与全部错误。
    ///
    /// 逐个 token 抛错会让使用者改一处跑一次，故这里做容错：
    /// 记录错误、强制推进至少一个字符、继续扫描，一次给出所有问题。
    pub fn collect(mut self) -> (Vec<Token<'a>>, Vec<LexError>) {
        let mut tokens = Vec::new();
        let mut errors = Vec::new();

        loop {
            let before = self.position;
            match self.next_token() {
                Ok(token) => {
                    let finished = token.kind == TokenKind::EndOfFile;
                    tokens.push(token);
                    if finished {
                        break;
                    }
                }
                Err(error) => {
                    errors.push(error);
                    let stalled = self.position == before;
                    if stalled {
                        self.advance_one();
                    }
                    if self.peek().is_none() {
                        break;
                    }
                }
            }
        }

        (tokens, errors)
    }

    // ---- 单字符与区间读写 ----

    /// 源码全文。
    fn text(&self) -> &'a str {
        self.source.text()
    }

    /// 指定字节偏移处的字符，越界返回 `None`。
    fn char_at(&self, index: usize) -> Option<char> {
        self.text()
            .get(index..)
            .and_then(|slice| slice.chars().next())
    }

    /// 当前字符。
    fn peek(&self) -> Option<char> {
        self.char_at(self.position)
    }

    /// 当前位置之后是否紧接一个 ASCII 字符。
    ///
    /// 只在偏移量为单字节前缀时使用（`?.`、`#x`、`.` 等），故偏移即字节偏移。
    fn char_ahead(&self, offset: usize) -> Option<char> {
        self.char_at(self.position + offset)
    }

    /// 消费一个字符。
    fn advance(&mut self, ch: char) {
        self.position += ch.len_utf8();
    }

    /// 按字节偏移前进一个字符。
    fn advance_one(&mut self) {
        if let Some(ch) = self.peek() {
            self.advance(ch);
        }
    }

    /// 构造 token。
    fn make(&self, kind: TokenKind, start: usize, end: usize, newline_before: bool) -> Token<'a> {
        Token::new(
            kind,
            Span::new(start as u32, end as u32),
            &self.text()[start..end],
            newline_before,
        )
    }

    /// 构造错误。
    fn fail(&self, code: LexErrorCode, start: usize, end: usize) -> LexError {
        LexError::new(code, Span::new(start as u32, end as u32))
    }

    // ---- 空白与注释 ----

    /// 跳过空白、行终止符与注释，其间累积换行标记。
    fn skip_trivia(&mut self) -> Result<(), LexError> {
        loop {
            let Some(ch) = self.peek() else {
                return Ok(());
            };

            // 字节顺序标记在规范里属于空白，且可以出现在任意位置
            if ch.is_whitespace() || ch == '\u{FEFF}' {
                self.newline_pending |= is_line_terminator(ch);
                self.advance(ch);
                continue;
            }

            if self.rest().starts_with("//") {
                self.scan_line_comment();
                continue;
            }

            if self.rest().starts_with("/*") {
                self.scan_block_comment()?;
                continue;
            }

            if self.is_at_input_start() && self.rest().starts_with("<!--") {
                self.scan_line_comment();
                continue;
            }

            if self.newline_pending && self.rest().starts_with("-->") {
                self.scan_line_comment();
                continue;
            }

            return Ok(());
        }
    }

    /// 是否尚未产出任何 token，用于限定 HTML 风格注释的出现位置。
    fn is_at_input_start(&self) -> bool {
        self.previous.is_none()
    }

    /// 当前剩余源码。
    fn rest(&self) -> &'a str {
        &self.text()[self.position..]
    }

    /// 扫描 `//` 注释到行尾。
    fn scan_line_comment(&mut self) {
        let start = self.position;
        self.position += 2;
        while let Some(ch) = self.peek() {
            if is_line_terminator(ch) {
                break;
            }
            self.advance(ch);
        }
        let end = self.position;
        self.push_comment(CommentKind::Line, start, end);
    }

    /// 扫描 `/* */` 注释，未闭合时报错并把位置停在输入末尾。
    fn scan_block_comment(&mut self) -> Result<(), LexError> {
        let start = self.position;
        self.position += 2;

        loop {
            let Some(ch) = self.peek() else {
                self.newline_pending = true;
                let end = self.position;
                self.push_comment(CommentKind::Block, start, end);
                return Err(self.fail(LexErrorCode::UnterminatedComment, start, end));
            };

            if ch == '*' && self.char_ahead(1) == Some('/') {
                self.position += 2;
                self.newline_pending = true;
                let end = self.position;
                self.push_comment(CommentKind::Block, start, end);
                return Ok(());
            }

            self.newline_pending |= is_line_terminator(ch);
            self.advance(ch);
        }
    }

    /// 收录一条注释，正文为去掉标记后的内容。
    fn push_comment(&mut self, kind: CommentKind, start: usize, end: usize) {
        let body_start = start + 2;
        let body_end = match kind {
            CommentKind::Line => end,
            // 闭合块注释剔除结尾 `*/`；未闭合时尾部本就不含该标记
            CommentKind::Block if self.text().get(end.saturating_sub(2)..end) == Some("*/") => {
                end - 2
            }
            CommentKind::Block => end,
        };
        self.comments.push(Comment {
            kind,
            span: Span::new(start as u32, end as u32),
            text: &self.text()[body_start..body_end],
        });
    }

    // ---- token 分派 ----

    /// 按当前字符分派到具体扫描流程。
    fn scan(
        &mut self,
        ch: char,
        start: usize,
        newline_before: bool,
    ) -> Result<Token<'a>, LexError> {
        // 模板文本段：模式栈顶为 Template 时，任何字符都按文本处理
        if self.in_template_text() {
            return self.scan_template_text(start, newline_before);
        }

        let token = match ch {
            '"' | '\'' => self.scan_string(ch, start, newline_before)?,
            '`' => self.scan_template_head(start, newline_before),
            '0'..='9' => self.scan_number(start, newline_before)?,
            '.' if self.is_digit_ahead(1) => self.scan_number(start, newline_before)?,
            '#' if self.char_ahead(1).is_some_and(is_identifier_start) => {
                self.scan_private_name(start, newline_before)
            }
            '/' if self.regex_allowed() => self.scan_regex(start, newline_before)?,
            '}' if self.resumes_template() => self.close_substitution(start, newline_before),
            ch if is_identifier_start(ch) || self.starts_with_escape(ch) => {
                self.scan_identifier(start, newline_before)?
            }
            _ => self.scan_punctuator(start, newline_before)?,
        };
        Ok(token)
    }

    /// 当前是否处于模板文本段。
    fn in_template_text(&self) -> bool {
        self.modes.last() == Some(&Mode::Template)
    }

    /// 当前 `}` 是否用于回接模板文本。
    fn resumes_template(&self) -> bool {
        self.modes.last() == Some(&Mode::Substitution { depth: 0 })
    }

    /// 下一个 `/` 是否应作为正则起始。
    fn regex_allowed(&self) -> bool {
        match self.regex_override {
            Some(override_value) => override_value,
            None => !self.previous.is_some_and(|kind| kind.ends_expression()),
        }
    }

    /// 当前位置后 offset 字节处是否为 ASCII 数字。
    fn is_digit_ahead(&self, offset: usize) -> bool {
        matches!(self.char_ahead(offset), Some('0'..='9'))
    }

    /// 标识符是否以 `\u` 转义起始。
    fn starts_with_escape(&self, ch: char) -> bool {
        ch == '\\' && matches!(self.char_ahead(1), Some('u' | 'U'))
    }

    // ---- 标识符与关键字 ----

    /// 扫描标识符，含 `\u` 转义形式，并据此判定是否保留字。
    fn scan_identifier(
        &mut self,
        start: usize,
        newline_before: bool,
    ) -> Result<Token<'a>, LexError> {
        while let Some(ch) = self.peek() {
            if is_identifier_part(ch) && !self.in_identifier_escape(ch) {
                self.advance(ch);
                continue;
            }

            // 标识符内的 Unicode 转义：\uXXXX 或 \u{...}，两者等价于对应字符
            if ch == '\\' {
                self.scan_identifier_escape()?;
                continue;
            }

            break;
        }

        let end = self.position;
        let raw = &self.text()[start..end];
        if raw.is_empty() {
            return Err(self.fail(LexErrorCode::InvalidIdentifier, start, end));
        }

        // 含转义的标识符不参与保留字匹配：`\u0066or` 是变量名，不是关键字 `for`
        let word = super::literal::decode_identifier(raw)
            .ok_or_else(|| self.fail(LexErrorCode::InvalidIdentifier, start, end))?;
        let contains_escape = raw.contains('\\');
        let kind = match Keyword::from_word(&word) {
            Some(keyword) if !contains_escape => TokenKind::Keyword(keyword),
            _ => TokenKind::Identifier,
        };
        Ok(self.make(kind, start, end, newline_before))
    }

    /// 转义字符在标识符扫描里需要单独处理，普通字符判定不包含它。
    fn in_identifier_escape(&self, ch: char) -> bool {
        ch == '\\'
    }

    /// 消费标识符中的一个 `\u` 转义序列。
    fn scan_identifier_escape(&mut self) -> Result<(), LexError> {
        let start = self.position;
        self.advance('\\');

        let Some(head) = self.peek() else {
            return Err(self.fail(LexErrorCode::InvalidIdentifier, start, self.position));
        };
        if head != 'u' && head != 'U' {
            return Err(self.fail(LexErrorCode::InvalidIdentifier, start, self.position));
        }
        self.advance(head);

        let braced = self.peek() == Some('{');
        if braced {
            self.advance('{');
        }

        let digits_start = self.position;
        while let Some(ch) = self.peek() {
            if ch.is_ascii_hexdigit() {
                self.advance(ch);
                continue;
            }
            break;
        }
        let digits_end = self.position;

        if digits_start == digits_end {
            return Err(self.fail(LexErrorCode::InvalidIdentifier, digits_start, digits_end));
        }

        if braced {
            if self.peek() != Some('}') {
                return Err(self.fail(LexErrorCode::InvalidIdentifier, digits_end, self.position));
            }
            self.advance('}');
        } else if digits_end - digits_start != 4 {
            return Err(self.fail(LexErrorCode::InvalidIdentifier, digits_start, digits_end));
        }

        Ok(())
    }

    /// 扫描 `#name` 私有名称。
    fn scan_private_name(&mut self, start: usize, newline_before: bool) -> Token<'a> {
        self.advance('#');
        let identifier_start = self.position;
        let mut token = self
            .scan_identifier_body(identifier_start)
            .unwrap_or_else(|_| {
                self.make(
                    TokenKind::Identifier,
                    identifier_start,
                    self.position,
                    newline_before,
                )
            });
        token.kind = TokenKind::PrivateName;
        token.text = &self.text()[start..self.position];
        token.span = Span::new(start as u32, self.position as u32);
        token
    }

    /// 扫描标识符主体，供私有名称复用，不做保留字判定。
    fn scan_identifier_body(&mut self, start: usize) -> Result<Token<'a>, LexError> {
        while let Some(ch) = self.peek() {
            if ch == '\\' {
                self.scan_identifier_escape()?;
                continue;
            }
            if is_identifier_part(ch) {
                self.advance(ch);
                continue;
            }
            break;
        }
        Ok(self.make(TokenKind::Identifier, start, self.position, false))
    }

    // ---- 数字 ----

    /// 扫描数字字面量，覆盖十进制、`x`/`b`/`o` 进位、分隔符下划线、指数与 BigInt 后缀。
    fn scan_number(&mut self, start: usize, newline_before: bool) -> Result<Token<'a>, LexError> {
        let big_int = if self.peek() == Some('0') {
            self.scan_prefixed_radix(start)?
        } else {
            self.scan_decimal(start)?
        };

        self.reject_trailing_identifier(start)?;
        Ok(self.make(
            TokenKind::Number { bigint: big_int },
            start,
            self.position,
            newline_before,
        ))
    }

    /// 数字之后紧跟标识符起始字符属于非法写法，例如 `1abc`。
    fn reject_trailing_identifier(&mut self, start: usize) -> Result<(), LexError> {
        let Some(ch) = self.peek() else {
            return Ok(());
        };
        if is_identifier_start(ch) || ch.is_ascii_digit() {
            return Err(self.fail(LexErrorCode::InvalidNumericLiteral, start, self.position));
        }
        Ok(())
    }

    /// 扫描带前缀的进位字面量，返回是否为 BigInt。
    ///
    /// 前导零的两种历史写法（`0755` 与 `018`）在严格模式下属非法，
    /// mio 的一切代码都是模块，因此一律报错而不是静默接受。
    fn scan_prefixed_radix(&mut self, start: usize) -> Result<bool, LexError> {
        self.advance('0');
        let Some(marker) = self.peek() else {
            return Ok(false);
        };

        let radix = match marker {
            'x' | 'X' => 16,
            'b' | 'B' => 2,
            'o' | 'O' => 8,
            '0'..='7' => {
                return Err(self.fail(LexErrorCode::InvalidNumericLiteral, start, start + 2));
            }
            '8' | '9' => {
                return Err(self.fail(LexErrorCode::InvalidNumericLiteral, start, start + 2));
            }
            _ => return self.scan_decimal_tail(start),
        };

        self.advance(marker);
        self.scan_digits(radix, start)?;

        let big_int = self.take_big_int_suffix();
        Ok(big_int)
    }

    /// 扫描十进制字面量，返回是否为 BigInt。
    fn scan_decimal(&mut self, start: usize) -> Result<bool, LexError> {
        if self.peek() == Some('.') {
            self.advance('.');
            self.scan_digits(10, start)?;
            self.scan_exponent(start)?;
            return Ok(false);
        }

        self.scan_digits(10, start)?;
        self.scan_decimal_tail(start)
    }

    /// 十进制主体之后的小数、指数与 BigInt 后缀。
    fn scan_decimal_tail(&mut self, start: usize) -> Result<bool, LexError> {
        let mut has_fraction = false;

        if self.peek() == Some('.') {
            has_fraction = true;
            self.advance('.');
            // `1.` 合法，小数部分可以没有数字，但 `1.e2` 不合法
            if matches!(self.peek(), Some('0'..='9')) {
                self.scan_digits(10, start)?;
            }
        }

        let has_exponent = self.scan_exponent(start)?;
        let big_int = self.take_big_int_suffix();

        let invalid = big_int && (has_fraction || has_exponent);
        if invalid {
            return Err(self.fail(LexErrorCode::InvalidNumericLiteral, start, self.position));
        }
        Ok(big_int)
    }

    /// 扫描指数部分，返回是否存在。
    fn scan_exponent(&mut self, start: usize) -> Result<bool, LexError> {
        let Some(marker) = self.peek() else {
            return Ok(false);
        };
        if marker != 'e' && marker != 'E' {
            return Ok(false);
        }

        self.advance(marker);
        if matches!(self.peek(), Some('+' | '-')) {
            let sign = self.peek().expect("符号刚被判定存在");
            self.advance(sign);
        }
        self.scan_digits(10, start)?;
        Ok(true)
    }

    /// 消费 BigInt 后缀 `n`，返回是否存在。
    fn take_big_int_suffix(&mut self) -> bool {
        if self.peek() != Some('n') {
            return false;
        }
        self.advance('n');
        true
    }

    /// 扫描指定进位的一串数字，允许数字之间的单下划线分隔符。
    ///
    /// 分隔符规则三条：不得起始、不得结尾、不得连续。三者任一违反即报错。
    fn scan_digits(&mut self, radix: u32, start: usize) -> Result<(), LexError> {
        let from = self.position;
        let mut digits = 0usize;
        let mut previous_was_separator = false;

        while let Some(ch) = self.peek() {
            if ch == '_' {
                let rejected = previous_was_separator || digits == 0;
                if rejected {
                    return Err(self.fail(
                        LexErrorCode::InvalidNumericLiteral,
                        start,
                        self.position,
                    ));
                }
                previous_was_separator = true;
                self.advance(ch);
                continue;
            }

            if ch.is_digit(radix) {
                previous_was_separator = false;
                digits += 1;
                self.advance(ch);
                continue;
            }

            break;
        }

        let empty = digits == 0;
        if empty || previous_was_separator {
            return Err(self.fail(LexErrorCode::InvalidNumericLiteral, start, from.max(start)));
        }
        Ok(())
    }

    // ---- 字符串 ----

    /// 扫描字符串字面量。
    ///
    /// 这里只定位结束引号并拒绝裸行终止符；转义序列的合法性与真值在
    /// `literal::cook_string` 中处理，避免同一套转义规则在两处重复实现。
    fn scan_string(
        &mut self,
        quote: char,
        start: usize,
        newline_before: bool,
    ) -> Result<Token<'a>, LexError> {
        self.advance(quote);

        loop {
            let Some(ch) = self.peek() else {
                return Err(self.fail(LexErrorCode::UnterminatedString, start, self.position));
            };

            match ch {
                c if c == quote => {
                    self.advance(c);
                    return Ok(self.make(TokenKind::String, start, self.position, newline_before));
                }
                '\n' | '\r' => {
                    return Err(self.fail(LexErrorCode::UnterminatedString, start, self.position));
                }
                '\\' => {
                    self.advance('\\');
                    if let Some(escaped) = self.peek() {
                        self.advance(escaped);
                    }
                }
                _ => self.advance(ch),
            }
        }
    }

    // ---- 模板字面量 ----

    /// 扫描起始反引号。
    fn scan_template_head(&mut self, start: usize, newline_before: bool) -> Token<'a> {
        self.modes.push(Mode::Template);
        self.advance('`');
        self.make(
            TokenKind::Template(TemplateDelimiter::Head),
            start,
            self.position,
            newline_before,
        )
    }

    /// 扫描模板文本段，直到遇到反引号或 `${`。
    fn scan_template_text(
        &mut self,
        start: usize,
        newline_before: bool,
    ) -> Result<Token<'a>, LexError> {
        loop {
            let Some(ch) = self.peek() else {
                return Err(self.fail(LexErrorCode::UnterminatedTemplate, start, self.position));
            };

            match ch {
                '`' => {
                    self.pop_mode();
                    self.advance('`');
                    return Ok(self.make(
                        TokenKind::Template(TemplateDelimiter::Tail),
                        start,
                        self.position,
                        newline_before,
                    ));
                }
                '$' if self.char_ahead(1) == Some('{') => {
                    self.position += 2;
                    self.modes.push(Mode::Substitution { depth: 0 });
                    return Ok(self.make(
                        TokenKind::Template(TemplateDelimiter::Substitution),
                        start,
                        self.position,
                        newline_before,
                    ));
                }
                '\\' => {
                    self.advance('\\');
                    if let Some(escaped) = self.peek() {
                        self.advance(escaped);
                    }
                }
                _ => self.advance(ch),
            }
        }
    }

    /// 模板替换区间的结束花括号。
    fn close_substitution(&mut self, start: usize, newline_before: bool) -> Token<'a> {
        self.pop_mode();
        self.advance('}');
        self.make(
            TokenKind::Template(TemplateDelimiter::Resume),
            start,
            self.position,
            newline_before,
        )
    }

    /// 弹出模式栈顶，但保留栈底，保证栈永不为空。
    fn pop_mode(&mut self) {
        if self.modes.len() > 1 {
            self.modes.pop();
        }
    }

    // ---- 正则 ----

    /// 扫描正则字面量，字符类内的 `/` 不作结束符处理。
    fn scan_regex(&mut self, start: usize, newline_before: bool) -> Result<Token<'a>, LexError> {
        self.advance('/');
        let mut in_class = false;

        loop {
            let Some(ch) = self.peek() else {
                return Err(self.fail(LexErrorCode::UnterminatedRegExp, start, self.position));
            };

            match ch {
                '\n' | '\r' => {
                    return Err(self.fail(LexErrorCode::UnterminatedRegExp, start, self.position));
                }
                '\\' => {
                    self.advance('\\');
                    let escaped = self.peek().ok_or_else(|| {
                        self.fail(LexErrorCode::UnterminatedRegExp, start, self.position)
                    })?;
                    self.advance(escaped);
                }
                '[' => {
                    in_class = true;
                    self.advance(ch);
                }
                ']' => {
                    in_class = false;
                    self.advance(ch);
                }
                '/' if !in_class => {
                    self.advance('/');
                    break;
                }
                _ => self.advance(ch),
            }
        }

        let flags_start = self.position;
        while let Some(ch) = self.peek() {
            if is_identifier_part(ch) {
                self.advance(ch);
                continue;
            }
            break;
        }

        let flags = &self.text()[flags_start..self.position];
        super::literal::check_regex_flags(flags).ok_or_else(|| {
            self.fail(LexErrorCode::InvalidRegExpFlag, flags_start, self.position)
        })?;

        Ok(self.make(
            TokenKind::RegularExpression,
            start,
            self.position,
            newline_before,
        ))
    }

    // ---- 运算符 ----

    /// 扫描运算符与界符，同时维护替换模式内的花括号深度。
    fn scan_punctuator(
        &mut self,
        start: usize,
        newline_before: bool,
    ) -> Result<Token<'a>, LexError> {
        let (punctuator, length) = Punctuator::match_longest(self.rest()).ok_or_else(|| {
            let ch = self.peek().expect("此处已由调用方确认存在字符");
            self.fail(LexErrorCode::InvalidCharacter, start, start + ch.len_utf8())
        })?;

        // `?.5` 里的 `?` 是三元符，不是可选链：规范规定 `?` 后接数字时不许形成 `?.`
        let is_optional_chain_on_digit =
            punctuator == Punctuator::OptionalChain && self.is_digit_ahead(length);
        let effective_length = if is_optional_chain_on_digit {
            1
        } else {
            length
        };
        let effective = if is_optional_chain_on_digit {
            Punctuator::Question
        } else {
            punctuator
        };

        self.position += effective_length;
        self.track_substitution_depth(effective);

        Ok(self.make(
            TokenKind::Punctuator(effective),
            start,
            self.position,
            newline_before,
        ))
    }

    /// 在模板替换区间内累加或回退花括号深度。
    fn track_substitution_depth(&mut self, punctuator: Punctuator) {
        let top = self.modes.last_mut();
        let Some(Mode::Substitution { depth }) = top else {
            return;
        };
        match punctuator {
            Punctuator::LeftBrace => *depth += 1,
            Punctuator::RightBrace => *depth = depth.saturating_sub(1),
            _ => {}
        }
    }
}

/// 判定字符可否起始标识符。
///
/// ECMAScript 采用 Unicode `ID_Start` 属性，完整判定需要 Unicode 表。
/// 这里以 `is_alphabetic` 加 `_`、`$` 近似，覆盖实际代码中的绝大多数写法；
/// 严格集合与 `ID_Continue` 的少数缺口（如加泰罗尼亚中点 U+00B7）
/// 留待阶段 B 引入 Unicode 属性表时补齐，此处不静默扩大语言子集。
fn is_identifier_start(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_alphabetic()
}

/// 判定字符可否续写标识符。
fn is_identifier_part(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '\u{200C}' | '\u{200D}')
}

/// 是否为行终止符。按规范，U+2028 与 U+2029 同属行终止符。
fn is_line_terminator(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 收集源码全部 token，断言无词法错误后返回其类别序列。
    fn kinds(text: &str) -> Vec<TokenKind> {
        let source = Source::new(text.to_string());
        let (tokens, errors) = Lexer::new(&source).collect();
        assert!(errors.is_empty(), "意外的词法错误：{errors:?}");
        tokens.iter().map(|token| token.kind).collect()
    }

    /// 收集源码全部 token，断言无词法错误后返回其原始文本（不含文件结束）。
    fn texts(text: &str) -> Vec<String> {
        let source = Source::new(text.to_string());
        let (tokens, errors) = Lexer::new(&source).collect();
        assert!(errors.is_empty(), "意外的词法错误：{errors:?}");
        tokens
            .iter()
            .filter(|token| token.kind != TokenKind::EndOfFile)
            .map(|token| token.text.to_string())
            .collect()
    }

    #[test]
    fn scans_identifiers_keywords_and_punctuation() {
        assert_eq!(
            kinds("const x = 1;"),
            vec![
                TokenKind::Keyword(Keyword::Const),
                TokenKind::Identifier,
                TokenKind::Punctuator(Punctuator::Assign),
                TokenKind::Number { bigint: false },
                TokenKind::Punctuator(Punctuator::Semicolon),
                TokenKind::EndOfFile,
            ]
        );
    }

    #[test]
    fn leaves_typescript_soft_keywords_as_identifiers() {
        // for / const 是保留字，type / as 在规范里仍是合法标识符
        assert_eq!(
            kinds("for const type as"),
            vec![
                TokenKind::Keyword(Keyword::For),
                TokenKind::Keyword(Keyword::Const),
                TokenKind::Identifier,
                TokenKind::Identifier,
                TokenKind::EndOfFile,
            ]
        );
    }

    #[test]
    fn token_carries_source_span() {
        let source = Source::new(String::from("let a"));
        let (tokens, _) = Lexer::new(&source).collect();
        assert_eq!(tokens[0].span, Span::new(0, 3));
        assert_eq!(tokens[0].text, "let");
    }

    #[test]
    fn escaped_identifier_is_not_a_keyword() {
        // `\u0066or` 还原后是 for，但含转义者不参与保留字匹配，应作标识符
        assert_eq!(
            kinds("\\u0066or"),
            vec![TokenKind::Identifier, TokenKind::EndOfFile]
        );
        assert_eq!(texts("\\u0066or"), vec![String::from("\\u0066or")]);
    }

    #[test]
    fn scans_number_literal_forms() {
        assert_eq!(
            kinds("0x1F 1_000 1e3 42n"),
            vec![
                TokenKind::Number { bigint: false },
                TokenKind::Number { bigint: false },
                TokenKind::Number { bigint: false },
                TokenKind::Number { bigint: true },
                TokenKind::EndOfFile,
            ]
        );
        assert_eq!(texts("42n"), vec![String::from("42n")]);
    }

    #[test]
    fn rejects_leading_zero_octal() {
        let source = Source::new(String::from("0755"));
        let error = Lexer::new(&source)
            .next_token()
            .expect_err("前导零八进制应报错");
        assert_eq!(error.code, LexErrorCode::InvalidNumericLiteral);
    }

    #[test]
    fn scans_string_literal() {
        assert_eq!(
            kinds(r#""a\nb""#),
            vec![TokenKind::String, TokenKind::EndOfFile]
        );
    }

    #[test]
    fn rejects_unterminated_string() {
        let source = Source::new(String::from("\"abc\n"));
        let error = Lexer::new(&source)
            .next_token()
            .expect_err("未闭合字符串应报错");
        assert_eq!(error.code, LexErrorCode::UnterminatedString);
    }

    #[test]
    fn scans_template_with_one_substitution() {
        // 文本段并入其后的替换符与结束符：Head, a${, b, }, c`, EOF
        assert_eq!(
            kinds("`a${b}c`"),
            vec![
                TokenKind::Template(TemplateDelimiter::Head),
                TokenKind::Template(TemplateDelimiter::Substitution),
                TokenKind::Identifier,
                TokenKind::Template(TemplateDelimiter::Resume),
                TokenKind::Template(TemplateDelimiter::Tail),
                TokenKind::EndOfFile,
            ]
        );
    }

    #[test]
    fn scans_template_with_two_substitutions() {
        assert_eq!(
            kinds("`x${a}y${b}z`"),
            vec![
                TokenKind::Template(TemplateDelimiter::Head),
                TokenKind::Template(TemplateDelimiter::Substitution),
                TokenKind::Identifier,
                TokenKind::Template(TemplateDelimiter::Resume),
                TokenKind::Template(TemplateDelimiter::Substitution),
                TokenKind::Identifier,
                TokenKind::Template(TemplateDelimiter::Resume),
                TokenKind::Template(TemplateDelimiter::Tail),
                TokenKind::EndOfFile,
            ]
        );
    }

    #[test]
    fn reads_slash_after_identifier_as_division() {
        // 标识符是表达式终点，其后 `/` 判为除号：`a / b` 得单个 Slash
        assert_eq!(
            kinds("a / b"),
            vec![
                TokenKind::Identifier,
                TokenKind::Punctuator(Punctuator::Slash),
                TokenKind::Identifier,
                TokenKind::EndOfFile,
            ]
        );
    }

    #[test]
    fn reads_slash_after_operator_as_regex() {
        // 赋值后期待表达式，`/` 开启正则
        assert_eq!(
            kinds("x = /ab+/gi"),
            vec![
                TokenKind::Identifier,
                TokenKind::Punctuator(Punctuator::Assign),
                TokenKind::RegularExpression,
                TokenKind::EndOfFile,
            ]
        );
    }

    #[test]
    fn regex_override_forces_regex_after_identifier() {
        // 启发式默认把标识符后的 `/` 当除号；语法分析器可强制改判为正则
        let source = Source::new(String::from("a /b/"));
        let mut lexer = Lexer::new(&source);
        assert_eq!(lexer.next_token().unwrap().kind, TokenKind::Identifier);
        lexer.set_regex_allowed(true);
        assert_eq!(
            lexer.next_token().unwrap().kind,
            TokenKind::RegularExpression
        );
    }

    #[test]
    fn scans_private_name() {
        assert_eq!(
            kinds("#secret"),
            vec![TokenKind::PrivateName, TokenKind::EndOfFile]
        );
        assert_eq!(texts("#secret"), vec![String::from("#secret")]);
    }

    #[test]
    fn matches_longest_punctuator() {
        assert_eq!(
            kinds("x >>>= 1"),
            vec![
                TokenKind::Identifier,
                TokenKind::Punctuator(Punctuator::UnsignedShiftRightAssign),
                TokenKind::Number { bigint: false },
                TokenKind::EndOfFile,
            ]
        );
    }

    #[test]
    fn reads_optional_chain() {
        assert_eq!(
            kinds("a?.b"),
            vec![
                TokenKind::Identifier,
                TokenKind::Punctuator(Punctuator::OptionalChain),
                TokenKind::Identifier,
                TokenKind::EndOfFile,
            ]
        );
    }

    #[test]
    fn marks_newline_before_for_asi() {
        let source = Source::new(String::from("a\nb"));
        let mut lexer = Lexer::new(&source);
        assert!(!lexer.next_token().unwrap().newline_before);
        assert!(lexer.next_token().unwrap().newline_before);
    }

    #[test]
    fn empty_source_yields_only_eof() {
        let source = Source::new(String::new());
        let token = Lexer::new(&source).next_token().unwrap();
        assert_eq!(token.kind, TokenKind::EndOfFile);
        assert_eq!(token.span, Span::new(0, 0));
    }

    #[test]
    fn collect_recovers_after_invalid_character() {
        // 非法字符不应终止扫描：记录错误后强制推进，继续产出后续 token
        let source = Source::new(String::from("\\x"));
        let (tokens, errors) = Lexer::new(&source).collect();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, LexErrorCode::InvalidCharacter);
        assert!(
            tokens
                .iter()
                .any(|token| token.kind == TokenKind::Identifier)
        );
    }

    #[test]
    fn collects_comments_separately_from_tokens() {
        let source = Source::new(String::from(
            "// line\n/* block */\n/** doc */\nlet a = 1;\n",
        ));
        let mut lexer = Lexer::new(&source);
        loop {
            let token = lexer.next_token().unwrap();
            if token.kind == TokenKind::EndOfFile {
                break;
            }
        }

        let comments = lexer.comments();
        assert_eq!(comments.len(), 3);
        assert_eq!(comments[0].kind, CommentKind::Line);
        assert_eq!(comments[1].kind, CommentKind::Block);
        assert_eq!(comments[2].kind, CommentKind::Block);
        let doc_count = comments.iter().filter(|comment| comment.is_doc()).count();
        assert_eq!(doc_count, 1, "仅 /** */ 应判为文档注释");
    }
}

//! 字面量真值转换。
//!
//! 扫描器只负责划定 token 边界，真值转换放在这里单独实现，理由有两条：
//! 转义规则的分支多、易错，独立成模块便于逐条测试；
//! 而绝大多数 token 用不到真值，延迟转换可以避免在 token 流里堆积临时字符串。

use std::borrow::Cow;

use super::error::LexErrorCode;

/// 真值转换失败的位置与原因。
///
/// `offset` 是相对字面量文本起点的字节偏移，由调用方换算成源码区间后再构造错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiteralError {
    /// 错误编码。
    pub code: LexErrorCode,
    /// 相对字面量起点的字节偏移。
    pub offset: usize,
}

impl LiteralError {
    /// 构造错误。
    pub fn new(code: LexErrorCode, offset: usize) -> Self {
        Self { code, offset }
    }
}

/// 把带引号的字符串字面量还原成真值。
pub fn cook_string(raw: &str) -> Result<String, LiteralError> {
    let inner = trim_quotes(raw);
    cook(inner, false)
}

/// 把模板文本段还原成真值。模板文本不含定界符，故直接处理。
pub fn cook_template_text(raw: &str) -> Result<String, LiteralError> {
    cook(raw, true)
}

/// 去掉字符串字面量首尾引号。
fn trim_quotes(raw: &str) -> &str {
    let shortened = raw.strip_prefix(['"', '\'']).unwrap_or(raw);
    shortened.strip_suffix(['"', '\'']).unwrap_or(shortened)
}

/// 逐字符处理转义，产出真值。
fn cook(inner: &str, template: bool) -> Result<String, LiteralError> {
    let mut output = String::with_capacity(inner.len());
    let mut position = 0usize;
    let bytes = inner.as_bytes();

    while position < bytes.len() {
        let ch = inner[position..].chars().next().expect("位置已由长度确认");
        let width = ch.len_utf8();

        if ch != '\\' {
            output.push(ch);
            position += width;
            continue;
        }

        let Some(escaped) = inner[position + width..].chars().next() else {
            // 结尾裸反斜杠属于非法转义
            return Err(LiteralError::new(
                LexErrorCode::InvalidEscapeSequence,
                position,
            ));
        };

        let consumed = cook_escape(inner, position, escaped, template, &mut output)?;
        position += consumed;
    }

    Ok(output)
}

/// 处理一个以反斜杠起始的序列，返回其消耗的字节数。
fn cook_escape(
    inner: &str,
    position: usize,
    escaped: char,
    template: bool,
    output: &mut String,
) -> Result<usize, LiteralError> {
    // 行延续：反斜杠紧跟行终止符，两者一起消失
    let simple = match escaped {
        'n' => Some('\n'),
        'r' => Some('\r'),
        't' => Some('\t'),
        'b' => Some('\u{08}'),
        'f' => Some('\u{0C}'),
        'v' => Some('\u{0B}'),
        '0' if !next_is_digit(inner, position + 2) => Some('\0'),
        c if c == '\\' || c == '\'' || c == '"' => Some(c),
        _ => None,
    };
    if let Some(resolved) = simple {
        output.push(resolved);
        return Ok(1 + resolved.len_utf8());
    }

    match escaped {
        'x' => cook_hex_escape(inner, position, 2, output),
        'u' => cook_unicode_escape(inner, position, output),
        '\n' | '\r' => Ok(skip_line_continuation(inner, position, escaped)),
        '0'..='7' => Err(LiteralError::new(
            LexErrorCode::InvalidEscapeSequence,
            position,
        )),
        // IdentityEscape：其余字符按字面保留。模板里未定义的反斜杠序列一律非法
        other if !template || is_identity_allowed(other) => {
            output.push(other);
            Ok(1 + other.len_utf8())
        }
        _ => Err(LiteralError::new(
            LexErrorCode::InvalidEscapeSequence,
            position,
        )),
    }
}

/// 判断某字符之后是否还是数字，用于区分 `\0` 与遗留八进制 `\012`。
fn next_is_digit(inner: &str, offset: usize) -> bool {
    inner
        .get(offset..)
        .and_then(|rest| rest.chars().next())
        .is_some_and(|ch| ch.is_ascii_digit())
}

/// 遗留八进制转义在严格模式与模块码中非法，模板里的 `\0` 后接数字同样非法。
fn is_identity_allowed(ch: char) -> bool {
    ch.is_alphabetic() || ch == '_' || ch == '$'
}

/// 处理 `\xHH`。
fn cook_hex_escape(
    inner: &str,
    position: usize,
    digits: usize,
    output: &mut String,
) -> Result<usize, LiteralError> {
    let body = inner
        .get(position + 2..)
        .filter(|rest| rest.len() >= digits)
        .map(|rest| &rest[..digits])
        .filter(|body| body.chars().all(|ch| ch.is_ascii_hexdigit()));

    match body {
        Some(text) => {
            let value = u32::from_str_radix(text, 16).expect("已确认为十六进制数字");
            let ch = char::from_u32(value)
                .ok_or_else(|| LiteralError::new(LexErrorCode::InvalidEscapeSequence, position))?;
            output.push(ch);
            Ok(2 + digits)
        }
        None => Err(LiteralError::new(
            LexErrorCode::InvalidEscapeSequence,
            position,
        )),
    }
}

/// 处理 `\uHHHH` 与 `\u{...}` 两种形态。
fn cook_unicode_escape(
    inner: &str,
    position: usize,
    output: &mut String,
) -> Result<usize, LiteralError> {
    let invalid = || LiteralError::new(LexErrorCode::InvalidEscapeSequence, position);
    let rest = inner.get(position + 2..).ok_or_else(invalid)?;

    if let Some(braced) = rest.strip_prefix('{') {
        let close = braced.find('}').ok_or_else(invalid)?;
        let digits = &braced[..close];
        let valid = !digits.is_empty()
            && digits.chars().all(|ch| ch.is_ascii_hexdigit())
            && digits.len() <= 8;
        if !valid {
            return Err(invalid());
        }
        let value = u32::from_str_radix(digits, 16).map_err(|_| invalid())?;
        let ch = char::from_u32(value).ok_or_else(invalid)?;
        output.push(ch);
        return Ok(2 + close + 2);
    }

    let fixed = 4;
    if rest.len() < fixed {
        return Err(invalid());
    }
    let digits = &rest[..fixed];
    if !digits.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let value = u32::from_str_radix(digits, 16).map_err(|_| invalid())?;
    let ch = char::from_u32(value).ok_or_else(invalid)?;
    output.push(ch);
    Ok(2 + fixed)
}

/// 计算行延续消耗的字节数，`\r\n` 作为一个序列处理。
fn skip_line_continuation(inner: &str, position: usize, escaped: char) -> usize {
    let after = position + 1 + escaped.len_utf8();
    let following = inner.get(after..).and_then(|rest| rest.chars().next());
    let consumed_if_lf = escaped == '\r' && following == Some('\n');
    if consumed_if_lf {
        after + 1 - position
    } else {
        after - position
    }
}

/// 还原标识符中的 Unicode 转义。
///
/// 规范规定：含转义的标识符不参与保留字匹配，因此本函数只服务于取名字，
/// 扫描器判定关键字时不使用它。无转义时直接返回借用，不分配。
pub fn decode_identifier(raw: &str) -> Option<Cow<'_, str>> {
    if !raw.contains('\\') {
        return Some(Cow::Borrowed(raw));
    }

    let mut output = String::with_capacity(raw.len());
    let mut position = 0usize;

    while position < raw.len() {
        let ch = raw[position..].chars().next()?;
        if ch != '\\' {
            output.push(ch);
            position += ch.len_utf8();
            continue;
        }

        let escaped = raw.get(position + 1..)?.chars().next()?;
        if escaped != 'u' {
            return None;
        }
        let mut scratch = String::new();
        let consumed = cook_unicode_escape(raw, position, &mut scratch).ok()?;
        output.push_str(&scratch);
        position += consumed;
    }

    Some(Cow::Owned(output))
}

/// 校验正则修饰符：字母须属于允许集合，不得重复，`u` 与 `v` 互斥。
pub fn check_regex_flags(flags: &str) -> Option<()> {
    let mut seen: Vec<char> = Vec::new();
    for ch in flags.chars() {
        if !matches!(ch, 'd' | 'g' | 'i' | 'm' | 's' | 'u' | 'v' | 'y') {
            return None;
        }
        if seen.contains(&ch) {
            return None;
        }
        seen.push(ch);
    }

    let conflicts = seen.contains(&'u') && seen.contains(&'v');
    if conflicts {
        return None;
    }
    Some(())
}

/// 求数字字面量的十进制真值。
///
/// 带 `n` 后缀的 BigInt 不在此处求值：任意精度需要另一套数值类型，
/// 属于值模型步骤，此处返回 `None` 表示交由上层按十进制文本处理。
pub fn number_value(raw: &str) -> Option<f64> {
    if raw.ends_with('n') {
        return None;
    }

    let cleaned: String = raw.chars().filter(|ch| *ch != '_').collect();
    let lowered = cleaned.to_lowercase();

    for (marker, radix) in [("0x", 16), ("0b", 2), ("0o", 8)] {
        if let Some(body) = lowered.strip_prefix(marker) {
            let value = u128::from_str_radix(body, radix).ok()?;
            return Some(value as f64);
        }
    }

    cleaned.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cooks_simple_escapes() {
        assert_eq!(cook_string(r#""a\nb""#).ok().as_deref(), Some("a\nb"));
        assert_eq!(cook_string(r#"'it\'s'"#).ok().as_deref(), Some("it's"));
        assert_eq!(
            cook_string(r#""tab\there""#).ok().as_deref(),
            Some("tab\there")
        );
        assert_eq!(cook_string(r#""\0""#).ok().as_deref(), Some("\0"));
    }

    #[test]
    fn cooks_hex_and_unicode_escapes() {
        assert_eq!(cook_string(r#""\x41""#).ok().as_deref(), Some("A"));
        assert_eq!(cook_string(r#""\u0041""#).ok().as_deref(), Some("A"));
        assert_eq!(
            cook_string(r#""\u{1F600}""#).ok().as_deref(),
            Some("\u{1F600}")
        );
    }

    #[test]
    fn rejects_malformed_escapes() {
        assert_eq!(
            cook_string(r#""\x4""#).err().map(|e| e.code),
            Some(LexErrorCode::InvalidEscapeSequence)
        );
        assert_eq!(
            cook_string(r#""\u12""#).err().map(|e| e.code),
            Some(LexErrorCode::InvalidEscapeSequence)
        );
        assert_eq!(
            cook_string(r#""\u{110000}""#).err().map(|e| e.code),
            Some(LexErrorCode::InvalidEscapeSequence)
        );
    }

    #[test]
    fn rejects_legacy_octal_escape() {
        // \1 \2 一类遗留八进制转义在严格模式与模块码中非法
        assert_eq!(
            cook_string(r#""\123""#).err().map(|e| e.code),
            Some(LexErrorCode::InvalidEscapeSequence)
        );
        assert_eq!(
            cook_string(r#""\01""#).err().map(|e| e.code),
            Some(LexErrorCode::InvalidEscapeSequence)
        );
    }

    #[test]
    fn drops_line_continuation() {
        assert_eq!(
            cook_string("line\\\nbreak").ok().as_deref(),
            Some("linebreak")
        );
        assert_eq!(
            cook_string("line\\\r\nbreak").ok().as_deref(),
            Some("linebreak")
        );
    }

    #[test]
    fn keeps_identity_escape() {
        assert_eq!(cook_string(r#""\a""#).ok().as_deref(), Some("a"));
    }

    #[test]
    fn cooks_template_text() {
        assert_eq!(cook_template_text(r"a\tb").ok().as_deref(), Some("a\tb"));
        assert_eq!(cook_template_text("plain").ok().as_deref(), Some("plain"));
    }

    #[test]
    fn decodes_escaped_identifier() {
        assert_eq!(decode_identifier("for").as_deref(), Some("for"));
        assert_eq!(decode_identifier(r"\u0066or").as_deref(), Some("for"));
        assert_eq!(
            decode_identifier(r"caf\u{E9}").as_deref(),
            Some("caf\u{E9}")
        );
        assert_eq!(decode_identifier(r"\uZZ"), None);
    }

    #[test]
    fn accepts_valid_regex_flags() {
        assert!(check_regex_flags("gi").is_some());
        assert!(check_regex_flags("dgimsy").is_some());
        assert!(check_regex_flags("").is_some());
    }

    #[test]
    fn rejects_bad_regex_flags() {
        assert_eq!(check_regex_flags("gg"), None);
        assert_eq!(check_regex_flags("uv"), None);
        assert_eq!(check_regex_flags("z"), None);
    }

    #[test]
    fn evaluates_number_literals() {
        assert_eq!(number_value("42"), Some(42.0));
        assert_eq!(number_value("1_000"), Some(1000.0));
        assert_eq!(number_value(".5"), Some(0.5));
        assert_eq!(number_value("1e3"), Some(1000.0));
        assert_eq!(number_value("0xff"), Some(255.0));
        assert_eq!(number_value("0b101"), Some(5.0));
        assert_eq!(number_value("0o17"), Some(15.0));
    }

    #[test]
    fn defers_big_int_literals() {
        assert_eq!(number_value("42n"), None);
    }
}

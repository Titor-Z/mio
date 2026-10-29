//! JSONC 预处理：把带注释的 JSON 文本转换为标准 JSON 文本。
//!
//! 处理三类非标准语法，与 `tsconfig.json` 保持一致：
//! 1. 行注释 `// ...`
//! 2. 块注释 `/* ... */`
//! 3. 对象与数组的尾逗号
//!
//! 另外剥离 UTF-8 BOM。
//!
//! 必须逐字符扫描而非正则替换：字符串字面量内部可能出现 `//`
//! （例如 `"https://example.com"`），正则会误删。

/// 把 JSONC 文本转换为标准 JSON 文本。
///
/// 保留换行以便错误信息中的行列号仍然指向原文位置。
pub fn strip(input: &str) -> String {
    let without_bom = input.strip_prefix('\u{feff}').unwrap_or(input);
    let without_comments = strip_comments(without_bom);
    strip_trailing_commas(&without_comments)
}

/// 逐字符剥离注释，字符串字面量内部原样保留。
fn strip_comments(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    let mut in_string = false;
    let mut escaped = false;

    while index < chars.len() {
        let current = chars[index];

        if in_string {
            output.push(current);
            if escaped {
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if current == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if current == '"' {
            in_string = true;
            output.push(current);
            index += 1;
            continue;
        }

        let next = chars.get(index + 1).copied();
        match (current, next) {
            ('/', Some('/')) => {
                index += 2;
                while index < chars.len() && chars[index] != '\n' {
                    index += 1;
                }
                output.push('\n');
                index += 1;
            }
            ('/', Some('*')) => {
                index += 2;
                while index + 1 < chars.len() && !(chars[index] == '*' && chars[index + 1] == '/') {
                    // 保留换行，使后续行列号与原文对齐
                    if chars[index] == '\n' {
                        output.push('\n');
                    }
                    index += 1;
                }
                index = (index + 2).min(chars.len());
            }
            _ => {
                output.push(current);
                index += 1;
            }
        }
    }

    output
}

/// 删除紧邻 `}` 或 `]` 之前的逗号。
///
/// 只处理「逗号 + 空白 + 闭合符」这一种形态，不动字符串内部内容
/// （此阶段注释已剥离，字符串状态需重新跟踪）。
fn strip_trailing_commas(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut output = String::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut index = 0;

    while index < chars.len() {
        let current = chars[index];

        if in_string {
            output.push(current);
            if escaped {
                escaped = false;
            } else if current == '\\' {
                escaped = true;
            } else if current == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }

        if current == '"' {
            in_string = true;
            output.push(current);
            index += 1;
            continue;
        }

        if current == ',' {
            let mut lookahead = index + 1;
            while lookahead < chars.len() && chars[lookahead].is_whitespace() {
                lookahead += 1;
            }
            let closes = matches!(chars.get(lookahead), Some('}') | Some(']'));
            if closes {
                index += 1;
                continue;
            }
        }

        output.push(current);
        index += 1;
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_line_comment() {
        let input = "{\n  // 说明\n  \"a\": 1\n}";
        let parsed: serde_json::Value = serde_json::from_str(&strip(input)).expect("应可解析");
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn strips_block_comment() {
        let input = "{ /* 多行\n注释 */ \"a\": 2 }";
        let parsed: serde_json::Value = serde_json::from_str(&strip(input)).expect("应可解析");
        assert_eq!(parsed["a"], 2);
    }

    #[test]
    fn keeps_comment_markers_inside_strings() {
        let input = "{\"url\": \"https://example.com/a//b\"}";
        let parsed: serde_json::Value = serde_json::from_str(&strip(input)).expect("应可解析");
        assert_eq!(parsed["url"], "https://example.com/a//b");
    }

    #[test]
    fn keeps_block_comment_markers_inside_strings() {
        let input = "{\"tpl\": \"a/*b*/c\"}";
        let parsed: serde_json::Value = serde_json::from_str(&strip(input)).expect("应可解析");
        assert_eq!(parsed["tpl"], "a/*b*/c");
    }

    #[test]
    fn strips_trailing_comma_in_object() {
        let input = "{\n  \"a\": 1,\n}";
        let parsed: serde_json::Value = serde_json::from_str(&strip(input)).expect("应可解析");
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn strips_trailing_comma_in_array() {
        let input = "{\"a\": [1, 2, ]}";
        let parsed: serde_json::Value = serde_json::from_str(&strip(input)).expect("应可解析");
        assert_eq!(parsed["a"][1], 2);
    }

    #[test]
    fn keeps_comma_inside_string_near_brace() {
        let input = "{\"a\": \"x, }\"}";
        let parsed: serde_json::Value = serde_json::from_str(&strip(input)).expect("应可解析");
        assert_eq!(parsed["a"], "x, }");
    }

    #[test]
    fn strips_bom() {
        let input = "\u{feff}{\"a\": 3}";
        let parsed: serde_json::Value = serde_json::from_str(&strip(input)).expect("应可解析");
        assert_eq!(parsed["a"], 3);
    }
}

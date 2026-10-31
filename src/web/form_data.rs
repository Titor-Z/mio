//! `FormData` 的纯数据层：字段值模型与 multipart/form-data 编码（RFC 7578）。
//!
//! 编码结果用于 `fetch` 的 body 上线；文件名/字段名中的 CR、LF 会被剔除，
//! 防止头部注入（RFC 7578 §6.1 的传输安全要求）。

use std::fmt::Write as _;

/// 一个字段的值：文本或文件（文件带文件名与 MIME 类型）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldValue {
    Text(String),
    File {
        filename: String,
        content_type: String,
        bytes: Vec<u8>,
    },
}

/// FormData 的有序字段表（允许重复键）。
pub type Entries = Vec<(String, FieldValue)>;

/// 追加一个字段（允许同名重复）。
pub fn append(entries: &mut Entries, name: String, value: FieldValue) {
    entries.push((name, value));
}

/// 首个匹配字段的值。
pub fn get<'a>(entries: &'a Entries, name: &str) -> Option<&'a FieldValue> {
    entries
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

/// 全部匹配字段的值（保持顺序）。
pub fn get_all(entries: &Entries, name: &str) -> Vec<FieldValue> {
    entries
        .iter()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .collect()
}

/// 规范 set：替换首个匹配并删除其余同名项；不存在则追加。
pub fn set(entries: &mut Entries, name: String, value: FieldValue) {
    let mut replaced = false;
    entries.retain(|(key, _)| {
        if key != &name {
            return true;
        }
        if replaced {
            return false;
        }
        replaced = true;
        true
    });
    if replaced {
        for entry in entries.iter_mut() {
            if entry.0 == name {
                entry.1 = value;
                break;
            }
        }
    } else {
        entries.push((name, value));
    }
}

/// 删除全部同名项。
pub fn delete(entries: &mut Entries, name: &str) {
    entries.retain(|(key, _)| key != name);
}

/// 是否存在同名项。
pub fn has(entries: &Entries, name: &str) -> bool {
    entries.iter().any(|(key, _)| key == name)
}

/// 由外部熵源（如时钟纳秒）派生 multipart 分隔串。
///
/// 规范只要求分隔串足够随机且在内容中不出现；这里用 16 位十六进制的
/// 混淆展开，长度与不可预测性对 v1 场景够用。
#[must_use]
pub fn generate_boundary(seed: u64) -> String {
    let mut mixed = seed ^ (seed.rotate_left(23)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let mut out = String::from("----MioFormBoundary");
    for _ in 0..4 {
        mixed = mixed.rotate_left(7).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
        let _ = write!(out, "{:02x}", (mixed >> 32) as u8);
    }
    out
}

/// 编码 multipart/form-data 报文体（含结束分隔行，不含结尾 CRLF 之后的内容）。
#[must_use]
pub fn encode_multipart(entries: &Entries, boundary: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, value) in entries {
        out.extend_from_slice(b"--");
        out.extend_from_slice(boundary.as_bytes());
        out.extend_from_slice(b"\r\ncontent-disposition: form-data; name=\"");
        write_quoted(&mut out, &strip_crlf(name));
        match value {
            FieldValue::Text(text) => {
                out.extend_from_slice(b"\"\r\n\r\n");
                out.extend_from_slice(text.as_bytes());
            }
            FieldValue::File {
                filename,
                content_type,
                bytes,
            } => {
                out.extend_from_slice(b"\"; filename=\"");
                write_quoted(&mut out, &strip_crlf(filename));
                out.extend_from_slice(b"\"\r\ncontent-type: ");
                out.extend_from_slice(strip_crlf(content_type).as_bytes());
                out.extend_from_slice(b"\r\n\r\n");
                out.extend_from_slice(bytes);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"--");
    out.extend_from_slice(boundary.as_bytes());
    out.extend_from_slice(b"--\r\n");
    out
}

/// 写入 multipart 头部字段值：转义反斜杠与双引号（RFC 7578 quoted-string）。
fn write_quoted(out: &mut Vec<u8>, text: &str) {
    for ch in text.chars() {
        match ch {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            _ => {
                let mut buffer = [0u8; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
}

/// 剔除 CR/LF：字段名与文件名不允许携带换行，否则可注入额外头部。
fn strip_crlf(text: &str) -> String {
    text.chars()
        .filter(|ch| *ch != '\r' && *ch != '\n')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_text_field_with_exact_bytes() {
        let entries = vec![("a".to_string(), FieldValue::Text("1".to_string()))];
        let body = encode_multipart(&entries, "B");
        assert_eq!(
            String::from_utf8(body).expect("编码应始终是 UTF-8"),
            "--B\r\ncontent-disposition: form-data; name=\"a\"\r\n\r\n1\r\n--B--\r\n"
        );
    }

    #[test]
    fn encodes_file_field_with_content_type_and_raw_bytes() {
        let entries = vec![(
            "upload".to_string(),
            FieldValue::File {
                filename: "p.bin".to_string(),
                content_type: "application/octet-stream".to_string(),
                bytes: vec![0, 1, 2],
            },
        )];
        let body = encode_multipart(&entries, "X");
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("name=\"upload\"; filename=\"p.bin\""),
            "disposition 应携带文件名：{text}"
        );
        assert!(text.contains("\r\ncontent-type: application/octet-stream\r\n\r\n"));
        assert!(body.ends_with(b"\x00\x01\x02\r\n--X--\r\n"));
    }

    #[test]
    fn escapes_quotes_and_backslashes_in_names() {
        let entries = vec![("a\"b\\c".to_string(), FieldValue::Text("v".to_string()))];
        let body = encode_multipart(&entries, "B");
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("name=\"a\\\"b\\\\c\""),
            "引号与反斜杠应被转义：{text}"
        );
    }

    #[test]
    fn strips_crlf_from_field_names_to_prevent_header_injection() {
        let entries = vec![(
            "a\r\ncontent-x: y".to_string(),
            FieldValue::Text("v".to_string()),
        )];
        let body = encode_multipart(&entries, "B");
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("name=\"acontent-x: y\""),
            "换行应被剔除而非转义：{text}"
        );
        // 注入行不复现：content-x 只出现在引号内部。
        assert!(!text.contains("\r\ncontent-x: y\r\n"));
    }

    #[test]
    fn set_replaces_first_and_drops_the_rest() {
        let mut entries = vec![
            ("a".to_string(), FieldValue::Text("1".to_string())),
            ("b".to_string(), FieldValue::Text("2".to_string())),
            ("a".to_string(), FieldValue::Text("3".to_string())),
        ];
        set(
            &mut entries,
            "a".to_string(),
            FieldValue::Text("9".to_string()),
        );
        assert_eq!(get(&entries, "a"), Some(&FieldValue::Text("9".to_string())));
        assert_eq!(get_all(&entries, "a").len(), 1);
        assert!(has(&entries, "b"));
        delete(&mut entries, "b");
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn boundary_is_stable_and_varied() {
        let first = generate_boundary(0x0123_4567_89AB_CDEF);
        let second = generate_boundary(0xFEDC_BA98_7654_3210);
        assert!(first.starts_with("----MioFormBoundary"));
        assert_ne!(first, second, "不同种子应产生不同分隔串");
    }
}

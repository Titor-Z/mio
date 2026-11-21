//! `@std/encoding` 内建模块的纯 Rust 实现（引擎无关）。
//!
//! 只含算法：base64 与 hex 的编解码，输入输出都是字节，引擎层再包装成
//! `Uint8Array`（约定 1）。编码一律输出标准形态（base64 带 `=` 填充、hex 小写）；
//! 解码宽容（base64 允许省略填充与空白，hex 接受大小写），非法输入返回错误文本。

/// base64 字母表（RFC 4648 标准表）。
const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// base64 编码：3 字节一组，末尾按剩余字节补 `=`。
pub fn encode_base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0] as u32;
        let second = chunk.get(1).copied().unwrap_or(0) as u32;
        let third = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (first << 16) | (second << 8) | third;

        out.push(BASE64_ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        out.push(BASE64_ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        out.push(match chunk.len() {
            1 => '=',
            _ => BASE64_ALPHABET[(triple >> 6) as usize & 0x3f] as char,
        });
        out.push(match chunk.len() {
            3 => BASE64_ALPHABET[triple as usize & 0x3f] as char,
            _ => '=',
        });
    }
    out
}

/// base64 解码：忽略 ASCII 空白、允许省略末尾填充；非法字符或长度报错。
pub fn decode_base64(text: &str) -> Result<Vec<u8>, String> {
    let mut symbols: Vec<u32> = Vec::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'=' => break,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => match BASE64_VALUE[byte as usize] {
                0xff => {
                    return Err(format!("base64 含非法字符 `{}`", byte as char));
                }
                value => symbols.push(value as u32),
            },
        }
    }

    if symbols.len() % 4 == 1 {
        return Err(String::from("base64 长度非法：余 1 个字符"));
    }

    let mut out = Vec::with_capacity(symbols.len() / 4 * 3);
    for chunk in symbols.chunks(4) {
        let mut triple = 0u32;
        for (index, value) in chunk.iter().enumerate() {
            triple |= value << (18 - index * 6);
        }
        out.push((triple >> 16) as u8);
        if chunk.len() > 2 {
            out.push((triple >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(triple as u8);
        }
    }
    Ok(out)
}

/// 每个字节对应的 base64 值，`0xff` 表示非法。
const BASE64_VALUE: [u8; 256] = {
    let mut table = [0xffu8; 256];
    let mut index = 0usize;
    while index < 64 {
        table[BASE64_ALPHABET[index] as usize] = index as u8;
        index += 1;
    }
    table
};

/// hex 编码：每字节两位小写十六进制。
pub fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0f) as u32, 16).unwrap_or('0'));
    }
    out
}

/// hex 解码：接受大小写，长度必须为偶数。
pub fn decode_hex(text: &str) -> Result<Vec<u8>, String> {
    let digits: Vec<u8> = text.bytes().collect();
    if !digits.len().is_multiple_of(2) {
        return Err(String::from("hex 长度非法：必须为偶数"));
    }
    let mut out = Vec::with_capacity(digits.len() / 2);
    for pair in digits.chunks(2) {
        let high =
            hex_value(pair[0]).ok_or_else(|| format!("hex 含非法字符 `{}`", pair[0] as char))?;
        let low =
            hex_value(pair[1]).ok_or_else(|| format!("hex 含非法字符 `{}`", pair[1] as char))?;
        out.push((high << 4) | low);
    }
    Ok(out)
}

/// 单个 ASCII 字符的十六进制值。
fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encodes_known_vectors() {
        assert_eq!(encode_base64(b""), "");
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_decodes_known_vectors_and_round_trips() {
        assert_eq!(decode_base64("").unwrap(), b"");
        assert_eq!(decode_base64("Zg==").unwrap(), b"f");
        // 省略填充与含空白都应接受。
        assert_eq!(decode_base64("Zg").unwrap(), b"f");
        assert_eq!(decode_base64("Zm9v\nYmFy").unwrap(), b"foobar");
        for sample in [b"\x00\x01\x02".to_vec(), (0u8..=255).collect::<Vec<u8>>()] {
            assert_eq!(decode_base64(&encode_base64(&sample)).unwrap(), sample);
        }
    }

    #[test]
    fn base64_rejects_invalid_input() {
        assert!(decode_base64("Zm9v!").is_err());
        assert!(decode_base64("Z").is_err());
    }

    #[test]
    fn hex_round_trips_and_validates() {
        assert_eq!(encode_hex(b"foo"), "666f6f");
        assert_eq!(decode_hex("666F6F").unwrap(), b"foo");
        assert_eq!(
            decode_hex(&encode_hex(&[0x00, 0xff, 0x10])).unwrap(),
            vec![0x00, 0xff, 0x10]
        );
        assert!(decode_hex("abc").is_err());
        assert!(decode_hex("zz").is_err());
    }
}

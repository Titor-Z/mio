//! URL 查询串的纯逻辑：`URLSearchParams` 的配对语义（解析、序列化、增删改查、排序）。
//!
//! 编解码遵循 `application/x-www-form-urlencoded`：空格编为 `+`，解码时 `+` 还原为
//! 空格；这正是什么规范对 URLSearchParams 的要求，直接复用 `form_urlencoded`。

/// 解码后的参数对序列（保持插入顺序，允许重复键）。
pub type Pairs = Vec<(String, String)>;

/// 解析查询串（不含前导 `?`）。空串得到空表。
pub fn parse_query(query: &str) -> Pairs {
    form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}

/// 序列化为查询串（不含 `?`，无尾随 `=` 省略值分隔——与规范一致：一律 `k=v`）。
pub fn serialize_query(pairs: &Pairs) -> String {
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .finish()
}

/// 首个匹配键的值。
pub fn get<'a>(pairs: &'a Pairs, name: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// 全部匹配键的值（保持顺序）。
pub fn get_all(pairs: &Pairs, name: &str) -> Vec<String> {
    pairs
        .iter()
        .filter(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
        .collect()
}

/// 追加一对。
pub fn append(pairs: &mut Pairs, name: String, value: String) {
    pairs.push((name, value));
}

/// 规范 set：替换首个匹配并删除其余同名项；不存在则追加。
pub fn set(pairs: &mut Pairs, name: String, value: String) {
    let mut replaced = false;
    pairs.retain(|(key, _)| {
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
        for pair in pairs.iter_mut() {
            if pair.0 == name {
                pair.1 = value;
                break;
            }
        }
    } else {
        pairs.push((name, value));
    }
}

/// 删除全部同名项。
pub fn delete(pairs: &mut Pairs, name: &str) {
    pairs.retain(|(key, _)| key != name);
}

/// 是否存在同名项。
pub fn has(pairs: &Pairs, name: &str) -> bool {
    pairs.iter().any(|(key, _)| key == name)
}

/// 按键稳定排序（规范：按键的 UTF-16 码元序；v1 用字节序，ASCII 键完全一致）。
pub fn sort(pairs: &mut Pairs) {
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plus_as_space_and_percent_escapes() {
        let pairs = parse_query("a=x+y&b=%E4%B8%AD&c=");
        assert_eq!(
            pairs,
            vec![
                ("a".to_string(), "x y".to_string()),
                ("b".to_string(), "中".to_string()),
                ("c".to_string(), String::new()),
            ]
        );
    }

    #[test]
    fn serializes_space_as_plus() {
        let pairs = vec![("q".to_string(), "a b&c".to_string())];
        assert_eq!(serialize_query(&pairs), "q=a+b%26c");
    }

    #[test]
    fn set_replaces_first_and_drops_the_rest() {
        let mut pairs = parse_query("a=1&b=2&a=3");
        set(&mut pairs, "a".to_string(), "9".to_string());
        assert_eq!(get(&pairs, "a"), Some("9"));
        assert_eq!(get_all(&pairs, "a"), vec!["9".to_string()]);
        // b 保留、顺序不变。
        assert_eq!(pairs[0].0, "a");
        assert_eq!(pairs[1].0, "b");
    }

    #[test]
    fn delete_removes_all_and_has_reports() {
        let mut pairs = parse_query("a=1&a=2&b=3");
        delete(&mut pairs, "a");
        assert!(!has(&pairs, "a"));
        assert!(has(&pairs, "b"));
        assert_eq!(pairs.len(), 1);
    }

    #[test]
    fn sort_orders_by_key_stably() {
        let mut pairs = parse_query("b=2&a=1&b=1");
        sort(&mut pairs);
        let keys: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["a", "b", "b"]);
        // 稳定：同名键保持原相对顺序。
        assert_eq!(pairs[1].1, "2");
        assert_eq!(pairs[2].1, "1");
    }
}

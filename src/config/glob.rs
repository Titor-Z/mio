//! 通配模式匹配，用于 `include` / `exclude` / `files`。
//!
//! 支持三种语法，与 `tsconfig.json` 的常见用法一致：
//! - `*` 匹配单个路径段内的任意字符序列
//! - `?` 匹配单个字符
//! - `**` 作为独立路径段时匹配零个或多个路径段
//!
//! 以路径段为单位匹配而非整串正则，好处是 `**` 的语义清晰且不会误伤分隔符。

/// 编译后的通配模式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Glob {
    /// 按 `/` 切分后的路径段序列。
    segments: Vec<Segment>,
}

/// 单个路径段的匹配形式。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    /// `**`：匹配零个或多个路径段。
    GlobStar,
    /// 普通段：由字面字符、`*`、`?` 组成。
    Pattern(Vec<Token>),
}

/// 普通段内的单个匹配单元。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// 字面字符。
    Literal(char),
    /// `*`：匹配任意长度的字符序列（可以为空）。
    Star,
    /// `?`：匹配任意单个字符。
    Question,
}

impl Glob {
    /// 编译一个通配模式。
    ///
    /// 空模式返回错误：静默匹配空串只会掩盖配置错误。
    pub fn compile(pattern: &str) -> Result<Self, String> {
        let normalized = pattern.trim();
        if normalized.is_empty() {
            return Err(String::from("通配模式不能为空"));
        }

        let segments = normalized
            .split('/')
            .filter(|segment| !segment.is_empty() || normalized == "/")
            .map(Self::compile_segment)
            .collect();

        Ok(Self { segments })
    }

    /// 编译单个路径段。
    fn compile_segment(segment: &str) -> Segment {
        if segment == "**" {
            return Segment::GlobStar;
        }

        let tokens = segment
            .chars()
            .map(|character| match character {
                '*' => Token::Star,
                '?' => Token::Question,
                literal => Token::Literal(literal),
            })
            .collect();

        Segment::Pattern(tokens)
    }

    /// 判定该模式是否匹配给定路径。路径使用 `/` 分隔。
    pub fn matches(&self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
        let mut memo = rustc_hash::FxHashMap::default();
        self.match_from(0, 0, &parts, &mut memo)
    }

    /// 带记忆化的递归匹配，避免 `**` 造成指数级回溯。
    fn match_from(
        &self,
        segment_index: usize,
        path_index: usize,
        parts: &[&str],
        memo: &mut rustc_hash::FxHashMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(cached) = memo.get(&(segment_index, path_index)) {
            return *cached;
        }

        let result = self.compute_match(segment_index, path_index, parts, memo);
        memo.insert((segment_index, path_index), result);
        result
    }

    /// 计算单步匹配结果。
    fn compute_match(
        &self,
        segment_index: usize,
        path_index: usize,
        parts: &[&str],
        memo: &mut rustc_hash::FxHashMap<(usize, usize), bool>,
    ) -> bool {
        let Some(segment) = self.segments.get(segment_index) else {
            return path_index == parts.len();
        };

        match segment {
            Segment::GlobStar => {
                // `**` 可以选择不消费任何路径段，也可以消费一个后继续保持 `**` 状态
                if self.match_from(segment_index + 1, path_index, parts, memo) {
                    return true;
                }
                if path_index < parts.len() {
                    return self.match_from(segment_index, path_index + 1, parts, memo);
                }
                false
            }
            Segment::Pattern(tokens) => {
                let Some(part) = parts.get(path_index) else {
                    return false;
                };
                if !Self::match_tokens(tokens, part) {
                    return false;
                }
                self.match_from(segment_index + 1, path_index + 1, parts, memo)
            }
        }
    }

    /// 单个路径段内的通配匹配。
    fn match_tokens(tokens: &[Token], text: &str) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let mut memo = rustc_hash::FxHashMap::default();
        Self::match_tokens_from(tokens, 0, &chars, 0, &mut memo)
    }

    /// 带记忆化的段内匹配。
    fn match_tokens_from(
        tokens: &[Token],
        token_index: usize,
        chars: &[char],
        char_index: usize,
        memo: &mut rustc_hash::FxHashMap<(usize, usize), bool>,
    ) -> bool {
        if let Some(cached) = memo.get(&(token_index, char_index)) {
            return *cached;
        }

        let result = match tokens.get(token_index) {
            None => char_index == chars.len(),
            Some(Token::Literal(expected)) => match chars.get(char_index) {
                Some(actual) if actual == expected => {
                    Self::match_tokens_from(tokens, token_index + 1, chars, char_index + 1, memo)
                }
                _ => false,
            },
            Some(Token::Question) => {
                char_index < chars.len()
                    && Self::match_tokens_from(tokens, token_index + 1, chars, char_index + 1, memo)
            }
            Some(Token::Star) => {
                Self::match_tokens_from(tokens, token_index + 1, chars, char_index, memo)
                    || (char_index < chars.len()
                        && Self::match_tokens_from(
                            tokens,
                            token_index,
                            chars,
                            char_index + 1,
                            memo,
                        ))
            }
        };

        memo.insert((token_index, char_index), result);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches(pattern: &str, path: &str) -> bool {
        Glob::compile(pattern).expect("模式应合法").matches(path)
    }

    #[test]
    fn matches_exact_path() {
        assert!(matches("src/main.ts", "src/main.ts"));
        assert!(!matches("src/main.ts", "src/other.ts"));
    }

    #[test]
    fn matches_single_star_within_segment() {
        assert!(matches("src/*.ts", "src/main.ts"));
        assert!(!matches("src/*.ts", "src/deep/main.ts"));
    }

    #[test]
    fn matches_globstar_across_segments() {
        assert!(matches("src/**/*.ts", "src/main.ts"));
        assert!(matches("src/**/*.ts", "src/a/b/c.ts"));
        assert!(!matches("src/**/*.ts", "src/a/b/c.js"));
    }

    #[test]
    fn matches_leading_globstar() {
        assert!(matches("**/*_test.ts", "a/b/c_test.ts"));
        assert!(matches("**/*_test.ts", "c_test.ts"));
    }

    #[test]
    fn matches_question_mark() {
        assert!(matches("a?.ts", "ab.ts"));
        assert!(!matches("a?.ts", "abc.ts"));
    }

    #[test]
    fn matches_directory_pattern() {
        assert!(matches("dist/**", "dist/a/b.js"));
        assert!(!matches("dist/**", "src/a.js"));
    }

    #[test]
    fn rejects_empty_pattern() {
        assert!(Glob::compile("   ").is_err());
    }

    #[test]
    fn star_does_not_cross_segments() {
        assert!(!matches("*.ts", "a/b.ts"));
    }
}

//! 模块说明符分类。
//!
//! 分类结果决定解析路径：内建模块走注册表，其余走文件系统探测。
//! 使用枚举而非布尔标记，保证每种类别在 `match` 中都必须被显式处理。

/// 模块说明符的类别。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Specifier {
    /// 内建模块命名空间，例如 `@std/path`。
    Builtin { name: String },
    /// 相对路径，以 `./` 或 `../` 开头。
    Relative { raw: String },
    /// 绝对文件系统路径。
    Absolute { raw: String },
    /// `file:` URL。
    FileUrl { raw: String },
    /// 其他形式。本版本一律视为非法，并给出明确报错。
    Unsupported { raw: String },
}

impl Specifier {
    /// 依据说明符文本判定类别。
    pub fn classify(raw: &str) -> Self {
        if raw.starts_with("@std/") {
            return Self::Builtin {
                name: raw.to_string(),
            };
        }
        if raw.starts_with("./") || raw.starts_with("../") || raw == "." || raw == ".." {
            return Self::Relative {
                raw: raw.to_string(),
            };
        }
        if raw.starts_with("file:") {
            return Self::FileUrl {
                raw: raw.to_string(),
            };
        }
        if raw.starts_with('/') {
            return Self::Absolute {
                raw: raw.to_string(),
            };
        }
        Self::Unsupported {
            raw: raw.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_builtin() {
        assert_eq!(
            Specifier::classify("@std/fs"),
            Specifier::Builtin {
                name: String::from("@std/fs")
            }
        );
    }

    #[test]
    fn classifies_relative() {
        assert!(matches!(
            Specifier::classify("./lib.ts"),
            Specifier::Relative { .. }
        ));
    }

    #[test]
    fn classifies_bare_name_as_unsupported() {
        assert!(matches!(
            Specifier::classify("lodash"),
            Specifier::Unsupported { .. }
        ));
    }
}

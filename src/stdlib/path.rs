//! `@std/path` 内建模块的纯 Rust 实现（POSIX 语义子集）。
//!
//! 本文件只含与引擎无关的算法：字符串进、字符串出，便于在阶段 B 复用与独立单测。
//! 把这些函数包装成脚本可见的 `NativeFunction`（依赖 `boa_*`）发生在 `engine/loader.rs`，
//! 以满足依赖隔离约束。目标平台为类 Unix，因此分隔符固定为 `/`。

use super::BuiltinFunction;

/// 分隔符常量，作为脚本侧 `separator` 导出。
pub const SEPARATOR: &str = "/";

/// 拼接各段并归一化，忽略空段；结果为空时返回 `.`。
pub fn join(parts: &[&str]) -> String {
    let filtered: Vec<&str> = parts
        .iter()
        .copied()
        .filter(|part| !part.is_empty())
        .collect();
    normalize(&filtered.join(SEPARATOR))
}

/// 归一化路径：折叠重复斜杠、消除 `.` 与 `..`，保留绝对性；空输入返回 `.`。
pub fn normalize(path: &str) -> String {
    if path.is_empty() {
        return String::from(".");
    }

    let is_absolute = path.starts_with(SEPARATOR);
    let trailing_slash = path.ends_with(SEPARATOR);
    let mut stack: Vec<&str> = Vec::new();

    for part in path.split(SEPARATOR) {
        match part {
            "" | "." => {}
            ".." => {
                if is_absolute {
                    stack.pop();
                } else {
                    match stack.last() {
                        None | Some(&"..") => stack.push(part),
                        Some(_) => {
                            stack.pop();
                        }
                    }
                }
            }
            other => stack.push(other),
        }
    }

    let mut result = stack.join(SEPARATOR);
    if is_absolute {
        result.insert(0, '/');
    } else if result.is_empty() {
        result.push('.');
    }
    if trailing_slash && !result.ends_with(SEPARATOR) {
        result.push('/');
    }
    result
}

/// 目录名：去掉最后一段；无目录部分时返回 `.`（相对）或 `/`（绝对）。
pub fn dirname(path: &str) -> String {
    if path.is_empty() {
        return String::from(".");
    }

    let is_absolute = path.starts_with(SEPARATOR);
    let mut segments: Vec<&str> = path
        .split(SEPARATOR)
        .filter(|part| !part.is_empty())
        .collect();

    if segments.is_empty() {
        return if is_absolute {
            String::from("/")
        } else {
            String::from(".")
        };
    }

    segments.pop();
    let joined = segments.join(SEPARATOR);
    if is_absolute {
        format!("/{joined}")
    } else if joined.is_empty() {
        String::from(".")
    } else {
        joined
    }
}

/// 基名：取最后一段，去掉结尾斜杠；给定 `suffix` 且匹配时剥离它。
pub fn basename(path: &str, suffix: Option<&str>) -> String {
    let trimmed = path.trim_end_matches(SEPARATOR);
    let name = match trimmed.rsplit_once(SEPARATOR) {
        Some((_left, right)) => right,
        None => trimmed,
    };

    if let Some(suffix) = suffix.filter(|value| !value.is_empty())
        && name.len() > suffix.len()
        && name.ends_with(suffix)
    {
        return name[..name.len() - suffix.len()].to_string();
    }
    name.to_string()
}

/// 扩展名：返回基名最后一个点及其后缀；点文件、无点或仅尾点分别处理。
pub fn extname(path: &str) -> String {
    let name = basename(path, None);
    match name.rfind('.') {
        // 无扩展名（含 POSIX 点文件如 `.gitignore`）。
        None | Some(0) => String::new(),
        Some(index) => name[index..].to_string(),
    }
}

// ---- 变参包装：把脚本侧的字符串实参数组接到上面的算法，缺失实参时返回错误文本 ----

fn require_one<'a>(args: &[&'a str], function: &str) -> Result<&'a str, String> {
    if args.len() != 1 {
        return Err(format!("{function} 需要 1 个参数，实际 {}", args.len()));
    }
    Ok(args[0])
}

/// `path` 模块对外暴露的函数表。脚本名沿用 Deno `@std/path` 的 camelCase。
pub const FUNCTIONS: &[BuiltinFunction] = &[
    BuiltinFunction {
        name: "join",
        call: |args| Ok(join(args)),
    },
    BuiltinFunction {
        name: "normalize",
        call: |args| Ok(normalize(require_one(args, "normalize")?)),
    },
    BuiltinFunction {
        name: "dirname",
        call: |args| Ok(dirname(require_one(args, "dirname")?)),
    },
    BuiltinFunction {
        name: "basename",
        call: |args| match args.len() {
            1 => Ok(basename(args[0], None)),
            2 => Ok(basename(args[0], Some(args[1]))),
            other => Err(format!("basename 需要 1 或 2 个参数，实际 {other}")),
        },
    },
    BuiltinFunction {
        name: "extname",
        call: |args| Ok(extname(require_one(args, "extname")?)),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_concatenates_and_normalizes() {
        assert_eq!(join(&["foo", "bar", "baz"]), "foo/bar/baz");
        assert_eq!(join(&["foo/", "/bar"]), "foo/bar");
        assert_eq!(join(&[]), ".");
        assert_eq!(join(&["/a", "b/", "c"]), "/a/b/c");
    }

    #[test]
    fn normalize_resolves_dot_segments() {
        assert_eq!(normalize("/foo/bar//baz/asdf/quux/.."), "/foo/bar/baz/asdf");
        assert_eq!(normalize("foo/../.."), "..");
        assert_eq!(normalize("foo/../../out/file"), "../out/file");
        assert_eq!(normalize("/foo/../.."), "/");
        assert_eq!(normalize(""), ".");
    }

    #[test]
    fn dirname_matches_posix_semantics() {
        assert_eq!(dirname(""), ".");
        assert_eq!(dirname("/"), "/");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(dirname("/a/b/"), "/a");
        assert_eq!(dirname("a/b"), "a");
        assert_eq!(dirname("a"), ".");
    }

    #[test]
    fn basename_strips_trailing_slash_and_suffix() {
        assert_eq!(basename("/foo/bar/baz.js", None), "baz.js");
        assert_eq!(basename("/foo/bar/baz.js", Some(".js")), "baz");
        assert_eq!(basename("/foo/bar/", None), "bar");
        assert_eq!(basename("baz", Some("baz")), "baz"); // suffix 等于基名时不剥离
        assert_eq!(basename("/", None), "");
    }

    #[test]
    fn extname_handles_edge_cases() {
        assert_eq!(extname(""), "");
        assert_eq!(extname("index.html"), ".html");
        assert_eq!(extname("index.tar.gz"), ".gz");
        assert_eq!(extname(".gitignore"), "");
        assert_eq!(extname("index."), ".");
        assert_eq!(extname("path/to/file.txt"), ".txt");
    }
}

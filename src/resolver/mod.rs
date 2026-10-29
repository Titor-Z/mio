//! 模块解析层：把模块说明符（specifier）翻译成磁盘路径。
//!
//! 这是阶段 A 与阶段 B 之间的第二个接缝：[`Specifier`] 的分类规则
//! 在两个阶段完全一致，只有路径探测的细节可能不同。
//!
//! 支持范围刻意收窄，不含 Node.js 生态语义：
//! - 相对路径、绝对路径
//! - `file:` URL
//! - `@std/*` 内建命名空间
//! - Import Maps 中的前缀映射与精确映射
//!
//! 不支持 node_modules 查找、package.json 入口推断与 CommonJS 识别。

mod specifier;

use std::fmt;
use std::io;
use std::path::{Component, Path, PathBuf};

pub use specifier::Specifier;

/// 候选扩展名，按优先级排列。TS 项目常以 `.js` 说明符引用 `.ts` 文件，
/// 归一化后需能自动补全为磁盘上实际存在的扩展名。
const CANDIDATE_EXTENSIONS: &[&str] = &["ts", "mts", "cts", "tsx", "js", "mjs", "cjs"];

/// 模块解析失败。携带面向用户的中文消息；错误不区分「未映射」「不存在」，
/// 因为两者在使用者看来都是「mio 找不到这个说明符」。
#[derive(Debug)]
pub struct ResolveError {
    message: String,
}

impl ResolveError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ResolveError {}

impl From<io::Error> for ResolveError {
    fn from(error: io::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// 模块解析器：持有 Import Maps 与扩展名补全规则，负责产出磁盘路径。
#[derive(Debug, Default)]
pub struct ModuleResolver {
    /// 精确映射：说明符 → 目标说明符。
    exact: rustc_hash::FxHashMap<String, String>,
    /// 前缀映射：说明符前缀 → 目标前缀，键值均以 `/` 结尾。
    prefix: Vec<(String, String)>,
}

impl ModuleResolver {
    /// 注册一条精确映射，对应 Import Maps 中不带斜杠结尾的键。
    pub fn insert_exact(&mut self, from: impl Into<String>, to: impl Into<String>) {
        self.exact.insert(from.into(), to.into());
    }

    /// 注册一条前缀映射，对应 Import Maps 中带斜杠结尾的键。
    pub fn insert_prefix(&mut self, from: impl Into<String>, to: impl Into<String>) {
        self.prefix.push((from.into(), to.into()));
    }

    /// 按 Import Maps 规则重写说明符；无匹配时原样返回。
    ///
    /// 精确映射优先于前缀映射，前缀映射按最长前缀优先，与规范一致。
    pub fn rewrite(&self, specifier: &str) -> String {
        if let Some(target) = self.exact.get(specifier) {
            return target.clone();
        }

        let mut best: Option<(&str, &str)> = None;
        for (from, to) in &self.prefix {
            if !specifier.starts_with(from.as_str()) {
                continue;
            }
            let longer = best.is_none_or(|(current, _)| from.len() > current.len());
            if longer {
                best = Some((from.as_str(), to.as_str()));
            }
        }

        match best {
            Some((from, to)) => format!("{}{}", to, &specifier[from.len()..]),
            None => specifier.to_string(),
        }
    }

    /// 把说明符解析为磁盘上的实际文件绝对路径。
    ///
    /// 步骤：Import Maps 重写 → 相对/绝对判定 → 手工归一化（不依赖文件存在）
    /// → 依次探测「原样 / 补全候选扩展名 / 目录 index」→ `canonicalize` 稳定化。
    /// 裸说明符（无映射的非相对非绝对路径）当前不支持，与规范文档一致。
    pub fn resolve(
        &self,
        specifier: &str,
        referrer: Option<&Path>,
    ) -> Result<PathBuf, ResolveError> {
        let rewritten = self.rewrite(specifier);
        let target = Path::new(&rewritten);
        let is_relative = rewritten.starts_with("./") || rewritten.starts_with("../");

        let joined = if is_relative {
            let dir = referrer.and_then(|path| path.parent()).ok_or_else(|| {
                ResolveError::new(format!("相对说明符 `{specifier}` 缺少引用者路径"))
            })?;
            dir.join(target)
        } else if target.is_absolute() {
            target.to_path_buf()
        } else {
            return Err(ResolveError::new(format!(
                "无法解析裸说明符 `{specifier}`（当前仅支持相对路径与绝对路径）"
            )));
        };

        let normalized = normalize(&joined)?;
        let found = probe(&normalized)
            .ok_or_else(|| ResolveError::new(format!("找不到 `{specifier}` 对应的文件")))?;
        std::fs::canonicalize(&found)
            .map_err(|_| ResolveError::new(format!("{} 规范化失败", found.display())))
    }
}

/// 手工归一化：消除 `.` 与 `..`，不要求路径存在（存在性由后续探测确认）。
fn normalize(path: &Path) -> Result<PathBuf, ResolveError> {
    let mut acc = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !acc.pop() {
                    return Err(ResolveError::new("解析结果越过模块根"));
                }
            }
            other => acc.push(other),
        }
    }
    Ok(acc)
}

/// 在候选扩展名与目录 index 中找出实际存在的文件；找不到返回 `None`。
fn probe(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    for extension in CANDIDATE_EXTENSIONS {
        let candidate = path.with_extension(extension);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if path.is_dir() {
        for extension in CANDIDATE_EXTENSIONS {
            let index = path.join(format!("index.{extension}"));
            if index.is_file() {
                return Some(index);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 创建一个带唯一后缀的临时目录，避免并发测试相互干扰。
    fn temp_dir(tag: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        dir.push(format!("mio-resolve-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        dir
    }

    #[test]
    fn rewrite_uses_exact_mapping() {
        let mut resolver = ModuleResolver::default();
        resolver.insert_exact("@std/path", "mio:path");
        assert_eq!(resolver.rewrite("@std/path"), "mio:path");
    }

    #[test]
    fn rewrite_uses_longest_prefix() {
        let mut resolver = ModuleResolver::default();
        resolver.insert_prefix("@app/", "./src/");
        resolver.insert_prefix("@app/deep/", "./src/deep/");
        assert_eq!(resolver.rewrite("@app/deep/x.ts"), "./src/deep/x.ts");
        assert_eq!(resolver.rewrite("@app/y.ts"), "./src/y.ts");
    }

    #[test]
    fn rewrite_keeps_unknown_specifier() {
        let resolver = ModuleResolver::default();
        assert_eq!(resolver.rewrite("./local.ts"), "./local.ts");
    }

    #[test]
    fn resolve_fills_extension_and_maps_js_to_ts() {
        let dir = temp_dir("ext");
        std::fs::write(dir.join("greet.ts"), "export const x = 1;").unwrap();
        let resolver = ModuleResolver::default();
        let referrer = dir.join("main.ts");

        let without_ext = resolver.resolve("./greet", Some(&referrer)).unwrap();
        assert!(without_ext.ends_with("greet.ts"));

        // TS 项目常以 `.js` 说明符引用 `.ts` 文件，应能自动映射。
        let js_spec = resolver.resolve("./greet.js", Some(&referrer)).unwrap();
        assert!(js_spec.ends_with("greet.ts"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_directory_index() {
        let dir = temp_dir("index");
        std::fs::create_dir_all(dir.join("math")).unwrap();
        std::fs::write(dir.join("math/index.ts"), "export const add = 1;").unwrap();
        let resolver = ModuleResolver::default();
        let got = resolver
            .resolve("./math", Some(&dir.join("main.ts")))
            .unwrap();
        assert!(got.ends_with("math/index.ts"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_rejects_bare_specifier() {
        let resolver = ModuleResolver::default();
        assert!(
            resolver
                .resolve("lodash", Some(Path::new("/tmp/main.ts")))
                .is_err()
        );
    }

    #[test]
    fn resolve_applies_import_map_prefix() {
        let dir = temp_dir("map");
        std::fs::write(dir.join("vendor.ts"), "export const v = 1;").unwrap();
        let mut resolver = ModuleResolver::default();
        resolver.insert_prefix("@app/", format!("{}/", dir.display()));
        let got = resolver
            .resolve("@app/vendor", Some(&dir.join("main.ts")))
            .unwrap();
        assert!(got.ends_with("vendor.ts"));
        std::fs::remove_dir_all(&dir).ok();
    }
}

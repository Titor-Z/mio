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
//!
//! 两条解析路径需严格区分，这是 Import Maps 语义的关键：
//! - **命中映射**：Import Maps 的目标地址按规范相对**基准目录**（配置文件所在目录）解析。
//! - **未命中映射**：说明符按书写者意图，相对**引用者文件**所在目录解析。
//!
//! 二者基准不同，混淆会让别名导入指向错误位置。

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

/// 模块解析器：持有 Import Maps、基准目录与扩展名补全规则，负责产出磁盘路径。
#[derive(Debug, Default)]
pub struct ModuleResolver {
    /// 精确映射：说明符 → 目标说明符。
    exact: rustc_hash::FxHashMap<String, String>,
    /// 前缀映射：说明符前缀 → 目标前缀，键值均以 `/` 结尾。
    prefix: Vec<(String, String)>,
    /// Import Maps 目标相对解析的基准目录（通常是配置文件所在目录）。
    base: PathBuf,
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

    /// 设定 Import Maps 目标相对解析的基准目录，返回自身以便链式调用。
    #[must_use]
    pub fn with_base(mut self, base: PathBuf) -> Self {
        self.base = base;
        self
    }

    /// 命中映射时返回重写后的目标地址，否则返回 `None`。
    ///
    /// 精确映射优先于前缀映射，前缀映射按最长前缀优先，与 Import Maps 规范一致。
    fn matched(&self, specifier: &str) -> Option<String> {
        if let Some(target) = self.exact.get(specifier) {
            return Some(target.clone());
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

        best.map(|(from, to)| format!("{}{}", to, &specifier[from.len()..]))
    }

    /// 按 Import Maps 规则重写说明符；无匹配时原样返回。
    pub fn rewrite(&self, specifier: &str) -> String {
        self.matched(specifier)
            .unwrap_or_else(|| specifier.to_string())
    }

    /// 把说明符解析为磁盘上的实际文件绝对路径。
    ///
    /// 步骤：判定是否命中 Import Maps →
    /// - 命中：目标相对**基准目录**解析（相对/绝对/内建命名空间）；
    /// - 未命中：说明符相对**引用者目录**解析（仅相对/绝对）。
    ///
    /// 随后统一做「手工归一化 → 扩展名补全 / 目录 index 探测 → `canonicalize`」。
    pub fn resolve(
        &self,
        specifier: &str,
        referrer: Option<&Path>,
    ) -> Result<PathBuf, ResolveError> {
        let joined = match self.matched(specifier) {
            Some(target) => self.mapped_to_path(&target)?,
            None => self.plain_to_path(specifier, referrer)?,
        };

        let normalized = normalize(&joined)?;
        let found = probe(&normalized)
            .ok_or_else(|| ResolveError::new(format!("找不到 `{specifier}` 对应的文件")))?;
        std::fs::canonicalize(&found)
            .map_err(|_| ResolveError::new(format!("{} 规范化失败", found.display())))
    }

    /// Import Maps 目标地址的解析：相对基准目录，绝对路径原样，`@std/*` 暂不支持。
    fn mapped_to_path(&self, target: &str) -> Result<PathBuf, ResolveError> {
        let path = Path::new(target);
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }
        if target.starts_with('@') {
            // 内建命名空间需要 stdlib 接入，本增量尚未提供，明确报错而非误指向磁盘。
            return Err(ResolveError::new(format!(
                "内建命名空间 `{target}` 尚未接入"
            )));
        }
        Ok(self.base.join(path))
    }

    /// 未被映射的说明符解析：相对路径基于引用者目录，绝对路径原样，其余（裸名）不支持。
    fn plain_to_path(
        &self,
        specifier: &str,
        referrer: Option<&Path>,
    ) -> Result<PathBuf, ResolveError> {
        let path = Path::new(specifier);
        let is_relative = specifier.starts_with("./") || specifier.starts_with("../");

        if is_relative {
            let dir = referrer.and_then(|p| p.parent()).ok_or_else(|| {
                ResolveError::new(format!("相对说明符 `{specifier}` 缺少引用者路径"))
            })?;
            Ok(dir.join(path))
        } else if path.is_absolute() {
            Ok(path.to_path_buf())
        } else {
            Err(ResolveError::new(format!(
                "无法解析裸说明符 `{specifier}`（当前仅支持相对路径与绝对路径）"
            )))
        }
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
    fn resolve_import_map_target_is_relative_to_base_not_referrer() {
        // 基准目录（模拟配置文件所在）与引用者目录不同：映射目标 ./lib/ 必须相对 base 解析。
        let base = temp_dir("base");
        let referrer_dir = temp_dir("referrer");
        std::fs::create_dir_all(base.join("lib")).unwrap();
        std::fs::write(base.join("lib/vendor.ts"), "export const v = 1;").unwrap();

        let mut resolver = ModuleResolver::default().with_base(base.clone());
        resolver.insert_prefix("@app/", "./lib/");

        let got = resolver
            .resolve("@app/vendor", Some(&referrer_dir.join("main.ts")))
            .unwrap();
        assert!(
            got.ends_with("lib/vendor.ts"),
            "应命中 base 下的 vendor.ts，实得 {got:?}"
        );

        std::fs::remove_dir_all(&base).ok();
        std::fs::remove_dir_all(&referrer_dir).ok();
    }

    #[test]
    fn resolve_maps_prefix_to_absolute_target() {
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

    #[test]
    fn resolve_rejects_unsupported_std_namespace() {
        let mut resolver = ModuleResolver::default();
        resolver.insert_exact("@std/path", "@std/path");
        let error = resolver
            .resolve("@std/path", Some(Path::new("/tmp/main.ts")))
            .expect_err("@std/* 尚未接入应报错");
        assert!(error.to_string().contains("尚未接入"));
    }
}

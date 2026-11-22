//! 远程模块下载缓存（设计记录见 `docs/Taolun.md` 的 `TL20261123T053153`）。
//!
//! 设计要点：
//! - URL 即模块身份，缓存只是可丢弃的物化结果；
//! - 不设索引文件，路径编码承担索引职责（`<scheme>/<host>/<name>_<hash8>.<ext>`）；
//! - 缓存根定死 `~/.mio/deps/`，不提供环境变量与配置覆盖；
//! - 默认 cache-first（命中即返回、永不主动问网络），重下与校验必须显式开关。
//!
//! 「判断过期」在此处被刻意降级为「不判断」：TTL 会让同一份代码昨天跑通、今天因缓存
//! 过期叠加上游不可用而跑不通，与项目对确定性的承诺冲突。缓存不新鲜可以接受，
//! 不可复现不可以接受。

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use super::lock::LockFile;
use crate::web::fetch::{FetchRequest, fetch_execute};

/// 单模块体积上限（10 MiB）。超出即报错，不做截断。
pub const MAX_BYTES: usize = 10 * 1024 * 1024;

/// 视为源码的响应类型白名单（前缀匹配）。仅在 URL 末段看不出是源码时才用。
///
/// 白名单不能无条件启用：`python3 -m http.server` 等简单服务器会把 `.ts` 当成
/// `video/mp2t`（MPEG 传输流）返回，若照白名单拒掉，最常见的自测环境就用不了。
pub const TEXTUAL_TYPES: &[&str] = &[
    "text/",
    "application/javascript",
    "application/typescript",
    "application/x-javascript",
    "application/ecmascript",
    "application/octet-stream",
];

/// 能从 URL 末段直接认出是源码的扩展名；命中时不再看 `content-type`。
const SOURCE_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "mjs", "cjs", "jsx"];

/// 缓存根目录名（定死，不可配置），位于用户主目录之下。
const CACHE_DIR_NAME: &str = "deps";

/// 远程缓存根目录：`~/.mio/deps/`。
///
/// 只在取主目录时读取 `HOME`（Windows 回退 `USERPROFILE`）——测试据此把 `HOME`
/// 指向临时目录隔离，产品侧不提供 `MIO_CACHE_DIR` 之类的后门。
pub fn cache_root() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .ok_or_else(|| String::from("无法确定用户主目录，远程模块缓存不可用"))?;
    Ok(PathBuf::from(home).join(".mio").join(CACHE_DIR_NAME))
}

/// 重下策略。默认 [`Reload::CacheFirst`]：命中即返回。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Reload {
    /// 缓存优先，永不主动问网络。
    #[default]
    CacheFirst,
    /// 全量重下（`--reload`）。
    All,
    /// 仅重下指定 URL（`--reload=<url>`）。
    Url(String),
}

/// 远程层的运行期选项，由命令行开关装配。
#[derive(Debug, Clone, Default)]
pub struct RemoteOptions {
    /// 重下策略。
    pub reload: Reload,
    /// 是否走条件请求（`--revalidate`），304 只更新元数据。
    pub revalidate: bool,
    /// 是否离线严格（`--cached-only`），未命中即报错。
    pub cached_only: bool,
    /// 是否只校验锁文件不写入（`--frozen`）。
    pub frozen: bool,
    /// 锁文件路径；`None` 表示本次运行不读写锁文件。
    pub lock_path: Option<PathBuf>,
}

/// 一份缓存元数据，与源码文件同目录、同名、扩展名 `.json`。
///
/// `etag` / `last_modified` 供条件请求使用，`sha256` 供 `mio.lock` 复用。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheMeta {
    /// 原始 URL（重定向后的落地地址不覆盖它）。
    pub url: String,
    /// 响应状态码。
    pub status: u16,
    /// 响应 `content-type`，缺省为 `None`。
    pub content_type: Option<String>,
    /// 响应 `etag`，用于 `If-None-Match`。
    pub etag: Option<String>,
    /// 响应 `last-modified`，用于 `If-Modified-Since`。
    pub last_modified: Option<String>,
    /// 抓取时间（Unix 秒），仅用于提示，不参与任何判定。
    pub fetched_at: u64,
    /// 内容 sha256（十六进制小写）。
    pub sha256: String,
}

/// 远程模块缓存：负责「说明符 → URL」与「URL → 本地文件」两段映射。
pub struct RemoteCache {
    /// 缓存根目录 `~/.mio/deps/`。
    root: PathBuf,
    /// 运行期选项。
    options: RemoteOptions,
    /// 反查表：缓存文件 → 原始 URL。远程模块内部的相对 import 据此做 URL join。
    urls_by_path: RefCell<rustc_hash::FxHashMap<PathBuf, String>>,
    /// 锁文件内容（`lock_path` 为 `None` 时保持空表）。
    lock: RefCell<LockFile>,
    /// 锁文件是否被本次运行改动。
    lock_dirty: Cell<bool>,
}

impl RemoteCache {
    /// 装配缓存层；主目录不可用或锁文件损坏时返回错误。
    pub fn new(options: RemoteOptions) -> Result<Self, String> {
        let lock = match &options.lock_path {
            Some(path) => LockFile::load(path)?,
            None => LockFile::default(),
        };
        Ok(Self {
            root: cache_root()?,
            options,
            urls_by_path: RefCell::new(rustc_hash::FxHashMap::default()),
            lock: RefCell::new(lock),
            lock_dirty: Cell::new(false),
        })
    }

    /// 收尾：把本次登记结果写回锁文件。返回被写入的路径（未写则为 `None`）。
    ///
    /// `--frozen` 下永不写；无远程模块或内容无变化时也不写。
    pub fn finish(&self) -> Result<Option<PathBuf>, String> {
        if self.options.frozen || !self.lock_dirty.get() {
            return Ok(None);
        }
        let Some(path) = &self.options.lock_path else {
            return Ok(None);
        };
        let written = self.lock.borrow().save(path)?;
        Ok(written.then(|| path.clone()))
    }

    /// 缓存根目录。
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 说明符 → 目标 URL。
    ///
    /// 绝对 `http(s)://` 说明符直接采用；否则要求引用者本身是远程模块，
    /// 按 URL 规则 join（`./`、`../`、`/x.ts` 都按 RFC 3986 处理）。
    pub fn locate(&self, specifier: &str, referrer: Option<&Path>) -> Option<String> {
        if let Some(url) = remote_url(specifier) {
            return Some(url);
        }
        let base = referrer.and_then(|path| self.urls_by_path.borrow().get(path).cloned())?;
        Url::parse(&base)
            .ok()?
            .join(specifier)
            .ok()
            .map(|joined| joined.to_string())
    }

    /// 把 URL 物化为本地缓存文件路径（命中直接返回，未命中才下载）。
    ///
    /// 返回规范化后的绝对路径：引擎入口会把路径 `canonicalize` 一次，两边必须落在
    /// 同一个键上，否则入口模块里的相对 import 查不到反查表。
    pub fn materialize(&self, url: &str) -> Result<PathBuf, String> {
        let path = self.cache_path(url)?;

        if !self.wants_reload(url) && path.is_file() {
            let path = self.remember(&path, url);
            self.settle_lock(url, &path)?;
            return Ok(path);
        }
        if self.options.cached_only {
            return Err(format!(
                "`--cached-only` 下远程模块未被缓存：{url}（去掉该开关或先正常运行一次）"
            ));
        }
        let path = self.remember(&self.download(url, &path)?, url);
        self.settle_lock(url, &path)?;
        Ok(path)
    }

    /// 登记「缓存文件 → URL」反查关系，返回规范化路径。
    fn remember(&self, path: &Path, url: &str) -> PathBuf {
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        self.urls_by_path
            .borrow_mut()
            .insert(canonical.clone(), url.to_string());
        canonical
    }

    /// 用缓存文件的实际内容与锁文件对账。
    ///
    /// 哈希从文件重新计算而非信任元数据，因为缓存文件可能被外部改动。
    fn settle_lock(&self, url: &str, path: &Path) -> Result<(), String> {
        if self.options.lock_path.is_none() {
            return Ok(());
        }
        let bytes = std::fs::read(path)
            .map_err(|error| format!("无法读取缓存文件 {}：{error}", path.display()))?;
        let digest = sha256_hex(&bytes);
        if self.options.frozen {
            return self.lock.borrow().verify(url, &digest);
        }
        if self.lock.borrow_mut().record(url, &digest) {
            self.lock_dirty.set(true);
        }
        Ok(())
    }

    /// 缓存文件对应的原始 URL（用于把报错里的缓存路径换回 URL）。
    pub fn display(&self, path: &Path) -> Option<String> {
        self.urls_by_path.borrow().get(path).cloned()
    }

    /// 本次运行是否应重下该 URL。
    fn wants_reload(&self, url: &str) -> bool {
        match &self.options.reload {
            Reload::CacheFirst => false,
            Reload::All => true,
            Reload::Url(target) => target == url,
        }
    }

    /// 下载并原子落盘（源码先写、元数据后写：元数据存在即代表这一条完整）。
    fn download(&self, url: &str, path: &Path) -> Result<PathBuf, String> {
        let meta_path = path.with_extension("json");
        let previous = read_meta(&meta_path);

        let mut request = FetchRequest::new(url.to_string());
        if self.options.revalidate
            && let Some(meta) = &previous
        {
            if let Some(etag) = &meta.etag {
                request
                    .headers
                    .push((String::from("if-none-match"), etag.clone()));
            }
            if let Some(modified) = &meta.last_modified {
                request
                    .headers
                    .push((String::from("if-modified-since"), modified.clone()));
            }
        }

        let response = fetch_execute(&request)?;

        // 304：内容未变，只刷新抓取时间。
        if response.status == 304 {
            if let Some(mut meta) = previous {
                meta.fetched_at = now_seconds();
                write_atomic(
                    &meta_path,
                    &serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?,
                )?;
                return Ok(path.to_path_buf());
            }
            return Err(format!("远程模块 `{url}` 返回 304 但本地无元数据"));
        }

        if !response.is_ok() {
            return Err(format!(
                "远程模块 `{url}` 下载失败：HTTP {} {}",
                response.status, response.status_text
            ));
        }

        if response.body.len() > MAX_BYTES {
            return Err(format!(
                "远程模块 `{url}` 体积超出上限（{} 字节 > {MAX_BYTES} 字节）",
                response.body.len()
            ));
        }

        let content_type = response.header("content-type").map(str::to_string);
        if let Some(kind) = &content_type
            && !looks_like_source(url)
            && !is_textual(kind)
        {
            return Err(format!("远程模块 `{url}` 的响应类型不是源码：`{kind}`"));
        }

        // 以下三项需在 body 被移走前取走。
        let status = response.status;
        let etag = response.header("etag").map(str::to_string);
        let last_modified = response.header("last-modified").map(str::to_string);

        let text = String::from_utf8(response.body)
            .map_err(|_| format!("远程模块 `{url}` 的响应不是合法的 UTF-8 文本"))?;

        // 扩展名已由路径编码归一，源码文件先落盘。
        write_atomic(path, text.as_bytes())?;

        let meta = CacheMeta {
            url: url.to_string(),
            status,
            content_type,
            etag,
            last_modified,
            fetched_at: now_seconds(),
            sha256: sha256_hex(text.as_bytes()),
        };
        write_atomic(
            &meta_path,
            &serde_json::to_vec_pretty(&meta).map_err(|error| error.to_string())?,
        )?;

        Ok(path.to_path_buf())
    }

    /// URL → 缓存文件路径：`<root>/<scheme>/<host>[_port]/<name>_<hash8>.<ext>`。
    fn cache_path(&self, url: &str) -> Result<PathBuf, String> {
        let parsed =
            Url::parse(url).map_err(|error| format!("非法远程模块 URL `{url}`：{error}"))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| format!("远程模块 URL 缺少主机名：`{url}`"))?;
        let host = match parsed.port() {
            Some(port) => format!("{host}_{port}"),
            None => host.to_string(),
        };
        let last_segment = parsed
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .unwrap_or("");

        let (stem, extension) = split_name(last_segment);
        let hash = &sha256_hex(url.as_bytes())[..8];
        Ok(self
            .root
            .join(parsed.scheme())
            .join(host)
            .join(format!("{stem}_{hash}.{extension}")))
    }
}

/// 判断说明符是否为绝对 `http(s)` URL，是则归一化后返回。
pub fn remote_url(specifier: &str) -> Option<String> {
    if !(specifier.starts_with("http://") || specifier.starts_with("https://")) {
        return None;
    }
    Url::parse(specifier).ok().map(|url| url.to_string())
}

/// 响应类型是否为可当源码用的文本类型（前缀匹配）。
fn is_textual(content_type: &str) -> bool {
    let kind = content_type.to_ascii_lowercase();
    TEXTUAL_TYPES
        .iter()
        .any(|allowed| kind.starts_with(allowed))
}

/// URL 末段是否带得上源码扩展名（忽略 query 与 fragment）。
fn looks_like_source(url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    extension.is_some_and(|extension| {
        let lower = extension.to_ascii_lowercase();
        SOURCE_EXTENSIONS.contains(&lower.as_str())
    })
}

/// URL 末段 → (文件名主干, 扩展名)。
///
/// 扩展名只认 `.tsx`（影响 OXC 的 `SourceType`），其余一律落成 `.ts`——
/// 若保留 `.js`，含类型标注的源码会被当作 JS 语法报错。
fn split_name(last_segment: &str) -> (String, &'static str) {
    let (stem, extension) = match last_segment.rsplit_once('.') {
        Some((stem, extension)) => (stem, extension),
        None => (last_segment, ""),
    };
    let stem = sanitize(if stem.is_empty() { "index" } else { stem });
    let extension = if extension.eq_ignore_ascii_case("tsx") {
        "tsx"
    } else {
        "ts"
    };
    (stem, extension)
}

/// 文件名主干白名单化：只保留字母、数字、`.`、`-`、`_`，并截断到 60 字符。
fn sanitize(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    if cleaned.is_empty() {
        String::from("index")
    } else {
        cleaned
    }
}

/// 内容 sha256（十六进制小写），供缓存取名与 `mio.lock` 复用。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 读取一份元数据；缺失或损坏都视为「没有元数据」。
fn read_meta(path: &Path) -> Option<CacheMeta> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 原子写入：先写本进程独占的 `.tmp`，再 `rename` 顶替目标文件。
fn write_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建缓存目录 {}：{error}", parent.display()))?;
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, content)
        .map_err(|error| format!("无法写入缓存临时文件 {}：{error}", temp.display()))?;
    std::fs::rename(&temp, path)
        .map_err(|error| format!("无法落盘缓存文件 {}：{error}", path.display()))
}

/// 当前 Unix 秒。
fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个指向临时主目录的缓存层。
    fn cache_with(options: RemoteOptions) -> (RemoteCache, PathBuf) {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "mio-remote-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cache = RemoteCache {
            root: dir.clone(),
            options,
            urls_by_path: RefCell::new(rustc_hash::FxHashMap::default()),
            lock: RefCell::new(LockFile::default()),
            lock_dirty: Cell::new(false),
        };
        (cache, dir)
    }

    #[test]
    fn cache_path_encodes_scheme_host_and_hash() {
        let (cache, dir) = cache_with(RemoteOptions::default());
        let path = cache
            .cache_path("https://Example.com/lib/mod.ts?v=1")
            .unwrap();
        assert!(path.starts_with(dir.join("https").join("example.com")));
        assert!(path.to_string_lossy().ends_with(".ts"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cache_path_keeps_port_and_tsx_extension() {
        let (cache, dir) = cache_with(RemoteOptions::default());
        let path = cache
            .cache_path("http://localhost:8787/app/view.tsx")
            .unwrap();
        assert!(path.starts_with(dir.join("http").join("localhost_8787")));
        assert!(path.to_string_lossy().ends_with(".tsx"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn js_extension_is_normalized_to_ts() {
        let (cache, dir) = cache_with(RemoteOptions::default());
        let path = cache.cache_path("https://a.com/x.js").unwrap();
        assert!(path.to_string_lossy().ends_with(".ts"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn locate_joins_relative_against_referrer_url() {
        let (cache, dir) = cache_with(RemoteOptions::default());
        let referrer = dir.join("mod_ab12cd34.ts");
        // 模拟一次已登记的物化结果。
        cache
            .urls_by_path
            .borrow_mut()
            .insert(referrer.clone(), String::from("https://a.com/lib/mod.ts"));

        assert_eq!(
            cache.locate("./util.ts", Some(&referrer)).as_deref(),
            Some("https://a.com/lib/util.ts")
        );
        assert_eq!(
            cache.locate("../shared/x.ts", Some(&referrer)).as_deref(),
            Some("https://a.com/shared/x.ts")
        );
        assert_eq!(
            cache
                .locate("https://b.com/y.ts", Some(&referrer))
                .as_deref(),
            Some("https://b.com/y.ts")
        );
        // 本地引用者不在反查表里 → 交回本地解析。
        assert_eq!(
            cache.locate("./util.ts", Some(Path::new("/tmp/local.ts"))),
            None
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sha256_is_stable_hex() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

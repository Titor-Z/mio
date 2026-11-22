//! 远程模块完整性锁文件 `mio.lock`（设计记录见 `docs/Taolun.md` 的 `TL20261123T053153`）。
//!
//! 定位：三层职责里的 **C 层（完整性层）**。缓存目录（B 层）是可丢弃的本地态，
//! 锁文件是不可丢弃的团队契约，因此两者分开存放。
//!
//! 只放不可变事实：URL 与内容 sha256。时间戳、ETag、重定向终址一律不进锁文件——
//! 它们易变，写进来只会产生 diff 噪音。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// 锁文件名（定死）。
pub const LOCK_FILE_NAME: &str = "mio.lock";

/// 锁文件当前版本。
const LOCK_VERSION: u32 = 1;

/// `mio.lock` 的内容：版本号 + 有序的「URL → 内容哈希」表。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockFile {
    /// 结构版本，便于将来迁移。
    pub version: u32,
    /// URL → `sha256-<hex>`；`BTreeMap` 保证键序稳定、diff 稳定。
    pub remote: BTreeMap<String, String>,
}

impl Default for LockFile {
    fn default() -> Self {
        Self {
            version: LOCK_VERSION,
            remote: BTreeMap::new(),
        }
    }
}

impl LockFile {
    /// 读取锁文件；文件不存在视为空表，存在但损坏则报错（不静默覆盖用户的契约）。
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.is_file() {
            return Ok(Self::default());
        }
        let raw = std::fs::read_to_string(path)
            .map_err(|error| format!("无法读取 {}：{error}", path.display()))?;
        serde_json::from_str(&raw).map_err(|error| format!("{} 解析失败：{error}", path.display()))
    }

    /// 登记一条「URL → 哈希」，返回表是否有变化。
    pub fn record(&mut self, url: &str, sha256: &str) -> bool {
        let value = format!("sha256-{sha256}");
        match self.remote.get(url) {
            Some(existing) if existing == &value => false,
            _ => {
                self.remote.insert(url.to_string(), value);
                true
            }
        }
    }

    /// 校验一条已登记条目与实测哈希是否一致；缺少该 URL 也算不一致（`--frozen` 下即失败）。
    pub fn verify(&self, url: &str, sha256: &str) -> Result<(), String> {
        let expected = self
            .remote
            .get(url)
            .ok_or_else(|| format!("{LOCK_FILE_NAME} 未登记远程模块：{url}"))?;
        let actual = format!("sha256-{sha256}");
        if expected == &actual {
            return Ok(());
        }
        Err(format!(
            "{LOCK_FILE_NAME} 与远程模块内容不一致：{url}\n  锁文件：{expected}\n  实际值：{actual}\n\
             确认上游变更后，去掉 --frozen 运行一次即可更新锁文件"
        ))
    }

    /// 落盘：内容与磁盘完全一致时跳过写入，避免无意义的 mtime 抖动。
    pub fn save(&self, path: &Path) -> Result<bool, String> {
        let serialized = serde_json::to_string_pretty(self)
            .map_err(|error| format!("{LOCK_FILE_NAME} 序列化失败：{error}"))?;
        let serialized = format!("{serialized}\n");
        if std::fs::read_to_string(path).is_ok_and(|existing| existing == serialized) {
            return Ok(false);
        }
        std::fs::write(path, serialized)
            .map_err(|error| format!("无法写入 {}：{error}", path.display()))?;
        Ok(true)
    }

    /// 表是否为空（无远程模块时不必生成锁文件）。
    pub fn is_empty(&self) -> bool {
        self.remote.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 临时目录下的锁文件路径。
    fn temp_lock(tag: &str) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "mio-lock-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(LOCK_FILE_NAME)
    }

    #[test]
    fn record_then_verify_passes() {
        let mut lock = LockFile::default();
        assert!(lock.record("https://a.com/x.ts", "abc"));
        assert!(!lock.record("https://a.com/x.ts", "abc"));
        lock.verify("https://a.com/x.ts", "abc").expect("应通过");
        assert!(lock.verify("https://a.com/x.ts", "def").is_err());
        assert!(lock.verify("https://a.com/y.ts", "abc").is_err());
    }

    #[test]
    fn save_and_load_roundtrip() {
        let path = temp_lock("roundtrip");
        let mut lock = LockFile::default();
        lock.record("https://b.com/z.ts", "0011");
        assert!(lock.save(&path).unwrap());
        // 内容未变时二次保存不写盘。
        assert!(!lock.save(&path).unwrap());
        let loaded = LockFile::load(&path).unwrap();
        loaded.verify("https://b.com/z.ts", "0011").expect("应通过");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn missing_file_is_empty_lock() {
        let path = temp_lock("missing");
        assert!(LockFile::load(&path).unwrap().is_empty());
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }
}

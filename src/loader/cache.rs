//! 模块实例缓存。
//!
//! 键为解析后的绝对路径，值为已解析的模块实例。
//! 缓存存在的意义有两点：同一文件不被重复转译；循环依赖能取到「正在加载中」的实例。

use std::path::{Path, PathBuf};

/// 以绝对路径为键的模块缓存。
///
/// 内部使用 [`rustc_hash::FxHashMap`]，因为键是本地路径字符串，
/// 不需要密码学强度哈希。
#[derive(Debug, Default)]
pub struct ModuleCache {
    entries: rustc_hash::FxHashMap<PathBuf, usize>,
}

impl ModuleCache {
    /// 记录一个已加载模块，`handle` 为上层指定的实例编号。
    pub fn insert(&mut self, path: &Path, handle: usize) {
        self.entries.insert(path.to_path_buf(), handle);
    }

    /// 查询已加载模块的实例编号。
    pub fn get(&self, path: &Path) -> Option<usize> {
        self.entries.get(path).copied()
    }

    /// 当前缓存中的模块数量。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 缓存是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_then_get_returns_handle() {
        let mut cache = ModuleCache::default();
        let path = Path::new("/tmp/a.ts");
        cache.insert(path, 7);
        assert_eq!(cache.get(path), Some(7));
    }

    #[test]
    fn unknown_path_returns_none() {
        let cache = ModuleCache::default();
        assert_eq!(cache.get(Path::new("/tmp/none.ts")), None);
        assert!(cache.is_empty());
    }
}

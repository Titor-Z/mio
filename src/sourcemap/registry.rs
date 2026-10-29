//! 源码映射注册表。
//!
//! 按文件路径登记转译产出的源码映射，错误上报时按路径查找并换算行列。

use std::path::{Path, PathBuf};

/// 一份源码映射：记录 JavaScript 行列到 TypeScript 行列的换算关系。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMapEntry {
    /// 对应的 TypeScript 源文件路径。
    pub source_path: PathBuf,
    /// 源码映射原文（JSON 文本）。
    pub mapping: String,
}

/// 以文件路径为键的源码映射注册表。
#[derive(Debug, Default)]
pub struct SourceMapRegistry {
    entries: rustc_hash::FxHashMap<PathBuf, SourceMapEntry>,
}

impl SourceMapRegistry {
    /// 登记一个文件的源码映射。
    pub fn register(&mut self, generated_path: &Path, entry: SourceMapEntry) {
        self.entries.insert(generated_path.to_path_buf(), entry);
    }

    /// 查询某个产物文件对应的源码映射。
    pub fn get(&self, generated_path: &Path) -> Option<&SourceMapEntry> {
        self.entries.get(generated_path)
    }

    /// 当前登记的映射数量。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 注册表是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_then_get_returns_entry() {
        let mut registry = SourceMapRegistry::default();
        let generated = Path::new("/tmp/a.js");
        registry.register(
            generated,
            SourceMapEntry {
                source_path: PathBuf::from("/tmp/a.ts"),
                mapping: String::from("{}"),
            },
        );
        assert_eq!(
            registry
                .get(generated)
                .map(|entry| entry.source_path.clone()),
            Some(PathBuf::from("/tmp/a.ts"))
        );
    }
}

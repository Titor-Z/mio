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

pub use specifier::Specifier;

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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

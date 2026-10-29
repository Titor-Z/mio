//! 内建标准库：`@std/*` 命名空间下的模块。
//!
//! 每个模块以 Rust 原生实现，通过合成模块（synthetic module）暴露给脚本。
//! API 形态参考 W3C 与 Deno 风格：模块导入、Promise 优先、`Uint8Array` 二进制，
//! 不提供回调式接口，也不模仿 Node.js 的命名与语义。
//!
//! 依赖隔离：本模块只描述「有哪些内建函数、它们做什么」，全部为纯 Rust 实现，
//! 不出现任何 `boa_*` 类型。把 [`BuiltinFunction`] 包装成脚本可见的 `NativeFunction`、
//! 组装成合成模块，是 `engine/loader.rs` 的职责（那里允许 `boa_*`）。
//! 阶段 B 更换引擎时，本模块的算法与描述表原样复用。

mod path;

/// 一个内建函数的底层实现：接收脚本侧已转为 Rust `&str` 的实参，返回字符串结果或错误文本。
///
/// 当前内建模块的函数均为「字符串进、字符串出」，故用统一的函数指针签名，避免为每个
/// 函数手写引擎相关的包装。需要返回非字符串（如布尔、对象）时，应在扩展本表的同时调整引擎包装。
pub type NativeImpl = fn(&[&str]) -> Result<String, String>;

/// 一个内建函数的描述：脚本侧导出名与实现。
pub struct BuiltinFunction {
    /// 脚本侧可见的函数名（如 `join`、`basename`）。
    pub name: &'static str,
    /// 底层实现。
    pub call: NativeImpl,
}

/// 一个内建模块的描述：可导入的函数与字符串常量集合。
pub struct BuiltinModule {
    /// 模块名（`@std/` 之后的部分，如 `path`）。
    pub name: &'static str,
    /// 导出的函数。
    pub functions: &'static [BuiltinFunction],
    /// 导出的字符串常量（如 `separator`）。
    pub string_constants: &'static [(&'static str, &'static str)],
}

/// 按 `@std/<name>` 中的 `name` 查找内建模块；未注册返回 `None`。
pub fn lookup(name: &str) -> Option<BuiltinModule> {
    match name {
        "path" => Some(BuiltinModule {
            name: "path",
            functions: path::FUNCTIONS,
            string_constants: &[("separator", path::SEPARATOR)],
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_returns_path_module_with_expected_exports() {
        let module = lookup("path").expect("@std/path 应已注册");
        let names: Vec<&str> = module.functions.iter().map(|f| f.name).collect();
        assert!(names.contains(&"join"));
        assert!(names.contains(&"basename"));
        assert_eq!(module.string_constants, &[("separator", "/")]);
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup("unknown").is_none());
    }
}

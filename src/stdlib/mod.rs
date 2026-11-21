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
//!
//! 两类装配方式：
//! - 「字符串进、字符串出」（`path`）用 [`BuiltinFunction`] 描述表，走通用装配；
//! - 返回值不是字符串的模块（`fs` / `os` / `env` / `encoding` / `log`）在 `engine` 侧
//!   各有一份专用构建器，本文件只挂一个空条目，让 `@std/<name>` 仍能被识别。

pub mod encoding;
pub mod env;
pub(crate) mod fs;
pub mod log;
pub mod os;
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
        "fs" => Some(BuiltinModule {
            name: "fs",
            functions: &[], // fs 由 engine 专用构建器装配（见 engine/loader.rs::build_fs_module）
            string_constants: &[],
        }),
        // 以下四个模块的导出包含数值 / 布尔 / 对象 / undefined，通用装配表达不了，
        // 同样由 engine 侧专用构建器装配；此处只登记名字，保证 `@std/<name>` 可识别。
        "os" => Some(BuiltinModule {
            name: "os",
            functions: &[],
            string_constants: &[],
        }),
        "env" => Some(BuiltinModule {
            name: "env",
            functions: &[],
            string_constants: &[],
        }),
        "encoding" => Some(BuiltinModule {
            name: "encoding",
            functions: &[],
            string_constants: &[],
        }),
        "log" => Some(BuiltinModule {
            name: "log",
            functions: &[],
            string_constants: &[],
        }),
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
    fn lookup_fs_exists() {
        let module = lookup("fs").expect("@std/fs 应已注册");
        assert!(module.functions.is_empty()); // fs 的导出由 engine 侧处理
    }

    #[test]
    fn lookup_extra_std_modules_exist() {
        for name in ["os", "env", "encoding", "log"] {
            let module = lookup(name).unwrap_or_else(|| panic!("@std/{name} 应已注册"));
            assert!(module.functions.is_empty()); // 导出均由 engine 侧专用构建器处理
        }
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(lookup("unknown").is_none());
    }
}

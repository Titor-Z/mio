//! mio —— TypeScript 一等公民的运行时。
//!
//! 设计目标分两阶段：
//! - 阶段 A（当前）：TypeScript 源码作为唯一输入语言，运行时内部完成类型擦除并执行。
//! - 阶段 B（预留）：自研词法、语法、语义与字节码虚拟机，直接执行类型信息，输出原生代码。
//!
//! 两阶段之间只保留两个接缝：`engine::Engine` 抽象与 `resolver::Specifier` 分类。
//! 其余模块（配置、模块加载、标准库、命令行）在两个阶段之间复用。

pub mod cli;
pub mod config;
pub mod engine;
pub mod lexer;
pub mod loader;
pub mod resolver;
pub mod sourcemap;
pub mod stdlib;
pub mod transpiler;
pub mod web;

/// 当前 mio 版本号，取自 `Cargo.toml`，供命令行 `--version` 与诊断信息使用。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 语言标识，用于诊断信息前缀与配置 schema 的标识字段。
pub const LANG_TAG: &str = "mio";

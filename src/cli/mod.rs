//! 命令行入口层。
//!
//! 负责把进程参数翻译成 [`Command`] 枚举，再交给 [`Cli::dispatch`] 分发。
//! 一律使用 `match` 做分支，不使用长 `if/else` 链。
//!
//! 对应用户可见命令：
//! - `mio run <file>`   执行一个 TypeScript 或 JavaScript 文件
//! - `mio eval <code>`  执行一段内联源码
//! - `mio repl`         交互式求值
//! - `mio check <file>` 仅做类型检查（内置 tsrs 编译器，无外部依赖）
//! - `mio task <name>`  执行 `mio.json` 中声明的任务
//! - `mio init`         在当前目录创建项目骨架
//! - `mio types`        刷新运行时的类型声明

mod args;
mod dispatch;
mod repl;
mod scaffold;

pub use args::{Cli, Command};
pub use dispatch::dispatch;

/// 命令行的统一错误类型，保持字符串化以便直接打印给用户。
pub type CliResult = Result<(), String>;

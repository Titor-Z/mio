//! 内建标准库：`@std/*` 命名空间下的模块。
//!
//! 每个模块以 Rust 原生实现，通过合成模块（synthetic module）暴露给脚本。
//! API 形态参考 W3C 与 Deno 风格：模块导入、Promise 优先、`Uint8Array` 二进制，
//! 不提供回调式接口，也不模仿 Node.js 的命名与语义。

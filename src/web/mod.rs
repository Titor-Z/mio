//! Web 标准库：以 WHATWG / W3C 现行标准为对齐目标，作为全局对象注入。
//!
//! 与 `stdlib` 的分工：本模块提供「浏览器里本来就有」的全局对象；
//! `stdlib` 提供需要 `import` 才能使用的 `@std/*` 模块。
//!
//! 六条类型约定（所有实现必须遵守）：
//! 1. 二进制数据用 `Uint8Array`
//! 2. 时间戳用毫秒浮点数
//! 3. 异步返回 `Promise`
//! 4. 取消用 `AbortSignal`
//! 5. 字符串按 USVString 处理
//! 6. 错误用 `TypeError` / `RangeError` 等标准类型
//!
//! `fetch` 现状（阶段 A）：请求支持 method/headers/文本与 Uint8Array body；
//! 取消走 `AbortController`（约定 4：Promise 粒度拒绝，socket 自然排空）；
//! 响应提供 `text()`/`json()`/`arrayBuffer()`（约定 1：二进制以 Uint8Array 承载）。
//! 仍不支持：数组对形式 headers、FormData、ReadableStream body 族。

pub mod fetch;

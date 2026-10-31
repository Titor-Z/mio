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
//! 数据类型现状（阶段 A）：`URL`/`URLSearchParams`（WHATWG 解析经 `url` crate，
//! URL 部件为构造时快照、searchParams 可变表经序号登记）、`TextEncoder`/
//! `TextDecoder`（仅 utf-8、非 fatal）、`Blob`/`FormData`（File 为带 name 的
//! Blob 形态；FormData 可作 fetch body 并自动 multipart 编码）。
//! 定时器 `setTimeout`/`setInterval`/`queueMicrotask` 在引擎绑定层实现。
//!
//! `fetch` 现状（阶段 A）：请求支持 method/headers/文本与 Uint8Array body；
//! 取消走 `AbortController`（约定 4：Promise 粒度拒绝，socket 自然排空）；
//! 响应提供 `text()`/`json()`/`arrayBuffer()`（约定 1：二进制以 Uint8Array 承载）。
//! 仍不支持：数组对形式 headers、FormData、ReadableStream body 族。

pub mod fetch;
pub mod form_data;
pub mod url;

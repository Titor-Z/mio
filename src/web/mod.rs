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

pub mod fetch;

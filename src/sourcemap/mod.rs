//! 源码映射层：把运行时错误位置从擦除后的 JavaScript 映射回 TypeScript。
//!
//! 这是「错误信息要指向用户真正写的代码」这一体验目标的实现位置。
//! 阶段 B 自研前端会直接携带原始位置，本层届时退化为恒等映射。

mod registry;

pub use registry::SourceMapRegistry;

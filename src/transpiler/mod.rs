//! 转译层：TypeScript 源码 → JavaScript 源码。
//!
//! 阶段 A 由 OXC 承担，封装为 [`Transpiler`] 结构体。
//! 阶段 B 将替换为自研前端，但对外接口保持「源码进、源码出」。
//!
//! 依赖约束：`oxc_*` 只允许出现在本模块内部。

/// 转译产出：擦除类型后的 JavaScript 源码 + 可选的源码映射。
#[derive(Debug, Clone)]
pub struct TranspileOutput {
    /// 擦除类型后的 JavaScript 源码。
    pub code: String,
    /// 源码映射原文，用于把运行时错误位置映射回 TypeScript 行列。
    pub sourcemap: Option<String>,
}

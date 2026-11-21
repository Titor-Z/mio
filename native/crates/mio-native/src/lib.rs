//! mio-native：把带类型标注的 TypeScript 子集 AOT 编译为本机机器码。
//!
//! 管线（见 `docs/语法讨论书.md`）：OXC parser → AST → [`lower`] 降为 TIR
//! → [`check`] 类型检查 → [`inline`] 小函数内联 → [`codegen`] 生成 Cranelift IR
//! → [`emit`] 产出 `.o` 并调 `cc` 链接。
//!
//! 本 crate 与阶段 A（Boa）零耦合：不引用根 crate 的任何模块。
//! 错误统一为 [`BuildError`]：lower 期精确到行列，check 期定位到函数声明行，
//! codegen/emit/link 期无位置信息。

mod builtin;
mod check;
mod codegen;
mod diag;
mod emit;
mod inline;
mod lower;
mod repr;
mod resolve;
mod types;

pub use diag::{BuildError, Stage};
pub use lower::Program;
pub use types::MachineType;

use std::path::Path;

/// 编译单个文件：解析 → 降低 → 检查 → 内联 → 生成 → 链接。
///
/// `out_path` 为可执行文件路径（`--emit-obj` 时为其同名 `.o`）；
/// `emit_obj_only` 为真时只产 `.o`，不写 runtime.c、不链接。
///
/// # Errors
/// 任一阶段失败即返回 `Err`，错误带阶段名与（尽可能精确的）位置。
pub fn compile_file(
    src_path: &str,
    out_path: &Path,
    emit_obj_only: bool,
) -> Result<(), BuildError> {
    let src = std::fs::read_to_string(src_path).map_err(|e| {
        BuildError::new(Stage::Lower, src_path, format!("读取 {src_path} 失败：{e}"))
    })?;
    compile_source(&src, src_path, out_path, emit_obj_only)
}

/// 编译一段源码：入口文件路径只用于诊断定位，不参与编译语义。
///
/// 主 crate 的 `mio build` 经此函数在进程内调用原生后端（不经过 `mio-native` 二进制）。
///
/// # Errors
/// 任一阶段失败即返回 `Err`。
pub fn compile_source(
    src: &str,
    src_path: &str,
    out_path: &Path,
    emit_obj_only: bool,
) -> Result<(), BuildError> {
    // 多文件：先展开 import 图（入口源码已在手，被引模块在 expand 内读盘）。
    let modules =
        resolve::expand(src_path, src).map_err(|e| BuildError::new(Stage::Lower, src_path, e))?;
    let program = lower::lower_multi(&modules)?;
    check::check(&program)?;
    let program = inline::inline(program);
    let module =
        codegen::codegen(&program).map_err(|e| BuildError::new(Stage::Codegen, src_path, e))?;
    emit::emit(module, src_path, out_path, emit_obj_only)?;
    Ok(())
}

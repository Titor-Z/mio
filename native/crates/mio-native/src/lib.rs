//! mio-native：把带类型标注的 TypeScript 子集 AOT 编译为本机机器码。
//!
//! 管线（见 `docs/语法讨论书.md`）：OXC parser → AST → [`lower`] 降为 TIR
//! → [`check`] 类型检查 → [`codegen`] 生成 Cranelift IR → [`emit`] 产出 `.o` 并调 `cc` 链接。
//!
//! 本 crate 与阶段 A（Boa）零耦合：不引用根 crate 的任何模块。

mod builtin;
mod check;
mod codegen;
mod emit;
mod inline;
mod lower;
mod repr;
mod types;

pub use lower::Program;
pub use types::MachineType;

/// 编译单个文件：解析 → 降低 → 检查 → 生成 → 链接。
///
/// `out_path` 为可执行文件路径（`--emit-obj` 时为其同名 `.o`）；
/// `emit_obj_only` 为真时只产 `.o`，不写 runtime.c、不链接。
///
/// # Errors
/// 任一阶段失败即返回 `Err`，信息带阶段名。
pub fn compile_file(src_path: &str, out_path: &Path, emit_obj_only: bool) -> Result<(), String> {
    let src =
        std::fs::read_to_string(src_path).map_err(|e| format!("读取 {src_path} 失败：{e}"))?;
    let program = lower::lower(&src)?;
    check::check(&program)?;
    let program = inline::inline(program);
    let module = codegen::codegen(&program)?;
    emit::emit(module, src_path, out_path, emit_obj_only)
}

use std::path::Path;

//! 目标文件落盘与链接：`ObjectModule` → `.o` → 调 `cc` 链接为可执行文件。
//!
//! 运行时约定：用户代码调用的 `mio_print_*` 由本模块生成的 `runtime.c` 提供，
//! 与用户 `.o` 一起交给 `cc` 链接。`--emit-obj` 时只产 `.o`（不写 runtime.c、
//! 不链接），供人工检查 Cranelift 产出。

use std::path::Path;

use cranelift_codegen::isa;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_object::ObjectModule;
use target_lexicon::Triple;

/// 运行时 C 源码：按静态类型分派的打印函数（MVP 唯一的内置运行时面）。
///
/// 说明：`print` 的格式串在这里固定，不进用户产物；`f32` 内部提升为 double
/// 传给 `printf`（C 的默认实参提升），避免直接传 float 的未定义行为。
const RUNTIME_C: &str = r#"#include <stdio.h>
#include <stdint.h>

void mio_print_i16(int16_t v) { printf("%hd\n", v); }
void mio_print_i32(int32_t v) { printf("%d\n", v); }
void mio_print_i64(int64_t v) { printf("%lld\n", v); }
void mio_print_u16(uint16_t v) { printf("%hu\n", v); }
void mio_print_u32(uint32_t v) { printf("%u\n", v); }
void mio_print_u64(uint64_t v) { printf("%llu\n", v); }
void mio_print_f32(float v) { printf("%g\n", (double)v); }
void mio_print_f64(double v) { printf("%g\n", v); }
void mio_print_bool(int v) { printf("%s\n", v ? "true" : "false"); }
"#;

/// 为本机目标创建一个 `ObjectModule`（opt_level=speed）。
///
/// # Errors
/// 目标三元组不受支持或 flag 非法时返回 `Err`。
pub fn new_module() -> Result<ObjectModule, String> {
    let mut flag_builder = settings::builder();
    flag_builder
        .set("opt_level", "speed")
        .map_err(|error| format!("设置 opt_level 失败：{error}"))?;
    let flags = settings::Flags::new(flag_builder);
    // 宿主完整 triple：决定目标文件格式（Mach-O/ELF/COFF），仅按架构名会落到 ELF。
    let isa = isa::lookup(Triple::host())
        .map_err(|error| format!("本机目标不可用：{error}"))?
        .finish(flags)
        .map_err(|error| format!("构建目标 ISA 失败：{error}"))?;
    let builder = cranelift_object::ObjectBuilder::new(
        isa,
        "mio-native",
        cranelift_module::default_libcall_names(),
    )
    .map_err(|error| format!("创建 ObjectBuilder 失败：{error}"))?;
    Ok(ObjectModule::new(builder))
}

/// 产出入口：收尾模块 → 写 `.o` →（可选）写 `runtime.c` 并调 `cc` 链接。
///
/// # Errors
/// 目标文件产出、写盘或 `cc` 链接失败即返回 `Err`，错误信息给出可手工执行的命令。
pub fn emit(
    module: ObjectModule,
    src_path: &str,
    out_path: &Path,
    emit_obj_only: bool,
) -> Result<(), String> {
    let product = module.finish();
    let bytes = product
        .emit()
        .map_err(|error| format!("目标文件序列化失败：{error}"))?;

    let obj_path = out_path.with_extension("o");
    std::fs::write(&obj_path, &bytes)
        .map_err(|error| format!("写 {} 失败：{error}", obj_path.display()))?;

    if emit_obj_only {
        println!("已产出目标文件：{}", obj_path.display());
        return Ok(());
    }

    // runtime.c 与 .o 同目录，链接后即删，保持项目目录干净。
    let runtime_path = obj_path.with_extension("runtime.c");
    std::fs::write(&runtime_path, RUNTIME_C)
        .map_err(|error| format!("写 {} 失败：{error}", runtime_path.display()))?;

    let output = std::process::Command::new("cc")
        .arg("-o")
        .arg(out_path)
        .arg(&obj_path)
        .arg(&runtime_path)
        .output()
        .map_err(|error| format!("无法启动 cc：{error}"))?;

    // 无论链接成败都清理中间产物；失败时把 cc 的原始输出带给用户。
    let _ = std::fs::remove_file(&runtime_path);
    if !output.status.success() {
        let _ = std::fs::remove_file(&obj_path);
        return Err(format!(
            "cc 链接失败（可手工执行：cc -o {} {} runtime.c）：\n{}",
            out_path.display(),
            obj_path.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let _ = std::fs::remove_file(&obj_path);
    let _ = src_path;
    Ok(())
}

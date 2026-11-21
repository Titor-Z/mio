//! 内置函数：`print` 的静态类型到运行时打印函数符号的映射。
//!
//! MVP 只有一个内置。`print` 按实参的**静态类型**分派到不同的运行时函数
//! （`mio_print_i32` 等，由 `emit` 生成的 `runtime.c` 提供），不存在运行时探测。

/// 内置函数名。
pub const PRINT: &str = "print";

/// `print` 的实参静态类型 → 运行时打印函数符号。`None` 表示本阶段不支持该类型。
#[must_use]
pub fn print_runtime(type_name: &str) -> Option<&'static str> {
    match type_name {
        "i16" => Some("mio_print_i16"),
        "i32" => Some("mio_print_i32"),
        "i64" => Some("mio_print_i64"),
        "u16" => Some("mio_print_u16"),
        "u32" => Some("mio_print_u32"),
        "u64" => Some("mio_print_u64"),
        "f32" => Some("mio_print_f32"),
        "f64" | "number" => Some("mio_print_f64"),
        "bool" | "boolean" => Some("mio_print_bool"),
        _ => None,
    }
}

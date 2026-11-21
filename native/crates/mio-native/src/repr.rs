//! 表示决策表：把类型映射到内存表示。
//!
//! 规则（语法讨论书第六节）：每个类型在编译期必须被判定为「一种确定的内存表示」；
//! 判不出来的，一律编译期报错。本阶段只产出 POD；引用类型（GcRef）与泛型标为
//! `Unsupported` 并报错，因为 MVP 不建堆、不建泛型。

use crate::types::MachineType;

/// 值的内存表示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Representation {
    /// 纯值：不含指针，可栈上分配，随栈帧释放，无需 GC。
    Pod(MachineType),
    /// 本阶段无法表示（`string`、数组、`class`、未特化泛型等）。
    Unsupported,
}

/// 表示决策表。输入一个类型名，输出其表示；判不出即 `Unsupported`。
///
/// MVP 只接受机器类型与 `number`（= `F64`）。`boolean` 为兼容 TS 写法接受，
/// 表示与 `bool` 相同（`I32`），但类型层面不互通。
#[must_use]
pub fn decide(type_name: &str) -> Representation {
    match type_name {
        "i8" => Representation::Pod(MachineType::I8),
        "i16" => Representation::Pod(MachineType::I16),
        "i32" => Representation::Pod(MachineType::I32),
        "i64" => Representation::Pod(MachineType::I64),
        "u8" => Representation::Pod(MachineType::U8),
        "u16" => Representation::Pod(MachineType::U16),
        "u32" => Representation::Pod(MachineType::U32),
        "u64" => Representation::Pod(MachineType::U64),
        "f32" => Representation::Pod(MachineType::F32),
        "f64" => Representation::Pod(MachineType::F64),
        // TypeScript 的 number 永远是 f64，不得改动。
        "number" => Representation::Pod(MachineType::F64),
        "bool" | "boolean" => Representation::Pod(MachineType::Bool),
        "void" => Representation::Pod(MachineType::Void),
        _ => Representation::Unsupported,
    }
}

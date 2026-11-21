//! 机器类型：mio 方言在 TypeScript 类型语法之上追加的数值类型。
//!
//! 每个类型一份宽度与对齐，是表示决策表（`repr.rs`）的输入。
//! `number` 保持 TypeScript 语义，永远映射到 `F64`。

/// 机器类型。顺序即声明顺序，不承载语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineType {
    /// 1 字节有符号整数
    I8,
    /// 2 字节有符号整数
    I16,
    /// 4 字节有符号整数
    I32,
    /// 8 字节有符号整数
    I64,
    /// 1 字节无符号整数
    U8,
    /// 2 字节无符号整数
    U16,
    /// 4 字节无符号整数
    U32,
    /// 8 字节无符号整数
    U64,
    /// 4 字节浮点（IEEE-754 单精度）
    F32,
    /// 8 字节浮点（IEEE-754 双精度）
    F64,
    /// 布尔，静态区按 `i32`(0/1) 表示
    Bool,
    /// 无返回值
    Void,
}

impl MachineType {
    /// 存储宽度（字节）。
    #[must_use]
    pub fn size(self) -> u8 {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::Bool | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
            Self::Void => 0,
        }
    }

    /// 对齐要求（字节）。本阶段简化为等于宽度，`Void` 无存储。
    #[must_use]
    pub fn align(self) -> u8 {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::Bool | Self::F32 => 4,
            Self::I64 | Self::U64 | Self::F64 => 8,
            Self::Void => 1,
        }
    }

    /// 是否为整数类型（参与整数运算分派）。
    #[must_use]
    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Self::I8
                | Self::I16
                | Self::I32
                | Self::I64
                | Self::U8
                | Self::U16
                | Self::U32
                | Self::U64
        )
    }

    /// 是否为浮点类型。
    #[must_use]
    pub fn is_float(self) -> bool {
        matches!(self, Self::F32 | Self::F64)
    }

    /// 是否为有符号整数。
    #[must_use]
    pub fn is_signed(self) -> bool {
        matches!(self, Self::I8 | Self::I16 | Self::I32 | Self::I64)
    }

    /// 是否为无符号整数。
    #[must_use]
    pub fn is_unsigned(self) -> bool {
        matches!(self, Self::U8 | Self::U16 | Self::U32 | Self::U64)
    }
}

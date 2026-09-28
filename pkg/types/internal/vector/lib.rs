// Copyright 2026 AsterSQL.

// 向量类型（VECTOR）子系统门面与截断上下文。
//
// 提供类型转换时的 Flags/Context（忽略截断错误或转为 warning），
// 并挂接 `vector` / `vector_functions` / `truncate` 实现。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

pub use tidb_util_context::errors;

/// 错误码常量（errno）。
pub mod errno {
    pub use tidb_errno::errcode::*;
}

/// 未指定长度标记，对齐 parser FieldType 中的 UnspecifiedLength。
pub const UnspecifiedLength: i32 = tidb_parser_types::types::UnspecifiedLength as i32;

/// 忽略截断错误标志位。
pub const FLAG_IGNORE_TRUNCATE_ERR: u16 = 1 << 0;
/// 将截断视为 warning 而非硬错误。
pub const FLAG_TRUNCATE_AS_WARNING: u16 = 1 << 1;

/// 类型转换行为标志集合。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Flags(pub u16);

impl Flags {
    /// 是否忽略截断错误。
    pub fn IgnoreTruncateErr(self) -> bool {
        self.0 & FLAG_IGNORE_TRUNCATE_ERR != 0
    }

    /// 是否将截断记为 warning。
    pub fn TruncateAsWarning(self) -> bool {
        self.0 & FLAG_TRUNCATE_AS_WARNING != 0
    }
}

/// 携带 Flags 与累积警告的转换上下文。
pub struct Context {
    flags: Flags,
    warnings: Vec<errors::SharedError>,
}

impl Context {
    /// 以给定标志构造空警告列表的上下文。
    pub fn new(flags: Flags) -> Self {
        Self {
            flags,
            warnings: Vec::new(),
        }
    }

    /// 返回当前标志。
    pub fn Flags(&self) -> Flags {
        self.flags
    }

    /// 追加一条转换警告。
    pub fn AppendWarning(&mut self, error: errors::SharedError) {
        self.warnings.push(error);
    }

    /// 查看已累积的警告切片。
    pub fn warnings(&self) -> &[errors::SharedError] {
        &self.warnings
    }
}

/// VECTOR FLOAT32 类型表示与解析。
#[path = "../../vector.rs"]
mod vector;
pub use vector::*;

/// 向量距离等 SQL 函数实现。
#[path = "../../vector_functions.rs"]
mod vector_functions;

/// 数值截断辅助。
#[path = "../../truncate.rs"]
mod truncate;

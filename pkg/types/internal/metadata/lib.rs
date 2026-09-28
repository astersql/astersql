// Copyright 2026 AsterSQL.

// 元数据与字段类型子系统门面。
//
// 聚合 AST/字符集/校对规则/错误类型，以及 Datum Kind 常量；
// 通过 `include!` 引入 enum、FieldType 构建器等实现，
// 对应 Go `types` 包中与列类型描述相关的基础定义。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 列字段类型描述（FieldType）。
pub use parser_types::types::FieldType;

/// SQL AST 节点。
pub mod ast {
    pub use parser_ast::*;
}

/// parser 侧类型定义再导出。
pub mod ast_types {
    pub use parser_types::types::*;
}

/// 字符集定义。
pub mod charset {
    pub use parser_types::charset::*;
}

/// 校对规则（collation）。
pub mod collate {
    pub use parser_collate::*;
}

/// 数据库 terror 错误包装。
pub mod dbterror {
    pub use tidb_dbterror::dbterror::*;
    pub type Error = tidb_dbterror::terror::Error;
}

/// errno 错误码。
pub mod errno {
    pub use tidb_dbterror::errno::*;
}

/// 共享错误工具。
pub mod errors {
    pub use tidb_dbterror::errors::*;
}

/// MySQL 类型相关常量。
pub mod mysql {
    pub use parser_types::mysql::*;
}

/// 结构体内存大小估算。
pub mod size {
    pub use tidb_size::*;
}

/// terror 错误分类。
pub mod terror {
    pub use tidb_dbterror::terror::*;
}

pub use parser_types::format;

/// 表达式操作码。
#[path = "../../../parser/opcode/opcode.rs"]
pub mod opcode;

/// Datum 种类：空值。
pub const KindNull: u8 = 0;
/// Datum 种类：有符号 64 位整数。
pub const KindInt64: u8 = 1;
/// Datum 种类：无符号 64 位整数。
pub const KindUint64: u8 = 2;
/// Datum 种类：单精度浮点。
pub const KindFloat32: u8 = 3;
/// Datum 种类：双精度浮点。
pub const KindFloat64: u8 = 4;
/// Datum 种类：字符串。
pub const KindString: u8 = 5;
/// Datum 种类：字节串。
pub const KindBytes: u8 = 6;
/// Datum 种类：二进制字面量。
pub const KindBinaryLiteral: u8 = 7;
/// Datum 种类：MySQL DECIMAL。
pub const KindMysqlDecimal: u8 = 8;
/// Datum 种类：DURATION。
pub const KindMysqlDuration: u8 = 9;
/// Datum 种类：ENUM。
pub const KindMysqlEnum: u8 = 10;
/// Datum 种类：BIT。
pub const KindMysqlBit: u8 = 11;
/// Datum 种类：SET。
pub const KindMysqlSet: u8 = 12;
/// Datum 种类：TIME/DATETIME。
pub const KindMysqlTime: u8 = 13;
/// Datum 种类：接口占位。
pub const KindInterface: u8 = 14;
/// Datum 种类：非空最小值哨兵。
pub const KindMinNotNull: u8 = 15;
/// Datum 种类：最大值哨兵。
pub const KindMaxValue: u8 = 16;
/// Datum 种类：原始字节。
pub const KindRaw: u8 = 17;
/// Datum 种类：JSON。
pub const KindMysqlJSON: u8 = 18;
/// Datum 种类：VECTOR FLOAT32。
pub const KindVectorFloat32: u8 = 19;

mod errors_defs {
    use crate::{dbterror, errno};
    include!("../../errors.rs");
}
pub use errors_defs::*;

mod enum_defs {
    use crate::{ErrTruncated, collate, errors};
    include!("../../enum.rs");
}
pub use enum_defs::*;

mod etc_defs {
    use crate::ast_types as ast;
    use crate::{
        ErrOverflow, FieldType, KindBinaryLiteral, KindBytes, KindFloat32, KindFloat64, KindInt64,
        KindInterface, KindMaxValue, KindMinNotNull, KindMysqlBit, KindMysqlDecimal,
        KindMysqlDuration, KindMysqlEnum, KindMysqlJSON, KindMysqlSet, KindMysqlTime, KindNull,
        KindRaw, KindString, KindUint64, KindVectorFloat32, charset, collate, errors, mysql,
        opcode,
    };
    include!("../../etc.rs");
}
pub use etc_defs::*;

mod eval_type_defs {
    use crate::ast_types as ast;
    include!("../../eval_type.rs");
}
pub use eval_type_defs::*;

mod explain_format_defs {
    include!("../../explain_format.rs");
}
pub use explain_format_defs::*;

mod field_name_defs {
    use crate::{ast, size};
    include!("../../field_name.rs");
}
pub use field_name_defs::*;

mod field_type_builder_defs {
    use crate::FieldType;
    include!("../../field_type_builder.rs");
}
pub use field_type_builder_defs::*;

#[cfg(test)]
#[path = "../../enum_4_aster_unit_test.rs"]
mod enum_4_aster_unit_test;

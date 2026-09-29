// Copyright 2026 AsterSQL.

// parser 测试驱动（test_driver）crate 入口。
//
// 为 parser 单元测试提供轻量依赖桩：字符集、MySQL 类型、Datum、MyDecimal 等，
// 避免测试直接耦合完整运行时。Datum 是表达式求值时的通用值容器。

/// 字符集与编码相关再导出，供测试构造带字符集前缀的字面量。
pub mod charset {
    pub use parser_charset::charset::*;
    pub use parser_charset::*;
}

/// 格式化辅助（AST Restore 等），对齐 Go format 包在测试中的用途。
pub use parser_format as format;

/// MySQL 类型、字符集与错误码等测试依赖。
pub mod mysql {
    pub use parser_mysql::*;
    pub use parser_mysql::{charset::*, r#type::*};
}

/// 类型系统（FieldType 等）再导出。
pub use parser_types as types;

/// 测试辅助函数与桩实现。
#[path = "test_driver_helper.rs"]
mod test_driver_helper;
pub use test_driver_helper::*;

/// MyDecimal（定点数）测试驱动实现。
#[path = "test_driver_mydecimal.rs"]
mod test_driver_mydecimal;
pub use test_driver_mydecimal::*;

/// Datum（通用值）测试驱动实现。
#[path = "test_driver_datum.rs"]
mod test_driver_datum;
pub use test_driver_datum::*;

/// 主测试驱动入口：ValueExpr 构造等。
#[path = "test_driver.rs"]
mod test_driver;
pub use test_driver::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 迁移期对 test_driver 行为的回归测试。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "test_driver_test.rs"]
mod test_driver_test;

#[cfg(test)]
#[path = "go_merge_35_test.rs"]
mod go_merge_35_test;

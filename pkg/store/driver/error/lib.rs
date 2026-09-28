// Copyright 2026 AsterSQL.

// store driver 错误转换 crate 入口。
//
// 聚合 dbterror / errno / terror / kv / sqlkiller / exeerrors 等依赖，并导出
// `error.rs` 中的 TiKvError、PdError、`ToTiDBErr` 等，供 TiKV 客户端错误映射为 TiDB 错误。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// pingcap/errors 风格的 SharedError 工具再导出。
pub mod errors {
    pub use tidb_dbterror::errors::*;
}
/// 错误分类与 NewStd 工厂再导出。
pub mod dbterror {
    pub use tidb_dbterror::dbterror::*;
}
/// 错误码（errno）常量再导出。
pub mod errno {
    pub use tidb_dbterror::errno::*;
}
/// terror 错误基类与全局哨兵（如 ErrResultUndetermined）再导出。
pub mod terror {
    pub use tidb_dbterror::terror::*;
}
/// MySQL 错误名映射再导出。
pub mod parser_mysql {
    pub use parser_mysql_crate::errname::*;
}
/// KV 层错误定义（内嵌 `kv/error.rs`）。
pub mod kv {
    use crate::*;
    include!("../../../kv/error.rs");
}
/// sqlkiller 查询中断信号常量再导出。
pub mod sqlkiller {
    pub use tidb_sqlkiller::sqlkiller::{
        MaxExecTimeExceeded, QueryInterrupted, QueryMemoryExceeded, RunawayQueryExceeded,
        ServerMemoryExceeded,
    };
}
/// 执行器侧内存/超时/runaway 错误再导出。
pub mod exeerrors {
    pub use exeerrors_crate::exeerrors::*;
}
/// 实际错误适配实现（TiKvError / ToTiDBErr）。
mod driver_error {
    include!("error.rs");
}
pub use driver_error::*;

/// Aster 迁移对照的错误转换单元测试。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

/// 与 Go TestConvertError 等对齐的基础转换测试。
#[cfg(test)]
#[path = "error_test.rs"]
mod error_test;

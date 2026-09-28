// Copyright 2026 AsterSQL.

// DDL 日志工具 crate 的入口模块。
//
// 本 crate 为 DDL（数据定义语言）子系统提供带 category 的专用 Logger，
// 便于按 `ddl` / `ddl-upgrading` / `ddl-ingest` 等类别过滤与采样日志。
// 本文件负责：
// - 重新导出通用日志依赖，模拟原 Go 包路径；
// - 声明并导出核心实现模块 `ddl_logutil`。

#![allow(non_snake_case)]

extern crate self as astersql_ddl_logutil;

/// 通用日志器相关导出（对齐 `astersql_util_logutil::general_logger`）。
pub mod general_logger {
    pub use astersql_util_logutil::general_logger::*;
}
/// 底层日志 API 导出（`BgLogger`、`LogField` 等）。
pub mod util_log {
    pub use astersql_util_logutil::log::*;
}
pub use util_log as log;
/// 慢查询日志相关导出。
pub mod slow_query_logger {
    pub use astersql_util_logutil::slow_query_logger::*;
}

/// 兼容命名空间：`util::logutil::log` 指向本 crate 的 `util_log`。
pub mod util {
    pub mod logutil {
        pub use crate::util_log as log;
    }
}

/// DDL 专用 Logger 工厂实现（`DDLLogger` / `SampleLogger` 等）。
#[path = "logutil.rs"]
pub mod ddl_logutil;
pub use ddl_logutil::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

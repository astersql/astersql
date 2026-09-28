// Copyright 2026 AsterSQL.

// stmtsummary v2 crate 根：持久化语句摘要（statement summary）的第二代实现。
//
// 语句摘要按 SQL digest（归一化指纹）聚合执行统计，供 `INFORMATION_SCHEMA`
// 与历史日志查询。本 crate 聚合 record、列工厂、日志落盘与内存/历史读取器，
// 并通过 re-export / `#[path]` 挂载对应测试。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

extern crate self as task_stmtsummary_v2;

pub use chrono_tz::UTC;
pub use task_stmtctx::TableEntry;
pub use task_stmtsummary::{StmtExecInfo, StmtExecLazyInfo};

/// 列元信息，来自 v1 stmtsummary 的 model 兼容层。
pub mod model {
    pub use task_stmtsummary::model::ColumnInfo;
}
/// MySQL 时间戳类型常量，用于摘要时间列包装。
pub mod mysql {
    pub use task_stmtsummary::mysql::{DefaultCollationName, TypeTimestamp};
}
/// Datum / Time 等类型别名，与 Go types 包对齐。
pub mod types {
    pub use task_stmtsummary::types::*;
}
/// 执行计划解码：将编码后的 plan 文本还原为可读 PLAN 列。
pub mod plancodec {
    pub use plancodec_dependency::DecodePlan;
}

// 核心实现模块：记录、列映射、日志、读取器与窗口摘要。
mod record;
pub use record::*;
mod column;
pub use column::*;
mod logger;
pub use logger::*;
mod reader;
pub use reader::*;
mod stmtsummary;
pub use stmtsummary::*;

#[cfg(test)]
pub mod testkit {
    pub mod testsetup {
        pub use testsetup_dependency::*;
    }
}

#[cfg(test)]
mod record_2_aster_unit_test {
    use super::*;
    include!("record_2_aster_unit_test.rs");
}

#[cfg(test)]
#[path = "column_test.rs"]
mod column_test;

#[cfg(test)]
mod main_test {
    use crate::testkit::testsetup;
    include!("main_test.rs");
}

#[cfg(test)]
#[path = "reader_test.rs"]
mod reader_test;

#[cfg(test)]
#[path = "record_test.rs"]
mod record_test;

#[cfg(test)]
#[path = "stmtsummary_benchmark_test.rs"]
mod stmtsummary_benchmark_test;

#[cfg(test)]
#[path = "stmtsummary_test.rs"]
mod stmtsummary_test;

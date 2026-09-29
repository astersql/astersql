// Copyright 2026 AsterSQL.

// 语句摘要（statement summary）工具 crate 入口。
//
// 对应 Go `pkg/util/stmtsummary`：按 digest 聚合 SQL 执行统计，提供 LRU 淘汰、
// 历史窗口、以及从内存摘要读出信息模式表行的 reader。测试模块覆盖淘汰、
// 聚合与 TestMain 兼容入口。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 用户身份类型再导出（权限过滤用）。
pub mod auth {
    pub use auth_dependency::parser::auth::auth::UserIdentity;
}
/// 列元信息再导出（reader 列工厂按列名解析）。
pub mod model {
    pub use model_dependency::group_4::ColumnInfo;
}
/// MySQL 时间类型常量再导出。
pub mod mysql {
    pub use mysql_dependency::charset;
    pub use mysql_dependency::charset::DefaultCollationName;
    pub use mysql_dependency::r#type::TypeTimestamp;
}
/// 执行计划解码再导出。
pub mod plancodec {
    pub use plancodec_dependency::DecodePlan;
}
/// 字符串集合再导出（digest checker）。
pub mod set {
    pub use set_dependency::string_set::{NewStringSet, StringSet};
}
/// Datum / Time 构造与时间转换再导出。
pub mod types {
    pub use types_dependency::datum::{
        Datum, NewFloat64Datum, NewIntDatum, NewStringDatum, NewTimeDatum, NewUintDatum,
    };
    pub use types_dependency::time::{FromGoTime, NewTime, Time};
}

/// 语句摘要核心：LRU map、统计聚合与 RU/网络流量汇总。
mod statement_summary;
pub use statement_summary::*;
pub use statement_summary::{StmtExecInfo, StmtExecLazyInfo, StmtSummaryByDigestMap};
/// 被 LRU 淘汰的 digest 按时间窗口聚合。
mod evicted;
pub use evicted::*;
/// 将内存摘要转为信息模式表行的 reader。
mod reader;
pub use reader::*;

#[cfg(test)]
#[path = "evicted_1_aster_unit_test.rs"]
/// AsterSQL 迁移补充：淘汰窗口匹配与 addInfo 合并。
mod evicted_1_aster_unit_test;

#[cfg(test)]
#[path = "statement_summary_2_aster_unit_test.rs"]
/// AsterSQL 迁移补充：digest key、聚合窗口与 RU/网络辅助函数。
mod statement_summary_2_aster_unit_test;

#[cfg(test)]
#[path = "statement_summary_test.rs"]
/// 对应 Go `statement_summary_test.go`。
mod statement_summary_test;

#[cfg(test)]
#[path = "evicted_test.rs"]
/// 对应 Go `evicted_test.go`。
mod evicted_test;
#[cfg(test)]
mod go_merge_34_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "reader_test.rs"]
mod reader_test;

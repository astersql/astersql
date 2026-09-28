// Copyright 2026 AsterSQL.

// Admin Pause DDL 测试包入口。
//
// 聚合测试数据生成、DDL 语句 case 矩阵、全局环境准备，以及
// pause/resume/cancel/负向 等测试子模块。对应 Go 的
// `pkg/ddl/tests/adminpause` 包。

#![allow(dead_code)]

/// 测试表 DDL 与随机行数据生成。
pub mod ddl_data_generation;
/// Admin Pause 用 DDL 语句 case 矩阵。
pub mod ddl_stmt_cases;
/// Mock domain / TestKit 等全局环境。
pub mod global;

pub use ddl_data_generation::{
    ADMIN_PAUSE_TEST_PARTITION_TABLE, ADMIN_PAUSE_TEST_PARTITION_TABLE_STMT,
    ADMIN_PAUSE_TEST_TABLE, ADMIN_PAUSE_TEST_TABLE_STMT, ADMIN_PAUSE_TEST_TABLE_STMT_WITH_VEC,
    ADMIN_PAUSE_TEST_TABLE_WITH_VEC, SqlExecutor, TestTableUser, generate_name, generate_phone,
    generate_string, generate_tbl_user, generate_tbl_user_parition, generate_tbl_user_with_vec,
};
pub use ddl_stmt_cases::{
    AutoIncrsedID, StmtCase, column_ddl_stmt_case, index_ddl_stmt_case, place_rul_ddl_stmt_case,
    schema_ddl_stmt_case, simple_run_stmt, table_ddl_stmt, table_partition_ddl_stmt_case,
};
pub use global::{DB_TEST_LEASE_MILLIS, LOGGER, PreparedDomain, prepare_domain};

/// 数据生成与 TiFlash 副作用契约测试。
#[cfg(test)]
mod ddl_data_generation_test;
/// DDL case 矩阵与 Go 的逐语句一致性测试。
#[cfg(test)]
mod ddl_stmt_cases_test;
/// Domain lease and TestKit lifecycle parity with Go global.go.
#[cfg(test)]
mod global_test;
/// 测试入口 / TestMain 的 Rust 契约测试。
#[cfg(test)]
mod main_test;
/// Pause 后 Cancel 行为测试。
#[cfg(test)]
mod pause_cancel_test;
/// Pause/Resume/Cancel 非法状态与缺失 job 的负向测试。
#[cfg(test)]
mod pause_negative_test;
/// Pause 后 Resume（含 User/System 操作者权限）测试。
#[cfg(test)]
mod pause_resume_test;

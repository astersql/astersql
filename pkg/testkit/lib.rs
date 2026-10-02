// Copyright 2026 AsterSQL.

// testkit crate 根模块。
//
// 聚合测试工具箱（TestKit）相关子模块：Mock 存储、会话管理、结果断言、
// 分步执行与测试数据加载等；对外再导出常用类型，并在 `#[cfg(test)]` 下挂载
// 若干统计/DDL/驱动集成测试文件。

#![allow(dead_code, non_snake_case)]

use std::fmt;

/// 测试侧统一错误类型，携带可读消息字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestError {
    message: String,
}

impl TestError {
    /// 由任意可转为 `String` 的消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// 返回错误消息文本。
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for TestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TestError {}

impl From<std::io::Error> for TestError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

/// 测试结果别名：默认成功单元 `()`，失败为 [`TestError`]。
pub type TestResult<T = ()> = std::result::Result<T, TestError>;

/// 异步 TestKit 子模块。
pub mod asynctestkit;
/// 数据库驱动与 MockDB 抽象。
pub mod db_driver;
/// 基于真实/模拟 DB 的 TestKit 封装。
pub mod dbtestkit;
/// 会话/进程列表 Mock 管理器。
pub mod mocksessionmanager;
/// Mock 存储与带 Domain 的分析统计 Store。
pub mod mockstore;
/// 查询结果行集与断言辅助。
pub mod result;
/// TiDB JSON 统计 fixture 加载。
pub mod stats_fixture;
/// 带断点的分步（stepped）TestKit。
pub mod stepped;
/// 从 JSON 套件加载/录制期望输出的测试数据。
#[path = "testdata/testdata.rs"]
pub mod testdata;
/// 核心 TestKit：执行 SQL、查询与会话变量。
pub mod testkit;

pub use db_driver::{
    AnalyzeStatsContext, AssignFromDbValue, CreateMockDB, Database, DbValue, ExecutionResult,
    MockDB, MockRow, MockRows, MockStmt, PreparedResultField, PreparedStatement, QueryRows,
};
pub use dbtestkit::DBTestKit;
pub use result::{Result, Rows, RowsWithSep};
pub use stats_fixture::LoadTableStats;
pub use testkit::{NewTestKit, TestKit, TestMemTracker, TestSession, TestSessionVars};

// 以下为通过 `#[path]` 挂入的集成测试，仅在 test 配置编译。
#[cfg(test)]
#[path = "analyze_stats_runtime_aster_unit_test.rs"]
mod analyze_stats_runtime_aster_unit_test;

#[cfg(test)]
#[path = "stats_runtime_aster_unit_test.rs"]
mod stats_runtime_aster_unit_test;

#[cfg(test)]
#[path = "mockstore_domain_stats_test.rs"]
mod mockstore_domain_stats_test;

#[cfg(test)]
#[path = "mockstore_domain_ddl_test.rs"]
mod mockstore_domain_ddl_test;

#[cfg(test)]
#[path = "mockstore_domain_infoschema_v2_test.rs"]
mod mockstore_domain_infoschema_v2_test;

#[cfg(test)]
#[path = "db_driver_test.rs"]
mod db_driver_test;

#[cfg(test)]
#[path = "testkit_test.rs"]
mod testkit_test;

#[cfg(test)]
#[path = "go_merge_49_partial_index_test.rs"]
mod go_merge_49_partial_index_test;

#[cfg(test)]
#[path = "asynctestkit_test.rs"]
mod asynctestkit_test;

#[cfg(test)]
#[path = "mocksessionmanager_test.rs"]
mod mocksessionmanager_test;

#[cfg(test)]
#[path = "mockstore_test.rs"]
mod mockstore_test;

#[cfg(test)]
#[path = "cascades_compatibility_test.rs"]
mod cascades_compatibility_test;

#[cfg(test)]
#[path = "result_test.rs"]
mod result_test;

#[cfg(test)]
#[path = "stats_fixture_test.rs"]
mod stats_fixture_test;

#[cfg(test)]
#[path = "testdata/testdata_test.rs"]
mod testdata_test;

#[cfg(test)]
#[path = "stepped_test.rs"]
mod stepped_test;

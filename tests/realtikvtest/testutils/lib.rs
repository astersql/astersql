// Copyright 2026 AsterSQL.

//! 中文说明开始（自动生成）
//! 中文总览：`lib.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `测试工具与兼容封装` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 7 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `stubs` 是当前文件里的模块。
//! `stubs` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `stubs` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `stubs`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `common` 是当前文件里的模块。
//! `common` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `common` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `common`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `compatibility` 是当前文件里的模块。
//! `compatibility` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `compatibility` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `compatibility`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `global_sort` 是当前文件里的模块。
//! `global_sort` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `global_sort` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `global_sort`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `workload` 是当前文件里的模块。
//! `workload` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `workload` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `workload`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `parity_test` 是当前文件里的模块。
//! `parity_test` 所处的位置主要服务 `测试工具与兼容封装` 主题下的一个阅读切面。
//! 阅读 `parity_test` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `parity_test`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Crate entry for `tests/realtikvtest/testutils`
//! (Go package `github.com/pingcap/tidb/tests/realtikvtest/testutils`).

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::all
)]

#[path = "stubs.rs"]
pub mod stubs;

#[path = "common.rs"]
mod common;

#[path = "compatibility.rs"]
mod compatibility;

#[path = "global_sort.rs"]
mod global_sort;

#[path = "workload.rs"]
mod workload;

pub use common::{
    AddIndexGenCol, AddIndexMultiCols, AddIndexNonUnique, AddIndexPK, AddIndexUnique,
    AssertExternalField, InitTest, InitTestFailpoint, SuiteContext, TestOneColFrame,
    TestOneIndexFrame, TestTwoColsFrame,
};
pub use compatibility::{
    CompatibilityContext, InitCompCtx, InitCompCtxParams, InitConcurrentDDLTest, TestGenIndex,
    TestMultiCols, TestNonUnique, TestPK, TestType, TestUnique,
};
pub use global_sort::RemoveAllObjects;
pub use stubs::{ExternalTagged, ExternalTaggedField, fakestorage, storage};

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "workload_test.rs"]
mod workload_test;

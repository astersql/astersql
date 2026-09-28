// Copyright 2026 AsterSQL.

// executor 内部通用工具库入口。
//
// 导出分区表相关的 tipb Executor 树改写（`partition_table`），以及随机串、
// 泄漏文件检查等通用辅助（`util`）；测试配置下挂接迁移单测。

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

extern crate self as astersql_executor_internal_util;

/// 分区表扫描等 tipb Executor 节点的最小模型与 TableID 更新逻辑。
mod partition_table;
pub use partition_table::*;
/// 随机字符串生成、函数名探测、临时文件泄漏检查等工具。
mod util;
pub use util::*;

/// 迁移期 Aster 单元测试（验证 UpdateExecutorTableID 等与 Go 路径一致）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

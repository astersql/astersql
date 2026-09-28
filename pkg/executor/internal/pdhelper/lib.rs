// Copyright 2026 AsterSQL.

// PD Helper crate 入口。
//
// 对外导出从 PD（Placement Driver，集群元数据与调度中心）或受限 SQL
// 获取表近似行数的辅助类型与全局实例；测试模块在 `cfg(test)` 下按路径挂载。
#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 核心实现：缓存、清理协程与近似行数查询逻辑。
mod pd;
/// 再导出 `pd` 模块中的公共 API，供外部直接 `use astersql_..._pdhelper::*`。
pub use pd::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// Aster 侧迁移单元测试（缓存命中/驱逐、大小表路径、失败缓存等）。
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
/// 包级 `TestMain` 等价设置：failpoint 与配置。
pub(crate) mod main_test;

#[cfg(test)]
#[path = "pd_test.rs"]
/// 与 Go `pd_test.go` 对齐的 TTL 缓存行为测试。
mod pd_test;

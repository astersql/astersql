// Copyright 2026 AsterSQL.

// Affinity（亲和性）子系统的 crate 入口。
//
// Affinity Group 用于将一组 key range 绑定到相近的存储节点，降低跨节点访问开销。
// 本文件负责：
// - 声明并导出 `interface`（包级 API）与 `manager`（PD / Mock 实现）；
// - 在测试配置下挂载对应单测模块。

extern crate self as astersql_domain_affinity;

/// 包级对外接口：初始化 Manager、创建/删除/查询 Affinity Group。
pub mod interface;
/// Manager 实现：PdManager、MockManager 及 PD HTTP 错误回退策略。
pub mod manager;
pub use interface::*;
pub use manager::*;

/// 包级 API 的 context 传播、重试与日志对齐测试。
#[cfg(test)]
#[path = "interface_test.rs"]
mod interface_test;
/// PdManager 行为单测（对齐 Go 侧 affinity manager 测试）。
#[cfg(test)]
#[path = "manager_test.rs"]
mod manager_test;
/// 迁移期 Aster 单元测试：覆盖创建回退、查询过滤与删除重试等路径。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

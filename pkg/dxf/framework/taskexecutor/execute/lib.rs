// Copyright 2026 AsterSQL.

// 任务执行器 execute 子包入口：导出接口并挂接测试模块。
//
// 对应 Go 的 `taskexecutor/execute` 包，提供 StepExecutor 契约与
// 子任务摘要类型，供调度框架与具体任务实现共用。

extern crate self as astersql_dxf_framework_taskexecutor_execute;
// 对外导出 StepExecutor、SubtaskSummary、FrameworkInfo 等。
mod interface;
pub use interface::*;

#[cfg(test)]
// Aster 迁移对照测试。
mod migration_aster_unit_test;

#[cfg(test)]
// 接口与速度估算单元测试。
mod interface_test;

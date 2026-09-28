// Copyright 2026 AsterSQL.

// `scheduler` crate 入口：导出调度命令、Scheduler trait 与 CPU 调度器。
//
// 通过 `cpu` 子模块转发 CPU 使用率采样，并在测试配置下挂载迁移期单元测试。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as astersql_resourcemanager_scheduler;

#[path = "../util/util.rs"]
/// 调度器依赖的工具类型（组件、goroutine 池接口等）。
pub mod util;
/// CPU 使用率采样的再导出。
pub mod cpu {
    pub use cpu_crate::GetCPUUsage;
}
/// 基于 CPU 的调度器实现。
pub mod cpu_scheduler;
/// 调度命令枚举与 Scheduler trait。
pub mod scheduler;
pub use cpu_scheduler::*;
pub use scheduler::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

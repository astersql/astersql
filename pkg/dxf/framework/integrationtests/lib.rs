// Copyright 2026 AsterSQL.

// DXF framework 集成测试包入口。
//
// 以 `#[path]` 挂载各主题测试模块（基准、错误处理、HA、pause/resume、rollback、
// scope、主路径、modify、资源控制等），仅在 `cfg(test)` 下编译。

#![allow(dead_code)]

/// 调度器开销基准与容量边界测试。
#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
/// 任务错误处理与人工恢复路径测试。
#[cfg(test)]
#[path = "framework_err_handling_test.rs"]
mod framework_err_handling_test;
/// 高可用：节点下线与多 owner 场景测试。
#[cfg(test)]
#[path = "framework_ha_test.rs"]
mod framework_ha_test;
/// 任务暂停与恢复状态机测试。
#[cfg(test)]
#[path = "framework_pause_and_resume_test.rs"]
mod framework_pause_and_resume_test;
/// 任务取消回滚路径测试。
#[cfg(test)]
#[path = "framework_rollback_test.rs"]
mod framework_rollback_test;
/// 目标 scope / 服务角色筛选测试。
#[cfg(test)]
#[path = "framework_scope_test.rs"]
mod framework_scope_test;
/// 框架主路径：提交、扩缩容、GC、cleanup 等。
#[cfg(test)]
#[path = "framework_test.rs"]
mod framework_test;
/// 测试入口与全局并发上限 guard。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// 任务修改相关集成测试。
#[cfg(test)]
#[path = "modify_test.rs"]
mod modify_test;
/// 资源控制与槽位管理集成测试。
#[cfg(test)]
#[path = "resource_control_test.rs"]
mod resource_control_test;

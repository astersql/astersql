// Copyright 2026 AsterSQL.

// TTL worker 包入口：导出配置、扫描、删除、作业管理、会话与定时调度等子模块。
//
// TTL（Time To Live）按表定义的过期列与间隔，周期性扫描并删除过期行。
// 本 crate 是 Go 版 `pkg/ttl/ttlworker` 的机械迁移基线，子模块职责大致为：
// - `job` / `job_manager`：表级清理作业的创建、心跳与收尾；
// - `scan` / `del`：按范围扫描过期主键并批量删除；
// - `session`：worker 会话状态与 TTL 工作校验；
// - `task_manager` / `timer` / `worker`：子任务调度、定时触发与 worker 生命周期。

#![allow(dead_code)]

pub mod config;
pub mod del;
pub mod job;
pub mod job_manager;
pub mod job_version_checker;
pub mod persistent;
pub mod scan;
pub mod session;
pub mod task_manager;
pub mod timer;
pub mod timer_sync;
pub mod worker;

#[cfg(test)]
mod config_test;
#[cfg(test)]
mod del_test;
#[cfg(test)]
mod job_manager_integration_test;
#[cfg(test)]
mod job_manager_test;
#[cfg(test)]
mod job_test;
#[cfg(test)]
mod job_version_checker_test;
#[cfg(test)]
mod persistent_test;
#[cfg(test)]
mod scan_integration_test;
#[cfg(test)]
mod scan_test;
#[cfg(test)]
mod session_integration_test;
#[cfg(test)]
mod session_test;
#[cfg(test)]
mod task_manager_integration_test;
#[cfg(test)]
mod task_manager_test;
#[cfg(test)]
mod timer_sync_test;
#[cfg(test)]
mod timer_test;
#[cfg(test)]
mod worker_test;

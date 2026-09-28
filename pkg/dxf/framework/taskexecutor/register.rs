// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// 任务类型 → `TaskExecutor` 工厂注册表。
//
// Manager 启动任务时按 `Task.Type` 查找工厂并实例化执行器。
// 注册表为进程级全局状态；测试需通过 `RegistryLockForTest` 串行化访问。

use crate::{Context, Param, Task, TaskExecutor, TaskType};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
/// 按任务创建 `TaskExecutor` 的工厂闭包类型。
pub type FactoryFn = Arc<dyn Fn(Context, Task, Param) -> Arc<dyn TaskExecutor> + Send + Sync>;
/// 进程内全局任务类型 → 工厂映射（懒初始化）。
fn factories() -> &'static RwLock<HashMap<TaskType, FactoryFn>> {
    static F: OnceLock<RwLock<HashMap<TaskType, FactoryFn>>> = OnceLock::new();
    F.get_or_init(|| RwLock::new(HashMap::new()))
}
/// 注册或覆盖某任务类型的执行器工厂。
pub fn RegisterTaskType(task_type: TaskType, factory: FactoryFn) {
    factories()
        .write()
        .expect("factory lock poisoned")
        .insert(task_type, factory);
}
/// 按类型查找工厂；未注册返回 None。
pub fn GetTaskExecutorFactory(task_type: &str) -> Option<FactoryFn> {
    factories()
        .read()
        .expect("factory lock poisoned")
        .get(task_type)
        .cloned()
}
/// 清空注册表（测试隔离用）。
pub fn ClearTaskExecutors() {
    factories().write().expect("factory lock poisoned").clear();
}
/// 测试用全局互斥：串行化对进程级注册表的并发访问。
#[cfg(test)]
/// `RegisterTaskType`/`ClearTaskExecutors` mutate a single process-wide
/// registry. Every test file across this crate that touches it (directly,
/// or transitively by running a real `TaskExecutor`/`Manager` that looks
/// factories up) must serialize through this one lock, since `cargo test`
/// runs all of them concurrently in the same process; per-file locks would
/// not exclude each other. Mirrors Go's package tests running sequentially
/// by default.
pub fn RegistryLockForTest() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

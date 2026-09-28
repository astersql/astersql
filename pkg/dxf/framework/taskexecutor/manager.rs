// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// 节点侧任务执行管理器。
//
// `Manager` 在执行节点上周期性拉取可执行任务，按 CPU slot 分配资源，
// 通过注册工厂创建 `TaskExecutor` 并在独立线程运行；同时处理暂停、回滚与元数据恢复。

use crate::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
/// 任务轮询间隔（纳秒），默认 300ms；测试可缩短。
static taskCheckIntervalNanos: AtomicU64 = AtomicU64::new(300_000_000);
/// TaskCheckInterval is the interval to check whether there are tasks to
/// run. Mirrors Go's mutable package-level `var` (see `SubtaskCheckInterval`
/// in task_executor.rs for the same pattern).
/// 读取当前任务检查间隔。
pub fn TaskCheckInterval() -> Duration {
    Duration::from_nanos(taskCheckIntervalNanos.load(Ordering::Acquire))
}
/// Mirrors Go directly reassigning the public package-level
/// `TaskCheckInterval` var from a test in another package; kept as a
/// regular (not test-gated) function so downstream crates' tests (e.g.
/// `astersql-dxf-example`) can shrink it the same way.
/// 测试用：设置任务检查间隔并返回旧值。
pub fn SetTaskCheckIntervalForTest(interval: Duration) -> Duration {
    let old = TaskCheckInterval();
    taskCheckIntervalNanos.store(interval.as_nanos() as u64, Ordering::Release);
    old
}
/// 节点元数据恢复循环间隔。
pub const recoverMetaInterval: Duration = Duration::from_secs(90);
/// 节点侧任务执行管理器：轮询、slot 分配、拉起/取消 executor。
pub struct Manager {
    /// 任务表访问接口。
    pub taskTable: Arc<dyn TaskTable>,
    /// 本节点已启动的任务执行器，按任务 ID 索引。
    taskExecutors: Mutex<HashMap<i64, Arc<dyn TaskExecutor>>>,
    /// 本执行节点 ID。
    pub id: String,
    /// 管理器生命周期上下文。
    ctx: Context,
    /// 后台工作线程句柄。
    workers: Mutex<Vec<JoinHandle<()>>>,
    /// 本地 CPU slot 管理器。
    pub slotManager: Arc<slotManager>,
    /// 节点资源快照。
    nodeResource: NodeResource,
    /// 服务作用域（写入节点元数据）。
    serviceScope: String,
}
/// 构造管理器；slot 容量取 `resource.TotalCPU`。
pub fn NewManager(
    ctx: Context,
    id: impl Into<String>,
    task_table: Arc<dyn TaskTable>,
    resource: NodeResource,
) -> Result<Arc<Manager>> {
    Ok(Arc::new(Manager {
        taskTable: task_table,
        taskExecutors: Mutex::new(HashMap::new()),
        id: id.into(),
        ctx,
        workers: Mutex::new(vec![]),
        slotManager: Arc::new(newSlotManager(resource.TotalCPU)),
        nodeResource: resource,
        serviceScope: String::new(),
    }))
}
impl Manager {
    /// 初始化本节点元数据（带重试）。
    pub fn InitMeta(&self) -> Result<()> {
        self.runWithRetry(
            || {
                self.taskTable
                    .InitMeta(&self.ctx, &self.id, &self.serviceScope)
            },
            "init meta failed",
        )
    }
    /// 恢复/刷新节点元数据（带重试）。
    pub fn recoverMeta(&self) -> Result<()> {
        self.runWithRetry(
            || {
                self.taskTable
                    .RecoverMeta(&self.ctx, &self.id, &self.serviceScope)
            },
            "recover meta failed",
        )
    }
    /// 启动后台循环：任务处理 + 元数据恢复。
    pub fn Start(self: &Arc<Self>) -> Result<()> {
        let manager = self.clone();
        self.workers
            .lock()
            .expect("workers lock poisoned")
            .push(thread::spawn(move || manager.handleTasksLoop()));
        let manager = self.clone();
        self.workers
            .lock()
            .expect("workers lock poisoned")
            .push(thread::spawn(move || manager.recoverMetaLoop()));
        Ok(())
    }
    /// 取消管理器上下文。
    pub fn Cancel(&self) {
        self.ctx.Cancel()
    }
    /// 停止：取消、等待 worker、关闭全部 executor。
    pub fn Stop(&self) {
        self.Cancel();
        loop {
            let workers = std::mem::take(&mut *self.workers.lock().expect("workers lock poisoned"));
            if workers.is_empty() {
                break;
            }
            for worker in workers {
                let _ = worker.join();
            }
        }
        let executors: Vec<_> = self
            .taskExecutors
            .lock()
            .expect("executors lock poisoned")
            .values()
            .cloned()
            .collect();
        for executor in executors {
            executor.Close();
        }
    }
    /// 周期轮询并处理任务。
    pub fn handleTasksLoop(self: Arc<Self>) {
        while self.waitForInterval(TaskCheckInterval()) {
            self.handleTasks();
        }
    }
    /// 一次扫描：收集可执行任务，并处理 Pausing/Reverting。
    pub fn handleTasks(self: &Arc<Self>) {
        let Ok(tasks) = self.taskTable.GetTaskExecInfoByExecID(&self.ctx, &self.id) else {
            return;
        };
        let mut executable = vec![];
        for info in tasks {
            match info.TaskBase.State {
                // Running 且尚未启动执行器 → 纳入可执行列表。
                TaskState::Running if !self.isExecutorStarted(info.TaskBase.ID) => {
                    executable.push(info)
                }
                TaskState::Pausing => {
                    let _ = self.handlePausingTask(info.TaskBase.ID);
                }
                TaskState::Reverting => {
                    let _ = self.handleRevertingTask(info.TaskBase.ID);
                }
                _ => {}
            }
        }
        self.handleExecutableTasks(&executable)
    }
    /// 按 slot 情况启动任务；若需抢占则先 Cancel 低优任务并中断本轮。
    pub fn handleExecutableTasks(self: &Arc<Self>, tasks: &[TaskExecInfo]) {
        for info in tasks {
            let (can, free) = self.slotManager.canAlloc(&info.TaskBase);
            // 需先释放低优任务占用的 slot，本轮不再继续分配。
            if !free.is_empty() {
                self.cancelTaskExecutors(&free);
                break;
            }
            if can && !self.startTaskExecutor(&info.TaskBase) {
                break;
            }
        }
    }
    /// 取消指定任务当前正在跑的子任务。
    pub fn cancelRunningSubtaskOf(&self, id: i64) {
        if let Some(executor) = self
            .taskExecutors
            .lock()
            .expect("executors lock poisoned")
            .get(&id)
        {
            executor.CancelRunningSubtask()
        }
    }
    /// 处理 Pausing：取消执行器并 Pause 子任务。
    pub fn handlePausingTask(&self, id: i64) -> Result<()> {
        if let Some(executor) = self
            .taskExecutors
            .lock()
            .expect("executors lock poisoned")
            .get(&id)
        {
            executor.Cancel()
        }
        self.taskTable.PauseSubtasks(&self.ctx, &self.id, id)
    }
    /// 处理 Reverting：取消运行中子任务并写回 Cancel。
    pub fn handleRevertingTask(&self, id: i64) -> Result<()> {
        self.cancelRunningSubtaskOf(id);
        self.taskTable.CancelSubtask(&self.ctx, &self.id, id)
    }
    /// 周期恢复节点元数据。
    pub fn recoverMetaLoop(self: Arc<Self>) {
        while self.waitForInterval(recoverMetaInterval) {
            let _ = self.recoverMeta();
        }
    }
    /// 等待一个轮询周期，并像 Go 的 ticker/select 一样及时响应上下文取消。
    fn waitForInterval(&self, interval: Duration) -> bool {
        let deadline = Instant::now() + interval;
        while !self.ctx.Done() {
            let now = Instant::now();
            if now >= deadline {
                return true;
            }
            thread::sleep((deadline - now).min(Duration::from_millis(10)));
        }
        false
    }
    /// 批量取消给定任务的执行器。
    pub fn cancelTaskExecutors(&self, tasks: &[TaskBase]) {
        let executors = self.taskExecutors.lock().expect("executors lock poisoned");
        for task in tasks {
            if let Some(executor) = executors.get(&task.ID) {
                executor.Cancel()
            }
        }
    }
    /// 分配 slot、查工厂、Init 并在独立线程 Run；失败则释放 slot。
    pub fn startTaskExecutor(self: &Arc<Self>, base: &TaskBase) -> bool {
        let Ok(task) = self.taskTable.GetTaskByID(&self.ctx, base.ID) else {
            return false;
        };
        if !self.slotManager.alloc(&task.TaskBase) {
            return false;
        }
        // 未注册的任务类型视为失败。
        let Some(factory) = GetTaskExecutorFactory(&task.TaskBase.Type) else {
            self.failSubtask(
                &ExecutorError(format!("task type {} not found", task.TaskBase.Type)),
                task.TaskBase.ID,
                None,
            );
            self.slotManager.free(task.TaskBase.ID);
            return false;
        };
        let extension = Arc::new(DefaultExtension);
        let param = Param {
            taskTable: self.taskTable.clone(),
            slotMgr: self.slotManager.clone(),
            nodeRc: self.getNodeResource(),
            execID: self.id.clone(),
            Extension: extension,
            TaskRuntime: None,
        };
        let executor = factory(self.ctx.clone(), task.clone(), param);
        if let Err(error) = executor.Init(&self.ctx) {
            self.failSubtask(&error, task.TaskBase.ID, Some(executor));
            self.slotManager.free(task.TaskBase.ID);
            return false;
        }
        self.addTaskExecutor(executor.clone());
        let manager = self.clone();
        // Run 结束后 Close、注销并释放 slot。
        let worker = thread::spawn(move || {
            executor.Run();
            executor.Close();
            manager.delTaskExecutor(executor.as_ref());
            manager.slotManager.free(task.TaskBase.ID);
        });
        self.workers
            .lock()
            .expect("workers lock poisoned")
            .push(worker);
        true
    }
    /// 返回节点资源副本。
    pub fn getNodeResource(&self) -> NodeResource {
        self.nodeResource.clone()
    }
    /// 登记已启动的执行器。
    pub fn addTaskExecutor(&self, executor: Arc<dyn TaskExecutor>) {
        self.taskExecutors
            .lock()
            .expect("executors lock poisoned")
            .insert(executor.GetTaskBase().ID, executor);
    }
    /// 移除执行器登记。
    pub fn delTaskExecutor(&self, executor: &dyn TaskExecutor) {
        self.taskExecutors
            .lock()
            .expect("executors lock poisoned")
            .remove(&executor.GetTaskBase().ID);
    }
    /// 任务是否已有执行器在跑。
    pub fn isExecutorStarted(&self, id: i64) -> bool {
        self.taskExecutors
            .lock()
            .expect("executors lock poisoned")
            .contains_key(&id)
    }
    /// 非可重试错误时将子任务标记失败。
    pub fn failSubtask(
        &self,
        error: &ExecutorError,
        id: i64,
        executor: Option<Arc<dyn TaskExecutor>>,
    ) {
        if executor.as_ref().is_some_and(|e| e.IsRetryableError(error)) {
            return;
        }
        let _ = self.runWithRetry(
            || self.taskTable.FailSubtask(&self.ctx, &self.id, id, error),
            "update to subtask failed",
        );
    }
    /// 最多 3 次指数退避重试；上下文取消则提前结束。
    pub fn runWithRetry(
        &self,
        mut action: impl FnMut() -> Result<()>,
        _message: &str,
    ) -> Result<()> {
        let mut last = Ok(());
        for attempt in 0..3 {
            last = action();
            if last.is_ok() {
                return last;
            }
            if self.ctx.Done() {
                return Err(self
                    .ctx
                    .Cause()
                    .unwrap_or_else(|| ExecutorError("context canceled".into())));
            }
            thread::sleep(Duration::from_millis(10 * 2u64.pow(attempt)));
        }
        last
    }
}
/// Manager 启动时使用的默认 Extension（幂等、空步骤执行器）。
struct DefaultExtension;
impl Extension for DefaultExtension {
    fn IsIdempotent(&self, _: &Subtask) -> bool {
        true
    }
    fn GetStepExecutor(&self, _: &Task) -> Result<Arc<dyn StepExecutor>> {
        Ok(Arc::new(BaseStepExecutor))
    }
    fn IsRetryableError(&self, _: &ExecutorError) -> bool {
        false
    }
}

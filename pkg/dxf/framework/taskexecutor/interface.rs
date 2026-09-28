// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0

// 任务执行器核心类型与 trait 定义。
//
// 定义 DXF 执行侧共用的错误类型、取消上下文、任务/子任务状态机、
// 资源描述，以及 `TaskTable`/`TaskExecutor`/`StepExecutor`/`Extension` 等接口。
// Task 表示一次分布式作业；Subtask 是其在某节点上的执行片段；Step 表示任务流水线阶段。

use std::any::Any;
use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::SystemTime;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 执行器错误：以字符串消息承载，对应 Go 侧 error 包装。
pub struct ExecutorError(pub String);
/// 将内部消息原样写出。
impl fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
/// 标记为标准 Error，便于与 `?` 及错误链协作。
impl std::error::Error for ExecutorError {}
/// 本模块统一结果类型。
pub type Result<T> = std::result::Result<T, ExecutorError>;

struct ContextState {
    cancelled: AtomicBool,
    cause: Mutex<Option<ExecutorError>>,
    parent: Option<Arc<ContextState>>,
}
impl ContextState {
    fn is_done(&self) -> bool {
        self.cancelled.load(AtomicOrdering::Acquire)
            || self.parent.as_ref().is_some_and(|p| p.is_done())
    }
    fn cause(&self) -> Option<ExecutorError> {
        self.cause
            .lock()
            .ok()
            .and_then(|cause| cause.clone())
            .or_else(|| self.parent.as_ref().and_then(|p| p.cause()))
    }
}
#[derive(Clone)]
/// 可取消上下文：用原子标志模拟 Go `context.Context` 的取消信号。
pub struct Context {
    /// 当前上下文及其父上下文的取消状态。
    state: Arc<ContextState>,
}
impl Default for Context {
    fn default() -> Self {
        Self {
            state: Arc::new(ContextState {
                cancelled: AtomicBool::new(false),
                cause: Mutex::new(None),
                parent: None,
            }),
        }
    }
}
impl Context {
    /// 创建未取消的后台上下文。
    pub fn Background() -> Self {
        Self::default()
    }
    /// 发出取消信号（Release 语义对 Done 可见）。
    pub fn Cancel(&self) {
        self.CancelWithCauseInternal(None)
    }
    /// 创建一个继承父上下文取消状态的子上下文。
    pub fn Child(parent: &Context) -> Self {
        Self {
            state: Arc::new(ContextState {
                cancelled: AtomicBool::new(false),
                cause: Mutex::new(None),
                parent: Some(parent.state.clone()),
            }),
        }
    }
    /// 以明确原因取消当前上下文。
    pub fn CancelWithCause(&self, cause: ExecutorError) {
        self.CancelWithCauseInternal(Some(cause))
    }
    fn CancelWithCauseInternal(&self, cause: Option<ExecutorError>) {
        if let Some(cause) = cause {
            if let Ok(mut current) = self.state.cause.lock() {
                if current.is_none() {
                    *current = Some(cause);
                }
            }
        }
        self.state.cancelled.store(true, AtomicOrdering::Release)
    }
    /// 查询是否已取消。
    pub fn Done(&self) -> bool {
        self.state.is_done()
    }
    /// 返回当前取消原因（包括父上下文的原因）。
    pub fn Cause(&self) -> Option<ExecutorError> {
        self.state.cause()
    }
}

/// 任务类型名，用于工厂注册表查找。
pub type TaskType = String;
/// 任务步骤（Step）：流水线中的阶段编号。
pub type Step = i64;
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 任务状态机：Pending→Running，可 Pausing/Reverting，终态 Succeed/Reverted/Failed。
pub enum TaskState {
    #[default]
    Pending,
    Running,
    Modifying,
    Pausing,
    Reverting,
    Succeed,
    Reverted,
    Failed,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 子任务状态：Pending/Running 及 Succeed/Failed/Canceled/Paused。
pub enum SubtaskState {
    #[default]
    Pending,
    Running,
    Succeed,
    Failed,
    Canceled,
    Paused,
}
#[derive(Clone, Debug, Eq, PartialEq)]
/// 任务基础元数据（不含 Meta 载荷）。
pub struct TaskBase {
    /// 任务唯一 ID。
    pub ID: i64,
    /// 业务侧任务键。
    pub Key: String,
    /// 任务类型，决定使用哪个 executor 工厂。
    pub Type: TaskType,
    /// 当前任务状态。
    pub State: TaskState,
    /// 当前所处步骤。
    pub Step: Step,
    /// 优先级；数值越小优先级越高（与 Compare 一致）。
    pub Priority: i32,
    /// 所需 CPU slot 数。
    pub RequiredSlots: i32,
    /// 创建时间，用于同优先级排序。
    pub CreateTime: SystemTime,
    /// Keyspace（多租户键空间）名；空表示默认。
    pub Keyspace: String,
}
/// 默认任务：ID=0、Pending、创建时间为 UNIX_EPOCH。
impl Default for TaskBase {
    fn default() -> Self {
        Self {
            ID: 0,
            Key: String::new(),
            Type: String::new(),
            State: TaskState::Pending,
            Step: 0,
            Priority: 0,
            RequiredSlots: 0,
            CreateTime: SystemTime::UNIX_EPOCH,
            Keyspace: String::new(),
        }
    }
}
impl TaskBase {
    /// 比较优先级：Priority → CreateTime → ID；返回 -1/0/1。
    pub fn Compare(&self, other: &Self) -> i32 {
        match self
            .Priority
            .cmp(&other.Priority)
            .then_with(|| self.CreateTime.cmp(&other.CreateTime))
            .then_with(|| self.ID.cmp(&other.ID))
        {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        }
    }
    /// 运行时占用 slot，当前等于 RequiredSlots。
    pub fn GetRuntimeSlots(&self) -> i32 {
        self.RequiredSlots
    }
}
#[derive(Clone, Debug, Default)]
/// 完整任务：基础信息 + Meta（序列化的任务参数）。
pub struct Task {
    pub TaskBase: TaskBase,
    /// 任务元数据载荷（不透明字节）。
    pub Meta: Vec<u8>,
}
#[derive(Clone, Debug, Default)]
/// 子任务基础信息。
pub struct SubtaskBase {
    /// 子任务唯一 ID。
    pub ID: i64,
    /// 所属任务 ID。
    pub TaskID: i64,
    /// 所属步骤。
    pub Step: Step,
    /// 当前子任务状态。
    pub State: SubtaskState,
    /// 执行节点 ID。
    pub ExecID: String,
}
#[derive(Clone, Debug, Default)]
/// 完整子任务：基础信息 + Meta。
pub struct Subtask {
    pub SubtaskBase: SubtaskBase,
    /// 子任务元数据载荷。
    pub Meta: Vec<u8>,
}
#[derive(Clone, Debug, Default)]
/// 供 Manager 轮询的任务执行摘要。
pub struct TaskExecInfo {
    pub TaskBase: TaskBase,
}
#[derive(Clone, Debug, Default)]
/// 节点资源容量：CPU/内存/磁盘。
pub struct NodeResource {
    /// 可用 CPU 核数（亦用作 slot 容量）。
    pub TotalCPU: i32,
    /// 可用内存字节数。
    pub TotalMem: i64,
    /// 可用磁盘字节数。
    pub TotalDisk: u64,
}
impl NodeResource {
    /// 按任务占用的 CPU slot 比例计算当前步骤资源。
    pub fn GetStepResource(&self, task: &TaskBase) -> StepResource {
        let slots = task.GetRuntimeSlots();
        let memory = if self.TotalCPU == 0 {
            0
        } else {
            (slots as f64 / self.TotalCPU as f64 * self.TotalMem as f64) as i64
        };
        StepResource {
            CPU: slots,
            Memory: memory,
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
/// 某一步骤可用的 CPU/内存配额。
pub struct StepResource {
    /// CPU 配额。
    pub CPU: i32,
    /// 内存配额。
    pub Memory: i64,
}
#[derive(Clone, Debug, Default)]
/// 子任务实时进度摘要。
pub struct SubtaskSummary {
    /// 已处理行数。
    pub RowCount: u64,
}

/// 任务/子任务持久化表抽象（对应系统表访问）。
pub trait TaskTable: Send + Sync {
    /// 按执行节点 ID 列出待处理任务摘要。
    fn GetTaskExecInfoByExecID(&self, _: &Context, _: &str) -> Result<Vec<TaskExecInfo>> {
        Ok(vec![])
    }
    /// 按 ID 加载完整任务。
    fn GetTaskByID(&self, _: &Context, id: i64) -> Result<Task>;
    /// 按状态集合查询任务列表。
    fn GetTasksInStates(&self, _: &Context, _: &[TaskState]) -> Result<Vec<Task>> {
        Ok(Vec::new())
    }
    /// 仅取任务基础信息。
    fn GetTaskBaseByID(&self, c: &Context, id: i64) -> Result<TaskBase> {
        Ok(self.GetTaskByID(c, id)?.TaskBase)
    }
    /// 按执行节点、步骤与状态过滤子任务。
    fn GetSubtasksByExecIDAndStepAndStates(
        &self,
        _: &Context,
        _: &str,
        _: i64,
        _: Step,
        _: &[SubtaskState],
    ) -> Result<Vec<Subtask>> {
        Ok(vec![])
    }
    /// 取第一个匹配状态的子任务（用于拉取待执行项）。
    fn GetFirstSubtaskInStates(
        &self,
        _: &Context,
        _: &str,
        _: i64,
        _: Step,
        _: &[SubtaskState],
    ) -> Result<Option<Subtask>> {
        Ok(None)
    }
    /// 初始化本节点在元数据表中的记录。
    fn InitMeta(&self, _: &Context, _: &str, _: &str) -> Result<()> {
        Ok(())
    }
    /// 恢复/刷新节点元数据（心跳或故障恢复）。
    fn RecoverMeta(&self, _: &Context, _: &str, _: &str) -> Result<()> {
        Ok(())
    }
    /// 将子任务标记为开始执行。
    fn StartSubtask(&self, _: &Context, _: i64, _: &str) -> Result<()> {
        Ok(())
    }
    /// 更新子任务状态及可选错误信息。
    fn UpdateSubtaskStateAndError(
        &self,
        _: &Context,
        _: &str,
        _: i64,
        _: SubtaskState,
        _: Option<&ExecutorError>,
    ) -> Result<()> {
        Ok(())
    }
    /// 将子任务标记为失败。
    fn FailSubtask(&self, _: &Context, _: &str, _: i64, _: &ExecutorError) -> Result<()> {
        Ok(())
    }
    /// 取消指定子任务。
    fn CancelSubtask(&self, _: &Context, _: &str, _: i64) -> Result<()> {
        Ok(())
    }
    /// 完成子任务并写入结果 Meta。
    fn FinishSubtask(&self, _: &Context, _: &str, _: i64, _: &[u8]) -> Result<()> {
        Ok(())
    }
    /// 暂停某任务在本节点上的子任务。
    fn PauseSubtasks(&self, _: &Context, _: &str, _: i64) -> Result<()> {
        Ok(())
    }
    /// 将运行中子任务回退为 Pending（故障恢复）。
    fn RunningSubtasksBack2Pending(&self, _: &Context, _: &[SubtaskBase]) -> Result<()> {
        Ok(())
    }
    /// 在新会话中执行回调（默认直接调用）。
    fn WithNewSession(&self, callback: &mut dyn FnMut() -> Result<()>) -> Result<()> {
        callback()
    }
    /// 更新子任务检查点（进度）。
    fn UpdateSubtaskCheckpoint(&self, _: &Context, _: i64, _: &dyn Any) -> Result<()> {
        Ok(())
    }
    /// 读取子任务检查点。
    fn GetSubtaskCheckpoint(&self, _: &Context, _: i64) -> Result<String> {
        Ok(String::new())
    }
}
/// 单任务执行器：管理该任务在本节点上的生命周期。
pub trait TaskExecutor: Send + Sync {
    /// 初始化执行器（资源/会话等）。
    fn Init(&self, ctx: &Context) -> Result<()>;
    /// 主循环：拉取并执行子任务直至结束或取消。
    fn Run(&self);
    /// 返回当前任务基础信息。
    fn GetTaskBase(&self) -> TaskBase;
    /// 取消正在运行的子任务（保留任务执行器）。
    fn CancelRunningSubtask(&self);
    /// 取消整个任务执行。
    fn Cancel(&self);
    /// 关闭并清理资源。
    fn Close(&self);
    /// 判断错误是否可重试。
    fn IsRetryableError(&self, error: &ExecutorError) -> bool;
}
/// 步骤执行器：真正跑一个 subtask 的业务逻辑。
pub trait StepExecutor: Send + Sync {
    /// 步骤初始化；默认空操作。
    fn Init(&self, _: &Context) -> Result<()> {
        Ok(())
    }
    /// 执行单个子任务；默认空操作。
    fn RunSubtask(&self, _: &Context, _: &mut Subtask) -> Result<()> {
        Ok(())
    }
    /// 实时进度；None 表示不支持。
    fn RealtimeSummary(&self) -> Option<SubtaskSummary> {
        None
    }
    /// 重置进度统计。
    fn ResetSummary(&self) {}
    /// 步骤清理。
    fn Cleanup(&self, _: &Context) -> Result<()> {
        Ok(())
    }
    /// 任务 Meta 变更回调；默认未实现。
    fn TaskMetaModified(&self, _: &Context, _: &[u8]) -> Result<()> {
        Err(ExecutorError("not implemented".into()))
    }
    /// 资源配额变更回调；默认未实现。
    fn ResourceModified(&self, _: &Context, _: &StepResource) -> Result<()> {
        Err(ExecutorError("not implemented".into()))
    }
}
/// 任务类型扩展：幂等性、步骤执行器工厂、可重试错误判断。
pub trait Extension: Send + Sync {
    /// 子任务是否幂等（崩溃恢复时可否重跑 Running 状态）。
    fn IsIdempotent(&self, subtask: &Subtask) -> bool;
    /// 按任务构造当前步骤的 `StepExecutor`。
    fn GetStepExecutor(&self, task: &Task) -> Result<Arc<dyn StepExecutor>>;
    /// 业务侧可重试错误判断。
    fn IsRetryableError(&self, error: &ExecutorError) -> bool;
}
#[derive(Default)]
/// 空步骤执行器基类，默认实现全部 no-op。
pub struct BaseStepExecutor;
/// 使用 trait 默认实现。
impl StepExecutor for BaseStepExecutor {}

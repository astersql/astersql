// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// DXF Scheduler（分布式任务框架调度器）的核心类型与 trait 定义。
//
// DXF（Distributed eXecution Framework）在 TiDB 集群上统一调度后台任务
// （如加索引、导入数据）。本模块定义 Task/Subtask 状态机、TaskManager
// （任务持久化边界）、Extension（任务类型扩展点）与 Scheduler 工厂注册表。

use crate::nodes::NodeManager;
use crate::slots::SlotManager;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Condvar, LazyLock, Mutex, RwLock};
use std::time::Duration;
use std::time::SystemTime;

/// 任务状态字符串别名（对齐 Go proto.TaskState）。
pub type TaskState = &'static str;
/// 子任务状态字符串别名。
pub type SubtaskState = &'static str;
/// 任务步骤（Step）：负数为框架保留阶段，非负为业务自定义步骤。
pub type Step = i64;

/// 任务已创建、尚未开始调度。
pub const TASK_STATE_PENDING: TaskState = "pending";
/// 任务正在执行当前步骤。
pub const TASK_STATE_RUNNING: TaskState = "running";
/// 任务成功完成。
pub const TASK_STATE_SUCCEED: TaskState = "succeed";
/// 任务因不可恢复错误失败。
pub const TASK_STATE_FAILED: TaskState = "failed";
/// 正在回滚（revert）已产生的副作用。
pub const TASK_STATE_REVERTING: TaskState = "reverting";
/// 等待人工或外部决议（如磁盘满后暂停）。
pub const TASK_STATE_AWAITING_RESOLUTION: TaskState = "awaiting-resolution";
/// 回滚完成。
pub const TASK_STATE_REVERTED: TaskState = "reverted";
/// 正在取消任务。
pub const TASK_STATE_CANCELLING: TaskState = "cancelling";
/// 正在暂停任务。
pub const TASK_STATE_PAUSING: TaskState = "pausing";
/// 任务已暂停。
pub const TASK_STATE_PAUSED: TaskState = "paused";
/// 正在从暂停恢复。
pub const TASK_STATE_RESUMING: TaskState = "resuming";
/// 正在应用运行时修改（如改并发槽位数）。
pub const TASK_STATE_MODIFYING: TaskState = "modifying";

/// 子任务待执行。
pub const SUBTASK_STATE_PENDING: SubtaskState = "pending";
/// 子任务正在某节点上执行。
pub const SUBTASK_STATE_RUNNING: SubtaskState = "running";
/// 子任务成功。
pub const SUBTASK_STATE_SUCCEED: SubtaskState = "succeed";
/// 子任务失败。
pub const SUBTASK_STATE_FAILED: SubtaskState = "failed";
/// 子任务已取消。
pub const SUBTASK_STATE_CANCELED: SubtaskState = "canceled";
/// 子任务已暂停。
pub const SUBTASK_STATE_PAUSED: SubtaskState = "paused";

/// 初始步骤：任务尚未进入业务步骤。
pub const STEP_INIT: Step = -1;
/// 完成步骤：所有业务步骤已结束。
pub const STEP_DONE: Step = -2;
/// 准备完成步骤：OnPrepare 之后、正式调度之前。
pub const STEP_PREPARED: Step = -3;
/// ExtraParams.prepare_mode：要求先走 OnPrepare。
pub const PREPARE_MODE_REQUIRED: i32 = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 调度器错误：简单字符串包装，对齐 Go 的 error 传递。
pub struct SchedulerError(pub String);

impl SchedulerError {
    /// 由消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SchedulerError {}

/// 本包统一 Result 类型。
pub type Result<T> = std::result::Result<T, SchedulerError>;

#[derive(Clone, Default)]
/// 可取消上下文：用 Condvar 通知协作取消（简化版 Go context）。
pub struct Context {
    cancelled: Arc<(Mutex<bool>, Condvar)>,
    cancellation_flag: Arc<std::sync::atomic::AtomicBool>,
}

impl Context {
    pub fn new() -> Self {
        Self::default()
    }
    /// 标记取消并唤醒等待方。
    pub fn cancel(&self) {
        self.cancellation_flag
            .store(true, std::sync::atomic::Ordering::Release);
        let (lock, wake) = &*self.cancelled;
        *lock.lock().expect("context lock poisoned") = true;
        wake.notify_all();
    }

    /// 是否已被取消。
    pub fn is_cancelled(&self) -> bool {
        self.cancellation_flag
            .load(std::sync::atomic::Ordering::Acquire)
    }

    /// 共享给对象存储的取消标志，与调度退避使用同一生命周期。
    pub fn cancellation_flag(&self) -> Arc<std::sync::atomic::AtomicBool> {
        self.cancellation_flag.clone()
    }

    /// 可被 cancel 立即中断的退避等待。
    pub fn wait(&self, duration: Duration) -> Result<()> {
        let (lock, wake) = &*self.cancelled;
        let cancelled = lock.lock().expect("context lock poisoned");
        if *cancelled {
            return Err(SchedulerError::new("context canceled"));
        }
        let (cancelled, _) = wake
            .wait_timeout(cancelled, duration)
            .expect("context wait lock poisoned");
        if *cancelled {
            Err(SchedulerError::new("context canceled"))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 任务扩展参数：恢复策略、磁盘满暂停、运行时槽位上限等。
pub struct ExtraParams {
    /// 是否需要人工介入恢复。
    pub manual_recovery: bool,
    /// KV 存储磁盘满时是否暂停任务。
    pub pause_on_kv_disk_full: bool,
    /// 运行时槽位上限；与 target_steps 配合可按步骤限流。
    pub max_runtime_slots: i32,
    /// max_runtime_slots 生效的步骤列表；空表示所有步骤。
    pub target_steps: Vec<Step>,
    /// 准备模式；见 PREPARE_MODE_REQUIRED。
    pub prepare_mode: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 任务基础元数据（不含 meta/错误等扩展字段）。
pub struct TaskBase {
    /// 任务 ID。
    pub id: i64,
    /// 业务唯一键（同类型任务去重）。
    pub key: String,
    /// 任务类型，用于查找 SchedulerFactory / Extension。
    pub task_type: String,
    /// 当前任务状态。
    pub state: TaskState,
    /// 当前步骤。
    pub step: Step,
    /// 优先级，数值越小越优先（见 compare）。
    pub priority: i32,
    /// 任务声明需要的槽位数。
    pub required_slots: i32,
    /// 目标节点角色范围（如 background）；空串时可能默认选 background。
    pub target_scope: String,
    /// 创建时间，用于同优先级排序。
    pub create_time: SystemTime,
    /// 最多使用的节点数；0 表示不限制。
    pub max_node_count: i32,
    /// 扩展参数。
    pub extra_params: ExtraParams,
    /// Keyspace（多租户键空间）标识。
    pub keyspace: String,
}

impl TaskBase {
    /// 是否已到终态（成功/已回滚/失败）。
    pub fn is_done(&self) -> bool {
        matches!(
            self.state,
            TASK_STATE_SUCCEED | TASK_STATE_REVERTED | TASK_STATE_FAILED
        )
    }

    /// 比较任务调度次序：优先 priority，再 create_time，再 id。
    /// 对齐 Go Compare：返回值更小表示 `self` 排名更高。
    /// Go's Compare contract: a negative result means `self` has higher rank.
    pub fn compare(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            .then_with(|| self.create_time.cmp(&other.create_time))
            .then_with(|| self.id.cmp(&other.id))
    }

    /// 实际运行占用的槽位数：若配置了 max_runtime_slots 且当前步骤命中
    /// target_steps（或列表为空），则取上限与 required_slots 的较小值。
    pub fn runtime_slots(&self) -> i32 {
        let limit = self.extra_params.max_runtime_slots;
        if limit > 0
            && (self.extra_params.target_steps.is_empty()
                || self.extra_params.target_steps.contains(&self.step))
        {
            limit.min(self.required_slots)
        } else {
            self.required_slots
        }
    }
}

impl Default for TaskBase {
    fn default() -> Self {
        Self {
            id: 0,
            key: String::new(),
            task_type: String::new(),
            state: TASK_STATE_PENDING,
            step: STEP_INIT,
            priority: 512,
            required_slots: 0,
            target_scope: String::new(),
            create_time: SystemTime::UNIX_EPOCH,
            max_node_count: 0,
            extra_params: ExtraParams::default(),
            keyspace: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 运行时修改项：类型与目标值（如改 RequiredSlots）。
pub struct Modification {
    /// 修改类型标识。
    pub kind: String,
    /// 修改目标值。
    pub to: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 完整任务：基础字段 + meta、错误、修改参数等。
pub struct Task {
    /// 基础元数据。
    pub base: TaskBase,
    /// 任务类型自定义元数据（通常为 JSON）。
    pub meta: Vec<u8>,
    /// 最近一次错误（失败/回滚原因）。
    pub error: Option<SchedulerError>,
    /// 进入 modifying 等状态前的先前状态。
    pub previous_state: TaskState,
    /// 待应用或已记录的修改列表。
    pub modifications: Vec<Modification>,
}

impl Default for Task {
    fn default() -> Self {
        Self {
            base: TaskBase::default(),
            meta: b"{}".to_vec(),
            error: None,
            previous_state: TASK_STATE_PENDING,
            modifications: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 子任务基础字段：归属任务、步骤、状态与执行节点。
pub struct SubtaskBase {
    /// 子任务 ID。
    pub id: i64,
    /// 所属任务 ID。
    pub task_id: i64,
    /// 所属步骤。
    pub step: Step,
    /// 子任务状态。
    pub state: SubtaskState,
    /// 执行节点 ID（ExecID）。
    pub exec_id: String,
    /// 子任务并发度。
    pub concurrency: i32,
    /// 同步骤内序数，用于稳定排序。
    pub ordinal: i32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 完整子任务：基础字段 + meta 与错误。
pub struct Subtask {
    /// 基础字段。
    pub base: SubtaskBase,
    /// 子任务自定义元数据。
    pub meta: Vec<u8>,
    /// 子任务错误。
    pub error: Option<SchedulerError>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 框架管理的节点视图：ID、角色与 CPU 核数。
pub struct ManagedNode {
    /// 节点 ID。
    pub id: String,
    /// 节点角色（如 background 或空）。
    pub role: String,
    /// CPU 核数，用作 slot capacity 参考。
    pub cpu_count: i32,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 节点资源总量（CPU/内存/磁盘），供调度决策参考。
pub struct NodeResource {
    /// 总 CPU。
    pub total_cpu: i32,
    /// 总内存字节数。
    pub total_memory: i64,
    /// 总磁盘容量。
    pub total_disk: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 上一批子任务汇总信息（如处理行数）。
pub struct SubtaskSummary {
    /// 处理行数。
    pub row_count: i64,
}

/// 调度器使用的存储边界（TaskManager）。方法对齐 Go TaskManager 的事务边界；
/// 具体实现决定如何获取 session。
/// Storage boundary used by the scheduler. Methods follow the Go TaskManager
/// transaction boundaries; implementations decide how sessions are acquired.
pub trait TaskManager: Send + Sync {
    fn top_unfinished_tasks(&self) -> Result<Vec<TaskBase>>;
    fn top_no_need_resource_tasks(&self) -> Result<Vec<TaskBase>>;
    fn all_tasks(&self) -> Result<Vec<TaskBase>>;
    fn all_subtasks(&self) -> Result<Vec<SubtaskBase>>;
    fn tasks_in_states(&self, states: &[TaskState]) -> Result<Vec<Task>>;
    /// Finished tasks bounded by the owner-local cleanup setting.
    fn cleanup_tasks(&self) -> Result<Vec<Task>> {
        let mut tasks =
            self.tasks_in_states(&[TASK_STATE_FAILED, TASK_STATE_REVERTED, TASK_STATE_SUCCEED])?;
        tasks.truncate(crate::proto::GetTaskCleanupBatchSize() as usize);
        Ok(tasks)
    }
    fn task_by_id(&self, task_id: i64) -> Result<Task>;
    fn task_base_by_id(&self, task_id: i64) -> Result<TaskBase>;
    fn all_nodes(&self) -> Result<Vec<ManagedNode>>;
    fn delete_dead_nodes(&self, nodes: &[String]) -> Result<()>;
    fn transfer_tasks_to_history(&self, tasks: &[Task]) -> Result<()>;
    fn gc_subtasks(&self) -> Result<()>;
    fn fail_task(&self, task_id: i64, current: TaskState, error: SchedulerError) -> Result<()>;
    fn revert_task(&self, task_id: i64, current: TaskState, error: SchedulerError) -> Result<()>;
    fn awaiting_resolve_task(
        &self,
        task_id: i64,
        current: TaskState,
        error: SchedulerError,
    ) -> Result<()>;
    fn reverted_task(&self, task_id: i64) -> Result<()>;
    fn paused_task(&self, task_id: i64) -> Result<()>;
    /// 因可恢复错误暂停任务，并把当前步骤的失败子任务转换为 paused。
    fn pause_task_on_error(
        &self,
        task_id: i64,
        current: TaskState,
        step: Step,
        error: SchedulerError,
    ) -> Result<()>;
    fn resumed_task(&self, task_id: i64) -> Result<()>;
    fn modified_task(&self, task: &Task) -> Result<()>;
    fn succeed_task(&self, task_id: i64) -> Result<()>;
    fn switch_task_step(
        &self,
        task: &Task,
        next_state: TaskState,
        next_step: Step,
        subtasks: &[Subtask],
    ) -> Result<()>;
    fn switch_task_step_in_batch(
        &self,
        task: &Task,
        next_state: TaskState,
        next_step: Step,
        subtasks: &[Subtask],
    ) -> Result<()>;
    fn switch_task_step_after_prepare(&self, task: &Task) -> Result<bool>;
    fn used_slots_on_nodes(&self) -> Result<HashMap<String, i32>>;
    fn active_subtasks(&self, task_id: i64) -> Result<Vec<SubtaskBase>>;
    fn subtask_count_by_states(
        &self,
        task_id: i64,
        step: Step,
    ) -> Result<HashMap<SubtaskState, i64>>;
    fn subtask_errors(&self, task_id: i64) -> Result<Vec<SchedulerError>>;
    fn resume_subtasks(&self, task_id: i64) -> Result<()>;
    fn update_subtask_exec_ids(&self, subtasks: &[SubtaskBase]) -> Result<()>;
    fn previous_subtask_metas(&self, task_id: i64, step: Step) -> Result<Vec<Vec<u8>>>;
    fn previous_subtask_summaries(&self, task_id: i64, step: Step) -> Result<Vec<SubtaskSummary>>;
}

/// 扩展回调可用的任务句柄：读取上一批 subtask 的 meta 与 summary。
pub trait TaskHandle: Send + Sync {
    fn previous_subtask_metas(&self, task_id: i64, step: Step) -> Result<Vec<Vec<u8>>>;
    fn previous_subtask_summaries(&self, task_id: i64, step: Step) -> Result<Vec<SubtaskSummary>>;
}

/// 任务类型扩展点：由具体任务（加索引等）实现步骤推进、选节点与 meta 修改。
pub trait Extension: Send + Sync {
    /// 调度周期回调（心跳/进度刷新）。
    fn on_tick(&self, _task: &Task) {}
    /// 接收 Scheduler 生命周期 context；旧扩展默认沿用原回调。
    fn on_tick_with_context(&self, _context: &Context, task: &Task) {
        self.on_tick(task);
    }
    /// 生成下一步的一批子任务 meta。
    fn on_next_subtasks_batch(
        &self,
        handle: &dyn TaskHandle,
        task: &mut Task,
        exec_ids: &[String],
        next_step: Step,
    ) -> Result<Vec<Vec<u8>>>;
    fn on_next_subtasks_batch_with_context(
        &self,
        _context: &Context,
        handle: &dyn TaskHandle,
        task: &mut Task,
        exec_ids: &[String],
        next_step: Step,
    ) -> Result<Vec<Vec<u8>>> {
        self.on_next_subtasks_batch(handle, task, exec_ids, next_step)
    }
    /// 任务完成时的清理回调。
    fn on_done(&self, handle: &dyn TaskHandle, task: &mut Task) -> Result<()>;
    fn on_done_with_context(
        &self,
        _context: &Context,
        handle: &dyn TaskHandle,
        task: &mut Task,
    ) -> Result<()> {
        self.on_done(handle, task)
    }
    /// 返回可执行该任务的节点实例列表。
    fn eligible_instances(&self, task: &Task) -> Result<Vec<String>>;
    /// 错误是否可重试。
    fn is_retryable_error(&self, error: &SchedulerError) -> bool;
    /// 根据当前任务计算下一步 Step。
    fn next_step(&self, task: &TaskBase) -> Step;
    /// 正式调度前的准备阶段（可改写 task meta）。
    fn on_prepare(&self, handle: &dyn TaskHandle, task: &mut Task) -> Result<()>;
    fn on_prepare_with_context(
        &self,
        _context: &Context,
        handle: &dyn TaskHandle,
        task: &mut Task,
    ) -> Result<()> {
        self.on_prepare(handle, task)
    }
    /// 按修改列表变换 meta。
    fn modify_meta(&self, old_meta: &[u8], modifications: &[Modification]) -> Result<Vec<u8>>;
}

#[derive(Clone)]
/// 构造 Scheduler 时注入的依赖参数。
pub struct Param {
    /// 任务存储管理器。
    pub task_manager: Arc<dyn TaskManager>,
    /// 节点管理器。
    pub node_manager: Arc<NodeManager>,
    /// 槽位管理器。
    pub slot_manager: Arc<SlotManager>,
    /// 本机 server ID。
    pub server_id: String,
    /// 是否已为本任务预留槽位。
    pub allocated_slots: bool,
    /// 可选的本机资源快照。
    pub node_resource: Option<NodeResource>,
}

impl Param {
    /// 返回节点资源引用。
    pub fn node_resource(&self) -> Option<&NodeResource> {
        self.node_resource.as_ref()
    }
}

/// 单个任务的调度器：初始化、推进一次、关闭与查询。
pub trait Scheduler: Send + Sync {
    /// 初始化调度器。
    fn init(&self) -> Result<()>;
    /// 推进一次调度；返回是否还需继续。
    fn schedule_once(&self) -> Result<bool>;
    /// 关闭并释放资源。
    fn close(&self);
    /// 当前任务快照。
    fn task(&self) -> Task;
    /// 任务类型扩展。
    fn extension(&self) -> Arc<dyn Extension>;
}

/// 按任务类型创建 Scheduler 的工厂。
pub type SchedulerFactory = Arc<dyn Fn(Task, Param) -> Arc<dyn Scheduler> + Send + Sync + 'static>;
/// 创建清理例程的工厂。
pub type CleanUpFactory = Arc<dyn Fn() -> Arc<dyn CleanUpRoutine> + Send + Sync + 'static>;

/// 全局 Scheduler 工厂注册表（按 task_type）。
static SCHEDULER_FACTORIES: LazyLock<RwLock<HashMap<String, SchedulerFactory>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
/// 全局清理工厂注册表。
static CLEANUP_FACTORIES: LazyLock<RwLock<HashMap<String, CleanUpFactory>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// 注册任务类型对应的 Scheduler 工厂（保留 Go 风格函数名）。
pub fn RegisterSchedulerFactory(task_type: impl Into<String>, factory: SchedulerFactory) {
    SCHEDULER_FACTORIES
        .write()
        .expect("scheduler factory lock poisoned")
        .insert(task_type.into(), factory);
}

/// 按任务类型查找 Scheduler 工厂。
pub fn get_scheduler_factory(task_type: &str) -> Option<SchedulerFactory> {
    SCHEDULER_FACTORIES
        .read()
        .expect("scheduler factory lock poisoned")
        .get(task_type)
        .cloned()
}

/// 清空所有 Scheduler 工厂（测试用）。
pub fn ClearSchedulerFactory() {
    SCHEDULER_FACTORIES
        .write()
        .expect("scheduler factory lock poisoned")
        .clear();
}

/// 任务完成后的清理例程。
pub trait CleanUpRoutine: Send + Sync {
    /// 执行清理。
    fn clean_up(&self, task: &mut Task) -> Result<()>;
    /// Optional batched capability; the owner invokes one instance per task type.
    fn batch_cleanup(&self) -> Option<&dyn BatchCleanUpRoutine> {
        None
    }
}

/// A successful group is transferred together; failures transfer none of that
/// group. Side effects and history transfer have no atomicity or rollback, so
/// implementations must be idempotent after partial failure and retry.
pub trait BatchCleanUpRoutine: CleanUpRoutine {
    fn clean_up_batch(&self, tasks: &mut [Task]) -> Result<()>;
}

/// 注册任务类型对应的清理工厂。
pub fn RegisterSchedulerCleanUpFactory(task_type: impl Into<String>, factory: CleanUpFactory) {
    CLEANUP_FACTORIES
        .write()
        .expect("cleanup factory lock poisoned")
        .insert(task_type.into(), factory);
}

/// 按任务类型查找清理工厂。
pub fn get_scheduler_cleanup_factory(task_type: &str) -> Option<CleanUpFactory> {
    CLEANUP_FACTORIES
        .read()
        .expect("cleanup factory lock poisoned")
        .get(task_type)
        .cloned()
}

/// 清空所有清理工厂（测试用）。
pub fn ClearSchedulerCleanUpFactory() {
    CLEANUP_FACTORIES
        .write()
        .expect("cleanup factory lock poisoned")
        .clear();
}

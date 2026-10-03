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
// DXF 任务（task）协议：状态、类型、优先级、并发上限与 ExtraParams。
//
// 任务是分布式框架的顶层作业单位，按 step 推进；`TaskBase` 不含大块 Meta
// 以便轻量调度。owner 节点可用内存旋钮 `maxConcurrentTask` 紧急调高并发。

// limitations under the License.

use super::modify::ModifyParam;
use super::step::{Step, Step2Str};
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering as CmpOrdering;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::SystemTime;

// see doc.go for more details.
/// 任务状态字符串别名。
pub type TaskState = &'static str;
/// 任务类型字符串别名（如 ImportInto、backfill）。
pub type TaskType = &'static str;
/// 是否进入 prepare-mode 调度的整型开关。
pub type PrepareMode = i32;

/// 等待调度。
pub const TaskStatePending: TaskState = "pending";
/// 正在运行。
pub const TaskStateRunning: TaskState = "running";
/// 成功完成（终态）。
pub const TaskStateSucceed: TaskState = "succeed";
/// 失败（终态）。
pub const TaskStateFailed: TaskState = "failed";
/// 正在回滚。
pub const TaskStateReverting: TaskState = "reverting";
/// 等待人工介入（如开启 ManualRecovery 后失败）。
pub const TaskStateAwaitingResolution: TaskState = "awaiting-resolution";
/// 已回滚完成（终态）。
pub const TaskStateReverted: TaskState = "reverted";
/// 正在取消。
pub const TaskStateCancelling: TaskState = "cancelling";
/// 正在暂停。
pub const TaskStatePausing: TaskState = "pausing";
/// 已暂停。
pub const TaskStatePaused: TaskState = "paused";
/// 正在从暂停恢复。
pub const TaskStateResuming: TaskState = "resuming";
/// 正在修改任务参数（如槽位/节点数）。
pub const TaskStateModifying: TaskState = "modifying";

// PrepareModeDisabled means prepare mode is disabled, this is the default.
/// 关闭 prepare-mode（默认，兼容旧行为）。
pub const PrepareModeDisabled: PrepareMode = 0;
// PrepareModeRequired means task scheduling must enter prepare mode.
/// 调度必须先走 prepare-mode。
pub const PrepareModeRequired: PrepareMode = 1;

// TaskTypeExt 对应 Go 中 TaskType.String，直接暴露底层任务类型字符串。
/// 返回类型字符串。
/// 任务类型扩展：对齐 Go TaskType.String。
pub trait TaskTypeExt {
    fn String(&self) -> String;
}

impl TaskTypeExt for TaskType {
    fn String(&self) -> String {
        (*self).to_string()
    }
}

// TaskStateExt 对应 Go 中 TaskState.String 与 CanMoveToModifying 方法。
/// 任务状态扩展：String 与可否进入 modifying。
pub trait TaskStateExt {
    /// 返回状态字符串。
    fn String(&self) -> String;
    /// 是否允许转入 modifying（pending/running/paused）。
    fn CanMoveToModifying(&self) -> bool;
}

impl TaskStateExt for TaskState {
    fn String(&self) -> String {
        (*self).to_string()
    }

    /// pending/running/paused 才可进入 modifying。
    // CanMoveToModifying checks if current state can move to 'modifying' state.
    fn CanMoveToModifying(&self) -> bool {
        *self == TaskStatePending || *self == TaskStateRunning || *self == TaskStatePaused
    }
}

// PrepareModeExt 对应 Go 中 PrepareMode.String，未知值保留 unknown(n) 格式。
/// disabled / required / unknown(n)。
/// PrepareMode 的可读字符串。
pub trait PrepareModeExt {
    fn String(&self) -> String;
}

impl PrepareModeExt for PrepareMode {
    fn String(&self) -> String {
        match *self {
            PrepareModeDisabled => "disabled".to_string(),
            PrepareModeRequired => "required".to_string(),
            _ => format!("unknown({})", self),
        }
    }
}

// TaskIDLabelName is the label name of task id.
/// 指标/标签中的任务 ID 键名。
pub const TaskIDLabelName: &str = "task_id";
// NormalPriority represents the normal priority of task.
/// 默认任务优先级（数值越小优先级越高）。
pub const NormalPriority: i32 = 512;

// maxConcurrentTaskLowerBound is the minimum allowed DXF task concurrency.
/// DXF 任务并发下限。
pub const maxConcurrentTaskLowerBound: i32 = 16;
// MaxConcurrentTaskUpperBound is the current safety cap for DXF task concurrency.
// TODO: remove this cap after the DXF scheduler no longer runs all schedulers on the owner node.
/// DXF 任务并发安全上限（scheduler 全在 owner 上跑时的保护）。
pub const MaxConcurrentTaskUpperBound: i32 = 1000;
// DefaultMaxConcurrentTask is the default DXF task concurrency.
/// 默认最大并发任务数。
pub const DefaultMaxConcurrentTask: i32 = maxConcurrentTaskLowerBound;

// maxConcurrentTask is an owner-local emergency tuning knob for DXF scheduling.
// It is intentionally kept in memory only: it is not persisted to TiKV, is reset
// on restart, and only affects the TiDB node that receives the update. Operators
// should change it through the DXF owner node when many small tasks are blocked
// by the default limit. Raising it increases scheduler overhead and memory usage
// on the owner, so the owner node may need a larger resource spec first.
/// owner 本地、仅内存的紧急并发旋钮；重启丢失，不写 TiKV。
static maxConcurrentTask: AtomicI64 = AtomicI64::new(DefaultMaxConcurrentTask as i64);

// init 在 Go 中负责初始化 atomic.Int64；Rust static 初始化已在声明处完成。

// GetMaxConcurrentTask returns the max concurrency of task.
/// 读取当前最大并发任务数。
pub fn GetMaxConcurrentTask() -> i32 {
    maxConcurrentTask.load(Ordering::SeqCst) as i32
}

// SetMaxConcurrentTask updates the max concurrency of task.
/// 在合法区间内更新最大并发；越界返回错误。
pub fn SetMaxConcurrentTask(value: i32) -> Result<(), String> {
    if value < maxConcurrentTaskLowerBound || value > MaxConcurrentTaskUpperBound {
        return Err(format!(
            "max_concurrent_task {} is out of range [{}, {}]",
            value, maxConcurrentTaskLowerBound, MaxConcurrentTaskUpperBound
        ));
    }

    // Go 使用 atomic.Store；这里保持 owner 本地内存旋钮的原子更新语义。
    maxConcurrentTask.store(value as i64, Ordering::SeqCst);
    Ok(())
}

// SetMaxConcurrentTaskForTest updates the max concurrency of task and returns a restore function.
/// 测试用设置，并返回恢复旧值的闭包。
pub fn SetMaxConcurrentTaskForTest(value: i32) -> impl FnOnce() {
    let old = GetMaxConcurrentTask();
    maxConcurrentTask.store(value as i64, Ordering::SeqCst);
    // Go 返回闭包用于测试收尾；用 FnOnce 表达恢复动作。
    move || {
        maxConcurrentTask.store(old as i64, Ordering::SeqCst);
    }
}

/// Owner-local, memory-only cleanup query bound; reset on process restart.
pub const DefaultTaskCleanupBatchSize: i64 = 20;
pub const TaskCleanupBatchSizeUpperBound: i64 = 1000;
static taskCleanupBatchSize: AtomicI64 = AtomicI64::new(DefaultTaskCleanupBatchSize);

pub fn GetTaskCleanupBatchSize() -> i64 {
    taskCleanupBatchSize.load(Ordering::SeqCst)
}
pub fn SetTaskCleanupBatchSize(value: i64) -> Result<(), String> {
    if !(1..=TaskCleanupBatchSizeUpperBound).contains(&value) {
        return Err(format!(
            "task_cleanup_batch_size {value} is out of range [1, {TaskCleanupBatchSizeUpperBound}]"
        ));
    }
    taskCleanupBatchSize.store(value, Ordering::SeqCst);
    Ok(())
}
pub fn SetTaskCleanupBatchSizeForTest(value: i64) -> impl FnOnce() {
    let old = taskCleanupBatchSize.swap(value, Ordering::SeqCst);
    move || {
        taskCleanupBatchSize.store(old, Ordering::SeqCst);
    }
}

// ExtraParams is the extra params of task.
// Note: only store params that's not used for filter or sort in this struct.
#[derive(Default, Deserialize, Serialize)]
/// 任务扩展参数（不参与过滤/排序的字段）。
pub struct ExtraParams {
    // ManualRecovery indicates whether the task can be recovered manually.
    // if enabled, the task will enter 'awaiting-resolution' state when it failed,
    // then the user can recover the task manually or fail it if it's not recoverable.
    #[serde(
        default,
        rename = "manual_recovery",
        skip_serializing_if = "std::ops::Not::not"
    )]
    /// 失败后进入 awaiting-resolution，供人工恢复或判定不可恢复。
    pub ManualRecovery: bool,
    // PauseOnKVDiskFull indicates whether the task should be paused instead of
    // reverted when TiKV reports disk full.
    #[serde(
        default,
        rename = "pause_on_kv_disk_full",
        skip_serializing_if = "std::ops::Not::not"
    )]
    /// TiKV 磁盘满时暂停而非回滚。
    pub PauseOnKVDiskFull: bool,
    // MaxRuntimeSlots is the max slots when running subtasks of this task in
    // TargetSteps steps.
    // normally it's 0, means we will use the RequiredSlots to run the subtasks.
    // if set, we will use the min of RequiredSlots and MaxRuntimeSlots as the
    // execution effective slots of the task step defined in TargetSteps.
    // this field is used to workaround OOM issue where TiDB might repeatedly
    // restart. the DXF framework won't detect changes in this field, so it's not
    // part of normal schedule workflow, when TiDB restarts the newest value will
    // be used.
    // RequiredSlots might be modified, and MaxRuntimeSlots is not touched in this
    // case due to above reason, so MaxRuntimeSlots might > RequiredSlots.
    #[serde(
        default,
        rename = "max_runtime_slots",
        skip_serializing_if = "is_zero_i32"
    )]
    /// 运行期有效槽位上限；0 表示用 RequiredSlots；用于缓解 OOM 反复重启。
    pub MaxRuntimeSlots: i32,
    // TargetSteps indicates the steps that MaxRuntimeSlots takes effect.
    // if empty or nil, MaxRuntimeSlots takes effect in all steps.
    // normally OOM only happens in some specific steps, so we can just limit the
    // concurrency in those steps to reduce the impact on the overall performance.
    #[serde(
        default,
        rename = "target_steps",
        skip_serializing_if = "Vec::is_empty"
    )]
    /// MaxRuntimeSlots 生效的 step 列表；空表示所有 step。
    pub TargetSteps: Vec<Step>,
    // PrepareMode controls whether this task requires prepare-mode scheduling.
    // default is PrepareModeDisabled for backward compatibility.
    #[serde(default, rename = "prepare_mode", skip_serializing_if = "is_zero_i32")]
    /// 是否要求 prepare-mode 调度。
    pub PrepareMode: PrepareMode,
}

// serde skip_serializing_if：零值不写出 JSON 字段。
fn is_zero_i32(value: &i32) -> bool {
    *value == 0
}

// TaskBase contains the basic information of a task.
// we define this to avoid load task meta which might be very large into memory.
/// 任务主键。
/// 任务基础信息（不含可能很大的 Meta）。
pub struct TaskBase {
    /// 业务侧任务唯一键。
    pub ID: i64,
    /// 任务类型。
    pub Key: String,
    /// 当前状态。
    pub Type: TaskType,
    /// 当前执行 step。
    pub State: TaskState,
    pub Step: Step,
    // Priority is the priority of task, the smaller value means the higher priority.
    /// 优先级，越小越高；合法范围 [1, 1024]。
    // valid range is [1, 1024], default is NormalPriority.
    pub Priority: i32,
    // RequiredSlots is the required slots of the task.
    // we use this field to allocate slots when scheduling and creating the task
    // executor, but the effective slots when running the task is determined by
    // GetRuntimeSlots.
    // in normal case, they are the same. but when meeting OOM and TiDB repeatedly
    // restarts, we might set a lower MaxRuntimeSlots in ExtraParams, then the
    // effective slots is smaller than RequiredSlots.
    // Note: in application layer, don't use this field directly, use GetRuntimeSlots
    // or GetResource of step executor instead.
    // Note: in the system table, we store it inside 'concurrency' column as
    /// 调度/创建 executor 时申请的槽位；实际运行可能被 MaxRuntimeSlots 压低。
    // required slots is introduced later.
    pub RequiredSlots: i32,
    // TargetScope indicates that the task should be running on tidb nodes which
    // contain the tidb_service_scope=TargetScope label.
    // To be compatible with previous version, if it's "" or "background", the
    // task try run on nodes of "background" scope,
    /// 目标 tidb_service_scope；空或 background 时兼容旧节点选择逻辑。
    // if there is no such nodes, will try nodes of "" scope.
    /// 创建时间。
    pub TargetScope: String,
    /// 最多使用的执行节点数。
    pub CreateTime: SystemTime,
    /// 扩展参数。
    pub MaxNodeCount: i32,
    pub ExtraParams: ExtraParams,
    // keyspace name is the keyspace that the task belongs to.
    /// 所属 keyspace（nextgen 集群）。
    // it's only useful for nextgen cluster.
    pub Keyspace: String,
}

impl TaskBase {
    /// 终态：succeed / reverted / failed。
    // IsDone checks if the task is done.
    pub fn IsDone(&self) -> bool {
        self.State == TaskStateSucceed
            || self.State == TaskStateReverted
            || self.State == TaskStateFailed
    }

    /// 与完整 Task 比较排名（委托 Compare）。
    // CompareTask a wrapper of Compare.
    pub fn CompareTask(&self, other: &Task) -> i32 {
        self.Compare(&other.TaskBase)
    }

    // Compare compares two tasks by task rank.
    /// 按优先级、创建时间、ID 比较；返回值 <0 表示 self 排名更高。
    // returns < 0 represents rank of t is higher than 'other'.
    pub fn Compare(&self, other: &TaskBase) -> i32 {
        // Go 先比较优先级，再比较创建时间，最后按 ID 稳定排序。
        if let Some(r) = cmp_to_i32(self.Priority.cmp(&other.Priority)) {
            return r;
        }
        if let Some(r) = cmp_to_i32(self.CreateTime.cmp(&other.CreateTime)) {
            return r;
        }
        cmp_to_i32(self.ID.cmp(&other.ID)).unwrap_or(0)
    }

    // GetRuntimeSlots gets the runtime slots of current task step.
    /// 当前 step 的有效运行槽位（考虑 MaxRuntimeSlots 与 TargetSteps）。
    // application layer might use this as the concurrency of the task step.
    pub fn GetRuntimeSlots(&self) -> i32 {
        // 有运行期上限时：无 TargetSteps 则全局生效，否则仅匹配当前 step。
        if self.ExtraParams.MaxRuntimeSlots > 0 {
            if self.ExtraParams.TargetSteps.is_empty() {
                return self.ExtraParams.MaxRuntimeSlots.min(self.RequiredSlots);
            }
            for step in &self.ExtraParams.TargetSteps {
                if *step == self.Step {
                    return self.ExtraParams.MaxRuntimeSlots.min(self.RequiredSlots);
                }
            }
        }
        self.RequiredSlots
    }

    /// 人类可读摘要，step 经 Step2Str 展开。
    // String implements fmt.Stringer interface.
    pub fn String(&self) -> String {
        let create_time: DateTime<Utc> = self.CreateTime.into();
        format!(
            "{{id: {}, key: {}, type: {}, state: {}, step: {}, priority: {}, required slots: {}, target scope: {}, create time: {}}}",
            self.ID,
            self.Key,
            self.Type,
            self.State,
            Step2Str(self.Type, self.Step),
            self.Priority,
            self.RequiredSlots,
            self.TargetScope,
            create_time.to_rfc3339_opts(SecondsFormat::AutoSi, true)
        )
    }
}

// cmp_to_i32 对应 Go 的 cmp.Compare 返回约定；Equal 时返回 None，便于继续比较下一字段。
// 相等返回 None 以便继续比下一字段；否则映射为 -1/1。
fn cmp_to_i32(ordering: CmpOrdering) -> Option<i32> {
    match ordering {
        CmpOrdering::Less => Some(-1),
        CmpOrdering::Greater => Some(1),
        CmpOrdering::Equal => None,
    }
}

// Task represents the task of distributed framework, see doc.go for more details.
/// 基础字段。
/// 完整任务：基础字段 + 调度器 ID、时间戳、Meta、错误与修改参数。
pub struct Task {
    pub TaskBase: TaskBase,
    /// 调度器 ID（当前未使用）。
    // SchedulerID is not used now.
    /// 任务开始时间。
    pub SchedulerID: String,
    /// 状态最近更新时间。
    pub StartTime: SystemTime,
    pub StateUpdateTime: SystemTime,
    // Meta is the metadata of task, it's read-only in most cases, but it can be
    // changed in below case, and framework will update the task meta in the storage.
    // 	- task switches to next step in Scheduler.OnNextSubtasksBatch
    // 	- on task cleanup, we might do some redaction on the meta.
    /// 任务元数据；切 step、cleanup 脱敏、modifying 时可能被框架改写。
    // 	- on task 'modifying', params inside the meta can be changed.
    /// 失败/回滚相关错误信息。
    pub Meta: Vec<u8>,
    /// 正在/待应用的修改参数。
    pub Error: Option<String>,
    pub ModifyParam: ModifyParam,
}

// 投影到 TaskBase，兼容 Go 嵌入字段访问。
impl Deref for Task {
    type Target = TaskBase;

    fn deref(&self) -> &Self::Target {
        &self.TaskBase
    }
}

impl DerefMut for Task {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.TaskBase
    }
}

// EmptyMeta is the empty meta of task/subtask.
/// 空 JSON 元数据占位。
pub static EmptyMeta: &[u8] = b"{}";

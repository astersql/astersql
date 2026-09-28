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
//
// DXF 子任务（subtask）协议类型与步骤资源配额。
//
// 子任务是任务某一 step 在单节点上的执行单元；同 step 的多个 subtask 可在
// 不同节点并行，但单节点同一时刻最多跑一个 subtask。另提供无锁 `Allocatable`
// 与 `StepResource`（CPU/内存配额）供执行侧限流。

use super::step::Step;
use super::task::TaskType;
use bytesize::KIB;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::SystemTime;

// see doc.go for more details.
/// 子任务状态字符串别名（见系统表/文档中的状态机）。
pub type SubtaskState = &'static str;

/// 已创建、尚未开始执行。
pub const SubtaskStatePending: SubtaskState = "pending";
/// 正在执行。
pub const SubtaskStateRunning: SubtaskState = "running";
/// 执行成功（终态之一）。
pub const SubtaskStateSucceed: SubtaskState = "succeed";
/// 执行失败（终态之一）。
pub const SubtaskStateFailed: SubtaskState = "failed";
/// 已取消（终态之一）。
pub const SubtaskStateCanceled: SubtaskState = "canceled";
/// 已暂停（非终态，可恢复）。
pub const SubtaskStatePaused: SubtaskState = "paused";

// SubtaskStateExt 对应 Go 中 SubtaskState 的 String 方法，直接返回底层字符串。
/// 返回状态字符串本身。
/// 子任务状态扩展：对齐 Go 的 String 方法。
pub trait SubtaskStateExt {
    fn String(&self) -> String;
}

impl SubtaskStateExt for SubtaskState {
    fn String(&self) -> String {
        (*self).to_string()
    }
}

// SubtaskBase contains the basic information of a subtask.
// we define this to avoid load subtask meta which might be very large into memory.
/// 子任务主键 ID。
/// 子任务基础信息（不含可能很大的 Meta），便于列表/调度时轻量加载。
pub struct SubtaskBase {
    /// 所属任务 step（框架或业务阶段）。
    pub ID: i64,
    /// 所属任务类型（如 ImportInto、backfill）。
    pub Step: Step,
    pub Type: TaskType,
    /// 父任务 ID（来自子任务表 task_key）。
    // taken from task_key of the subtask table
    /// 当前子任务状态。
    pub TaskID: i64,
    pub State: SubtaskState,
    // Concurrency is the concurrency of the subtask.
    // it's initialized as the task's required slots, and it's NOT used now.
    // if the required slot of task is modified, the concurrency of un-finished
    // subtasks of the task will be updated too.
    // some subtasks like post-process of import into, don't consume too many resources,
    /// 并发度：初值多为任务所需槽位数；部分轻量 step 可调低（尚未全面使用）。
    // can lower this value, can use this field to implement such feature later.
    pub Concurrency: i32,
    // ExecID is the ID of target executor, right now it's the same as instance_id,
    /// 目标执行节点 ID，形如 IP:PORT（见 GenerateExecID）。
    // its value is IP:PORT, see GenerateExecID
    /// 创建时间。
    pub ExecID: String,
    pub CreateTime: SystemTime,
    // StartTime is the time when the subtask is started.
    /// 开始时间；未开始时为 epoch 零值。
    // it's 0 if it hasn't started yet.
    pub StartTime: SystemTime,
    // Ordinal is the ordinal of subtask, should be unique for some task and step.
    /// 同任务同 step 内的序号，从 1 起且应唯一。
    // starts from 1.
    pub Ordinal: i32,
}

/// 调试用摘要，字段顺序对齐 Go 的 fmt 输出。
impl SubtaskBase {
    // String 对应 Go 的 fmt.Sprintf 输出，保持字段顺序和展示内容。
    pub fn String(&self) -> String {
        format!(
            "[ID={}, Step={}, Type={}, TaskID={}, State={}, ExecID={}]",
            self.ID, self.Step, self.Type, self.TaskID, self.State, self.ExecID
        )
    }

    /// 是否已到终态（成功/取消/失败）。
    // IsDone checks if the subtask is done.
    pub fn IsDone(&self) -> bool {
        // Go 这里用状态常量做完成态判断；保留相同的三态集合。
        self.State == SubtaskStateSucceed
            || self.State == SubtaskStateCanceled
            || self.State == SubtaskStateFailed
    }
}

// Subtask represents the subtask of distribute framework.
// subtasks of a task are run in parallel on different nodes, but on each node,
// at most 1 subtask can be run at the same time, see StepExecutor too.
/// 基础字段（通过 Deref 可直接访问）。
/// 完整子任务：在 SubtaskBase 上叠加更新时间、Meta 与 Summary。
pub struct Subtask {
    pub SubtaskBase: SubtaskBase,
    // UpdateTime is the time when the subtask is updated.
    // it can be used as subtask end time if the subtask is finished.
    /// 最近更新时间；完成后可当作结束时间。
    // it's 0 if it hasn't started yet.
    pub UpdateTime: SystemTime,
    // Meta is the metadata of subtask, should not be nil.
    // meta of different subtasks of same step must be different too.
    // NOTE: this field can be changed by StepExecutor.OnFinished method, to store
    // some result, and framework will update the subtask meta in the storage.
    /// 子任务元数据；同 step 各 subtask 的 meta 应不同；OnFinished 可写回结果。
    // On other code path, this field should be read-only.
    /// 执行摘要/统计文本。
    pub Meta: Vec<u8>,
    pub Summary: String,
}

// NewSubtask create a new subtask.
/// 构造新子任务；初始 State 为空、时间戳为 epoch，ID 由存储侧分配。
pub fn NewSubtask(
    step: Step,
    taskID: i64,
    tp: TaskType,
    execID: String,
    concurrency: i32,
    meta: Vec<u8>,
    ordinal: i32,
) -> Box<Subtask> {
    // Go 返回 *Subtask；这里用 Box 表达“堆上拥有的子任务”。
    Box::new(Subtask {
        SubtaskBase: SubtaskBase {
            ID: 0,
            Step: step,
            Type: tp,
            TaskID: taskID,
            State: "",
            ExecID: execID,
            CreateTime: SystemTime::UNIX_EPOCH,
            StartTime: SystemTime::UNIX_EPOCH,
            Concurrency: concurrency,
            Ordinal: ordinal,
        },
        UpdateTime: SystemTime::UNIX_EPOCH,
        Meta: meta,
        Summary: String::new(),
    })
}

// 将 Subtask 投影为 SubtaskBase，便于沿用 Go 的嵌入字段访问习惯。
impl Deref for Subtask {
    type Target = SubtaskBase;

    fn deref(&self) -> &Self::Target {
        &self.SubtaskBase
    }
}

impl DerefMut for Subtask {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.SubtaskBase
    }
}

// Allocatable is a resource with capacity that can be allocated, it's routine safe.
/// 总容量。
/// 可分配资源池：容量固定，已用量原子更新，协程/线程安全。
pub struct Allocatable {
    /// 当前已占用。
    capacity: i64,
    used: AtomicI64,
}

// NewAllocatable creates a new Allocatable.
/// 创建指定容量的可分配资源。
pub fn NewAllocatable(capacity: i64) -> Allocatable {
    Allocatable {
        capacity,
        used: AtomicI64::new(0),
    }
}

impl Allocatable {
    /// 返回总容量。
    // Capacity returns the capacity of the Allocatable.
    pub fn Capacity(&self) -> i64 {
        self.capacity
    }

    /// 返回当前已用量。
    // Used returns the used resource of the Allocatable.
    pub fn Used(&self) -> i64 {
        self.used.load(Ordering::SeqCst)
    }

    /// 尝试占用 `n`；超容量则失败，否则 CAS 重试直至成功。
    // Alloc allocates v from the Allocatable.
    pub fn Alloc(&self, n: i64) -> bool {
        loop {
            // 读取当前 used，若 used+n 超容量则失败；否则 CAS 写入。
            let used = self.used.load(Ordering::SeqCst);
            let next = used.wrapping_add(n);
            if next > self.capacity {
                return false;
            }

            // Go 使用 atomic.Int64.CompareAndSwap 做无锁抢占；这里保留 CAS 重试语义。
            if self
                .used
                .compare_exchange(used, next, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return true;
            }
        }
    }

    /// 释放 `n`；与 Go 一样不做下溢保护。
    // Free frees v from the Allocatable.
    pub fn Free(&self, n: i64) {
        // Go 的 Add(-n) 不做下界保护；Rust 实现保持相同释放语义。
        self.used.fetch_sub(n, Ordering::SeqCst);
    }
}

// StepResource is the max resource that a task step can use.
// it's also the max resource that a subtask can use, as we run subtasks of task
// step in sequence.
/// CPU 配额（通常按核/槽位抽象）。
/// 任务某一 step 可用的最大资源（也是单 subtask 上限，因同 step 串行执行）。
pub struct StepResource {
    /// 内存配额（字节）。
    pub CPU: Allocatable,
    pub Mem: Allocatable,
}

impl StepResource {
    /// 人类可读的 CPU/内存容量摘要。
    // String implements Stringer interface.
    pub fn String(&self) -> String {
        format!(
            "[CPU={}, Mem={}]",
            self.CPU.Capacity(),
            bytes_size(self.Mem.Capacity() as f64)
        )
    }

    // MemoryPerCore returns the memory per core of the StepResource.
    /// 每核内存；CPU 容量非正时退回总内存。
    // When CPU capacity is not positive, it falls back to returning total memory.
    pub fn MemoryPerCore(&self) -> i64 {
        if self.CPU.Capacity() <= 0 {
            return self.Mem.Capacity();
        }
        self.Mem.Capacity() / self.CPU.Capacity()
    }
}

// 将字节数格式化为带单位的短字符串（对齐 Go humanize/类似展示）。
fn bytes_size(size: f64) -> String {
    const UNITS: [&str; 9] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB", "ZiB", "YiB"];
    let mut value = size;
    let mut unit = 0;
    while value >= KIB as f64 && unit < UNITS.len() - 1 {
        value /= KIB as f64;
        unit += 1;
    }
    format!("{}{}", format_go_general_4(value), UNITS[unit])
}

/// 对齐 Go `fmt` 的 `%.4g`：四位有效数字，并使用相同的科学计数法阈值。
fn format_go_general_4(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }

    let mut exponent = value.abs().log10().floor() as i32;
    if !(-4..4).contains(&exponent) {
        let mut mantissa = value / 10_f64.powi(exponent);
        let rounded = format!("{mantissa:.3}").parse::<f64>().unwrap();
        if rounded.abs() >= 10.0 {
            mantissa = rounded / 10.0;
            exponent += 1;
        }
        let mut number = format!("{mantissa:.3}");
        if number.contains('.') {
            number = number
                .trim_end_matches('0')
                .trim_end_matches('.')
                .to_string();
        }
        return format!("{number}e{exponent:+03}");
    }

    let precision = (3 - exponent).max(0) as usize;
    let mut number = format!("{value:.precision$}");
    if number.contains('.') {
        number = number
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string();
    }
    number
}

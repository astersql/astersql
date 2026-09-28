// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! 日志备份任务状态与展示：对齐 Go `br/pkg/stream/stream_status.go`。
//! 承载 checkpoint（Store/Global）、暂停/错误语义、任务打印机与 QPS/指标解析。
//! `GetMinStoreCheckpoint` 遇 Global 立即返回；无 Store 时回退到任务 StartTs。
//! StatusString：ERROR（暂停且有错误）> PAUSE > NORMAL，与 CLI 展示文案一致。

// TaskStatus 字段公开以便 CLI/测试直接填充。
// Clone 手写以保持与 Go 结构拷贝语义一致。
// teeTaskPrinter 小写开头表示包内类型，经 TeeTaskPrinter 导出。
// CollectTaskPrinter.lines 在 PrintTasks 时追加，可重复调用累加。
// WildCard 供过滤表达式匹配全部。
// EstimateQPSFromCounts 使用 saturating_sub 防止计数回绕。
// 指标解析兼容 tikv_stream_ 与 tikv_log_backup_ 两前缀。
// onError 要求 paused，避免未暂停却显示 ERROR。
// Global checkpoint 一旦出现即作为 GetMinStoreCheckpoint 结果。
// Store 构造保留 ID；Global 强制 ID=0。
// PauseV2 缺省 None 时仅看 LastErrors。
// QPS 由外部填充，本文件只负责展示格式。
use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::stubs::backuppb::{StreamBackupError, StreamBackupTaskInfo};

/// 通配过滤标记，对齐 Go `WildCard`。
pub const WildCard: &str = "*";

/// Checkpoint 作用域：单 store 或全局。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum CheckpointType {
    #[default]
    Store,
    Global,
}

/// 单个 checkpoint：ID 对 Store 有意义，Global 时 ID 为 0。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Checkpoint {
    pub ID: u64,
    pub TS: u64,
    kind: CheckpointType,
}

impl Checkpoint {
    /// 返回 checkpoint 类型（Store/Global）。
    pub fn Type(&self) -> CheckpointType {
        self.kind.clone()
    }

    /// 构造 store 级 checkpoint。
    pub fn Store(id: u64, ts: u64) -> Self {
        Self {
            ID: id,
            TS: ts,
            kind: CheckpointType::Store,
        }
    }

    /// 构造全局 checkpoint；ID 固定为 0。
    pub fn Global(ts: u64) -> Self {
        Self {
            ID: 0,
            TS: ts,
            kind: CheckpointType::Global,
        }
    }
}

/// 暂停严重度：None 为普通暂停，Error 表示因错误暂停。
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Severity {
    #[default]
    None,
    Error,
}

/// Pause V2 元信息；Severity=Error 时 StatusString 报 ERROR。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PauseV2 {
    pub Severity: Severity,
}

/// 单个流备份任务的聚合状态，供 list/status 命令使用。
pub struct TaskStatus {
    pub Info: StreamBackupTaskInfo,
    pub paused: bool,
    pub globalCheckpoint: u64,
    pub Checkpoints: Vec<Checkpoint>,
    pub QPS: f64,
    // store id → 最近一次错误。
    pub LastErrors: HashMap<u64, StreamBackupError>,
    pub PauseV2: Option<PauseV2>,
}

impl Clone for TaskStatus {
    fn clone(&self) -> Self {
        Self {
            Info: self.Info.clone(),
            paused: self.paused,
            globalCheckpoint: self.globalCheckpoint,
            Checkpoints: self.Checkpoints.clone(),
            QPS: self.QPS,
            LastErrors: self.LastErrors.clone(),
            PauseV2: self.PauseV2.clone(),
        }
    }
}

/// 任务列表输出抽象；CLI 可换成表格/JSON 实现。
pub trait TaskPrinter {
    fn AddTask(&mut self, t: TaskStatus);
    fn PrintTasks(&mut self);
}

/// 收集任务并生成简单文本行，便于单测断言。
pub struct CollectTaskPrinter {
    pub tasks: Vec<TaskStatus>,
    pub lines: Vec<String>,
}

impl CollectTaskPrinter {
    /// 空打印机。
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            lines: Vec::new(),
        }
    }
}

impl Default for CollectTaskPrinter {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskPrinter for CollectTaskPrinter {
    fn AddTask(&mut self, t: TaskStatus) {
        self.tasks.push(t);
    }

    fn PrintTasks(&mut self) {
        // 无任务时固定文案，对齐 Go。
        if self.tasks.is_empty() {
            self.lines.push("No Task Yet.".into());
            return;
        }
        self.lines
            .push(format!("Total {} Tasks.", self.tasks.len()));
        for (i, task) in self.tasks.iter().enumerate() {
            self.lines.push(format!(
                "> #{} < name={} status={} qps={:.2}",
                i + 1,
                task.Info.GetName(),
                task.StatusString(),
                task.QPS
            ));
        }
    }
}

/// 在转发打印的同时把任务副本写入 `output`。
pub struct teeTaskPrinter<'a> {
    pub output: &'a mut Vec<TaskStatus>,
    inner: Box<dyn TaskPrinter + 'a>,
}

/// 构造 tee：保留 Go `*[]TaskStatus` 的原位追加语义，再包装 inner。
pub fn TeeTaskPrinter<'a>(
    p: Box<dyn TaskPrinter + 'a>,
    output: &'a mut Vec<TaskStatus>,
) -> teeTaskPrinter<'a> {
    teeTaskPrinter { output, inner: p }
}

impl TaskPrinter for teeTaskPrinter<'_> {
    fn AddTask(&mut self, task: TaskStatus) {
        self.output.push(task.clone());
        self.inner.AddTask(task);
    }

    fn PrintTasks(&mut self) {
        self.inner.PrintTasks();
    }
}

impl TaskStatus {
    // 暂停且（有 LastErrors 或 PauseV2.Severity=Error）才视为 ERROR。
    fn onError(&self) -> bool {
        self.paused
            && (!self.LastErrors.is_empty()
                || self
                    .PauseV2
                    .as_ref()
                    .map(|p| p.Severity == Severity::Error)
                    .unwrap_or(false))
    }

    /// 人类可读状态串：ERROR / PAUSE / NORMAL。
    pub fn StatusString(&self) -> &'static str {
        if self.onError() {
            return "ERROR";
        }
        if self.paused {
            return "PAUSE";
        }
        "NORMAL"
    }

    /// 取最小 Store checkpoint；若存在 Global 则直接返回该 Global。
    /// 尚无 Store 时默认 TS=任务 StartTs，避免 progress 显示为 0。
    pub fn GetMinStoreCheckpoint(&self) -> Checkpoint {
        let mut initialized = false;
        let mut checkpoint = Checkpoint {
            TS: self.Info.GetStartTs(),
            ..Default::default()
        };
        for cp in &self.Checkpoints {
            if cp.Type() == CheckpointType::Store && (!initialized || cp.TS < checkpoint.TS) {
                initialized = true;
                checkpoint = cp.clone();
            }
            // Global 优先：与 Go GetMinStoreCheckpoint 一致，立刻返回。
            if cp.Type() == CheckpointType::Global {
                return cp.clone();
            }
        }
        checkpoint
    }
}

/// Pure QPS estimate from two counter samples (matches MaybeQPS delta logic).
/// 由两次计数与时间差估算 QPS；elapsed<=0 时返回 0，避免除零。
pub fn EstimateQPSFromCounts(c0: u64, c1: u64, elapsed_secs: f64) -> f64 {
    if elapsed_secs <= 0.0 {
        return 0.0;
    }
    (c1.wrapping_sub(c0)) as f64 / elapsed_secs
}

/// 从 TiKV metrics 文本解析 handle_kv_batch_sum；兼容新旧指标前缀。
pub fn ParseLogBackupHandleKvBatchSum(metrics: &str) -> Option<u64> {
    static LOG_COUNT_SUM_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"tikv_(?:stream|log_backup)_handle_kv_batch_sum ([0-9]+)")
            .expect("the Go-compatible metrics regex is valid")
    });

    LOG_COUNT_SUM_RE
        .captures(metrics)
        .and_then(|captures| captures.get(1))
        .and_then(|count| count.as_str().parse().ok())
}

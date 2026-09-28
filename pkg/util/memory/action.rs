// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 内存超限动作（ActionOnExceed）与 OOM 兜底链。
//
// Tracker 配额超限时按优先级触发日志、杀死查询（SQLKiller）等动作；
// `BaseOOMAction` 维护 fallback 链并跳过已 finished 的节点。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::tracker::Tracker;
use crate::logutil::log::{BgLogger, LogField, LogLevel};
use crate::sqlkiller;

/// 获取互斥锁；若已 poison 则仍取出内层守卫。
fn lock_unpoisoned<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Shared equivalent of a Go `ActionOnExceed` interface value.
/// 对应 Go `ActionOnExceed` 接口值的共享句柄（`Arc`+`Mutex`）。
pub type ActionHandle = Arc<Mutex<dyn ActionOnExceed + Send>>;

/// Action taken when memory usage exceeds a tracker's quota.
///
/// Implementors must be thread-safe, matching the Go interface contract.
/// Tracker 内存超过配额时执行的动作；实现须线程安全。
pub trait ActionOnExceed: Send {
    fn Action(&self, tracker: &Tracker);
    fn SetFallback(&self, action: Option<ActionHandle>);
    fn GetFallback(&self) -> Option<ActionHandle>;
    fn GetPriority(&self) -> i64;
    fn SetFinished(&self);
    fn IsFinished(&self) -> bool;
}

/// 包装底层动作并覆盖 `GetPriority`。
pub struct actionWithPriority {
    action: ActionHandle,
    priority: i64,
}

/// 构造带自定义优先级的动作包装。
pub fn NewActionWithPriority(action: ActionHandle, priority: i64) -> actionWithPriority {
    actionWithPriority { action, priority }
}

impl ActionOnExceed for actionWithPriority {
    fn Action(&self, tracker: &Tracker) {
        lock_unpoisoned(&self.action).Action(tracker);
    }

    fn SetFallback(&self, action: Option<ActionHandle>) {
        lock_unpoisoned(&self.action).SetFallback(action);
    }

    fn GetFallback(&self) -> Option<ActionHandle> {
        lock_unpoisoned(&self.action).GetFallback()
    }

    fn GetPriority(&self) -> i64 {
        self.priority
    }

    fn SetFinished(&self) {
        lock_unpoisoned(&self.action).SetFinished();
    }

    fn IsFinished(&self) -> bool {
        lock_unpoisoned(&self.action).IsFinished()
    }
}

/// Common fallback and completion state embedded by OOM actions.
/// 各 OOM 动作内嵌的 fallback 与 finished 状态。
pub struct BaseOOMAction {
    fallbackAction: Mutex<Option<ActionHandle>>,
    finished: AtomicBool,
}

impl Default for BaseOOMAction {
    fn default() -> Self {
        Self {
            fallbackAction: Mutex::new(None),
            finished: AtomicBool::new(false),
        }
    }
}

impl BaseOOMAction {
    /// 设置兜底动作句柄。
    pub fn SetFallback(&self, action: Option<ActionHandle>) {
        *lock_unpoisoned(&self.fallbackAction) = action;
    }

    /// 标记本动作已完成，后续 GetFallback 会跳过。
    pub fn SetFinished(&self) {
        self.finished.store(true, Ordering::SeqCst);
    }

    /// 是否已标记完成。
    pub fn IsFinished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// Returns the first unfinished fallback, unlinking finished actions.
    /// 返回第一个未完成的 fallback，并顺带摘除已 finished 的节点。
    pub fn GetFallback(&self) -> Option<ActionHandle> {
        loop {
            let fallback = lock_unpoisoned(&self.fallbackAction).clone()?;
            let finished = lock_unpoisoned(&fallback).IsFinished();
            if !finished {
                return Some(fallback);
            }

            let next = lock_unpoisoned(&fallback).GetFallback();
            self.SetFallback(next);
        }
    }

    /// 若存在未完成 fallback，则对其调用 Action。
    pub fn TriggerFallBackAction(&self, tracker: &Tracker) {
        if let Some(fallback) = self.GetFallback() {
            lock_unpoisoned(&fallback).Action(tracker);
        }
    }
}

/// PanicOnExceed 默认优先级（数值最小）。
pub const DefPanicPriority: i64 = 0;
/// LogOnExceed 默认优先级。
pub const DefLogPriority: i64 = 1;
/// 磁盘溢出（spill）默认优先级。
pub const DefSpillPriority: i64 = 2;
/// 游标 fetch 溢出默认优先级。
pub const DefCursorFetchSpillPriority: i64 = 3;
/// 限速默认优先级。
pub const DefRateLimitPriority: i64 = 4;

/// 可选日志钩子：参数为连接 ID（ConnID）。
type LogHook = Box<dyn Fn(u64) + Send + Sync>;

/// 组装“内存超配额”告警的公共日志字段。
fn memory_exceeded_fields(tracker: &Tracker) -> [LogField; 4] {
    [
        LogField::I64("label".to_owned(), i64::from(tracker.Label())),
        LogField::I64("consumed".to_owned(), tracker.BytesConsumed()),
        LogField::I64("quota".to_owned(), tracker.GetBytesLimit()),
        LogField::String("tracker".to_owned(), tracker.String()),
    ]
}

/// Logs a warning only once when memory usage exceeds the quota.
/// 配额超限时只记录一次警告（或调用 hook）。
pub struct LogOnExceed {
    logHook: Mutex<Option<LogHook>>,
    pub BaseOOMAction: BaseOOMAction,
    pub ConnID: u64,
    acted: Mutex<bool>,
}

impl Default for LogOnExceed {
    fn default() -> Self {
        Self::new(0)
    }
}

impl LogOnExceed {
    /// 按连接 ID 构造 LogOnExceed。
    pub fn new(conn_id: u64) -> Self {
        Self {
            logHook: Mutex::new(None),
            BaseOOMAction: BaseOOMAction::default(),
            ConnID: conn_id,
            acted: Mutex::new(false),
        }
    }

    /// 注入测试/观测用日志钩子。
    pub fn SetLogHook(&self, hook: LogHook) {
        *lock_unpoisoned(&self.logHook) = Some(hook);
    }
}

impl ActionOnExceed for LogOnExceed {
    fn Action(&self, tracker: &Tracker) {
        // 仅首次超限生效，后续调用直接返回。
        let mut acted = lock_unpoisoned(&self.acted);
        if *acted {
            return;
        }
        *acted = true;

        if let Some(hook) = lock_unpoisoned(&self.logHook).as_ref() {
            hook(self.ConnID);
        } else {
            BgLogger().log(
                LogLevel::Warn,
                "memory exceeds quota",
                memory_exceeded_fields(tracker),
            );
        }
    }

    fn SetFallback(&self, action: Option<ActionHandle>) {
        self.BaseOOMAction.SetFallback(action);
    }

    fn GetFallback(&self) -> Option<ActionHandle> {
        self.BaseOOMAction.GetFallback()
    }

    fn GetPriority(&self) -> i64 {
        DefLogPriority
    }

    fn SetFinished(&self) {
        self.BaseOOMAction.SetFinished();
    }

    fn IsFinished(&self) -> bool {
        self.BaseOOMAction.IsFinished()
    }
}

/// Sends `QueryMemoryExceeded` and raises the resulting SQLKiller error.
/// 发送 `QueryMemoryExceeded` 杀查询信号，并将 HandleSignal 错误以 panic 抛出。
pub struct PanicOnExceed {
    pub Killer: Option<Arc<sqlkiller::SQLKiller>>,
    logHook: Mutex<Option<LogHook>>,
    pub BaseOOMAction: BaseOOMAction,
    pub ConnID: u64,
    acted: Mutex<bool>,
}

impl Default for PanicOnExceed {
    fn default() -> Self {
        Self {
            Killer: None,
            logHook: Mutex::new(None),
            BaseOOMAction: BaseOOMAction::default(),
            ConnID: 0,
            acted: Mutex::new(false),
        }
    }
}

impl PanicOnExceed {
    /// 绑定 SQLKiller 与连接 ID 构造 PanicOnExceed。
    pub fn new(killer: Arc<sqlkiller::SQLKiller>, conn_id: u64) -> Self {
        Self {
            Killer: Some(killer),
            ConnID: conn_id,
            ..Self::default()
        }
    }

    /// 注入仅首次触发时调用的日志钩子。
    pub fn SetLogHook(&self, hook: LogHook) {
        *lock_unpoisoned(&self.logHook) = Some(hook);
    }
}

impl ActionOnExceed for PanicOnExceed {
    fn Action(&self, tracker: &Tracker) {
        // 首次才打日志/hook；无论是否首次都发送杀查询信号。
        let mut acted = lock_unpoisoned(&self.acted);
        if !*acted {
            if let Some(hook) = lock_unpoisoned(&self.logHook).as_ref() {
                hook(self.ConnID);
            } else {
                let mut fields = memory_exceeded_fields(tracker).to_vec();
                fields.insert(
                    0,
                    LogField::U64("conn".to_owned(), tracker.SessionID.Load()),
                );
                BgLogger().log(LogLevel::Warn, "memory exceeds quota", fields);
            }
        }
        *acted = true;

        let killer = self
            .Killer
            .as_ref()
            .expect("PanicOnExceed.Killer is nil, matching Go nil pointer failure");
        killer.SendKillSignal(sqlkiller::QueryMemoryExceeded);
        if let Err(error) = killer.HandleSignal() {
            panic!("{error}");
        }
    }

    fn SetFallback(&self, action: Option<ActionHandle>) {
        self.BaseOOMAction.SetFallback(action);
    }

    fn GetFallback(&self) -> Option<ActionHandle> {
        self.BaseOOMAction.GetFallback()
    }

    fn GetPriority(&self) -> i64 {
        DefPanicPriority
    }

    fn SetFinished(&self) {
        self.BaseOOMAction.SetFinished();
    }

    fn IsFinished(&self) -> bool {
        self.BaseOOMAction.IsFinished()
    }
}

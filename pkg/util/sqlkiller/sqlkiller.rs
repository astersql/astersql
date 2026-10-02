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

// SQL 查询终止（SQL Killer）工具：记录首个 kill 原因、广播事件并映射为执行错误。
//
// 对应 Go `pkg/util/sqlkiller`。会话在执行过程中可被中断、超时报错、内存超限、
// runaway 控制或内存仲裁器强制终止；`HandleSignal` 把原子信号转成 `SharedError`。

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::errors::{ErrorArg, SharedError};
use crate::exeerrors;
use crate::logutil::log::{BgLogger, LogField, LogLevel};

/// kill 信号取值类型，与 Go `uint32` / iota 常量对齐。
pub type KillSignal = u32;

// KillSignal types. Values must remain aligned with the Go iota declaration.
/// 未指定 / 无 kill。
pub const UnspecifiedKillSignal: KillSignal = 0;
/// 查询被中断（如 KILL QUERY、连接断开）。
pub const QueryInterrupted: KillSignal = 1;
/// 超过 `max_execution_time` 等语句最长执行时间。
pub const MaxExecTimeExceeded: KillSignal = 2;
/// 单条 SQL 内存配额超限。
pub const QueryMemoryExceeded: KillSignal = 3;
/// 实例级（tidb-server）全局内存控制器触发。
pub const ServerMemoryExceeded: KillSignal = 4;
/// runaway 查询管控在 TiDB 侧判定超限。
pub const RunawayQueryExceeded: KillSignal = 5;
/// 内存仲裁器（mem arbitrator）主动杀掉查询。
pub const KilledByMemArbitrator: KillSignal = 6;

/// 一次性广播事件的内部状态：关闭标志 + 条件变量。
#[derive(Debug, Default)]
struct KillEvent {
    closed: Mutex<bool>,
    ready: Condvar,
}

/// A clonable receiver for the one-shot broadcast used by `SQLKiller`.
/// 可克隆的一次性 kill 事件接收端；关闭后所有 waiter 被唤醒。
#[derive(Clone, Debug)]
pub struct KillEventChan {
    event: Arc<KillEvent>,
}

impl KillEventChan {
    /// 创建未关闭的新事件通道。
    fn new() -> Self {
        Self {
            event: Arc::new(KillEvent::default()),
        }
    }

    /// 将事件标为已关闭并唤醒全部等待者（幂等）。
    fn close(&self) {
        let mut closed = self.event.closed.lock().expect("kill event mutex poisoned");
        if !*closed {
            *closed = true;
            self.event.ready.notify_all();
        }
    }

    /// 查询事件是否已关闭（即已发出 kill）。
    pub fn is_closed(&self) -> bool {
        *self.event.closed.lock().expect("kill event mutex poisoned")
    }

    /// Waits until the event is closed, returning false only on timeout.
    /// 阻塞直到事件关闭；仅超时时返回 false。
    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let closed = self.event.closed.lock().expect("kill event mutex poisoned");
        if *closed {
            return true;
        }
        let (closed, _) = self
            .event
            .ready
            .wait_timeout_while(closed, timeout, |closed| !*closed)
            .expect("kill event mutex poisoned while waiting");
        *closed
    }
}

/// 当前会话 kill 事件通道与附加描述（如仲裁原因文案）。
#[derive(Debug, Default)]
struct KillEventState {
    ch: Option<KillEventChan>,
    desc: String,
    triggered: bool,
}

/// Mutex-backed equivalent of the Go atomic pointer API used by this package.
/// 互斥锁实现的原子指针，对应 Go `atomic.Pointer` 的 Load/Store。
pub struct AtomicPointer<T> {
    value: Mutex<Option<T>>,
}

impl<T: Clone> AtomicPointer<T> {
    /// 读取当前指针值（克隆）。
    pub fn Load(&self) -> Option<T> {
        self.value
            .lock()
            .expect("AtomicPointer mutex poisoned")
            .clone()
    }

    /// 写入新指针值。
    pub fn Store(&self, value: Option<T>) {
        *self.value.lock().expect("AtomicPointer mutex poisoned") = value;
    }
}

impl<T> Default for AtomicPointer<T> {
    fn default() -> Self {
        Self {
            value: Mutex::new(None),
        }
    }
}

/// 连接是否仍存活的回调；返回 false 表示应中断查询。
pub type ConnectionAliveFn = Arc<dyn Fn() -> bool + Send + Sync + 'static>;
/// 结果集写完后的收尾回调。
pub type FinishFn = Box<dyn FnMut() + Send + 'static>;

/// Records the first query kill reason and wakes all event waiters.
/// 会话级 SQL Killer：记录首个 kill 原因并唤醒事件等待方。
pub struct SQLKiller {
    /// 可选的结果集完成回调。
    pub Finish: Mutex<Option<FinishFn>>,
    killEvent: Mutex<KillEventState>,
    /// 连接 ID，写入错误参数与日志。
    pub ConnID: AtomicU64,
    // Kept as a distinct field to preserve the Go structure's synchronization contract.
    /// 与 Go 一致的独立锁，保护 Finish 回调的设置/清理/调用。
    pub FinishFuncLock: Mutex<()>,
    /// 原子 kill 信号；仅首次从 Unspecified 写入成功。
    pub Signal: AtomicU32,
    /// 是否正在写出结果集（Go 结构字段保留）。
    pub InWriteResultSet: AtomicBool,
    lastCheckTime: AtomicPointer<Instant>,
    /// 可选的连接存活检查函数。
    pub IsConnectionAlive: AtomicPointer<ConnectionAliveFn>,
}

#[cfg(test)]
#[path = "go_merge_34_test.rs"]
mod go_merge_34_test;

#[cfg(test)]
#[path = "sqlkiller_test.rs"]
mod sqlkiller_test;

impl Default for SQLKiller {
    fn default() -> Self {
        Self {
            Finish: Mutex::new(None),
            killEvent: Mutex::new(KillEventState::default()),
            ConnID: AtomicU64::new(0),
            FinishFuncLock: Mutex::new(()),
            Signal: AtomicU32::new(UnspecifiedKillSignal),
            InWriteResultSet: AtomicBool::new(false),
            lastCheckTime: AtomicPointer::default(),
            IsConnectionAlive: AtomicPointer::default(),
        }
    }
}

impl SQLKiller {
    /// 构造默认 SQLKiller。
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a receiver closed when a kill signal is sent.
    /// 返回 kill 事件接收端；若已触发则返回已关闭通道。
    pub fn GetKillEventChan(&self) -> KillEventChan {
        let mut event = self.killEvent.lock().expect("killEvent mutex poisoned");
        if let Some(ch) = &event.ch {
            return ch.clone();
        }

        let ch = KillEventChan::new();
        if event.triggered {
            ch.close();
        }
        event.ch = Some(ch.clone());
        ch
    }

    /// 将当前事件标为已触发并关闭通道（只生效一次）。
    fn triggerKillEventLocked(event: &mut KillEventState) {
        if event.triggered {
            return;
        }
        if let Some(ch) = &event.ch {
            ch.close();
        }
        event.triggered = true;
    }

    /// 重置事件状态：关闭未触发的旧通道并清空描述。
    fn resetKillEventLocked(event: &mut KillEventState) {
        if !event.triggered
            && let Some(ch) = &event.ch
        {
            ch.close();
        }
        event.ch = None;
        event.triggered = false;
        event.desc.clear();
    }

    /// 发送带附加原因描述的 kill 信号（用于内存仲裁等）。
    pub fn SendKillSignalWithKillEventReason(&self, signal: KillSignal, desc: String) {
        let (sent, event_desc) = {
            let mut event = self.killEvent.lock().expect("killEvent mutex poisoned");
            event.desc = desc;
            let result = self.sendKillSignalLocked(signal, &event);
            Self::triggerKillEventLocked(&mut event);
            result
        };
        if sent {
            self.logKillSignal(signal, &event_desc);
        }
    }

    /// 仅首次成功写入 Signal；调用方已持有 killEvent 锁。
    fn sendKillSignalLocked(&self, reason: KillSignal, event: &KillEventState) -> (bool, String) {
        let sent = self
            .Signal
            .compare_exchange(
                UnspecifiedKillSignal,
                reason,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok();
        (
            sent,
            if sent {
                event.desc.clone()
            } else {
                String::new()
            },
        )
    }

    /// 对内部发送路径也使用同一把锁；日志在锁外输出。
    fn sendKillSignal(&self, reason: KillSignal) {
        let (sent, desc) = {
            let event = self.killEvent.lock().expect("killEvent mutex poisoned");
            self.sendKillSignalLocked(reason, &event)
        };
        if sent {
            self.logKillSignal(reason, &desc);
        }
    }

    fn logKillSignal(&self, reason: KillSignal, desc: &str) {
        let error = self.getKillError(reason, desc);
        BgLogger().log(
            LogLevel::Warn,
            "kill initiated",
            [
                LogField::U64(
                    "connection ID".to_owned(),
                    self.ConnID.load(Ordering::SeqCst),
                ),
                LogField::String(
                    "reason".to_owned(),
                    error.map_or_else(
                        || "unspecified kill signal".to_owned(),
                        |err| err.to_string(),
                    ),
                ),
            ],
        );
    }

    /// 发送 kill 信号并触发事件广播。
    pub fn SendKillSignal(&self, reason: KillSignal) {
        let (sent, desc) = {
            let mut event = self.killEvent.lock().expect("killEvent mutex poisoned");
            let result = self.sendKillSignalLocked(reason, &event);
            Self::triggerKillEventLocked(&mut event);
            result
        };
        if sent {
            fail::eval("go_merge_34_before_log_kill_signal", |_| {});
            self.logKillSignal(reason, &desc);
        }
    }

    /// 读取当前原子 kill 信号。
    pub fn GetKillSignal(&self) -> KillSignal {
        self.Signal.load(Ordering::SeqCst)
    }

    /// 将信号常量映射为对应的执行错误；未指定则返回 None。
    fn getKillError(&self, status: KillSignal, desc: &str) -> Option<SharedError> {
        let conn_id = self.ConnID.load(Ordering::SeqCst);
        match status {
            QueryInterrupted => Some(exeerrors::ErrQueryInterrupted.GenWithStackByArgs(&[])),
            MaxExecTimeExceeded => Some(exeerrors::ErrMaxExecTimeExceeded.GenWithStackByArgs(&[])),
            QueryMemoryExceeded => Some(
                exeerrors::ErrMemoryExceedForQuery.GenWithStackByArgs(&[ErrorArg::from(conn_id)]),
            ),
            ServerMemoryExceeded => Some(
                exeerrors::ErrMemoryExceedForInstance
                    .GenWithStackByArgs(&[ErrorArg::from(conn_id)]),
            ),
            RunawayQueryExceeded => Some(
                exeerrors::ErrResourceGroupQueryRunawayInterrupted
                    .FastGenByArgs(&[ErrorArg::from("runaway exceed tidb side")]),
            ),
            KilledByMemArbitrator => {
                Some(exeerrors::ErrQueryExecStopped.GenWithStackByArgs(&[
                    ErrorArg::from(desc.to_owned()),
                    ErrorArg::from(conn_id),
                ]))
            }
            _ => None,
        }
    }

    /// 在 FinishFuncLock 保护下调用结果集完成回调。
    pub fn FinishResultSet(&self) {
        let _guard = self
            .FinishFuncLock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(finish) = self
            .Finish
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_mut()
        {
            finish();
        }
    }

    /// 设置结果集完成回调。
    pub fn SetFinishFunc(&self, callback: FinishFn) {
        let _guard = self
            .FinishFuncLock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *self
            .Finish
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(callback);
    }

    /// 清除结果集完成回调。
    pub fn ClearFinishFunc(&self) {
        let _guard = self
            .FinishFuncLock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *self
            .Finish
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }

    /// Handles failpoint injection, throttled connection checks, and signal-to-error mapping.
    /// 处理 failpoint 注入、节流的连接存活检查，以及信号到错误的映射。
    pub fn HandleSignal(&self) -> Result<(), SharedError> {
        // failpoint：随机 panic/随机信号，仅用于测试注入。
        if let Some(p) = fail::eval("randomPanic", |value| {
            value.and_then(|value| value.parse::<i32>().ok())
        })
        .flatten()
            && rand::random::<f64>() > f64::from(p) / 1000.0
            && self.ConnID.load(Ordering::SeqCst) != 0
        {
            let _event = self.killEvent.lock().expect("killEvent mutex poisoned");
            self.Signal
                .store(rand::random::<u32>() % 5, Ordering::SeqCst);
        }

        // 节流检查连接存活：首次只记录时间，间隔之后才真正回调。
        if let Some(is_connection_alive) = self.IsConnectionAlive.Load() {
            let check_interval = if crate::intest::InTest {
                Duration::from_millis(1)
            } else {
                Duration::from_secs(1)
            };
            let now = Instant::now();
            match self.lastCheckTime.Load() {
                None => self.lastCheckTime.Store(Some(now)),
                Some(last) if now.duration_since(last) > check_interval => {
                    self.lastCheckTime.Store(Some(now));
                    if !is_connection_alive() {
                        self.sendKillSignal(QueryInterrupted);
                    }
                }
                Some(_) => {}
            }
        }

        let mut status = self.Signal.load(Ordering::SeqCst);
        let mut desc = String::new();
        if status == KilledByMemArbitrator {
            let event = self.killEvent.lock().expect("killEvent mutex poisoned");
            status = self.Signal.load(Ordering::SeqCst);
            desc.clone_from(&event.desc);
        }
        if status == ServerMemoryExceeded {
            BgLogger().log(
                LogLevel::Warn,
                "global memory controller, NeedKill signal is received successfully",
                [LogField::U64(
                    "conn".to_owned(),
                    self.ConnID.load(Ordering::SeqCst),
                )],
            );
        }
        self.getKillError(status, &desc).map_or(Ok(()), Err)
    }

    /// 立即检查连接存活；失活则发送 QueryInterrupted。
    pub fn CheckConnectionAlive(&self) {
        if let Some(is_connection_alive) = self.IsConnectionAlive.Load()
            && !is_connection_alive()
        {
            self.sendKillSignal(QueryInterrupted);
        }
    }

    /// 重置信号与事件，供下一条语句复用同一 SQLKiller。
    pub fn Reset(&self) {
        let old_status = {
            let mut event = self.killEvent.lock().expect("killEvent mutex poisoned");
            let old = self.Signal.swap(UnspecifiedKillSignal, Ordering::SeqCst);
            fail::eval("go_merge_34_after_reset_signal_swap", |_| {});
            Self::resetKillEventLocked(&mut event);
            old
        };
        if old_status != UnspecifiedKillSignal {
            BgLogger().log(
                LogLevel::Warn,
                "kill finished",
                [LogField::U64(
                    "conn".to_owned(),
                    self.ConnID.load(Ordering::SeqCst),
                )],
            );
        }
        self.lastCheckTime.Store(None);
    }
}

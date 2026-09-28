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

// 服务端内存上限控制器：周期观测堆占用并 kill 最大内存消费者。
//
// 当实例堆内存超过 `tidb_server_memory_limit` 时，向占用最高的会话发送
// `ServerMemoryExceeded` kill 信号；维护环形操作历史供诊断查询。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::ptr;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};

use crate::{logutil, memory, mysql, sessmgr, sqlkiller, types};

// Process global Observation indicators for memory limit.
/// 观测到的实例堆内存峰值（字节）。
pub static MemoryMaxUsed: memory::atomicutil::Uint64 = memory::atomicutil::Uint64::new(0);
/// 最近一次因内存超限发起 kill 的时间。
pub static SessionKillLast: LazyLock<memory::atomicutil::Time> =
    LazyLock::new(|| memory::atomicutil::Time::new(go_zero_time()));
/// 累计 kill 会话次数。
pub static SessionKillTotal: memory::atomicutil::Int64 = memory::atomicutil::Int64::new(0);
/// 当前是否正处于 kill 流程中。
pub static IsKilling: memory::atomicutil::Bool = memory::atomicutil::Bool::new(false);
/// 全局内存操作历史管理器（环形缓冲，容量 50）。
pub static GlobalMemoryOpsHistoryManager: LazyLock<Mutex<memoryOpsHistoryManager>> =
    LazyLock::new(|| Mutex::new(memoryOpsHistoryManager::default()));

/// Go 零时间（约公元前一年）对应的 SystemTime，用作原子时间初值。
fn go_zero_time() -> SystemTime {
    SystemTime::UNIX_EPOCH
        .checked_sub(Duration::from_secs(62_135_596_800))
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// 服务端内存上限控制器句柄：持有退出通道与会话管理器。
/// Handle is the handler for server memory limit.
pub struct Handle {
    /// 退出信号接收端；收到或断开时 Run 循环结束。
    exitCh: Receiver<()>,
    /// 会话管理器，用于按连接 ID 查询 ProcessInfo。
    sm: RwLock<Option<Arc<dyn sessmgr::Manager>>>,
}

/// 构造内存上限控制器；需再 SetSessionManager 后才能 Run。
/// NewServerMemoryLimitHandle builds a new server memory limit handler.
pub fn NewServerMemoryLimitHandle(exitCh: Receiver<()>) -> Box<Handle> {
    Box::new(Handle {
        exitCh,
        sm: RwLock::new(None),
    })
}

impl Handle {
    /// 注入会话管理器并返回自身，便于链式调用。
    /// SetSessionManager sets the Manager used to fetch active-session info.
    pub fn SetSessionManager(&self, sm: Arc<dyn sessmgr::Manager>) -> &Handle {
        *self.sm.write().expect("session manager lock poisoned") = Some(sm);
        self
    }

    /// 每 100ms 检查一次内存；退出通道触发时返回。
    /// Run checks process memory every 100ms until the exit channel fires.
    pub fn Run(&self) {
        let sm = self
            .sm
            .read()
            .expect("session manager lock poisoned")
            .clone()
            .expect("session manager must be set before Run");
        let mut sessionToBeKilled = sessionToBeKilled::default();
        // 超时则执行一轮仲裁与 kill 判定；收到退出或通道关闭则结束。
        loop {
            match self.exitCh.recv_timeout(Duration::from_millis(100)) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                Err(RecvTimeoutError::Timeout) => {
                    memory::HandleGlobalMemArbitratorRuntime();
                    killSessIfNeeded(
                        &mut sessionToBeKilled,
                        memory::ServerMemoryLimit.Load(),
                        sm.as_ref(),
                    );
                }
            }
        }
    }
}

/// 控制器与单测共用的最小进程信息读取面。
/// Minimal read surface used by the controller and its focused tests.
pub(crate) trait ProcessInfoProvider {
    /// 按连接 ID 取活跃会话信息；不存在则返回 None。
    fn get_process_info(&self, id: u64) -> Option<Arc<sessmgr::ProcessInfo>>;
}

impl<T> ProcessInfoProvider for T
where
    T: sessmgr::Manager + ?Sized,
{
    fn get_process_info(&self, id: u64) -> Option<Arc<sessmgr::ProcessInfo>> {
        self.GetProcessInfo(id)
    }
}

/// 跨 tick 保留的「正在被 kill 的会话」状态。
/// State retained across memory-controller ticks for the SQL currently being killed.
pub(crate) struct sessionToBeKilled {
    /// 是否已发出 kill 且等待生效。
    pub(crate) isKilling: bool,
    /// 该 SQL 开始时间，用于确认仍是同一条语句。
    pub(crate) sqlStartTime: Option<SystemTime>,
    /// 目标会话连接 ID。
    pub(crate) sessionID: u64,
    /// 会话根内存 Tracker 裸指针（外部所有权）。
    pub(crate) sessionTracker: *mut memory::Tracker,
    /// 发起 kill 的时间。
    pub(crate) killStartTime: Option<SystemTime>,
    /// 上次告警日志时间，用于 5 秒节流。
    pub(crate) lastLogTime: Option<SystemTime>,
}

impl Default for sessionToBeKilled {
    fn default() -> Self {
        Self {
            isKilling: false,
            sqlStartTime: None,
            sessionID: 0,
            sessionTracker: ptr::null_mut(),
            killStartTime: None,
            lastLogTime: None,
        }
    }
}

impl sessionToBeKilled {
    /// 清空 kill 相关字段，回到空闲态。
    fn reset(&mut self) {
        self.isKilling = false;
        self.sqlStartTime = None;
        self.sessionID = 0;
        self.sessionTracker = ptr::null_mut();
        self.killStartTime = None;
        self.lastLogTime = None;
    }
}

/// 距给定时刻的经过时间；无效则返回 0。
fn since(instant: Option<SystemTime>) -> Duration {
    instant
        .and_then(|instant| SystemTime::now().duration_since(instant).ok())
        .unwrap_or_default()
}

/// 按 Unicode 标量截断字符串，用于日志中缩短 SQL 文本。
fn truncated(value: &str, length: usize) -> String {
    value.chars().take(length).collect()
}

/// 读取会话当前内存占用字节数。
fn info_memory_usage(info: &sessmgr::ProcessInfo) -> i64 {
    info.MemTracker
        .as_deref()
        .expect("active process must have a memory tracker")
        .BytesConsumed()
}

/// 后台 Warn 级别日志封装。
fn warn(message: impl Into<String>, fields: Vec<logutil::LogField>) {
    logutil::BgLogger().log(logutil::LogLevel::Warn, message, fields);
}

// Rust 无 Go runtime.GC 等价物；保留空函数以对齐调用点顺序。
// Rust releases owned resources deterministically and has no tracing-GC equivalent to runtime.GC.
fn runtime_gc() {}

/// 核心内存上限控制逻辑；包内可见以便单测直接调用。
/// Core memory-limit controller, kept package-visible for the independent focused test.
pub(crate) fn killSessIfNeeded<P>(s: &mut sessionToBeKilled, mut bt: u64, sm: &P)
where
    P: ProcessInfoProvider + ?Sized,
{
    // 已在 kill 中：同一 SQL 仍存活则节流打日志，超时强制结束结果集。
    if s.isKilling {
        if let Some(info) = sm.get_process_info(s.sessionID)
            && Some(info.Time) == s.sqlStartTime
        {
            if since(s.lastLogTime) > Duration::from_secs(5) {
                warn(
                    format!(
                        "global memory controller failed to kill the top-consumer in {}s",
                        since(s.killStartTime).as_secs()
                    ),
                    vec![
                        logutil::LogField::U64("conn".into(), info.ID),
                        logutil::LogField::String("sql digest".into(), info.Digest.clone()),
                        logutil::LogField::String("sql text".into(), truncated(&info.Info, 100)),
                        logutil::LogField::I64("sql memory usage".into(), info_memory_usage(&info)),
                    ],
                );
                s.lastLogTime = Some(SystemTime::now());

                let seconds = since(s.killStartTime).as_secs();
                // kill 超过 60 秒仍未结束，强制 FinishResultSet。
                if seconds >= 60 {
                    warn(
                        format!(
                            "global memory controller failed to kill the top-consumer in {seconds} seconds. Attempting to force close the executors."
                        ),
                        vec![],
                    );
                    // SAFETY: MemUsageTop1Tracker is an externally owned tracker pointer. TiDB
                    // keeps the session tracker alive while ProcessInfo still describes this SQL.
                    unsafe {
                        (*s.sessionTracker)
                            .Killer
                            .as_ref()
                            .expect("session tracker must have a SQL killer")
                            .FinishResultSet();
                    }
                } else {
                    return;
                }
            } else {
                return;
            }
        }

        // Preserve the Go ordering exactly: reset clears sessionTracker before this CAS.
        s.reset();
        IsKilling.Store(false);
        let _ = memory::MemUsageTop1Tracker.compare_exchange(
            s.sessionTracker,
            ptr::null_mut(),
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        runtime_gc();
        warn(
            "global memory controller killed the top1 memory consumer successfully",
            vec![],
        );
    }

    // 限额为 0 表示功能关闭。
    if bt == 0 {
        return;
    }

    // failpoint：测试中把限额压到 1 以强制进入超限分支。
    if let Some(value) = fail::eval("issue42662_2", |value| value).flatten()
        && value.parse::<bool>().unwrap_or(false)
    {
        bt = 1;
    }

    // 更新观测到的堆占用峰值。
    let instanceStats = memory::ReadMemStats();
    if instanceStats.heap_inuse > MemoryMaxUsed.Load() {
        MemoryMaxUsed.Store(instanceStats.heap_inuse);
    }

    // 全局内存仲裁开启时由仲裁器接管，此处直接返回。
    if memory::UsingGlobalMemArbitration() {
        return;
    }

    let limitSessMinSize = memory::ServerMemoryLimitSessMinSize.Load();
    // 堆占用超过限额时，尝试 kill top1 内存消费者。
    if instanceStats.heap_inuse > bt {
        let mut tracker = memory::MemUsageTop1Tracker.load(Ordering::SeqCst);
        if !tracker.is_null() {
            // SAFETY: the global top1 slot stores pointers to live session-root trackers.
            let (sessionID, memUsage) =
                unsafe { ((*tracker).SessionID.Load(), (*tracker).BytesConsumed()) };
            if (memUsage as u64) < limitSessMinSize {
                let _ = memory::MemUsageTop1Tracker.compare_exchange(
                    tracker,
                    ptr::null_mut(),
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                );
                tracker = ptr::null_mut();
            } else if let Some(info) = sm.get_process_info(sessionID) {
                warn(
                    "global memory controller tries to kill the top1 memory consumer",
                    vec![
                        logutil::LogField::U64("conn".into(), info.ID),
                        logutil::LogField::String("sql digest".into(), info.Digest.clone()),
                        logutil::LogField::String("sql text".into(), truncated(&info.Info, 100)),
                        logutil::LogField::U64("tidb_server_memory_limit".into(), bt),
                        logutil::LogField::U64("heap inuse".into(), instanceStats.heap_inuse),
                        logutil::LogField::I64("sql memory usage".into(), info_memory_usage(&info)),
                    ],
                );
                s.sessionID = sessionID;
                s.sqlStartTime = Some(info.Time);
                s.isKilling = true;
                s.sessionTracker = tracker;
                // SAFETY: tracker was loaded from the live global slot and remains externally owned.
                unsafe {
                    (*tracker)
                        .Killer
                        .as_ref()
                        .expect("session tracker must have a SQL killer")
                        .SendKillSignal(sqlkiller::ServerMemoryExceeded);
                }

                let killTime = SystemTime::now();
                SessionKillTotal.Add(1);
                SessionKillLast.Store(killTime);
                IsKilling.Store(true);
                GlobalMemoryOpsHistoryManager
                    .lock()
                    .expect("history lock poisoned")
                    .recordOne(&info, killTime, bt, instanceStats.heap_inuse);
                let now = SystemTime::now();
                s.lastLogTime = Some(now);
                s.killStartTime = Some(now);
            }
        }

        // If no session exceeds the minimum, retain Go's five-second log throttle.
        if tracker.is_null() {
            if s.lastLogTime.is_none() {
                s.lastLogTime = Some(SystemTime::now());
            }
            if since(s.lastLogTime) < Duration::from_secs(5) {
                return;
            }
            warn(
                "global memory controller tries to kill the top1 memory consumer, but no one larger than tidb_server_memory_limit_sess_min_size is found",
                vec![logutil::LogField::U64(
                    "tidb_server_memory_limit_sess_min_size".into(),
                    limitSessMinSize,
                )],
            );
            s.lastLogTime = Some(SystemTime::now());
        }
    }
}

/// 环形历史中的单条内存控制操作记录。
/// One entry in the most recent memory-controller operations.
#[derive(Clone, Default)]
struct memoryOpsHistory {
    /// kill 发生时间。
    killTime: Option<SystemTime>,
    /// 当时的服务器内存限额。
    memoryLimit: u64,
    /// 当时的堆占用。
    memoryCurrent: u64,
    // id,user,host,db,command,time,state,info,digest,mem,disk,txnStart,...
    processInfoDatum: Vec<sessmgr::ProcessListValue>,
}

/// 固定容量（50）环形历史管理器，对应 Go 包内互斥保护结构。
/// Fixed-size circular history matching the Go package's mutex-protected manager.
pub struct memoryOpsHistoryManager {
    /// 环形槽位。
    infos: Vec<memoryOpsHistory>,
    /// 下一次写入下标。
    pub offsets: usize,
}

impl Default for memoryOpsHistoryManager {
    fn default() -> Self {
        Self {
            infos: vec![memoryOpsHistory::default(); 50],
            offsets: 0,
        }
    }
}

/// 将进程列表单元格转为 Datum。
fn process_list_datum(value: sessmgr::ProcessListValue) -> types::Datum {
    match value {
        sessmgr::ProcessListValue::Null => types::Datum::default(),
        sessmgr::ProcessListValue::Unsigned(value) => types::NewUintDatum(value),
        sessmgr::ProcessListValue::Signed(value) => types::NewIntDatum(value),
        sessmgr::ProcessListValue::Float(value) => types::NewFloat64Datum(value),
        sessmgr::ProcessListValue::Text(value) => types::NewStringDatum(value),
    }
}

impl memoryOpsHistoryManager {
    /// 重置 50 槽环形缓冲与写指针。
    pub fn init(&mut self) {
        self.infos = vec![memoryOpsHistory::default(); 50];
        self.offsets = 0;
    }

    /// 记录一次 SessionKill 操作；写满后环绕覆盖最旧项。
    pub fn recordOne(
        &mut self,
        info: &sessmgr::ProcessInfo,
        killTime: SystemTime,
        memoryLimit: u64,
        memoryCurrent: u64,
    ) {
        let op = memoryOpsHistory {
            killTime: Some(killTime),
            memoryLimit,
            memoryCurrent,
            processInfoDatum: info.ToRow(chrono_tz::UTC),
        };

        // Preserve Go's value-copy behavior: SetString mutates the local Datum copy only.
        let mut sqlInfo = process_list_datum(op.processInfoDatum[7].clone());
        sqlInfo.SetString(
            truncated(&sqlInfo.GetString(), 256),
            mysql::DefaultCollationName.to_owned(),
        );

        self.infos[self.offsets] = op;
        self.offsets += 1;
        if self.offsets >= 50 {
            self.offsets = 0;
        }
    }

    /// 按从旧到新顺序导出非空历史行为 Datum 行。
    pub fn GetRows(&self) -> Vec<Vec<types::Datum>> {
        let mut rows = Vec::with_capacity(self.infos.len());
        for i in 0..self.infos.len() {
            let pos = (self.offsets + i) % self.infos.len();
            let info = &self.infos[pos];
            let Some(killTime) = info.killTime else {
                continue;
            };
            let goTime = DateTime::<Utc>::from(killTime).with_timezone(&chrono_tz::UTC);
            let killTime = types::NewTime(types::FromGoTime(goTime), mysql::TypeDatetime, 0);
            rows.push(vec![
                types::NewTimeDatum(killTime),
                types::NewStringDatum("SessionKill".to_owned()),
                types::NewUintDatum(info.memoryLimit),
                types::NewUintDatum(info.memoryCurrent),
                process_list_datum(info.processInfoDatum[0].clone()),
                process_list_datum(info.processInfoDatum[9].clone()),
                process_list_datum(info.processInfoDatum[13].clone()),
                process_list_datum(info.processInfoDatum[2].clone()),
                process_list_datum(info.processInfoDatum[3].clone()),
                process_list_datum(info.processInfoDatum[1].clone()),
                process_list_datum(info.processInfoDatum[8].clone()),
                process_list_datum(info.processInfoDatum[7].clone()),
            ]);
        }
        rows
    }
}

/// 初始化包级全局历史，对齐 Go 包 init。
/// Reinitializes the package-global history, mirroring Go's package init hook.
pub fn init() {
    GlobalMemoryOpsHistoryManager
        .lock()
        .expect("history lock poisoned")
        .init();
}

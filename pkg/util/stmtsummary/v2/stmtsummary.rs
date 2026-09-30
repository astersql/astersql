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

// 语句摘要 v2 核心：时间窗口、LRU 驱逐与持久化（对应 Go `stmtsummary.go`）。
//
// 按 digest（可选用户）聚合 `StmtRecord`；窗口到期轮转落盘；可选异步写驱逐日志。
// 全局函数在 v2 未 Setup 时回退到 v1 `StmtSummaryByDigestMap`。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use chrono_tz::UTC;
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, tick};
use lru::LruCache;
use parking_lot::Mutex;
use task_stmtsummary::{StmtDigestKeyPool, StmtExecInfo};

use crate::{
    MemWindowSnapshot, MemorySummarySource, NewStmtRecord, StmtRecord, marshalEvictedStmtRecord,
    marshalStmtRecord, setGlobalMaxSQLLength,
};

/// 默认启用语句摘要。
pub const defaultEnabled: bool = true;
/// 默认不统计内部 SQL。
pub const defaultEnableInternalQuery: bool = false;
/// 默认每个窗口最多保留的语句条数。
pub const defaultMaxStmtCount: u32 = 3000;
/// 默认采样 SQL 最大长度。
pub const defaultMaxSQLLength: u32 = 32768;
/// 默认窗口刷新间隔（秒），30 分钟。
pub const defaultRefreshInterval: u32 = 30 * 60;
/// 轮转检查周期（秒）。
pub const defaultRotateCheckInterval: u64 = 1;
/// 驱逐日志通道容量。
pub const evictedLogChanCap: usize = 1024;
/// 驱逐日志批量刷盘条数。
pub const evictedLogBatchSize: usize = 64;
/// 驱逐日志定时刷盘间隔。
pub const evictedLogFlushInterval: Duration = Duration::from_millis(100);
/// 驱逐丢弃计数上报间隔。
pub const evictedDropReportInterval: Duration = Duration::from_secs(30);

type Result<T> = std::result::Result<T, String>;
type LockedRecord = Arc<Mutex<StmtRecord>>;

/// 文件存储配置（文件名与滚动参数；本移植主要使用 Filename）。
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub Filename: String,
    pub FileMaxSize: i32,
    pub FileMaxDays: i32,
    pub FileMaxBackups: i32,
}

/// 窗口快照：起止时间、当前记录列表与待持久化的聚合驱逐记录。
#[derive(Clone)]
struct WindowSnapshot {
    begin: SystemTime,
    records: Vec<StmtRecord>,
    evicted_for_persist: StmtRecord,
}

/// 持久化后端：刷窗口、写驱逐日志、fsync。
trait stmtStorage: Send + Sync {
    fn persist(&self, window: WindowSnapshot, end: SystemTime);
    fn logEvicted(&self, records: &[StmtRecord]);
    fn sync(&self) -> io::Result<()>;
    fn persistedEvictedCount(&self) -> usize {
        0
    }
}

/// The package logger owns JSON formatting in the integrated crate. This task-local
/// storage keeps the same durable append and sync contract without hiding I/O errors.
/// 追加写 JSON 行到本地文件的存储实现。
struct fileStmtStorage {
    file: Mutex<File>,
}

impl fileStmtStorage {
    fn new(filename: &str) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(filename)?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }

    /// 写一行完整的语句摘要 JSON；驱逐行额外携带 `evicted: true`。
    fn writeRecord(file: &mut File, record: &StmtRecord, evicted: bool) -> io::Result<()> {
        let encoded = if evicted {
            marshalEvictedStmtRecord(record)
        } else {
            marshalStmtRecord(record)
        }
        .map_err(io::Error::other)?;
        file.write_all(&encoded)?;
        file.write_all(b"\n")
    }
}

impl stmtStorage for fileStmtStorage {
    fn persist(&self, window: WindowSnapshot, end: SystemTime) {
        let begin = unixSeconds(window.begin);
        let end = unixSeconds(end);
        let mut file = self.file.lock();
        for mut record in window.records {
            record.Begin = begin;
            record.End = end;
            let _ = Self::writeRecord(&mut file, &record, false);
        }
        // 未单独入队写日志的驱逐聚合，在窗口结束时一并落盘。
        if window.evicted_for_persist.ExecCount > 0 {
            let mut record = window.evicted_for_persist;
            record.Begin = begin;
            record.End = end;
            let _ = Self::writeRecord(&mut file, &record, true);
        }
    }

    fn logEvicted(&self, records: &[StmtRecord]) {
        let mut file = self.file.lock();
        for record in records {
            let _ = Self::writeRecord(&mut file, record, true);
        }
    }

    fn sync(&self) -> io::Result<()> {
        self.file.lock().sync_all()
    }
}

/// 测试用内存存储：记录窗口快照与驱逐行。
#[derive(Default)]
struct mockStmtStorage {
    windows: Mutex<Vec<WindowSnapshot>>,
    evicted: Mutex<Vec<StmtRecord>>,
}

impl stmtStorage for mockStmtStorage {
    fn persist(&self, window: WindowSnapshot, _end: SystemTime) {
        self.windows.lock().push(window);
    }
    fn logEvicted(&self, records: &[StmtRecord]) {
        self.evicted.lock().extend_from_slice(records);
    }
    fn sync(&self) -> io::Result<()> {
        Ok(())
    }
    fn persistedEvictedCount(&self) -> usize {
        self.evicted.lock().len()
    }
}

/// 当前窗口内被 LRU 挤出的 digests 与聚合指标。
#[derive(Clone)]
pub struct stmtEvicted {
    keys: HashSet<Vec<u8>>,
    pub other: StmtRecord,
    pub otherForPersist: StmtRecord,
}

impl stmtEvicted {
    fn new() -> Self {
        Self {
            keys: HashSet::new(),
            other: newEvictedAggregateRecord(),
            otherForPersist: newEvictedAggregateRecord(),
        }
    }

    /// 合并一条被驱逐记录；若已入队写日志则不再并入 otherForPersist。
    fn add(&mut self, key: &[u8], record: &StmtRecord, queued_for_log: bool) {
        self.keys.insert(key.to_vec());
        self.other.Merge(record);
        if !queued_for_log {
            self.otherForPersist.Merge(record);
        }
    }

    /// 被驱逐的不同 key 数量。
    pub fn count(&self) -> usize {
        self.keys.len()
    }
}

/// 构造用于聚合驱逐统计的空白记录（MinLatency 取极大值以便后续 min）。
pub fn newEvictedAggregateRecord() -> StmtRecord {
    StmtRecord {
        MinLatency: Duration::from_nanos(i64::MAX as u64),
        FirstSeen: SystemTime::now(),
        LastSeen: SystemTime::now(),
        ..Default::default()
    }
}

/// 一个刷新周期内的摘要窗口：LRU 缓存 + 驱逐汇总。
pub struct stmtWindow {
    pub begin: SystemTime,
    lru: LruCache<Vec<u8>, LockedRecord>,
    pub evicted: stmtEvicted,
    pub evictedCount: i64,
}

impl stmtWindow {
    fn new(begin: SystemTime, capacity: u32) -> Self {
        let capacity = NonZeroUsize::new(capacity.max(1) as usize).unwrap();
        Self {
            begin,
            lru: LruCache::new(capacity),
            evicted: stmtEvicted::new(),
            evictedCount: 0,
        }
    }

    fn clear(&mut self) {
        self.lru.clear();
        self.evicted = stmtEvicted::new();
        self.evictedCount = 0;
    }

    /// 克隆当前 LRU 中全部记录与待持久化驱逐聚合。
    fn snapshot(&self) -> WindowSnapshot {
        WindowSnapshot {
            begin: self.begin,
            records: self
                .lru
                .iter()
                .map(|(_, record)| record.lock().clone())
                .collect(),
            evicted_for_persist: self.evicted.otherForPersist.clone(),
        }
    }
}

/// 共享内部状态：开关选项、窗口、存储与驱逐通道。
struct StmtSummaryInner {
    optEnabled: AtomicBool,
    optEnableInternalQuery: AtomicBool,
    optMaxStmtCount: AtomicU32,
    optMaxSQLLength: AtomicU32,
    optRefreshInterval: AtomicU32,
    optPersistEvicted: AtomicBool,
    optGroupByUser: AtomicBool,
    window: Mutex<stmtWindow>,
    storage: Arc<dyn stmtStorage>,
    evictedTx: Sender<StmtRecord>,
    evictedRx: Receiver<StmtRecord>,
    evictedDropped: AtomicU64,
    closed: AtomicBool,
    stop: AtomicBool,
}

/// 语句摘要实例：对外 API + 后台驱逐日志 / 轮转线程。
pub struct StmtSummary {
    inner: Arc<StmtSummaryInner>,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

impl StmtSummary {
    /// 创建实例并启动驱逐日志循环；`rotate` 为真时再启窗口轮转线程。
    fn create(max_count: u32, storage: Arc<dyn stmtStorage>, rotate: bool) -> Arc<Self> {
        let (evictedTx, evictedRx) = bounded(evictedLogChanCap);
        let inner = Arc::new(StmtSummaryInner {
            optEnabled: AtomicBool::new(defaultEnabled),
            optEnableInternalQuery: AtomicBool::new(defaultEnableInternalQuery),
            optMaxStmtCount: AtomicU32::new(defaultMaxStmtCount),
            optMaxSQLLength: AtomicU32::new(defaultMaxSQLLength),
            optRefreshInterval: AtomicU32::new(if rotate {
                defaultRefreshInterval
            } else {
                365 * 24 * 60 * 60
            }),
            optPersistEvicted: AtomicBool::new(false),
            optGroupByUser: AtomicBool::new(false),
            window: Mutex::new(stmtWindow::new(SystemTime::now(), max_count)),
            storage,
            evictedTx,
            evictedRx,
            evictedDropped: AtomicU64::new(0),
            closed: AtomicBool::new(false),
            stop: AtomicBool::new(false),
        });
        let summary = Arc::new(Self {
            inner: inner.clone(),
            workers: Mutex::new(Vec::new()),
        });
        summary.workers.lock().push(thread::spawn({
            let inner = inner.clone();
            move || evictedLogLoop(inner)
        }));
        if rotate {
            summary
                .workers
                .lock()
                .push(thread::spawn(move || rotateLoop(inner)));
        }
        summary
    }

    /// 是否启用摘要采集。
    pub fn Enabled(&self) -> bool {
        self.inner.optEnabled.load(Ordering::Acquire)
    }

    /// 设置启用状态；关闭时清空窗口。
    pub fn SetEnabled(&self, value: bool) -> Result<()> {
        self.inner.optEnabled.store(value, Ordering::Release);
        if !value {
            self.Clear();
        }
        Ok(())
    }

    /// 是否统计内部查询。
    pub fn EnableInternalQuery(&self) -> bool {
        self.inner.optEnableInternalQuery.load(Ordering::Acquire)
    }

    /// 设置内部查询开关；关闭时移除已有内部记录。
    pub fn SetEnableInternalQuery(&self, value: bool) -> Result<()> {
        self.inner
            .optEnableInternalQuery
            .store(value, Ordering::Release);
        if !value {
            self.ClearInternal();
        }
        Ok(())
    }

    /// 当前窗口最大语句条数。
    pub fn MaxStmtCount(&self) -> u32 {
        self.inner.optMaxStmtCount.load(Ordering::Acquire)
    }

    /// 调整容量；若缩小则弹出 LRU 尾部并计入驱逐。
    pub fn SetMaxStmtCount(&self, value: u32) -> Result<()> {
        let value = value.max(1);
        self.inner.optMaxStmtCount.store(value, Ordering::Release);
        let mut window = self.inner.window.lock();
        while window.lru.len() > value as usize {
            if let Some((key, record)) = window.lru.pop_lru() {
                onEvict(&self.inner, &mut window, key, record);
            }
        }
        window
            .lru
            .resize(NonZeroUsize::new(value as usize).unwrap());
        Ok(())
    }

    pub fn MaxSQLLength(&self) -> u32 {
        self.inner.optMaxSQLLength.load(Ordering::Acquire)
    }

    pub fn SetMaxSQLLength(&self, value: u32) -> Result<()> {
        self.inner.optMaxSQLLength.store(value, Ordering::Release);
        setGlobalMaxSQLLength(value);
        Ok(())
    }

    pub fn RefreshInterval(&self) -> u32 {
        self.inner.optRefreshInterval.load(Ordering::Acquire)
    }

    pub fn SetRefreshInterval(&self, value: u32) -> Result<()> {
        self.inner
            .optRefreshInterval
            .store(value.max(1), Ordering::Release);
        Ok(())
    }

    pub fn PersistEvicted(&self) -> bool {
        self.inner.optPersistEvicted.load(Ordering::Acquire)
    }
    pub fn SetPersistEvicted(&self, value: bool) -> Result<()> {
        self.inner.optPersistEvicted.store(value, Ordering::Release);
        Ok(())
    }
    pub fn GroupByUser(&self) -> bool {
        self.inner.optGroupByUser.load(Ordering::Acquire)
    }

    /// 切换按用户分组；策略变化时清空窗口以免键语义混乱。
    pub fn SetGroupByUser(&self, value: bool) -> Result<()> {
        let mut window = self.inner.window.lock();
        if self.inner.optGroupByUser.load(Ordering::Acquire) != value {
            self.inner.optGroupByUser.store(value, Ordering::Release);
            window.clear();
        }
        Ok(())
    }

    /// 将一次执行记入当前窗口；新键可能挤出 LRU 尾部触发 onEvict。
    pub fn Add(&self, info: &StmtExecInfo) {
        if self.inner.closed.load(Ordering::Acquire) {
            return;
        }
        let (record, existing) = {
            let mut window = self.inner.window.lock();
            if self.inner.closed.load(Ordering::Acquire) {
                return;
            }
            let user = if self.inner.optGroupByUser.load(Ordering::Acquire) {
                info.User.as_str()
            } else {
                ""
            };
            let key = digestKey(info, user);
            if let Some(record) = window.lru.get(&key).cloned() {
                (record, true)
            } else {
                let record = Arc::new(Mutex::new(*NewStmtRecord(info)));
                if let Some((evicted_key, evicted_record)) = window.lru.push(key, record.clone()) {
                    onEvict(&self.inner, &mut window, evicted_key, evicted_record);
                }
                (record, false)
            }
        };
        record.lock().Add(info);
        let _ = existing;
    }

    /// 返回当前窗口驱逐汇总行（begin/end/count）；无驱逐时为 None。
    pub fn Evicted(&self) -> Option<Vec<task_stmtsummary::types::Datum>> {
        let window = self.inner.window.lock();
        let count = window.evicted.count() as i64;
        if count == 0 {
            return None;
        }
        let begin = task_stmtsummary::types::NewTime(
            task_stmtsummary::types::FromGoTime(
                DateTime::<Utc>::from(window.begin).with_timezone(&UTC),
            ),
            task_stmtsummary::mysql::TypeTimestamp,
            0,
        );
        let end = task_stmtsummary::types::NewTime(
            task_stmtsummary::types::FromGoTime(
                DateTime::<Utc>::from(SystemTime::now()).with_timezone(&UTC),
            ),
            task_stmtsummary::mysql::TypeTimestamp,
            0,
        );
        Some(vec![
            task_stmtsummary::types::NewTimeDatum(begin),
            task_stmtsummary::types::NewTimeDatum(end),
            task_stmtsummary::types::NewIntDatum(count),
        ])
    }

    pub fn Clear(&self) {
        self.inner.window.lock().clear();
    }

    /// 仅移除标记为内部查询的记录。
    pub fn ClearInternal(&self) {
        let mut window = self.inner.window.lock();
        let keys: Vec<Vec<u8>> = window
            .lru
            .iter()
            .filter_map(|(key, record)| record.lock().IsInternal.then(|| key.clone()))
            .collect();
        for key in keys {
            window.lru.pop(&key);
        }
    }

    /// 关闭：停后台线程、刷当前窗口并 fsync。
    pub fn Close(&self) {
        if self.inner.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.inner.stop.store(true, Ordering::Release);
        for worker in self.workers.lock().drain(..) {
            let _ = worker.join();
        }
        let now = SystemTime::now();
        let snapshot = {
            let mut window = self.inner.window.lock();
            let replacement = stmtWindow::new(now, self.MaxStmtCount());
            std::mem::replace(&mut *window, replacement).snapshot()
        };
        if !snapshot.records.is_empty() {
            self.inner.storage.persist(snapshot, now);
        }
        let _ = self.inner.storage.sync();
    }

    pub fn Len(&self) -> usize {
        self.inner.window.lock().lru.len()
    }
    #[cfg(test)]
    pub(crate) fn recordForTest(&self, digest: &str) -> Option<Arc<Mutex<StmtRecord>>> {
        self.inner
            .window
            .lock()
            .lru
            .iter()
            .find(|(_, record)| record.lock().Digest == digest)
            .map(|(_, record)| Arc::clone(record))
    }
    #[cfg(test)]
    pub(crate) fn rotateForTest(&self) {
        let begin = self.inner.window.lock().begin;
        rotateWindow(&self.inner, begin + Duration::from_secs(2));
    }
    pub fn EvictedCount(&self) -> i64 {
        self.inner.window.lock().evictedCount
    }
    pub fn IsClosed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }
    pub fn PersistedEvictedCount(&self) -> usize {
        self.inner.storage.persistedEvictedCount()
    }
    pub fn DroppedEvictedCount(&self) -> u64 {
        self.inner.evictedDropped.load(Ordering::Acquire)
    }
}

impl Drop for StmtSummary {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::Release);
        for worker in self.workers.get_mut().drain(..) {
            let _ = worker.join();
        }
    }
}

impl MemorySummarySource for StmtSummary {
    fn currentWindowSnapshot(&self) -> Option<MemWindowSnapshot> {
        if self.inner.closed.load(Ordering::Acquire) {
            return None;
        }
        let window = self.inner.window.lock();
        let evicted = (window.evicted.other.ExecCount > 0).then(|| window.evicted.other.clone());
        Some(MemWindowSnapshot {
            begin: unixSeconds(window.begin),
            records: window
                .lru
                .iter()
                .map(|(_, record)| record.lock().clone())
                .collect(),
            evicted,
        })
    }
}

/// 由 schema/digest/prevSQL/plan/资源组/用户 生成 LRU 键字节。
fn digestKey(info: &StmtExecInfo, user: &str) -> Vec<u8> {
    let mut key = StmtDigestKeyPool.Get();
    key.Init(
        &info.SchemaName,
        &info.Digest,
        &info.PrevSQLDigest,
        &info.PlanDigest,
        &info.ResourceGroupName,
        user,
    );
    let bytes = key.Hash().to_vec();
    StmtDigestKeyPool.Put(key);
    bytes
}

/// LRU 弹出时：累计驱逐计数，可选入队写日志，并更新聚合。
fn onEvict(
    inner: &Arc<StmtSummaryInner>,
    window: &mut stmtWindow,
    key: Vec<u8>,
    record: LockedRecord,
) {
    window.evictedCount += 1;
    let mut snapshot = record.lock().clone();
    let queued = if inner.optPersistEvicted.load(Ordering::Acquire) {
        snapshot.Begin = unixSeconds(window.begin);
        snapshot.End = unixSeconds(SystemTime::now());
        match inner.evictedTx.try_send(snapshot.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                inner.evictedDropped.fetch_add(1, Ordering::Relaxed);
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    } else {
        false
    };
    window.evicted.add(&key, &snapshot, queued);
}

/// 后台批量消费驱逐通道并写入存储。
fn evictedLogLoop(inner: Arc<StmtSummaryInner>) {
    let flush_tick = tick(evictedLogFlushInterval);
    let report_tick = tick(evictedDropReportInterval);
    let mut batch = Vec::with_capacity(evictedLogBatchSize);
    let mut last_report = 0;
    loop {
        if inner.stop.load(Ordering::Acquire) {
            while let Ok(record) = inner.evictedRx.try_recv() {
                batch.push(record);
            }
            flushBatch(&inner, &mut batch);
            return;
        }
        crossbeam_channel::select! {
            recv(inner.evictedRx) -> message => match message {
                Ok(record) => {
                    batch.push(record);
                    while batch.len() < evictedLogBatchSize {
                        match inner.evictedRx.try_recv() { Ok(record) => batch.push(record), Err(_) => break }
                    }
                    if batch.len() >= evictedLogBatchSize { flushBatch(&inner, &mut batch); }
                }
                Err(_) => { flushBatch(&inner, &mut batch); return; }
            },
            recv(flush_tick) -> _ => flushBatch(&inner, &mut batch),
            recv(report_tick) -> _ => {
                let current = inner.evictedDropped.load(Ordering::Acquire);
                if current > last_report { last_report = current; }
            },
            default(Duration::from_millis(10)) => {}
        }
    }
}

fn flushBatch(inner: &StmtSummaryInner, batch: &mut Vec<StmtRecord>) {
    if batch.is_empty() {
        return;
    }
    inner.storage.logEvicted(batch);
    batch.clear();
}

/// 到期后用新窗口替换旧窗口，并将旧窗口快照持久化。
fn rotateLoop(inner: Arc<StmtSummaryInner>) {
    let ticker = tick(Duration::from_secs(defaultRotateCheckInterval));
    while !inner.stop.load(Ordering::Acquire) {
        if ticker.recv_timeout(Duration::from_millis(100)).is_err() {
            continue;
        }
        rotateWindow(&inner, SystemTime::now());
    }
}

fn rotateWindow(inner: &StmtSummaryInner, now: SystemTime) {
    let snapshot = {
        let mut window = inner.window.lock();
        let elapsed = now.duration_since(window.begin).unwrap_or_default();
        if elapsed <= Duration::from_secs(inner.optRefreshInterval.load(Ordering::Acquire) as u64) {
            return;
        }
        let replacement = stmtWindow::new(now, inner.optMaxStmtCount.load(Ordering::Acquire));
        std::mem::replace(&mut *window, replacement).snapshot()
    };
    if !snapshot.records.is_empty() {
        inner.storage.persist(snapshot, now);
    }
}

fn unixSeconds(time: SystemTime) -> i64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

/// 克隆记录供日志路径使用（Box 包装与 Go 侧一致）。
pub fn cloneRecordForLog(record: &StmtRecord) -> Box<StmtRecord> {
    Box::new(record.clone())
}

/// 按配置创建启用轮转的生产摘要实例。
pub fn NewStmtSummary(config: &Config) -> Result<Arc<StmtSummary>> {
    if config.Filename.is_empty() {
        return Err("stmtsummary: empty filename".to_owned());
    }
    let storage = fileStmtStorage::new(&config.Filename).map_err(|error| error.to_string())?;
    Ok(StmtSummary::create(
        defaultMaxStmtCount,
        Arc::new(storage),
        true,
    ))
}

/// 测试用：内存存储且不启动轮转（刷新间隔设为一年）。
pub fn NewStmtSummary4Test(maxStmtCount: u32) -> Arc<StmtSummary> {
    StmtSummary::create(
        maxStmtCount.max(1),
        Arc::new(mockStmtStorage::default()),
        false,
    )
}

static GLOBAL_STMT_SUMMARY: OnceLock<RwLock<Option<Arc<StmtSummary>>>> = OnceLock::new();
static GLOBAL_PERSISTENT_ENABLED: AtomicBool = AtomicBool::new(false);
fn global() -> &'static RwLock<Option<Arc<StmtSummary>>> {
    GLOBAL_STMT_SUMMARY.get_or_init(|| RwLock::new(None))
}

fn activeGlobal() -> Option<Arc<StmtSummary>> {
    if GLOBAL_PERSISTENT_ENABLED.load(Ordering::Acquire) {
        global()
            .read()
            .expect("global statement summary lock poisoned")
            .clone()
    } else {
        None
    }
}

#[cfg(test)]
pub(crate) fn installedForTest() -> Option<Arc<StmtSummary>> {
    global().read().unwrap().clone()
}

/// 安装全局 v2 摘要实例。
pub fn Setup(config: &Config) -> Result<()> {
    let summary = match NewStmtSummary(config) {
        Ok(summary) => summary,
        Err(error) => {
            GLOBAL_PERSISTENT_ENABLED.store(false, Ordering::Release);
            return Err(format!(
                "stmtsummary v2 persistent mode disabled; falling back to v1 in-memory aggregation: {error}"
            ));
        }
    };
    let previous = global()
        .write()
        .expect("global statement summary lock poisoned")
        .replace(summary);
    GLOBAL_PERSISTENT_ENABLED.store(true, Ordering::Release);
    if let Some(previous) = previous {
        previous.Close();
    }
    Ok(())
}

/// 关闭并卸下全局实例。
pub fn Close() {
    GLOBAL_PERSISTENT_ENABLED.store(false, Ordering::Release);
    if let Some(summary) = global()
        .write()
        .expect("global statement summary lock poisoned")
        .take()
    {
        summary.Close();
    }
}

/// 全局 Add：优先 v2，未 Setup 时回退 v1。
pub fn Add(info: &StmtExecInfo) {
    if let Some(summary) = activeGlobal() {
        summary.Add(info);
    } else {
        task_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .expect("v1 summary lock poisoned")
            .AddStatement(info);
    }
}

pub fn Enabled() -> bool {
    activeGlobal().as_ref().map_or_else(
        || {
            task_stmtsummary::StmtSummaryByDigestMap
                .lock()
                .expect("v1 summary lock poisoned")
                .Enabled()
        },
        |summary| summary.Enabled(),
    )
}

pub fn EnabledInternal() -> bool {
    activeGlobal().as_ref().map_or_else(
        || {
            task_stmtsummary::StmtSummaryByDigestMap
                .lock()
                .expect("v1 summary lock poisoned")
                .EnabledInternal()
        },
        |summary| summary.EnableInternalQuery(),
    )
}

pub fn SetEnabled(value: bool) -> Result<()> {
    if let Some(summary) = activeGlobal() {
        summary.SetEnabled(value)
    } else {
        task_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .expect("v1 summary lock poisoned")
            .SetEnabled(value)
    }
}

pub fn SetEnableInternalQuery(value: bool) -> Result<()> {
    if let Some(summary) = activeGlobal() {
        summary.SetEnableInternalQuery(value)
    } else {
        task_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .expect("v1 summary lock poisoned")
            .SetEnabledInternalQuery(value)
    }
}

pub fn SetRefreshInterval(value: i64) -> Result<()> {
    if let Some(summary) = activeGlobal() {
        summary.SetRefreshInterval(value.max(1) as u32)
    } else {
        task_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .expect("v1 summary lock poisoned")
            .SetRefreshInterval(value)
    }
}

/// v2 无 history size 概念；已 Setup 时为 no-op，否则转发给 v1。
pub fn SetHistorySize(value: i32) -> Result<()> {
    if activeGlobal().is_some() {
        Ok(())
    } else {
        task_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .expect("v1 summary lock poisoned")
            .SetHistorySize(value)
    }
}

pub fn SetMaxStmtCount(value: i32) -> Result<()> {
    if let Some(summary) = activeGlobal() {
        summary.SetMaxStmtCount(value.max(1) as u32)
    } else {
        task_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .expect("v1 summary lock poisoned")
            .SetMaxStmtCount(value.max(1) as u32)
    }
}

pub fn SetMaxSQLLength(value: i32) -> Result<()> {
    if let Some(summary) = global()
        .read()
        .expect("global statement summary lock poisoned")
        .clone()
    {
        summary.SetMaxSQLLength(value.max(0) as u32)
    } else {
        task_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .expect("v1 summary lock poisoned")
            .SetMaxSQLLength(value)
    }
}

pub fn SetPersistEvicted(value: bool) -> Result<()> {
    if let Some(summary) = global()
        .read()
        .expect("global statement summary lock poisoned")
        .clone()
    {
        summary.SetPersistEvicted(value)
    } else {
        Ok(())
    }
}

/// 同时更新 v1 与（若存在）v2 的按用户分组选项。
pub fn SetGroupByUser(value: bool) -> Result<()> {
    task_stmtsummary::StmtSummaryByDigestMap
        .lock()
        .expect("v1 summary lock poisoned")
        .SetGroupByUser(value)?;
    if let Some(summary) = global()
        .read()
        .expect("global statement summary lock poisoned")
        .clone()
    {
        summary.SetGroupByUser(value)?;
    }
    Ok(())
}

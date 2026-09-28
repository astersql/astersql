// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// TopSQL CPU 采集：从进程级 pprof 剖析数据聚合 SQL/计划的 CPU 时间。
//
// 后台循环按开关注册/注销 ProfileConsumer，解析 sql_digest / plan_digest /
// sql_global_uid 标签，回调 Collector 与 ProcessCPUTimeUpdater。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals
)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use cpuprofile::{ProfileConsumer, ProfileData};
use pprof::protos::{Message, Profile, Sample};

// pprof 样本标签键，与 Go TopSQL 埋点一致。
const labelSQLDigest: &str = "sql_digest";
const labelPlanDigest: &str = "plan_digest";
const labelSQLUID: &str = "sql_global_uid";

// ProcessCPUTimeUpdater Introduce this interface due to the dependency cycle.
/// 按连接与 SQL ID 回写进程 CPU 时间（打破依赖环引入的接口）。
pub trait ProcessCPUTimeUpdater: Send + Sync {
    fn UpdateProcessCPUTime(&self, connID: u64, sqlID: u64, cpuTime: Duration);
}

// Collector uses to collect SQL execution cpu time.
/// 消费按 SQL/计划聚合后的 CPU 时间记录。
pub trait Collector: Send + Sync {
    fn Collect(&self, stats: Vec<SQLCPUTimeRecord>);
}

/// A single record of how much CPU time a SQL plan consumes in one interval.
/// 单个采样间隔内某 SQL 计划消耗的 CPU 时间记录；无计划时 PlanDigest 可为空。
/// PlanDigest can be empty for statements without plans and optimizer samples.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SQLCPUTimeRecord {
    pub SQLDigest: Vec<u8>,
    pub PlanDigest: Vec<u8>,
    pub CPUTimeMs: u32,
}

/// Consumes completed profiles from the process-wide CPU profiler.
/// 从进程级 CPU profiler 消费完成的 profile；Start/Stop 非线程安全（与 Go 一致）。
/// Like the Go type, Start and Stop are intentionally not thread-safe.
pub struct SQLCPUCollector {
    collector: Arc<dyn Collector>,
    updater: Option<Arc<dyn ProcessCPUTimeUpdater>>,
    cancel: Option<crossbeam_channel::Sender<()>>,
    worker: Option<JoinHandle<()>>,
    started: bool,
    registered: Arc<AtomicBool>,
    collect_interval: Duration,
}

/// 构造默认 1s 采集间隔的 SQLCPUCollector。
pub fn NewSQLCPUCollector<C>(collector: Arc<C>) -> SQLCPUCollector
where
    C: Collector + 'static,
{
    SQLCPUCollector {
        collector,
        updater: None,
        cancel: None,
        worker: None,
        started: false,
        registered: Arc::new(AtomicBool::new(false)),
        collect_interval: Duration::from_secs(1),
    }
}

impl SQLCPUCollector {
    /// 设置进程 CPU 时间更新回调。
    pub fn SetProcessCPUUpdater<U>(&mut self, updater: Arc<U>)
    where
        U: ProcessCPUTimeUpdater + 'static,
    {
        self.updater = Some(updater);
    }

    /// Overrides the Go package's defCollectTickerInterval for focused tests.
    /// 覆盖默认采集间隔，便于聚焦测试。
    pub fn set_collect_interval(&mut self, interval: Duration) {
        assert!(!interval.is_zero(), "collect interval must be positive");
        self.collect_interval = interval;
    }

    /// 是否已启动采集循环。
    pub fn is_started(&self) -> bool {
        self.started
    }

    // Start registers a consumer through the background loop. Repeated starts
    // are ignored, matching the Go started guard.
    /// 启动后台采集；重复 Start 被忽略（对齐 Go started 守卫）。
    pub fn Start(&mut self) {
        if self.started {
            return;
        }
        self.started = true;

        let (cancel, cancelled) = crossbeam_channel::bounded(1);
        self.cancel = Some(cancel);
        let collector = self.collector.clone();
        let updater = self.updater.clone();
        let registered = self.registered.clone();
        let interval = self.collect_interval;
        self.worker = Some(std::thread::spawn(move || {
            collectSQLCPULoop(collector, updater, cancelled, registered, interval)
        }));
        log::info!("sql cpu collector started");
    }

    // Stop cancels the loop, waits for its deferred unregister, and is
    // idempotent just like the Go implementation.
    /// 停止采集并等待注销；幂等。
    pub fn Stop(&mut self) {
        if !self.started {
            return;
        }
        self.started = false;
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.try_send(());
        }
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                log::error!("sql cpu collector worker panicked");
            }
        }
        log::info!("sql cpu collector stopped");
    }
}

impl Drop for SQLCPUCollector {
    fn drop(&mut self) {
        self.Stop();
    }
}

// 主循环：按 TopSQL 开关注册消费者，接收 profile 并分发。
/// SQL CPU 采集后台循环。
fn collectSQLCPULoop(
    collector: Arc<dyn Collector>,
    updater: Option<Arc<dyn ProcessCPUTimeUpdater>>,
    cancelled: crossbeam_channel::Receiver<()>,
    registered: Arc<AtomicBool>,
    interval: Duration,
) {
    let (profileConsumer, profiles) = crossbeam_channel::bounded(1);
    let ticker = crossbeam_channel::tick(interval);

    // Go defers Recover, WaitGroup.Done, unregister and ticker.Stop. Catching
    // the body here ensures unregister still runs after malformed input panics.
    let result = catch_unwind(AssertUnwindSafe(|| {
        loop {
            if topsql_state::TopSQLEnabled() {
                doRegister(&profileConsumer, &registered);
            } else {
                doUnregister(&profileConsumer, &registered);
            }

            crossbeam_channel::select! {
                recv(cancelled) -> _ => break,
                recv(ticker) -> _ => {},
                recv(profiles) -> data => match data {
                    Ok(data) => handleProfileData(&data, &collector, updater.as_ref()),
                    Err(_) => break,
                },
            }
        }
    }));

    doUnregister(&profileConsumer, &registered);
    if result.is_err() {
        log::error!("top-sql startAnalyzeProfileWorker panicked");
    }
}

/// 幂等注册 ProfileConsumer。
fn doRegister(profileConsumer: &ProfileConsumer, registered: &AtomicBool) {
    if registered.swap(true, Ordering::SeqCst) {
        return;
    }
    cpuprofile::Register(Some(profileConsumer.clone()));
}

/// 幂等注销 ProfileConsumer。
fn doUnregister(profileConsumer: &ProfileConsumer, registered: &AtomicBool) {
    if !registered.swap(false, Ordering::SeqCst) {
        return;
    }
    cpuprofile::Unregister(Some(profileConsumer.clone()));
}

// 解码 pprof、按 SQL 标签聚合，并可选更新进程级 CPU。
/// 处理一次 ProfileData。
fn handleProfileData<C, U>(data: &ProfileData, collector: &Arc<C>, updater: Option<&Arc<U>>)
where
    C: Collector + ?Sized,
    U: ProcessCPUTimeUpdater + ?Sized,
{
    if data.Error.is_some() {
        return;
    }
    let profile = match Profile::decode(data.Data.as_slice()) {
        Ok(profile) => profile,
        Err(error) => {
            log::error!("parse profile error: {error}");
            return;
        }
    };

    collector.Collect(parseCPUProfileBySQLLabels(&profile));
    let process_records = parseCPUProfileForProcess(&profile);
    if process_records.is_empty() {
        return;
    }
    let updater = updater.expect("process CPU updater is required for sql_global_uid samples");
    for (connID, sqlID, total) in process_records {
        updater.UpdateProcessCPUTime(connID, sqlID, durationFromNanos(total));
    }
}

/// Aggregate the last profile value by SQL and plan labels.
/// 按 sql_digest / plan_digest 聚合样本中最后一个 value 类型。
fn parseCPUProfileBySQLLabels(profile: &Profile) -> Vec<SQLCPUTimeRecord> {
    let mut sqlMap: HashMap<String, sqlStats> = HashMap::new();
    // Match Go's `len(sampleType)-1`: an empty profile is valid while an
    // actually accessed missing sample type still fails at the value lookup.
    let idx = profile.sample_type.len().wrapping_sub(1);
    for sample in &profile.sample {
        let digests = sampleLabelValues(profile, sample, labelSQLDigest);
        if digests.is_empty() {
            continue;
        }
        for digest in digests {
            let stmt = sqlMap.entry(digest).or_insert_with(|| sqlStats {
                plans: HashMap::new(),
                total: 0,
            });
            stmt.total += sample.value[idx];
            for plan in sampleLabelValues(profile, sample, labelPlanDigest) {
                *stmt.plans.entry(plan).or_insert(0) += sample.value[idx];
            }
        }
    }
    createSQLStats(sqlMap)
}

/// 将聚合结果调优并解码为 SQLCPUTimeRecord 列表。
fn createSQLStats(sqlMap: HashMap<String, sqlStats>) -> Vec<SQLCPUTimeRecord> {
    let mut stats = Vec::with_capacity(sqlMap.len());
    for (hexSQLDigest, mut stmt) in sqlMap {
        stmt.tune();
        let sqlDigest = match hex::decode(&hexSQLDigest) {
            Ok(digest) => digest,
            Err(error) => {
                log::error!("decode sql digest failed; sqlDigest={hexSQLDigest}; error={error}");
                continue;
            }
        };
        for (hexPlanDigest, value) in stmt.plans {
            let planDigest = match hex::decode(&hexPlanDigest) {
                Ok(digest) => digest,
                Err(error) => {
                    log::error!(
                        "decode plan digest failed; planDigest={hexPlanDigest}; error={error}"
                    );
                    continue;
                }
            };
            stats.push(SQLCPUTimeRecord {
                SQLDigest: sqlDigest.clone(),
                PlanDigest: planDigest,
                CPUTimeMs: (value / 1_000_000) as u32,
            });
        }
    }
    stats
}

/// 单条 SQL 的计划耗时映射与总计。
struct sqlStats {
    plans: HashMap<String, i64>,
    total: i64,
}

impl sqlStats {
    /// Adds optimizer time under the empty plan digest, preserving every Go
    /// 将优化器时间归入空 plan digest，保留 Go 各分支语义。
    /// branch: no plan, one plan, multiple plans, and non-positive remainder.
    fn tune(&mut self) {
        if self.plans.is_empty() {
            self.plans.insert(String::new(), self.total);
            return;
        }
        if self.plans.len() == 1 {
            if let Some(plan) = self.plans.keys().next().cloned() {
                self.plans.insert(plan, self.total);
            }
            return;
        }
        let optimize = self.total - self.plans.values().sum::<i64>();
        if optimize <= 0 {
            return;
        }
        *self.plans.entry(String::new()).or_insert(0) += optimize;
    }
}

/// 连接上最新 SQL ID 对应的进程 CPU 累计。
struct processCPUTimeRecord {
    sqlID: u64,
    total: i64,
}

/// Aggregate process CPU time while retaining only the newest SQL ID for each
/// 按连接保留最新 sqlID 并聚合进程 CPU；样本逆序遍历（与 Go 一致）。
/// connection. Samples are traversed in reverse as in the Go implementation.
fn parseCPUProfileForProcess(profile: &Profile) -> Vec<(u64, u64, i64)> {
    let mut sqlMap: HashMap<u64, processCPUTimeRecord> = HashMap::new();
    // Keep empty profiles harmless, as in Go, without hiding malformed
    // labelled samples that have no corresponding sample type/value.
    let idx = profile.sample_type.len().wrapping_sub(1);
    for sample in profile.sample.iter().rev() {
        for sqlUID in sampleLabelValues(profile, sample, labelSQLUID) {
            let keys: Vec<&str> = sqlUID.split('_').collect();
            let connID = keys[0].parse::<u64>().unwrap_or(0);
            let sqlID = keys[1].parse::<u64>().unwrap_or(0);
            if let Some(timeRecord) = sqlMap.get_mut(&connID) {
                if sqlID < timeRecord.sqlID {
                    continue;
                } else if sqlID > timeRecord.sqlID {
                    timeRecord.sqlID = sqlID;
                    timeRecord.total = sample.value[idx];
                } else {
                    timeRecord.total += sample.value[idx];
                }
            } else {
                sqlMap.insert(
                    connID,
                    processCPUTimeRecord {
                        sqlID,
                        total: sample.value[idx],
                    },
                );
            }
        }
    }
    sqlMap
        .into_iter()
        .map(|(connID, record)| (connID, record.sqlID, record.total))
        .collect()
}

/// 取出样本中指定标签键的全部字符串值。
fn sampleLabelValues(profile: &Profile, sample: &Sample, wanted: &str) -> Vec<String> {
    sample
        .label
        .iter()
        .filter_map(|label| {
            let key = profile.string_table.get(usize::try_from(label.key).ok()?)?;
            if key != wanted {
                return None;
            }
            profile
                .string_table
                .get(usize::try_from(label.str).ok()?)
                .cloned()
        })
        .collect()
}

/// 纳秒转 Duration（负值按 0）。
fn durationFromNanos(nanos: i64) -> Duration {
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(0))
}

/// Rust-native carrier for the pprof labels placed in a Go context. pprof-rs
/// 承载 pprof 标签的上下文；在 CtxWithProcessInfo 边界写入线程本地。
/// does not expose Go's goroutine context type, so callers pass this value at
/// the same boundary and CtxWithProcessInfo installs its labels thread-locally.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProfileContext {
    labels: HashMap<String, String>,
}

impl ProfileContext {
    /// 读取单个标签值。
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.get(key).map(String::as_str)
    }

    /// 返回全部标签映射。
    pub fn labels(&self) -> &HashMap<String, String> {
        &self.labels
    }
}

thread_local! {
    static CURRENT_THREAD_PROFILE_LABELS: RefCell<HashMap<String, String>> =
        RefCell::new(HashMap::new());
}

/// 读取当前线程本地的 profile 标签快照。
pub fn current_thread_profile_labels() -> HashMap<String, String> {
    CURRENT_THREAD_PROFILE_LABELS.with(|labels| labels.borrow().clone())
}

/// 写入 sql_digest 标签。
pub fn CtxWithSQLDigest(mut ctx: ProfileContext, sqlDigest: String) -> ProfileContext {
    ctx.labels.insert(labelSQLDigest.to_owned(), sqlDigest);
    ctx
}

/// 写入 sql_digest 与 plan_digest 标签。
pub fn CtxWithSQLAndPlanDigest(
    mut ctx: ProfileContext,
    sqlDigest: String,
    planDigest: String,
) -> ProfileContext {
    ctx.labels.insert(labelSQLDigest.to_owned(), sqlDigest);
    ctx.labels.insert(labelPlanDigest.to_owned(), planDigest);
    ctx
}

/// 写入 sql_global_uid，并同步到线程本地标签。
pub fn CtxWithProcessInfo(mut ctx: ProfileContext, connID: u64, sqlID: u64) -> ProfileContext {
    ctx.labels
        .insert(labelSQLUID.to_owned(), format!("{connID}_{sqlID}"));
    CURRENT_THREAD_PROFILE_LABELS.with(|labels| *labels.borrow_mut() = ctx.labels.clone());
    ctx
}

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

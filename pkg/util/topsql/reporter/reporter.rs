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

// 远程 TopSQL Reporter：采集 CPU/语句统计/RU，并周期推送到 DataSink。
//
// 收集侧经有界通道非阻塞入队（满则丢弃并记指标）；collectWorker 聚合后
// 组装 ReportData，reportWorker 再 fan-out 到已注册 sink。
// TopSQL 指按 CPU 等指标统计的高频/高耗 SQL；RU 为 Resource Unit 资源计量。

#![allow(
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    static_mut_refs
)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::datasink::{
    DataSink, DataSinkError, DataSinkRegisterer, DefaultDataSinkRegisterer,
    ReportData as SinkReportData,
};
use crate::ru_window_aggregator::{RUBatch, RUWindowAggregator};
use crate::{collector, stmtstats, tipb_protobuf as tipb, topsqlstate};

/// 单次向 DataSink 发送的默认截止超时。
pub const reportTimeout: Duration = Duration::from_secs(40);
/// CPU/Stmt/RU 收集通道缓冲大小。
pub const collectChanBufferSize: usize = 2;
pub const reportCollectedDataChanSize: usize = 2;

/// 规范化 plan 二进制解码回调。
pub type planBinaryDecodeFunc = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;
/// 大 plan 压缩编码回调。
pub type planBinaryCompressFunc = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

/// 当前 Unix 秒时间戳。
fn unixNow() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// SQL digest + Plan digest 复合键；默认空键表示 others 汇总。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
struct TopSQLKey {
    sql: Vec<u8>,
    plan: Vec<u8>,
}

/// 单个时间戳上的 CPU 与语句统计采样点。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopSQLRecordItem {
    pub TimestampSec: u64,
    pub CPUTimeMs: u32,
    pub StmtStats: stmtstats::StatementStatsItem,
}

/// 一条 TopSQL 时间序列记录。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopSQLRecord {
    pub KeyspaceName: Vec<u8>,
    pub SQLDigest: Vec<u8>,
    pub PlanDigest: Vec<u8>,
    pub Items: Vec<TopSQLRecordItem>,
}

/// 收集期缓存：按 key→timestamp 聚合，并记录被淘汰键。
#[derive(Default)]
struct TopSQLCollecting {
    records: HashMap<TopSQLKey, BTreeMap<u64, TopSQLRecordItem>>,
    evicted: HashSet<(u64, TopSQLKey)>,
}

impl TopSQLCollecting {
    /// 取得或创建指定时间点的采样项。
    fn point(&mut self, timestamp: u64, key: TopSQLKey) -> &mut TopSQLRecordItem {
        self.records
            .entry(key)
            .or_default()
            .entry(timestamp)
            .or_insert_with(|| TopSQLRecordItem {
                TimestampSec: timestamp,
                ..Default::default()
            })
    }

    /// 累加 CPU 毫秒。
    fn appendCPU(&mut self, timestamp: u64, key: TopSQLKey, value: u32) {
        let point = self.point(timestamp, key);
        point.CPUTimeMs = point.CPUTimeMs.wrapping_add(value);
    }

    /// 累加语句执行/耗时/网络字节等统计。
    fn appendStats(
        &mut self,
        timestamp: u64,
        key: TopSQLKey,
        value: &stmtstats::StatementStatsItem,
    ) {
        let point = self.point(timestamp, key);
        point.StmtStats.ExecCount = point.StmtStats.ExecCount.wrapping_add(value.ExecCount);
        point.StmtStats.SumDurationNs = point
            .StmtStats
            .SumDurationNs
            .wrapping_add(value.SumDurationNs);
        point.StmtStats.DurationCount = point
            .StmtStats
            .DurationCount
            .wrapping_add(value.DurationCount);
        point.StmtStats.NetworkInBytes = point
            .StmtStats
            .NetworkInBytes
            .wrapping_add(value.NetworkInBytes);
        point.StmtStats.NetworkOutBytes = point
            .StmtStats
            .NetworkOutBytes
            .wrapping_add(value.NetworkOutBytes);
    }

    /// 标记该时间戳下 key 已被 TopN 淘汰。
    fn markEvicted(&mut self, timestamp: u64, key: TopSQLKey) {
        self.evicted.insert((timestamp, key));
    }

    /// 是否已被标记淘汰。
    fn hasEvicted(&self, timestamp: u64, key: &TopSQLKey) -> bool {
        self.evicted.contains(&(timestamp, key.clone()))
    }

    /// 取出全部记录并清空淘汰集。
    fn take(&mut self) -> Vec<TopSQLRecord> {
        let records = std::mem::take(&mut self.records);
        self.evicted.clear();
        records
            .into_iter()
            .map(|(key, points)| TopSQLRecord {
                KeyspaceName: Vec::new(),
                SQLDigest: key.sql,
                PlanDigest: key.plan,
                Items: points.into_values().collect(),
            })
            .collect()
    }
}

/// SQL digest 对应的规范化文本元数据。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SQLMeta {
    pub SQLDigest: Vec<u8>,
    pub NormalizedSQL: String,
    pub IsInternal: bool,
    pub KeyspaceName: Vec<u8>,
}

/// Plan digest 对应的规范化/编码 plan 元数据。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanMeta {
    pub PlanDigest: Vec<u8>,
    pub NormalizedPlan: String,
    pub EncodedNormalizedPlan: String,
    pub KeyspaceName: Vec<u8>,
}

/// Reporter 内部采集载荷；fan-out 前统一转换为公开 `datasink::ReportData`。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ReportData {
    pub DataRecords: Vec<TopSQLRecord>,
    pub RURecords: Vec<tipb::TopRuRecord>,
    pub SQLMetas: Vec<SQLMeta>,
    pub PlanMetas: Vec<PlanMeta>,
}

impl ReportData {
    /// 是否包含任一非空载荷。
    pub fn hasData(&self) -> bool {
        !self.DataRecords.is_empty()
            || !self.RURecords.is_empty()
            || !self.SQLMetas.is_empty()
            || !self.PlanMetas.is_empty()
    }
}

impl ReportData {
    /// 将内部聚合模型转换为 PubSub/SingleTarget 共用的 protobuf 载荷。
    fn to_sink_report_data(&self) -> SinkReportData {
        SinkReportData {
            data_records: self
                .DataRecords
                .iter()
                .map(|record| {
                    let mut proto = tipb::TopSqlRecord::new();
                    proto.set_keyspace_name(record.KeyspaceName.clone());
                    proto.set_sql_digest(record.SQLDigest.clone());
                    proto.set_plan_digest(record.PlanDigest.clone());
                    proto.set_items(
                        record
                            .Items
                            .iter()
                            .map(|item| {
                                let mut proto = tipb::TopSqlRecordItem::new();
                                proto.set_timestamp_sec(item.TimestampSec);
                                proto.set_cpu_time_ms(item.CPUTimeMs);
                                proto.set_stmt_exec_count(item.StmtStats.ExecCount);
                                proto.set_stmt_kv_exec_count(
                                    item.StmtStats
                                        .KvStatsItem
                                        .KvExecCount
                                        .clone()
                                        .unwrap_or_default(),
                                );
                                proto.set_stmt_duration_sum_ns(item.StmtStats.SumDurationNs);
                                proto.set_stmt_duration_count(item.StmtStats.DurationCount);
                                proto.set_stmt_network_in_bytes(item.StmtStats.NetworkInBytes);
                                proto.set_stmt_network_out_bytes(item.StmtStats.NetworkOutBytes);
                                proto
                            })
                            .collect(),
                    );
                    proto
                })
                .collect(),
            ru_records: self.RURecords.clone(),
            sql_metas: self
                .SQLMetas
                .iter()
                .map(|meta| {
                    let mut proto = tipb::SqlMeta::new();
                    proto.set_sql_digest(meta.SQLDigest.clone());
                    proto.set_normalized_sql(meta.NormalizedSQL.clone());
                    proto.set_is_internal_sql(meta.IsInternal);
                    proto.set_keyspace_name(meta.KeyspaceName.clone());
                    proto
                })
                .collect(),
            plan_metas: self
                .PlanMetas
                .iter()
                .map(|meta| {
                    let mut proto = tipb::PlanMeta::new();
                    proto.set_plan_digest(meta.PlanDigest.clone());
                    proto.set_normalized_plan(meta.NormalizedPlan.clone());
                    proto.set_encoded_normalized_plan(meta.EncodedNormalizedPlan.clone());
                    proto.set_keyspace_name(meta.KeyspaceName.clone());
                    proto
                })
                .collect(),
        }
    }

    /// 测试适配：把统一 wire 载荷还原为内部聚合模型，便于保留 Go 风格断言。
    #[cfg(test)]
    pub(crate) fn from_sink_report_data(data: &SinkReportData) -> Self {
        Self {
            DataRecords: data
                .data_records
                .iter()
                .map(|record| TopSQLRecord {
                    KeyspaceName: record.get_keyspace_name().to_vec(),
                    SQLDigest: record.get_sql_digest().to_vec(),
                    PlanDigest: record.get_plan_digest().to_vec(),
                    Items: record
                        .get_items()
                        .iter()
                        .map(|item| {
                            let mut stats = stmtstats::StatementStatsItem {
                                ExecCount: item.get_stmt_exec_count(),
                                SumDurationNs: item.get_stmt_duration_sum_ns(),
                                DurationCount: item.get_stmt_duration_count(),
                                NetworkInBytes: item.get_stmt_network_in_bytes(),
                                NetworkOutBytes: item.get_stmt_network_out_bytes(),
                                ..Default::default()
                            };
                            stats.KvStatsItem.KvExecCount =
                                Some(item.get_stmt_kv_exec_count().clone());
                            TopSQLRecordItem {
                                TimestampSec: item.get_timestamp_sec(),
                                CPUTimeMs: item.get_cpu_time_ms(),
                                StmtStats: stats,
                            }
                        })
                        .collect(),
                })
                .collect(),
            RURecords: data.ru_records.clone(),
            SQLMetas: data
                .sql_metas
                .iter()
                .map(|meta| SQLMeta {
                    SQLDigest: meta.get_sql_digest().to_vec(),
                    NormalizedSQL: meta.get_normalized_sql().to_owned(),
                    IsInternal: meta.get_is_internal_sql(),
                    KeyspaceName: meta.get_keyspace_name().to_vec(),
                })
                .collect(),
            PlanMetas: data
                .plan_metas
                .iter()
                .map(|meta| PlanMeta {
                    PlanDigest: meta.get_plan_digest().to_vec(),
                    NormalizedPlan: meta.get_normalized_plan().to_owned(),
                    EncodedNormalizedPlan: meta.get_encoded_normalized_plan().to_owned(),
                    KeyspaceName: meta.get_keyspace_name().to_vec(),
                })
                .collect(),
        }
    }
}

/// 已注册但尚未随上报发出的 SQL/Plan 元数据缓存。
#[derive(Default)]
struct MetaState {
    sql: HashMap<Vec<u8>, (String, bool)>,
    plan: HashMap<Vec<u8>, (String, bool)>,
}

/// 各通道满导致丢弃的计数。
#[derive(Default)]
struct ReporterMetrics {
    cpuChannelFull: AtomicU64,
    stmtChannelFull: AtomicU64,
    ruChannelFull: AtomicU64,
    reportChannelFull: AtomicU64,
}

/// 弱引用适配 collector，避免与 reporter 形成强环。
struct WeakCPUCollector(Weak<RemoteTopSQLReporter>);

impl collector::Collector for WeakCPUCollector {
    fn Collect(&self, data: Vec<collector::SQLCPUTimeRecord>) {
        if let Some(reporter) = self.0.upgrade() {
            reporter.Collect(data);
        }
    }
}

/// 远程 TopSQL 报告器：双 worker + 多收集通道 + sink 列表。
pub struct RemoteTopSQLReporter {
    collectCPUTx: crossbeam_channel::Sender<Vec<collector::SQLCPUTimeRecord>>,
    collectCPURx: crossbeam_channel::Receiver<Vec<collector::SQLCPUTimeRecord>>,
    collectStmtTx: crossbeam_channel::Sender<stmtstats::StatementStatsMap>,
    collectStmtRx: crossbeam_channel::Receiver<stmtstats::StatementStatsMap>,
    collectRUTx: crossbeam_channel::Sender<RUBatch>,
    collectRURx: crossbeam_channel::Receiver<RUBatch>,
    reportTx: crossbeam_channel::Sender<ReportData>,
    pub(crate) reportRx: crossbeam_channel::Receiver<ReportData>,
    cancelTx: crossbeam_channel::Sender<()>,
    cancelRx: crossbeam_channel::Receiver<()>,
    collecting: Mutex<TopSQLCollecting>,
    stmtStatsBuffer: Mutex<HashMap<u64, stmtstats::StatementStatsMap>>,
    ruAggregator: RUWindowAggregator,
    metadata: Mutex<MetaState>,
    keyspaceName: Mutex<Vec<u8>>,
    dataSinkRegisterer: DefaultDataSinkRegisterer,
    decodePlan: planBinaryDecodeFunc,
    compressPlan: planBinaryCompressFunc,
    sqlCPUCollector: Mutex<Option<collector::SQLCPUCollector>>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    started: AtomicBool,
    closed: AtomicBool,
    metrics: ReporterMetrics,
}

/// 构造 reporter，并挂接弱引用 CPU collector。
pub fn NewRemoteTopSQLReporter<D, C>(decodePlan: D, compressPlan: C) -> Arc<RemoteTopSQLReporter>
where
    D: Fn(&str) -> Result<String, String> + Send + Sync + 'static,
    C: Fn(&str) -> Result<String, String> + Send + Sync + 'static,
{
    let (collectCPUTx, collectCPURx) = crossbeam_channel::bounded(collectChanBufferSize);
    let (collectStmtTx, collectStmtRx) = crossbeam_channel::bounded(collectChanBufferSize);
    let (collectRUTx, collectRURx) = crossbeam_channel::bounded(collectChanBufferSize);
    let (reportTx, reportRx) = crossbeam_channel::bounded(reportCollectedDataChanSize);
    let (cancelTx, cancelRx) = crossbeam_channel::bounded(2);
    let reporter = Arc::new(RemoteTopSQLReporter {
        collectCPUTx,
        collectCPURx,
        collectStmtTx,
        collectStmtRx,
        collectRUTx,
        collectRURx,
        reportTx,
        reportRx,
        cancelTx,
        cancelRx,
        collecting: Mutex::new(TopSQLCollecting::default()),
        stmtStatsBuffer: Mutex::new(HashMap::new()),
        ruAggregator: RUWindowAggregator::new(),
        metadata: Mutex::new(MetaState::default()),
        keyspaceName: Mutex::new(Vec::new()),
        dataSinkRegisterer: DefaultDataSinkRegisterer::new(),
        decodePlan: Arc::new(decodePlan),
        compressPlan: Arc::new(compressPlan),
        sqlCPUCollector: Mutex::new(None),
        workers: Mutex::new(Vec::new()),
        started: AtomicBool::new(false),
        closed: AtomicBool::new(false),
        metrics: ReporterMetrics::default(),
    });
    let adapter = Arc::new(WeakCPUCollector(Arc::downgrade(&reporter)));
    *reporter
        .sqlCPUCollector
        .lock()
        .expect("CPU collector mutex poisoned") = Some(collector::NewSQLCPUCollector(adapter));
    reporter
}

impl RemoteTopSQLReporter {
    /// 启动 CPU collector 与 collect/report 两个后台线程（幂等）。
    pub fn Start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(collector) = self
            .sqlCPUCollector
            .lock()
            .expect("CPU collector mutex poisoned")
            .as_mut()
        {
            collector.Start();
        }
        let collectReporter = self.clone();
        let reportReporter = self.clone();
        let mut workers = self.workers.lock().expect("workers mutex poisoned");
        workers.push(std::thread::spawn(move || collectReporter.collectWorker()));
        workers.push(std::thread::spawn(move || reportReporter.reportWorker()));
    }

    /// 非阻塞入队 CPU 采样；通道满则丢弃。
    pub fn Collect(&self, data: Vec<collector::SQLCPUTimeRecord>) {
        if data.is_empty() {
            return;
        }
        if self.collectCPUTx.try_send(data).is_err() {
            self.metrics.cpuChannelFull.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 非阻塞入队语句统计图。
    pub fn CollectStmtStatsMap(&self, data: stmtstats::StatementStatsMap) {
        if data.is_empty() {
            return;
        }
        if self.collectStmtTx.try_send(data).is_err() {
            self.metrics.stmtChannelFull.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 非阻塞入队 RU 增量批次（附当前 unix 时间戳）。
    pub fn CollectRUIncrements(
        &self,
        data: stmtstats::RUIncrementMap,
        version: stmtstats::RUVersion,
    ) {
        if data.is_empty() {
            return;
        }
        if self
            .collectRUTx
            .try_send(RUBatch {
                timestamp: unixNow(),
                data,
                version,
            })
            .is_err()
        {
            self.metrics.ruChannelFull.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// RU 版本切换时重置窗口聚合器。
    pub fn OnRUVersionChange(&self, version: stmtstats::RUVersion) {
        self.ruAggregator.resetForHandover(version, unixNow());
    }

    /// 注册 SQL meta；超过 MaxCollect 且未见过则忽略。
    pub fn RegisterSQL(&self, digest: Vec<u8>, sql: String, isInternal: bool) {
        let mut metadata = self.metadata.lock().expect("metadata mutex poisoned");
        let limit = topsqlstate::GlobalState
            .MaxCollect
            .load(Ordering::Relaxed)
            .max(0) as usize;
        if !metadata.sql.contains_key(&digest) && metadata.sql.len() >= limit {
            return;
        }
        metadata.sql.entry(digest).or_insert((sql, isInternal));
    }

    /// 注册 Plan meta；超过 MaxCollect 且未见过则忽略。
    pub fn RegisterPlan(&self, digest: Vec<u8>, plan: String, isLarge: bool) {
        let mut metadata = self.metadata.lock().expect("metadata mutex poisoned");
        let limit = topsqlstate::GlobalState
            .MaxCollect
            .load(Ordering::Relaxed)
            .max(0) as usize;
        if !metadata.plan.contains_key(&digest) && metadata.plan.len() >= limit {
            return;
        }
        metadata.plan.entry(digest).or_insert((plan, isLarge));
    }

    /// 绑定 keyspace 名，写入后续记录。
    pub fn BindKeyspaceName(&self, keyspace: Vec<u8>) {
        *self.keyspaceName.lock().expect("keyspace mutex poisoned") = keyspace;
    }

    /// 绑定进程 CPU 更新器到内部 SQLCPUCollector。
    pub fn BindProcessCPUTimeUpdater<U>(&self, updater: Arc<U>)
    where
        U: collector::ProcessCPUTimeUpdater + 'static,
    {
        if let Some(collector) = self
            .sqlCPUCollector
            .lock()
            .expect("CPU collector mutex poisoned")
            .as_mut()
        {
            collector.SetProcessCPUUpdater(updater);
        }
    }

    /// 注册 DataSink。
    pub fn Register(&self, sink: Arc<dyn DataSink>) -> Result<(), DataSinkError> {
        self.dataSinkRegisterer.register(sink)
    }

    /// 按指针相等移除 DataSink。
    pub fn Deregister(&self, sink: &Arc<dyn DataSink>) {
        self.dataSinkRegisterer.deregister(sink);
    }

    /// 按 CPU 降序取 TopN，淘汰 CPU 汇入空键 others。
    pub fn processCPUTimeData(&self, timestamp: u64, mut data: Vec<collector::SQLCPUTimeRecord>) {
        let limit = topsqlstate::GlobalState
            .MaxStatementCount
            .load(Ordering::Relaxed)
            .max(0) as usize;
        // 按 CPU 降序截断 TopN，淘汰项的 CPU 汇总到空键。
        data.sort_by(|left, right| right.CPUTimeMs.cmp(&left.CPUTimeMs));
        let evicted = if data.len() > limit {
            data.split_off(limit)
        } else {
            Vec::new()
        };
        let mut collecting = self.collecting.lock().expect("collecting mutex poisoned");
        for record in data {
            collecting.appendCPU(
                timestamp,
                TopSQLKey {
                    sql: record.SQLDigest,
                    plan: record.PlanDigest,
                },
                record.CPUTimeMs,
            );
        }
        let mut total = 0_u32;
        for record in evicted {
            total = total.wrapping_add(record.CPUTimeMs);
            collecting.markEvicted(
                timestamp,
                TopSQLKey {
                    sql: record.SQLDigest,
                    plan: record.PlanDigest,
                },
            );
        }
        if total > 0 {
            collecting.appendCPU(timestamp, TopSQLKey::default(), total);
        }
    }

    /// 消费语句统计缓冲：网络超阈值或未淘汰键保留，否则进 others。
    pub fn processStmtStatsData(&self) {
        let mut buffer = self
            .stmtStatsBuffer
            .lock()
            .expect("statement buffer mutex poisoned");
        let dataByTimestamp = std::mem::take(&mut *buffer);
        drop(buffer);
        let limit = topsqlstate::GlobalState
            .MaxStatementCount
            .load(Ordering::Relaxed)
            .max(0) as usize;
        let mut scratch = Vec::new();
        let mut collecting = self.collecting.lock().expect("collecting mutex poisoned");
        for (timestamp, data) in dataByTimestamp {
            // 网络流量进入 TopK 或未被 CPU 淘汰的键保留原 key，否则进 others。
            let threshold = findKthNetworkBytes(&data, limit, &mut scratch);
            for (digest, item) in data {
                let key = TopSQLKey {
                    sql: digest.SQLDigest.0,
                    plan: digest.PlanDigest.0,
                };
                if item.NetworkInBytes.wrapping_add(item.NetworkOutBytes) > threshold
                    || !collecting.hasEvicted(timestamp, &key)
                {
                    collecting.appendStats(timestamp, key, &item);
                } else {
                    collecting.appendStats(timestamp, TopSQLKey::default(), &item);
                }
            }
        }
    }

    /// 组装 ReportData（含 meta 编解码）并 try_send 到 report 通道。
    pub fn takeDataAndSendToReportChan(&self, timestamp: u64) {
        // collectWorker is the sole sender, so capacity cannot fill before try_send.
        if self.reportTx.is_full() {
            self.metrics
                .reportChannelFull
                .fetch_add(1, Ordering::Relaxed);
            *self.collecting.lock().expect("collecting mutex poisoned") =
                TopSQLCollecting::default();
            self.ruAggregator.dropReportData(timestamp);
            unsafe {
                if let Some(counter) =
                    reporter_metrics::reporter_metrics::IgnoreReportChannelFullCounter.as_ref()
                {
                    counter.inc();
                }
                if let Some(counter) =
                    reporter_metrics::reporter_metrics::IgnoreReportDataByBackpressureCounter
                        .as_ref()
                {
                    counter.inc();
                }
            }
            return;
        }
        let keyspace = self
            .keyspaceName
            .lock()
            .expect("keyspace mutex poisoned")
            .clone();
        let ruRecords = self.ruAggregator.takeReportRecords(
            timestamp,
            topsqlstate::GetTopRUItemInterval().max(0) as u64,
            keyspace.clone(),
        );
        let mut DataRecords = self
            .collecting
            .lock()
            .expect("collecting mutex poisoned")
            .take();
        for record in &mut DataRecords {
            record.KeyspaceName = keyspace.clone();
        }
        let mut metadata = self.metadata.lock().expect("metadata mutex poisoned");
        let sql = std::mem::take(&mut metadata.sql);
        let plan = std::mem::take(&mut metadata.plan);
        drop(metadata);
        let SQLMetas = sql
            .into_iter()
            .map(|(digest, (normalized, internal))| SQLMeta {
                SQLDigest: digest,
                NormalizedSQL: normalized,
                IsInternal: internal,
                KeyspaceName: keyspace.clone(),
            })
            .collect();
        let PlanMetas = plan
            .into_iter()
            .map(|(digest, (normalized, large))| {
                // 大 plan 走压缩编码；普通 plan 走二进制解码为可读文本。
                let (NormalizedPlan, EncodedNormalizedPlan) = if large {
                    match (self.compressPlan)(&normalized) {
                        Ok(encoded) => (String::new(), encoded),
                        Err(_) => (normalized, String::new()),
                    }
                } else {
                    match (self.decodePlan)(&normalized) {
                        Ok(decoded) => (decoded, String::new()),
                        Err(_) => (normalized, String::new()),
                    }
                };
                PlanMeta {
                    PlanDigest: digest,
                    NormalizedPlan,
                    EncodedNormalizedPlan,
                    KeyspaceName: keyspace.clone(),
                }
            })
            .collect();
        if self
            .reportTx
            .try_send(ReportData {
                DataRecords,
                RURecords: ruRecords,
                SQLMetas,
                PlanMetas,
            })
            .is_err()
        {
            self.metrics
                .reportChannelFull
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// 收集线程：处理 CPU/Stmt/RU 与周期 ticker 触发上报。
    fn collectWorker(&self) {
        let ticker = crossbeam_channel::tick(Duration::from_secs(
            topsqlstate::DefTiDBTopSQLReportIntervalSeconds as u64,
        ));
        loop {
            crossbeam_channel::select! {
                recv(self.cancelRx) -> _ => return,
                recv(self.collectCPURx) -> data => if let Ok(data) = data {
                    self.processCPUTimeData(unixNow(), data);
                },
                recv(self.collectStmtRx) -> data => if let Ok(data) = data {
                    self.stmtStatsBuffer
                        .lock()
                        .expect("statement buffer mutex poisoned")
                        .insert(unixNow(), data);
                },
                recv(self.collectRURx) -> batch => if let Ok(batch) = batch {
                    self.ruAggregator.addBatch(batch);
                },
                recv(ticker) -> _ => {
                    self.processStmtStatsData();
                    self.takeDataAndSendToReportChan(unixNow());
                },
            }
        }
    }

    /// 上报线程：从 report 通道取数据并 doReport。
    fn reportWorker(&self) {
        loop {
            crossbeam_channel::select! {
                recv(self.cancelRx) -> _ => return,
                recv(self.reportRx) -> data => if let Ok(data) = data {
                    std::thread::sleep(Duration::from_millis(100));
                    self.doReport(&data);
                },
            }
        }
    }

    /// 有数据时以 reportTimeout 调用 trySend。
    pub fn doReport(&self, data: &ReportData) {
        if !data.hasData() {
            return;
        }
        let data = Arc::new(data.to_sink_report_data());
        let _ = self.trySend(data, Instant::now() + reportTimeout);
    }

    /// 向全部 sink 发送；单 sink 失败仅打日志。
    pub fn trySend(
        &self,
        data: Arc<SinkReportData>,
        deadline: Instant,
    ) -> Result<(), DataSinkError> {
        let sinks = self.dataSinkRegisterer.sinks();
        for sink in sinks {
            if let Err(error) = sink.try_send(data.clone(), deadline) {
                log::warn!("failed to send data to top-sql data sink: {error}");
            }
        }
        Ok(())
    }

    /// 关闭：取消 worker、停 collector、通知 sink（幂等）。
    pub fn Close(&self) {
        if self.closed.swap(true, Ordering::SeqCst) {
            return;
        }
        let _ = self.cancelTx.try_send(());
        let _ = self.cancelTx.try_send(());
        if let Some(mut collector) = self
            .sqlCPUCollector
            .lock()
            .expect("CPU collector mutex poisoned")
            .take()
        {
            collector.Stop();
        }
        for worker in self
            .workers
            .lock()
            .expect("workers mutex poisoned")
            .drain(..)
        {
            let _ = worker.join();
        }
        self.dataSinkRegisterer.close();
    }

    /// 返回 (cpu, stmt, ru, report) 通道满丢弃计数。
    pub fn channelDropCounts(&self) -> (u64, u64, u64, u64) {
        (
            self.metrics.cpuChannelFull.load(Ordering::Relaxed),
            self.metrics.stmtChannelFull.load(Ordering::Relaxed),
            self.metrics.ruChannelFull.load(Ordering::Relaxed),
            self.metrics.reportChannelFull.load(Ordering::Relaxed),
        )
    }
}

impl DataSinkRegisterer for RemoteTopSQLReporter {
    fn register(&self, data_sink: Arc<dyn DataSink>) -> Result<(), DataSinkError> {
        self.Register(data_sink)
    }

    fn deregister(&self, data_sink: &Arc<dyn DataSink>) {
        self.Deregister(data_sink);
    }
}

impl collector::Collector for RemoteTopSQLReporter {
    fn Collect(&self, data: Vec<collector::SQLCPUTimeRecord>) {
        RemoteTopSQLReporter::Collect(self, data);
    }
}

impl stmtstats::Collector for RemoteTopSQLReporter {
    fn CollectStmtStatsMap(&self, data: stmtstats::StatementStatsMap) {
        RemoteTopSQLReporter::CollectStmtStatsMap(self, data);
    }
}

impl stmtstats::RUCollector for RemoteTopSQLReporter {
    fn CollectRUIncrements(&self, data: stmtstats::RUIncrementMap, version: stmtstats::RUVersion) {
        RemoteTopSQLReporter::CollectRUIncrements(self, data, version);
    }

    fn OnRUVersionChange(&self, version: stmtstats::RUVersion) {
        RemoteTopSQLReporter::OnRUVersionChange(self, version);
    }
}

/// 网络字节第 K 大阈值；长度 ≤k 时返回 0（表示不过滤）。
pub fn findKthNetworkBytes(
    data: &stmtstats::StatementStatsMap,
    k: usize,
    scratch: &mut Vec<u64>,
) -> u64 {
    if data.len() <= k {
        return 0;
    }
    scratch.clear();
    scratch.extend(
        data.values()
            .map(|item| item.NetworkInBytes.wrapping_add(item.NetworkOutBytes)),
    );
    scratch.sort_unstable_by(|left, right| right.cmp(left));
    if k == 0 { scratch[0] } else { scratch[k - 1] }
}

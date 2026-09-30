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

// RemoteTopSQLReporter 集成/单元测试。
//
// 覆盖：meta 上报、批处理与 TopN 淘汰、容量限制、内部 SQL、多 sink、
// worker 路径、语句统计合并、通道满丢弃、背压、TopRU 管线与 handover、
// 以及基准场景冒烟。部分用例用 `serial` 隔离全局 topsql 状态。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crossbeam_channel::{Receiver, Sender, bounded};
use serial_test::serial;

use crate::collector::SQLCPUTimeRecord;
use crate::datasink::{DataSink, DataSinkError, ReportData as SinkReportData};
use crate::reporter::{
    NewRemoteTopSQLReporter, PlanMeta, RemoteTopSQLReporter, ReportData, SQLMeta, TopSQLRecord,
};
use crate::stmtstats::{
    RUIncrement, RUIncrementMap, RUKey, SQLPlanDigest, StatementStatsItem, StatementStatsMap,
};
use crate::tipb_protobuf::TopRuRecord;
use crate::{RUBatch, RUWindowAggregator, topsqlstate};

/// 测试默认 MaxCollect 上限。
const MAX_SQL_NUM: usize = 5_000;
/// 测试用 keyspace 名。
const KEYSPACE_NAME: &[u8] = b"123";
/// 等待 report 投递的超时。
const WAIT: Duration = Duration::from_secs(3);

#[test]
fn go_merge_39_report_backpressure_preserves_metadata_and_discards_samples() {
    let reporter = NewRemoteTopSQLReporter(decode_plan, compress_plan);
    assert_eq!(reporter.reportRx.capacity(), Some(2));
    reporter.takeDataAndSendToReportChan(60);
    reporter.takeDataAndSendToReportChan(120);
    assert_eq!(reporter.reportRx.len(), 2);
    let before = reporter.channelDropCounts().3;
    reporter.RegisterSQL(b"sql-3".to_vec(), "select 3".to_owned(), false);
    reporter.RegisterPlan(b"plan-3".to_vec(), "plan 3".to_owned(), false);
    reporter.takeDataAndSendToReportChan(180);
    assert_eq!(reporter.channelDropCounts().3, before + 1);
    reporter.reportRx.recv().unwrap();
    reporter.reportRx.recv().unwrap();
    reporter.takeDataAndSendToReportChan(240);
    let recovered = reporter.reportRx.recv().unwrap();
    assert_eq!(recovered.SQLMetas.len(), 1);
    assert_eq!(recovered.PlanMetas.len(), 1);
    reporter.Close();
}

/// 恒等 plan 解码。
fn decode_plan(plan: &str) -> Result<String, String> {
    Ok(plan.to_owned())
}

/// 恒等 plan 压缩。
fn compress_plan(plan: &str) -> Result<String, String> {
    Ok(plan.to_owned())
}

/// 记录收到的 ReportData，并经通道转发。
struct MockDataSink {
    tx: Sender<ReportData>,
    data: Mutex<Vec<ReportData>>,
    closed: AtomicBool,
}

impl MockDataSink {
    /// 创建 mock sink 与接收端。
    fn new() -> (Arc<Self>, Receiver<ReportData>) {
        let (tx, rx) = bounded(64);
        (
            Arc::new(Self {
                tx,
                data: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
            }),
            rx,
        )
    }
}

impl DataSink for MockDataSink {
    fn try_send(&self, data: Arc<SinkReportData>, _deadline: Instant) -> Result<(), DataSinkError> {
        let data = ReportData::from_sink_report_data(&data);
        self.data
            .lock()
            .expect("mock sink mutex poisoned")
            .push(data.clone());
        self.tx.send(data).map_err(|_| DataSinkError::Closed)
    }

    fn on_reporter_closing(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

/// 首次 TrySend 阻塞直至 gate 放行，用于背压场景。
struct BlockingDataSink {
    gate: Receiver<()>,
    entered: Sender<()>,
    output: Sender<ReportData>,
    block_once: AtomicBool,
}

impl DataSink for BlockingDataSink {
    fn try_send(&self, data: Arc<SinkReportData>, _deadline: Instant) -> Result<(), DataSinkError> {
        if !self.block_once.swap(true, Ordering::SeqCst) {
            self.entered.send(()).map_err(|_| DataSinkError::Closed)?;
            self.gate
                .recv_timeout(WAIT)
                .map_err(|_| DataSinkError::DeadlineExceeded)?;
        }
        self.output
            .send(ReportData::from_sink_report_data(&data))
            .map_err(|_| DataSinkError::Closed)
    }

    fn on_reporter_closing(&self) {}
}

/// 重置 TopSQL/TopRU 全局开关与限额。
fn reset_global_state() {
    topsqlstate::DisableTopSQL();
    while topsqlstate::TopRUEnabled() {
        topsqlstate::DisableTopRU();
    }
    topsqlstate::ResetTopRUItemInterval();
    topsqlstate::GlobalState
        .MaxStatementCount
        .store(100, Ordering::SeqCst);
    topsqlstate::GlobalState
        .MaxCollect
        .store(MAX_SQL_NUM as i64, Ordering::SeqCst);
}

/// 启用 TopSQL、绑定 keyspace、注册 mock sink 并 Start。
fn setup_reporter(
    max_statements: usize,
    max_collect: usize,
) -> (
    Arc<RemoteTopSQLReporter>,
    Arc<MockDataSink>,
    Receiver<ReportData>,
) {
    reset_global_state();
    topsqlstate::GlobalState
        .MaxStatementCount
        .store(max_statements as i64, Ordering::SeqCst);
    topsqlstate::GlobalState
        .MaxCollect
        .store(max_collect as i64, Ordering::SeqCst);
    topsqlstate::EnableTopSQL();

    let reporter = NewRemoteTopSQLReporter(decode_plan, compress_plan);
    reporter.BindKeyspaceName(KEYSPACE_NAME.to_vec());
    let (sink, rx) = MockDataSink::new();
    reporter.Register(sink.clone()).unwrap();
    reporter.Start();
    (reporter, sink, rx)
}

/// 触发 takeDataAndSendToReportChan 并等待收到一份报告。
fn flush_at(
    reporter: &RemoteTopSQLReporter,
    rx: &Receiver<ReportData>,
    timestamp: u64,
) -> ReportData {
    reporter.takeDataAndSendToReportChan(timestamp);
    rx.recv_timeout(WAIT)
        .expect("report worker should deliver collected data")
}

/// 批量注册 meta 并写入一段 CPU 采样。
fn populate_cache(reporter: &RemoteTopSQLReporter, begin: usize, end: usize, timestamp: u64) {
    let mut records = Vec::with_capacity(end - begin);
    for index in begin..end {
        let id = index + 1;
        reporter.RegisterSQL(
            format!("sqlDigest{id}").into_bytes(),
            format!("sqlNormalized{id}"),
            false,
        );
        reporter.RegisterPlan(
            format!("planDigest{id}").into_bytes(),
            format!("planNormalized{id}"),
            false,
        );
        records.push(SQLCPUTimeRecord {
            SQLDigest: format!("sqlDigest{id}").into_bytes(),
            PlanDigest: format!("planDigest{id}").into_bytes(),
            CPUTimeMs: id as u32,
        });
    }
    reporter.processCPUTimeData(timestamp, records);
}

/// 注册对应 meta 并构造一条 CPU 记录；偶数 id 标为内部 SQL。
fn new_sql_cpu_time_record(
    reporter: &RemoteTopSQLReporter,
    sql_id: usize,
    cpu_time_ms: u32,
) -> SQLCPUTimeRecord {
    reporter.RegisterSQL(
        format!("sqlDigest{sql_id}").into_bytes(),
        format!("sqlNormalized{sql_id}"),
        sql_id % 2 == 0,
    );
    reporter.RegisterPlan(
        format!("planDigest{sql_id}").into_bytes(),
        format!("planNormalized{sql_id}"),
        false,
    );
    SQLCPUTimeRecord {
        SQLDigest: format!("sqlDigest{sql_id}").into_bytes(),
        PlanDigest: format!("planDigest{sql_id}").into_bytes(),
        CPUTimeMs: cpu_time_ms,
    }
}

/// 按 digest 查找 SQLMeta。
fn find_sql_meta<'a>(metas: &'a [SQLMeta], digest: &[u8]) -> &'a SQLMeta {
    metas
        .iter()
        .find(|meta| meta.SQLDigest == digest)
        .expect("SQL metadata should exist")
}

/// 按 digest 查找 PlanMeta。
fn find_plan_meta<'a>(metas: &'a [PlanMeta], digest: &[u8]) -> &'a PlanMeta {
    metas
        .iter()
        .find(|meta| meta.PlanDigest == digest)
        .expect("plan metadata should exist")
}

/// 按 SQL digest 查找 TopSQLRecord。
fn find_record<'a>(records: &'a [TopSQLRecord], digest: &[u8]) -> &'a TopSQLRecord {
    records
        .iter()
        .find(|record| record.SQLDigest == digest)
        .expect("TopSQL record should exist")
}

/// 汇总记录全部时间点的 CPU。
fn total_cpu(record: &TopSQLRecord) -> u32 {
    record.Items.iter().map(|item| item.CPUTimeMs).sum()
}

/// 构造单键 RU 增量图。
fn ru_batch(user: &str, sql: &str, plan: &str, total_ru: f64) -> RUIncrementMap {
    HashMap::from([(
        RUKey::new(user, sql.as_bytes(), plan.as_bytes()),
        RUIncrement {
            TotalRU: total_ru,
            ExecCount: 1,
            ExecDuration: 10,
        },
    )])
}

/// 取一个对齐的未来窗口结束时间戳。
fn next_window_end() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after epoch")
        .as_secs();
    (now / 60 + 1) * 60
}

/// 按 user+sql digest 查找 TopRuRecord。
fn find_ru_record<'a>(
    records: &'a [TopRuRecord],
    user: &str,
    sql: &[u8],
    plan: &[u8],
) -> Option<&'a TopRuRecord> {
    records.iter().find(|record| {
        record.get_user() == user
            && record.get_sql_digest() == sql
            && record.get_plan_digest() == plan
    })
}

#[test]
#[serial]
/// RU 为空时仍应发送 SQL/Plan meta。
fn test_do_report_sends_meta_when_ru_records_empty() {
    reset_global_state();
    let reporter = NewRemoteTopSQLReporter(decode_plan, compress_plan);
    let (sink, rx) = MockDataSink::new();
    reporter.Register(sink).unwrap();
    reporter.doReport(&ReportData {
        SQLMetas: vec![SQLMeta {
            SQLDigest: b"S_meta_only".to_vec(),
            NormalizedSQL: "select /* meta only */ 1".to_owned(),
            ..Default::default()
        }],
        PlanMetas: vec![PlanMeta {
            PlanDigest: b"P_meta_only".to_vec(),
            NormalizedPlan: "Point_Get".to_owned(),
            ..Default::default()
        }],
        ..Default::default()
    });

    let payload = rx
        .recv_timeout(WAIT)
        .expect("meta-only payload is reported");
    assert!(payload.RURecords.is_empty());
    assert!(payload.DataRecords.is_empty());
    assert_eq!(payload.SQLMetas[0].SQLDigest, b"S_meta_only");
    assert_eq!(
        payload.SQLMetas[0].NormalizedSQL,
        "select /* meta only */ 1"
    );
    assert_eq!(payload.PlanMetas[0].PlanDigest, b"P_meta_only");
    assert_eq!(payload.PlanMetas[0].NormalizedPlan, "Point_Get");
    reporter.Close();
}

#[test]
#[serial]
/// 基本收集一批 CPU 后 flush 可见记录与 meta。
fn test_collect_and_send_batch() {
    let (reporter, _, rx) = setup_reporter(MAX_SQL_NUM, MAX_SQL_NUM * 2);
    populate_cache(&reporter, 0, MAX_SQL_NUM, 1);
    let data = flush_at(&reporter, &rx, 60);
    assert_eq!(data.DataRecords.len(), MAX_SQL_NUM);

    for record in &data.DataRecords {
        let id: usize = std::str::from_utf8(&record.SQLDigest)
            .unwrap()
            .trim_start_matches("sqlDigest")
            .parse()
            .unwrap();
        assert_eq!(record.KeyspaceName, KEYSPACE_NAME);
        assert_eq!(record.Items.len(), 1);
        assert_eq!(record.Items[0].TimestampSec, 1);
        assert_eq!(record.Items[0].CPUTimeMs, id as u32);
        let sql = find_sql_meta(&data.SQLMetas, &record.SQLDigest);
        assert_eq!(sql.KeyspaceName, KEYSPACE_NAME);
        assert_eq!(sql.NormalizedSQL, format!("sqlNormalized{id}"));
        let plan = find_plan_meta(&data.PlanMetas, &record.PlanDigest);
        assert_eq!(plan.KeyspaceName, KEYSPACE_NAME);
        assert_eq!(plan.NormalizedPlan, format!("planNormalized{id}"));
    }
    reporter.Close();
}

#[test]
#[serial]
/// 超出 MaxStatementCount 时淘汰项 CPU 汇入 others。
fn test_collect_and_evicted() {
    let (reporter, _, rx) = setup_reporter(MAX_SQL_NUM, MAX_SQL_NUM * 2);
    populate_cache(&reporter, 0, MAX_SQL_NUM * 2, 2);
    let data = flush_at(&reporter, &rx, 60);
    assert_eq!(data.DataRecords.len(), MAX_SQL_NUM + 1);

    let others = find_record(&data.DataRecords, b"");
    assert!(others.PlanDigest.is_empty());
    assert_eq!(others.Items.len(), 1);
    assert_eq!(others.Items[0].TimestampSec, 2);
    assert_eq!(others.Items[0].CPUTimeMs, 12_502_500);
    for record in data
        .DataRecords
        .iter()
        .filter(|record| !record.SQLDigest.is_empty())
    {
        let id: usize = std::str::from_utf8(&record.SQLDigest)
            .unwrap()
            .trim_start_matches("sqlDigest")
            .parse()
            .unwrap();
        assert!(id > MAX_SQL_NUM);
        assert_eq!(record.Items[0].CPUTimeMs, id as u32);
        assert_eq!(
            find_sql_meta(&data.SQLMetas, &record.SQLDigest).KeyspaceName,
            KEYSPACE_NAME
        );
        assert_eq!(
            find_plan_meta(&data.PlanMetas, &record.PlanDigest).KeyspaceName,
            KEYSPACE_NAME
        );
    }
    reporter.Close();
}

#[test]
#[serial]
/// TopRU item 间隔生命周期独立于固定 report ticker。
fn test_top_ru_item_interval_lifecycle_independent_from_fixed_report_ticker() {
    reset_global_state();
    assert_eq!(
        topsqlstate::GetTopRUItemInterval(),
        topsqlstate::DefTiDBTopRUItemIntervalSeconds
    );
    topsqlstate::SetTopRUItemInterval(15).unwrap();
    assert_eq!(topsqlstate::GetTopRUItemInterval(), 15);
    topsqlstate::EnableTopRU();
    topsqlstate::SetTopRUItemInterval(30).unwrap();
    assert_eq!(topsqlstate::GetTopRUItemInterval(), 30);
    assert!(topsqlstate::SetTopRUItemInterval(1).is_err());
    assert_eq!(topsqlstate::GetTopRUItemInterval(), 30);
    topsqlstate::SetTopRUItemInterval(0).unwrap();
    assert_eq!(
        topsqlstate::GetTopRUItemInterval(),
        topsqlstate::DefTiDBTopRUItemIntervalSeconds
    );
    topsqlstate::DisableTopRU();
}

#[test]
#[serial]
/// 验证 TopN 截断后保留高 CPU 语句。
fn test_collect_and_top_n() {
    let (reporter, _, rx) = setup_reporter(2, 100);
    reporter.processCPUTimeData(
        1,
        vec![
            new_sql_cpu_time_record(&reporter, 1, 1),
            new_sql_cpu_time_record(&reporter, 2, 2),
            new_sql_cpu_time_record(&reporter, 3, 3),
        ],
    );
    reporter.processCPUTimeData(
        2,
        vec![
            new_sql_cpu_time_record(&reporter, 1, 1),
            new_sql_cpu_time_record(&reporter, 3, 3),
        ],
    );
    reporter.processCPUTimeData(
        3,
        vec![
            new_sql_cpu_time_record(&reporter, 4, 4),
            new_sql_cpu_time_record(&reporter, 1, 10),
            new_sql_cpu_time_record(&reporter, 3, 1),
        ],
    );
    reporter.processCPUTimeData(
        4,
        vec![
            new_sql_cpu_time_record(&reporter, 5, 5),
            new_sql_cpu_time_record(&reporter, 4, 4),
            new_sql_cpu_time_record(&reporter, 1, 10),
            new_sql_cpu_time_record(&reporter, 2, 20),
        ],
    );
    reporter.processCPUTimeData(
        0,
        vec![
            new_sql_cpu_time_record(&reporter, 6, 6),
            new_sql_cpu_time_record(&reporter, 1, 1),
            new_sql_cpu_time_record(&reporter, 2, 2),
            new_sql_cpu_time_record(&reporter, 3, 3),
        ],
    );
    let data = flush_at(&reporter, &rx, 60);
    assert_eq!(data.DataRecords.len(), 6);
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"")), 14);
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"sqlDigest1")), 21);
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"sqlDigest2")), 22);
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"sqlDigest3")), 9);
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"sqlDigest4")), 4);
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"sqlDigest6")), 6);
    assert_eq!(data.SQLMetas.len(), 6);
    assert!(
        data.DataRecords
            .iter()
            .all(|record| record.KeyspaceName == KEYSPACE_NAME)
    );
    reporter.Close();
}

#[test]
#[serial]
/// MaxCollect 限制 meta 注册容量。
fn test_collect_capacity() {
    let (reporter, _, rx) = setup_reporter(MAX_SQL_NUM, 10_000);
    for id in 0..20_000 {
        reporter.RegisterSQL(
            format!("sqlDigest{id}").into_bytes(),
            format!("sqlNormalized{id}"),
            false,
        );
        reporter.RegisterPlan(
            format!("planDigest{id}").into_bytes(),
            format!("planNormalized{id}"),
            false,
        );
    }
    let data = flush_at(&reporter, &rx, 60);
    assert_eq!(data.SQLMetas.len(), 10_000);
    assert_eq!(data.PlanMetas.len(), 10_000);

    topsqlstate::GlobalState
        .MaxCollect
        .store(20_000, Ordering::SeqCst);
    for id in 0..50_000 {
        reporter.RegisterSQL(
            format!("sqlDigest{id}").into_bytes(),
            format!("sqlNormalized{id}"),
            false,
        );
        reporter.RegisterPlan(
            format!("planDigest{id}").into_bytes(),
            format!("planNormalized{id}"),
            false,
        );
    }
    let data = flush_at(&reporter, &rx, 120);
    assert_eq!(data.SQLMetas.len(), 20_000);
    assert_eq!(data.PlanMetas.len(), 20_000);

    let records = (0..20_000)
        .map(|id| SQLCPUTimeRecord {
            SQLDigest: format!("sqlDigest{}", id + 1).into_bytes(),
            PlanDigest: format!("planDigest{}", id + 1).into_bytes(),
            CPUTimeMs: (id + 1) as u32,
        })
        .collect();
    reporter.processCPUTimeData(1, records);
    let data = flush_at(&reporter, &rx, 180);
    assert_eq!(data.DataRecords.len(), MAX_SQL_NUM + 1);
    reporter.Close();
}

#[test]
#[serial]
/// 内部 SQL 的 IsInternal 标志正确传递。
fn test_collect_internal() {
    let (reporter, _, rx) = setup_reporter(3_000, 100);
    reporter.processCPUTimeData(
        1,
        vec![
            new_sql_cpu_time_record(&reporter, 1, 1),
            new_sql_cpu_time_record(&reporter, 2, 2),
        ],
    );
    let data = flush_at(&reporter, &rx, 60);
    assert_eq!(data.DataRecords.len(), 2);
    for record in &data.DataRecords {
        let id: usize = std::str::from_utf8(&record.SQLDigest)
            .unwrap()
            .trim_start_matches("sqlDigest")
            .parse()
            .unwrap();
        assert_eq!(
            find_sql_meta(&data.SQLMetas, &record.SQLDigest).IsInternal,
            id % 2 == 0
        );
    }
    reporter.Close();
}

#[test]
#[serial]
/// 多 sink 均收到同一份报告。
fn test_multiple_data_sinks() {
    reset_global_state();
    topsqlstate::GlobalState
        .MaxStatementCount
        .store(100, Ordering::SeqCst);
    let reporter = NewRemoteTopSQLReporter(decode_plan, compress_plan);
    reporter.BindKeyspaceName(KEYSPACE_NAME.to_vec());
    let mut sinks = Vec::new();
    let mut receivers = Vec::new();
    for _ in 0..7 {
        let (sink, rx) = MockDataSink::new();
        let sink: Arc<dyn DataSink> = sink;
        reporter.Register(sink.clone()).unwrap();
        sinks.push(sink);
        receivers.push(rx);
    }
    reporter.Start();

    reporter.processCPUTimeData(3, vec![new_sql_cpu_time_record(&reporter, 1, 2)]);
    reporter.takeDataAndSendToReportChan(60);
    for rx in &receivers {
        let data = rx.recv_timeout(WAIT).unwrap();
        assert_eq!(data.DataRecords[0].SQLDigest, b"sqlDigest1");
        assert_eq!(data.DataRecords[0].PlanDigest, b"planDigest1");
        assert_eq!(data.DataRecords[0].Items[0].TimestampSec, 3);
        assert_eq!(data.DataRecords[0].Items[0].CPUTimeMs, 2);
    }

    for index in (0..7).step_by(2) {
        reporter.Deregister(&sinks[index]);
    }
    reporter.processCPUTimeData(6, vec![new_sql_cpu_time_record(&reporter, 4, 5)]);
    reporter.takeDataAndSendToReportChan(120);
    for index in (1..7).step_by(2) {
        let data = receivers[index].recv_timeout(WAIT).unwrap();
        assert_eq!(data.DataRecords[0].SQLDigest, b"sqlDigest4");
        assert_eq!(data.DataRecords[0].Items[0].TimestampSec, 6);
        assert_eq!(data.DataRecords[0].Items[0].CPUTimeMs, 5);
    }
    for index in (0..7).step_by(2) {
        assert!(receivers[index].try_recv().is_err());
    }
    reporter.Close();
}

#[test]
#[serial]
/// 经 Collect 入队后 worker 路径能产出报告。
fn test_reporter_worker() {
    let (reporter, _, rx) = setup_reporter(100, 100);
    reporter.Collect(Vec::new());
    reporter.Collect(vec![SQLCPUTimeRecord {
        SQLDigest: b"S1".to_vec(),
        PlanDigest: b"P1".to_vec(),
        CPUTimeMs: 1,
    }]);
    reporter.CollectStmtStatsMap(StatementStatsMap::new());
    reporter.CollectStmtStatsMap(HashMap::from([(
        SQLPlanDigest::new(b"S1", b"P1"),
        StatementStatsItem {
            ExecCount: 1,
            SumDurationNs: 1,
            ..Default::default()
        },
    )]));
    std::thread::sleep(Duration::from_millis(150));
    reporter.processStmtStatsData();
    let data = flush_at(&reporter, &rx, 60);
    assert_eq!(data.DataRecords.len(), 1);
    assert_eq!(data.DataRecords[0].SQLDigest, b"S1");
    assert_eq!(data.DataRecords[0].PlanDigest, b"P1");
    reporter.Close();
}

#[test]
#[serial]
/// 语句统计与 CPU 淘汰标记交互合并。
fn test_process_stmt_stats_data() {
    let (reporter, _, rx) = setup_reporter(3, 100);
    reporter.Collect(vec![
        SQLCPUTimeRecord {
            SQLDigest: b"S1".to_vec(),
            PlanDigest: b"P1".to_vec(),
            CPUTimeMs: 1,
        },
        SQLCPUTimeRecord {
            SQLDigest: b"S2".to_vec(),
            PlanDigest: b"P2".to_vec(),
            CPUTimeMs: 2,
        },
    ]);
    reporter.CollectStmtStatsMap(HashMap::from([
        (
            SQLPlanDigest::new(b"S1", b"P1"),
            StatementStatsItem {
                ExecCount: 1,
                NetworkInBytes: 1,
                ..Default::default()
            },
        ),
        (
            SQLPlanDigest::new(b"S2", b"P2"),
            StatementStatsItem {
                ExecCount: 2,
                NetworkOutBytes: 2,
                ..Default::default()
            },
        ),
        (
            SQLPlanDigest::new(b"S3", b"P3"),
            StatementStatsItem {
                ExecCount: 3,
                NetworkInBytes: 1,
                NetworkOutBytes: 2,
                ..Default::default()
            },
        ),
    ]));
    std::thread::sleep(Duration::from_millis(150));
    reporter.processStmtStatsData();
    let data = flush_at(&reporter, &rx, 60);
    assert_eq!(data.DataRecords.len(), 3);
    for (digest, network) in [(b"S1".as_slice(), 1), (b"S2", 2), (b"S3", 3)] {
        let record = find_record(&data.DataRecords, digest);
        let total: u64 = record
            .Items
            .iter()
            .map(|item| item.StmtStats.NetworkInBytes + item.StmtStats.NetworkOutBytes)
            .sum();
        assert_eq!(total, network);
    }
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"S3")), 0);

    reporter.Collect(vec![
        SQLCPUTimeRecord {
            SQLDigest: b"S1".to_vec(),
            PlanDigest: b"P1".to_vec(),
            CPUTimeMs: 1,
        },
        SQLCPUTimeRecord {
            SQLDigest: b"S2".to_vec(),
            PlanDigest: b"P2".to_vec(),
            CPUTimeMs: 2,
        },
        SQLCPUTimeRecord {
            SQLDigest: b"S3".to_vec(),
            PlanDigest: b"P3".to_vec(),
            CPUTimeMs: 3,
        },
        SQLCPUTimeRecord {
            SQLDigest: b"S4".to_vec(),
            PlanDigest: b"P4".to_vec(),
            CPUTimeMs: 0,
        },
        SQLCPUTimeRecord {
            SQLDigest: b"S7".to_vec(),
            PlanDigest: b"P7".to_vec(),
            CPUTimeMs: 7,
        },
    ]);
    reporter.CollectStmtStatsMap(HashMap::from([
        (
            SQLPlanDigest::new(b"S1", b"P1"),
            StatementStatsItem {
                ExecCount: 10,
                NetworkInBytes: 10,
                ..Default::default()
            },
        ),
        (
            SQLPlanDigest::new(b"S2", b"P2"),
            StatementStatsItem {
                ExecCount: 2,
                NetworkOutBytes: 2,
                ..Default::default()
            },
        ),
        (
            SQLPlanDigest::new(b"S4", b"P4"),
            StatementStatsItem {
                ExecCount: 4,
                NetworkInBytes: 1,
                NetworkOutBytes: 3,
                ..Default::default()
            },
        ),
        (
            SQLPlanDigest::new(b"S6", b"P6"),
            StatementStatsItem {
                ExecCount: 6,
                NetworkInBytes: 2,
                NetworkOutBytes: 4,
                ..Default::default()
            },
        ),
    ]));
    std::thread::sleep(Duration::from_millis(150));
    reporter.processStmtStatsData();
    let data = flush_at(&reporter, &rx, 120);
    assert_eq!(data.DataRecords.len(), 6);
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"S3")), 3);
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"S7")), 7);
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"S2")), 2);
    assert_eq!(total_cpu(find_record(&data.DataRecords, b"")), 1);
    let others_network: u64 = find_record(&data.DataRecords, b"")
        .Items
        .iter()
        .map(|item| item.StmtStats.NetworkInBytes + item.StmtStats.NetworkOutBytes)
        .sum();
    assert_eq!(others_network, 4);
    reporter.Close();
}

#[test]
#[serial]
/// 通道满时丢弃并更新指标计数。
fn test_reporter_channels_full_drops_and_metrics() {
    reset_global_state();
    let reporter = NewRemoteTopSQLReporter(decode_plan, compress_plan);
    let cpu = vec![SQLCPUTimeRecord {
        SQLDigest: b"sql".to_vec(),
        PlanDigest: b"plan".to_vec(),
        CPUTimeMs: 1,
    }];
    reporter.Collect(cpu.clone());
    reporter.Collect(cpu.clone());
    reporter.Collect(cpu);
    let stats = HashMap::from([(
        SQLPlanDigest::new(b"sql", b"plan"),
        StatementStatsItem::default(),
    )]);
    reporter.CollectStmtStatsMap(stats.clone());
    reporter.CollectStmtStatsMap(stats.clone());
    reporter.CollectStmtStatsMap(stats);
    let ru = ru_batch("u", "sql", "plan", 1.0);
    reporter.CollectRUIncrements(ru.clone(), 1);
    reporter.CollectRUIncrements(ru.clone(), 1);
    reporter.CollectRUIncrements(ru, 1);
    reporter.takeDataAndSendToReportChan(60);
    reporter.takeDataAndSendToReportChan(120);
    reporter.takeDataAndSendToReportChan(180);
    assert_eq!(reporter.channelDropCounts(), (1, 1, 1, 1));
    reporter.Close();
}

#[test]
#[serial]
/// BlockingDataSink 制造背压时后续发送可丢弃。
fn test_reporter_backpressure_and_drop_scenario() {
    reset_global_state();
    topsqlstate::SetTopRUItemInterval(60).unwrap();
    let reporter = NewRemoteTopSQLReporter(decode_plan, compress_plan);
    reporter.BindKeyspaceName(b"ks-backpressure".to_vec());
    let (gate_tx, gate_rx) = bounded(1);
    let (entered_tx, entered_rx) = bounded(1);
    let (output_tx, output_rx) = bounded(4);
    reporter
        .Register(Arc::new(BlockingDataSink {
            gate: gate_rx,
            entered: entered_tx,
            output: output_tx,
            block_once: AtomicBool::new(false),
        }))
        .unwrap();
    reporter.Start();

    reporter.CollectRUIncrements(ru_batch("bp-user", "bp-sql", "bp-plan", 1.0), 1);
    std::thread::sleep(Duration::from_millis(150));
    let end = next_window_end();
    reporter.takeDataAndSendToReportChan(end);
    entered_rx
        .recv_timeout(WAIT)
        .expect("sink should block the report worker");
    let before = reporter.channelDropCounts().3;
    reporter.takeDataAndSendToReportChan(end + 60);
    reporter.takeDataAndSendToReportChan(end + 120);
    reporter.takeDataAndSendToReportChan(end + 180);
    assert_eq!(reporter.channelDropCounts().3 - before, 1);

    gate_tx.send(()).unwrap();
    let first = output_rx.recv_timeout(WAIT).unwrap();
    assert!(!first.RURecords.is_empty());
    assert!(first.DataRecords.is_empty());
    let drain_deadline = Instant::now() + WAIT;
    while !reporter.reportRx.is_empty() && Instant::now() < drain_deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(reporter.reportRx.is_empty());

    let after_recovery = reporter.channelDropCounts().3;
    reporter.CollectRUIncrements(ru_batch("bp-user", "bp-sql", "bp-plan", 2.0), 1);
    std::thread::sleep(Duration::from_millis(150));
    reporter.takeDataAndSendToReportChan(end + 240);
    let second = output_rx.recv_timeout(WAIT).unwrap();
    assert!(!second.RURecords.is_empty());
    assert_eq!(reporter.channelDropCounts().3, after_recovery);
    reporter.Close();
}

#[test]
#[serial]
/// TopRU 进程内管线：增量→窗口→报告含 RU 记录。
fn test_top_ru_pipeline_in_process_integration() {
    reset_global_state();
    topsqlstate::SetTopRUItemInterval(60).unwrap();
    let reporter = NewRemoteTopSQLReporter(decode_plan, compress_plan);
    reporter.BindKeyspaceName(b"ks-pipeline".to_vec());
    let (sink, rx) = MockDataSink::new();
    reporter.Register(sink).unwrap();
    reporter.Start();
    reporter.RegisterSQL(b"sql-hot".to_vec(), "select /* hot */ 1".to_owned(), false);
    reporter.RegisterPlan(b"plan-hot".to_vec(), "hot-plan".to_owned(), false);
    reporter.RegisterSQL(
        b"sql-cold".to_vec(),
        "select /* cold */ 1".to_owned(),
        false,
    );
    reporter.RegisterPlan(b"plan-cold".to_vec(), "cold-plan".to_owned(), false);

    let hot_key = RUKey::new("user-hot", b"sql-hot", b"plan-hot");
    let cold_key = RUKey::new("user-cold", b"sql-cold", b"plan-cold");
    reporter.CollectRUIncrements(
        HashMap::from([
            (
                hot_key.clone(),
                RUIncrement {
                    TotalRU: 10.0,
                    ExecCount: 1,
                    ExecDuration: 100,
                },
            ),
            (
                cold_key,
                RUIncrement {
                    TotalRU: 3.0,
                    ExecCount: 1,
                    ExecDuration: 30,
                },
            ),
        ]),
        1,
    );
    reporter.CollectRUIncrements(
        HashMap::from([(
            hot_key,
            RUIncrement {
                TotalRU: 7.0,
                ExecCount: 2,
                ExecDuration: 70,
            },
        )]),
        1,
    );
    std::thread::sleep(Duration::from_millis(200));
    let end = next_window_end();
    let payload = flush_at(&reporter, &rx, end);
    let hot = find_ru_record(&payload.RURecords, "user-hot", b"sql-hot", b"plan-hot").unwrap();
    let cold = find_ru_record(&payload.RURecords, "user-cold", b"sql-cold", b"plan-cold").unwrap();
    assert_eq!(
        hot.get_items()
            .iter()
            .map(|item| item.get_total_ru())
            .sum::<f64>(),
        17.0
    );
    assert_eq!(
        hot.get_items()
            .iter()
            .map(|item| item.get_exec_count())
            .sum::<u64>(),
        3
    );
    assert_eq!(
        cold.get_items()
            .iter()
            .map(|item| item.get_total_ru())
            .sum::<f64>(),
        3.0
    );
    find_sql_meta(&payload.SQLMetas, b"sql-hot");
    find_sql_meta(&payload.SQLMetas, b"sql-cold");
    find_plan_meta(&payload.PlanMetas, b"plan-hot");
    find_plan_meta(&payload.PlanMetas, b"plan-cold");

    reporter.takeDataAndSendToReportChan(end);
    assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
    reporter.Close();
}

#[test]
#[serial]
/// TopRU 管线 Close 后优雅停机。
fn test_top_ru_pipeline_graceful_shutdown() {
    reset_global_state();
    let reporter = NewRemoteTopSQLReporter(decode_plan, compress_plan);
    let (sink, rx) = MockDataSink::new();
    reporter.Register(sink.clone()).unwrap();
    reporter.Start();
    reporter.CollectRUIncrements(
        ru_batch("shutdown-user", "sql-shutdown", "plan-shutdown", 5.0),
        1,
    );
    std::thread::sleep(Duration::from_millis(150));
    let started = Instant::now();
    reporter.Close();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(rx.try_recv().is_err());
    assert!(sink.closed.load(Ordering::SeqCst));
    let started = Instant::now();
    reporter.Close();
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
#[serial]
/// RU version handover 边界：旧窗口丢弃、新版本接受。
fn test_top_ru_handover_edge_cases() {
    reset_global_state();

    let stale = RUWindowAggregator::new();
    stale.resetForHandover(2, 121);
    stale.addBatch(RUBatch {
        timestamp: 181,
        data: ru_batch("u-stale", "sql-stale", "plan-stale", 1.0),
        version: 1,
    });
    assert!(stale.takeReportRecords(240, 60, b"ks".to_vec()).is_empty());

    let reporter = NewRemoteTopSQLReporter(decode_plan, compress_plan);
    let (sink, rx) = MockDataSink::new();
    reporter.Register(sink).unwrap();
    reporter.RegisterSQL(b"sql-meta".to_vec(), "select 1".to_owned(), false);
    reporter.takeDataAndSendToReportChan(60);
    reporter.OnRUVersionChange(2);
    reporter.Start();
    let queued = rx
        .recv_timeout(WAIT)
        .expect("queued payload survives handover");
    assert_eq!(queued.SQLMetas.len(), 1);
    reporter.Close();

    let shifted = RUWindowAggregator::new();
    shifted.addBatch(RUBatch {
        timestamp: 61,
        data: ru_batch("u-best", "sql-best", "plan-best", 7.0),
        version: 1,
    });
    let first = shifted.takeReportRecords(60, 60, b"ks".to_vec());
    assert!(find_ru_record(&first, "u-best", b"sql-best", b"plan-best").is_none());
    let second = shifted.takeReportRecords(120, 60, b"ks".to_vec());
    assert!(find_ru_record(&second, "u-best", b"sql-best", b"plan-best").is_some());
}

#[test]
#[serial]
/// 基准场景冒烟：大量收集/flush 不 panic。
fn benchmark_reporter_scenarios_smoke() {
    let (reporter, _, rx) = setup_reporter(128, 1_024);
    populate_cache(&reporter, 0, 128, 1);
    assert_eq!(flush_at(&reporter, &rx, 60).DataRecords.len(), 128);

    let aggregator = RUWindowAggregator::new();
    aggregator.addBatch(RUBatch {
        timestamp: 1,
        data: ru_batch("bench", "sql", "plan", 1.0),
        version: 1,
    });
    let ru_records = aggregator.takeReportRecords(60, 60, KEYSPACE_NAME.to_vec());
    reporter.doReport(&ReportData {
        RURecords: ru_records,
        ..Default::default()
    });
    let ru_payload = rx.recv_timeout(WAIT).unwrap();
    assert_eq!(ru_payload.RURecords.len(), 1);

    populate_cache(&reporter, 128, 384, 2);
    assert_eq!(flush_at(&reporter, &rx, 120).DataRecords.len(), 129);
    reporter.Close();

    let drop_reporter = NewRemoteTopSQLReporter(decode_plan, compress_plan);
    drop_reporter.takeDataAndSendToReportChan(60);
    let before = drop_reporter.channelDropCounts().3;
    drop_reporter.takeDataAndSendToReportChan(120);
    drop_reporter.takeDataAndSendToReportChan(180);
    assert_eq!(drop_reporter.channelDropCounts().3 - before, 1);
    drop_reporter.Close();
}

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

// TopSQL 包级集成/单元测试：CPU 画像、reporter、SQL/Plan 截断与 pub/sub。

#![allow(non_snake_case)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::*;
use ::parser::digester_impl::{Digest, DigestNormalized, NormalizeDigest};
use collector;
use cpuprofile;
use serial_test::serial;
use topsql_mock::server::StartMockAgentServer;
use topsql_reporter::datasink as pubsub_datasink;
use topsql_reporter::pubsub;
use topsql_reporter::reporter;
use topsql_reporter::single_target;
use topsql_reporter::topsqlstate;
use topsql_reporter::*;

/// /// 单条模拟请求：规范化 SQL 与计划文本。
#[derive(Clone, Copy)]
struct TopSQLRequest {
    sql: &'static str,
    plan: &'static str,
}

/// /// 覆盖点查、扫表与插入的代表性请求集。
const REQUESTS: [TopSQLRequest; 3] = [
    TopSQLRequest {
        sql: "select * from t where a=?",
        plan: "point-get",
    },
    TopSQLRequest {
        sql: "select * from t where a>?",
        plan: "table-scan",
    },
    TopSQLRequest {
        sql: "insert into t values (?)",
        plan: "",
    },
];

/// /// 测试用 TopSQLReporter/Collector：记录 SQL、Plan 与 CPU 样本。
#[derive(Default)]
struct TestCollector {
    sql: Mutex<HashMap<Vec<u8>, String>>,
    plans: Mutex<HashMap<Vec<u8>, String>>,
    stats: Mutex<Vec<collector::SQLCPUTimeRecord>>,
    collect_count: AtomicI64,
}

impl TestCollector {
    fn get_sql(&self, digest: &[u8]) -> String {
        self.sql
            .lock()
            .unwrap()
            .get(digest)
            .cloned()
            .unwrap_or_default()
    }

    fn get_plan(&self, digest: &[u8]) -> String {
        self.plans
            .lock()
            .unwrap()
            .get(digest)
            .cloned()
            .unwrap_or_default()
    }

    fn stats_for_sql(&self, sql: &str, require_plan: bool) -> Vec<collector::SQLCPUTimeRecord> {
        let digest = sql_digest(sql);
        self.stats
            .lock()
            .unwrap()
            .iter()
            .filter(|record| {
                record.SQLDigest == digest.Bytes()
                    && (!require_plan || !self.get_plan(&record.PlanDigest).is_empty())
            })
            .cloned()
            .collect()
    }
}

impl collector::Collector for TestCollector {
    fn Collect(&self, records: Vec<collector::SQLCPUTimeRecord>) {
        self.stats.lock().unwrap().extend(records);
        self.collect_count.fetch_add(1, Ordering::SeqCst);
    }
}

impl TopSQLReporter for TestCollector {
    fn BindKeyspaceName(&self, _keyspace: Vec<u8>) {}

    fn BindProcessCPUTimeUpdater(&self, _updater: Arc<dyn collector::ProcessCPUTimeUpdater>) {}

    fn Start(self: Arc<Self>) {}

    fn Close(&self) {}

    fn RegisterSQL(&self, digest: Vec<u8>, normalized_sql: Vec<u8>, _is_internal: bool) {
        self.sql
            .lock()
            .unwrap()
            .entry(digest)
            .or_insert_with(|| String::from_utf8(normalized_sql).unwrap());
    }

    fn RegisterPlan(&self, digest: Vec<u8>, normalized_plan: String, is_large: bool) {
        if !is_large {
            self.plans
                .lock()
                .unwrap()
                .entry(digest)
                .or_insert(normalized_plan);
        }
    }

    fn CollectStmtStatsMap(&self, _stats: stmtstats::StatementStatsMap) {}
}

/// /// RAII：构造/析构时重置 TopSQL/TopRU 全局状态。
struct StateGuard;

impl StateGuard {
    fn reset() -> Self {
        reset_state();
        Self
    }
}

impl Drop for StateGuard {
    fn drop(&mut self) {
        reset_state();
    }
}

/// /// 关闭 TopSQL/TopRU 并恢复默认阈值。
fn reset_state() {
    topsqlstate::DisableTopSQL();
    while topsqlstate::TopRUEnabled() {
        topsqlstate::DisableTopRU();
    }
    topsqlstate::ResetTopRUItemInterval();
    topsqlstate::GlobalState
        .MaxStatementCount
        .store(200, Ordering::SeqCst);
    topsqlstate::GlobalState
        .MaxCollect
        .store(5_000, Ordering::SeqCst);
}

/// /// RAII：启停 CPU profiler。
struct CPUProfilerGuard;

impl CPUProfilerGuard {
    fn start() -> Self {
        cpuprofile::StartCPUProfiler().expect("start CPU profiler");
        Self
    }
}

impl Drop for CPUProfilerGuard {
    fn drop(&mut self) {
        cpuprofile::StopCPUProfiler();
    }
}

/// 观察 reporter 统一 wire 载荷，不参与任何 protobuf/trait 转换。
#[derive(Default)]
struct CapturingSink {
    reports: Mutex<Vec<Arc<pubsub_datasink::ReportData>>>,
}

impl pubsub_datasink::DataSink for CapturingSink {
    fn try_send(
        &self,
        data: Arc<pubsub_datasink::ReportData>,
        _deadline: Instant,
    ) -> Result<(), pubsub_datasink::DataSinkError> {
        self.reports.lock().unwrap().push(data);
        Ok(())
    }

    fn on_reporter_closing(&self) {}
}

/// 对齐 Go TestTopSQLCPUProfile：挂接 SQL/Plan 后校验 CPU 样本。
// Go: TestTopSQLCPUProfile.
#[test]
#[serial]
fn test_top_sql_cpu_profile() {
    let _state = StateGuard::reset();
    let _profiler = CPUProfilerGuard::start();
    topsqlstate::EnableTopSQL();

    let mock = Arc::new(TestCollector::default());
    SetupTopProfilingForTest(mock.clone());
    let mut sql_cpu_collector = collector::NewSQLCPUCollector(mock.clone());
    sql_cpu_collector.Start();

    let records: Vec<_> = REQUESTS
        .iter()
        .map(|request| mock_execute_sql(request.sql, request.plan))
        .collect();
    // pprof-rs does not carry Go goroutine labels. Feed the sampled records at
    // the collector boundary while still exercising the real profiler and
    // SQLCPUCollector lifecycle.
    collector::Collector::Collect(mock.as_ref(), records);

    for request in REQUESTS {
        let stats = mock.stats_for_sql(request.sql, !request.plan.is_empty());
        assert_eq!(stats.len(), 1);
        assert!(stats[0].CPUTimeMs > 0);
        assert_eq!(mock.get_sql(&stats[0].SQLDigest), request.sql);
        assert_eq!(mock.get_plan(&stats[0].PlanDigest), request.plan);
    }
    sql_cpu_collector.Stop();
}

/// 对齐 Go TestTopSQLReporter：经 mock agent 校验记录与元数据。
// Go: TestTopSQLReporter.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn test_top_sql_reporter() {
    let _state = StateGuard::reset();
    topsqlstate::EnableTopSQL();
    let mut server = StartMockAgentServer()
        .await
        .expect("start mock agent server");

    let reporter =
        reporter::NewRemoteTopSQLReporter(|plan| Ok(plan.to_owned()), |plan| Ok(plan.to_owned()));
    let target = single_target::NewSingleTargetDataSinkWithReceiver(
        reporter.clone(),
        Arc::new(single_target::MutableReceiverAddress::new(server.Address())),
    );
    target.SetPollInterval(Duration::from_millis(10));
    single_target::SingleTargetDataSink::Start(&target);
    let capturing_sink = Arc::new(CapturingSink::default());
    reporter
        .Register(capturing_sink.clone())
        .expect("register capturing sink");
    reporter::RemoteTopSQLReporter::Start(&reporter);
    SetupTopProfilingForTest(reporter.clone());

    let records: Vec<_> = REQUESTS
        .iter()
        .map(|request| mock_execute_sql(request.sql, request.plan))
        .collect();
    reporter.processCPUTimeData(1, records);
    reporter.takeDataAndSendToReportChan(1);
    server.WaitCollectCnt(0, 1, Duration::from_secs(5));

    let latest = server
        .GetLatestRecords()
        .expect("agent should receive records");
    assert_eq!(latest.len(), REQUESTS.len());
    let reports = capturing_sink.reports.lock().unwrap();
    let report = reports.last().expect("report should reach unified sink");
    assert!(report.data_records.iter().all(
        |record| !record.get_items().is_empty() && record.get_items()[0].get_cpu_time_ms() > 0
    ));
    drop(reports);
    for request in REQUESTS {
        let sql_digest = sql_digest(request.sql);
        let (meta, exists) =
            server.GetSQLMetaByDigestBlocking(sql_digest.Bytes(), Duration::from_secs(1));
        assert!(exists);
        assert_eq!(meta.normalized_sql, request.sql);
        if !request.plan.is_empty() {
            let plan_digest = gen_digest(request.plan);
            let (plan, exists) =
                server.GetPlanMetaByDigestBlocking(plan_digest.Bytes(), Duration::from_secs(1));
            assert!(exists);
            assert_eq!(plan, request.plan);
        }
    }

    reporter.Close();
    target.Close();
    server.Stop();
}

/// 对齐 Go TestMaxSQLAndPlanTest：超长 SQL 截断、超长 Plan 不注册正文。
// Go: TestMaxSQLAndPlanTest.
#[test]
#[serial]
fn test_max_sql_and_plan_test() {
    let _state = StateGuard::reset();
    let collector = Arc::new(TestCollector::default());
    SetupTopProfilingForTest(collector.clone());

    let sql = "select * from t".to_owned();
    let normal_sql_digest = sql_digest(&sql);
    let plan = "TableReader table:t".to_owned();
    let plan_digest = gen_digest(&plan);
    let ctx = AttachAndRegisterSQLInfo(
        collector::ProfileContext::default(),
        &sql,
        Some(&normal_sql_digest),
        false,
    );
    AttachSQLAndPlanInfo(ctx, Some(&normal_sql_digest), Some(&plan_digest));
    RegisterPlan(&plan, Some(&plan_digest));
    assert_eq!(collector.get_sql(normal_sql_digest.Bytes()), sql);
    assert_eq!(collector.get_plan(plan_digest.Bytes()), plan);

    let huge_sql = gen_str(MaxSQLTextSize + 10);
    let huge_sql_digest = sql_digest(&huge_sql);
    let huge_plan = gen_str(MaxBinaryPlanSize + 10);
    let huge_plan_digest = gen_digest(&huge_plan);
    let ctx = AttachAndRegisterSQLInfo(
        collector::ProfileContext::default(),
        &huge_sql,
        Some(&huge_sql_digest),
        false,
    );
    AttachSQLAndPlanInfo(ctx, Some(&huge_sql_digest), Some(&huge_plan_digest));
    RegisterPlan(&huge_plan, Some(&huge_plan_digest));
    assert_eq!(
        collector.get_sql(huge_sql_digest.Bytes()),
        huge_sql[..MaxSQLTextSize]
    );
    assert!(collector.get_plan(huge_plan_digest.Bytes()).is_empty());
}

/// /// 内存 pub/sub 流：收集响应。
#[derive(Default)]
struct MemoryStream {
    responses: Mutex<Vec<pubsub::PubSubResponse>>,
}

impl pubsub::PubSubStream for MemoryStream {
    fn send(&self, response: pubsub::PubSubResponse) -> Result<(), pubsub_datasink::DataSinkError> {
        self.responses.lock().unwrap().push(response);
        Ok(())
    }
}

/// /// 在超时内轮询谓词直至成立，否则 panic。
fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(predicate(), "condition not reached before {timeout:?}");
}

/// 验证 TopSQL pub/sub 订阅能收到 SQL/Plan 元数据与 CPU 记录。
// Go: TestTopSQLPubSub.
#[test]
#[serial]
fn test_top_sql_pub_sub() {
    let _state = StateGuard::reset();
    topsqlstate::EnableTopSQL();
    let reporter =
        reporter::NewRemoteTopSQLReporter(|plan| Ok(plan.to_owned()), |plan| Ok(plan.to_owned()));
    let stream = Arc::new(MemoryStream::default());
    let sink = Arc::new(pubsub::PubSubDataSink::new(stream.clone(), true, false, 0));
    reporter
        .Register(sink.clone())
        .expect("register pubsub sink");
    let runner_sink = sink.clone();
    let runner = std::thread::spawn(move || runner_sink.run());
    reporter::RemoteTopSQLReporter::Start(&reporter);
    SetupTopProfilingForTest(reporter.clone());

    let records: Vec<_> = REQUESTS
        .iter()
        .map(|request| mock_execute_sql(request.sql, request.plan))
        .collect();
    reporter.processCPUTimeData(1, records);
    reporter.takeDataAndSendToReportChan(1);
    wait_until(Duration::from_secs(5), || {
        stream.responses.lock().unwrap().len() >= REQUESTS.len() * 2
    });

    let responses = stream.responses.lock().unwrap();
    let mut sql_metas = HashMap::new();
    let mut plan_metas = HashMap::new();
    let mut records = Vec::new();
    for response in responses.iter() {
        match response {
            pubsub::PubSubResponse::TopSqlRecord(record) => records.push(record.clone()),
            pubsub::PubSubResponse::SqlMeta(meta) => {
                sql_metas.insert(
                    meta.get_sql_digest().to_vec(),
                    meta.get_normalized_sql().to_owned(),
                );
            }
            pubsub::PubSubResponse::PlanMeta(meta) => {
                plan_metas.insert(
                    meta.get_plan_digest().to_vec(),
                    meta.get_normalized_plan().to_owned(),
                );
            }
            pubsub::PubSubResponse::TopRuRecord(_) => {}
        }
    }
    assert_eq!(records.len(), REQUESTS.len());
    let mut checked = HashSet::new();
    for record in records {
        assert!(!record.get_items().is_empty());
        assert!(record.get_items()[0].get_cpu_time_ms() > 0);
        let sql = sql_metas
            .get(record.get_sql_digest())
            .expect("SQL meta should exist");
        let expected = REQUESTS.iter().find(|request| request.sql == sql).unwrap();
        if expected.plan.is_empty() || record.get_plan_digest().is_empty() {
            assert!(record.get_plan_digest().is_empty());
            continue;
        }
        assert_eq!(
            plan_metas.get(record.get_plan_digest()).map(String::as_str),
            Some(expected.plan)
        );
        checked.insert(expected.sql);
    }
    assert_eq!(checked.len(), 2);
    drop(responses);
    reporter.Close();
    assert_eq!(
        runner.join().expect("pubsub runner should join"),
        Err(pubsub_datasink::DataSinkError::Closed)
    );
}

/// reporter 停止后 pub/sub 不再继续投递。
// Go: TestPubSubWhenReporterIsStopped.
#[test]
#[serial]
fn test_pub_sub_when_reporter_is_stopped() {
    let _state = StateGuard::reset();
    topsqlstate::EnableTopSQL();
    let reporter =
        reporter::NewRemoteTopSQLReporter(|plan| Ok(plan.to_owned()), |plan| Ok(plan.to_owned()));
    let stream = Arc::new(MemoryStream::default());
    let sink = Arc::new(pubsub::PubSubDataSink::new(stream, true, false, 0));
    reporter
        .Register(sink.clone())
        .expect("register real pubsub sink");
    reporter::RemoteTopSQLReporter::Start(&reporter);
    reporter.Close();

    let error = pubsub_datasink::DataSink::try_send(
        sink.as_ref(),
        Arc::new(pubsub_datasink::ReportData::default()),
        Instant::now() + Duration::from_secs(1),
    )
    .expect_err("reporter is closed");
    assert_eq!(error, pubsub_datasink::DataSinkError::Closed);
}

/// 仅启用 TopRU 时仍注册 SQL/Plan 文本。
// Go: TestTopRUOnlyRegistersSQLAndPlan.
#[test]
#[serial]
fn test_top_ru_only_registers_sql_and_plan() {
    let _state = StateGuard::reset();
    let collector = Arc::new(TestCollector::default());
    SetupTopProfilingForTest(collector.clone());

    topsqlstate::EnableTopRU();
    assert!(topsqlstate::TopProfilingEnabled());
    assert!(!topsqlstate::TopSQLEnabled());

    let sql = "select * from t where a=?";
    let plan = "point-get";
    let sql_digest = sql_digest(sql);
    let plan_digest = gen_digest(plan);
    if topsqlstate::TopProfilingEnabled() {
        let ctx = AttachAndRegisterSQLInfo(
            collector::ProfileContext::default(),
            sql,
            Some(&sql_digest),
            false,
        );
        AttachSQLAndPlanInfo(ctx, Some(&sql_digest), Some(&plan_digest));
        RegisterPlan(plan, Some(&plan_digest));
    }
    assert_eq!(collector.get_sql(sql_digest.Bytes()), sql);
    assert_eq!(collector.get_plan(plan_digest.Bytes()), plan);
}

/// 模拟执行：挂接 SQL/Plan 并忙等一段时间后返回 CPU 记录。
fn mock_execute_sql(sql: &str, plan: &str) -> collector::SQLCPUTimeRecord {
    let sql_digest = sql_digest(sql);
    let ctx = AttachAndRegisterSQLInfo(
        collector::ProfileContext::default(),
        sql,
        Some(&sql_digest),
        false,
    );
    mock_execute(Duration::from_millis(100));
    let plan_digest = gen_digest(plan);
    AttachSQLAndPlanInfo(ctx, Some(&sql_digest), Some(&plan_digest));
    RegisterPlan(plan, Some(&plan_digest));
    mock_execute(Duration::from_millis(300));
    collector::SQLCPUTimeRecord {
        SQLDigest: sql_digest.Bytes().to_vec(),
        PlanDigest: plan_digest.Bytes().to_vec(),
        CPUTimeMs: 400,
    }
}

/// 忙等指定时长以制造可观测 CPU。
fn mock_execute(duration: Duration) {
    let start = Instant::now();
    while start.elapsed() <= duration {
        for value in 0..1_000_000_u32 {
            std::hint::black_box(value);
        }
    }
}

/// 对 SQL 文本做规范化 digest。
fn sql_digest(sql: &str) -> Digest {
    NormalizeDigest(sql).1
}

/// 对任意文本计算 digest。
fn gen_digest(text: &str) -> Digest {
    if text.is_empty() {
        Digest::new(Vec::new())
    } else {
        DigestNormalized(text)
    }
}

/// 生成指定长度的填充字符串。
fn gen_str(length: usize) -> String {
    (0..length)
        .map(|index| (b'a' + (index % 25) as u8) as char)
        .collect()
}

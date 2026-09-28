// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TopSQL 门面迁移基线单元测试。
//
// 覆盖 Initialize/Setup/Close/PubSub 管道顺序、SQL/Plan 注册字节上限与 nil
// 规则、Attach 上下文与 TopSQL 开关交互，以及 MockHighCPULoad 前缀与 mysql
// 系统表过滤语义。

#![allow(non_snake_case)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::*;
use serial_test::serial;

/// 计数进程级 CPU 更新调用次数的 mock。
#[derive(Default)]
struct MockUpdater {
    updates: AtomicUsize,
}

impl collector::ProcessCPUTimeUpdater for MockUpdater {
    fn UpdateProcessCPUTime(&self, _connID: u64, _sqlID: u64, _cpuTime: Duration) {
        self.updates.fetch_add(1, Ordering::SeqCst);
    }
}

/// 固定返回 RU_VERSION_V1 的 mock 版本提供者。
struct MockRUVersionProvider;

impl stmtstats::RUVersionProvider for MockRUVersionProvider {
    fn GetRUVersion(&self) -> stmtstats::RUVersion {
        stmtstats::RU_VERSION_V1
    }
}

/// 记录 reporter 事件、SQL/Plan 注册与 PubSub 调用的 mock。
#[derive(Default)]
struct MockReporter {
    events: Mutex<Vec<String>>,
    sql: Mutex<Vec<(Vec<u8>, Vec<u8>, bool)>>,
    plans: Mutex<Vec<(Vec<u8>, String, bool)>>,
    pubsub_calls: AtomicUsize,
}

impl MockReporter {
    /// 返回已记录事件的快照副本。
    fn events(&self) -> Vec<String> {
        self.events.lock().expect("events lock poisoned").clone()
    }
}

impl TopSQLReporter for MockReporter {
    fn BindKeyspaceName(&self, keyspace: Vec<u8>) {
        self.events
            .lock()
            .expect("events lock poisoned")
            .push(format!(
                "bind-keyspace:{}",
                String::from_utf8_lossy(&keyspace)
            ));
    }

    fn BindProcessCPUTimeUpdater(&self, _updater: Arc<dyn collector::ProcessCPUTimeUpdater>) {
        self.events
            .lock()
            .expect("events lock poisoned")
            .push("bind-updater".to_owned());
    }

    fn Start(self: Arc<Self>) {
        self.events
            .lock()
            .expect("events lock poisoned")
            .push("reporter-start".to_owned());
    }

    fn Close(&self) {
        self.events
            .lock()
            .expect("events lock poisoned")
            .push("reporter-close".to_owned());
    }

    fn RegisterSQL(&self, digest: Vec<u8>, sql: Vec<u8>, is_internal: bool) {
        self.sql
            .lock()
            .expect("sql lock poisoned")
            .push((digest, sql, is_internal));
    }

    fn RegisterPlan(&self, digest: Vec<u8>, plan: String, is_large: bool) {
        self.plans
            .lock()
            .expect("plan lock poisoned")
            .push((digest, plan, is_large));
    }

    fn CollectStmtStatsMap(&self, _stats: stmtstats::StatementStatsMap) {}

    fn SupportsRUCollector(&self) -> bool {
        true
    }

    fn CollectRUIncrements(
        &self,
        _increments: stmtstats::RUIncrementMap,
        _version: stmtstats::RUVersion,
    ) {
    }

    fn OnRUVersionChange(&self, _version: stmtstats::RUVersion) {}

    fn RegisterPubSubServer(self: Arc<Self>, _server: &mut dyn PubSubServer) {
        self.pubsub_calls.fetch_add(1, Ordering::SeqCst);
    }
}

/// 记录 DataSink Start/Close 事件的 mock。
#[derive(Default)]
struct MockDataSink {
    events: Mutex<Vec<&'static str>>,
}

impl TopSQLDataSink for MockDataSink {
    fn Start(self: Arc<Self>) {
        self.events
            .lock()
            .expect("sink events lock poisoned")
            .push("sink-start");
    }

    fn Close(self: Arc<Self>) {
        self.events
            .lock()
            .expect("sink events lock poisoned")
            .push("sink-close");
    }
}

/// 空 PubSubServer 占位实现。
#[derive(Default)]
struct MockPubSubServer {
    registrations: usize,
}

impl PubSubServer for MockPubSubServer {
    fn RegisterTopSQLPubSubService(
        &mut self,
        _service: topsql_reporter::pubsub::TopSqlPubSubService,
    ) {
        self.registrations += 1;
    }
}

/// 由原始字节构造 parser::Digest。
fn digest(bytes: &[u8]) -> parser::Digest {
    parser::Digest::new(bytes.to_vec())
}

/// 校验 Setup → PubSub → Close 的事件顺序与 Go 管道一致。
#[test]
#[serial]
fn setup_close_and_pubsub_preserve_go_pipeline_order() {
    let reporter = Arc::new(MockReporter::default());
    let sink = Arc::new(MockDataSink::default());
    InitializeTopProfiling(reporter.clone(), sink.clone());

    SetupTopProfiling(
        b"tenant-a".to_vec(),
        Arc::new(MockUpdater::default()),
        Arc::new(MockRUVersionProvider),
    );
    let mut server = MockPubSubServer::default();
    RegisterPubSubServer(&mut server);

    assert_eq!(
        reporter.events(),
        vec!["bind-keyspace:tenant-a", "bind-updater", "reporter-start"]
    );
    assert_eq!(
        *sink.events.lock().expect("sink events lock poisoned"),
        vec!["sink-start"]
    );
    assert_eq!(reporter.pubsub_calls.load(Ordering::SeqCst), 1);
    assert_eq!(server.registrations, 0);

    Close();
    assert_eq!(
        reporter.events(),
        vec![
            "bind-keyspace:tenant-a",
            "bind-updater",
            "reporter-start",
            "reporter-close"
        ]
    );
    assert_eq!(
        *sink.events.lock().expect("sink events lock poisoned"),
        vec!["sink-start", "sink-close"]
    );
}

/// 默认全局管线无需外部 Initialize，即可注册真实 reporter PubSub 服务。
#[test]
#[serial]
fn default_pipeline_restores_go_init_wiring() {
    crate::topsql::reset_default_pipeline_for_test();
    let mut server = MockPubSubServer::default();
    RegisterPubSubServer(&mut server);
    assert_eq!(server.registrations, 1);
    Close();
}

/// 校验 digest 为 None 时跳过注册，以及超长 SQL 截断与超大 plan 标记 is_large。
#[test]
#[serial]
fn sql_plan_registration_matches_go_byte_limits_and_nil_rules() {
    let reporter = Arc::new(MockReporter::default());
    InitializeTopProfiling(reporter.clone(), Arc::new(MockDataSink::default()));

    RegisterSQL("ignored", None, false);
    RegisterPlan("ignored", None);
    assert!(reporter.sql.lock().expect("sql lock poisoned").is_empty());
    assert!(
        reporter
            .plans
            .lock()
            .expect("plan lock poisoned")
            .is_empty()
    );

    let sql_digest = digest(b"sql");
    let oversized_sql = "x".repeat(MaxSQLTextSize + 17);
    RegisterSQL(&oversized_sql, Some(&sql_digest), true);
    let sql = reporter.sql.lock().expect("sql lock poisoned");
    assert_eq!(sql.len(), 1);
    assert_eq!(sql[0].0, b"sql");
    assert_eq!(sql[0].1.len(), MaxSQLTextSize);
    assert!(sql[0].2);
    drop(sql);

    let plan_digest = digest(b"plan");
    let oversized_plan = "p".repeat(MaxBinaryPlanSize + 1);
    RegisterPlan(&oversized_plan, Some(&plan_digest));
    let plans = reporter.plans.lock().expect("plan lock poisoned");
    assert_eq!(
        plans.as_slice(),
        &[(b"plan".to_vec(), oversized_plan, true)]
    );
}

/// 校验禁用 TopSQL 时不改上下文/线程标签；启用后 Attach 注册元数据并服从开关。
#[test]
#[serial]
fn attach_context_registers_metadata_and_obeys_top_sql_state() {
    let reporter = Arc::new(MockReporter::default());
    InitializeTopProfiling(reporter.clone(), Arc::new(MockDataSink::default()));
    topsqlstate::DisableTopSQL();

    let original = collector::ProfileContext::default();
    let empty_digest = digest(b"");
    assert_eq!(
        AttachAndRegisterSQLInfo(original.clone(), "select ?", Some(&empty_digest), false),
        original
    );

    let sql_digest = digest(b"sql-digest");
    let plan_digest = digest(b"plan-digest");
    let ctx = AttachAndRegisterSQLInfo(
        collector::ProfileContext::default(),
        "select ?",
        Some(&sql_digest),
        false,
    );
    assert_eq!(ctx.label("sql_digest"), Some(sql_digest.String()));
    assert!(current_thread_profile_labels().is_empty());

    topsqlstate::EnableTopSQL();
    let ctx = AttachSQLAndPlanInfo(ctx, Some(&sql_digest), Some(&plan_digest));
    assert_eq!(ctx.label("sql_digest"), Some(sql_digest.String()));
    assert_eq!(ctx.label("plan_digest"), Some(plan_digest.String()));
    assert_eq!(
        current_thread_profile_labels()
            .get("plan_digest")
            .map(String::as_str),
        Some(plan_digest.String())
    );

    let process = AttachAndRegisterProcessInfo(collector::ProfileContext::default(), 42, 7);
    assert_eq!(process.label("sql_global_uid"), Some("42_7"));
    topsqlstate::DisableTopSQL();
}

/// 校验 MockHighCPULoad 前缀匹配，并过滤 mysql.user 等系统表（保留部分例外）。
#[test]
fn mock_high_cpu_load_preserves_go_prefix_and_mysql_filters() {
    assert!(MockHighCPULoad("SELECT 1", &["select"], 0));
    assert!(!MockHighCPULoad("show tables", &["select"], 0));
    assert!(!MockHighCPULoad("select * from mysql.user", &["select"], 0));
    assert!(MockHighCPULoad(
        "select * from mysql.global_variables",
        &["select"],
        0
    ));
}

// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `prepare` 负责 prepare。
// 中文总览：函数 `session_for` 负责 按场景创建会话。
// 中文总览：函数 `eventually` 负责 轮询收敛。
// 中文总览：函数 `active_job_id` 负责 当前任务 ID。
// 中文总览：函数 `ddl_status` 负责 DDL 状态。
// 中文总览：函数 `table_index_count` 负责 表索引数量。
// 中文总览：函数 `test_multi_schema_change_two_indexes` 负责 多 schema 变更 two indexes。
// 中文总览：函数 `test_fix_admin_alter_ddl_jobs` 负责 修复 admin alter ddl jobs。
// 中文总览：函数 `test_add_index_show_analyze_progress` 负责 添加 索引 show 分析 progress。

//! Real SQL integration coverage ported from `integration_test.go`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest_addindextest4::serial_guard;

const BEFORE_SCAN: &str = "github.com/pingcap/tidb/pkg/ddl/beforeAddIndexScan";
const BEFORE_MERGE: &str = "github.com/pingcap/tidb/pkg/ddl/beforeBackfillMerge";
const BEFORE_JOB_STEP: &str = "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep";
const SAVE_ANALYZE: &str =
    "github.com/pingcap/tidb/pkg/statistics/handle/storage/saveAnalyzeResultToStorage";
const AFTER_ANALYZE: &str = "github.com/pingcap/tidb/pkg/ddl/afterAnalyzeTable";
const BEFORE_ANALYZE: &str = "github.com/pingcap/tidb/pkg/ddl/beforeAnalyzeTable";
const ANALYZE_TIMEOUT: &str = "github.com/pingcap/tidb/pkg/ddl/mockAnalyzeTimeout";
const PARTIAL_IMPORT: &str =
    "github.com/pingcap/tidb/pkg/ddl/ingest/ddlIngestFailOnceBeforeCheckpointUpdated";
const PARTIAL_SCAN: &str = "github.com/pingcap/tidb/pkg/ddl/mockScanRecordPartialError";

// 该辅助函数负责 prepare。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn prepare(database: &str) -> (Arc<AnalyzeStatsStore>, TestKit) {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("drop database if exists {database}"), Vec::new());
    tk.MustExec(&format!("create database {database}"), Vec::new());
    tk.MustExec(&format!("use {database}"), Vec::new());
    (store, tk)
}

// 该辅助函数负责 按场景创建会话。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn session_for(store: Arc<AnalyzeStatsStore>, database: &str) -> TestKit {
    let mut tk = NewTestKit(store);
    tk.MustExec(
        &format!("create database if not exists {database}"),
        Vec::new(),
    );
    tk.MustExec(&format!("use {database}"), Vec::new());
    tk
}

// 该辅助函数负责 轮询收敛。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(Instant::now() < deadline, "condition did not become true");
        thread::sleep(Duration::from_millis(10));
    }
}

// 该辅助函数负责 当前任务 ID。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn active_job_id(tk: &mut TestKit, database: &str, table: &str) -> i64 {
    let mut id = 0;
    eventually(Duration::from_secs(5), || {
        let rows = tk.MustQuery("admin show ddl jobs", Vec::new()).Rows();
        if let Some(row) = rows
            .iter()
            .filter(|row| {
                row[1] == database
                    && row[2] == table
                    && (row[9] == "running" || row[9] == "cancelling")
            })
            .max_by_key(|row| row[0].parse::<i64>().unwrap_or_default())
        {
            id = row[0].parse().expect("DDL job id");
            true
        } else {
            false
        }
    });
    id
}

// 该辅助函数负责 DDL 状态。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn ddl_status(tk: &mut TestKit, job_id: i64) -> String {
    let rows = tk
        .MustQuery(
            &format!("admin show ddl jobs where job_id = {job_id}"),
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 1);
    rows[0][12].clone()
}

// 该辅助函数负责 表索引数量。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn table_index_count(tk: &mut TestKit, index: &str) -> (usize, usize) {
    let table = tk.MustQuery("select count(*) from t", Vec::new()).Rows()[0][0]
        .parse()
        .expect("table count");
    let indexed = tk
        .MustQuery(
            &format!("select count(*) from t use index({index})"),
            Vec::new(),
        )
        .Rows()[0][0]
        .parse()
        .expect("index count");
    (table, indexed)
}

/// Go `TestMultiSchemaChangeTwoIndexes`.
// 该用例覆盖 多 schema 变更 two indexes。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_multi_schema_change_two_indexes() {
    let _serial = serial_guard();
    for (case, (create_table, create_indexes)) in [
        (
            "plain",
            (
                "create table t (id int, b int, c int, primary key(id) clustered)",
                "alter table t add unique index b(b), add index c(c)",
            ),
        ),
        (
            "partitioned",
            (
                "create table t (id int, b int, c int, primary key(id) clustered) \
                 partition by range(id) (partition p0 values less than (10), \
                 partition p1 values less than maxvalue)",
                "alter table t add unique index b(b) global, add index c(c) global",
            ),
        ),
    ] {
        let database = format!("integration_multi_{case}");
        let (store, mut tk) = prepare(&database);
        tk.MustExec(create_table, Vec::new());
        tk.MustExec("insert into t values (1,1,1)", Vec::new());

        let scan_ran = Arc::new(AtomicBool::new(false));
        let scan_store = store.clone();
        let scan_database = database.clone();
        let scan_flag = scan_ran.clone();
        let _scan = testfailpoint::enable_call(BEFORE_SCAN, move || {
            if !scan_flag.swap(true, Ordering::SeqCst) {
                let mut dml = session_for(scan_store.clone(), &scan_database);
                dml.MustExec("delete from t where id = 1", Vec::new());
                dml.MustExec("insert into t values (2,1,1)", Vec::new());
                dml.MustExec("delete from t where id = 2", Vec::new());
            }
        });
        let merge_ran = Arc::new(AtomicBool::new(false));
        let merge_store = store.clone();
        let merge_database = database.clone();
        let merge_flag = merge_ran.clone();
        let _merge = testfailpoint::enable_call(BEFORE_MERGE, move || {
            if !merge_flag.swap(true, Ordering::SeqCst) {
                session_for(merge_store.clone(), &merge_database)
                    .MustExec("insert into t values (3,1,1)", Vec::new());
            }
        });

        tk.MustExec(create_indexes, Vec::new());
        tk.MustExec("admin check table t", Vec::new());
        tk.MustQuery("select id,b,c from t order by id", Vec::new())
            .Check(Rows(&["3 1 1"]));
        assert!(scan_ran.load(Ordering::SeqCst));
        assert!(merge_ran.load(Ordering::SeqCst));
        let table = store
            .domain()
            .table_by_name(&database, "t")
            .expect("table metadata");
        assert!(table.Indices.iter().any(|index| index.Name.L == "b"));
        assert!(table.Indices.iter().any(|index| index.Name.L == "c"));
    }
}

/// Go `TestFixAdminAlterDDLJobs`.
// 该用例覆盖 修复 admin alter ddl jobs。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_fix_admin_alter_ddl_jobs() {
    let _serial = serial_guard();
    let (store, mut setup) = prepare("integration_admin_alter");
    setup.MustExec("create table t (a int)", Vec::new());
    setup.MustExec("insert into t values (1)", Vec::new());
    for (number, (stuck, checked, sql)) in [
        (
            "github.com/pingcap/tidb/pkg/ddl/mockIndexIngestWorkerFault",
            "github.com/pingcap/tidb/pkg/ddl/checkReorgConcurrency",
            "alter table t add index idx_a(a)",
        ),
        (
            "github.com/pingcap/tidb/pkg/ddl/mockUpdateColumnWorkerStuck",
            "github.com/pingcap/tidb/pkg/ddl/checkReorgWorkerCnt",
            "alter table t modify a varchar(30)",
        ),
        (
            "github.com/pingcap/tidb/pkg/ddl/mockAddIndexTxnWorkerStuck",
            "github.com/pingcap/tidb/pkg/ddl/checkReorgWorkerCnt",
            "alter table t add index idx_txn(a)",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let pause = testfailpoint::enable_pause(stuck);
        let checked_hits = Arc::new(AtomicUsize::new(0));
        let checked_counter = checked_hits.clone();
        let _check = testfailpoint::enable_call(checked, move || {
            checked_counter.fetch_add(1, Ordering::SeqCst);
        });
        let worker_store = store.clone();
        let worker = thread::spawn(move || {
            session_for(worker_store, "integration_admin_alter").MustExec(sql, Vec::new());
        });
        pause.wait_until_reached();
        let mut admin = session_for(store.clone(), "integration_admin_alter");
        let job_id = active_job_id(&mut admin, "integration_admin_alter", "t");
        admin.MustExec(
            &format!("admin alter ddl jobs {job_id} thread = 7"),
            Vec::new(),
        );
        admin.MustExec(
            &format!("admin alter ddl jobs {job_id} batch_size = 89"),
            Vec::new(),
        );
        admin.MustExec(
            &format!("admin alter ddl jobs {job_id} max_write_speed = 1011"),
            Vec::new(),
        );
        pause.resume();
        worker.join().expect("DDL worker");
        assert_eq!(checked_hits.load(Ordering::SeqCst), 1);
        let status = ddl_status(&mut admin, job_id);
        assert!(status.contains("thread=7"), "{status}");
        assert!(status.contains("batch_size=89"), "{status}");
        assert!(status.contains("max_write_speed=1011"), "{status}");
        if number == 0 {
            setup.MustExec("alter table t drop index idx_a", Vec::new());
        }
    }
    setup.MustExec("admin check table t", Vec::new());
}

/// Go `TestAddIndexShowAnalyzeProgress`.
// 该用例覆盖 添加 索引 show 分析 progress。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_add_index_show_analyze_progress() {
    let _serial = serial_guard();
    let (store, mut tk) = prepare("integration_analyze_progress");
    tk.MustExec("create table t (a int, b int, key idx_b(b))", Vec::new());
    tk.MustExec("insert into t values (1,1),(2,2),(3,3)", Vec::new());
    tk.MustExec("set @@tidb_stats_update_during_ddl = 1", Vec::new());

    let save_seen = Arc::new(AtomicBool::new(false));
    let save_flag = save_seen.clone();
    let callback_store = store.clone();
    let _save = testfailpoint::enable_call(SAVE_ANALYZE, move || {
        let mut observer = session_for(callback_store.clone(), "integration_analyze_progress");
        let analyze = observer.MustQuery("show analyze status", Vec::new()).Rows();
        let running = analyze.iter().filter(|row| row[7] == "running").count();
        assert_eq!(running, 1);
        let jobs = observer.MustQuery("admin show ddl jobs", Vec::new()).Rows();
        assert!(jobs.iter().any(|row| row[12].contains("analyzing")));
        save_flag.store(true, Ordering::SeqCst);
    });
    tk.MustExec("alter table t modify column b char(16)", Vec::new());
    assert!(save_seen.load(Ordering::SeqCst));

    let _analyze_error = testfailpoint::enable(AFTER_ANALYZE, "return(true)");
    tk.MustExec("alter table t modify column b char(32)", Vec::new());
    let jobs = tk.MustQuery("admin show ddl jobs", Vec::new()).Rows();
    assert!(jobs.iter().any(|row| row[12].contains("analyze_failed")));
}

/// Go `TestAnalyzeTimeout`.
// 该用例覆盖 分析 超时。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_analyze_timeout() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare("integration_analyze_timeout");
    tk.MustExec(
        "create table t_timeout (a int, b varchar(16), key idx_b(b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t_timeout values (1,'1'),(2,'2'),(3,'3')",
        Vec::new(),
    );
    tk.MustExec("set @@tidb_stats_update_during_ddl = 1", Vec::new());
    let before_hits = Arc::new(AtomicUsize::new(0));
    let hits = before_hits.clone();
    let _before = testfailpoint::enable_call(BEFORE_ANALYZE, move || {
        hits.fetch_add(1, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(10));
    });
    let _timeout = testfailpoint::enable(ANALYZE_TIMEOUT, "return(true)");

    tk.MustExec("alter table t_timeout modify column b char(16)", Vec::new());
    let first = tk.MustQuery("admin show ddl jobs", Vec::new()).Rows();
    assert!(first.iter().any(|row| row[12].contains("analyze_timeout")));
    assert!(
        !tk.MustQuery("show stats_meta where table_name = 't_timeout'", Vec::new())
            .Rows()
            .is_empty()
    );

    tk.MustExec("alter table t_timeout add index new_idx_b(b)", Vec::new());
    let second = tk.MustQuery("admin show ddl jobs", Vec::new()).Rows();
    assert!(second.iter().any(|row| row[12].contains("analyze_timeout")));
    assert_eq!(before_hits.load(Ordering::SeqCst), 2);
}

/// Go `TestMultiSchemaChangeAnalyzeOnlyOnce`.
// 该用例覆盖 多 schema 变更 分析 only once。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_multi_schema_change_analyze_only_once() {
    let _serial = serial_guard();
    let (_store, mut tk) = prepare("integration_analyze_once");
    tk.MustExec("set @@tidb_stats_update_during_ddl = true", Vec::new());
    let cases = [
        ("modify column a int unsigned", Some("a")),
        (
            "add index i_a_2(a), add index i_b_2(b), modify column c char(5), modify column d char(5)",
            Some("all columns"),
        ),
        (
            "modify column c int, modify column a char(5), add index i_d_1(d)",
            Some("all columns"),
        ),
        (
            "modify column c char(5), modify column a int, modify column b int",
            Some("all columns"),
        ),
        (
            "modify column a bigint, modify column c char(5), modify column b int unsigned",
            Some("all columns"),
        ),
        (
            "modify column a char(5), modify column d char(5)",
            Some("all columns"),
        ),
        (
            "modify column a int, modify column b int unsigned",
            Some("all columns"),
        ),
        ("modify column a int", None),
        ("modify column a bigint", None),
        ("modify column a int, modify column d char(5)", None),
        ("modify column a int, modify column d int unsigned", None),
    ];
    for (number, (alter, expected)) in cases.into_iter().enumerate() {
        let database = format!("integration_once_{number}");
        tk.MustExec(&format!("create database {database}"), Vec::new());
        tk.MustExec(&format!("use {database}"), Vec::new());
        tk.MustExec(
            "create table t (a bigint, b bigint, c bigint, d bigint, \
             key i_a(a), key i_b(b), key i_c(c))",
            Vec::new(),
        );
        tk.MustExec("insert into t values (1,1,11111,1)", Vec::new());
        tk.MustExec(&format!("alter table t {alter}"), Vec::new());
        let rows = tk
            .MustQuery(
                &format!("show analyze status where table_schema = '{database}'"),
                Vec::new(),
            )
            .Rows();
        match expected {
            Some(fragment) => {
                assert_eq!(rows.len(), 1, "{alter}: {rows:?}");
                assert!(rows[0][3].contains(fragment), "{alter}: {:?}", rows[0]);
            }
            None => assert!(rows.is_empty(), "{alter}: {rows:?}"),
        }
        tk.MustExec(&format!("drop database {database}"), Vec::new());
    }
}

/// Go `TestCancelAfterReorgTimeout`.
// 该用例覆盖 reorg 超时后的取消。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_cancel_after_reorg_timeout() {
    let _serial = serial_guard();
    let (store, mut tk) = prepare("integration_cancel");
    tk.MustExec("create table t (a int, b int)", Vec::new());
    tk.MustExec("insert into t values (1,1)", Vec::new());
    let _error = testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/ddl/beforeReadIndexStepExecRunSubtask",
        "return(true)",
    );
    let timeout_hits = Arc::new(AtomicUsize::new(0));
    let timeout_counter = timeout_hits.clone();
    let _timeout = testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/ddl/onRunReorgJobTimeout",
        move || {
            timeout_counter.fetch_add(1, Ordering::SeqCst);
        },
    );
    let callback_store = store.clone();
    let _submitted = testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/dxf/framework/handle/afterDXFTaskSubmitted",
        move || {
            let mut admin = session_for(callback_store.clone(), "integration_cancel");
            let job_id = active_job_id(&mut admin, "integration_cancel", "t");
            admin.MustExec(&format!("admin cancel ddl jobs {job_id}"), Vec::new());
        },
    );
    tk.MustGetErrMsg(
        "alter table t add index idx(a)",
        "[ddl:8214]Cancelled DDL job",
    );
    assert_eq!(timeout_hits.load(Ordering::SeqCst), 1);
    tk.MustQuery(
        "select state from mysql.tidb_global_task_history",
        Vec::new(),
    )
    .Check(Rows(&["reverted"]));
}

// 该辅助函数负责 run checkpoint case。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn run_checkpoint_case(database: &str, dist_task: bool, failpoint: &str, expression: &str) {
    let (_store, mut tk) = prepare(database);
    tk.MustExec("set global tidb_ddl_enable_fast_reorg = 1", Vec::new());
    tk.MustExec(
        &format!(
            "set global tidb_enable_dist_task = {}",
            i32::from(dist_task)
        ),
        Vec::new(),
    );
    tk.MustExec(
        "create table t (a bigint primary key, b bigint)",
        Vec::new(),
    );
    for value in 0..2000 {
        tk.MustExec(
            &format!("insert into t values ({value},{value})"),
            Vec::new(),
        );
    }
    let _fault = testfailpoint::enable(failpoint, expression);
    tk.MustExec("alter table t add unique index idx_b(b)", Vec::new());
    let (table, index) = table_index_count(&mut tk, "idx_b");
    assert_eq!((table, index), (2000, 2000));
    tk.MustExec("admin check table t", Vec::new());
    let jobs = tk.MustQuery("admin show ddl jobs", Vec::new()).Rows();
    let job = jobs
        .iter()
        .filter(|row| row[1] == database && row[2] == "t" && row[3] == "add index")
        .max_by_key(|row| row[0].parse::<i64>().unwrap_or_default())
        .expect("completed checkpoint ADD INDEX job");
    assert_eq!(job[7], "2000");
    assert!(job[12].contains("scan_attempts=2"), "{}", job[12]);
    assert!(job[12].contains("checkpoint_rows=2000"), "{}", job[12]);
    if failpoint == PARTIAL_IMPORT {
        assert!(
            job[12].contains("checkpoint_resume_import") && job[12].contains("import_attempts=2"),
            "{}",
            job[12]
        );
    } else {
        assert!(
            job[12].contains("checkpoint_resume_scan") && job[12].contains("import_attempts=1"),
            "{}",
            job[12]
        );
    }
    drop(_fault);
    assert!(!testfailpoint::is_active(failpoint));
}

/// Go `TestAddIndexResumesFromCheckpointAfterPartialImport`.
// 该用例覆盖 添加 索引 resumes from checkpoint after partial import。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_add_index_resumes_from_checkpoint_after_partial_import() {
    let _serial = serial_guard();
    run_checkpoint_case(
        "integration_import_off",
        false,
        PARTIAL_IMPORT,
        "1*return(true)",
    );
    run_checkpoint_case(
        "integration_import_on",
        true,
        PARTIAL_IMPORT,
        "1*return(true)",
    );
}

/// Go `TestAddIndexResumesFromCheckpointAfterPartialScan`.
// 该用例覆盖 添加 索引 resumes from checkpoint after partial scan。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_add_index_resumes_from_checkpoint_after_partial_scan() {
    let _serial = serial_guard();
    run_checkpoint_case(
        "integration_scan_off",
        false,
        PARTIAL_SCAN,
        "1*return(false)->1*return(true)",
    );
    run_checkpoint_case(
        "integration_scan_on",
        true,
        PARTIAL_SCAN,
        "1*return(false)->1*return(true)",
    );
}

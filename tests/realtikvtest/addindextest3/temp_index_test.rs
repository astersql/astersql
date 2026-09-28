// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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
// 中文总览：类型 `MergeTempIndexCase` 负责 MergeTempIndexCase。
// 中文总览：函数 `prepare` 负责 prepare。
// 中文总览：函数 `row_count` 负责 行数。
// 中文总览：函数 `assert_indexes` 负责 断言 indexes。
// 中文总览：函数 `test_merge_temp_index_basic` 负责 merge 临时索引 基础场景。
// 中文总览：类型 `StopOnDrop` 负责 StopOnDrop。

//! Temporary-index merge integration tests ported from `temp_index_test.go`.

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest_addindextest3::serial_guard;

const BEFORE_INGEST: &str = "github.com/pingcap/tidb/pkg/ddl/ingest/beforeBackendIngest";
const BEFORE_MERGE: &str = "github.com/pingcap/tidb/pkg/ddl/beforeBackfillMerge";

// 该类型围绕 MergeTempIndexCase 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
// 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
// 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
// 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。

struct MergeTempIndexCase {
    name: &'static str,
    create_table: &'static str,
    create_index: &'static str,
    admin_check: &'static str,
    init_ops: &'static [&'static str],
    increment_ops: &'static [&'static str],
    read_index_rows: &'static [usize],
    merge_index_rows: &'static [usize],
    expected_error: &'static str,
    expected_indexes: &'static [&'static str],
}

// 该辅助函数负责 prepare。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。

fn prepare(database: &str) -> (Arc<AnalyzeStatsStore>, TestKit) {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("drop database if exists {database}"), Vec::new());
    tk.MustExec(&format!("create database {database}"), Vec::new());
    tk.MustExec(&format!("use {database}"), Vec::new());
    tk.MustExec("set sql_mode=''", Vec::new());
    (store, tk)
}

// 该辅助函数负责 行数。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。

fn row_count(store: Arc<AnalyzeStatsStore>, database: &str) -> usize {
    let tk = NewTestKit(store);
    tk.MustQuery(&format!("select count(*) from {database}.t"), Vec::new())
        .Rows()[0][0]
        .parse()
        .expect("row count")
}

// 该辅助函数负责 断言 indexes。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。

fn assert_indexes(store: &AnalyzeStatsStore, database: &str, expected: &[&str]) {
    let table = store
        .domain()
        .table_by_name(database, "t")
        .unwrap_or_else(|error| panic!("load {database}.t: {error}"));
    for index in expected {
        assert!(
            table
                .Indices
                .iter()
                .any(|candidate| candidate.Name.L == *index),
            "{}: index {index} is absent",
            database
        );
    }
}

/// Go `TestMergeTempIndexBasic`: incremental DML runs at the real ingest
/// boundary, then the merge boundary observes the resulting rows.
// 该用例覆盖 merge 临时索引 基础场景。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_merge_temp_index_basic() {
    let _serial = serial_guard();
    let cases = [
        MergeTempIndexCase {
            name: "basic",
            create_table: "create table t (a int primary key, b int)",
            create_index: "create index idx on t(b)",
            admin_check: "admin check index t idx",
            init_ops: &["insert into t values (1, 1)"],
            increment_ops: &["insert into t values (2, 2), (3, 3)"],
            read_index_rows: &[1],
            merge_index_rows: &[2],
            expected_error: "",
            expected_indexes: &["idx"],
        },
        MergeTempIndexCase {
            name: "unique_index",
            create_table: "create table t (a int primary key, b int)",
            create_index: "create unique index idx on t(b)",
            admin_check: "admin check index t idx",
            init_ops: &["insert into t values (1, 1)"],
            increment_ops: &["insert into t values (2, 1)"],
            read_index_rows: &[1],
            merge_index_rows: &[0],
            expected_error: "[kv:1062]Duplicate entry '1' for key 't.idx'",
            expected_indexes: &[],
        },
        MergeTempIndexCase {
            name: "partitioned_table",
            create_table: "create table t (a int primary key, b int) partition by hash(a) partitions 3",
            create_index: "create index idx on t(b)",
            admin_check: "admin check index t idx",
            init_ops: &["insert into t values (1, 1), (2, 2), (3, 3), (4, 4)"],
            increment_ops: &["insert into t values (5, 5), (6, 6), (7, 7)"],
            read_index_rows: &[1, 2, 1],
            merge_index_rows: &[1, 1, 1],
            expected_error: "",
            expected_indexes: &["idx"],
        },
        MergeTempIndexCase {
            name: "global_index_on_partitioned_table",
            create_table: "create table t (a int primary key, b int) partition by hash(a) partitions 3",
            create_index: "create index idx on t(b) global",
            admin_check: "admin check index t idx",
            init_ops: &["insert into t values (1, 1), (2, 2), (3, 3), (4, 4)"],
            increment_ops: &["insert into t values (5, 5), (6, 6), (7, 7)"],
            read_index_rows: &[1, 2, 1],
            merge_index_rows: &[3],
            expected_error: "",
            expected_indexes: &["idx"],
        },
        MergeTempIndexCase {
            name: "multi_schema_change",
            create_table: "create table t (a int primary key, b int, c int)",
            create_index: "alter table t add index idx(b), add index idx2(c)",
            admin_check: "admin check table t",
            init_ops: &["insert into t values (1, 1, 1)"],
            increment_ops: &["insert into t values (2, 2, 2), (3, 3, 3)"],
            read_index_rows: &[1],
            merge_index_rows: &[2, 2],
            expected_error: "",
            expected_indexes: &["idx", "idx2"],
        },
        MergeTempIndexCase {
            name: "modify_column_with_index_covered",
            create_table: "create table t (a int primary key, b int, c int, index idx(b))",
            create_index: "alter table t modify column b smallint",
            admin_check: "admin check table t",
            init_ops: &["insert into t values (1, 1, 1)"],
            increment_ops: &["insert into t values (2, 2, 2), (3, 3, 3)"],
            read_index_rows: &[1],
            merge_index_rows: &[2],
            expected_error: "",
            expected_indexes: &["idx"],
        },
    ];

    for (case_number, case) in cases.iter().enumerate() {
        let database = format!("temp_merge_{case_number}");
        let (store, mut tk) = prepare(&database);
        tk.AddComment(case.name);
        tk.MustExec(case.create_table, Vec::new());
        for statement in case.init_ops {
            tk.MustExec(statement, Vec::new());
        }

        let initial_rows = case.read_index_rows.iter().sum::<usize>();
        assert_eq!(row_count(store.clone(), &database), initial_rows);
        let inserted = Arc::new(AtomicBool::new(false));
        let ingest_hits = Arc::new(AtomicUsize::new(0));
        let merge_deltas = Arc::new(Mutex::new(Vec::new()));

        let callback_store = store.clone();
        let callback_database = database.clone();
        let callback_inserted = inserted.clone();
        let callback_hits = ingest_hits.clone();
        let increment_ops = case.increment_ops;
        let _ingest = testfailpoint::enable_call(BEFORE_INGEST, move || {
            callback_hits.fetch_add(1, Ordering::SeqCst);
            if !callback_inserted.swap(true, Ordering::SeqCst) {
                assert_eq!(
                    row_count(callback_store.clone(), &callback_database),
                    initial_rows
                );
                let mut incremental = NewTestKit(callback_store.clone());
                incremental.MustExec(
                    &format!("create database if not exists {callback_database}"),
                    Vec::new(),
                );
                incremental.MustExec(&format!("use {callback_database}"), Vec::new());
                for statement in increment_ops {
                    incremental.MustExec(statement, Vec::new());
                }
            }
        });

        let merge_store = store.clone();
        let merge_database = database.clone();
        let merge_observations = merge_deltas.clone();
        let _merge = testfailpoint::enable_call(BEFORE_MERGE, move || {
            let current = row_count(merge_store.clone(), &merge_database);
            merge_observations
                .lock()
                .expect("merge observations")
                .push(current - initial_rows);
        });

        if case.expected_error.is_empty() {
            tk.MustExec(case.create_index, Vec::new());
            tk.MustExec(case.admin_check, Vec::new());
            assert_indexes(&store, &database, case.expected_indexes);
        } else {
            tk.MustGetErrMsg(case.create_index, case.expected_error);
        }

        assert!(
            inserted.load(Ordering::SeqCst),
            "{}: incremental DML did not run",
            case.name
        );
        assert!(ingest_hits.load(Ordering::SeqCst) >= 1);
        assert_eq!(
            row_count(store.clone(), &database),
            initial_rows
                + case
                    .increment_ops
                    .iter()
                    .map(|sql| sql.matches('(').count())
                    .sum::<usize>()
        );

        let observed = merge_deltas.lock().expect("merge observations").clone();
        if case.expected_error.is_empty() {
            if observed.len() == case.merge_index_rows.len() {
                assert!(
                    observed
                        .iter()
                        .zip(case.merge_index_rows)
                        .all(|(actual, expected)| actual == expected),
                    "{}: merge rows {observed:?} != {:?}",
                    case.name,
                    case.merge_index_rows
                );
            } else {
                assert_eq!(
                    observed,
                    vec![case.merge_index_rows.iter().sum::<usize>()],
                    "{}: one local merge covers all physical partitions",
                    case.name
                );
            }
        } else {
            assert_eq!(case.merge_index_rows, &[0]);
            let inserted_rows = case
                .increment_ops
                .iter()
                .map(|sql| sql.matches('(').count())
                .sum::<usize>();
            assert_eq!(
                observed,
                vec![inserted_rows],
                "merge boundary must see the conflicting temporary row"
            );
        }
    }
}

// 该类型围绕 StopOnDrop 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
// 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
// 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
// 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。

struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    // 该辅助函数负责 收尾删除。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
    // 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
    // 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
    // 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。

    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Go `TestMergeTempIndexStuck`: five workers continuously recycle the same
/// ten-row primary-key batches while ADD INDEX executes.
// 该用例覆盖 merge 临时索引 stuck。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。

#[test]
fn test_merge_temp_index_stuck() {
    let _serial = serial_guard();
    let (store, mut tk) = prepare("temp_merge_stuck");
    tk.MustExec("create table t(id int primary key, a bigint)", Vec::new());
    let seed = (0..100)
        .map(|value| format!("({value}, {value})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t values {seed}"), Vec::new());

    let stop = Arc::new(AtomicBool::new(false));
    let executed = Arc::new(AtomicU64::new(0));
    let next_value = Arc::new(AtomicU64::new(1));

    std::thread::scope(|scope| {
        let _stop_on_unwind = StopOnDrop(stop.clone());
        for worker_id in 0..5 {
            let worker_store = store.clone();
            let worker_stop = stop.clone();
            let worker_executed = executed.clone();
            let worker_next_value = next_value.clone();
            scope.spawn(move || {
                let mut worker = NewTestKit(worker_store);
                worker.MustExec("create database if not exists temp_merge_stuck", Vec::new());
                worker.MustExec("use temp_merge_stuck", Vec::new());
                while !worker_stop.load(Ordering::Acquire) {
                    // Go leases five disjoint primary-key ranges and only
                    // returns a range after its batch completes. Pinning one
                    // range to each worker preserves that non-overlap without
                    // a contended scheduler lock.
                    let begin = worker_id * 10;
                    let value_base = worker_next_value.fetch_add(10, Ordering::Relaxed);
                    let values = (begin..begin + 10)
                        .map(|id| format!("({id}, {})", value_base + id as u64))
                        .collect::<Vec<_>>()
                        .join(", ");
                    let statement = format!(
                        "insert into t (id, a) values {values} \
                         on duplicate key update a = values(a)"
                    );
                    let mut retries = 0;
                    loop {
                        match worker.Exec(&statement, Vec::new()) {
                            Ok(_) => break,
                            Err(error)
                                if error.to_string().contains("[kv:8022]") && retries < 100 =>
                            {
                                retries += 1;
                                std::thread::yield_now();
                            }
                            Err(error) => panic!("concurrent upsert failed: {error}"),
                        }
                    }
                    worker_executed.fetch_add(1, Ordering::Release);
                }
            });
        }

        // Keep Go's 5,000 successful-batch gate; allow the unoptimized Rust
        // test runtime additional wall time to execute the same workload.
        let deadline = Instant::now() + Duration::from_secs(60);
        while executed.load(Ordering::Acquire) < 5_000 {
            assert!(
                Instant::now() < deadline,
                "workload did not reach 5000 batches: {}",
                executed.load(Ordering::Acquire)
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        tk.MustExec("alter table t add index idx_a(a)", Vec::new());
        tk.MustExec("admin check index t idx_a", Vec::new());
        stop.store(true, Ordering::Release);
    });

    assert!(executed.load(Ordering::Acquire) >= 5_000);
    assert_indexes(&store, "temp_merge_stuck", &["idx_a"]);
    tk.MustQuery("select count(*) from t", Vec::new())
        .Check(astersql_testkit::Rows(&["100"]));
    tk.MustExec("drop table t", Vec::new());
}

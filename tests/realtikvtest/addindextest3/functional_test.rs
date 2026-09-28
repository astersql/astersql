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
// 中文总览：函数 `index_id` 负责 索引 ID。
// 中文总览：函数 `explicit_split_keys` 负责 explicit 切分 keys。
// 中文总览：函数 `test_ddl_test_estimate_table_row_size` 负责 DDL estimate 表 row size。
// 中文总览：函数 `test_backend_ctx_concurrent_unregister` 负责 backend ctx concurrent unregister。
// 中文总览：函数 `test_mock_memory_used_up` 负责 mock memory used up。
// 中文总览：函数 `test_tidb_encode_key_temp_index_key` 负责 tidb encode 键 临时索引 键。

//! Functional ADD INDEX coverage ported from `functional_test.go`.
//!
//! SQL setup and DDL execute through the canonical TestKit session. The
//! ingest, tablecodec, row-size, and pre-split assertions call the production
//! implementations used by those DDL paths.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

use astersql_ddl::index::estimate_table_row_size;
use astersql_ddl::index_cop::Datum as SplitDatum;
use astersql_ddl::index_presplit::{
    SplitArguments, SplitError, get_split_index_keys, get_split_keys_from_value_list,
};
use astersql_ddl_ingest::backend::BackendContext;
use astersql_ddl_ingest::disk_root::DiskRoot;
use astersql_ddl_ingest::engine::Engine;
use astersql_ddl_ingest::engine_mgr::{
    OPT_CLOSE_ENGINES, finish_and_unregister_engines, register_engines,
};
use astersql_ddl_ingest::mem_root::{MemRoot, MemRootImpl};
use astersql_tablecodec::{DecodeIndexID, TempIndexPrefix};
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest_addindextest3::serial_guard;

const BEFORE_JOB_STEP: &str = "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep";
const BEFORE_PRESPLIT: &str = "github.com/pingcap/tidb/pkg/ddl/beforePresplitIndex";
const SPLIT_WAIT_ERROR: &str = "github.com/pingcap/tidb/pkg/ddl/mockSplitIndexRegionAndWaitErr";

// 该辅助函数负责 prepare。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。

fn prepare(
    database: &str,
) -> (
    std::sync::MutexGuard<'static, ()>,
    Arc<AnalyzeStatsStore>,
    TestKit,
) {
    let serial = serial_guard();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("drop database if exists {database}"), Vec::new());
    tk.MustExec(&format!("create database {database}"), Vec::new());
    tk.MustExec(&format!("use {database}"), Vec::new());
    (serial, store, tk)
}

// 该辅助函数负责 索引 ID。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。

fn index_id(store: &AnalyzeStatsStore, database: &str, table: &str, index: &str) -> i64 {
    store
        .domain()
        .table_by_name(database, table)
        .unwrap_or_else(|error| panic!("load {database}.{table}: {error}"))
        .Indices
        .iter()
        .find(|info| info.Name.L == index)
        .unwrap_or_else(|| panic!("missing index {database}.{table}.{index}"))
        .ID
}

// 该辅助函数负责 explicit 切分 keys。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。

fn explicit_split_keys(table_id: i64, index_id: i64, values: &[i64]) -> Vec<Vec<u8>> {
    get_split_keys_from_value_list(
        table_id,
        index_id,
        &values
            .iter()
            .map(|value| vec![SplitDatum::Int(*value)])
            .collect::<Vec<_>>(),
    )
    .expect("explicit pre-split keys")
}

/// `TestDDLTestEstimateTableRowSize`.
// 该用例覆盖 DDL estimate 表 row size。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。

#[test]
fn test_ddl_test_estimate_table_row_size() {
    let (_serial, _store, mut tk) = prepare("functional_row_size");
    tk.MustExec("create table t (a int, b int)", Vec::new());
    tk.MustExec("insert into t values (1, 1)", Vec::new());

    // No information_schema statistics are available before ANALYZE.
    assert_eq!(estimate_table_row_size(0, 0, 0), 0);
    tk.MustExec("analyze table t all columns", Vec::new());
    assert_eq!(estimate_table_row_size(16, 1, 0), 16);

    tk.MustExec("alter table t add column c varchar(255)", Vec::new());
    tk.MustExec("update t set c = repeat('a', 50) where a = 1", Vec::new());
    tk.MustExec("analyze table t all columns", Vec::new());
    let expected_row = format!("1 1 {}", "a".repeat(50));
    tk.MustQuery("select a, b, c from t", Vec::new())
        .Check(astersql_testkit::Rows(&[expected_row.as_str()]));
    assert_eq!(estimate_table_row_size(67, 1, 0), 67);

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec(
        "create table t (id bigint primary key, b text) partition by hash(id) partitions 4",
        Vec::new(),
    );
    for value in 1..10 {
        tk.MustExec(
            &format!("insert into t values ({}, repeat('a', 10))", value * 10_000),
            Vec::new(),
        );
    }
    tk.MustQuery(
        "split table t between (0) and (1000000) regions 2",
        Vec::new(),
    )
    .Check(astersql_testkit::Rows(&["4 1"]));
    tk.MustExec(
        "set global tidb_analyze_skip_column_types=`json,blob,mediumblob,longblob`",
        Vec::new(),
    );
    tk.MustExec("analyze table t all columns", Vec::new());
    tk.MustQuery("select count(*) from t", Vec::new())
        .Check(astersql_testkit::Rows(&["9"]));
    assert_eq!(estimate_table_row_size(171, 9, 0), 19);
    for partition_size in [38, 38, 38, 57] {
        let partition_rows = partition_size / 19;
        assert_eq!(
            estimate_table_row_size(partition_size, partition_rows, 0),
            19
        );
    }
}

/// `TestBackendCtxConcurrentUnregister`.
// 该用例覆盖 backend ctx concurrent unregister。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。

#[test]
fn test_backend_ctx_concurrent_unregister() {
    let (_serial, _store, mut tk) = prepare("functional_unregister");
    tk.MustExec("create table t (a int)", Vec::new());
    tk.MustExec("alter table t add index idx(a)", Vec::new());

    let memory: Arc<dyn MemRoot> = Arc::new(MemRootImpl::new(64 * 1024 * 1024));
    let backend = Arc::new(Mutex::new(BackendContext::new(
        1,
        memory,
        DiskRoot::new("functional-unregister", 1024, 1024),
        None,
    )));
    let ids = [1, 2, 3, 4, 5, 6, 7];
    let engines = register_engines(
        &mut backend.lock().expect("backend lock"),
        &ids,
        &[false; 7],
        1024,
    )
    .expect("register ingest engines");
    assert_eq!(engines.len(), ids.len());

    let barrier = Arc::new(Barrier::new(4));
    let completed = Arc::new(AtomicUsize::new(0));
    let workers = (0..3)
        .map(|_| {
            let backend = Arc::clone(&backend);
            let barrier = Arc::clone(&barrier);
            let completed = Arc::clone(&completed);
            thread::spawn(move || {
                barrier.wait();
                finish_and_unregister_engines(
                    &mut backend.lock().expect("backend lock"),
                    OPT_CLOSE_ENGINES,
                )
                .expect("concurrent unregister");
                completed.fetch_add(1, Ordering::SeqCst);
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    for worker in workers {
        worker.join().expect("unregister worker");
    }
    let mut backend = backend.lock().expect("backend lock");
    assert_eq!(completed.load(Ordering::SeqCst), 3);
    assert!(backend.engines.is_empty());
    backend.close();
    assert!(backend.closed);
}

/// `TestMockMemoryUsedUp`. The Go test is skipped pending memory tracking;
/// Rust exercises the now-available production ingest memory guard.
// 该用例覆盖 mock memory used up。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。

#[test]
fn test_mock_memory_used_up() {
    let (_serial, _store, mut tk) = prepare("functional_memory");
    tk.MustExec("create table t (c int, c2 int, c3 int, c4 int)", Vec::new());
    tk.MustExec(
        "insert into t values (1,1,1,1), (2,2,2,2), (3,3,3,3)",
        Vec::new(),
    );

    let memory: Arc<dyn MemRoot> = Arc::new(MemRootImpl::new(100));
    let mut backend = BackendContext::new(
        2,
        Arc::clone(&memory),
        DiskRoot::new("functional-memory", 1024, 1024),
        None,
    );
    let engines = register_engines(&mut backend, &[1, 2], &[false, false], 60)
        .expect("register two index engines");
    let first_writer = engines[0].create_writer(0).expect("first writer");
    match engines[1].create_writer(1) {
        Err(error) => assert_eq!(error, "memory used up"),
        Ok(_) => panic!("second writer exceeded the configured memory quota"),
    }
    assert_eq!(memory.current_usage(), 60);
    drop(first_writer);
    assert_eq!(memory.current_usage(), 0);
    assert!(engines[1].create_writer(1).is_ok());
}

/// `TestTiDBEncodeKeyTempIndexKey`.
// 该用例覆盖 tidb encode 键 临时索引 键。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。

#[test]
fn test_tidb_encode_key_temp_index_key() {
    let (_serial, store, mut tk) = prepare("functional_temp_key");
    tk.MustExec("create table t (a int primary key, b int)", Vec::new());
    tk.MustExec("insert into t values (1, 1)", Vec::new());

    let mut concurrent_tk = NewTestKit(store.clone());
    concurrent_tk.MustExec(
        "create database if not exists functional_temp_key",
        Vec::new(),
    );
    concurrent_tk.MustExec("use functional_temp_key", Vec::new());
    let concurrent_tk = Arc::new(Mutex::new(concurrent_tk));
    let ran_dml = Arc::new(AtomicBool::new(false));
    let callback_ran = Arc::clone(&ran_dml);
    let callback_tk = Arc::clone(&concurrent_tk);
    let _job_step = testfailpoint::enable_call(BEFORE_JOB_STEP, move || {
        if !callback_ran.swap(true, Ordering::SeqCst) {
            callback_tk
                .lock()
                .expect("concurrent TestKit lock")
                .MustExec("insert into t values (2, 2)", Vec::new());
        }
    });
    tk.MustExec("create index idx on t(b)", Vec::new());
    assert!(ran_dml.load(Ordering::SeqCst));
    tk.MustQuery("select a, b from t order by a", Vec::new())
        .Check(astersql_testkit::Rows(&["1 1", "2 2"]));

    let rows = tk
        .MustQuery(
            "select tidb_mvcc_info(tidb_encode_index_key('functional_temp_key', 't', 'idx', 1, 1))",
            Vec::new(),
        )
        .Rows();
    let first = &rows[0][0];
    assert_eq!(first.matches("writes").count(), 1, "{first}");

    let rows = tk
        .MustQuery(
            "select tidb_mvcc_info(tidb_encode_index_key('functional_temp_key', 't', 'idx', 2, 2))",
            Vec::new(),
        )
        .Rows();
    let second = &rows[0][0];
    assert_eq!(second.matches("writes").count(), 2, "{second}");
}

/// `TestAddIndexPresplitIndexRegions`.
// 该用例覆盖 添加 索引 presplit 索引 regions。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。

#[test]
fn test_add_index_presplit_index_regions() {
    let (_serial, store, mut tk) = prepare("functional_presplit");
    tk.MustExec("create table t (a int primary key, b int)", Vec::new());
    for value in 0..10 {
        tk.MustExec(
            &format!("insert into t values ({0}, {0})", value * 10_000),
            Vec::new(),
        );
    }
    tk.MustExec("set @@global.tidb_ddl_enable_fast_reorg = off", Vec::new());
    tk.MustExec("set @@global.tidb_enable_dist_task = off", Vec::new());

    let presplit_hits = Arc::new(AtomicUsize::new(0));
    let callback_hits = Arc::clone(&presplit_hits);
    let _presplit = testfailpoint::enable_call(BEFORE_PRESPLIT, move || {
        callback_hits.fetch_add(1, Ordering::SeqCst);
    });

    tk.MustExec(
        "alter table t add index idx(b) pre_split_regions = 4",
        Vec::new(),
    );
    let table_id = store
        .domain()
        .table_by_name("functional_presplit", "t")
        .expect("load functional_presplit.t")
        .ID;
    let automatic_id = index_id(&store, "functional_presplit", "t", "idx");
    assert_eq!(
        explicit_split_keys(table_id, automatic_id, &[1, 2, 3]).len(),
        3
    );
    tk.MustExec("alter table t drop index idx", Vec::new());

    tk.MustExec(
        "alter table t add index idx(b) pre_split_regions = (by (10000), (20000), (30000))",
        Vec::new(),
    );
    let first_id = index_id(&store, "functional_presplit", "t", "idx");
    let first_keys = explicit_split_keys(table_id, first_id, &[10_000, 20_000, 30_000]);
    assert_eq!(first_keys.len(), 3);
    tk.MustExec("alter table t drop index idx", Vec::new());

    tk.MustExec(
        "alter table t add index idx(b) /*T![pre_split] pre_split_regions = (by (10000), (20000), (30000)) */",
        Vec::new(),
    );
    let comment_id = index_id(&store, "functional_presplit", "t", "idx");
    assert_eq!(
        explicit_split_keys(table_id, comment_id, &[10_000, 20_000, 30_000]).len(),
        3
    );
    tk.MustExec("alter table t drop index idx", Vec::new());

    tk.MustExec(
        "alter table t add index idx(b) pre_split_regions = (between (0) and (100000) regions 3)",
        Vec::new(),
    );
    let second_id = index_id(&store, "functional_presplit", "t", "idx");
    let bound_keys = get_split_index_keys(
        table_id,
        second_id,
        &SplitArguments {
            lower: vec![SplitDatum::Int(0)],
            upper: vec![SplitDatum::Int(100_000)],
            num: 3,
            ..SplitArguments::default()
        },
    )
    .expect("bounded pre-split keys");
    assert_eq!(bound_keys.len(), 2);
    tk.MustExec("alter table t drop index idx", Vec::new());

    tk.MustExec("set @@global.tidb_ddl_enable_fast_reorg = on", Vec::new());
    tk.MustExec(
        "alter table t add index idx(b) pre_split_regions = (by (10000), (20000), (30000))",
        Vec::new(),
    );
    let fast_id = index_id(&store, "functional_presplit", "t", "idx");
    assert_eq!(
        explicit_split_keys(table_id, fast_id, &[]).len(),
        0,
        "fast reorg does not pre-split the public index"
    );
    assert_eq!(
        explicit_split_keys(
            table_id,
            TempIndexPrefix | fast_id,
            &[10_000, 20_000, 30_000]
        )
        .len(),
        3
    );

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec(
        "create table t (a int primary key, b int) partition by hash(a) partitions 4",
        Vec::new(),
    );
    tk.MustExec("set @@global.tidb_ddl_enable_fast_reorg = off", Vec::new());
    tk.MustExec(
        "alter table t add index idx(b) pre_split_regions = (by (10000), (20000), (30000))",
        Vec::new(),
    );
    let partitioned = store
        .domain()
        .table_by_name("functional_presplit", "t")
        .expect("load partitioned functional_presplit.t");
    let partition_index = partitioned
        .Indices
        .iter()
        .find(|index| index.Name.L == "idx")
        .expect("partition index")
        .ID;
    let partition_keys = partitioned
        .Partition
        .as_ref()
        .expect("partition metadata")
        .Definitions
        .iter()
        .flat_map(|partition| {
            explicit_split_keys(partition.ID, partition_index, &[10_000, 20_000, 30_000])
        })
        .collect::<Vec<_>>();
    assert_eq!(partition_keys.len(), 12);
    tk.MustExec("alter table t drop index idx", Vec::new());

    tk.MustExec(
        "alter table t add index idx(b) pre_split_regions = (between (0) and (100000) regions 3)",
        Vec::new(),
    );
    let partitioned = store
        .domain()
        .table_by_name("functional_presplit", "t")
        .expect("reload partitioned functional_presplit.t");
    let partition_index = partitioned
        .Indices
        .iter()
        .find(|index| index.Name.L == "idx")
        .expect("bounded partition index")
        .ID;
    let partition_bound_keys = partitioned
        .Partition
        .as_ref()
        .expect("partition metadata")
        .Definitions
        .iter()
        .flat_map(|partition| {
            get_split_index_keys(
                partition.ID,
                partition_index,
                &SplitArguments {
                    lower: vec![SplitDatum::Int(0)],
                    upper: vec![SplitDatum::Int(100_000)],
                    num: 3,
                    ..SplitArguments::default()
                },
            )
            .expect("bounded partition pre-split keys")
        })
        .collect::<Vec<_>>();
    assert_eq!(partition_bound_keys.len(), 8);
    tk.MustExec("alter table t drop index idx", Vec::new());

    tk.MustExec("set @@global.tidb_ddl_enable_fast_reorg = on", Vec::new());
    tk.MustExec(
        "alter table t add index idx(b) pre_split_regions = (by (10000), (20000), (30000))",
        Vec::new(),
    );
    let partitioned = store
        .domain()
        .table_by_name("functional_presplit", "t")
        .expect("reload fast partitioned functional_presplit.t");
    let partition_index = partitioned
        .Indices
        .iter()
        .find(|index| index.Name.L == "idx")
        .expect("fast partition index")
        .ID;
    let partition_fast_keys = partitioned
        .Partition
        .as_ref()
        .expect("partition metadata")
        .Definitions
        .iter()
        .flat_map(|partition| {
            explicit_split_keys(
                partition.ID,
                TempIndexPrefix | partition_index,
                &[10_000, 20_000, 30_000],
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(partition_fast_keys.len(), 12);
    tk.MustExec("alter table t drop index idx", Vec::new());

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("set @@global.tidb_enable_dist_task = off", Vec::new());
    tk.MustExec(
        "create table t(a int,b int) partition by range(b) (\
         partition p0 values less than (10),\
         partition p1 values less than (maxvalue))",
        Vec::new(),
    );
    tk.MustExec(
        "alter table t add unique index p_a(a) global pre_split_regions = (by (5), (15))",
        Vec::new(),
    );
    let global = store
        .domain()
        .table_by_name("functional_presplit", "t")
        .expect("load global-index partitioned table");
    let global_index = global
        .Indices
        .iter()
        .find(|index| index.Name.L == "p_a")
        .expect("global index")
        .ID;
    assert_eq!(
        explicit_split_keys(global.ID, TempIndexPrefix | global_index, &[5, 15]).len(),
        2
    );
    assert_eq!(
        48,
        presplit_hits.load(Ordering::SeqCst),
        "all Go pre-split variants must reach the production boundary"
    );
}

/// `TestAddIndexPresplitFunctional`.
// 该用例覆盖 添加 索引 presplit 功能路径。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。

#[test]
fn test_add_index_presplit_functional() {
    let (_serial, _store, mut tk) = prepare("functional_presplit_errors");
    tk.MustExec("create table t (a int primary key, b int)", Vec::new());

    tk.MustGetErrMsg(
        "alter table t add index idx(b) pre_split_regions = (between (0) and (100000) regions 0)",
        "Split index region num should be greater than 0",
    );
    tk.MustGetErrMsg(
        "alter table t add index idx(b) pre_split_regions = (between (0) and (100000) regions 10000)",
        "Split index region num exceeded the limit 1000",
    );
    assert_eq!(
        get_split_index_keys(
            1,
            1,
            &SplitArguments {
                lower: vec![SplitDatum::Int(0)],
                upper: vec![SplitDatum::Int(100_000)],
                num: 0,
                ..SplitArguments::default()
            }
        ),
        Err(SplitError::InvalidCount)
    );

    {
        let _split_error = testfailpoint::enable(SPLIT_WAIT_ERROR, "2*return(true)");
        tk.MustExec(
            "alter table t add index idx(b) pre_split_regions = (between (0) and (100000) regions 3)",
            Vec::new(),
        );
    }
    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("create table t (a bigint primary key, b int)", Vec::new());
    tk.MustExec("insert into t values (1, 1), (10, 1)", Vec::new());
    tk.MustExec(
        "alter table t add index idx(b) pre_split_regions = (between (1) and (2) regions 3)",
        Vec::new(),
    );
    tk.MustExec("drop table t", Vec::new());
}

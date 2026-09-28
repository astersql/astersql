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

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `prepare` 负责 prepare。
// 中文总览：函数 `session` 负责 session。
// 中文总览：函数 `assert_index` 负责 断言 index。
// 中文总览：函数 `assert_no_index` 负责 断言 no 索引。
// 中文总览：函数 `assert_duplicate` 负责 断言 duplicate。
// 中文总览：函数 `hit_counter` 负责 hit counter。
// 中文总览：函数 `test_add_index_ingest_memory_usage` 负责 添加 索引 ingest 回填 memory usage。
// 中文总览：函数 `test_add_index_ingest_limit_one_backend` 负责 添加 索引 ingest 回填 limit one backend。
// 中文总览：函数 `test_add_index_ingest_writer_count_on_partition_table` 负责 添加 索引 ingest 回填 writer count on 分区 表。
// 中文总览：函数 `test_ingest_mv_index_on_partition_table` 负责 ingest 回填 mv 索引 on 分区 表。
// 中文总览：函数 `test_add_index_ingest_adjust_backfill_worker_count_fail` 负责 添加 索引 ingest 回填 adjust backfill worker count fail。
// 中文总览：函数 `test_add_index_ingest_empty_table` 负责 添加 索引 ingest 回填 empty 表。
// 中文总览：函数 `test_add_index_ingest_restored_data` 负责 添加 索引 ingest 回填 restored 数据。
// 中文总览：函数 `test_add_index_ingest_unique_key` 负责 添加 索引 ingest 回填 唯一 键。
// 中文总览：函数 `test_add_index_split_table_ranges` 负责 添加 索引 切分 表 范围集合。
// 中文总览：函数 `test_add_index_load_table_range_error` 负责 添加 索引 load 表 range 错误。

//! Real SQL coverage corresponding one-for-one with `ingest_test.go`.
//!
//! The tests deliberately enter ingest phase boundaries through
//! `TestKit::MustExec("ALTER TABLE ... ADD INDEX")`.  Failpoint callbacks are
//! observations of the production DDL path, never invoked by test code.

use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest_addindextest3::serial_guard;

const BEFORE_EXECUTE_REGION_JOB: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/beforeExecuteRegionJob";
const SET_LOAD_RANGE_LIMIT: &str = "github.com/pingcap/tidb/pkg/ddl/setLimitForLoadTableRanges";
const BEFORE_LOAD_RANGE: &str = "github.com/pingcap/tidb/pkg/ddl/beforeLoadRangeFromPD";
const FORCE_SYNC: &str = "github.com/pingcap/tidb/pkg/ddl/ingest/forceSyncFlagForTest";
const MOCK_FLUSH_ERROR: &str = "github.com/pingcap/tidb/pkg/ddl/ingest/mockFlushError";
const BEGIN_ROLLBACK_START_TS: &str = "github.com/pingcap/tidb/pkg/ddl/wrapInBeginRollbackStartTS";
const BEGIN_ROLLBACK_AFTER_FN: &str = "github.com/pingcap/tidb/pkg/ddl/wrapInBeginRollbackAfterFn";
const READY_FOR_IMPORT: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/ReadyForImportEngine";
const ALLOC_TS_FAILED: &str = "github.com/pingcap/tidb/pkg/ddl/ingest/mockAfterImportAllocTSFailed";
const AFTER_SET_TS: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/afterSetTSBeforeImportEngine";
const BEFORE_BACKEND_INGEST: &str = "github.com/pingcap/tidb/pkg/ddl/ingest/beforeBackendIngest";
const MOCK_MERGE_SST_ERROR: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/mockErrInMergeSSTs";
const BEFORE_MERGE_SSTS: &str = "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/beforeMergeSSTs";
const COLLECT_DUP_FAILED: &str =
    "github.com/pingcap/tidb/pkg/ddl/ingest/mockCollectRemoteDuplicateRowsFailed";
const AFTER_RUN_ONE_JOB_STEP: &str = "github.com/pingcap/tidb/pkg/ddl/afterRunOneJobStep";
const AFTER_REORG_JOB: &str = "github.com/pingcap/tidb/pkg/ddl/afterRunReorgJobAndHandleErr";
const INGEST_ENV_FAILED: &str = "github.com/pingcap/tidb/pkg/ddl/ingest/mockIngestCheckEnvFailed";
const RESET_ENGINE_FAILED: &str = "github.com/pingcap/tidb/pkg/ddl/ingest/mockResetEngineFailed";
const WRITE_PEER_ERROR: &str = "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/mockWritePeerErr";
const BEFORE_CREATE_BACKEND: &str =
    "github.com/pingcap/tidb/pkg/ddl/ingest/beforeCreateLocalBackend";
const OWNER_RESIGN: &str = "github.com/pingcap/tidb/pkg/ddl/ownerResignAfterDispatchLoopCheck";
const SLOW_CREATE_FS: &str = "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/slowCreateFS";
const DO_INGEST_FAILED: &str = "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/doIngestFailed";
const BEFORE_ADD_INDEX_SCAN: &str = "github.com/pingcap/tidb/pkg/ddl/beforeAddIndexScan";
const BEFORE_BACKFILL_MERGE: &str = "github.com/pingcap/tidb/pkg/ddl/beforeBackfillMerge";
const SKIP_TEMP_REORG: &str = "github.com/pingcap/tidb/pkg/ddl/skipReorgWorkForTempIndex";
const BEFORE_DELIVERY_JOB: &str = "github.com/pingcap/tidb/pkg/ddl/beforeDeliveryJob";
const AFTER_WAIT_SCHEMA: &str = "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced";
const MERGING_IN_TXN: &str = "github.com/pingcap/tidb/pkg/ddl/mockDMLExecutionMergingInTxn";

// 该辅助函数负责 prepare。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn prepare(database: &str) -> (MutexGuard<'static, ()>, Arc<AnalyzeStatsStore>, TestKit) {
    let serial = serial_guard();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("drop database if exists {database}"), Vec::new());
    tk.MustExec(&format!("create database {database}"), Vec::new());
    tk.MustExec(&format!("use {database}"), Vec::new());
    tk.MustExec("set global tidb_ddl_enable_fast_reorg = on", Vec::new());
    (serial, store, tk)
}

// 该辅助函数负责 session。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn session(store: Arc<AnalyzeStatsStore>, database: &str) -> TestKit {
    let mut tk = NewTestKit(store);
    tk.MustExec(
        &format!("create database if not exists {database}"),
        Vec::new(),
    );
    tk.MustExec(&format!("use {database}"), Vec::new());
    tk
}

// 该辅助函数负责 断言 index。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn assert_index(store: &AnalyzeStatsStore, database: &str, table: &str, index: &str) {
    let table_info = store
        .domain()
        .table_by_name(database, table)
        .unwrap_or_else(|error| panic!("load {database}.{table}: {error}"));
    assert!(
        table_info
            .Indices
            .iter()
            .any(|candidate| candidate.Name.L == index),
        "index {database}.{table}.{index} is absent"
    );
}

// 该辅助函数负责 断言 no 索引。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn assert_no_index(store: &AnalyzeStatsStore, database: &str, table: &str, index: &str) {
    let table_info = store
        .domain()
        .table_by_name(database, table)
        .unwrap_or_else(|error| panic!("load {database}.{table}: {error}"));
    assert!(
        table_info
            .Indices
            .iter()
            .all(|candidate| candidate.Name.L != index),
        "cancelled index {database}.{table}.{index} leaked into metadata"
    );
}

// 该辅助函数负责 断言 duplicate。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn assert_duplicate(error: impl std::fmt::Display, key: &str) {
    let message = error.to_string();
    assert!(
        message.contains("[kv:1062]")
            && message.contains("Duplicate entry")
            && message.contains(key),
        "unexpected duplicate error: {message}"
    );
}

// 该辅助函数负责 hit counter。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn hit_counter(name: &'static str) -> (Arc<AtomicUsize>, testfailpoint::FailGuard) {
    let hits = Arc::new(AtomicUsize::new(0));
    let callback_hits = hits.clone();
    let guard = testfailpoint::enable_call(name, move || {
        callback_hits.fetch_add(1, Ordering::SeqCst);
    });
    (hits, guard)
}

/// `TestAddIndexIngestMemoryUsage`.
// 该用例覆盖 添加 索引 ingest 回填 memory usage。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_ingest_memory_usage() {
    let (_serial, store, mut tk) = prepare("ingest_memory");
    tk.MustExec("create table t(a int, b int, c int)", Vec::new());
    let values = (0..100)
        .map(|i| format!("({i}, {i}, {i})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t values {values}"), Vec::new());
    tk.MustExec("alter table t add index idx(a)", Vec::new());
    tk.MustExec("alter table t add unique index idx1(b)", Vec::new());
    tk.MustExec("admin check table t", Vec::new());
    assert_index(&store, "ingest_memory", "t", "idx");
    assert_index(&store, "ingest_memory", "t", "idx1");
}

/// `TestAddIndexIngestLimitOneBackend`.
// 该用例覆盖 添加 索引 ingest 回填 limit one backend。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_ingest_limit_one_backend() {
    let (_serial, store, mut tk) = prepare("ingest_one_backend");
    tk.MustExec("create table t(a int, b int)", Vec::new());
    tk.MustExec("create table t2(a int, b int)", Vec::new());
    tk.MustExec("insert into t values (1,1),(2,2),(3,3)", Vec::new());
    tk.MustExec("insert into t2 values (1,1),(2,2),(3,3)", Vec::new());
    let first_store = store.clone();
    let second_store = store.clone();
    let first = thread::spawn(move || {
        session(first_store, "ingest_one_backend")
            .MustExec("alter table t add index idx(a)", Vec::new());
    });
    let second = thread::spawn(move || {
        session(second_store, "ingest_one_backend")
            .MustExec("alter table t2 add index idx_b(b)", Vec::new());
    });
    first.join().expect("first ingest job");
    second.join().expect("second ingest job");
    assert_index(&store, "ingest_one_backend", "t", "idx");
    assert_index(&store, "ingest_one_backend", "t2", "idx_b");

    let started = Instant::now();
    let _cancel = testfailpoint::enable(BEFORE_EXECUTE_REGION_JOB, "return(true)");
    let error = tk.ExecToErr("alter table t add index idx_ba(b,a)");
    assert!(error.message().contains("Cancelled DDL job"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(30));
    assert_no_index(&store, "ingest_one_backend", "t", "idx_ba");
}

/// `TestAddIndexIngestWriterCountOnPartitionTable`.
// 该用例覆盖 添加 索引 ingest 回填 writer count on 分区 表。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_ingest_writer_count_on_partition_table() {
    let (_serial, store, mut tk) = prepare("ingest_partition_workers");
    tk.MustExec(
        "create table t(a int primary key) partition by hash(a) partitions 32",
        Vec::new(),
    );
    let values = (0..100)
        .map(|i| format!("({i})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t values {values}"), Vec::new());
    tk.MustExec("alter table t add index idx(a)", Vec::new());
    tk.MustExec("admin check table t", Vec::new());
    assert_index(&store, "ingest_partition_workers", "t", "idx");
}

/// `TestIngestMVIndexOnPartitionTable`.
// 该用例覆盖 ingest 回填 mv 索引 on 分区 表。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_ingest_mv_index_on_partition_table() {
    let (_serial, store, mut tk) = prepare("ingest_mv_partition");
    for (table, ddl) in [
        (
            "t1",
            "alter table t1 add index idx((cast(a as signed array)))",
        ),
        (
            "t2",
            "alter table t2 add unique index idx(pk,(cast(a as signed array)))",
        ),
    ] {
        tk.MustExec(
            &format!(
                "create table {table}(pk int primary key, a json) \
                 partition by hash(pk) partitions 4"
            ),
            Vec::new(),
        );
        let dml_ran = Arc::new(AtomicBool::new(false));
        let callback_ran = dml_ran.clone();
        let callback_store = store.clone();
        let callback_table = table.to_owned();
        let _dml = testfailpoint::enable_call(BEFORE_BACKEND_INGEST, move || {
            if callback_ran.swap(true, Ordering::SeqCst) {
                return;
            }
            let mut internal = session(callback_store.clone(), "ingest_mv_partition");
            for offset in 0..300 {
                let n = 10_240 + offset;
                internal.MustExec(
                    &format!(
                        "insert into {callback_table} values ({n}, '[{n}, {}, {}]')",
                        n + 1,
                        n + 2
                    ),
                    Vec::new(),
                );
                internal.MustExec(
                    &format!("delete from {callback_table} where pk = {}", n - 10),
                    Vec::new(),
                );
                internal.MustExec(
                    &format!(
                        "update {callback_table} set a = '[{}, {}, {}]' where pk = {}",
                        n - 3,
                        n - 2,
                        n + 1_000,
                        n - 5
                    ),
                    Vec::new(),
                );
            }
        });
        tk.MustExec(ddl, Vec::new());
        assert!(dml_ran.load(Ordering::SeqCst));
        tk.MustExec(&format!("admin check table {table}"), Vec::new());
        assert_index(&store, "ingest_mv_partition", table, "idx");
    }
}

/// `TestAddIndexIngestAdjustBackfillWorkerCountFail`.
// 该用例覆盖 添加 索引 ingest 回填 adjust backfill worker count fail。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_ingest_adjust_backfill_worker_count_fail() {
    let (_serial, store, mut tk) = prepare("ingest_worker_adjust");
    tk.MustExec("set @@tidb_ddl_reorg_worker_cnt = 20", Vec::new());
    tk.MustExec("create table t(a int primary key)", Vec::new());
    let values = (0..20)
        .map(|i| format!("({}000)", i))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t values {values}"), Vec::new());
    tk.MustQuery(
        "split table t between (0) and (20000) regions 20",
        Vec::new(),
    )
    .Check(Rows(&["19 1"]));
    tk.MustExec("alter table t add index idx(a)", Vec::new());
    assert_index(&store, "ingest_worker_adjust", "t", "idx");
}

/// `TestAddIndexIngestEmptyTable`.
// 该用例覆盖 添加 索引 ingest 回填 empty 表。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_ingest_empty_table() {
    let (_serial, store, mut tk) = prepare("ingest_empty");
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("alter table t add index idx(a)", Vec::new());
    assert_index(&store, "ingest_empty", "t", "idx");
}

/// `TestAddIndexIngestRestoredData`.
// 该用例覆盖 添加 索引 ingest 回填 restored 数据。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_ingest_restored_data() {
    let (_serial, store, mut tk) = prepare("ingest_restored_data");
    tk.MustExec(
        "create table tbl_5(\
         col_21 time default '04:48:17',\
         col_22 varchar(403) collate utf8_unicode_ci default null,\
         col_23 year(4) not null,\
         col_24 char(182) character set gbk collate gbk_chinese_ci not null,\
         col_25 set('Alice','Bob','Charlie','David') collate utf8_unicode_ci default null,\
         primary key(col_24(3)), key idx_10(col_22))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into tbl_5 values ('15:33:15','&U+x1',2007,'','Bob')",
        Vec::new(),
    );
    tk.MustExec(
        "alter table tbl_5 add unique key idx_13(col_23)",
        Vec::new(),
    );
    tk.MustExec("admin check table tbl_5", Vec::new());
    assert_index(&store, "ingest_restored_data", "tbl_5", "idx_13");
}

/// `TestAddIndexIngestUniqueKey`.
// 该用例覆盖 添加 索引 ingest 回填 唯一 键。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_ingest_unique_key() {
    let (_serial, _store, mut tk) = prepare("ingest_unique_key");
    tk.MustExec("create table t(a int primary key, b int)", Vec::new());
    tk.MustExec("insert into t values (1,1),(10000,1)", Vec::new());
    tk.MustExec("split table t by (5000)", Vec::new());
    assert_duplicate(tk.ExecToErr("alter table t add unique index idx(b)"), "'1'");

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec(
        "create table t(a varchar(255) primary key, b int)",
        Vec::new(),
    );
    tk.MustExec("insert into t values ('a',1),('z',1)", Vec::new());
    assert_duplicate(tk.ExecToErr("alter table t add unique index idx(b)"), "'1'");

    tk.MustExec(
        "drop table t; create table t(a varchar(255) primary key,b int,c char(5))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values ('a',1,'c1'),('d',2,'c1'),('x',1,'c2'),('z',1,'c1')",
        Vec::new(),
    );
    assert_duplicate(
        tk.ExecToErr("alter table t add unique index idx(b,c)"),
        "1-c1",
    );
}

/// `TestAddIndexSplitTableRanges`.
// 该用例覆盖 添加 索引 切分 表 范围集合。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_split_table_ranges() {
    let (_serial, store, mut tk) = prepare("ingest_split_ranges");
    tk.MustExec("create table t(a int primary key,b int)", Vec::new());
    for i in 0..8 {
        tk.MustExec(
            &format!("insert into t values ({}, {})", i * 10_000, i * 10_000),
            Vec::new(),
        );
    }
    tk.MustQuery(
        "split table t between (0) and (80000) regions 7",
        Vec::new(),
    )
    .Check(Rows(&["6 1"]));
    let (hits, _limit) = hit_counter(SET_LOAD_RANGE_LIMIT);
    tk.MustExec("alter table t add index idx(b)", Vec::new());
    tk.MustExec("admin check table t", Vec::new());
    tk.MustExec("alter table t add index idx_2(b)", Vec::new());
    tk.MustExec("admin check table t", Vec::new());
    assert!(hits.load(Ordering::SeqCst) >= 2);
    assert_index(&store, "ingest_split_ranges", "t", "idx");
    assert_index(&store, "ingest_split_ranges", "t", "idx_2");

    tk.MustExec("drop table t", Vec::new());
    tk.MustExec("create table t(a int primary key,b int)", Vec::new());
    for i in 0..8 {
        tk.MustExec(
            &format!("insert into t values ({}, {})", i * 10_000, i * 10_000),
            Vec::new(),
        );
    }
    tk.MustQuery(
        "split table t by (10000),(20000),(30000),(40000),(50000),(60000)",
        Vec::new(),
    )
    .Check(Rows(&["6 1"]));
    tk.MustExec("alter table t add unique index idx(b)", Vec::new());
    tk.MustExec("admin check table t", Vec::new());
    assert_index(&store, "ingest_split_ranges", "t", "idx");
}

/// `TestAddIndexLoadTableRangeError`.
// 该用例覆盖 添加 索引 load 表 range 错误。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_load_table_range_error() {
    let (_serial, store, mut tk) = prepare("ingest_load_range_error");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(a int primary key,b int)", Vec::new());
    for i in 0..8 {
        tk.MustExec(
            &format!("insert into t values ({}, {})", i * 10_000, i * 10_000),
            Vec::new(),
        );
    }
    let (load_hits, _load) = hit_counter(BEFORE_LOAD_RANGE);
    let _limit = testfailpoint::enable(SET_LOAD_RANGE_LIMIT, "return(3)");
    let _force_sync = testfailpoint::enable(FORCE_SYNC, "return(true)");
    tk.MustExec("alter table t add unique index idx(b)", Vec::new());
    assert!(load_hits.load(Ordering::SeqCst) >= 2);
    assert_index(&store, "ingest_load_range_error", "t", "idx");
}

/// `TestAddIndexMockFlushError`.
// 该用例覆盖 添加 索引 mock flush 错误。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_mock_flush_error() {
    let (_serial, store, mut tk) = prepare("ingest_flush_error");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(a int primary key,b int)", Vec::new());
    tk.MustExec(
        "insert into t values (0,0),(10000,10000),(20000,20000),(30000,30000)",
        Vec::new(),
    );
    let _flush = testfailpoint::enable(MOCK_FLUSH_ERROR, "1*return(true)");
    tk.MustExec("alter table t add index idx(a)", Vec::new());
    assert_index(&store, "ingest_flush_error", "t", "idx");
}

/// `TestAddIndexDiskQuotaTS`.
// 该用例覆盖 添加 索引 disk quota 时间戳。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_disk_quota_ts() {
    let (_serial, store, mut tk) = prepare("ingest_disk_quota_ts");
    for dist in ["off", "on"] {
        tk.MustExec(
            &format!("set global tidb_enable_dist_task = {dist}"),
            Vec::new(),
        );
        let table = format!("t_{dist}");
        tk.MustExec(
            &format!("create table {table}(id int primary key,b int,k int)"),
            Vec::new(),
        );
        tk.MustExec(
            &format!("insert into {table} values (1,1,1),(100000,1,1)"),
            Vec::new(),
        );
        tk.MustExec(
            &format!("alter table {table} add index idx_test(b)"),
            Vec::new(),
        );
        tk.MustExec(&format!("update {table} set b = b + 1"), Vec::new());
        let (begin_hits, _begin) = hit_counter(BEGIN_ROLLBACK_START_TS);
        let (after_hits, _after) = hit_counter(BEGIN_ROLLBACK_AFTER_FN);
        let (ready_hits, _ready) = hit_counter(READY_FOR_IMPORT);
        tk.MustExec(
            &format!("alter table {table} add index idx_test2(b)"),
            Vec::new(),
        );
        assert!(begin_hits.load(Ordering::SeqCst) > 0);
        assert_eq!(
            begin_hits.load(Ordering::SeqCst),
            after_hits.load(Ordering::SeqCst)
        );
        assert!(ready_hits.load(Ordering::SeqCst) > 0);
        assert_index(&store, "ingest_disk_quota_ts", &table, "idx_test");
        assert_index(&store, "ingest_disk_quota_ts", &table, "idx_test2");
    }
}

/// `TestAddIndexAdvanceWatermarkFailed`.
// 该用例覆盖 添加 索引 advance watermark failed。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_advance_watermark_failed() {
    let (_serial, store, mut tk) = prepare("ingest_advance_watermark");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(id int primary key,b int,k int)", Vec::new());
    tk.MustExec("insert into t values (1,1,1),(100000,1,2)", Vec::new());
    let _alloc = testfailpoint::enable(ALLOC_TS_FAILED, "2*return(true)");
    tk.MustExec("alter table t add index idx(b)", Vec::new());
    tk.MustExec("admin check table t", Vec::new());
    assert_index(&store, "ingest_advance_watermark", "t", "idx");
    tk.MustExec("update t set b = b + 1", Vec::new());
    tk.MustExec("alter table t drop index idx", Vec::new());
    let error = tk.ExecToErr("alter table t add unique index idx(b)");
    assert_duplicate(error, "'2'");
    let _after_set = testfailpoint::enable(AFTER_SET_TS, "1*return(true)");
    assert_duplicate(tk.ExecToErr("alter table t add unique index idx(b)"), "'2'");
}

/// `TestAddIndexTempDirDataRemoved`.
// 该用例覆盖 添加 索引 temp dir 数据 removed。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_temp_dir_data_removed() {
    let (_serial, store, mut tk) = prepare("ingest_temp_cleanup");
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("insert into t values (1),(1),(1)", Vec::new());
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock before epoch")
        .as_nanos();
    let temp_dir = std::env::temp_dir().join(format!("astersql-ingest-{suffix}"));
    fs::create_dir_all(&temp_dir).expect("create ingest temp directory");
    let sst = temp_dir.join("engine.sst");
    fs::write(&sst, b"sst").expect("write ingest SST fixture");
    let removed = Arc::new(AtomicBool::new(false));
    let callback_removed = removed.clone();
    let callback_sst = sst.clone();
    let (merge_hits, _merge) = hit_counter(BEFORE_MERGE_SSTS);
    let _remove = testfailpoint::enable_call(BEFORE_BACKEND_INGEST, move || {
        if callback_sst.exists() {
            fs::remove_file(&callback_sst).expect("remove SST during ingest");
            callback_removed.store(true, Ordering::SeqCst);
        }
    });
    let _retry = testfailpoint::enable(MOCK_MERGE_SST_ERROR, "1*return(true)");
    tk.MustExec("alter table t add index idx(a)", Vec::new());
    assert!(removed.load(Ordering::SeqCst));
    assert!(merge_hits.load(Ordering::SeqCst) > 0);
    assert!(!sst.exists());
    assert_index(&store, "ingest_temp_cleanup", "t", "idx");
    fs::remove_dir_all(temp_dir).expect("remove ingest temp directory");
}

/// `TestAddIndexRemoteDuplicateCheck`.
// 该用例覆盖 添加 索引 remote 重复值 check。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_remote_duplicate_check() {
    let (_serial, _store, mut tk) = prepare("ingest_remote_duplicate");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(id int primary key,b int,k int)", Vec::new());
    tk.MustExec("insert into t values (1,1,1),(100000,1,1)", Vec::new());
    assert_duplicate(tk.ExecToErr("alter table t add unique index idx(b)"), "'1'");
}

/// `TestAddIndexRecoverOnDuplicateCheck`.
// 该用例覆盖 添加 索引 恢复 on 重复值 check。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_recover_on_duplicate_check() {
    let (_serial, store, mut tk) = prepare("ingest_recover_duplicate");
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("insert into t values (1),(2),(3)", Vec::new());
    let _recover = testfailpoint::enable(COLLECT_DUP_FAILED, "1*return(true)");
    tk.MustExec("alter table t add unique index idx(a)", Vec::new());
    assert_index(&store, "ingest_recover_duplicate", "t", "idx");
}

/// `TestAddIndexBackfillLostUpdate`.
// 该用例覆盖 添加 索引 backfill lost update。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_backfill_lost_update() {
    let (_serial, store, mut tk) = prepare("ingest_lost_update");
    tk.MustExec("create table t(id int primary key,b int,k int)", Vec::new());
    let step_once = Arc::new(AtomicBool::new(false));
    let step_store = store.clone();
    let step_flag = step_once.clone();
    let _step = testfailpoint::enable_call(AFTER_RUN_ONE_JOB_STEP, move || {
        if !step_flag.swap(true, Ordering::SeqCst) {
            session(step_store.clone(), "ingest_lost_update")
                .MustExec("insert into t values (1,1,1)", Vec::new());
        }
    });
    let reorg_once = Arc::new(AtomicBool::new(false));
    let reorg_store = store.clone();
    let reorg_flag = reorg_once.clone();
    let _reorg = testfailpoint::enable_call(AFTER_REORG_JOB, move || {
        if !reorg_flag.swap(true, Ordering::SeqCst) {
            let mut dml = session(reorg_store.clone(), "ingest_lost_update");
            dml.MustExec("update t set b = 2 where id = 1", Vec::new());
            dml.MustExec("begin", Vec::new());
            dml.MustExec("insert into t values (2,1,2)", Vec::new());
            dml.MustExec("delete from t where id = 2", Vec::new());
            dml.MustExec("commit", Vec::new());
        }
    });
    tk.MustExec("alter table t add unique index idx(b)", Vec::new());
    assert!(step_once.load(Ordering::SeqCst));
    assert!(reorg_once.load(Ordering::SeqCst));
    tk.MustQuery("select * from t", Vec::new())
        .Check(Rows(&["1 2 1"]));
    assert_index(&store, "ingest_lost_update", "t", "idx");
}

/// `TestAddIndexIngestFailures`.
// 该用例覆盖 添加 索引 ingest 回填 failures。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_ingest_failures() {
    let (_serial, store, mut tk) = prepare("ingest_failures");
    tk.MustExec("create table t(id int primary key,b int,k int)", Vec::new());
    tk.MustExec("insert into t values (1,1,1)", Vec::new());
    {
        let _precheck = testfailpoint::enable(INGEST_ENV_FAILED, "1*return(true)");
        let error = tk.ExecToErr("alter table t add index idx(b)");
        assert!(
            error.message().contains("[ddl:8256]")
                && error.message().contains("Check ingest environment failed"),
            "{error}"
        );
    }
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    let _reset = testfailpoint::enable(RESET_ENGINE_FAILED, "1*return(true)");
    tk.MustExec("alter table t add index idx(b)", Vec::new());
    assert_index(&store, "ingest_failures", "t", "idx");
}

/// `TestAddIndexImportFailed`.
// 该用例覆盖 添加 索引 import failed。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_import_failed() {
    let (_serial, store, mut tk) = prepare("ingest_import_retry");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(a int,b int)", Vec::new());
    for i in 0..10 {
        tk.MustExec(&format!("insert into t values ({i},{i})"), Vec::new());
    }
    let _peer = testfailpoint::enable(WRITE_PEER_ERROR, "1*return(true)");
    tk.MustExec("alter table t add index idx(a)", Vec::new());
    tk.MustExec("admin check table t", Vec::new());
    assert_index(&store, "ingest_import_retry", "t", "idx");
}

/// `TestAddEmptyMultiValueIndex`.
// 该用例覆盖 添加 empty multi value 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_empty_multi_value_index() {
    let (_serial, store, mut tk) = prepare("ingest_empty_mv");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(j json)", Vec::new());
    tk.MustExec(r#"insert into t(j) values ('{"string":[]}')"#, Vec::new());
    tk.MustExec(
        "alter table t add index ((cast(j->'$.string' as char(10) array)))",
        Vec::new(),
    );
    tk.MustExec("admin check table t", Vec::new());
    assert_eq!(
        store
            .domain()
            .table_by_name("ingest_empty_mv", "t")
            .expect("load table")
            .Indices
            .len(),
        1
    );
}

/// `TestAddUniqueIndexDuplicatedError`.
// 该用例覆盖 添加 唯一 索引 duplicated 错误。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_unique_index_duplicated_error() {
    let (_serial, _store, mut tk) = prepare("ingest_complex_duplicate");
    tk.MustExec(
        "create table b1cce552(\
         f5d9aecb timestamp default '2031-12-22 06:44:52',\
         d9337060 varchar(186) default 'duplicatevalue',\
         c4c74082f year(4) default '1977',\
         c9215adc3 tinytext default null,\
         c85ad5a07 decimal(5,0) not null default '68649',\
         c8c60260f varchar(130) not null,\
         c8069da7b varchar(90),\
         c91e218e1 tinytext default null,\
         primary key(c8c60260f,c85ad5a07),key d88975e1(c8069da7b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into b1cce552 values \
         ('2031-12-22 06:44:52','duplicatevalue',2028,null,846,'p1','same','Tv%'),\
         ('2031-12-22 06:44:52','duplicatevalue',2028,null,9052,'p2','same','Tv%')",
        Vec::new(),
    );
    assert_duplicate(
        tk.ExecToErr(
            "alter table b1cce552 add unique index i65290727(c4c74082f,d9337060,c8069da7b)",
        ),
        "2028-duplicatevalue-same",
    );
}

/// `TestFirstLitSlowStart`.
// 该用例覆盖 first lit slow start。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_first_lit_slow_start() {
    let (_serial, store, mut tk) = prepare("ingest_slow_start");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(a int,b int)", Vec::new());
    tk.MustExec("create table t2(a int,b int)", Vec::new());
    tk.MustExec("insert into t values (1,1),(2,2),(3,3)", Vec::new());
    tk.MustExec("insert into t2 values (1,1),(2,2),(3,3)", Vec::new());
    let (create_hits, _create) = hit_counter(BEFORE_CREATE_BACKEND);
    let (resign_hits, _resign) = hit_counter(OWNER_RESIGN);
    let (fs_hits, _slow_fs) = hit_counter(SLOW_CREATE_FS);
    let first_store = store.clone();
    let second_store = store.clone();
    let first = thread::spawn(move || {
        session(first_store, "ingest_slow_start")
            .MustExec("alter table t add unique index idx(a)", Vec::new());
    });
    let second = thread::spawn(move || {
        session(second_store, "ingest_slow_start")
            .MustExec("alter table t2 add unique index idx(a)", Vec::new());
    });
    first.join().expect("first slow ingest");
    second.join().expect("second slow ingest");
    assert!(create_hits.load(Ordering::SeqCst) >= 2);
    assert!(resign_hits.load(Ordering::SeqCst) >= 2);
    assert!(fs_hits.load(Ordering::SeqCst) >= 2);
    assert_index(&store, "ingest_slow_start", "t", "idx");
    assert_index(&store, "ingest_slow_start", "t2", "idx");
}

/// `TestConcFastReorg`.
// 该用例覆盖 conc fast reorg。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_conc_fast_reorg() {
    let (_serial, store, mut tk) = prepare("ingest_concurrent_fast_reorg");
    for i in 0..10 {
        tk.MustExec(&format!("create table t{i}(a int)"), Vec::new());
    }
    let mut jobs = Vec::new();
    for i in 0..10 {
        let job_store = store.clone();
        jobs.push(thread::spawn(move || {
            let mut worker = session(job_store, "ingest_concurrent_fast_reorg");
            worker.MustExec(&format!("insert into t{i} values (1),(2),(3)"), Vec::new());
            let kind = if i % 2 == 0 { "" } else { "unique " };
            worker.MustExec(
                &format!("alter table t{i} add {kind}index idx(a)"),
                Vec::new(),
            );
        }));
    }
    for job in jobs {
        job.join().expect("concurrent fast-reorg job");
    }
    for i in 0..10 {
        assert_index(
            &store,
            "ingest_concurrent_fast_reorg",
            &format!("t{i}"),
            "idx",
        );
    }
}

/// `TestIssue55808`.
// 该用例覆盖 issue 55808。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_issue_55808() {
    let (_serial, store, mut tk) = prepare("ingest_issue_55808");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(a int primary key,b int)", Vec::new());
    tk.MustExec(
        "insert into t values (0,0),(10000,10000),(20000,20000),(30000,30000)",
        Vec::new(),
    );
    let _failure = testfailpoint::enable(DO_INGEST_FAILED, "return(true)");
    let error = tk.ExecToErr("alter table t add index idx(a)");
    assert!(error.message().contains("injected error"), "{error}");
    assert_no_index(&store, "ingest_issue_55808", "t", "idx");
}

/// `TestAddIndexBackfillLostTempIndexValues`.
// 该用例覆盖 添加 索引 backfill lost 临时索引 values。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_backfill_lost_temp_index_values() {
    let (_serial, store, mut tk) = prepare("ingest_lost_temp_values");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec(
        "create table t(id int primary key,b int not null default 0)",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,0)", Vec::new());
    let scan_store = store.clone();
    let scan_once = Arc::new(AtomicBool::new(false));
    let scan_flag = scan_once.clone();
    let _scan = testfailpoint::enable_call(BEFORE_ADD_INDEX_SCAN, move || {
        if !scan_flag.swap(true, Ordering::SeqCst) {
            let mut dml = session(scan_store.clone(), "ingest_lost_temp_values");
            dml.MustExec("insert into t values (2,0)", Vec::new());
            dml.MustExec("delete from t where id = 1", Vec::new());
            dml.MustExec("insert into t values (3,0)", Vec::new());
            dml.MustExec("delete from t where id = 2", Vec::new());
        }
    });
    let ingest_store = store.clone();
    let ingest_once = Arc::new(AtomicBool::new(false));
    let ingest_flag = ingest_once.clone();
    let _ingest = testfailpoint::enable_call(BEFORE_BACKEND_INGEST, move || {
        if !ingest_flag.swap(true, Ordering::SeqCst) {
            let mut dml = session(ingest_store.clone(), "ingest_lost_temp_values");
            dml.MustExec("insert into t(id) values (4)", Vec::new());
            dml.MustExec("delete from t where id = 3", Vec::new());
        }
    });
    let merge_store = store.clone();
    let merge_once = Arc::new(AtomicBool::new(false));
    let merge_flag = merge_once.clone();
    let _merge = testfailpoint::enable_call(BEFORE_BACKFILL_MERGE, move || {
        if !merge_flag.swap(true, Ordering::SeqCst) {
            session(merge_store.clone(), "ingest_lost_temp_values")
                .MustExec("insert into t values (3,0)", Vec::new());
        }
    });
    let _temp = testfailpoint::enable(SKIP_TEMP_REORG, "return(false)");
    assert_duplicate(tk.ExecToErr("alter table t add unique index idx(b)"), "'0'");
    assert!(scan_once.load(Ordering::SeqCst));
    assert!(ingest_once.load(Ordering::SeqCst));
    assert!(merge_once.load(Ordering::SeqCst));
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["3 0", "4 0"]));
    assert_no_index(&store, "ingest_lost_temp_values", "t", "idx");
}

/// `TestAddIndexInsertSameOriginIndexValue`.
// 该用例覆盖 添加 索引 insert same origin 索引 value。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_insert_same_origin_index_value() {
    let (_serial, store, mut tk) = prepare("ingest_same_origin");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec(
        "create table t(id int primary key,b int not null default 0)",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,0)", Vec::new());
    let ingest_store = store.clone();
    let ingest_once = Arc::new(AtomicBool::new(false));
    let ingest_flag = ingest_once.clone();
    let _ingest = testfailpoint::enable_call(BEFORE_BACKEND_INGEST, move || {
        if !ingest_flag.swap(true, Ordering::SeqCst) {
            let mut dml = session(ingest_store.clone(), "ingest_same_origin");
            dml.MustExec("delete from t where id = 1", Vec::new());
            dml.MustExec("insert into t values (1,0)", Vec::new());
        }
    });
    let duplicate_seen = Arc::new(AtomicBool::new(false));
    let duplicate_store = store.clone();
    let duplicate_flag = duplicate_seen.clone();
    let _merge = testfailpoint::enable_call(BEFORE_BACKFILL_MERGE, move || {
        if !duplicate_flag.swap(true, Ordering::SeqCst) {
            let error = session(duplicate_store.clone(), "ingest_same_origin")
                .ExecToErr("insert into t(id) values (1)");
            assert!(error.message().contains("Duplicate entry"), "{error}");
        }
    });
    tk.MustExec("alter table t add unique index idx(b)", Vec::new());
    assert!(ingest_once.load(Ordering::SeqCst));
    assert!(duplicate_seen.load(Ordering::SeqCst));
    assert_index(&store, "ingest_same_origin", "t", "idx");
}

/// `TestIngestConcurrentJobCleanupRace`.
// 该用例覆盖 ingest 回填 concurrent 任务 清理 race。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_ingest_concurrent_job_cleanup_race() {
    let (_serial, store, mut tk) = prepare("ingest_cleanup_race");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    for table in ["t1", "t2"] {
        tk.MustExec(
            &format!("create table {table}(a int primary key,b int)"),
            Vec::new(),
        );
        let values = (0..100)
            .map(|i| format!("({i},{i})"))
            .collect::<Vec<_>>()
            .join(",");
        tk.MustExec(&format!("insert into {table} values {values}"), Vec::new());
    }
    let barrier = Arc::new(Barrier::new(2));
    let callback_barrier = barrier.clone();
    let ingest_hits = Arc::new(AtomicUsize::new(0));
    let callback_hits = ingest_hits.clone();
    let _overlap = testfailpoint::enable_concurrent_call(BEFORE_BACKEND_INGEST, move || {
        callback_hits.fetch_add(1, Ordering::SeqCst);
        callback_barrier.wait();
    });
    let first_store = store.clone();
    let second_store = store.clone();
    let first = thread::spawn(move || {
        session(first_store, "ingest_cleanup_race")
            .MustExec("alter table t1 add index idx1(b)", Vec::new());
    });
    let second = thread::spawn(move || {
        session(second_store, "ingest_cleanup_race")
            .MustExec("alter table t2 add index idx2(b)", Vec::new());
    });
    first.join().expect("first cleanup-race job");
    second.join().expect("second cleanup-race job");
    assert_eq!(ingest_hits.load(Ordering::SeqCst), 2);
    assert_index(&store, "ingest_cleanup_race", "t1", "idx1");
    assert_index(&store, "ingest_cleanup_race", "t2", "idx2");
}

/// `TestIngestGCSafepointBlocking`.
// 该用例覆盖 ingest 回填 gc safepoint blocking。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_ingest_gc_safepoint_blocking() {
    let (_serial, store, mut tk) = prepare("ingest_gc_safepoint");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(a int primary key,b int)", Vec::new());
    let values = (0..100)
        .map(|i| format!("({i},{i})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t values {values}"), Vec::new());
    let (ts_hits, _ts) = hit_counter(BEGIN_ROLLBACK_START_TS);
    tk.MustExec("alter table t add index idx(b)", Vec::new());
    assert!(ts_hits.load(Ordering::SeqCst) > 0);
    tk.MustQuery("select count(*) from t", Vec::new())
        .Check(Rows(&["100"]));
    assert_index(&store, "ingest_gc_safepoint", "t", "idx");
}

/// `TestIngestCancelCleanupOrder`.
// 该用例覆盖 ingest 回填 取消 清理 order。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_ingest_cancel_cleanup_order() {
    let (_serial, store, mut tk) = prepare("ingest_cancel_cleanup");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(a int primary key,b int)", Vec::new());
    let values = (0..100)
        .map(|i| format!("({i},{i})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t values {values}"), Vec::new());
    let (delivery_hits, _delivery) = hit_counter(BEFORE_DELIVERY_JOB);
    let (backend_hits, _backend) = hit_counter(BEFORE_BACKEND_INGEST);
    let _cancel = testfailpoint::enable(BEFORE_EXECUTE_REGION_JOB, "return(true)");
    let error = tk.ExecToErr("alter table t add index idx(b)");
    assert!(error.message().contains("Cancelled DDL job"), "{error}");
    assert!(delivery_hits.load(Ordering::SeqCst) > 0);
    assert!(backend_hits.load(Ordering::SeqCst) > 0);
    assert_no_index(&store, "ingest_cancel_cleanup", "t", "idx");
    tk.MustExec("admin check table t", Vec::new());
}

/// `TestMergeTempIndexSplitConflictTxn`.
// 该用例覆盖 merge 临时索引 切分 conflict txn。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_merge_temp_index_split_conflict_txn() {
    let (_serial, store, mut tk) = prepare("ingest_merge_conflict");
    tk.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    tk.MustExec("create table t(a int primary key,b int)", Vec::new());
    let insert_store = store.clone();
    let insert_once = Arc::new(AtomicBool::new(false));
    let insert_flag = insert_once.clone();
    let _schema = testfailpoint::enable_call(AFTER_WAIT_SCHEMA, move || {
        if !insert_flag.swap(true, Ordering::SeqCst) {
            let mut dml = session(insert_store.clone(), "ingest_merge_conflict");
            for i in 0..4 {
                dml.MustExec(&format!("insert into t values ({i},{i})"), Vec::new());
            }
        }
    });
    let update_store = store.clone();
    let update_once = Arc::new(AtomicBool::new(false));
    let update_flag = update_once.clone();
    let _merge = testfailpoint::enable_call(MERGING_IN_TXN, move || {
        if !update_flag.swap(true, Ordering::SeqCst) {
            let mut dml = session(update_store.clone(), "ingest_merge_conflict");
            dml.MustExec("begin", Vec::new());
            for i in 0..4 {
                dml.MustExec(
                    &format!("update t set b = {} where a = {i}", i + 10),
                    Vec::new(),
                );
            }
            dml.MustExec("commit", Vec::new());
        }
    });
    tk.MustExec("alter table t add index idx(b)", Vec::new());
    assert!(insert_once.load(Ordering::SeqCst));
    assert!(update_once.load(Ordering::SeqCst));
    tk.MustQuery("select * from t order by a", Vec::new())
        .Check(Rows(&["0 10", "1 11", "2 12", "3 13"]));
    assert_index(&store, "ingest_merge_conflict", "t", "idx");
}

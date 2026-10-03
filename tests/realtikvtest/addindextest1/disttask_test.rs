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
// 中文总览：函数 `assert_index` 负责 断言 index。
// 中文总览：函数 `test_add_index_dist_basic` 负责 添加 索引 dist 基础场景。
// 中文总览：函数 `test_add_index_dist_auto_pause_on_kv_disk_full` 负责 添加 索引 dist 自动 暂停 on kv disk full。
// 中文总览：函数 `test_add_index_dist_cancel_with_partition` 负责 添加 索引 dist 取消 携带 partition。
// 中文总览：函数 `test_add_index_dist_cancel` 负责 添加 索引 dist 取消。
// 中文总览：函数 `test_add_index_dist_pause_and_resume` 负责 添加 索引 dist 暂停 and 恢复。
// 中文总览：函数 `test_add_index_invalid_dist_task_variable_setting` 负责 添加 索引 invalid dist task variable setting。
// 中文总览：函数 `test_add_index_for_current_timestamp_column` 负责 添加 索引 for 当前时间戳 列。
// 中文总览：函数 `test_add_uk_error_message` 负责 添加 uk 错误 信息。
// 中文总览：函数 `test_add_index_dist_lock_acquire_failed` 负责 添加 索引 dist lock acquire failed。

//! Distributed add-index integration coverage ported from `disttask_test.go`.
//!
//! Every DDL assertion below runs through the canonical `pkg/testkit` session
//! and `Domain`; failpoints are observed only when production ALTER execution
//! reaches the matching boundary.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use astersql_dxf_framework_handle as handle;
use astersql_dxf_framework_handle::{
    AccessStats, Context, Error, MeterItem, ObjectStorage, Runtime,
};
use astersql_dxf_framework_proto as proto;
use astersql_dxf_framework_storage as storage;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{NewTestKit, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;

const AFTER_RUN_SUBTASK: &str =
    "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterRunSubtask";
const DISK_FULL: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/WriteToTiKVNotEnoughDiskSpace";
const SUBTASK_FINISH: &str =
    "github.com/pingcap/tidb/pkg/ddl/mockDMLExecutionAddIndexSubTaskFinish";
const AFTER_BACKFILL_DONE: &str = "github.com/pingcap/tidb/pkg/ddl/afterBackfillStateRunningDone";
const AFTER_UPDATE_JOB: &str = "github.com/pingcap/tidb/pkg/ddl/afterUpdateJobToTable";
const BEFORE_EXECUTE_REGION_JOB: &str =
    "github.com/pingcap/tidb/pkg/ingestor/ingestctrl/beforeExecuteRegionJob";
const PAUSED_STATE: &str =
    "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/mockDMLExecutionOnPausedState";
const SYNC_TASK_PAUSE: &str = "github.com/pingcap/tidb/pkg/ddl/syncDDLTaskPause";
const PAUSE_AFTER_FINISH: &str = "github.com/pingcap/tidb/pkg/ddl/pauseAfterDistTaskFinished";
const AFTER_WAIT_SCHEMA: &str = "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced";
const ACQUIRE_DIST_LOCK_FAILED: &str =
    "github.com/pingcap/tidb/pkg/owner/mockAcquireDistLockFailed";
const NO_ENOUGH_SLOTS: &str =
    "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/mockNoEnoughSlots";
const AFTER_CANCEL_SUBTASK: &str =
    "github.com/pingcap/tidb/pkg/dxf/framework/taskexecutor/afterCancelSubtaskExec";
const CLEANUP_TASK: &str =
    "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/processCleanupTaskBatch";

// 该辅助函数负责 prepare。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn prepare(
    database: &str,
) -> (
    std::sync::MutexGuard<'static, ()>,
    Arc<astersql_testkit::mockstore::AnalyzeStatsStore>,
    TestKit,
) {
    let serial = astersql_tests_realtikvtest_addindextest1::serial_guard();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec(&format!("drop database if exists {database}"), Vec::new());
    tk.MustExec(&format!("create database {database}"), Vec::new());
    tk.MustExec(&format!("use {database}"), Vec::new());
    (serial, store, tk)
}

// 该辅助函数负责 断言 index。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

fn assert_index(
    store: &astersql_testkit::mockstore::AnalyzeStatsStore,
    database: &str,
    table: &str,
    index: &str,
) {
    let table_info = store
        .domain()
        .table_by_name(database, table)
        .unwrap_or_else(|error| panic!("load {database}.{table}: {error}"));
    assert!(
        table_info
            .Indices
            .iter()
            .any(|candidate| candidate.Name.L == index),
        "index {index} is absent from {database}.{table}"
    );
}

/// `TestAddIndexDistBasic`.
// 该用例覆盖 添加 索引 dist 基础场景。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_index_dist_basic() {
    let (_serial, store, mut tk) = prepare("dist_basic");
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    tk.MustExec("set global tidb_ddl_reorg_worker_cnt = 111", Vec::new());
    tk.MustExec(
        "create table t(a bigint auto_random primary key) partition by hash(a) partitions 20",
        Vec::new(),
    );
    for _ in 0..5 {
        tk.MustExec("insert into t values (), (), (), (), (), ()", Vec::new());
    }
    tk.MustExec("alter table t add index idx(a)", Vec::new());
    tk.MustExec("admin check table t", Vec::new());
    assert_index(&store, "dist_basic", "t", "idx");

    tk.MustExec(
        "create table t1(a bigint auto_random primary key)",
        Vec::new(),
    );
    for _ in 0..4 {
        tk.MustExec("insert into t1 values (), (), (), (), (), ()", Vec::new());
    }
    tk.MustExec("alter table t1 add index idx(a)", Vec::new());

    // The first completed subtask is cancelled by production and retried; the
    // successful ALTER proves the retry did not escape into test-side logic.
    let _cancel_once = testfailpoint::enable(AFTER_RUN_SUBTASK, "1*return(true)");
    tk.MustExec("alter table t1 add index idx1(a)", Vec::new());
    assert_index(&store, "dist_basic", "t1", "idx1");
    tk.MustExec("admin check table t1", Vec::new());
}

/// `TestAddIndexDistAutoPauseOnKVDiskFull`.
// 该用例覆盖 添加 索引 dist 自动 暂停 on kv disk full。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_index_dist_auto_pause_on_kv_disk_full() {
    let (_serial, store, mut tk) = prepare("dist_disk_full");
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());

    tk.MustExec("create table t(a int, b int)", Vec::new());
    tk.MustExec("insert into t values (1, 1), (2, 2), (3, 3)", Vec::new());
    {
        let _disk_full = testfailpoint::enable(DISK_FULL, "return(true)");
        let error = tk.ExecToErr("alter table t add index idx_b(b)");
        assert!(
            error.message().starts_with("[ddl:8200]") && error.message().contains("TiKV disk full"),
            "unexpected disk-full error: {error}"
        );
    }
    tk.MustExec("alter table t add index idx_b(b)", Vec::new());
    assert_index(&store, "dist_disk_full", "t", "idx_b");

    tk.MustExec("create table t_multi(a int, b int, c int)", Vec::new());
    tk.MustExec(
        "insert into t_multi values (1, 1, 1), (2, 2, 2), (3, 3, 3)",
        Vec::new(),
    );
    {
        let _disk_full = testfailpoint::enable(DISK_FULL, "return(true)");
        let error =
            tk.ExecToErr("alter table t_multi add column d int default 0, add index idx_b(b)");
        assert!(
            error.message().starts_with("[ddl:8200]") && error.message().contains("TiKV disk full"),
            "unexpected multi-schema disk-full error: {error}"
        );
    }
    tk.MustExec(
        "alter table t_multi add column d int default 0, add index idx_b(b)",
        Vec::new(),
    );
    assert_index(&store, "dist_disk_full", "t_multi", "idx_b");
    tk.MustQuery("select d from t_multi order by a", Vec::new())
        .Check(astersql_testkit::Rows(&["0", "0", "0"]));
}

/// `TestAddIndexDistCancelWithPartition`.
// 该用例覆盖 添加 索引 dist 取消 携带 partition。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_index_dist_cancel_with_partition() {
    let (_serial, store, mut tk) = prepare("dist_cancel_partition");
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    tk.MustExec(
        "create table t(a bigint auto_random primary key) partition by hash(a) partitions 8",
        Vec::new(),
    );
    for _ in 0..4 {
        tk.MustExec("insert into t values (), (), (), (), (), ()", Vec::new());
    }

    {
        let _cancel = testfailpoint::enable(SUBTASK_FINISH, "1*return(true)");
        let error = tk.ExecToErr("alter table t add index idx(a)");
        assert!(
            error.message().contains("Cancelled DDL job"),
            "unexpected cancellation error: {error}"
        );
    }
    tk.MustExec("admin check table t", Vec::new());
    tk.MustExec("alter table t add index idx2(a)", Vec::new());
    assert_index(&store, "dist_cancel_partition", "t", "idx2");
}

/// `TestAddIndexDistCancel`.
// 该用例覆盖 添加 索引 dist 取消。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_index_dist_cancel() {
    let (_serial, store, mut tk) = prepare("dist_cancel");
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    tk.MustExec("create table t(a int, b int)", Vec::new());
    tk.MustExec("insert into t values (1, 1), (2, 2), (3, 3)", Vec::new());
    tk.MustExec("create table t2(a int, b int)", Vec::new());
    tk.MustExec("insert into t2 values (1, 1), (2, 2), (3, 3)", Vec::new());

    let backfill_hits = Arc::new(AtomicUsize::new(0));
    let hook_hits = Arc::clone(&backfill_hits);
    let _backfill = testfailpoint::enable_call(AFTER_BACKFILL_DONE, move || {
        hook_hits.fetch_add(1, Ordering::SeqCst);
    });
    let _update_once = testfailpoint::enable(AFTER_UPDATE_JOB, "1*return(true)");
    let store2 = Arc::clone(&store);
    let second = thread::spawn(move || {
        let mut tk2 = NewTestKit(store2);
        tk2.MustExec("create database if not exists dist_cancel", Vec::new());
        tk2.MustExec("use dist_cancel", Vec::new());
        tk2.MustExec("alter table t2 add index idx_b(b)", Vec::new());
    });
    tk.MustExec("alter table t add index idx(a)", Vec::new());
    second.join().expect("second add-index worker");
    assert!(backfill_hits.load(Ordering::SeqCst) >= 2);
    assert_index(&store, "dist_cancel", "t", "idx");
    assert_index(&store, "dist_cancel", "t2", "idx_b");

    let cancel = testfailpoint::enable_pause(BEFORE_EXECUTE_REGION_JOB);
    let store2 = Arc::clone(&store);
    let alter = thread::spawn(move || {
        let mut tk2 = NewTestKit(store2);
        tk2.MustExec("use dist_cancel", Vec::new());
        tk2.ExecToErr("alter table t add index idx_ba(b, a)")
    });
    cancel.wait_until_reached();
    let jobs = tk.MustQuery("admin show ddl jobs", Vec::new()).Rows();
    let job_id = jobs
        .iter()
        .find(|row| row[1] == "dist_cancel" && row[2] == "t" && row[9] == "running")
        .map(|row| row[0].clone())
        .expect("running ADD INDEX job");
    let started = Instant::now();
    tk.MustExec(&format!("admin cancel ddl jobs {job_id}"), Vec::new());
    cancel.resume();
    let error = alter.join().expect("cancelled add-index worker");
    assert!(
        error.message().contains("Cancelled DDL job"),
        "unexpected timely-cancel error: {error}"
    );
    assert!(started.elapsed() < Duration::from_secs(20));
}

/// `TestAddIndexDistPauseAndResume`.
// 该用例覆盖 添加 索引 dist 暂停 and 恢复。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_index_dist_pause_and_resume() {
    let (_serial, store, mut setup) = prepare("dist_pause");
    setup.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    setup.MustExec("create table t(a int, b int)", Vec::new());
    setup.MustExec("insert into t values (1, 1), (2, 2), (3, 3)", Vec::new());

    let state = Arc::new((Mutex::new((0usize, 0usize)), Condvar::new()));
    let callback_state = Arc::clone(&state);
    let _finish = testfailpoint::enable_call(SUBTASK_FINISH, move || {
        let (lock, changed) = &*callback_state;
        let mut counts = lock.lock().expect("pause state poisoned");
        counts.0 += 1;
        let generation = counts.0;
        changed.notify_all();
        drop(
            changed
                .wait_while(counts, |counts| counts.1 < generation)
                .expect("pause state poisoned"),
        );
    });
    let paused_hits = Arc::new(AtomicUsize::new(0));
    let paused_hook_hits = Arc::clone(&paused_hits);
    let _paused = testfailpoint::enable_call(PAUSED_STATE, move || {
        paused_hook_hits.fetch_add(1, Ordering::SeqCst);
    });
    let sync_hits = Arc::new(AtomicUsize::new(0));
    let sync_hook_hits = Arc::clone(&sync_hits);
    let _sync = testfailpoint::enable_call(SYNC_TASK_PAUSE, move || {
        sync_hook_hits.fetch_add(1, Ordering::SeqCst);
    });

    let worker_store = Arc::clone(&store);
    let worker = thread::spawn(move || {
        let mut tk = NewTestKit(worker_store);
        tk.MustExec("create database if not exists dist_pause", Vec::new());
        tk.MustExec("use dist_pause", Vec::new());
        tk.MustExec("alter table t add index idx1(a)", Vec::new());
    });
    for expected in 1..=3 {
        let (lock, changed) = &*state;
        let counts = lock.lock().expect("pause state poisoned");
        let mut counts = changed
            .wait_while(counts, |counts| counts.0 < expected)
            .expect("pause state poisoned");
        counts.1 = expected;
        changed.notify_all();
    }
    worker.join().expect("pause/resume add-index worker");
    assert_eq!(paused_hits.load(Ordering::SeqCst), 3);
    assert_eq!(sync_hits.load(Ordering::SeqCst), 3);
    assert_index(&store, "dist_pause", "t", "idx1");
    drop(_finish);
    drop(_paused);
    drop(_sync);
    // The fail crate can retain an in-flight callback action after its RAII
    // guard is dropped. Remove the three production hooks explicitly before
    // the second ALTER so it cannot enter the previous generation barrier.
    testfailpoint::disable(SUBTASK_FINISH);
    testfailpoint::disable(PAUSED_STATE);
    testfailpoint::disable(SYNC_TASK_PAUSE);

    let after_schema = Arc::new(AtomicUsize::new(0));
    let after_schema_hook = Arc::clone(&after_schema);
    let _after_schema = testfailpoint::enable_call(AFTER_WAIT_SCHEMA, move || {
        after_schema_hook.fetch_add(1, Ordering::SeqCst);
    });
    let after_finish = Arc::new(AtomicUsize::new(0));
    let after_finish_hook = Arc::clone(&after_finish);
    let _pause_after_finish = testfailpoint::enable_call(PAUSE_AFTER_FINISH, move || {
        after_finish_hook.fetch_add(1, Ordering::SeqCst);
    });
    setup.MustExec("alter table t add unique index idx3(b)", Vec::new());
    assert!(after_schema.load(Ordering::SeqCst) > 0);
    assert_eq!(after_finish.load(Ordering::SeqCst), 1);
    assert_index(&store, "dist_pause", "t", "idx3");
}

/// `TestAddIndexInvalidDistTaskVariableSetting`.
// 该用例覆盖 添加 索引 invalid dist task variable setting。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_index_invalid_dist_task_variable_setting() {
    let (_serial, store, mut tk) = prepare("dist_invalid");
    tk.MustExec("set global tidb_ddl_enable_fast_reorg = off", Vec::new());
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    tk.MustExec("create table t(a int)", Vec::new());

    for sql in [
        "alter table t add index idx(a)",
        "alter table t add column b int, add index idx(a)",
    ] {
        let error = tk.ExecToErr(sql);
        assert!(
            error.message().starts_with("[ddl:8200]"),
            "sql={sql:?}, expected error 8200, got {error}"
        );
    }
    tk.MustExec(
        "alter table t add column b int, add column c int",
        Vec::new(),
    );
    let table = store.domain().table_by_name("dist_invalid", "t").unwrap();
    assert!(table.Columns.iter().any(|column| column.Name.L == "b"));
    assert!(table.Columns.iter().any(|column| column.Name.L == "c"));
}

/// `TestAddIndexForCurrentTimestampColumn`.
// 该用例覆盖 添加 索引 for 当前时间戳 列。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_index_for_current_timestamp_column() {
    let (_serial, store, mut tk) = prepare("dist_timestamp");
    tk.MustExec("set global tidb_ddl_enable_fast_reorg = on", Vec::new());
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    tk.MustExec(
        "create table t(a timestamp default current_timestamp)",
        Vec::new(),
    );
    tk.MustExec("insert into t values ()", Vec::new());
    tk.MustExec("alter table t add index idx(a)", Vec::new());
    tk.MustExec("admin check table t", Vec::new());
    assert_index(&store, "dist_timestamp", "t", "idx");
}

/// `TestAddUKErrorMessage`.
// 该用例覆盖 添加 uk 错误 信息。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_uk_error_message() {
    let (_serial, _store, mut tk) = prepare("dist_uk");
    tk.MustExec("set global tidb_ddl_enable_fast_reorg = on", Vec::new());
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    tk.MustExec("create table t(a int primary key, b int)", Vec::new());
    tk.MustExec(
        "insert into t values (5, 1), (10005, 1), (20005, 1), (30005, 1)",
        Vec::new(),
    );
    let error = tk.ExecToErr("alter table t add unique index uk(b)");
    assert!(
        error
            .message()
            .contains("Duplicate entry '1' for key 't.uk'"),
        "unexpected duplicate-key error: {error}"
    );
}

/// `TestAddIndexDistLockAcquireFailed`.
// 该用例覆盖 添加 索引 dist lock acquire failed。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_index_dist_lock_acquire_failed() {
    let (_serial, store, mut tk) = prepare("dist_lock_retry");
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    for (sequence, injected_error) in [
        ("lease", "requested lease not found"),
        ("compacted", "mvcc: required revision has been compacted"),
    ] {
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec("create table t(a int, b int)", Vec::new());
        tk.MustExec("insert into t values (1, 1)", Vec::new());
        let expression = format!("1*return({injected_error:?})");
        let _lock_error = testfailpoint::enable(ACQUIRE_DIST_LOCK_FAILED, &expression);
        tk.MustExec(
            &format!("alter table t add index idx_{sequence}(b)"),
            Vec::new(),
        );
        assert_index(&store, "dist_lock_retry", "t", &format!("idx_{sequence}"));
    }
}

/// `TestAddIndexScheduleAway`.
// 该用例覆盖 添加 索引 schedule away。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_index_schedule_away() {
    let (_serial, store, mut tk) = prepare("dist_schedule");
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());
    tk.MustExec("create table t(a int, b int)", Vec::new());
    tk.MustExec("insert into t values (1, 1)", Vec::new());

    let cancelled = Arc::new(AtomicUsize::new(0));
    let cancelled_hook = Arc::clone(&cancelled);
    let _after_cancel = testfailpoint::enable_call(AFTER_CANCEL_SUBTASK, move || {
        cancelled_hook.fetch_add(1, Ordering::SeqCst);
    });
    let _no_slots = testfailpoint::enable(NO_ENOUGH_SLOTS, "1*return(true)");
    tk.MustExec("alter table t add index idx(b)", Vec::new());
    assert_eq!(cancelled.load(Ordering::SeqCst), 1);
    assert_index(&store, "dist_schedule", "t", "idx");
}

/// `TestAddIndexDistCleanUpBlock`.
// 该用例覆盖 添加 索引 dist clean up block。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_add_index_dist_clean_up_block() {
    let (_serial, store, mut tk) = prepare("dist_cleanup");
    tk.MustExec("set global tidb_enable_dist_task = on", Vec::new());

    let gate = Arc::new((Mutex::new((0usize, false)), Condvar::new()));
    let callback_gate = Arc::clone(&gate);
    let _cleanup = testfailpoint::enable_call(CLEANUP_TASK, move || {
        let (lock, changed) = &*callback_gate;
        let mut state = lock.lock().expect("cleanup gate poisoned");
        state.0 += 1;
        changed.notify_all();
        drop(
            changed
                .wait_while(state, |state| !state.1)
                .expect("cleanup gate poisoned"),
        );
    });

    let mut workers = Vec::new();
    for index in 0..4 {
        let worker_store = Arc::clone(&store);
        workers.push(thread::spawn(move || {
            let mut worker = NewTestKit(worker_store);
            worker.MustExec("create database if not exists dist_cleanup", Vec::new());
            worker.MustExec("use dist_cleanup", Vec::new());
            worker.MustExec(&format!("create table t{index}(a int, b int)"), Vec::new());
            worker.MustExec(&format!("insert into t{index} values (1, 1)"), Vec::new());
            worker.MustExec(
                &format!("alter table t{index} add index idx(b)"),
                Vec::new(),
            );
        }));
    }
    let (lock, changed) = &*gate;
    let state = lock.lock().expect("cleanup gate poisoned");
    let mut state = changed
        .wait_while(state, |state| state.0 < 4)
        .expect("cleanup gate poisoned");
    state.1 = true;
    changed.notify_all();
    drop(state);
    for worker in workers {
        worker.join().expect("cleanup-block add-index worker");
    }
    for index in 0..4 {
        assert_index(&store, "dist_cleanup", &format!("t{index}"), "idx");
    }
}

// 该类型围绕 CloudRuntime 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

#[derive(Clone)]
struct CloudRuntime {
    uri: String,
    cluster_id: Option<u64>,
}

impl Runtime for CloudRuntime {
    // 该辅助函数负责 读取 cpu count of node。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_cpu_count_of_node(&self, _ctx: &Context) -> handle::Result<i32> {
        Ok(1)
    }

    // 该辅助函数负责 读取 task by 键 携带 history。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_task_by_key_with_history(
        &self,
        _ctx: &Context,
        _key: &str,
    ) -> handle::Result<Option<proto::Task>> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 创建 task。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn create_task(
        &self,
        _ctx: &Context,
        _key: &str,
        _task_type: proto::TaskType,
        _keyspace: &str,
        _required_slots: i32,
        _target_scope: &str,
        _max_node_count: i32,
        _extra_params: proto::ExtraParams,
        _meta: Vec<u8>,
    ) -> handle::Result<i64> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 读取 task by id。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_task_by_id(&self, _ctx: &Context, _id: i64) -> handle::Result<proto::Task> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 读取 task by id 携带 history。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_task_by_id_with_history(&self, _ctx: &Context, _id: i64) -> handle::Result<proto::Task> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 读取 task base by id 携带 history。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_task_base_by_id_with_history(
        &self,
        _ctx: &Context,
        _id: i64,
    ) -> handle::Result<proto::TaskBase> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 读取 task by 键。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_task_by_key(&self, _ctx: &Context, _key: &str) -> handle::Result<Option<proto::Task>> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 取消 task。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn cancel_task(&self, _ctx: &Context, _id: i64) -> handle::Result<()> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 暂停 task。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn pause_task(&self, _ctx: &Context, _key: &str) -> handle::Result<bool> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 恢复 task。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn resume_task(&self, _ctx: &Context, _key: &str) -> handle::Result<bool> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 读取 task bases in states。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_task_bases_in_states(
        &self,
        _ctx: &Context,
        _states: &[proto::TaskState],
    ) -> handle::Result<Vec<proto::TaskBase>> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 读取 all nodes。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_all_nodes(&self, _ctx: &Context) -> handle::Result<Vec<proto::ManagedNode>> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 读取 busy nodes。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_busy_nodes(&self, _ctx: &Context) -> handle::Result<Vec<handle::schstatus::Node>> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 owner exec id。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn owner_exec_id(&self, _ctx: &Context) -> handle::Result<String> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 读取 active task 摘要。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_active_task_summary(
        &self,
        _ctx: &Context,
    ) -> handle::Result<storage::ActiveTaskSummary> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 list history tasks。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn list_history_tasks(
        &self,
        _ctx: &Context,
        _page_size: i32,
        _page_token: i64,
        _keyspace: &str,
    ) -> handle::Result<storage::HistoryTaskPage> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 local cpu count。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn local_cpu_count(&self) -> i32 {
        1
    }

    // 该辅助函数负责 update 暂停 scale in flag。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn update_pause_scale_in_flag(
        &self,
        _ctx: &Context,
        _flag: &handle::schstatus::TTLFlag,
    ) -> handle::Result<()> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 读取 pause scale in flag。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_pause_scale_in_flag(
        &self,
        _ctx: &Context,
    ) -> handle::Result<Option<handle::schstatus::TTLFlag>> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 读取 schedule tune factors。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn get_schedule_tune_factors(
        &self,
        _ctx: &Context,
        _keyspace: &str,
    ) -> handle::Result<Option<handle::schstatus::TTLTuneFactors>> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 is next gen。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn is_next_gen(&self) -> bool {
        false
    }

    // 该辅助函数负责 service scope。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn service_scope(&self) -> String {
        String::new()
    }

    // 该辅助函数负责 云存储 storage uri。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn cloud_storage_uri(&self) -> String {
        self.uri.clone()
    }

    // 该辅助函数负责 sem enabled。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn sem_enabled(&self) -> bool {
        false
    }

    // 该辅助函数负责 集群 id。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn cluster_id(&self, _ctx: &Context) -> Option<u64> {
        self.cluster_id
    }

    // 该辅助函数负责 创建 object store。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn new_object_store(
        &self,
        _ctx: &Context,
        _uri: &str,
        _recording: Option<Arc<AccessStats>>,
    ) -> handle::Result<Arc<dyn ObjectStorage>> {
        Err(Error::new("unused"))
    }

    // 该辅助函数负责 write meter 数据。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn write_meter_data(
        &self,
        _ctx: &Context,
        _timestamp: i64,
        _key: &str,
        _item: &MeterItem,
    ) -> handle::Result<()> {
        Err(Error::new("unused"))
    }
}

// 该类型围绕 RuntimeRestore 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。

struct RuntimeRestore(Option<Arc<dyn Runtime>>);

impl Drop for RuntimeRestore {
    // 该辅助函数负责 收尾删除。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。

    fn drop(&mut self) {
        handle::ClearRuntime();
        if let Some(previous) = self.0.take() {
            handle::InstallRuntime(previous);
        }
    }
}

/// `TestUseClusterIdInGlobalSortPath`.
// 该用例覆盖 使用 集群 id in 全局排序 path。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。

#[test]
fn test_use_cluster_id_in_global_sort_path() {
    let _serial = astersql_tests_realtikvtest_addindextest1::serial_guard();
    let uri = "s3://bucket/path/to/folder?access-key=aaaaa&secret-access-key=bbbbb&endpoint=http://abc.com&force-path-style=false&region=Beijing&provider=aws";
    let previous = handle::InstallRuntime(Arc::new(CloudRuntime {
        uri: uri.to_owned(),
        cluster_id: Some(42),
    }));
    let _restore = RuntimeRestore(previous);
    let context = Context::background();
    assert_eq!(
        handle::GetCloudStorageURI(&context).unwrap(),
        "s3://bucket/path/to/folder/dxf/42/?access-key=aaaaa&secret-access-key=bbbbb&endpoint=http://abc.com&force-path-style=false&region=Beijing&provider=aws"
    );

    handle::InstallRuntime(Arc::new(CloudRuntime {
        uri: uri.to_owned(),
        cluster_id: None,
    }));
    assert_eq!(
        handle::GetCloudStorageURI(&context).unwrap(),
        "s3://bucket/path/to/folder/dxf/?access-key=aaaaa&secret-access-key=bbbbb&endpoint=http://abc.com&force-path-style=false&region=Beijing&provider=aws"
    );
}

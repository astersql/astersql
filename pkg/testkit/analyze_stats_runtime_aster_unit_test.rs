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

// ANALYZE / 统计信息运行时规范行为的单元测试。
//
// 覆盖：`SHOW STATS_META` 与 Domain 统计上下文共享状态、ANALYZE 预刷盘
// （preflush）在取消/注入错误下的语义、动态/静态分区剪枝模式下的刷盘目标，
// 以及 SQL Killer 中断 pause failpoint 与并行 build worker 时不泄漏 active 计数。

use crate::mockstore::CreateAnalyzeStatsStore;
use crate::{DbValue, TestKit};
use astersql_session::testutil::TestSession;

/// INSERT 后 modify_count 上升；ANALYZE 后与 SHOW STATS_META 一致且历史为空。
#[test]
fn analyze_and_show_stats_share_canonical_session_state() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store.clone());
    testkit.MustExec("create table t(a int)", Vec::new());
    let context = testkit
        .AnalyzeStatsContext()
        .expect("shared Domain statistics context");
    let table_id = context.table("test", "t").unwrap().table_id;
    let insert = testkit
        .Exec(
            "insert into t values (?), (?), (?)",
            vec![DbValue::I64(1), DbValue::I64(2), DbValue::I64(3)],
        )
        .expect("prepared INSERT through ConcreteSession");
    assert_eq!(insert.affected_rows, 3);
    let changed = context.table("test", "t").unwrap();
    assert_eq!(changed.row_count, 3);
    assert_eq!(changed.modify_count, 3);
    // ANALYZE 前 SHOW STATS_META 仍为未分析占位。
    testkit
        .MustQuery("show stats_meta where table_name = 't'", Vec::new())
        .Check(vec![vec![
            "test",
            "t",
            "",
            "1970-01-01 00:00:00",
            "0",
            "0",
            "<nil>",
        ]]);
    testkit.MustExec("analyze table t", Vec::new());
    testkit
        .MustQuery("show stats_meta", Vec::new())
        .Check(vec![vec![
            "test",
            "t",
            "",
            "1970-01-01 00:00:00",
            "0",
            "3",
            "1970-01-01 00:00:00",
        ]]);

    let analyzed = context.table("test", "t").unwrap();
    assert_eq!(analyzed.row_count, 3);
    assert_eq!(analyzed.modify_count, 0);
    assert!(analyzed.version > 0);
    let history = context.history(table_id);
    assert!(history.is_empty());
}

/// 取消与本地 preflush 注入错误均不得刷盘；成功路径才递增 preflush_count。
#[test]
fn canonical_preflush_uses_cancelled_statement_context_and_local_handle() {
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    session.Execute("create table preflush_t(a int)").unwrap();
    session
        .Execute("insert into preflush_t values (1), (2)")
        .unwrap();
    let context = domain.stats_context();
    let table_id = context.table("test", "preflush_t").unwrap().table_id;
    assert_eq!(context.table("test", "preflush_t").unwrap().row_count, 2);
    assert_eq!(
        context
            .persisted_physical_stats(table_id)
            .unwrap()
            .realtime_count,
        0
    );
    assert_eq!(context.pending_stats_delta_ids(), vec![table_id]);
    assert_eq!(context.preflush_count(), 0);

    let error = session
        .ExecuteCancelledAnalyzeForTest("analyze table preflush_t")
        .unwrap_err();
    assert!(error.to_string().contains("context canceled"));
    assert_eq!(context.preflush_count(), 0);
    assert_eq!(context.pending_stats_delta_ids(), vec![table_id]);
    assert_eq!(
        context
            .persisted_physical_stats(table_id)
            .unwrap()
            .realtime_count,
        0
    );
    assert!(context.analyze_jobs().is_empty());

    // 注入本地 delta flush 失败，ANALYZE 应中止且不计入 preflush。
    domain.inject_stats_preflush_error_for_test("local stats delta flush failed");
    let error = match session.Execute("analyze table preflush_t") {
        Ok(_) => panic!("injected local preflush error must abort ANALYZE"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("local stats delta flush failed"));
    assert_eq!(context.preflush_count(), 0);
    assert_eq!(context.pending_stats_delta_ids(), vec![table_id]);
    assert_eq!(
        context
            .persisted_physical_stats(table_id)
            .unwrap()
            .realtime_count,
        0
    );
    assert!(context.analyze_jobs().is_empty());

    // 清除失败注入后正常 ANALYZE：刷盘一次并清空 pending delta。
    session.Execute("analyze table preflush_t").unwrap();
    assert_eq!(context.preflush_count(), 1);
    assert!(context.pending_stats_delta_ids().is_empty());
    assert_eq!(context.last_preflush_ids(), vec![table_id]);
    assert_eq!(context.physical_stats(table_id).unwrap().realtime_count, 2);
    assert_eq!(context.analyze_jobs().len(), 1);
}

/// 动态剪枝刷全局+分区；静态剪枝下 pending 仅目标分区，ANALYZE 仍刷全量 ID 集。
#[test]
fn canonical_preflush_targets_dynamic_global_and_static_partitions() {
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    session
        .Execute("create table target_t(a int) partition by hash(a) partitions 3")
        .unwrap();
    session
        .Execute("insert into target_t values (1), (2), (3)")
        .unwrap();
    let context = domain.stats_context();
    let catalog = context.catalog();
    let (key, table) = catalog
        .get(&("test".to_owned(), "target_t".to_owned()))
        .unwrap();
    let mut partitions = table
        .GetPartitionInfo()
        .unwrap()
        .Definitions
        .iter()
        .map(|partition| partition.ID)
        .collect::<Vec<_>>();
    partitions.sort_unstable();

    session.Execute("analyze table target_t").unwrap();
    let mut dynamic_ids = partitions.clone();
    dynamic_ids.push(key.table_id);
    dynamic_ids.sort_unstable();
    assert_eq!(context.last_preflush_ids(), dynamic_ids.clone());

    // 切到静态分区剪枝后，单行 INSERT 的 pending 仅为命中分区。
    session
        .Execute("set @@tidb_partition_prune_mode = 'static'")
        .unwrap();
    session.Execute("insert into target_t values (4)").unwrap();
    let target_partition =
        table.GetPartitionInfo().unwrap().Definitions[4_usize % partitions.len()].ID;
    assert_eq!(context.pending_stats_delta_ids(), vec![target_partition]);
    assert_eq!(
        context
            .persisted_physical_stats(target_partition)
            .unwrap()
            .realtime_count,
        1
    );
    session.Execute("analyze table target_t").unwrap();
    assert_eq!(context.last_preflush_ids(), dynamic_ids);
    assert!(context.pending_stats_delta_ids().is_empty());
    assert_eq!(
        context
            .persisted_physical_stats(target_partition)
            .unwrap()
            .realtime_count,
        2
    );
}

/// failpoint 暂停 ANALYZE 时发送 Kill：统计快照不变，失败 job 记入历史。
#[test]
fn sql_killer_interrupts_failpoint_paused_analyze() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store.clone());
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec("create table t(a int)", Vec::new());
    testkit.MustExec("insert into t values (1)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    testkit.MustExec("insert into t values (2)", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t").unwrap().table_id;
    let current_before = context.physical_stats(table_id).unwrap();
    let history_before = context.history(table_id);
    let jobs_before = context.analyze_jobs();
    let killer = store.sql_killer();
    // 后台线程跑 ANALYZE，主线程等到 pause 点后发 Kill。
    let pause = astersql_session::runtime::EnableAnalyzePauseForTest(&killer);
    let worker = std::thread::spawn(move || testkit.Exec("analyze table t", Vec::new()));
    pause.wait_until_reached();
    store.sql_killer().SendKillSignal(1);
    drop(pause);
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.message().contains("Query execution was interrupted"));
    assert_eq!(context.physical_stats(table_id).unwrap(), current_before);
    assert_eq!(context.history(table_id), history_before);
    let jobs_after = context.analyze_jobs();
    assert_eq!(jobs_after.len(), jobs_before.len() + 1);
    let failed = jobs_after.last().unwrap();
    assert_eq!(failed.database, "test");
    assert_eq!(failed.table, "t");
    assert_eq!(failed.state, "failed");
    assert_eq!(
        failed.fail_reason.as_deref(),
        Some("[executor:1317]Query execution was interrupted")
    );
}

/// 并行 build 被 Kill 后 active_workers_after 归零，统计快照不被部分更新。
#[test]
fn sql_killer_cancels_parallel_build_workers_without_leaking_active_count() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store.clone());
    testkit.MustExec(
        "create table t(a int) partition by hash(a) partitions 4",
        Vec::new(),
    );
    let values = (0..100)
        .map(|value| format!("({value})"))
        .collect::<Vec<_>>()
        .join(",");
    testkit.MustExec(&format!("insert into t values {values}"), Vec::new());
    testkit.MustExec("set @@tidb_build_stats_concurrency = 4", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t").unwrap().table_id;
    let current_before = context.physical_stats(table_id).unwrap();
    let history_before = context.history(table_id);
    let killer = store.sql_killer();
    let pause = astersql_session::runtime::EnableAnalyzeBuildPauseForTest(&killer);
    let worker = std::thread::spawn(move || testkit.Exec("analyze table t", Vec::new()));
    pause.wait_until_reached();
    killer.SendKillSignal(1);
    drop(pause);
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.message().contains("Query execution was interrupted"));
    assert_eq!(context.physical_stats(table_id).unwrap(), current_before);
    assert_eq!(context.history(table_id), history_before);
    let jobs = context.analyze_jobs();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].state, "failed");
    assert!(jobs[0].max_concurrency > 1);
    assert_eq!(jobs[0].active_workers_after, 0);
}

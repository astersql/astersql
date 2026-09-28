// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 历史统计信息（historical stats）持久化、回退与 GC 的集成测试。
//
// 历史统计：将 ANALYZE / flush stats 产生的表级元数据与可选 JSON dump
// 按版本保存，供事后诊断或按时间点恢复统计快照；
// GC 负责清理已删表或过期版本。

#![allow(non_snake_case)]

use std::time::Duration;

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;

/// 使用可记录 ANALYZE 统计的 mock store 创建 TestKit。
fn historical_testkit() -> TestKit {
    TestKit::new(CreateAnalyzeStatsStore())
}

#[test]
/// 开启历史统计后，ANALYZE 应写入 meta 与可解码的 JSON dump。
fn TestRecordHistoryStatsAfterAnalyze() {
    let mut testkit = historical_testkit();
    testkit.MustExec(
        "create table t(a int, b varchar(10), index idx(a,b))",
        Vec::new(),
    );
    testkit.MustExec("insert into t values(1,'a'),(2,'b')", Vec::new());
    testkit.MustExec("analyze table t with 2 topn", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t").unwrap().table_id;
    assert!(context.history(table_id).is_empty());

    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec("analyze table t with 2 topn", Vec::new());
    let history = context.history(table_id);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].source, "analyze");
    assert_eq!((history[0].modify_count, history[0].row_count), (0, 2));
    let snapshot = context
        .historical_snapshot(table_id, history[0].version)
        .unwrap();
    assert!(snapshot.is_historical);
    assert_eq!((snapshot.modify_count, snapshot.row_count), (0, 2));
    let blocks = context
        .historical_json_blocks(table_id, history[0].version)
        .expect("enabled ANALYZE must persist the explicit historical dump");
    let (source, decoded) = context.decode_historical_json_blocks(&blocks).unwrap();
    assert_eq!(source, "analyze");
    assert_eq!(decoded, context.physical_stats(table_id).unwrap());
}

#[test]
/// flush stats_delta 在开启后记录 meta；delete/update 正确累加 modify/row count。
fn TestRecordHistoryStatsMetaAfterAnalyze() {
    let mut testkit = historical_testkit();
    testkit.MustExec("create table t(a int, b int)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t").unwrap().table_id;

    for _ in 0..5 {
        testkit.MustExec("insert into t values(1,1),(2,2),(3,3)", Vec::new());
        testkit.MustExec("flush stats_delta t", Vec::new());
    }
    assert!(context.history(table_id).is_empty());

    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    for _ in 0..5 {
        testkit.MustExec("insert into t values(1,1),(2,2),(3,3)", Vec::new());
        testkit.MustExec("flush stats_delta t", Vec::new());
    }
    let insert = context.history(table_id);
    assert_eq!(insert.len(), 5);
    assert!(insert.iter().all(|history| history.source == "flush stats"));
    assert_eq!(
        insert
            .iter()
            .map(|history| (history.modify_count, history.row_count))
            .collect::<Vec<_>>(),
        [(18, 18), (21, 21), (24, 24), (27, 27), (30, 30)]
    );

    testkit.MustExec("delete from t where a = 1", Vec::new());
    testkit.MustExec("flush stats_delta t", Vec::new());
    let deleted = context.history(table_id);
    assert_eq!((deleted[5].modify_count, deleted[5].row_count), (40, 20));

    testkit.MustExec("update t set b = 9 where a = 2", Vec::new());
    testkit.MustExec("flush stats_delta t", Vec::new());
    let updated = context.history(table_id);
    assert_eq!((updated[6].modify_count, updated[6].row_count), (50, 20));
    assert!(
        updated
            .windows(2)
            .all(|pair| pair[0].version < pair[1].version)
    );
}

#[test]
/// DROP TABLE 后 GC 应清除该表历史与物理统计。
fn TestGCHistoryStatsAfterDropTable() {
    let mut testkit = historical_testkit();
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec("create table t(a int)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t").unwrap().table_id;
    assert_eq!(context.history(table_id).len(), 1);

    testkit.MustExec("drop table t", Vec::new());
    assert_eq!(context.history(table_id).len(), 1);
    context
        .gc_dropped_stats()
        .expect("gc dropped historical stats");
    assert!(context.history(table_id).is_empty());
    assert!(context.physical_stats(table_id).is_none());
}

#[test]
/// ALTER 改表后历史 dump 保持不变，物理统计反映当前 schema。
fn TestAssertHistoricalStatsAfterAlterTable() {
    let mut testkit = historical_testkit();
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec(
        "create table t(a int, b varchar(10), c int, index idx(c))",
        Vec::new(),
    );
    testkit.MustExec("insert into t values(1,'a',2)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t").unwrap().table_id;
    let history = context.history(table_id);
    let blocks = context
        .historical_json_blocks(table_id, history[0].version)
        .unwrap();
    let (_, original) = context.decode_historical_json_blocks(&blocks).unwrap();

    testkit.MustExec("alter table t drop column b", Vec::new());
    testkit.MustExec("alter table t drop index idx", Vec::new());
    let current = context.physical_stats(table_id).unwrap();
    assert_eq!(current.columns.len(), 2);
    assert!(current.indexes.is_empty());
    assert_eq!(context.history(table_id), history);
    assert_eq!(
        context.decode_historical_json_blocks(&blocks).unwrap().1,
        original
    );
}

#[test]
/// 按存活时长 GC 过期历史，不影响当前物理统计。
fn TestGCOutdatedHistoryStats() {
    let mut testkit = historical_testkit();
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec("create table t(a int)", Vec::new());
    testkit.MustExec("insert into t values(1)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t").unwrap().table_id;
    let current = context.physical_stats(table_id).unwrap();
    assert_eq!(
        context.gc_historical_stats_older_than(Duration::from_secs(86_400)),
        0
    );
    std::thread::sleep(Duration::from_millis(2));
    assert_eq!(context.gc_historical_stats_older_than(Duration::ZERO), 1);
    assert!(context.history(table_id).is_empty());
    assert_eq!(context.physical_stats(table_id).unwrap(), current);
}

#[test]
/// 动态裁剪下分区表 ANALYZE 同步写入全局与各分区历史。
/// 分区：将大表按键切分为独立物理表，便于裁剪与并行。
fn TestPartitionTableHistoricalStats() {
    let mut testkit = historical_testkit();
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec(
        "create table t(a int, b int, index idx(b)) partition by hash(a) partitions 2",
        Vec::new(),
    );
    testkit.MustExec("insert into t values(1,1),(2,2)", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t").unwrap().table_id;
    let catalog = context.catalog();
    let partitions = &catalog
        .get(&("test".to_owned(), "t".to_owned()))
        .unwrap()
        .1
        .GetPartitionInfo()
        .unwrap()
        .Definitions;
    let version = context.history(table_id)[0].version;
    assert!(partitions.iter().all(|partition| {
        let history = context.history(partition.ID);
        history.len() == 1 && history[0].version == version
    }));
    for id in std::iter::once(table_id).chain(partitions.iter().map(|partition| partition.ID)) {
        let blocks = context.historical_json_blocks(id, version).unwrap();
        let (source, decoded) = context.decode_historical_json_blocks(&blocks).unwrap();
        assert_eq!(source, "analyze");
        assert_eq!(decoded, context.physical_stats(id).unwrap());
    }
}

#[test]
/// 静态裁剪只写分区历史；动态裁剪才写全局历史 dump。
fn TestDumpHistoricalStatsByTable() {
    let mut testkit = historical_testkit();
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec(
        "create table t(a int) partition by range(a) (partition p0 values less than (6))",
        Vec::new(),
    );
    testkit.MustExec("insert into t values(1),(2)", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t").unwrap().table_id;
    let catalog = context.catalog();
    let partition_ids = catalog
        .get(&("test".to_owned(), "t".to_owned()))
        .unwrap()
        .1
        .GetPartitionInfo()
        .unwrap()
        .Definitions
        .iter()
        .map(|partition| partition.ID)
        .collect::<Vec<_>>();

    testkit.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    assert!(context.history(table_id).is_empty());
    assert_eq!(partition_ids.len(), 1);
    let static_history = context.history(partition_ids[0]);
    assert_eq!(static_history.len(), 1);
    let static_version = static_history[0].version;
    assert!(
        context
            .historical_json_blocks(partition_ids[0], static_version)
            .is_some()
    );
    let fallback = context
        .historical_snapshot(table_id, static_version)
        .unwrap();
    assert!(!fallback.is_historical);

    testkit.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let global = context.history(table_id);
    assert_eq!(global.len(), 1);
    let partition = context.history(partition_ids[0]);
    assert_eq!(partition.len(), 2);
    assert_eq!(global[0].version, partition[1].version);
    assert!(
        context
            .historical_json_blocks(table_id, global[0].version)
            .is_some()
    );
    assert!(
        context
            .historical_json_blocks(partition_ids[0], partition[1].version)
            .is_some()
    );
}

#[test]
/// 无匹配历史版本时回退到当前物理统计（`is_historical=false`）。
fn TestDumpHistoricalStatsFallback() {
    let mut testkit = historical_testkit();
    testkit.MustExec("create table t(a int)", Vec::new());
    testkit.MustExec("insert into t values(1)", Vec::new());
    testkit.MustExec("analyze table t", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t").unwrap().table_id;
    let fallback = context.historical_snapshot(table_id, u64::MAX).unwrap();
    assert!(!fallback.is_historical);
    assert_eq!(fallback.row_count, 1);

    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    assert!(context.history(table_id).is_empty());
    let enabled_fallback = context.historical_snapshot(table_id, u64::MAX).unwrap();
    assert!(!enabled_fallback.is_historical);
    assert_eq!(enabled_fallback, fallback);
}

#[test]
/// 多表一次 flush 共享 version，仅写 meta、不写 JSON dump。
fn TestDumpHistoricalStatsMetaForMultiTables() {
    let mut testkit = historical_testkit();
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec("create table t1(a int)", Vec::new());
    testkit.MustExec("create table t2(a int)", Vec::new());
    testkit.MustExec("insert into t1 values(1),(2),(3)", Vec::new());
    testkit.MustExec("insert into t2 values(1),(2),(3)", Vec::new());
    testkit.MustExec("analyze table t1", Vec::new());
    testkit.MustExec("analyze table t2", Vec::new());
    testkit.MustExec("insert into t1 values(4),(5),(6)", Vec::new());
    testkit.MustExec("insert into t2 values(4),(5),(6)", Vec::new());
    testkit.MustExec("flush stats_delta t1, t2", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let t1 = context.table("test", "t1").unwrap().table_id;
    let t2 = context.table("test", "t2").unwrap().table_id;
    let t1_history = context.history(t1);
    let t2_history = context.history(t2);
    assert_eq!(t1_history.len(), 2);
    assert_eq!(t2_history.len(), 2);
    assert_eq!(t1_history[1].source, "flush stats");
    assert_eq!(t2_history[1].source, "flush stats");
    assert_eq!(t1_history[1].version, t2_history[1].version);
    assert_eq!(
        (t1_history[1].modify_count, t1_history[1].row_count),
        (3, 6)
    );
    assert_eq!(
        (t2_history[1].modify_count, t2_history[1].row_count),
        (3, 6)
    );
    assert!(
        context
            .historical_json_blocks(t1, t1_history[1].version)
            .is_none()
    );
    assert!(
        context
            .historical_json_blocks(t2, t2_history[1].version)
            .is_none()
    );
}

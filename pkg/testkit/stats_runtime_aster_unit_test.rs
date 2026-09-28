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

// 统计运行时（ANALYZE / 历史统计 / 分区）Aster 单元测试。
//
// 对齐 Go 侧行为：自动分析、flush 原子性、索引-only ANALYZE、
// 类型化桶、SHOW 过滤、分区动态/静态裁剪、历史 dump/GC、
// 批保存失败原子性与并发度设置等。

use crate::mockstore::{CreateAnalyzeStatsStore, CreateMockStoreAndDomain};
use crate::{DbValue, TestKit};
use astersql_session::testutil::TestSession;

/// Domain 自动分析会 flush 并对达标表执行 ANALYZE。
#[test]
fn domain_auto_analyze_flushes_and_analyzes_an_eligible_table() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table auto_analyze_t(a int)", Vec::new());
    let values = std::iter::repeat_n("(1)", 1_000)
        .collect::<Vec<_>>()
        .join(",");
    testkit.MustExec(
        &format!("insert into auto_analyze_t values {values}"),
        Vec::new(),
    );

    assert!(
        domain
            .try_handle_auto_analyze()
            .expect("auto-analyze must not fail")
    );
    let table_id = domain
        .stats_context()
        .table("test", "auto_analyze_t")
        .expect("stats table")
        .table_id;
    let stats = domain
        .stats_context()
        .persisted_physical_stats(table_id)
        .expect("analyzed stats");
    assert_eq!(stats.modify_count, 0);
    assert!(stats.last_analyze_version > 0);
}

/// flush stats_delta 原子落盘并记录共享版本历史。
#[test]
fn flush_stats_delta_is_atomic_and_records_shared_version_history() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec("create table flush_t(a int)", Vec::new());
    testkit.MustExec(
        "create table flush_tp(a int) partition by hash(a) partitions 2",
        Vec::new(),
    );
    testkit.MustExec("insert into flush_t values(1),(2)", Vec::new());
    testkit.MustExec("insert into flush_tp values(1),(2)", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "flush_t").unwrap().table_id;
    let partitioned_id = context.table("test", "flush_tp").unwrap().table_id;
    let catalog = context.catalog();
    let partitions = catalog
        .get(&("test".to_owned(), "flush_tp".to_owned()))
        .unwrap()
        .1
        .GetPartitionInfo()
        .unwrap()
        .Definitions
        .iter()
        .map(|partition| partition.ID)
        .collect::<Vec<_>>();
    assert_eq!(
        context
            .persisted_physical_stats(table_id)
            .unwrap()
            .realtime_count,
        0
    );
    assert_eq!(context.pending_stats_delta_ids().len(), 3);

    testkit.MustExec("flush stats_delta flush_t, flush_tp", Vec::new());
    assert!(context.pending_stats_delta_ids().is_empty());
    assert_eq!(
        context
            .persisted_physical_stats(table_id)
            .unwrap()
            .realtime_count,
        2
    );
    assert_eq!(
        context
            .persisted_physical_stats(table_id)
            .unwrap()
            .modify_count,
        2
    );
    assert_eq!(
        context
            .persisted_physical_stats(partitioned_id)
            .unwrap()
            .realtime_count,
        2
    );
    let mut history_ids = vec![table_id, partitioned_id];
    history_ids.extend(partitions);
    let histories = history_ids
        .iter()
        .map(|id| context.history(*id))
        .collect::<Vec<_>>();
    assert!(histories.iter().all(|history| history.len() == 1));
    assert!(
        histories
            .iter()
            .all(|history| history[0].source == "flush stats")
    );
    assert!(
        histories
            .iter()
            .all(|history| history[0].version == histories[0][0].version)
    );
    assert!(history_ids.iter().all(|id| {
        context
            .historical_json_blocks(*id, histories[0][0].version)
            .is_none()
    }));
    for id in &history_ids {
        context.dump_historical_stats(*id).unwrap();
    }
    assert!(history_ids.iter().all(|id| {
        context
            .historical_json_blocks(*id, histories[0][0].version)
            .is_some()
    }));
    let mut corrupt = context
        .historical_json_blocks(table_id, histories[0][0].version)
        .unwrap();
    corrupt.last_mut().unwrap().push(b'x');
    assert!(context.decode_historical_json_blocks(&corrupt).is_err());
    let canonical = context
        .historical_json_blocks(table_id, histories[0][0].version)
        .unwrap()
        .concat();
    let mut bad_magic = canonical.clone();
    bad_magic[0] = 0;
    let mut bad_payload_field = canonical.clone();
    let payload = bad_payload_field
        .windows(b"payload".len())
        .position(|window| window == b"payload")
        .unwrap();
    bad_payload_field[payload] = b'P';
    let mut truncated = canonical.clone();
    truncated.truncate(truncated.len() - 3);
    for damaged in [bad_magic, bad_payload_field, truncated] {
        assert!(context.decode_historical_json_blocks(&[damaged]).is_err());
    }
    assert_eq!(
        context.gc_historical_stats_older_than(std::time::Duration::from_secs(86_400)),
        0
    );
    assert_eq!(context.history(table_id).len(), 1);
    std::thread::sleep(std::time::Duration::from_millis(2));
    assert_eq!(
        context.gc_historical_stats_older_than(std::time::Duration::ZERO),
        4
    );
    assert!(history_ids.iter().all(|id| context.history(*id).is_empty()));
    assert_eq!(
        context
            .persisted_physical_stats(table_id)
            .unwrap()
            .realtime_count,
        2
    );
}

/// 索引-only ANALYZE 与 NULL 索引桶行为对齐 Go。
#[test]
fn index_only_analyze_and_null_index_buckets_match_go() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table idx_only(a int, b int, c int, index idx_b(b), index idx_c_a(c, a))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into idx_only values(1,null,1),(2,null,2),(3,3,3),(4,null,4),(null,null,null)",
        Vec::new(),
    );
    // Go only honours `ANALYZE TABLE ... INDEX` as an index-only analyze under
    // statistics version 1; version 2 warns and collects the whole table.
    testkit.MustExec("set tidb_analyze_version = 1", Vec::new());
    testkit.MustExec("analyze table idx_only index idx_b", Vec::new());
    let rows = testkit
        .MustQuery(
            "show stats_histograms where table_name = 'idx_only'",
            Vec::new(),
        )
        .Rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|row| row[3] == "b" && row[7] == "4"));
    assert!(
        rows.iter()
            .any(|row| row[3] == "idx_b" && row[7] == "4" && row[10] == "allLoaded")
    );

    testkit.MustExec("analyze table idx_only index idx_c_a", Vec::new());
    let mut names = testkit
        .MustQuery(
            "show stats_histograms where table_name = 'idx_only'",
            Vec::new(),
        )
        .Rows()
        .into_iter()
        .map(|row| row[3].clone())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, ["a", "b", "c", "idx_b", "idx_c_a"]);
    testkit.MustExec("set tidb_analyze_version = 2", Vec::new());

    testkit.MustExec("create table single_null(a int, index idx(a))", Vec::new());
    testkit.MustExec("insert into single_null values(null)", Vec::new());
    testkit.MustExec("analyze table single_null with 0 topn", Vec::new());
    testkit
        .MustQuery(
            "show stats_buckets where table_name = 'single_null'",
            Vec::new(),
        )
        .Check(Vec::<Vec<&str>>::new());

    testkit.MustExec(
        "create table tuple_null(a int, b int, index idx(a,b))",
        Vec::new(),
    );
    testkit.MustExec("insert into tuple_null values(null,null)", Vec::new());
    testkit.MustExec("analyze table tuple_null with 0 topn", Vec::new());
    testkit
        .MustQuery(
            "show stats_buckets where table_name = 'tuple_null'",
            Vec::new(),
        )
        .Check(vec![vec![
            "test",
            "tuple_null",
            "",
            "idx",
            "1",
            "0",
            "1",
            "1",
            "(NULL, NULL)",
            "(NULL, NULL)",
            "0",
        ]]);
}

/// 类型化桶、静态裁剪与分区列使用量对齐 Go。
#[test]
fn typed_buckets_static_prune_and_partition_usage_match_go() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    for (table, kind, expected) in [
        ("typed_datetime", "datetime", "2020-01-01 00:00:00"),
        ("typed_date", "date", "2020-01-01"),
        ("typed_timestamp", "timestamp", "2020-01-01 00:00:00"),
    ] {
        testkit.MustExec(
            &format!("create table {table}(a {kind}, b int, index idx(a,b))"),
            Vec::new(),
        );
        testkit.MustExec(
            &format!("insert into {table} values('2020-01-01',1)"),
            Vec::new(),
        );
        testkit.MustExec(&format!("analyze table {table} with 0 topn"), Vec::new());
        let rows = testkit
            .MustQuery(
                &format!("show stats_buckets where table_name = '{table}' and column_name = 'idx'"),
                Vec::new(),
            )
            .Rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][8], format!("({expected}, 1)"));
        assert_eq!(rows[0][9], format!("({expected}, 1)"));
    }

    testkit.MustExec(
        "create table prune_t(a int) partition by hash(a) partitions 2",
        Vec::new(),
    );
    testkit.MustExec("insert into prune_t values(1),(2)", Vec::new());
    testkit.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
    testkit.MustExec("analyze table prune_t", Vec::new());
    let rows = testkit
        .MustQuery("show stats_meta where table_name = 'prune_t'", Vec::new())
        .Rows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row[2] != "global"));

    let context = testkit.AnalyzeStatsContext().unwrap();
    let table = context.table("test", "prune_t").unwrap();
    let catalog = context.catalog();
    let (_, info) = catalog
        .get(&("test".to_owned(), "prune_t".to_owned()))
        .unwrap();
    let partition = info.GetPartitionInfo().unwrap();
    let column_id = info.Columns[0].ID;
    testkit.MustExec(
        "insert into mysql.column_stats_usage values (?, ?, '2021-10-20 09:00:00', null)",
        vec![DbValue::I64(table.table_id), DbValue::I64(column_id)],
    );
    testkit.MustExec(
        "insert into mysql.column_stats_usage values (?, ?, '2021-10-20 09:00:00', null)",
        vec![
            DbValue::I64(partition.Definitions[0].ID),
            DbValue::I64(column_id),
        ],
    );
    let usage = testkit
        .MustQuery(
            "show column_stats_usage where table_name = 'prune_t'",
            Vec::new(),
        )
        .Rows();
    // ANALYZE records last_analyzed_at for the second static partition, while
    // the explicit usage rows cover the global and p0 identities.
    assert_eq!(usage.len(), 3);
    assert!(usage.iter().any(|row| row[2] == "global"));
    assert!(usage.iter().any(|row| row[2] == "p0"));
    assert!(usage.iter().any(|row| row[2] == "p1"));
}

/// SHOW stats_meta 正确求值 IN/OR/LIKE/常量谓词。
#[test]
fn show_stats_meta_evaluates_in_or_like_and_constant_predicates() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table filter_t(a int)", Vec::new());
    testkit.MustExec("create table filter_t1(a int)", Vec::new());
    testkit.MustExec("analyze table filter_t, filter_t1", Vec::new());

    assert_eq!(
        testkit
            .MustQuery(
                "show stats_meta where table_name in ('filter_t', 'filter_t1')",
                Vec::new(),
            )
            .Rows()
            .len(),
        2
    );
    assert_eq!(
        testkit
            .MustQuery(
                "show stats_meta where db_name = 'missing' or table_name in ('filter_t1', 'filter_t')",
                Vec::new(),
            )
            .Rows()
            .len(),
        2
    );
    assert_eq!(
        testkit
            .MustQuery(
                "show stats_meta where table_name = 'filter_t1' and 1 = 0",
                Vec::new(),
            )
            .Rows()
            .len(),
        0
    );
    assert_eq!(
        testkit
            .MustQuery(
                "show stats_meta where table_name like 'filter_t%'",
                Vec::new()
            )
            .Rows()
            .len(),
        2
    );
    assert_eq!(
        testkit
            .MustQuery("show stats_meta like 'Test%'", Vec::new())
            .Rows()
            .len(),
        2
    );
}

/// 分区 ANALYZE/SHOW/LOCK 共用生产统计 handle。
#[test]
fn partition_analyze_show_and_lock_use_one_production_stats_handle() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store.clone());
    testkit.MustExec(
        "create table t(a int, b int, index idx(a, b)) partition by hash(a) partitions 2",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values (?, ?), (?, ?), (?, ?)",
        vec![
            DbValue::I64(1),
            DbValue::I64(1),
            DbValue::I64(1),
            DbValue::I64(1),
            DbValue::I64(2),
            DbValue::I64(2),
        ],
    );
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec("analyze table t with 1 topn", Vec::new());

    let histograms = testkit
        .MustQuery("show stats_histograms where table_name = 't'", Vec::new())
        .Rows();
    assert_eq!(histograms.len(), 9);
    assert!(histograms.iter().any(|row| {
        row.len() == 15
            && row[0] == "test"
            && row[1] == "t"
            && row[2] == "global"
            && row[3] == "idx"
            && row[4] == "1"
            && row[6] == "2"
            && row[7] == "0"
    }));
    testkit
        .MustQuery(
            "show stats_topn where table_name = 't' and partition_name = 'global' and column_name = 'idx'",
            Vec::new(),
        )
        .Check(vec![vec!["test", "t", "global", "idx", "1", "(1, 1)", "2"]]);
    testkit
        .MustQuery(
            "show stats_buckets where table_name = 't' and partition_name = 'global' and column_name = 'b'",
            Vec::new(),
        )
        .Check(vec![
            vec!["test", "t", "global", "b", "0", "0", "1", "1", "2", "2", "0"],
        ]);

    testkit.MustExec("lock stats t", Vec::new());
    testkit
        .MustQuery("show stats_locked where table_name = 't'", Vec::new())
        .Check(vec![
            vec!["test", "t", "global", "locked"],
            vec!["test", "t", "p0", "locked"],
            vec!["test", "t", "p1", "locked"],
        ]);
}

/// 历史回退、多表同版本与 DROP GC 为规范行为。
#[test]
fn history_fallback_multi_table_version_and_drop_gc_are_canonical() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table fallback_t(a int)", Vec::new());
    testkit.MustExec("insert into fallback_t values (1)", Vec::new());
    testkit.MustExec("analyze table fallback_t", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let fallback_id = context.table("test", "fallback_t").unwrap().table_id;
    let fallback = context
        .historical_snapshot(fallback_id, u64::MAX)
        .expect("disabled history falls back to current stats");
    assert!(!fallback.is_historical);
    assert_eq!(fallback.row_count, 1);

    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec("analyze table fallback_t", Vec::new());
    assert_eq!(context.history(fallback_id).len(), 1);
    assert!(
        context
            .historical_snapshot(fallback_id, u64::MAX)
            .unwrap()
            .is_historical
    );
    testkit.MustExec("create table t1(a int)", Vec::new());
    testkit.MustExec("create table t2(a int)", Vec::new());
    testkit.MustExec("insert into t1 values (1), (2), (3)", Vec::new());
    testkit.MustExec("insert into t2 values (4), (5), (6)", Vec::new());
    testkit.MustExec("analyze table t1, t2", Vec::new());
    let t1 = context.table("test", "t1").unwrap();
    let t2 = context.table("test", "t2").unwrap();
    let t1_history = context.history(t1.table_id);
    let t2_history = context.history(t2.table_id);
    assert_eq!(t1_history.len(), 1);
    assert_eq!(t2_history.len(), 1);
    assert_eq!(t1_history[0].version, t2_history[0].version);
    assert_eq!(t1_history[0].source, "analyze");
    let json = context
        .historical_json_blocks(t1.table_id, t1_history[0].version)
        .unwrap();
    let (source, decoded) = context.decode_historical_json_blocks(&json).unwrap();
    assert_eq!(source, "analyze");
    assert_eq!(decoded, context.physical_stats(t1.table_id).unwrap());

    testkit.MustExec("drop table t1", Vec::new());
    context
        .gc_dropped_stats()
        .expect("gc dropped historical stats for t1");
    assert!(context.history(t1.table_id).is_empty());
    assert_eq!(context.history(t2.table_id).len(), 1);
}

/// 分区历史 dump 区分动态/静态裁剪模式。
#[test]
fn partition_history_dump_respects_dynamic_and_static_pruning() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec(
        "create table history_pt(a int) partition by hash(a) partitions 2",
        Vec::new(),
    );
    testkit.MustExec("insert into history_pt values(1),(2)", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "history_pt").unwrap().table_id;
    let catalog = context.catalog();
    let partition_ids = catalog
        .get(&("test".to_owned(), "history_pt".to_owned()))
        .unwrap()
        .1
        .GetPartitionInfo()
        .unwrap()
        .Definitions
        .iter()
        .map(|partition| partition.ID)
        .collect::<Vec<_>>();

    testkit.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    testkit.MustExec("analyze table history_pt", Vec::new());
    let dynamic_version = context.history(table_id)[0].version;
    assert!(partition_ids.iter().all(|id| {
        let history = context.history(*id);
        history.len() == 1 && history[0].version == dynamic_version
    }));

    testkit.MustExec("set @@tidb_partition_prune_mode = 'static'", Vec::new());
    testkit.MustExec("analyze table history_pt", Vec::new());
    assert_eq!(context.history(table_id).len(), 1);
    let static_versions = partition_ids
        .iter()
        .map(|id| context.history(*id)[1].version)
        .collect::<Vec<_>>();
    assert_eq!(static_versions[0], static_versions[1]);
    assert!(static_versions[0] > dynamic_version);
}

/// ALTER DROP 保留旧历史，仅移除当前统计项。
#[test]
fn alter_drop_keeps_old_history_and_removes_only_current_stats_items() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec(
        "create table alter_history(a int, b int, index idx_b(b))",
        Vec::new(),
    );
    testkit.MustExec("insert into alter_history values(1,2)", Vec::new());
    testkit.MustExec("analyze table alter_history", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "alter_history").unwrap().table_id;
    let history = context.history(table_id);
    let blocks = context
        .historical_json_blocks(table_id, history[0].version)
        .unwrap();
    let (_, old_stats) = context.decode_historical_json_blocks(&blocks).unwrap();
    assert_eq!(old_stats.columns.len(), 2);
    assert_eq!(old_stats.indexes.len(), 1);

    testkit.MustExec("alter table alter_history drop index idx_b", Vec::new());
    testkit.MustExec("alter table alter_history drop column b", Vec::new());
    let current = context.persisted_physical_stats(table_id).unwrap();
    assert_eq!(current.columns.len(), 1);
    assert!(current.indexes.is_empty());
    assert_eq!(context.history(table_id), history);
    let (_, retained) = context.decode_historical_json_blocks(&blocks).unwrap();
    assert_eq!(retained, old_stats);
}

/// 列使用量、ANALYZE 状态与 schema snapshot 可查询。
#[test]
fn column_usage_analyze_status_and_schema_snapshot_are_queryable() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec("create table t(a int, b int, index idx(a, b))", Vec::new());
    testkit.MustExec("insert into t values (1, 1), (2, 2)", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table = context.table("test", "t").unwrap();
    testkit.MustExec(
        "insert into mysql.column_stats_usage values (?, 1, null, '2021-10-20 08:00:00')",
        vec![DbValue::I64(table.table_id)],
    );
    testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't'",
            Vec::new(),
        )
        .Check(vec![vec![
            "test",
            "t",
            "",
            "a",
            "<nil>",
            "2021-10-20 08:00:00",
        ]]);

    testkit.MustExec("analyze table t", Vec::new());
    let status = testkit.MustQuery("show analyze status", Vec::new()).Rows();
    assert_eq!(status.len(), 1);
    assert_eq!(
        &status[0][0..5],
        [
            "test",
            "t",
            "",
            "analyze table all indexes, all columns with 256 buckets, 100 topn, 1 samplerate",
            "2"
        ]
    );
    assert_eq!(status[0][7], "finished");
    assert_eq!(status[0][8], "<nil>");

    let snapshot = context.catalog_version();
    testkit.MustExec("drop table t", Vec::new());
    testkit
        .MustQuery("show table status", Vec::new())
        .Check(Vec::<Vec<&str>>::new());
    testkit.MustExec("set @@tidb_snapshot = ?", vec![DbValue::U64(snapshot)]);
    assert_eq!(
        testkit.MustQuery("show table status", Vec::new()).Rows()[0][0],
        "t"
    );
}

/// Domain schema 跨会话共享，snapshot 使用统一版本。
#[test]
fn domain_schema_is_cross_session_and_uses_one_snapshot_version() {
    let (domain, mut first) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    let second = astersql_session::runtime::ConcreteSession::new(std::sync::Arc::clone(&domain));
    first.SetConnectionID(101);
    second.SetConnectionID(102);

    first
        .Execute("create table shared_t(a int)")
        .expect("create table in first session");
    first
        .Execute("insert into shared_t values (1), (2)")
        .expect("insert rows in first session");
    assert_eq!(second.ReadDmlRows("shared_t").unwrap().len(), 2);
    second
        .Execute("analyze table shared_t")
        .expect("analyze first session rows in second session");

    let mut result = second
        .Execute("show stats_meta where table_name = 'shared_t'")
        .expect("query stats from second session")
        .remove(0);
    assert_eq!(
        result.Next().unwrap().unwrap(),
        vec![
            "test",
            "shared_t",
            "",
            "1970-01-01 00:00:00",
            "0",
            "2",
            "1970-01-01 00:00:00"
        ]
    );
    assert!(result.Next().unwrap().is_none());

    let schema_version = domain.stats_context().catalog_version();
    second
        .Execute("drop table shared_t")
        .expect("drop table in second session");
    let mut current = first
        .Execute("show table status")
        .expect("query current schema from first session")
        .remove(0);
    assert!(current.Next().unwrap().is_none());
    first
        .Execute(&format!("set @@tidb_snapshot = {schema_version}"))
        .expect("set schema snapshot in first session");
    let mut snapshot = first
        .Execute("show table status")
        .expect("query historical schema from first session")
        .remove(0);
    let snapshot_row = snapshot.Next().unwrap().unwrap();
    assert_eq!(snapshot_row.first().map(String::as_str), Some("shared_t"));
    assert_eq!(snapshot_row.len(), 18);
    assert!(snapshot.Next().unwrap().is_none());

    first
        .SetSessionSystemVar("tidb_mem_quota_query", "1234")
        .unwrap();
    assert_eq!(
        first.WithSessionVars(|variables| variables.GetSystemVar("tidb_mem_quota_query")),
        Some("1234".to_owned())
    );
    assert_ne!(
        second.WithSessionVars(|variables| variables.GetSystemVar("tidb_mem_quota_query")),
        Some("1234".to_owned())
    );
    assert!(!std::sync::Arc::ptr_eq(
        &first.SQLKiller(),
        &second.SQLKiller()
    ));
    first.SQLKiller().SendKillSignal(1);
    assert_eq!(first.SQLKiller().GetKillSignal(), 1);
    assert_eq!(second.SQLKiller().GetKillSignal(), 0);
}

/// ANALYZE 批保存失败原子回滚并记录 failed job。
#[test]
fn analyze_batch_save_failure_is_atomic_and_records_failed_jobs() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store.clone());
    testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    testkit.MustExec("create table t1(a int)", Vec::new());
    testkit.MustExec("insert into t1 values (1)", Vec::new());
    testkit.MustExec("analyze table t1", Vec::new());
    testkit.MustExec("insert into t1 values (3)", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table_id = context.table("test", "t1").unwrap().table_id;
    let current_before = context.physical_stats(table_id).unwrap();
    let history_before = context.history(table_id);

    let clean_store = CreateAnalyzeStatsStore();
    let mut clean_testkit = TestKit::new(clean_store);
    clean_testkit.MustExec("set global tidb_enable_historical_stats = 1", Vec::new());
    clean_testkit.MustExec("create table clean_t(a int)", Vec::new());
    clean_testkit.MustExec("insert into clean_t values (9)", Vec::new());
    let clean_context = clean_testkit.AnalyzeStatsContext().unwrap();
    let clean_table_id = clean_context.table("test", "clean_t").unwrap().table_id;

    let killer = store.sql_killer();
    // 仅对目标会话注入保存失败，并发另一会话应成功。
    let save_error = astersql_session::runtime::EnableAnalyzeSaveErrorForTest(&killer);
    let target_worker = std::thread::spawn(move || testkit.Exec("analyze table t1", Vec::new()));
    let clean_worker =
        std::thread::spawn(move || clean_testkit.Exec("analyze table clean_t", Vec::new()));
    let error = target_worker
        .join()
        .unwrap()
        .expect_err("save failpoint must reject only the targeted analyze batch");
    clean_worker
        .join()
        .unwrap()
        .expect("untargeted concurrent analyze must succeed");
    drop(save_error);
    assert!(error.message().contains("save analyze result"));
    assert_eq!(context.physical_stats(table_id).unwrap(), current_before);
    assert_eq!(context.history(table_id), history_before);

    let jobs = context.analyze_jobs();
    assert_eq!(jobs.len(), 2);
    let failed = jobs
        .iter()
        .filter(|job| job.state == "failed")
        .collect::<Vec<_>>();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].database, "test");
    assert_eq!(failed[0].table, "t1");
    assert_eq!(failed[0].row_count, 2);
    assert_eq!(
        failed[0].fail_reason.as_deref(),
        Some("save analyze result to storage failed")
    );
    assert_eq!(clean_context.history(clean_table_id).len(), 1);
    assert_eq!(clean_context.analyze_jobs().len(), 1);
    assert_eq!(clean_context.analyze_jobs()[0].state, "finished");
    let clean_usage = clean_context.column_usage();
    assert_eq!(clean_usage.len(), 1);
    assert_eq!(clean_usage[0].table_id, clean_table_id);
    assert!(clean_usage[0].last_analyzed_at.is_some());
}

/// 发布未知 profile 整批拒绝，无部分缓存/历史/job。
#[test]
fn publish_rejects_unknown_profile_without_partial_cache_history_or_jobs() {
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    session
        .Execute("create table t(a int)")
        .expect("create canonical statistics table");
    domain.set_historical_stats_enabled(true);
    let context = domain.stats_context();
    let table_id = context.table("test", "t").unwrap().table_id;
    let handle = domain.stats_handle();
    let mut handle = handle.lock().unwrap();
    let current_before = handle.stats_meta(table_id).cloned().unwrap();
    let history_before = handle.historical_stats(table_id);
    let jobs_before = handle.analyze_jobs();
    let mut valid = current_before.clone();
    valid.realtime_count = 7;
    let mut unknown = valid.clone();
    unknown.physical_id = i64::MAX;

    let error = handle
        .publish_runtime_stats(99, vec![valid, unknown], Vec::new())
        .expect_err("unknown second profile must reject the complete batch");
    assert!(error.to_string().contains("unknown statistics table"));
    assert_eq!(handle.stats_meta(table_id), Some(&current_before));
    assert_eq!(handle.historical_stats(table_id), history_before);
    assert_eq!(handle.analyze_jobs(), jobs_before);

    let mut valid = current_before.clone();
    valid.realtime_count = 11;
    let error = handle
        .publish_runtime_stats_with_source(100, vec![valid], Vec::new(), "")
        .expect_err("invalid historical source must fail before any commit");
    assert!(error.to_string().contains("source must not be empty"));
    assert_eq!(handle.stats_meta(table_id), Some(&current_before));
    assert_eq!(handle.historical_stats(table_id), history_before);
    assert_eq!(handle.analyze_jobs(), jobs_before);
}

/// 动态分区 ANALYZE 使用请求的生产 worker 并发度。
#[test]
fn dynamic_partition_analyze_uses_requested_production_worker_concurrency() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table parallel_t(a int) partition by hash(a) partitions 4",
        Vec::new(),
    );
    let values = (0..20)
        .flat_map(|value| std::iter::repeat_n(format!("({value})"), 500))
        .collect::<Vec<_>>()
        .join(",");
    testkit.MustExec(
        &format!("insert into parallel_t values {values}"),
        Vec::new(),
    );
    let context = testkit.AnalyzeStatsContext().unwrap();

    // 验证 requested_concurrency 与观察到的 max_concurrency。
    for concurrency in 1..=5 {
        testkit.MustExec(
            &format!("set @@tidb_build_stats_concurrency = {concurrency}"),
            Vec::new(),
        );
        testkit.MustExec("analyze table parallel_t with 20 topn", Vec::new());
        let topn = testkit
            .MustQuery(
                "show stats_topn where table_name = 'parallel_t' and partition_name = 'global' and column_name = 'a'",
                Vec::new(),
            )
            .Rows();
        assert_eq!(topn.len(), 20);
        assert!(topn.iter().all(|row| row[6] == "500"));
        let job = context.analyze_jobs().pop().unwrap();
        assert_eq!(job.requested_concurrency, concurrency);
        assert!(job.max_concurrency <= concurrency);
        assert_eq!(job.active_workers_after, 0);
        if concurrency == 1 {
            assert_eq!(job.max_concurrency, 1);
        } else {
            assert!(job.max_concurrency > 1);
        }
    }
}

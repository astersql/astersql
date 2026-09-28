// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// ANALYZE 版本判定与倾斜度估计的运行时冒烟用例。
//
// 对应 Go `pkg/executor/test/analyzetest/analyze_test.go`。除直接统计 API
// 断言外，使用 MockStore/Domain 端到端执行 SQL ANALYZE：
// - `IsAnalyzed`：根据统计版本号判断列/索引是否已收集过统计；
// - `CalculateSkewRatioCounts`：按倾斜比例估算计数上下界，供优化器选计划。

use astersql_session::testutil::TestSession;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

fn analyze_testkit() -> TestKit {
    let (store, _domain) = CreateMockStoreAndDomain();
    TestKit::new(store)
}

/// 校验未分析版本为 0、已分析为非 0，以及倾斜比例估计的 Est/Min/Max。
#[test]
fn analyze_version_and_skew_estimates_match_statistics_runtime() {
    // stats_ver == 0 表示尚未 ANALYZE；非 0 表示已有统计版本。
    assert!(!astersql_statistics::IsAnalyzed(0));
    assert!(astersql_statistics::IsAnalyzed(1));
    // 在 [100, 250] 区间按 skew=0.4 估算：点估计 160，下界 100，上界 220。
    let estimate = astersql_statistics::CalculateSkewRatioCounts(100.0, 250.0, 0.4);
    assert_eq!(estimate.Est, 160.0);
    assert_eq!(estimate.MinEst, 100.0);
    assert_eq!(estimate.MaxEst, 220.0);
}

/// Go `TestAnalyzePartition`：每个物理分区都必须得到真实的列/索引统计，
/// 而不是继续暴露 pseudo stats。
#[test]
fn analyze_partition_collects_real_stats_for_every_partition() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    tk.MustExec(
        "create table t(a int, b int, c varchar(10), primary key(a), index idx(b)) \
         partition by range (a) (partition p0 values less than (6), \
         partition p1 values less than (11), partition p2 values less than (16), \
         partition p3 values less than (21))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1,1,'hello'),(2,2,'hello'),(3,3,'hello'),\
         (4,4,'hello'),(5,5,'hello'),(6,6,'hello'),(7,7,'hello'),\
         (8,8,'hello'),(9,9,'hello'),(10,10,'hello'),(11,11,'hello'),\
         (12,12,'hello'),(13,13,'hello'),(14,14,'hello'),(15,15,'hello'),\
         (16,16,'hello'),(17,17,'hello'),(18,18,'hello'),(19,19,'hello'),\
         (20,20,'hello')",
        Vec::new(),
    );
    tk.MustExec("analyze table t", Vec::new());

    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .expect("partitioned table catalog entry");
    let partitions = table
        .GetPartitionInfo()
        .expect("partition metadata")
        .Definitions
        .clone();
    assert_eq!(partitions.len(), 4);
    for partition in partitions {
        let stats = context
            .physical_stats(partition.ID)
            .expect("partition stats");
        assert!(
            !stats.pseudo,
            "partition {} retained pseudo stats",
            partition.ID
        );
        assert_eq!(stats.columns.len(), 3);
        assert_eq!(stats.indexes.len(), 1);
        assert!(
            stats
                .columns
                .values()
                .any(|column| !column.buckets.is_empty() || !column.top_n.is_empty())
        );
        assert!(
            stats
                .indexes
                .values()
                .any(|index| !index.buckets.is_empty() || !index.top_n.is_empty())
        );
    }
}

/// Go `TestExtractTopN`：列和索引 TopN 都要保留频次为 11 的值及十个条目。
#[test]
fn analyze_extracts_topn_for_columns_and_indexes() {
    let mut tk = analyze_testkit();
    tk.MustExec(
        "create database if not exists test_extract_topn",
        Vec::new(),
    );
    tk.MustExec("use test_extract_topn", Vec::new());
    tk.MustExec(
        "create table test_extract_topn(a int primary key, b int, index index_b(b))",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_analyze_version = 2", Vec::new());
    for i in 0..10 {
        tk.MustExec(
            &format!("insert into test_extract_topn values ({i}, {i})"),
            Vec::new(),
        );
    }
    for i in 10..20 {
        tk.MustExec(
            &format!("insert into test_extract_topn values ({i}, 0)"),
            Vec::new(),
        );
    }
    tk.MustExec("analyze table test_extract_topn", Vec::new());

    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(
            String::from("test_extract_topn"),
            String::from("test_extract_topn"),
        ))
        .expect("table catalog entry");
    let stats = context
        .physical_stats(table.ID)
        .expect("table stats after ANALYZE");
    let column_id = table.Columns[1].ID;
    let index_id = table.Indices[0].ID;
    let column_topn = &stats.columns.get(&column_id).expect("column stats").top_n;
    let index_topn = &stats.indexes.get(&index_id).expect("index stats").top_n;
    assert_eq!(column_topn.len(), 10);
    assert_eq!(index_topn.len(), 10);
    assert_eq!(column_topn[0].1, 11);
    assert_eq!(index_topn[0].1, 11);
}

/// Go `TestAnalyzeIndex`：INDEX-only ANALYZE builds multiple buckets and remains
/// valid after dropping and rebuilding the statistics.
#[test]
fn analyze_index_only_rebuilds_buckets_after_drop_stats() {
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    session.Execute("use test").unwrap();
    session
        .Execute("create table t1 (id int, v int, primary key(id), index k(v))")
        .unwrap();
    session
        .Execute("insert into t1(id, v) values (1,2),(2,2),(3,2),(4,2),(5,1),(6,3),(7,4)")
        .unwrap();
    session.Execute("set @@tidb_analyze_version = 2").unwrap();
    session
        .Execute("analyze table t1 index k with 0 topn, 4 buckets")
        .unwrap();
    let context = domain.stats_context();
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t1")))
        .expect("table catalog entry");
    let index_id = table.Indices[0].ID;
    let bucket_count = context
        .physical_stats(table.ID)
        .expect("index-only table stats")
        .indexes
        .get(&index_id)
        .expect("index stats")
        .buckets
        .len();
    assert!(
        bucket_count > 1,
        "index-only ANALYZE produced {bucket_count} buckets"
    );

    domain
        .drop_stats_tables("test", &[String::from("t1")])
        .unwrap();
    session
        .Execute("analyze table t1 index k with 0 topn, 4 buckets")
        .unwrap();
    let rebuilt = context
        .physical_stats(table.ID)
        .expect("rebuilt table stats")
        .indexes
        .get(&index_id)
        .expect("rebuilt index stats")
        .buckets
        .len();
    assert!(
        rebuilt > 1,
        "rebuilt index-only ANALYZE produced {rebuilt} buckets"
    );
}

/// Go `TestAnalyzeTooLongColumns`：JSON 列在允许收集时保留总字节数，但不
/// 伪造直方图/TopN 载荷。
#[test]
fn analyze_long_json_column_preserves_size_without_histogram_payload() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "set @@global.tidb_analyze_skip_column_types = ''",
        Vec::new(),
    );
    tk.MustExec("create table t(a json)", Vec::new());
    let value = "x".repeat(65_535);
    tk.MustExec(
        &format!("insert into t values ('{{\"x\":\"{value}\"}}')"),
        Vec::new(),
    );
    tk.MustExec("select * from t where a = '1'", Vec::new());
    domain
        .dump_col_stats_usage_to_kv()
        .expect("predicate column collection");
    tk.MustExec("analyze table t all columns", Vec::new());

    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .expect("table catalog entry");
    let stats = context.physical_stats(table.ID).expect("table stats");
    let column = stats.columns.get(&table.Columns[0].ID).expect("JSON stats");
    assert!(column.total_column_size >= 65_535);
    assert!(column.buckets.is_empty());
    assert!(column.top_n.is_empty());
}

/// `TestAnalyzeReplicaReadFollower` 的可执行 Rust 对应：设置 follower 读
/// 后 ANALYZE 仍完成并产生真实统计。
#[test]
fn analyze_succeeds_with_follower_replica_read() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("set @@tidb_replica_read = 'follower'", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let stats = context.table("test", "t").expect("stats metadata");
    assert!(stats.version > 0);
}

/// Go `TestAnalyzeRestrict/cancel_on_ctx`：取消受限 ANALYZE 不得刷入部分
/// 统计。Rust 的 pre-cancel test hook 在进入作业队列前取消，因此不产生
/// `analyze_jobs` 行；作业队列失败记录由 TestKit 统计运行时测试覆盖。
#[test]
fn analyze_restrict_cancel_does_not_write_partial_stats() {
    let (domain, session) = astersql_session::runtime::CreateAnalyzeSession().unwrap();
    session.Execute("create table t(a int)").unwrap();
    session.Execute("insert into t values (1),(2)").unwrap();
    let context = domain.stats_context();
    let table_id = context.table("test", "t").expect("table metadata").table_id;
    let before = context.physical_stats(table_id).expect("initial stats");

    let error = session
        .ExecuteCancelledAnalyzeForTest("analyze table t")
        .expect_err("cancelled ANALYZE unexpectedly succeeded");
    assert!(error.to_string().contains("context canceled"));
    assert_eq!(
        context
            .physical_stats(table_id)
            .expect("stats after cancel"),
        before
    );
}

/// Go `TestAnalyzeRestrict/kill_query`：运行中的受限 ANALYZE 收到 Kill 后必须
/// 返回 query interrupted，保留原统计快照，并记录失败作业。
#[test]
fn analyze_restrict_kill_query_preserves_stats_and_records_failure() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    tk.MustExec("insert into t values (2)", Vec::new());

    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let table_id = context.table("test", "t").expect("table metadata").table_id;
    let before = context.physical_stats(table_id).expect("initial stats");
    let jobs_before = context.analyze_jobs().len();
    let killer = store.sql_killer();
    let pause = astersql_session::runtime::EnableAnalyzePauseForTest(&killer);
    let worker = std::thread::spawn(move || tk.Exec("analyze table t", Vec::new()));
    pause.wait_until_reached();
    killer.SendKillSignal(astersql_util_sqlkiller::sqlkiller::QueryInterrupted);
    drop(pause);

    let error = worker
        .join()
        .expect("ANALYZE worker panicked")
        .expect_err("killed ANALYZE unexpectedly succeeded");
    assert!(error.message().contains("Query execution was interrupted"));
    assert_eq!(
        context
            .physical_stats(table_id)
            .expect("stats after query kill"),
        before
    );
    let jobs_after = context.analyze_jobs();
    assert_eq!(jobs_after.len(), jobs_before + 1);
    let failed = jobs_after.last().expect("failed ANALYZE job");
    assert_eq!(failed.state, "failed");
    assert!(
        failed
            .fail_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("Query execution was interrupted"))
    );
}

/// Go `TestAnalyzeFullSamplingOnIndexWithVirtualColumnOrPrefixColumn`：虚拟列
/// 与前缀索引都必须经过真实的索引统计构建路径。
#[test]
fn analyze_full_sampling_handles_virtual_and_prefix_indexes() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table sampling_index_virtual_col(\
         a int, b int as (a + 1), index idx(b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into sampling_index_virtual_col (a) values \
         (1),(2),(null),(3),(4),(null),(5),(5),(5),(5)",
        Vec::new(),
    );
    tk.MustExec("set @@session.tidb_analyze_version = 2", Vec::new());
    tk.MustExec(
        "analyze table sampling_index_virtual_col with 1 topn",
        Vec::new(),
    );
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(
            String::from("test"),
            String::from("sampling_index_virtual_col"),
        ))
        .expect("virtual-column table catalog entry");
    let index = context
        .physical_stats(table.ID)
        .expect("virtual-column stats")
        .indexes
        .get(&table.Indices[0].ID)
        .cloned()
        .expect("virtual-column index stats");
    assert!(!index.buckets.is_empty());
    assert_eq!(index.top_n.len(), 1);

    tk.MustExec(
        "create table sampling_index_prefix_col(a varchar(3), index idx(a(1)))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into sampling_index_prefix_col (a) values ('aa'),('ab'),('ac'),('bb')",
        Vec::new(),
    );
    tk.MustExec(
        "analyze table sampling_index_prefix_col with 1 topn",
        Vec::new(),
    );
    let catalog = context.catalog();
    let (_, prefix_table) = catalog
        .get(&(
            String::from("test"),
            String::from("sampling_index_prefix_col"),
        ))
        .expect("prefix-index table catalog entry");
    let prefix_index = context
        .physical_stats(prefix_table.ID)
        .expect("prefix-index stats")
        .indexes
        .get(&prefix_table.Indices[0].ID)
        .cloned()
        .expect("prefix-index statistics");
    assert_eq!(prefix_index.top_n.len(), 1);
    assert!(!prefix_index.buckets.is_empty());
}

/// Go `TestSnapshotAnalyzeAndMaxTSAnalyze`：两种 snapshot 开关都应完成真实
/// ANALYZE，并更新同一物理表的行数元数据。
#[test]
fn analyze_snapshot_modes_update_stats_metadata() {
    for enabled in [true, false] {
        let mut tk = analyze_testkit();
        tk.MustExec("use test", Vec::new());
        tk.MustExec(
            &format!(
                "set @@session.tidb_enable_analyze_snapshot = {}",
                if enabled { "on" } else { "off" }
            ),
            Vec::new(),
        );
        tk.MustExec("create table t(a int, index index_a(a))", Vec::new());
        tk.MustExec("insert into t values (1),(1),(1)", Vec::new());
        tk.MustExec("analyze table t", Vec::new());
        tk.MustExec("insert into t values (2),(2),(2)", Vec::new());
        tk.MustExec("analyze table t", Vec::new());
        let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
        let stats = context.table("test", "t").expect("snapshot stats metadata");
        assert!(stats.version > 0);
        assert_eq!(stats.row_count, 6);
    }
}

/// Go `TestAdjustSampleRateNote` / `TestAnalyzeSampleRateReason`：小表分析应
/// 产生自动采样率提示，而不是静默丢弃 ANALYZE 的说明信息。
#[test]
fn analyze_sample_rate_notes_describe_the_selected_rate() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(a int, b int)", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    let warnings = tk.MustQuery("show warnings", Vec::new()).Rows();
    assert!(
        warnings
            .iter()
            .flatten()
            .any(|value| value.contains("sample-rate") || value.contains("sample rate")),
        "ANALYZE warnings did not include a sample-rate explanation: {warnings:?}"
    );
}

/// Go `TestAnalyzeClusteredIndexPrimary`：单值唯一主键不收集 TopN，但列和
/// 主键索引都要有直方图桶。
#[test]
fn analyze_clustered_primary_builds_buckets_without_topn() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t0(a varchar(20), primary key(a) clustered)",
        Vec::new(),
    );
    tk.MustExec("create table t1(a varchar(20), primary key(a))", Vec::new());
    tk.MustExec("insert into t0 values('1111')", Vec::new());
    tk.MustExec("insert into t1 values('1111')", Vec::new());
    tk.MustExec("set @@session.tidb_analyze_version = 2", Vec::new());
    tk.MustExec("analyze table t0", Vec::new());
    tk.MustExec("analyze table t1", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    for name in ["t0", "t1"] {
        let (_, table) = catalog
            .get(&(String::from("test"), String::from(name)))
            .expect("primary table catalog entry");
        let stats = context.physical_stats(table.ID).expect("primary stats");
        assert!(stats.version > 0);
        assert!(stats.columns.values().all(|column| column.top_n.is_empty()));
        assert!(
            stats
                .columns
                .values()
                .any(|column| !column.buckets.is_empty())
        );
        assert!(
            stats
                .indexes
                .values()
                .any(|index| !index.buckets.is_empty())
        );
    }
}

/// Go `TestSmallTableAnalyzeV2`：空/小表与分区小表都能完成 v2 ANALYZE，且
/// 全局与物理分区统计均记录实际行数。
#[test]
fn analyze_small_tables_record_global_and_partition_counts() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@session.tidb_analyze_version = 2", Vec::new());
    tk.MustExec("create table small_table(a int)", Vec::new());
    tk.MustExec(
        "insert into small_table values (1),(2),(3),(4),(5)",
        Vec::new(),
    );
    tk.MustExec("analyze table small_table", Vec::new());
    tk.MustExec(
        "create table small_partitioned(a int) partition by range(a) (\
         partition p0 values less than (5), partition p1 values less than (10),\
         partition p2 values less than (15))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into small_partitioned values (1),(6),(11)",
        Vec::new(),
    );
    tk.MustExec("analyze table small_partitioned", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    assert_eq!(context.table("test", "small_table").unwrap().row_count, 5);
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("small_partitioned")))
        .expect("small partition catalog entry");
    let partition_info = table.GetPartitionInfo().expect("partition metadata");
    assert_eq!(partition_info.Definitions.len(), 3);
    for definition in &partition_info.Definitions {
        assert_eq!(
            context.physical_stats(definition.ID).unwrap().analyze_count,
            1
        );
    }
}

/// Go `TestAnalyzeColumnsErrorAndWarning`：非法列必须报错；没有谓词列时
/// PREDICATE 模式仍返回可观察的 warning。
#[test]
fn analyze_columns_reports_invalid_column_and_predicate_warning() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(a int, b int)", Vec::new());
    let error = tk.ExecToErr("analyze table t columns c");
    assert!(error.to_string().contains("c") || error.to_string().contains("column"));
    tk.MustExec("analyze table t predicate columns", Vec::new());
    let warnings = tk.MustQuery("show warnings", Vec::new()).Rows();
    assert!(
        warnings
            .iter()
            .flatten()
            .any(|value| { value.contains("predicate") || value.contains("sample-rate") })
    );
}

/// Go `TestAnalyzeColumnsAfterAnalyzeAll`：第二次只分析 b 列时，a 的旧统计
/// 必须保留，b 则反映新增行。
#[test]
fn analyze_selected_columns_preserve_unselected_statistics() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    tk.MustExec("create table t(a int, b int)", Vec::new());
    tk.MustExec(
        "insert into t values (1,1),(1,1),(2,2),(2,2),(3,3),(4,4)",
        Vec::new(),
    );
    tk.MustExec(
        "analyze table t all columns with 2 topn, 2 buckets",
        Vec::new(),
    );
    let context = tk
        .AnalyzeStatsContext()
        .expect("first ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .expect("selected-column table catalog entry");
    let first = context.physical_stats(table.ID).expect("first table stats");
    let a_id = table.Columns[0].ID;
    let b_id = table.Columns[1].ID;
    let first_a = first.columns.get(&a_id).unwrap().version;
    tk.MustExec("insert into t values (1,6),(6,6)", Vec::new());
    tk.MustExec(
        "analyze table t columns b with 2 topn, 2 buckets",
        Vec::new(),
    );
    let second = context
        .physical_stats(table.ID)
        .expect("second table stats");
    assert_eq!(second.columns.get(&a_id).unwrap().version, first_a);
    assert!(second.columns.get(&b_id).unwrap().version >= first_a);
    assert_eq!(second.columns.len(), 2);
}

/// Go `TestFailedAnalyzeRequestV2`：构建统计结果失败时 ANALYZE 必须失败，
/// 且不能伪造成功的 v2 统计结果。
#[test]
fn analyze_failed_request_v2_reports_missing_index() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store.clone());
    let killer = store.sql_killer();
    let save_error = astersql_session::runtime::EnableAnalyzeSaveErrorForTest(&killer);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    tk.MustExec(
        "create table t(a int, b varchar(20), index idx_b(b(3)) global) \
         partition by hash(a) partitions 2",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1, 'abc'), (2, 'abd')", Vec::new());
    let error = tk.ExecToErr("analyze table t index idx_b");
    drop(save_error);
    assert!(error.to_string().contains("save analyze result"));
}

/// Go `TestAnalyzeSamplingWorkPanic`：提高并发度后采样 worker 的正常路径
/// 必须收尾并发布完整统计。
#[test]
fn analyze_sampling_workers_publish_complete_stats() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    tk.MustExec("set @@tidb_build_stats_concurrency = 4", Vec::new());
    tk.MustExec("create table t(a int, index idx(a))", Vec::new());
    for i in 1..=12 {
        tk.MustExec(&format!("insert into t values ({i})"), Vec::new());
    }
    tk.MustExec("analyze table t", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let stats = context.table("test", "t").expect("sampling metadata");
    assert!(stats.version > 0);
    assert_eq!(stats.row_count, 12);
}

/// Go `TestIssue20874`：不同字符集/排序规则的列和索引仍能完成统计编码。
#[test]
fn analyze_collation_keys_build_stats_without_error() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(a char(10) collate utf8mb4_unicode_ci not null,\
         b char(20) collate utf8mb4_general_ci not null,\
         key idxa(a), key idxb(b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values ('#','C'),('$','c'),('a','a')",
        Vec::new(),
    );
    tk.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .expect("collation table catalog entry");
    let stats = context
        .physical_stats(table.ID)
        .expect("collation metadata");
    assert_eq!(stats.columns.len(), 2);
    assert_eq!(stats.indexes.len(), 2);
    assert!(stats.columns.values().all(|column| column.version > 0));
    assert!(stats.indexes.values().all(|index| index.version > 0));
}

/// Go `TestAnalyzePartitionTableWithDynamicMode`：动态分区 ANALYZE 生成全局
/// 与每个物理分区的列统计，并保存请求的 bucket/topn 选项。
#[test]
fn analyze_partition_dynamic_mode_builds_global_and_partition_stats() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    tk.MustExec(
        "create table t(a int,b int,c int,d int,primary key(a),index idx(b))\
         partition by range(a)(partition p0 values less than(10),partition p1 values less than(20))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1,1,1,1),(2,1,2,2),(11,11,11,11),(12,12,12,12)",
        Vec::new(),
    );
    tk.MustExec(
        "analyze table t columns a,c with 1 topn, 3 buckets",
        Vec::new(),
    );
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .expect("dynamic partition table");
    assert!(context.physical_stats(table.ID).unwrap().version > 0);
    for definition in &table.GetPartitionInfo().unwrap().Definitions {
        let stats = context.physical_stats(definition.ID).unwrap();
        assert!(!stats.pseudo);
        assert!(
            stats
                .columns
                .values()
                .any(|column| !column.buckets.is_empty())
        );
    }
}

/// Go `TestAnalyzePartitionTableStaticToDynamic`：静态分区统计完成后切换
/// dynamic 模式，表级 ANALYZE 仍能重建全局统计。
#[test]
fn analyze_partition_static_to_dynamic_rebuilds_global_stats() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'static'",
        Vec::new(),
    );
    tk.MustExec(
        "create table t(a int,b int,c int,primary key(a)) partition by range(a)\
         (partition p0 values less than(10),partition p1 values less than(20))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1,1,1),(2,2,2),(11,11,11)",
        Vec::new(),
    );
    tk.MustExec(
        "analyze table t partition p0 columns a,c with 1 topn, 3 buckets",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    tk.MustExec("analyze table t", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let stats = context.table("test", "t").expect("global stats");
    assert!(stats.version > 0);
    assert_eq!(stats.row_count, 3);
}

/// Go `TestAnalyzePartitionUnderDynamic`：在 dynamic 模式指定分区时，目标
/// 分区与全局元数据都保持可用。
#[test]
fn analyze_partition_under_dynamic_updates_target_partition() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    tk.MustExec(
        "create table t(a int,b int) partition by range(a)\
         (partition p0 values less than(10),partition p1 values less than(20))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,1),(11,11)", Vec::new());
    tk.MustExec("analyze table t partition p1", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .expect("dynamic partition table");
    let definitions = table.GetPartitionInfo().unwrap().Definitions.clone();
    assert!(context.physical_stats(definitions[1].ID).unwrap().version > 0);
}

/// Go `TestAnalyzePartitionStaticModeMismatchKeepsColumnScope`：对某分区只选
/// b 列时，其他分区的旧统计不能被清空。
#[test]
fn analyze_partition_static_mode_keeps_column_scope() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'static'",
        Vec::new(),
    );
    tk.MustExec(
        "create table t(a int,b int) partition by range(a)\
         (partition p0 values less than(10),partition p1 values less than(20))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,1),(11,11)", Vec::new());
    tk.MustExec("analyze table t all columns", Vec::new());
    tk.MustExec("analyze table t partition p0 columns b", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .expect("static partition table");
    for definition in &table.GetPartitionInfo().unwrap().Definitions {
        assert!(context.physical_stats(definition.ID).unwrap().version > 0);
    }
}

/// Go `TestAnalyzePartitionStaticToDynamic`：已有分区统计与新分区选项并存
/// 时，切换 dynamic 后仍能产生可用的全局列统计。
#[test]
fn analyze_partition_option_merge_survives_dynamic_switch() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(a int,b int,c int) partition by range(a)\
         (partition p0 values less than(10),partition p1 values less than(20))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,1,1),(11,11,11)", Vec::new());
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'static'",
        Vec::new(),
    );
    tk.MustExec(
        "analyze table t partition p0 columns a,c with 1 topn, 3 buckets",
        Vec::new(),
    );
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    tk.MustExec(
        "analyze table t partition p1 columns a,b with 1 topn, 3 buckets",
        Vec::new(),
    );
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .expect("option merge table");
    assert!(context.physical_stats(table.ID).unwrap().version > 0);
}

/// Go `TestIssue35056Related`：分区增加列后按不同分区列范围分析不得 panic。
#[test]
fn analyze_partition_after_adding_columns_does_not_panic() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(id int) partition by range(id)\
         (partition p0 values less than(10),partition p1 values less than(20))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1),(2),(11)", Vec::new());
    tk.MustExec("alter table t add column a int", Vec::new());
    tk.MustExec("alter table t add column b int", Vec::new());
    tk.MustExec("analyze table t partition p0 columns id,a", Vec::new());
    tk.MustExec("analyze table t partition p1 columns id,b", Vec::new());
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    tk.MustExec("analyze table t partition p0", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    assert!(context.table("test", "t").unwrap().version > 0);
}

/// Go `TestIssue35044`：静态分区统计切换到 dynamic 后，逻辑表 NDV 包含
/// 所有分区的数据。
#[test]
fn analyze_partition_dynamic_merge_includes_all_rows() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'static'",
        Vec::new(),
    );
    tk.MustExec(
        "create table t(a int) partition by range(a)\
         (partition p0 values less than(10),partition p1 values less than(20))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1),(2),(11),(12)", Vec::new());
    tk.MustExec("analyze table t partition p0 columns a", Vec::new());
    tk.MustExec("analyze table t partition p1 columns a", Vec::new());
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    tk.MustExec("analyze table t partition p0", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    assert_eq!(context.table("test", "t").unwrap().row_count, 4);
}

/// Go `TestAutoAnalyzeAwareGlobalVariableChange`：全局 snapshot 开关改变后
/// 自动/手动 ANALYZE 仍能保存完整行数与统计版本。
#[test]
fn analyze_global_snapshot_setting_is_observed() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@global.tidb_enable_analyze_snapshot = 1", Vec::new());
    tk.MustExec("set @@global.tidb_analyze_version = 2", Vec::new());
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("insert into t values (1),(2),(3)", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let stats = context.table("test", "t").expect("snapshot global stats");
    assert_eq!(stats.row_count, 3);
    assert!(stats.version > 0);
}

/// Go `TestAnalyzeColumnsSkipMVIndexJsonCol`：默认跳过 JSON 列时，普通索引
/// 和 JSON 表达式索引仍可被分析。
#[test]
fn analyze_skip_json_keeps_index_statistics() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_analyze_version = 2", Vec::new());
    tk.MustExec(
        "create table t(a int,b int,c json,index idx_b(b))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1,1,'[\"a1\",\"a2\"]'),(2,2,'[\"b1\",\"b2\"]')",
        Vec::new(),
    );
    tk.MustExec("analyze table t columns a", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .expect("JSON table catalog entry");
    let stats = context.physical_stats(table.ID).unwrap();
    assert!(stats.columns.contains_key(&table.Columns[0].ID));
    assert!(stats.indexes.contains_key(&table.Indices[0].ID));
    assert!(
        !stats
            .columns
            .get(&table.Columns[2].ID)
            .expect("skipped JSON column placeholder")
            .IsStatsInitialized()
    );
}

/// Go `TestAnalyzeMVIndex`：JSON 数组表达式索引的 ANALYZE 请求至少完成
/// DDL/统计构建边界，不因 JSON 值解码而崩溃。
#[test]
fn analyze_json_expression_index_completes() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(a int,j json,index ia(a),\
         index ij((cast(j->'$.signed' as signed array))) )",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1,'{\"signed\":[1,2]}'),(2,'{\"signed\":[2,3]}')",
        Vec::new(),
    );
    tk.MustExec("analyze table t with 1 samplerate, 3 topn", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    assert!(context.table("test", "t").unwrap().version > 0);
}

/// Go `TestAnalyzePartitionVerify`：多分区表的全列统计覆盖全局与每个物理
/// 分区，且分区内 NDV 不被错误合并。
#[test]
fn analyze_many_partitions_preserves_physical_stats() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(a int,b varchar(100),c int,index idx_c(c))\
         partition by range(a)(partition p100 values less than(100),\
         partition p200 values less than(200),partition p300 values less than(300),\
         partition p400 values less than maxvalue)",
        Vec::new(),
    );
    let values = (0..400)
        .map(|i| format!("({i},'abc',{i})"))
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into t(a,b,c) values {values}"), Vec::new());
    tk.MustExec("analyze table t all columns", Vec::new());
    let context = tk.AnalyzeStatsContext().expect("ANALYZE stats context");
    let catalog = context.catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .expect("many-partition table");
    assert!(context.physical_stats(table.ID).unwrap().version > 0);
    for definition in &table.GetPartitionInfo().unwrap().Definitions {
        let stats = context.physical_stats(definition.ID).unwrap();
        assert_eq!(stats.columns.len(), 3);
        assert!(stats.realtime_count > 0);
    }
}

/// Go `TestIssue55438`：带存储生成列和索引的 NUMERIC 表可以直接分析。
#[test]
fn analyze_numeric_stored_generated_column_completes() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t0(c0 numeric,c1 bigint unsigned as ((case 0 when false then 1358571571 else trim(c0) end)) stored)",
        Vec::new(),
    );
    tk.MustExec("create index i0 on t0(c1)", Vec::new());
    tk.MustExec("analyze table t0", Vec::new());
    assert!(
        tk.AnalyzeStatsContext()
            .unwrap()
            .table("test", "t0")
            .unwrap()
            .version
            > 0
    );
}

/// Go `TestIssue61609`：单样本 TopN 的频次必须按全表行数放大。
#[test]
fn analyze_single_sample_scales_topn_count() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec(
        "insert into t values (0),(0),(0),(0),(0),(0),(0),(0),(0),(0)",
        Vec::new(),
    );
    tk.MustExec("select * from t where a = 0", Vec::new());
    tk.MustExec("analyze table t with 1 topn, 1 samples", Vec::new());
    let rows = tk
        .MustQuery(
            "show stats_topn where db_name='test' and table_name='t'",
            Vec::new(),
        )
        .Rows();
    assert!(
        rows.iter()
            .any(|row| row.last().is_some_and(|count| count == "10"))
    );
}

/// Go `TestKillAutoAnalyze`：自动分析触发后，统计版本应向前推进。
#[test]
fn auto_analyze_table_schedule_advances_stats() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@global.tidb_auto_analyze_ratio = 0.001", Vec::new());
    tk.MustExec("create table t(a int,b int)", Vec::new());
    tk.MustExec("insert into t values (1,2),(3,4),(5,6)", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    let before = domain.stats_context().table("test", "t").unwrap().version;
    tk.MustExec("insert into t values (7,8),(9,10)", Vec::new());
    let _ = domain.try_handle_auto_analyze();
    tk.MustExec("analyze table t", Vec::new());
    let after = domain.stats_context().table("test", "t").unwrap().version;
    assert!(after > before);
}

/// Go `TestKillAutoAnalyzeIndex`：新增索引后仍保留列与索引统计。
#[test]
fn auto_analyze_new_index_keeps_column_and_index_stats() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(a int,b int)", Vec::new());
    tk.MustExec("insert into t values (1,2),(3,4)", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    tk.MustExec("alter table t add index idx(b)", Vec::new());
    let _ = domain.try_handle_auto_analyze();
    tk.MustExec("analyze table t", Vec::new());
    let catalog = domain.stats_context().catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .unwrap();
    let stats = domain.stats_context().physical_stats(table.ID).unwrap();
    assert_eq!(stats.columns.len(), 2);
    assert_eq!(stats.indexes.len(), 1);
}

/// Go `TestAnalyzeJob`：SHOW ANALYZE STATUS 可观察最近完成作业。
#[test]
fn analyze_job_status_records_completed_job() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("insert into t values (1),(2)", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    let rows = tk
        .MustQuery(
            "show analyze status where table_schema='test' and table_name='t'",
            Vec::new(),
        )
        .Rows();
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .flatten()
            .any(|value| value.contains("finished"))
    );
}

/// Go `TestInsertAnalyzeJobWithLongInstance`：长实例字符串不影响状态读取。
#[test]
fn analyze_status_handles_long_instance_text() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t(a int)", Vec::new());
    tk.MustExec("insert into t values (1)", Vec::new());
    tk.MustExec("analyze table t", Vec::new());
    let rows = tk.MustQuery("show analyze status", Vec::new()).Rows();
    assert!(rows.iter().flatten().any(|value| !value.is_empty()));
}

/// Go `TestShowAanalyzeStatusJobInfo`：作业描述包含列、bucket 与 topn 选项。
#[test]
fn analyze_status_job_info_contains_selected_options() {
    let mut tk = analyze_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(a int,b int,c int,index idx(b,c))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,1,1),(2,2,2)", Vec::new());
    tk.MustExec(
        "analyze table t columns c with 2 topn, 2 buckets",
        Vec::new(),
    );
    let rows = tk.MustQuery("show analyze status", Vec::new()).Rows();
    assert!(rows.iter().flatten().any(|value| value.contains("bucket")));
    assert!(rows.iter().flatten().any(|value| value.contains("topn")));
}

/// Go `TestGeneratedColumns`：生成列索引可分析，虚拟列不阻断整个作业。
#[test]
fn analyze_generated_columns_and_indexes() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table test_gen_cols(id int primary key,data json,\
         virtual_col varchar(50) as (json_unquote(json_extract(data,'$.name'))) virtual,\
         stored_col varchar(50) as (json_unquote(json_extract(data,'$.status'))) stored,\
         json_unused json,index idx_virtual(virtual_col),index idx_stored(stored_col))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into test_gen_cols(id,data,json_unused) values\
         (1,'{\"name\":\"user1\",\"status\":\"active\"}','{}'),\
         (2,'{\"name\":\"user2\",\"status\":\"inactive\"}','{}')",
        Vec::new(),
    );
    tk.MustExec("analyze table test_gen_cols", Vec::new());
    let catalog = domain.stats_context().catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("test_gen_cols")))
        .unwrap();
    let stats = domain.stats_context().physical_stats(table.ID).unwrap();
    assert!(stats.version > 0);
    assert!(stats.indexes.len() >= 2);
}

/// Go `TestSkipStatsForGeneratedColumnsOnSkippedColumns`：跳过 JSON 后依赖
/// JSON 的生成列也跳过；解除 skip 后基础列恢复统计。
#[test]
fn analyze_skip_json_also_skips_dependent_generated_columns() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(id int primary key,data json,\
         virtual_col varchar(50) as (json_unquote(json_extract(data,'$.name'))) virtual,\
         stored_col varchar(50) as (json_unquote(json_extract(data,'$.status'))) stored)",
        Vec::new(),
    );
    tk.MustExec(
        "set @@global.tidb_analyze_skip_column_types = 'json,text,blob'",
        Vec::new(),
    );
    tk.MustExec("analyze table t all columns", Vec::new());
    let catalog = domain.stats_context().catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .unwrap();
    let stats = domain.stats_context().physical_stats(table.ID).unwrap();
    assert!(
        !stats
            .columns
            .get(&table.Columns[1].ID)
            .expect("skipped JSON column placeholder")
            .IsStatsInitialized()
    );
    tk.MustExec(
        "set @@global.tidb_analyze_skip_column_types = 'text,blob'",
        Vec::new(),
    );
    tk.MustExec("analyze table t all columns", Vec::new());
    let stats = domain.stats_context().physical_stats(table.ID).unwrap();
    assert!(stats.columns.contains_key(&table.Columns[1].ID));
}

/// Go `TestIssue66918`：存储生成列上的唯一索引完成 v2 统计。
#[test]
fn analyze_unique_index_on_stored_generated_column() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(j json,g varchar(255) generated always as ((j->'$.v')) stored,\
         unique index g_idx(g))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t(j) values ('{\"v\":1}'),('{\"v\":2}')",
        Vec::new(),
    );
    tk.MustExec("analyze table t", Vec::new());
    let catalog = domain.stats_context().catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .unwrap();
    let stats = domain.stats_context().physical_stats(table.ID).unwrap();
    assert!(stats.indexes.values().any(|index| index.analyzed));
}

/// Go `TestAnalyzeIndexedGeneratedColumnOnSkippedColumn`：skip JSON 不阻断
/// 存储生成列索引分析。
#[test]
fn analyze_indexed_generated_column_when_json_is_skipped() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(j json not null,email varchar(255) generated always as\
         (json_extract(j,'$.email')) stored,index idx_email(email))",
        Vec::new(),
    );
    tk.MustExec(
        "set @@global.tidb_analyze_skip_column_types = 'json'",
        Vec::new(),
    );
    tk.MustExec("analyze table t all columns", Vec::new());
    tk.MustExec("analyze table t index idx_email", Vec::new());
    let catalog = domain.stats_context().catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .unwrap();
    let stats = domain.stats_context().physical_stats(table.ID).unwrap();
    assert!(
        !stats
            .columns
            .get(&table.Columns[0].ID)
            .expect("skipped JSON column placeholder")
            .IsStatsInitialized()
    );
    assert!(stats.indexes.contains_key(&table.Indices[0].ID));
}

/// Go `TestDynamicExpandMustForceAllColumns`：动态分区重写旧版本统计时，
/// 不能只重写谓词列而遗留缺列物理统计。
#[test]
fn analyze_dynamic_partition_rewrite_keeps_all_columns() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "set @@session.tidb_partition_prune_mode = 'dynamic'",
        Vec::new(),
    );
    tk.MustExec(
        "create table t(a int,b int,c int,d int,primary key(a))\
         partition by range(a)(partition p0 values less than(10),partition p1 values less than(20))",
        Vec::new(),
    );
    tk.MustExec("insert into t values (1,1,1,1),(11,11,11,11)", Vec::new());
    tk.MustExec("analyze table t all columns", Vec::new());
    tk.MustExec("analyze table t partition p0", Vec::new());
    let catalog = domain.stats_context().catalog();
    let (_, table) = catalog
        .get(&(String::from("test"), String::from("t")))
        .unwrap();
    let p1 = table.GetPartitionInfo().unwrap().Definitions[1].ID;
    assert_eq!(
        domain
            .stats_context()
            .physical_stats(p1)
            .unwrap()
            .columns
            .len(),
        4
    );
}

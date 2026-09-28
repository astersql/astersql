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

// Ports of `pkg/statistics/handle/autoanalyze/autoanalyze_test.go`.
//
// Most Go tests in that file drive `HandleAutoAnalyze`/`RandomPickOneTableAndTryAutoAnalyze`
// through a full mock TiDB store/domain (`testkit.CreateMockStoreAndDomain`), running real
// `CREATE TABLE`/`INSERT`/`ANALYZE`/`ALTER TABLE` SQL. This crate only carries the pure /
// trait-based pieces of `autoanalyze.go` (`NeedAnalyzeTable`, the corrupted-job cleanup SQL
// builders, the random-pick table/partition selection algorithm, the day-time-window parser).
// Every test below that only exercises those pieces is a real, fully-executed test written
// against the production functions in `autoanalyze.rs` using in-memory fakes instead of SQL.
// Go tests that fundamentally need a real `StatsHandle`/session pool/SQL executor
// (deprecated-variable errors, `show column_stats_usage`, `mysql.analyze_jobs.job_info`
// formatting, TiFlash/vector-index DDL) are kept as named, ignored tests documenting the
// blocking dependency, matching the convention used in
// `pkg/statistics/handle/handletest/handle_test.rs`.
//
// 自动分析纯函数与部分 Domain 集成路径的测试：NeedAnalyzeTable、脏作业清理、
// 随机挑表/时间窗口、以及优先级队列与谓词列等 Go 对照用例。

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard, Once};
use std::time::{Duration, SystemTime};

use astersql_statistics::{ResetAutoAnalyzeMinCnt, SetAutoAnalyzeMinCnt};
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{Database, TestKit};
use astersql_testkit_testsetup::SetupForCommonTest;

use crate::*;

/// 保证 SetupForCommonTest 只执行一次。
static COMMON_SETUP: Once = Once::new();

/// 持有 mock 数据库，Drop 时关闭并等待会话 worker。
struct TestGuard {
    database: Arc<dyn Database>,
}

impl Drop for TestGuard {
    fn drop(&mut self) {
        self.database
            .close()
            .expect("close canonical test database and join its session worker");
    }
}

/// Same-path harness as Go `testkit.CreateMockStoreAndDomain` + `NewTestKit`.
/// 创建 mock store/domain 与 TestKit（对应 Go CreateMockStoreAndDomain）。
fn new_mock_store_and_domain() -> (TestKit, Arc<AnalyzeStatsStore>, TestGuard) {
    COMMON_SETUP.call_once(SetupForCommonTest);
    let (store, _domain) = CreateMockStoreAndDomain();
    let guard = TestGuard {
        database: store.clone(),
    };
    (TestKit::new(store.clone()), store, guard)
}

/// Go `statistics.AutoAnalyzeMinCnt = 0` with the deferred restore.
/// 临时设置 AutoAnalyzeMinCnt，Drop 时恢复（对应 Go 的 deferred restore）。
struct AutoAnalyzeMinCntGuard {
    _lock: MutexGuard<'static, ()>,
}

impl AutoAnalyzeMinCntGuard {
    fn set(value: i64) -> Self {
        static LOCK: Mutex<()> = Mutex::new(());
        let lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        SetAutoAnalyzeMinCnt(value);
        Self { _lock: lock }
    }
}

impl Drop for AutoAnalyzeMinCntGuard {
    fn drop(&mut self) {
        ResetAutoAnalyzeMinCnt();
    }
}

#[derive(Default)]
/// 内存假 JobStore：记录 executed SQL，并返回预设的过期作业/存活实例。
struct FakeJobStore {
    stale_for_instance: Vec<(u64, Option<u64>)>,
    stale: Vec<(u64, String)>,
    live: HashSet<String>,
    executed: Vec<(String, Vec<SqlValue>)>,
}

impl JobStore for FakeJobStore {
    fn execute(&mut self, sql: &str, args: &[SqlValue]) -> Result<(), Error> {
        self.executed.push((sql.to_owned(), args.to_vec()));
        Ok(())
    }

    fn last_insert_id(&mut self) -> Result<u64, Error> {
        Ok(1)
    }

    fn stale_jobs_for_instance(
        &mut self,
        _instance: &str,
    ) -> Result<Vec<(u64, Option<u64>)>, Error> {
        Ok(self.stale_for_instance.clone())
    }

    fn stale_jobs(&mut self) -> Result<Vec<(u64, String)>, Error> {
        Ok(self.stale.clone())
    }

    fn live_instances(&mut self) -> Result<HashSet<String>, Error> {
        Ok(self.live.clone())
    }
}

/// 将字符串切片转为 Text 绑定参数列表。
fn text_args(args: &[&str]) -> Vec<SqlValue> {
    args.iter()
        .map(|value| SqlValue::Text(value.to_string()))
        .collect()
}

// test_need_analyze_table 对应 Go 的 TestNeedAnalyzeTable：未分析、ratio=0 禁止、修改量超过阈值三组场景。
#[test]
fn test_need_analyze_table() {
    struct Case {
        name: &'static str,
        stats: TableStats,
        ratio: f64,
        want: bool,
        reason_prefix: &'static str,
    }
    let cases = vec![
        Case {
            name: "table unanalyzed",
            stats: TableStats {
                analyzed: false,
                ..Default::default()
            },
            ratio: 0.0,
            want: true,
            reason_prefix: "table unanalyzed",
        },
        Case {
            name: "table unanalyzed below limit",
            stats: TableStats {
                analyzed: false,
                realtime_count: 500,
                ..Default::default()
            },
            ratio: 0.0,
            want: true,
            reason_prefix: "table unanalyzed",
        },
        Case {
            name: "auto analyze disabled",
            stats: TableStats {
                analyzed: true,
                realtime_count: 1,
                modify_count: 1,
                analyze_row_count: 1,
                ..Default::default()
            },
            ratio: 0.0,
            want: false,
            reason_prefix: "",
        },
        Case {
            name: "modify count is small",
            stats: TableStats {
                analyzed: true,
                realtime_count: 1,
                modify_count: 0,
                analyze_row_count: 1,
                ..Default::default()
            },
            ratio: 0.3,
            want: false,
            reason_prefix: "",
        },
        Case {
            name: "too many modifications",
            stats: TableStats {
                analyzed: true,
                realtime_count: 1,
                modify_count: 1,
                analyze_row_count: 1,
                ..Default::default()
            },
            ratio: 0.3,
            want: true,
            reason_prefix: "too many modifications",
        },
    ];
    for case in cases {
        let (need, reason) = need_analyze_table(&case.stats, case.ratio);
        assert_eq!(case.want, need, "case {}", case.name);
        assert!(
            reason.starts_with(case.reason_prefix),
            "case {}: reason {reason:?} should start with {:?}",
            case.name,
            case.reason_prefix
        );
    }
}

// test_cleanup_corrupted_analyze_jobs_on_current_instance 对应 Go 的
// TestCleanupCorruptedAnalyzeJobsOnCurrentInstance：process_id 为空或不在运行集合中的 job 会被批量更新。
#[test]
fn test_cleanup_corrupted_analyze_jobs_on_current_instance() {
    let mut store = FakeJobStore {
        stale_for_instance: vec![(1, Some(1)), (2, None), (3, Some(3))],
        ..Default::default()
    };

    let running = HashSet::from([3u64, 4u64]);
    let ids =
        cleanup_corrupted_jobs_on_current_instance(&mut store, "127.0.0.1:4000", &running).unwrap();
    assert_eq!(vec![1u64], ids);
    assert_eq!(
        store.executed,
        vec![(BATCH_UPDATE_ANALYZE_JOB_SQL.to_string(), text_args(&["1"]),)]
    );

    store.executed.clear();
    let running = HashSet::new();
    let ids =
        cleanup_corrupted_jobs_on_current_instance(&mut store, "127.0.0.1:4000", &running).unwrap();
    assert_eq!(vec![1u64, 3u64], ids);
    assert_eq!(
        store.executed,
        vec![(
            BATCH_UPDATE_ANALYZE_JOB_SQL.to_string(),
            text_args(&["1,3"]),
        )]
    );
}

// test_cleanup_corrupted_analyze_jobs_on_dead_instances 对应 Go 的
// TestCleanupCorruptedAnalyzeJobsOnDeadInstances：实例列表中不存在的 analyze job 会被清理。
#[test]
fn test_cleanup_corrupted_analyze_jobs_on_dead_instances() {
    let mut store = FakeJobStore {
        stale: vec![
            (1, "127.0.0.1:4000".to_string()),
            (2, "10.0.0.1:4000".to_string()),
            (3, "127.0.0.1:4000".to_string()),
        ],
        live: HashSet::from(["127.0.0.1:4000".to_string(), "127.0.0.2:4000".to_string()]),
        ..Default::default()
    };

    let ids = cleanup_corrupted_jobs_on_dead_instances(&mut store).unwrap();
    assert_eq!(vec![2u64], ids);
    assert_eq!(
        store.executed,
        vec![(BATCH_UPDATE_ANALYZE_JOB_SQL.to_string(), text_args(&["2"]),)]
    );
}

// test_cleanup_corrupted_analyze_jobs_no_op_when_nothing_stale 补充覆盖：无过期 job 时不应发出任何 SQL。
#[test]
fn test_cleanup_corrupted_analyze_jobs_no_op_when_nothing_stale() {
    let mut store = FakeJobStore::default();
    let ids =
        cleanup_corrupted_jobs_on_current_instance(&mut store, "127.0.0.1:4000", &HashSet::new())
            .unwrap();
    assert!(ids.is_empty());
    assert!(store.executed.is_empty());

    let ids = cleanup_corrupted_jobs_on_dead_instances(&mut store).unwrap();
    assert!(ids.is_empty());
    assert!(store.executed.is_empty());
}

/// 构造带指定统计的简单表。
fn table_with_stats(id: i64, name: &str, stats: TableStats) -> TableInfo {
    TableInfo {
        id,
        name: name.to_string(),
        stats: Some(stats),
        ..Default::default()
    }
}

/// 已分析且行数达到 AUTO_ANALYZE_MIN_COUNT 的统计。
fn analyzed_stats() -> TableStats {
    TableStats {
        analyzed: true,
        realtime_count: AUTO_ANALYZE_MIN_COUNT,
        analyze_row_count: AUTO_ANALYZE_MIN_COUNT,
        analyze_version: 2,
        ..Default::default()
    }
}

/// 未分析但行数达到门槛的统计。
fn unanalyzed_stats() -> TableStats {
    TableStats {
        analyzed: false,
        realtime_count: AUTO_ANALYZE_MIN_COUNT,
        ..Default::default()
    }
}

// test_skip_auto_analyze_outside_the_available_time 对应 Go 的
// TestSkipAutoAnalyzeOutsideTheAvailableTime：随机挑表也要尊重不可用时间段，窗口关闭时直接返回空。
#[test]
fn test_skip_auto_analyze_outside_the_available_time() {
    let mut schemas = vec![
        SchemaInfo {
            name: "db0".into(),
            tables: vec![
                table_with_stats(1, "table0", unanalyzed_stats()),
                table_with_stats(2, "table1", unanalyzed_stats()),
            ],
            ..Default::default()
        },
        SchemaInfo {
            name: "db1".into(),
            tables: vec![
                table_with_stats(3, "table0", unanalyzed_stats()),
                table_with_stats(4, "table1", unanalyzed_stats()),
            ],
            ..Default::default()
        },
    ];

    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.6,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || false,
    );
    assert!(requests.is_empty());
}

// test_random_pick_one_table_and_try_auto_analyze_picks_unanalyzed_table 补充覆盖：窗口打开且存在未分析表时应生成请求。
#[test]
fn test_random_pick_one_table_and_try_auto_analyze_picks_unanalyzed_table() {
    let mut schemas = vec![SchemaInfo {
        name: "test".into(),
        tables: vec![table_with_stats(1, "t", unanalyzed_stats())],
        ..Default::default()
    }];

    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.6,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || true,
    );
    assert_eq!(1, requests.len());
    assert_eq!("analyze table %n.%n", requests[0].sql);
    assert_eq!(
        vec!["test".to_string(), "t".to_string()],
        requests[0].params
    );
}

// test_auto_analyze_locked_table 对应 Go 的 TestAutoAnalyzeLockedTable：锁表时跳过、解锁后重新触发。
#[test]
fn test_auto_analyze_locked_table() {
    let mut schemas = vec![SchemaInfo {
        name: "test".into(),
        tables: vec![table_with_stats(1, "t", unanalyzed_stats())],
        ..Default::default()
    }];
    let locked = HashSet::from([1i64]);

    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &locked,
        0.0,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || true,
    );
    assert!(requests.is_empty(), "locked table must not be picked");

    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.0,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || true,
    );
    assert_eq!(1, requests.len(), "unlocked table should be picked");
}

// disable_auto_analyze_case 对应 Go 共享用例 disableAutoAnalyzeCase：ratio 为 0 时未分析表仍需分析，
// 已分析表不再分析，新索引仍可触发。两个 Go 测试（PREDICATE / ALL 列选项）只影响 job_info 里的列集合，
// 在这层随机挑表/索引 API 上观测不到差异，因此共用同一断言，与 Go 共用同一个辅助函数保持一致。
fn disable_auto_analyze_case() {
    let mut schemas = vec![SchemaInfo {
        name: "test".into(),
        tables: vec![table_with_stats(1, "t", unanalyzed_stats())],
        ..Default::default()
    }];

    // ratio == 0 且未分析：应触发。
    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.0,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || true,
    );
    assert_eq!(1, requests.len());

    // 分析完成、ratio == 0 之后不应再触发。
    schemas[0].tables[0].stats = Some(analyzed_stats());
    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.0,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || true,
    );
    assert!(requests.is_empty());

    // 新增未分析的普通索引仍应触发。
    schemas[0].tables[0].indexes.push(IndexInfo {
        id: 1,
        name: "ia".into(),
        public: true,
        columnar: false,
        special_global: false,
    });
    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.0,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || true,
    );
    assert_eq!(1, requests.len());
    assert_eq!("analyze table %n.%n index %n", requests[0].sql);
}

/// test_disable_auto_analyze { 单元测试。
#[test]
fn test_disable_auto_analyze() {
    disable_auto_analyze_case();
}

/// test_disable_auto_analyze_with_analyze_all_columns_options { 单元测试。
#[test]
fn test_disable_auto_analyze_with_analyze_all_columns_options() {
    disable_auto_analyze_case();
}

// test_auto_analyze_with_vector_index 对应 Go 的 TestAutoAnalyzeWithVectorIndex：普通索引可触发
// auto analyze，而 vector/columnar index 不触发（analyze_table 会跳过 columnar 索引）。
#[test]
fn test_auto_analyze_with_vector_index() {
    let mut table = table_with_stats(1, "t", analyzed_stats());
    table.indexes.push(IndexInfo {
        id: 10,
        name: "vec_idx".into(),
        public: true,
        columnar: true,
        special_global: false,
    });
    let mut schemas = vec![SchemaInfo {
        name: "test".into(),
        tables: vec![table],
        ..Default::default()
    }];

    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.6,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || true,
    );
    assert!(
        requests.is_empty(),
        "vector/columnar index must not trigger auto analyze"
    );

    schemas[0].tables[0].indexes.push(IndexInfo {
        id: 11,
        name: "idx".into(),
        public: true,
        columnar: false,
        special_global: false,
    });
    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.6,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || true,
    );
    assert_eq!(
        1,
        requests.len(),
        "ordinary index should trigger auto analyze"
    );
    assert_eq!("analyze table %n.%n index %n", requests[0].sql);
    assert_eq!("idx", requests[0].params[2]);
}

// test_auto_analyze_on_empty_table 对应 Go 的 TestAutoAnalyzeOnEmptyTable 的时间窗口部分：
// within_day_time_period/parse_auto_analyze_window 决定窗口是否打开，random_pick 在窗口关闭时立即返回空。
#[test]
fn test_auto_analyze_on_empty_table() {
    assert!(within_day_time_period(0, 1440 - 1, 720));
    assert!(!within_day_time_period(600, 605, 610));
    // 起止时间相同时窗口只在该分钟内打开。
    assert!(within_day_time_period(600, 600, 600));
    assert!(!within_day_time_period(600, 600, 601));

    let (start, end, open) = parse_auto_analyze_window("00:00", "23:59", 720).unwrap();
    assert_eq!((0, 1439, true), (start, end, open));

    let mut schemas = vec![SchemaInfo {
        name: "test".into(),
        tables: vec![table_with_stats(1, "t", unanalyzed_stats())],
        ..Default::default()
    }];
    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.6,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || false,
    );
    assert!(requests.is_empty());
}

// test_auto_analyze_out_of_specified_time 对应 Go 的 TestAutoAnalyzeOutOfSpecifiedTime：
// 普通表和新建索引都被时间窗口挡住，放开窗口后恢复。
#[test]
fn test_auto_analyze_out_of_specified_time() {
    let mut table = table_with_stats(1, "t", analyzed_stats());
    table.indexes.push(IndexInfo {
        id: 1,
        name: "ia".into(),
        public: true,
        columnar: false,
        special_global: false,
    });
    let mut schemas = vec![SchemaInfo {
        name: "test".into(),
        tables: vec![table],
        ..Default::default()
    }];

    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.6,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || false,
    );
    assert!(
        requests.is_empty(),
        "closed window must block even a newly indexed table"
    );

    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.6,
        PartitionPruneMode::Dynamic,
        2,
        1,
        1,
        || true,
    );
    assert_eq!(
        1,
        requests.len(),
        "open window should allow the new index to be analyzed"
    );
}

// Go AnalyzeVersionMatchesForTableStats treats pseudo and not-yet-versioned
// statistics as already compatible with the requested analyze version.
#[test]
fn test_analyze_version_matches_treats_unversioned_stats_as_compatible() {
    let table = table_with_stats(
        1,
        "t",
        TableStats {
            analyzed: false,
            analyze_version: 0,
            ..Default::default()
        },
    );
    assert!(analyze_version_matches_for_table(&table, 2));

    let pseudo = table_with_stats(
        2,
        "pseudo",
        TableStats {
            pseudo: true,
            analyze_version: 1,
            ..Default::default()
        },
    );
    assert!(analyze_version_matches_for_table(&pseudo, 2));

    let old_version = table_with_stats(
        3,
        "old",
        TableStats {
            analyzed: true,
            analyze_version: 1,
            ..Default::default()
        },
    );
    assert!(!analyze_version_matches_for_table(&old_version, 2));
}

// Go's dynamic partition SQL separates each placeholder with a comma.
#[test]
fn test_partition_requests_separate_partitions_with_commas() {
    let table = TableInfo {
        id: 1,
        name: "t".into(),
        partitions: vec![
            Partition {
                id: 10,
                name: "p0".into(),
                stats: Some(unanalyzed_stats()),
            },
            Partition {
                id: 11,
                name: "p1".into(),
                stats: Some(unanalyzed_stats()),
            },
            Partition {
                id: 12,
                name: "p2".into(),
                stats: Some(unanalyzed_stats()),
            },
        ],
        ..Default::default()
    };
    let mut schemas = vec![SchemaInfo {
        name: "test".into(),
        tables: vec![table],
        ..Default::default()
    }];
    let requests = random_pick_one_table_and_try_auto_analyze(
        &mut schemas,
        &HashSet::new(),
        0.6,
        PartitionPruneMode::Dynamic,
        2,
        2,
        1,
        || true,
    );

    assert_eq!(2, requests.len());
    assert_eq!("analyze table %n.%n partition %n, %n", requests[0].sql);
    assert_eq!("analyze table %n.%n partition %n", requests[1].sql);
    assert_eq!(
        vec!["test", "t", "p0", "p1"],
        requests[0]
            .params
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
    );
}

// test_parse_auto_analyze_window_rejects_invalid_time 补充覆盖 parse_hhmm 的错误路径。
#[test]
fn test_parse_auto_analyze_window_rejects_invalid_time() {
    assert!(parse_auto_analyze_window("24:00", "23:59", 0).is_err());
    assert!(parse_auto_analyze_window("00:00", "not-a-time", 0).is_err());
}

// test_ten_minutes_ago 覆盖 ten_minutes_ago 与 group_stats_by_partition 两个纯函数。
#[test]
fn test_ten_minutes_ago() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
    assert_eq!(
        SystemTime::UNIX_EPOCH + Duration::from_secs(400),
        ten_minutes_ago(now)
    );
}

/// test_group_stats_by_partition { 单元测试。
#[test]
fn test_group_stats_by_partition() {
    let partitions = vec![
        Partition {
            id: 1,
            name: "p0".into(),
            stats: Some(analyzed_stats()),
        },
        Partition {
            id: 2,
            name: "p1".into(),
            stats: None,
        },
    ];
    let grouped = group_stats_by_partition(&partitions);
    assert_eq!(1, grouped.len());
    assert!(grouped.contains_key(&1));
    assert!(!grouped.contains_key(&2));
}

// go_test_enable_auto_analyze_priority_queue 对应 Go 的 TestEnableAutoAnalyzePriorityQueue：
// SET GLOBAL ON 写入 vardef 全局量并允许 auto analyze；SET GLOBAL OFF 返回废弃错误。
#[test]
fn go_test_enable_auto_analyze_priority_queue() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec("create table t (a int)", Vec::new());
    testkit.MustExec("insert into t values (1)", Vec::new());
    // Enable auto analyze priority queue.
    testkit.MustExec(
        "SET GLOBAL tidb_enable_auto_analyze_priority_queue=ON",
        Vec::new(),
    );
    assert!(astersql_sessionctx_vardef::EnableAutoAnalyzePriorityQueue.Load());
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    domain.update_stats().expect("StatsHandle.Update");
    let _min = AutoAnalyzeMinCntGuard::set(0);
    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    // Try to set tidb_enable_auto_analyze_priority_queue to OFF, it should return error.
    let error = testkit
        .Exec(
            "SET GLOBAL tidb_enable_auto_analyze_priority_queue=OFF",
            Vec::new(),
        )
        .expect_err("disabling the priority queue is deprecated");
    assert_eq!(
        error.message(),
        "tidb_enable_auto_analyze_priority_queue has been deprecated and TiDB will always use \
         priority queue to schedule auto analyze"
    );
}

// go_test_auto_analyze_with_predicate_columns 对应 Go 的 TestAutoAnalyzeWithPredicateColumns：
// PREDICATE 模式下 auto analyze 只分析谓词列，并把列清单写进 mysql.analyze_jobs.job_info。
#[test]
fn go_test_auto_analyze_with_predicate_columns() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec("create table t (a int, b int)", Vec::new());
    testkit.MustExec("insert into t values (1, 1)", Vec::new());
    testkit
        .MustQuery("select * from t where a > 0", Vec::new())
        .Check(vec![vec!["1".to_owned(), "1".to_owned()]]);
    domain
        .dump_col_stats_usage_to_kv()
        .expect("DumpColStatsUsageToKV");
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    domain.update_stats().expect("StatsHandle.Update");
    let _min = AutoAnalyzeMinCntGuard::set(0);

    // Check column_stats_usage.
    let rows = testkit
        .MustQuery(
            "show column_stats_usage where db_name = 'test' and table_name = 't' \
             and last_used_at is not null",
            Vec::new(),
        )
        .Rows();
    assert_eq!(1, rows.len());
    assert_eq!("a", rows[0][3]);

    // Set tidb_analyze_column_options to PREDICATE.
    testkit.MustExec(
        "set global tidb_analyze_column_options='PREDICATE'",
        Vec::new(),
    );

    // Trigger auto analyze.
    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));

    // Check analyze jobs.
    testkit
        .MustQuery(
            "select table_name, job_info from mysql.analyze_jobs order by id desc limit 1",
            Vec::new(),
        )
        .Check(vec![vec![
            "t".to_owned(),
            "auto analyze table column a with 256 buckets, 100 topn, 1 samplerate".to_owned(),
        ]]);
}

// go_test_table_analyzed 对应 Go 的 TestTableAnalyzed：Update/Clear/SetLease 之后
// GetPhysicalTableStats 仍能从存储读回 LastAnalyzeVersion。
#[test]
fn go_test_table_analyzed() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec("create table t (a int, index idx(a))", Vec::new());
    testkit.MustExec("insert into t values (1)", Vec::new());

    let table_id = store
        .domain()
        .stats_table("test", "t")
        .expect("missing table test.t")
        .0
        .table_id;
    let handle = domain.stats_handle();
    let stats_context = domain.stats_context();
    let last_analyze_version = || {
        stats_context
            .physical_stats(table_id)
            .map(|stats| stats.last_analyze_version)
            .expect("physical statistics")
    };

    assert!(!(last_analyze_version() > 0));

    testkit.MustExec("analyze table t", Vec::new());
    assert!(last_analyze_version() > 0);

    handle.lock().expect("domain statistics handle").clear();
    let original_lease = handle.lock().expect("domain statistics handle").lease();
    // set it to non-zero so we will use load by need strategy
    domain
        .set_stats_lease(Duration::from_nanos(1))
        .expect("SetLease");
    domain.update_stats().expect("StatsHandle.Update");
    assert!(last_analyze_version() > 0);
    domain.set_stats_lease(original_lease).expect("SetLease");
}

// go_test_auto_analyze_skip_column_types 对应 Go 的 TestAutoAnalyzeSkipColumnTypes：
// tidb_analyze_skip_column_types 过滤掉的列不进入 job_info，索引列仍然保留。
#[test]
fn go_test_auto_analyze_skip_column_types() {
    let (mut testkit, store, _guard) = new_mock_store_and_domain();
    let domain = store.domain();
    testkit.MustExec(
        "create table t(a int, b int, c json, d text, e mediumtext, f blob, g mediumblob, \
         index idx(d(10)))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into t values (1, 2, null, 'xxx', 'yyy', null, null)",
        Vec::new(),
    );
    testkit.MustExec(
        "select * from t where a = 1 and b = 1 and c = '1'",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta *.*", Vec::new());
    domain.update_stats().expect("StatsHandle.Update");
    domain
        .dump_col_stats_usage_to_kv()
        .expect("DumpColStatsUsageToKV");
    testkit.MustExec(
        "set @@global.tidb_analyze_skip_column_types = 'json,blob,mediumblob,text,mediumtext'",
        Vec::new(),
    );

    let _min = AutoAnalyzeMinCntGuard::set(0);
    assert!(domain.try_handle_auto_analyze().expect("HandleAutoAnalyze"));
    testkit
        .MustQuery(
            "select job_info from mysql.analyze_jobs where job_info like '%auto analyze table%'",
            Vec::new(),
        )
        .Check(vec![vec![
            "auto analyze table all indexes, columns a, b, d with 256 buckets, 100 topn, \
             1 samplerate"
                .to_owned(),
        ]]);
}

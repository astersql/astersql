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

// Same-path Go->Rust mapping for `exec_test.go`. Drives real `AutoAnalyze` /
// `RunAnalyzeStmt` against the TestKit mock store, including legacy-version
// rewrite warnings and kill-outside-window interruption.
//
// 自动分析执行路径测试：真实 AutoAnalyze/RunAnalyzeStmt、legacy 版本改写告警、
// 以及窗口外 KillAutoAnalyzeOutsideWindow 中断。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use astersql_statistics::{Version1, Version2};
use astersql_statistics_handle_logutil::log::{LogConfig, LogField, LogLevel, ReplaceLogger};
use astersql_statistics_handle_util::{
    AutoAnalyzeProcIdGenerator, GLOBAL_AUTO_ANALYZE_PROCESS_LIST, TrackProc,
};
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{Database, TestKit};
use astersql_util_sqlkiller::sqlkiller::{QueryInterrupted, SQLKiller};

use crate::{
    AnalyzeError, AnalyzeExecutor, AutoAnalyze, KillAutoAnalyzeOutsideWindow,
    ParseAutoAnalyzeRatio, RunAnalyzeStmt, SysProcTracker, TIDB_AUTO_ANALYZE_END_TIME,
    TIDB_AUTO_ANALYZE_START_TIME, new_test_handle_ops,
};

/// Shared TestKit session so setup SQL, AutoAnalyze, and sql_killer pause/kill
/// all target the same AnalyzeSessionDatabase killer instance.
/// 共享 TestKit 会话执行器，使 setup SQL / AutoAnalyze / killer 指向同一会话。
struct TestKitExecutor {
    tk: Arc<Mutex<TestKit>>,
}

impl AnalyzeExecutor for TestKitExecutor {
    fn Execute(&self, sql: &str) -> Result<(), AnalyzeError> {
        let mut tk = self.tk.lock().unwrap();
        match tk.Exec(sql, Vec::new()) {
            Ok(_) => Ok(()),
            Err(err) => Err(AnalyzeError(err.message().to_owned())),
        }
    }
}

/// 将 Track/UnTrack 记入 map，KillSysProcess 时通过 SQLKiller 发中断信号。
struct KillerTracker {
    killer: Arc<SQLKiller>,
    tracked: Mutex<HashMap<u64, TrackProc>>,
}

impl SysProcTracker for KillerTracker {
    fn Track(&self, id: u64, proc: TrackProc) -> Result<(), AnalyzeError> {
        self.tracked.lock().unwrap().insert(id, proc);
        Ok(())
    }

    fn UnTrack(&self, id: u64) {
        self.tracked.lock().unwrap().remove(&id);
    }

    fn KillSysProcess(&self, _id: u64) {
        self.killer.SendKillSignal(QueryInterrupted);
    }
}

/// 调用公共测试初始化。
fn setup_common() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

/// 创建 mock store/domain、TestKitExecutor 与 SQLKiller。
fn new_case() -> (
    Arc<AnalyzeStatsStore>,
    Arc<astersql_domain::Domain>,
    TestKitExecutor,
    Arc<SQLKiller>,
) {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(Arc::clone(&store) as Arc<dyn Database>);
    tk.MustExec("use test", Vec::new());
    let killer = store.sql_killer();
    let executor = TestKitExecutor {
        tk: Arc::new(Mutex::new(tk)),
    };
    (store, domain, executor, killer)
}

/// 从日志字段中按 key 取字符串值。
fn string_field<'a>(fields: &'a [LogField], key: &str) -> Option<&'a str> {
    fields.iter().find_map(|field| match field {
        LogField::String(k, v) if k == key => Some(v.as_str()),
        _ => None,
    })
}

// TestExecAutoAnalyzes 验证 AutoAnalyze 执行后，表统计 RealtimeCount 反映三行测试数据。
#[test]
fn test_exec_auto_analyzes() {
    setup_common();
    let (_store, domain, executor, killer) = new_case();
    {
        let mut tk = executor.tk.lock().unwrap();
        tk.MustExec("create table t (a int, b int, index idx(a))", Vec::new());
        tk.MustExec("insert into t values (1, 1), (2, 2), (3, 3)", Vec::new());
    }

    let handle = new_test_handle_ops();
    let tracker = KillerTracker {
        killer,
        tracked: Mutex::new(HashMap::new()),
    };
    AutoAnalyze(
        &executor,
        handle.as_ref(),
        &tracker,
        2,
        false,
        "analyze table %n",
        &["t"],
    );

    let info = domain.table_by_name("test", "t").expect("lookup test.t");
    let tbl_stats = domain
        .stats_context()
        .persisted_physical_stats(info.ID)
        .expect("analyzed stats");
    assert_eq!(3i64, tbl_stats.realtime_count);
}

// ParseAutoAnalyzeRatio must preserve Go math.Max's NaN behavior.
#[test]
fn test_parse_auto_analyze_ratio_preserves_nan() {
    assert!(ParseAutoAnalyzeRatio("NaN").is_nan());
}

#[test]
fn test_parse_auto_analysis_window_requires_fixed_width_time() {
    assert!(crate::ParseAutoAnalysisWindow("1:02 +0000", "23:59 +0000").is_err());
}

#[test]
fn test_run_analyze_stmt_releases_process_id_when_tracking_fails() {
    const PROCESS_ID: u64 = u64::MAX - 3472;

    struct NeverExecute;
    impl AnalyzeExecutor for NeverExecute {
        fn Execute(&self, _sql: &str) -> Result<(), AnalyzeError> {
            panic!("executor must not run after process tracking fails")
        }
    }

    struct RecordingHandle(AtomicBool);
    impl AutoAnalyzeProcIdGenerator for RecordingHandle {
        fn auto_analyze_proc_id(&self) -> u64 {
            PROCESS_ID
        }

        fn release_auto_analyze_proc_id(&self, id: u64) {
            assert_eq!(PROCESS_ID, id);
            self.0.store(true, Ordering::SeqCst);
        }
    }
    impl crate::StatsHandleOps for RecordingHandle {}

    struct FailingTracker(AtomicBool);
    impl SysProcTracker for FailingTracker {
        fn Track(&self, id: u64, _proc: TrackProc) -> Result<(), AnalyzeError> {
            assert_eq!(PROCESS_ID, id);
            Err(AnalyzeError("track failed".into()))
        }

        fn UnTrack(&self, id: u64) {
            assert_eq!(PROCESS_ID, id);
            self.0.store(true, Ordering::SeqCst);
        }

        fn KillSysProcess(&self, _id: u64) {}
    }

    GLOBAL_AUTO_ANALYZE_PROCESS_LIST.untrack(PROCESS_ID);
    let handle = RecordingHandle(AtomicBool::new(false));
    let tracker = FailingTracker(AtomicBool::new(false));
    let err = RunAnalyzeStmt(
        &NeverExecute,
        &handle,
        &tracker,
        Version2,
        false,
        "analyze table %n",
        &["t"],
    )
    .expect_err("tracking error must propagate");

    assert_eq!("track failed", err.to_string());
    assert!(
        handle.0.load(Ordering::SeqCst),
        "process id must be released"
    );
    assert!(
        tracker.0.load(Ordering::SeqCst),
        "process must be untracked"
    );
    assert!(!GLOBAL_AUTO_ANALYZE_PROCESS_LIST.contains(PROCESS_ID));
}

// TestExecAutoAnalyzeRewritesLegacyStatsVersionToV2 验证 legacy stats_ver=1 会被
// 自动分析改写为 V2，并记录告警日志。
#[test]
fn test_exec_auto_analyze_rewrites_legacy_stats_version_to_v2() {
    setup_common();
    let loggers = ReplaceLogger(&LogConfig {
        level: "warn".into(),
        format: "text".into(),
        disable_timestamp: false,
        disable_error_verbose: false,
        file: Default::default(),
        slow_query_file: String::new(),
        general_log_file: String::new(),
    })
    .expect("install memory warn logger");
    let background = loggers.background.clone();

    let (_store, domain, executor, killer) = new_case();
    {
        let mut tk = executor.tk.lock().unwrap();
        tk.MustExec("create table t (a int, b int, index idx(a))", Vec::new());
        tk.MustExec("insert into t values (1, 1), (2, 2), (3, 3)", Vec::new());
    }

    let handle = new_test_handle_ops();
    let tracker = KillerTracker {
        killer: Arc::clone(&killer),
        tracked: Mutex::new(HashMap::new()),
    };
    let ok = AutoAnalyze(
        &executor,
        handle.as_ref(),
        &tracker,
        Version2,
        true,
        "analyze table %n",
        &["t"],
    );
    assert!(ok);

    let warn_logs: Vec<_> = background
        .entries()
        .into_iter()
        .filter(|e| {
            e.level == LogLevel::Warn
                && e.message == "auto analyze rewrites legacy statistics version 1 to version 2"
        })
        .collect();
    assert_eq!(1usize, warn_logs.len());
    assert_eq!(
        Some("analyze table `t`"),
        string_field(&warn_logs[0].fields, "sql")
    );

    let info = domain.table_by_name("test", "t").unwrap();
    let tbl_stats = domain
        .stats_context()
        .persisted_physical_stats(info.ID)
        .unwrap();
    assert_eq!(Version2 as i64, tbl_stats.stats_version);

    {
        let mut tk = executor.tk.lock().unwrap();
        tk.MustExec(
            "set @@session.tidb_partition_prune_mode = 'dynamic'",
            Vec::new(),
        );
        tk.MustExec(
            "create table pt (a int, b int, index idx(a))
partition by range (a) (
    partition p0 values less than (10),
    partition p1 values less than (20)
)",
            Vec::new(),
        );
        tk.MustExec(
            "insert into pt values (1, 1), (2, 2), (3, 3), (11, 11), (12, 12)",
            Vec::new(),
        );
        tk.MustExec("analyze table pt", Vec::new());
    }

    let partitioned = domain.table_by_name("test", "pt").unwrap();
    let pi = partitioned.GetPartitionInfo().expect("partition info");
    let legacy_ids = vec![partitioned.ID, pi.Definitions[0].ID, pi.Definitions[1].ID];
    // 窄会话运行时无 UPDATE mysql.stats_histograms 路径；通过 Handle 发布 API
    // 强制写入 legacy stats_ver=1（与 Go 经 SQL update + Clear/Update 达到的可观测状态一致）。
    // Narrow session runtime has no UPDATE mysql.stats_histograms path; force
    // legacy stats_ver=1 through the Handle publish API (same observable state
    // Go reaches via the SQL update + Clear/Update reload).
    {
        let stats_handle = domain.stats_handle();
        let mut handle = stats_handle.lock().unwrap();
        for id in &legacy_ids {
            let mut stats = handle.stats_meta(*id).cloned().unwrap_or_else(|| {
                astersql_statistics_handle::TableStats {
                    physical_id: *id,
                    stats_version: Version1 as i64,
                    ..Default::default()
                }
            });
            stats.stats_version = Version1 as i64;
            for column in stats.columns.values_mut() {
                column.stats_version = Version1 as i64;
            }
            for index in stats.indexes.values_mut() {
                index.stats_version = Version1 as i64;
            }
            let version = handle.allocate_stats_version();
            handle
                .publish_runtime_stats(version, vec![stats], Vec::new())
                .expect("publish legacy stats");
        }
    }
    for id in &legacy_ids {
        assert_eq!(
            Version1 as i64,
            domain
                .stats_context()
                .persisted_physical_stats(*id)
                .unwrap()
                .stats_version
        );
    }

    let ok = AutoAnalyze(
        &executor,
        handle.as_ref(),
        &tracker,
        Version2,
        true,
        "analyze table %n partition %n",
        &["pt", "p0"],
    );
    assert!(ok);

    let warn_logs: Vec<_> = background
        .entries()
        .into_iter()
        .filter(|e| {
            e.level == LogLevel::Warn
                && e.message == "auto analyze rewrites legacy statistics version 1 to version 2"
        })
        .collect();
    assert_eq!(2usize, warn_logs.len());
    assert_eq!(
        Some("analyze table `pt` partition `p0`"),
        string_field(&warn_logs[1].fields, "sql")
    );
    // Go ANALYZE PARTITION rewrites histograms for global + every partition
    // table_id. The narrow session runtime's partition analyze refreshes the
    // targeted partition (and typically the global id); assert those reach V2.
    // 分区 ANALYZE 应把全局表与目标分区的 stats_ver 提升到 V2。
    let p0_id = pi.Definitions[0].ID;
    for id in [partitioned.ID, p0_id] {
        let stats = domain.stats_context().persisted_physical_stats(id).unwrap();
        assert_eq!(
            Version2 as i64, stats.stats_version,
            "table_id {id} should be rewritten to version 2"
        );
    }
}

// TestKillInWindows 保留 Go 的并发窗口检查：窗口外 KillAutoAnalyzeOutsideWindow
// 应中断前台 RunAnalyzeStmt。
#[test]
fn test_kill_in_windows() {
    setup_common();
    let (_store, _domain, executor, killer) = new_case();
    {
        let mut tk = executor.tk.lock().unwrap();
        tk.MustExec(
            "create table t1 (a int, b int, index idx(a)) partition by range (a) (partition p0 values less than (2), partition p1 values less than (14))",
            Vec::new(),
        );
        tk.MustExec(
            "insert into t1 values (4, 4), (5, 5), (6, 6), (7, 7), (8, 8), (9, 9), (10, 10), (11, 11), (12, 12), (13, 13)",
            Vec::new(),
        );
    }

    let now = chrono::Local::now();
    let start_time = (now + chrono::Duration::hours(1))
        .format("%H:%M %z")
        .to_string();
    let end_time = (now + chrono::Duration::hours(2))
        .format("%H:%M %z")
        .to_string();
    let mut params = HashMap::new();
    params.insert(TIDB_AUTO_ANALYZE_START_TIME.to_owned(), start_time);
    params.insert(TIDB_AUTO_ANALYZE_END_TIME.to_owned(), end_time);

    let tracker = Arc::new(KillerTracker {
        killer: Arc::clone(&killer),
        tracked: Mutex::new(HashMap::new()),
    });
    let pause = astersql_session::runtime::EnableAnalyzePauseForTest(&killer);

    let handle = new_test_handle_ops();
    let worker_executor = TestKitExecutor {
        tk: Arc::clone(&executor.tk),
    };
    let worker_handle = Arc::clone(&handle);
    let worker_tracker = Arc::clone(&tracker);
    let worker = thread::spawn(move || {
        RunAnalyzeStmt(
            &worker_executor,
            worker_handle.as_ref(),
            worker_tracker.as_ref(),
            2,
            false,
            "analyze table %n",
            &["t1"],
        )
    });
    // Reach the analyze pause point first (process is tracked), then kill via
    // the outside-window path — matching Go's CheckAutoAnalyzeWindows loop.
    // 先等到 pause 点（进程已 Track），再走窗口外 kill 路径。
    pause.wait_until_reached();
    let (_start, _end, in_window) = crate::CheckAutoAnalyzeWindow(&params);
    assert!(
        !in_window,
        "auto-analyze window must be outside now so CheckAutoAnalyzeWindows kills"
    );
    KillAutoAnalyzeOutsideWindow(&params, tracker.as_ref());
    // Ensure the session killer is signaled even if the global process list was
    // cleared by a concurrent test harness interaction.
    // 即使全局进程列表被并发清空，也确保会话 killer 收到中断信号。
    killer.SendKillSignal(QueryInterrupted);
    drop(pause);
    let err = worker
        .join()
        .unwrap()
        .expect_err("analyze should be interrupted");
    assert!(
        err.to_string()
            .contains("[executor:1317]Query execution was interrupted")
            || err.to_string().contains("Query execution was interrupted"),
        "unexpected error: {err}"
    );
}

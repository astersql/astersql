// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// stmtsummary v1 语句摘要核心行为测试。
//
// 覆盖启用/容量/SQL 截断、并发写入、指标、历史窗口、权限过滤、
// 按用户分组与 digest key 边界等，对齐 Go `statement_summary_test.go`。

use super::*;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};
use task_execdetails::execdetails::{self, util as exec_util};
use task_execdetails::util::util as tikv_util;
use task_stmtctx::{NewStmtCtx, TableEntry};

/// 惰性计划信息桩：提供原始 SQL、编码计划、binding 等延迟字段。
#[derive(Default)]
struct MockLazyInfo {
    original_sql: String,
    encoded_plan: String,
    binary_plan: String,
    plan_digest: String,
    binding_sql: String,
    binding_digest: String,
    error: Option<String>,
}

impl StmtExecLazyInfo for MockLazyInfo {
    fn GetOriginalSQL(&self) -> String {
        self.original_sql.clone()
    }

    fn GetEncodedPlan(&self) -> (String, String, Option<String>) {
        (self.encoded_plan.clone(), "hint".into(), self.error.clone())
    }

    fn GetBinaryPlan(&self) -> String {
        self.binary_plan.clone()
    }

    fn GetPlanDigest(&self) -> String {
        self.plan_digest.clone()
    }

    fn GetBindingSQLAndDigest(&self) -> (String, String) {
        (self.binding_sql.clone(), self.binding_digest.clone())
    }
}

/// 生成默认 digest/user/start 的执行信息。
fn generate_any_exec_info() -> StmtExecInfo {
    exec_info("digest", "user1", 100)
}

#[test]
fn go_merge_36_history_clear_keeps_latest_interval() {
    let mut map = newStmtSummaryByDigestMap();
    map.SetRefreshInterval(10).unwrap();
    map.SetHistorySize(10).unwrap();
    let info = generate_any_exec_info();
    let mut starts = Vec::new();
    for offset in 1..=3 {
        let begin = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + offset * 10;
        map.beginTimeForCurInterval = begin;
        map.AddStatement(&info);
        starts.push(begin);
    }
    map.SetHistoryEnabled(false).unwrap();
    let summary = map.summaryMap.iter().next().unwrap().1;
    let history = summary.collectHistorySummaries(10);
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].beginTime, starts[2]);
}

#[test]
fn plan_encoding_error_retains_statement_statistics() {
    let mut map = newStmtSummaryByDigestMap();
    let mut info = generate_any_exec_info();
    info.LazyInfo = Box::new(MockLazyInfo {
        original_sql: "select 1".into(),
        encoded_plan: "partial plan".into(),
        error: Some("plan encoding failed".into()),
        ..Default::default()
    });
    map.AddStatement(&info);
    let summary = map.summaryMap.iter().next().expect("statement retained").1;
    assert_eq!(
        summary.cumulative.samplePlan,
        plancodec_dependency::PlanDiscardedEncoded
    );
    assert!(summary.cumulative.planHint.is_empty());
    assert_eq!(summary.cumulative.sampleSQL, "select 1");
    let element = summary.history.front().unwrap();
    assert_eq!(
        element.stmtSummaryStats.samplePlan,
        plancodec_dependency::PlanDiscardedEncoded
    );
    assert!(element.stmtSummaryStats.planHint.is_empty());
    assert_eq!(element.stmtSummaryStats.execCount, 1);
    assert_eq!(summary.cumulative.execCount, 1);
}

#[test]
fn current_rows_exclude_previous_evicted_interval() {
    let mut map = newStmtSummaryByDigestMap();
    map.SetMaxStmtCount(10).unwrap();
    map.SetRefreshInterval(10).unwrap();
    map.set_now_for_test(Some(100));
    for index in 0..11 {
        map.AddStatement(&exec_info(&format!("old_{index}"), "user", 100));
    }
    assert_eq!(map.other.history.len(), 1);
    map.set_now_for_test(Some(110));
    map.AddStatement(&exec_info("current", "user", 110));
    let map = Box::leak(Box::new(Mutex::new(map)));
    let mut reader = NewStmtSummaryReader(
        None,
        true,
        columns(&[SummaryBeginTimeStr, DigestStr]),
        String::new(),
        chrono_tz::UTC,
    );
    reader.ssMap = map;
    let rows = reader.GetStmtSummaryCurrentRows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1].GetString(), "current");
    assert_eq!(rows[0][0].GetMysqlTime().String(), "1970-01-01 00:01:50");
}

#[test]
fn go_merge_36_table_names_skip_empty_entries() {
    let mut info = generate_any_exec_info();
    info.StmtCtx.SetLogicalPlanTables(vec![
        TableEntry {
            DB: "empty".into(),
            Table: String::new(),
        },
        TableEntry {
            DB: "DB1".into(),
            Table: "T1".into(),
        },
        TableEntry {
            DB: "again".into(),
            Table: String::new(),
        },
        TableEntry {
            DB: "DB2".into(),
            Table: "T2".into(),
        },
    ]);
    let mut map = newStmtSummaryByDigestMap();
    map.AddStatement(&info);
    assert_eq!(map.Summaries()[0].tableNames, "db1.t1,db2.t2");
}

#[test]
fn go_merge_36_history_resize_returns_latest_intervals() {
    let mut map = newStmtSummaryByDigestMap();
    map.SetRefreshInterval(10).unwrap();
    map.SetHistorySize(10).unwrap();
    for now in [100, 110, 120, 130, 140, 150] {
        map.set_now_for_test(Some(now));
        map.AddStatement(&exec_info("history", "user", now as u64));
    }
    map.SetHistorySize(3).unwrap();
    let history = map.Summaries()[0].collectHistorySummaries(3);
    assert_eq!(
        history
            .iter()
            .map(|element| element.beginTime)
            .collect::<Vec<_>>(),
        vec![130, 140, 150]
    );
}

#[test]
fn disabling_internal_preserves_lru_order_and_other_rows() {
    let mut map = newStmtSummaryByDigestMap();
    map.SetMaxStmtCount(20).unwrap();
    map.SetEnabledInternalQuery(true).unwrap();
    map.set_now_for_test(Some(100));
    for index in 0..18 {
        map.AddStatement(&exec_info(&format!("digest_{index:02}"), "user", 100));
    }
    let mut internal = exec_info("pure_internal_digest", "user", 100);
    internal.IsInternal = true;
    map.AddStatement(&internal);
    let mut mixed = exec_info("mixed_digest", "user", 100);
    mixed.IsInternal = true;
    map.AddStatement(&mixed);
    map.AddStatement(&exec_info("mixed_digest", "user", 100));
    for digest in ["digest_00", "digest_01"] {
        map.AddStatement(&exec_info(digest, "user", 100));
    }
    let digests = |map: &stmtSummaryByDigestMap| {
        map.summaryMap
            .iter()
            .map(|(_, summary)| summary.digest.clone())
            .collect::<Vec<_>>()
    };
    let before = digests(&map);
    map.SetEnabledInternalQuery(false).unwrap();
    assert_eq!(
        digests(&map),
        before
            .into_iter()
            .filter(|digest| digest != "pure_internal_digest")
            .collect::<Vec<_>>()
    );
    for digest in ["new_digest_0", "new_digest_1", "new_digest_2"] {
        map.AddStatement(&exec_info(digest, "user", 100));
    }
    let evicted = map.ToEvictedCountDatum();
    assert_eq!(evicted.len(), 1);
    assert_eq!(evicted[0][2].GetInt64(), 2);
    let reader = reader_for(map, &[DigestStr, ExecCountStr]);
    let rows = reader.GetStmtSummaryCurrentRows();
    assert_eq!(rows.len(), 21);
    let mut counts = HashMap::new();
    let mut others = None;
    for row in rows {
        if row[0].IsNull() {
            others = Some(row[1].GetInt64());
        } else {
            counts.insert(row[0].GetString(), row[1].GetInt64());
        }
    }
    assert_eq!(counts.len(), 20);
    for digest in ["digest_00", "digest_01", "mixed_digest"] {
        assert_eq!(counts.get(digest), Some(&2));
    }
    for digest in ["pure_internal_digest", "digest_02", "digest_03"] {
        assert!(!counts.contains_key(digest));
    }
    assert_eq!(others, Some(2));
}

/// 构造带 Coprocessor/提交细节与 RU/CPU 的完整 StmtExecInfo 夹具。
pub(crate) fn exec_info(digest: &str, user: &str, start: u64) -> StmtExecInfo {
    let mut stmt_ctx = *NewStmtCtx();
    stmt_ctx.StmtType = "Select".into();
    stmt_ctx.SetLogicalPlanTables(vec![
        TableEntry {
            DB: "DB1".into(),
            Table: "TB1".into(),
        },
        TableEntry {
            DB: "DB2".into(),
            Table: "TB2".into(),
        },
        TableEntry {
            DB: "ignored".into(),
            Table: String::new(),
        },
    ]);
    *stmt_ctx.IndexNames.lock().unwrap() = vec!["idx_a".into()];
    stmt_ctx.SetAffectedRows(7);
    stmt_ctx.IsTiKV.store(true, Ordering::Relaxed);

    let scan = exec_util::ScanDetail {
        TotalKeys: 11,
        ProcessedKeys: 9,
        RocksdbBlockReadByte: 128,
        ..Default::default()
    };
    let commit = exec_util::CommitDetails {
        PrewriteTime: Duration::from_millis(3),
        CommitTime: Duration::from_millis(5),
        WriteKeys: 4,
        WriteSize: 64,
        TxnRetry: 2,
        ..Default::default()
    };
    {
        let mut mu = commit.Mu.Lock();
        mu.CommitBackoffTime = 13;
        mu.PrewriteBackoffTypes = vec!["txnlock".into()];
        mu.CommitBackoffTypes = vec!["rpc".into(), "txnlock".into()];
    }
    let details = execdetails::ExecDetails {
        CopExecDetails: execdetails::CopExecDetails {
            ScanDetail: Some(scan),
            TimeDetail: exec_util::TimeDetail {
                ProcessTime: Duration::from_millis(17),
                WaitTime: Duration::from_millis(19),
            },
            BackoffTime: Duration::from_millis(23),
            ..Default::default()
        },
        CommitDetail: Some(commit),
        ..Default::default()
    };

    // 填充 TiKV 侧执行细节桩，供 RocksDB/网络等列聚合。
    let tikv_details = tikv_util::ExecDetails::default();
    tikv_details.set_all_for_test([0, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71]);

    StmtExecInfo {
        SchemaName: "test".into(),
        Charset: "utf8mb4".into(),
        Collation: "utf8mb4_bin".into(),
        NormalizedSQL: "select * from t".into(),
        Digest: digest.into(),
        PlanDigest: "plan-digest".into(),
        User: user.into(),
        TotalLatency: Duration::from_millis(101),
        ParseLatency: Duration::from_millis(2),
        CompileLatency: Duration::from_millis(3),
        StmtCtx: stmt_ctx,
        CopTasks: Some(execdetails::CopTasksSummary {
            NumCopTasks: 2,
            MaxProcessAddress: "tikv-1".into(),
            MaxProcessTime: Duration::from_millis(7),
            TotProcessTime: Duration::from_millis(12),
            MaxWaitAddress: "tikv-2".into(),
            MaxWaitTime: Duration::from_millis(8),
            TotWaitTime: Duration::from_millis(14),
        }),
        ExecDetail: details,
        MemMax: 512,
        MemArbitration: 1.5,
        DiskMax: 256,
        StartTime: SystemTime::UNIX_EPOCH + Duration::from_secs(start),
        Succeed: true,
        PlanInCache: true,
        PlanInBinding: true,
        ExecRetryCount: 3,
        ExecRetryTime: Duration::from_millis(41),
        WriteSQLRespDuration: Duration::from_millis(43),
        ResultRows: 5,
        TiKVExecDetails: tikv_details,
        Prepared: true,
        ResourceGroupName: "rg".into(),
        RUDetail: Some(exec_util::RUDetails {
            read_ru: 2.5,
            write_ru: 3.5,
            ..Default::default()
        }),

        CPUUsages: ppcpuusage::CPUUsages {
            TidbCPUTime: Duration::from_millis(47),
            TikvCPUTime: Duration::from_millis(53),
        },
        LazyInfo: Box::new(MockLazyInfo {
            original_sql: "select * from t".into(),
            encoded_plan: "plan".into(),
            binary_plan: "binary".into(),
            plan_digest: "lazy-plan-digest".into(),
            binding_sql: "select /*+ use_index(t idx_a) */ * from t".into(),
            binding_digest: "binding-digest".into(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// 按列名列表构造 ColumnInfo 向量。
fn columns(names: &[&str]) -> Vec<model::ColumnInfo> {
    names
        .iter()
        .map(|name| {
            let mut column = model::ColumnInfo::default();
            column.Name.O = (*name).into();
            column
        })
        .collect()
}

/// 用泄漏的 Mutex 包装 map 作为 reader 的 ssMap 后端。
fn reader_for(map: stmtSummaryByDigestMap, names: &[&str]) -> stmtSummaryReader {
    // 测试中泄漏 Mutex 以获得 'static 后端，避免生命周期缠绕。
    let storage = Box::leak(Box::new(Mutex::new(map)));
    let mut reader = NewStmtSummaryReader(
        None,
        false,
        columns(names),
        "127.0.0.1:4000".into(),
        chrono_tz::UTC,
    );
    reader.ssMap = storage;
    reader
}

/// 校验默认启用状态，以及 setter 保留 Go 的“直接存值”语义。
#[test]
fn test_set_up() {
    let mut summaries = newStmtSummaryByDigestMap();
    assert!(summaries.Enabled());
    assert!(!summaries.EnabledInternal());
    summaries.SetRefreshInterval(0).unwrap();
    assert_eq!(summaries.refreshInterval(), 0);
    summaries.SetHistorySize(-1).unwrap();
    assert_eq!(summaries.historySize(), -1);
    assert!(summaries.SetMaxStmtCount(0).is_err());
    assert_eq!(summaries.maxStmtCount(), 0);
    summaries.SetMaxSQLLength(-1).unwrap();
    assert_eq!(summaries.maxSQLLength(), -1);
    summaries.SetRefreshInterval(1800).unwrap();
    summaries.SetHistorySize(24).unwrap();
    summaries.SetMaxStmtCount(3000).unwrap();
    summaries.SetMaxSQLLength(32768).unwrap();
    assert_eq!(summaries.refreshInterval(), 1800);
    assert_eq!(summaries.historySize(), 24);
}

/// Go 的底层 setter 不校验零刷新间隔；绕过系统变量层后写入时会除零。
#[test]
#[should_panic]
fn test_zero_refresh_interval_preserves_go_failure() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetRefreshInterval(0).unwrap();
    summaries.set_now_for_test(Some(1));
    summaries.AddStatement(&exec_info("digest", "alice", 1));
}

/// 负截断长度同样由上层拦截；直接使用会保持 Go 的越界失败。
#[test]
#[should_panic]
fn test_negative_max_sql_length_preserves_go_failure() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetMaxSQLLength(-1).unwrap();
    summaries.set_now_for_test(Some(1));
    summaries.AddStatement(&exec_info("digest", "alice", 1));
}

/// 校验单条/多条 Add 后按 digest 聚合的执行次数。
#[test]
fn test_add_statement() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetRefreshInterval(10).unwrap();
    summaries.set_now_for_test(Some(100));
    summaries.AddStatement(&exec_info("digest", "alice", 90));
    summaries.AddStatement(&exec_info("digest", "bob", 95));
    let values = summaries.Summaries();
    assert_eq!(values.len(), 1);
    let summary = &values[0];
    assert_eq!(summary.schemaName, "test");
    assert_eq!(summary.tableNames, "db1.tb1,db2.tb2");
    assert_eq!(summary.bindingDigest, "binding-digest");
    let stats = &summary.history.back().unwrap().stmtSummaryStats;
    assert_eq!(stats.execCount, 2);
    assert_eq!(stats.sumLatency, Duration::from_millis(202));
    assert_eq!(stats.sumAffectedRows, 14);
    assert_eq!(stats.sumTotalKeys, 22);
    assert_eq!(stats.commitCount, 2);
    assert_eq!(stats.backoffTypes.get("txnlock"), Some(&4));
    assert_eq!(stats.authUsers.len(), 2);
    assert_eq!(stats.StmtRUSummary.SumRRU, 5.0);
    assert_eq!(
        stats.StmtNetworkTrafficSummary.UnpackedBytesSentTiKVTotal,
        82
    );
    assert!(stats.storageKV);
    assert!(!stats.storageMPP);
}

/// 校验摘要记录转 Datum 行时关键列取值。
#[test]
fn test_to_datum() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.set_now_for_test(Some(100));
    summaries.AddStatement(&generate_any_exec_info());
    let reader = reader_for(
        summaries,
        &[SchemaNameStr, DigestStr, ExecCountStr, AvgLatencyStr],
    );
    let rows = reader.GetStmtSummaryCurrentRows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0].GetString(), "test");
    assert_eq!(rows[0][1].GetString(), "digest");
    assert_eq!(rows[0][2].GetInt64(), 1);
    assert_eq!(
        rows[0][3].GetInt64(),
        Duration::from_millis(101).as_nanos() as i64
    );
}

/// 并发 Add 同一 digest，最终 ExecCount 应累加正确。
#[test]
fn test_add_statement_parallel() {
    let summaries = Arc::new(Mutex::new(newStmtSummaryByDigestMap()));
    summaries.lock().unwrap().set_now_for_test(Some(100));
    let mut workers = Vec::new();
    for _ in 0..10 {
        let summaries = Arc::clone(&summaries);
        workers.push(thread::spawn(move || {
            for _ in 0..100 {
                summaries
                    .lock()
                    .unwrap()
                    .AddStatement(&generate_any_exec_info());
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    let guard = summaries.lock().unwrap();
    assert_eq!(guard.Len(), 1);
    assert_eq!(
        guard.Summaries()[0]
            .history
            .back()
            .unwrap()
            .stmtSummaryStats
            .execCount,
        1000
    );
}

/// 超过 max_stmt_count 时应触发 LRU 淘汰。
#[test]
fn test_max_stmt_count() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.set_now_for_test(Some(100));
    summaries.SetMaxStmtCount(1).unwrap();
    for digest in ["a", "b", "c"] {
        summaries.AddStatement(&exec_info(digest, "user", 100));
        assert_eq!(summaries.Len(), 1);
    }
    assert_eq!(summaries.CurrentWindowEvictedCount(), 2);
    assert_eq!(summaries.ToEvictedCountDatum()[0][2].GetInt64(), 2);
}

/// 校验 SampleSQL / NormalizedSQL 按 max_sql_length 截断。
#[test]
fn test_max_sql_length() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetMaxSQLLength(8).unwrap();
    summaries.set_now_for_test(Some(100));
    summaries.AddStatement(&generate_any_exec_info());
    let summary = summaries.Summaries().pop().unwrap();
    assert_eq!(summary.normalizedSQL, "select *(len:15)");
    assert_eq!(
        summary.history[0].stmtSummaryStats.sampleSQL,
        "select *(len:15)"
    );

    summaries.SetMaxSQLLength(5).unwrap();
    let mut unicode = exec_info("unicode", "user", 100);
    unicode.NormalizedSQL = "你好世界".into();
    summaries.AddStatement(&unicode);
    assert!(
        summaries
            .Summaries()
            .iter()
            .any(|item| item.normalizedSQL == "你(len:12)")
    );
}

/// 校验 formatSQL 在截断边界上的克隆语义。
#[test]
fn test_format_sql_clone() {
    let summaries = newStmtSummaryByDigestMap();
    summaries.SetMaxSQLLength(4096).unwrap();
    let mut source = String::from("select * from t");
    let formatted = formatSQL(&source);
    source.push_str(" where a = 1");
    assert_eq!(formatted, "select * from t");
    assert_ne!(formatted, source);
}

/// 并发调整 max_stmt_count 与写入不应 panic。
#[test]
fn test_set_max_stmt_count_parallel() {
    let summaries = Arc::new(Mutex::new(newStmtSummaryByDigestMap()));
    summaries.lock().unwrap().set_now_for_test(Some(100));
    let mut workers = Vec::new();
    for cap in 1..10 {
        let summaries = Arc::clone(&summaries);
        workers.push(thread::spawn(move || {
            let mut guard = summaries.lock().unwrap();
            guard.SetMaxStmtCount(cap).unwrap();
            guard.AddStatement(&exec_info(&format!("digest-{cap}"), "user", 100));
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    let guard = summaries.lock().unwrap();
    assert!(guard.Len() <= guard.maxStmtCount() as usize);
}

/// 校验启用后指标计数器随 Add 增加。
#[test]
fn test_stmt_summary_metrics() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.set_now_for_test(Some(100));
    summaries.AddStatement(&exec_info("a", "u", 100));
    summaries.AddStatement(&exec_info("b", "u", 100));
    assert_eq!(summaries.Len(), 2);
    assert_eq!(summaries.CurrentWindowEvictedCount(), 0);
    summaries.Clear();
    assert_eq!(
        (summaries.Len(), summaries.CurrentWindowEvictedCount()),
        (0, 0)
    );
}

/// 容量变更后指标与淘汰行为仍一致。
#[test]
fn test_stmt_summary_metrics_after_capacity_change() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.set_now_for_test(Some(100));
    summaries.SetMaxStmtCount(1).unwrap();
    summaries.AddStatement(&exec_info("a", "u", 100));
    summaries.AddStatement(&exec_info("b", "u", 100));
    assert_eq!(
        (summaries.Len(), summaries.CurrentWindowEvictedCount()),
        (1, 1)
    );
    summaries.SetMaxStmtCount(2).unwrap();
    summaries.AddStatement(&exec_info("c", "u", 100));
    assert_eq!(
        (summaries.Len(), summaries.CurrentWindowEvictedCount()),
        (2, 1)
    );
}

/// 禁用摘要后 Add 不再累积。
#[test]
fn test_disable_stmt_summary() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.set_now_for_test(Some(100));
    summaries.SetEnabled(false).unwrap();
    summaries.AddStatement(&generate_any_exec_info());
    assert_eq!(summaries.Len(), 0);
    summaries.SetEnabled(true).unwrap();
    summaries.AddStatement(&generate_any_exec_info());
    assert_eq!(summaries.Len(), 1);
}

/// 并发启停摘要开关与写入。
#[test]
fn test_enable_summary_parallel() {
    let summaries = Arc::new(Mutex::new(newStmtSummaryByDigestMap()));
    let mut workers = Vec::new();
    for index in 0..100 {
        let summaries = Arc::clone(&summaries);
        workers.push(thread::spawn(move || {
            let mut guard = summaries.lock().unwrap();
            guard.SetEnabled(index % 2 == 0).unwrap();
            let _ = guard.Enabled();
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    let mut guard = summaries.lock().unwrap();
    guard.SetEnabled(true).unwrap();
    assert!(guard.Enabled());
}

/// 校验 backoff 类型 map 格式化为按次数降序字符串。
#[test]
fn test_format_backoff_types() {
    assert_eq!(formatBackoffTypes(&HashMap::new()), None);
    let values = HashMap::from([
        ("tikvRPC".into(), 1),
        ("txnlock".into(), 2),
        ("pdRPC".into(), 2),
    ]);
    assert_eq!(
        formatBackoffTypes(&values).as_deref(),
        Some("pdRPC:2,txnlock:2,tikvRPC:1")
    );
}

/// 刷新当前窗口应归档旧区间并开启新 begin。
#[test]
fn test_refresh_current_summary() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetRefreshInterval(10).unwrap();
    summaries.SetHistorySize(3).unwrap();
    for now in [100, 111, 121] {
        summaries.set_now_for_test(Some(now));
        summaries.AddStatement(&generate_any_exec_info());
    }
    let history = &summaries.Summaries()[0].history;
    assert_eq!(history.len(), 3);
    assert_eq!(
        history
            .iter()
            .map(|item| item.beginTime)
            .collect::<Vec<_>>(),
        vec![100, 110, 120]
    );
    assert!(
        history
            .iter()
            .all(|item| item.stmtSummaryStats.execCount == 1)
    );
}

/// 校验 history_size 限制下历史窗口保留数量。
#[test]
fn test_summary_history() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetRefreshInterval(10).unwrap();
    summaries.SetHistorySize(2).unwrap();
    for now in [100, 111, 121] {
        summaries.set_now_for_test(Some(now));
        summaries.AddStatement(&generate_any_exec_info());
    }
    assert_eq!(summaries.Summaries()[0].history.len(), 2);
    let reader = reader_for(summaries, &[SummaryBeginTimeStr, ExecCountStr]);
    let rows = reader.GetStmtSummaryHistoryRows();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row[1].GetInt64() == 1));
}

/// 事务内上一条 SQL 样本 PrevSQL 的记录。
#[test]
fn test_prev_sql() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.set_now_for_test(Some(100));
    let mut first = generate_any_exec_info();
    first.PrevSQL = "select 1".into();
    first.PrevSQLDigest = "prev-digest".into();
    summaries.AddStatement(&first);
    let mut second = generate_any_exec_info();
    second.PrevSQL = "select 2".into();
    second.PrevSQLDigest = "prev-digest".into();
    summaries.AddStatement(&second);
    let summary = &summaries.Summaries()[0];
    assert_eq!(summary.history[0].stmtSummaryStats.prevSQL, "select 1");
    assert_eq!(summary.history[0].stmtSummaryStats.execCount, 2);
}

/// 校验摘要区间 End 时间填充。
#[test]
fn test_end_time() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetRefreshInterval(60).unwrap();
    summaries.set_now_for_test(Some(125));
    summaries.AddStatement(&generate_any_exec_info());
    let item = summaries.Summaries()[0].history.back().unwrap().clone();
    assert_eq!((item.beginTime, item.endTime), (120, 180));
}

/// Point Get 路径下计划相关字段仍可写入摘要。
#[test]
fn test_point_get() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.set_now_for_test(Some(100));
    let mut info = generate_any_exec_info();
    info.PlanDigest.clear();
    info.LazyInfo = Box::new(MockLazyInfo {
        original_sql: "select * from t where id = ?".into(),
        plan_digest: "point-get-plan-digest".into(),
        ..Default::default()
    });
    summaries.AddStatement(&info);
    assert_eq!(summaries.Summaries()[0].planDigest, "point-get-plan-digest");
}

/// 无 PROCESS 权限时仅能看到 AuthUsers 含己的摘要行。
#[test]
fn test_access_privilege() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.set_now_for_test(Some(100));
    summaries.AddStatement(&exec_info("digest", "alice", 100));
    let storage = Box::leak(Box::new(Mutex::new(summaries)));
    let make_user = |name: &str| auth::UserIdentity {
        username: name.into(),
        hostname: "%".into(),
        ..Default::default()
    };

    let mut alice = NewStmtSummaryReader(
        Some(make_user("alice")),
        false,
        columns(&[DigestStr]),
        String::new(),
        chrono_tz::UTC,
    );
    alice.ssMap = storage;
    assert_eq!(alice.GetStmtSummaryCurrentRows().len(), 1);

    let mut charlie = NewStmtSummaryReader(
        Some(make_user("charlie")),
        false,
        columns(&[DigestStr]),
        String::new(),
        chrono_tz::UTC,
    );
    charlie.ssMap = storage;
    assert!(charlie.GetStmtSummaryCurrentRows().is_empty());

    let mut process = NewStmtSummaryReader(
        Some(make_user("charlie")),
        true,
        columns(&[DigestStr]),
        String::new(),
        chrono_tz::UTC,
    );
    process.ssMap = storage;
    assert_eq!(process.GetStmtSummaryCurrentRows().len(), 1);
}

/// 开启按用户分组时不同用户同 digest 分桶。
#[test]
fn test_add_statement_group_by_user() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.set_now_for_test(Some(100));
    summaries.SetGroupByUser(true).unwrap();
    summaries.AddStatement(&exec_info("digest", "alice", 100));
    summaries.AddStatement(&exec_info("digest", "bob", 100));
    assert_eq!(summaries.Len(), 2);
    summaries.SetGroupByUser(false).unwrap();
    assert_eq!(
        summaries.Len(),
        0,
        "changing the key layout clears incompatible entries"
    );
}

/// 校验 digest key 编码边界与相等比较。
#[test]
fn test_stmt_digest_key_boundary() {
    let mut legacy = StmtDigestKey::default();
    legacy.Init("schema", "digest", "prev", "plan", "rg", "");
    assert_eq!(legacy.Hash(), b"digestschemaprevplanrg");

    let mut first = StmtDigestKey::default();
    first.Init("schema", "digest", "prev", "plan", "rg", "alice");
    let mut second = StmtDigestKey::default();
    second.Init("schema", "digest", "prev", "plan", "rga", "lice");
    assert_ne!(first.Hash(), second.Hash());
    assert_eq!(
        &first.Hash()["digestschemaprevplanrg".len()..][..4],
        &5_u32.to_be_bytes()
    );
}

#[test]
fn ia_statistics_accumulate_and_round_trip_through_chunk() {
    let mut map = newStmtSummaryByDigestMap();
    map.set_now_for_test(Some(100));
    for (count, bytes, millis) in [(3, 4096, 5), (5, 8192, 9)] {
        let mut info = generate_any_exec_info();
        info.ExecDetail.CopExecDetails.ScanDetail = if count == 0 {
            None
        } else {
            Some(exec_util::ScanDetail {
                IaRemoteReadSegmentCount: count,
                IaRemoteReadSegmentBytes: bytes,
                IaRemoteReadSegmentDuration: Duration::from_millis(millis),
                ..Default::default()
            })
        };
        map.AddStatement(&info);
    }
    let reader = reader_for(
        map,
        &[
            AvgIARemoteReadSegmentCountStr,
            MaxIARemoteReadSegmentCountStr,
            AvgIARemoteReadSegmentSizeStr,
            MaxIARemoteReadSegmentSizeStr,
            AvgIARemoteReadSegmentWaitTimeStr,
            MaxIARemoteReadSegmentWaitTimeStr,
        ],
    );
    let rows = reader.GetStmtSummaryCurrentRows();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row[0].GetFloat64(), 4.0);
    assert_eq!(row[1].GetUint64(), 5);
    assert_eq!(row[2].GetFloat64(), 6144.0);
    assert_eq!(row[3].GetUint64(), 8192);
    assert_eq!(row[4].GetInt64(), 7_000_000);
    assert_eq!(row[5].GetInt64(), 9_000_000);
    use chunk_dependency::{NewChunkWithCapacity, mysql as chunk_mysql, types as chunk_types};
    let fields = (0..6)
        .map(|index| {
            let mut field = chunk_types::NewFieldType(if index == 0 || index == 2 {
                chunk_mysql::TypeDouble
            } else {
                chunk_mysql::TypeLonglong
            });
            if index == 1 || index == 3 {
                field.AddFlag(chunk_mysql::UnsignedFlag);
            }
            *field
        })
        .collect::<Vec<_>>();
    let mut chunk = NewChunkWithCapacity(fields.clone(), 1);
    for (index, value) in row.iter().enumerate() {
        chunk.AppendDatum(index, value);
    }
    let decoded = chunk.GetRow(0).GetDatumRow(&fields);
    assert_eq!(decoded[0].GetFloat64(), row[0].GetFloat64());
    assert_eq!(decoded[1].GetUint64(), 5);
    assert_eq!(decoded[2].GetFloat64(), 6144.0);
    assert_eq!(decoded[3].GetUint64(), 8192);
    assert_eq!(decoded[4].GetInt64(), 7_000_000);
    assert_eq!(decoded[5].GetInt64(), 9_000_000);
    let mut nil_info = generate_any_exec_info();
    nil_info.ExecDetail.CopExecDetails.ScanDetail = None;
    reader.ssMap.lock().unwrap().AddStatement(&nil_info);
    let rows = reader.GetStmtSummaryCurrentRows();
    assert_eq!(rows[0][0].GetFloat64(), 8.0 / 3.0);
    assert_eq!(rows[0][2].GetFloat64(), 4096.0);
    assert_eq!(rows[0][4].GetInt64(), 14_000_000 / 3);
    assert_eq!(rows[0][1].GetUint64(), 5);
}

#[test]
fn ia_unsigned_averages_keep_float_range_and_zero_execution_semantics() {
    let reader = reader_for(newStmtSummaryByDigestMap(), &[]);
    let factories = columnValueFactoryMap();
    let mut stats = stmtSummaryStats::default();
    stats.execCount = 2;
    stats.sumIARemoteReadSegmentCount = 1_u64 << 63;
    stats.sumIARemoteReadSegmentSize = 1_u64 << 63;
    for name in [
        AvgIARemoteReadSegmentCountStr,
        AvgIARemoteReadSegmentSizeStr,
    ] {
        assert_eq!(
            factories[name](&reader, None, None, &stats)
                .into_datum()
                .GetFloat64(),
            (1_u64 << 63) as f64 / 2.0
        );
    }
    stats.execCount = 0;
    for name in [
        AvgIARemoteReadSegmentCountStr,
        AvgIARemoteReadSegmentSizeStr,
    ] {
        assert_eq!(
            factories[name](&reader, None, None, &stats)
                .into_datum()
                .GetFloat64(),
            0.0
        );
    }
}

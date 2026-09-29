// Copyright 2026 AsterSQL.
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

// 语句摘要聚合迁移期单元测试：digest key、窗口历史、选项与 RU/网络辅助。
//
// 对照 Go `statement_summary` 行为，覆盖按用户分组、容量淘汰、内部查询过滤
// 以及计划/SQL 长度截断规则。

use super::*;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};
use task_execdetails::execdetails::{self, util as exec_util};
use task_execdetails::util::util as tikv_util;
use task_stmtctx::{NewStmtCtx, TableEntry};

/// 测试用懒加载执行信息：原 SQL、计划编码与 binding。
#[derive(Default)]
struct TestLazyInfo {
    original_sql: String,
    encoded_plan: String,
    binary_plan: String,
    plan_digest: String,
    binding_sql: String,
    binding_digest: String,
    error: Option<String>,
}

impl StmtExecLazyInfo for TestLazyInfo {
    fn GetOriginalSQL(&self) -> String {
        self.original_sql.clone()
    }

    fn GetEncodedPlan(&self) -> (String, String, Option<String>) {
        (
            self.encoded_plan.clone(),
            "hint".to_owned(),
            self.error.clone(),
        )
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

/// 构造带完整执行细节的 StmtExecInfo 测试夹具。
fn exec_info(digest: &str, user: &str, start_offset: u64) -> StmtExecInfo {
    let mut stmt_ctx = *NewStmtCtx();
    stmt_ctx.StmtType = "Select".to_owned();
    stmt_ctx.SetLogicalPlanTables(vec![
        TableEntry {
            DB: "Test".to_owned(),
            Table: "T".to_owned(),
        },
        TableEntry {
            DB: "Ignored".to_owned(),
            Table: String::new(),
        },
    ]);
    *stmt_ctx.IndexNames.lock().unwrap() = vec!["idx_a".to_owned()];
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
        mu.PrewriteBackoffTypes = vec!["txnlock".to_owned()];
        mu.CommitBackoffTypes = vec!["rpc".to_owned(), "txnlock".to_owned()];
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

    let tikv_details = tikv_util::ExecDetails::default();
    tikv_details.set_all_for_test([0, 29, 31, 37, 41, 43, 47, 53, 59, 61, 67, 71]);

    StmtExecInfo {
        SchemaName: "DB".to_owned(),
        Charset: "utf8mb4".to_owned(),
        Collation: "utf8mb4_bin".to_owned(),
        NormalizedSQL: "select * from t".to_owned(),
        Digest: digest.to_owned(),
        PlanDigest: "plan-digest".to_owned(),
        User: user.to_owned(),
        TotalLatency: Duration::from_millis(101),
        ParseLatency: Duration::from_millis(2),
        CompileLatency: Duration::from_millis(3),
        StmtCtx: stmt_ctx,
        CopTasks: Some(execdetails::CopTasksSummary {
            NumCopTasks: 2,
            MaxProcessAddress: "tikv-1".to_owned(),
            MaxProcessTime: Duration::from_millis(7),
            TotProcessTime: Duration::from_millis(12),
            MaxWaitAddress: "tikv-2".to_owned(),
            MaxWaitTime: Duration::from_millis(8),
            TotWaitTime: Duration::from_millis(14),
        }),
        ExecDetail: details,
        MemMax: 512,
        MemArbitration: 1.5,
        DiskMax: 256,
        StartTime: SystemTime::UNIX_EPOCH + Duration::from_secs(start_offset),
        Succeed: true,
        PlanInCache: true,
        PlanInBinding: true,
        ExecRetryCount: 3,
        ExecRetryTime: Duration::from_millis(41),
        WriteSQLRespDuration: Duration::from_millis(43),
        ResultRows: 5,
        TiKVExecDetails: tikv_details,
        Prepared: true,
        ResourceGroupName: "rg".to_owned(),
        RUDetail: Some(exec_util::RUDetails {
            read_ru: 2.5,
            write_ru: 3.5,
            ..Default::default()
        }),

        CPUUsages: ppcpuusage::CPUUsages {
            TidbCPUTime: Duration::from_millis(47),
            TikvCPUTime: Duration::from_millis(53),
        },
        LazyInfo: Box::new(TestLazyInfo {
            original_sql: "select * from t".to_owned(),
            encoded_plan: "plan".to_owned(),
            binary_plan: "binary".to_owned(),
            plan_digest: "lazy-plan-digest".to_owned(),
            binding_sql: "select /*+ use_index(t idx_a) */ * from t".to_owned(),
            binding_digest: "binding-digest".to_owned(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// digest key 空用户布局与带用户长度边界分隔。
#[test]
fn digest_key_keeps_legacy_layout_and_separates_user_boundary() {
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

/// AddStatement 聚合指标与刷新窗口裁剪与 Go 一致。
#[test]
fn add_statement_matches_go_aggregation_and_window_history() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetRefreshInterval(10).unwrap();
    summaries.SetHistorySize(2).unwrap();
    summaries.set_now_for_test(Some(100));

    summaries.AddStatement(&exec_info("digest", "alice", 90));
    summaries.AddStatement(&exec_info("digest", "bob", 95));
    let current = summaries.Summaries();
    assert_eq!(current.len(), 1, "group-by-user is disabled by default");
    let summary = &current[0];
    assert_eq!(summary.tableNames, "test.t");
    assert_eq!(summary.history.len(), 1);
    let stats = &summary.history.back().unwrap().stmtSummaryStats;
    assert_eq!(stats.execCount, 2);
    assert_eq!(stats.sumLatency, Duration::from_millis(202));
    assert_eq!(stats.sumWarnings, 0);
    assert_eq!(stats.sumAffectedRows, 14);
    assert_eq!(stats.sumTotalKeys, 22);
    assert_eq!(stats.maxTotalKeys, 11);
    assert_eq!(stats.commitCount, 2);
    assert_eq!(stats.backoffTypes.get("txnlock"), Some(&4));
    assert_eq!(stats.authUsers.len(), 2);
    assert_eq!(stats.StmtRUSummary.SumRRU, 5.0);
    assert_eq!(stats.StmtRUSummary.MaxWRU, 3.5);
    assert_eq!(
        stats.StmtNetworkTrafficSummary.UnpackedBytesSentTiKVTotal,
        82
    );
    assert!(stats.storageKV);
    assert!(!stats.storageMPP);

    summaries.set_now_for_test(Some(111));
    summaries.AddStatement(&exec_info("digest", "alice", 111));
    summaries.set_now_for_test(Some(121));
    summaries.AddStatement(&exec_info("digest", "alice", 121));
    let current = summaries.Summaries();
    assert_eq!(
        current[0].history.len(),
        2,
        "oldest window is trimmed like Go list"
    );
    assert_eq!(current[0].history.front().unwrap().beginTime, 110);
    assert_eq!(current[0].history.back().unwrap().beginTime, 120);
}

/// 按用户分组、容量上限与内部查询开关语义。
#[test]
fn options_grouping_capacity_and_internal_filter_match_go() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.set_now_for_test(Some(100));
    summaries.SetGroupByUser(true).unwrap();
    summaries.AddStatement(&exec_info("same", "alice", 100));
    summaries.AddStatement(&exec_info("same", "bob", 100));
    assert_eq!(summaries.Len(), 2);

    summaries.SetMaxStmtCount(1).unwrap();
    assert_eq!(summaries.Len(), 1);
    assert_eq!(
        summaries.CurrentWindowEvictedCount(),
        0,
        "Go SetCapacity does not invoke on-evict"
    );

    let mut internal = exec_info("internal", "root", 100);
    internal.IsInternal = true;
    summaries.AddStatement(&internal);
    assert_eq!(
        summaries.Len(),
        1,
        "internal queries are disabled by default"
    );
    summaries.SetEnabledInternalQuery(true).unwrap();
    summaries.AddStatement(&internal);
    assert_eq!(summaries.Len(), 1, "capacity remains enforced");
    summaries.SetEnabledInternalQuery(false).unwrap();
    assert_eq!(
        summaries.Len(),
        0,
        "internal-only rows are cleared when disabling"
    );
}

/// backoff 格式化、均值、RU 与网络流量汇总辅助函数。
#[test]
fn formatting_ru_and_network_helpers_match_go() {
    let mut backoffs = HashMap::new();
    assert_eq!(formatBackoffTypes(&backoffs), None);
    backoffs.insert("rpc".to_owned(), 1);
    backoffs.insert("txnlock".to_owned(), 2);
    assert_eq!(
        formatBackoffTypes(&backoffs).as_deref(),
        Some("txnlock:2,rpc:1")
    );
    assert_eq!(avgInt(7, 2), 3);
    assert_eq!(avgFloat(7, 2), 3.5);
    assert_eq!(avgSumFloat(7.0, 2), 3.5);
    assert_eq!(convertEmptyToNil(""), None);
    assert_eq!(convertEmptyToNil("x").as_deref(), Some("x"));

    let mut ru = StmtRUSummary::default();
    let detail = exec_util::RUDetails {
        read_ru: 2.0,
        write_ru: 3.0,
        ..Default::default()
    };
    ru.Add(Some(&detail));
    ru.Add(None);
    assert_eq!((ru.SumRRU, ru.SumWRU), (2.0, 3.0));
    let mut merged = StmtRUSummary::default();
    merged.Merge(&ru);
    assert_eq!((merged.SumRRU, merged.SumWRU), (2.0, 3.0));

    let raw = tikv_util::ExecDetails::default();
    raw.set_all_for_test([0, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8]);
    let mut traffic = StmtNetworkTrafficSummary::default();
    traffic.Add(Some(&raw));
    traffic.Merge(Some(&StmtNetworkTrafficSummary {
        UnpackedBytesSentTiKVTotal: 10,
        ..Default::default()
    }));
    traffic.Merge(None);
    assert_eq!(traffic.UnpackedBytesSentTiKVTotal, 11);
    assert_eq!(traffic.UnpackedBytesReceivedTiFlashCrossZone, 8);
}

/// 计划编码失败保留摘要并标记计划丢弃；SQL 超长按 maxSQLLength 截断。
#[test]
fn plan_errors_and_sql_limits_follow_go_first_sample_rules() {
    let mut bad = exec_info("bad", "u", 1);
    bad.LazyInfo = Box::new(TestLazyInfo {
        error: Some("encode failed".to_owned()),
        ..Default::default()
    });
    let stats = newStmtSummaryStats(&bad, 32).unwrap();
    assert_eq!(stats.samplePlan, "[discard]");
    assert!(stats.planHint.is_empty());

    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetMaxSQLLength(8).unwrap();
    summaries.set_now_for_test(Some(10));
    summaries.AddStatement(&exec_info("long", "u", 10));
    let summary = summaries.Summaries().pop().unwrap();
    assert_eq!(summary.normalizedSQL, "select *(len:15)");
    assert_eq!(
        summary.history[0].stmtSummaryStats.sampleSQL,
        "select *(len:15)"
    );
}

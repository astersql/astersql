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

// AsterSQL 补充的 `StmtRecord` 聚合单元测试。
//
// 相对 Go 移植测试，用更完整的 Coprocessor / 提交细节 / TiKV 流量与 RU 夹具，
// 校验 `NewStmtRecord`/`Add`/`Merge`、计划与 SQL 截断，以及摘要窗口驱逐行为。

use super::*;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};
use task_execdetails::execdetails::{self, util as exec_util};
use task_execdetails::util::util as tikv_util;
use task_stmtctx::{NewStmtCtx, TableEntry};
use task_stmtsummary::{StmtExecInfo, StmtExecLazyInfo};

/// 可配置原始 SQL / 编码计划 / 二进制计划 / plan digest 的懒加载夹具。
#[derive(Default)]
struct LazyInfo {
    sql: String,
    encoded_plan: String,
    binary_plan: String,
    plan_digest: String,
}

impl StmtExecLazyInfo for LazyInfo {
    fn GetOriginalSQL(&self) -> String {
        self.sql.clone()
    }
    fn GetEncodedPlan(&self) -> (String, String, Option<String>) {
        (
            self.encoded_plan.clone(),
            "use_index(t, idx)".to_owned(),
            None,
        )
    }
    fn GetBinaryPlan(&self) -> String {
        self.binary_plan.clone()
    }
    fn GetPlanDigest(&self) -> String {
        self.plan_digest.clone()
    }
    fn GetBindingSQLAndDigest(&self) -> (String, String) {
        (
            "select /*+ use_index(t, idx) */ * from t".to_owned(),
            "binding-digest".to_owned(),
        )
    }
}

/// 构造带完整执行细节的 `StmtExecInfo`，供聚合与截断断言使用。
fn exec_info(digest: &str, user: &str, start: u64) -> StmtExecInfo {
    let mut ctx = *NewStmtCtx();
    ctx.StmtType = "Select".to_owned();
    ctx.SetLogicalPlanTables(vec![
        TableEntry {
            DB: "DB1".to_owned(),
            Table: "T1".to_owned(),
        },
        TableEntry {
            DB: "DB2".to_owned(),
            Table: "T2".to_owned(),
        },
    ]);
    *ctx.IndexNames.lock().unwrap() = vec!["idx".to_owned()];
    ctx.SetAffectedRows(7);
    ctx.IsTiKV.store(true, Ordering::Relaxed);

    // 填充两阶段提交（2PC）相关耗时与退避类型，供 CommitDetail 聚合路径使用。
    let commit = exec_util::CommitDetails {
        PrewriteTime: Duration::from_millis(3),
        CommitTime: Duration::from_millis(5),
        GetCommitTsTime: Duration::from_millis(2),
        LocalLatchTime: Duration::from_millis(1),
        WriteKeys: 4,
        WriteSize: 64,
        TxnRetry: 2,
        ..Default::default()
    };
    commit
        .ResolveLock
        .ResolveLockTime
        .store(11, Ordering::Relaxed);
    commit.PrewriteRegionNum.store(6, Ordering::Relaxed);
    {
        let mut mu = commit.Mu.Lock();
        mu.CommitBackoffTime = 13;
        mu.PrewriteBackoffTypes = vec!["txnlock".to_owned()];
        mu.CommitBackoffTypes = vec!["rpc".to_owned(), "txnlock".to_owned()];
    }

    // TiKV 侧等待/流量计数器：set_all_for_test 写入固定原子值便于断言。
    let tikv = tikv_util::ExecDetails::default();
    tikv.set_all_for_test([0, 17, 19, 23, 29, 31, 37, 41, 43, 47, 53, 59]);

    StmtExecInfo {
        SchemaName: "schema".to_owned(),
        Charset: "utf8mb4".to_owned(),
        Collation: "utf8mb4_bin".to_owned(),
        NormalizedSQL: "select * from t".to_owned(),
        Digest: digest.to_owned(),
        PlanDigest: String::new(),
        User: user.to_owned(),
        TotalLatency: Duration::from_millis(101),
        ParseLatency: Duration::from_millis(2),
        CompileLatency: Duration::from_millis(3),
        StmtCtx: ctx,
        CopTasks: Some(execdetails::CopTasksSummary {
            NumCopTasks: 2,
            MaxProcessAddress: "tikv-1".to_owned(),
            MaxProcessTime: Duration::from_millis(7),
            MaxWaitAddress: "tikv-2".to_owned(),
            MaxWaitTime: Duration::from_millis(8),
            ..Default::default()
        }),
        ExecDetail: execdetails::ExecDetails {
            CopExecDetails: execdetails::CopExecDetails {
                ScanDetail: Some(exec_util::ScanDetail {
                    TotalKeys: 11,
                    ProcessedKeys: 9,
                    RocksdbDeleteSkippedCount: 3,
                    RocksdbKeySkippedCount: 4,
                    RocksdbBlockCacheHitCount: 5,
                    RocksdbBlockReadCount: 6,
                    RocksdbBlockReadByte: 128,
                    ..Default::default()
                }),
                TimeDetail: exec_util::TimeDetail {
                    ProcessTime: Duration::from_millis(17),
                    WaitTime: Duration::from_millis(19),
                },
                BackoffTime: Duration::from_millis(23),
                ..Default::default()
            },
            CommitDetail: Some(commit),
            ..Default::default()
        },
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
        TiKVExecDetails: tikv,
        Prepared: true,
        KeyspaceName: "keyspace".to_owned(),
        KeyspaceID: 42,
        ResourceGroupName: "rg".to_owned(),
        RUDetail: Some(exec_util::RUDetails {
            read_ru: 1.25,
            write_ru: 2.5,
            ru_wait_duration: Duration::from_millis(29),
            ..Default::default()
        }),

        PlanCacheUnqualified: "too many values".to_owned(),
        LazyInfo: Box::new(LazyInfo {
            sql: "select * from t".to_owned(),
            encoded_plan: "plan".to_owned(),
            binary_plan: "binary".to_owned(),
            plan_digest: "lazy-plan-digest".to_owned(),
        }),
        ..Default::default()
    }
}

/// 校验 New/Add/Merge 后表名小写化、延迟/提交/RU/流量等聚合与 Go 一致。
#[test]
fn record_new_add_and_merge_match_go_aggregation() {
    let info = exec_info("digest", "alice", 10);
    let mut record = NewStmtRecord(&info);
    assert_eq!(record.TableNames, "db1.t1,db2.t2");
    assert_eq!(record.PlanDigest, "lazy-plan-digest");
    assert_eq!(record.MinResultRows, i64::MAX);

    record.Add(&info);
    assert_eq!(record.ExecCount, 1);
    assert_eq!(record.SumLatency, info.TotalLatency);
    assert_eq!(record.SumWarnings, 0);
    assert_eq!(record.SumTotalKeys, 11);
    assert_eq!(record.SumCommitBackoffTime, 13);
    assert_eq!(record.SumBackoffTimes, 3);
    assert_eq!(record.BackoffTypes["txnlock"], 2);
    assert_eq!(record.SumRRU, 1.25);
    assert_eq!(record.MaxWRU, 2.5);
    assert_eq!(record.UnpackedBytesSentTiKVTotal, 29);
    assert_eq!(record.UnpackedBytesReceivedTiFlashCrossZone, 59);
    assert!(record.StorageKV);
    assert!(!record.StorageMPP);

    // 合并另一条同等记录后，求和指标翻倍。
    let mut other = NewStmtRecord(&info);
    other.Add(&info);
    record.Merge(&other);
    assert_eq!(record.ExecCount, 2);
    assert_eq!(record.SumLatency, info.TotalLatency * 2);
    assert_eq!(record.SumRRU, 2.5);
    assert_eq!(record.SumErrors, 0);
}

/// 超限 SQL/文本计划/二进制计划应被截断或替换为 discard 占位。
#[test]
fn record_truncates_plans_and_sql_at_configured_limits() {
    let _guard = crate::testkit::SQL_LENGTH_TEST_LOCK.lock().unwrap();
    SetGlobalMaxSQLLengthForTest(4);
    SetMaxEncodedPlanSizeInBytesForTest(3);
    let mut info = exec_info("digest", "alice", 10);
    info.LazyInfo = Box::new(LazyInfo {
        sql: "abcdefgh".to_owned(),
        encoded_plan: "1234".to_owned(),
        binary_plan: "5678".to_owned(),
        plan_digest: "fallback".to_owned(),
    });
    let record = NewStmtRecord(&info);
    assert_eq!(record.SampleSQL, "abcd(len:8)");
    assert_eq!(record.SamplePlan, "[discard]");
    assert_ne!(record.SampleBinaryPlan, "5678");
    // 恢复全局上限，避免污染后续测试。
    SetGlobalMaxSQLLengthForTest(defaultMaxSQLLength);
    SetMaxEncodedPlanSizeInBytesForTest(1024 * 1024);
}

/// 校验按用户分组、驱逐持久化开关与 Close 后状态。
#[test]
fn summary_options_grouping_eviction_and_close_match_go() {
    let summary = NewStmtSummary4Test(2);
    summary.SetGroupByUser(true).unwrap();
    summary.SetPersistEvicted(true).unwrap();
    // 同 digest 不同用户应各占一条；再插入第三条触发 LRU 驱逐。
    summary.Add(&exec_info("same", "alice", 1));
    summary.Add(&exec_info("same", "bob", 2));
    assert_eq!(summary.Len(), 2);

    summary.Add(&exec_info("third", "carol", 3));
    assert_eq!(summary.Len(), 2);
    assert_eq!(summary.EvictedCount(), 1);
    assert_eq!(summary.Evicted().unwrap()[2].GetInt64(), 1);

    summary.SetEnabled(false).unwrap();
    assert_eq!(summary.Len(), 0);
    summary.Close();
    assert!(summary.IsClosed());
    assert_eq!(summary.PersistedEvictedCount(), 1);
}

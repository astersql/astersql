// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 执行明细与运行时统计的 Aster 补充单元测试。
//
// 覆盖 String/zap 字段顺序与零值过滤、SyncExecDetails 百分位、RuntimeStatsColl 合并，
// 以及提交/加锁/RU（Resource Unit，资源计量）运行时统计的格式化行为。

use super::*;
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[test]
/// String 与 ToZapFields 字段顺序及零值过滤应与 Go 一致。
fn exec_details_string_and_zap_fields_follow_go_order_and_zero_filtering() {
    let details = ExecDetails {
        CopExecDetails: CopExecDetails {
            ScanDetail: Some(util::ScanDetail {
                ProcessedKeys: 3,
                TotalKeys: 5,
                GetSnapshotDuration: Duration::from_millis(250),
                RocksdbBlockReadCount: 2,
                RocksdbBlockReadDuration: Duration::from_millis(125),
                ..Default::default()
            }),
            TimeDetail: util::TimeDetail {
                ProcessTime: Duration::from_secs(2),
                WaitTime: Duration::from_millis(500),
            },
            BackoffTime: Duration::from_millis(250),
            ..Default::default()
        },
        LockKeysDetail: Some(util::LockKeysDetails {
            TotalTime: Duration::from_millis(125),
            ..Default::default()
        }),
        CopTime: Duration::from_secs(3),
        RequestCount: 4,
        ..Default::default()
    };

    assert_eq!(
        details.String(),
        "Cop_time: 3 Process_time: 2 Wait_time: 0.5 Backoff_time: 0.25 LockKeys_time: 0.125 Request_count: 4 Process_keys: 3 Total_keys: 5 Get_snapshot_time: 0.250 Rocksdb_block_read_count: 2 Rocksdb_block_read_time: 0.125"
    );
    let fields = details.ToZapFields();
    let keys: Vec<_> = fields.iter().map(|field| field.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "cop_time",
            "process_time",
            "wait_time",
            "backoff_time",
            "request_count",
            "total_keys",
            "process_keys",
        ]
    );
}

#[test]
/// 合并多次 cop 明细后，平均/P90/最大地址与退避统计应符合 Go 百分位语义。
fn sync_exec_details_merges_cop_tasks_and_calculates_go_percentiles() {
    let sync = SyncExecDetails::default();
    let mut first_backoff_sleep = HashMap::new();
    first_backoff_sleep.insert("rpc".to_owned(), Duration::from_millis(10));
    let mut first_backoff_times = HashMap::new();
    first_backoff_times.insert("rpc".to_owned(), 2);
    sync.MergeCopExecDetails(
        Some(&CopExecDetails {
            ScanDetail: Some(util::ScanDetail {
                ProcessedKeys: 2,
                TotalKeys: 3,
                ..Default::default()
            }),
            TimeDetail: util::TimeDetail {
                ProcessTime: Duration::from_millis(10),
                WaitTime: Duration::from_millis(2),
            },
            CalleeAddress: "store-1".to_owned(),
            BackoffSleep: first_backoff_sleep,
            BackoffTimes: first_backoff_times,
            ..Default::default()
        }),
        Duration::from_millis(12),
    );
    sync.MergeCopExecDetails(
        Some(&CopExecDetails {
            TimeDetail: util::TimeDetail {
                ProcessTime: Duration::from_millis(30),
                WaitTime: Duration::from_millis(4),
            },
            CalleeAddress: "store-2".to_owned(),
            ..Default::default()
        }),
        Duration::from_millis(34),
    );

    let merged = sync.GetExecDetails();
    assert_eq!(merged.RequestCount, 2);
    assert_eq!(merged.CopTime, Duration::from_millis(46));
    assert_eq!(merged.CopExecDetails.ScanDetail.unwrap().ProcessedKeys, 2);

    let summary = sync.CopTasksDetails().expect("two cop tasks");
    assert_eq!(summary.NumCopTasks, 2);
    assert_eq!(summary.ProcessTimeStats.AvgTime, Duration::from_millis(20));
    assert_eq!(summary.ProcessTimeStats.P90Time, Duration::from_millis(30));
    assert_eq!(summary.ProcessTimeStats.MaxAddress, "store-2");
    assert_eq!(
        summary.BackoffTimeStatsMap["rpc"].AvgTime,
        Duration::from_millis(10)
    );
    assert_eq!(summary.TotBackoffTimes["rpc"], 2);
}

#[test]
/// RecordCopStats 首次写入不应重复累计扫描/时间明细。
fn runtime_stats_coll_does_not_double_count_the_first_cop_detail() {
    let mut coll = NewRuntimeStatsColl(None);
    let scan = util::ScanDetail {
        ProcessedKeys: 7,
        TotalKeys: 9,
        ..Default::default()
    };
    let time = util::TimeDetail {
        ProcessTime: Duration::from_millis(11),
        WaitTime: Duration::from_millis(3),
    };
    assert_eq!(
        coll.RecordCopStats(8, kv::TiKV, Some(&scan), time, None, None),
        8
    );
    let stats = coll.GetCopStats(8).expect("cop stats created");
    assert_eq!(stats.scanDetail, scan);
    assert_eq!(stats.timeDetail, time);
}

#[derive(Clone)]
/// 可合并的自定义 RuntimeStats，用于验证同类型 Merge。
struct MergeableStats(i32);

impl RuntimeStats for MergeableStats {
    fn String(&self) -> String {
        format!("custom:{}", self.0)
    }
    fn Merge(&mut self, other: &dyn RuntimeStats) {
        if let Some(other) = other.as_any().downcast_ref::<Self>() {
            self.0 += other.0;
        }
    }
    fn CloneBox(&self) -> Box<dyn RuntimeStats> {
        Box::new(self.clone())
    }
    fn Tp(&self) -> i32 {
        88
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

#[test]
/// 同类型 RegisterStats 应 Merge；BasicRuntimeStats 格式与 Go 一致。
fn runtime_stats_coll_merges_equal_types_and_basic_stats_match_go_format() {
    let mut coll = NewRuntimeStatsColl(None);
    coll.RegisterStats(1, Box::new(MergeableStats(2)));
    coll.RegisterStats(1, Box::new(MergeableStats(3)));
    assert_eq!(coll.GetRootStats(1).String(), "custom:5");

    let basic = coll.GetBasicRuntimeStats(2, true).expect("basic stats");
    basic.RecordOpen(Duration::from_millis(1));
    basic.Record(Duration::from_millis(2), 6);
    basic.RecordClose(Duration::from_millis(3));
    assert_eq!(basic.GetActRows(), 6);
    assert_eq!(basic.GetTime(), 6_000_000);
    assert_eq!(basic.String(), "time:6ms, open:1ms, close:3ms, loops:1");
}

#[test]
/// 从 ExecutorExecutionSummary 解析 plan id，并 RecordOneCopTask。
fn execution_summary_redirects_plan_id_and_records_one_task() {
    let mut coll = NewRuntimeStatsColl(None);
    let summary = tipb::ExecutorExecutionSummary {
        NumIterations: Some(4),
        NumProducedRows: Some(12),
        TimeProcessedNs: Some(5_000_000),
        ExecutorId: "table_scan_42".to_owned(),
        ..Default::default()
    };
    assert_eq!(getPlanIDFromExecutionSummary(&summary), (42, true));
    assert_eq!(coll.RecordOneCopTask(7, kv::TiFlash, &summary), 42);
    assert_eq!(coll.GetCopCountAndRows(42), (1, 12));
}

#[test]
/// 提交运行时统计应对退避类型去重，并格式化原子字段。
fn commit_runtime_stats_deduplicate_backoffs_and_format_atomic_details() {
    let commit = util::CommitDetails {
        PrewriteTime: Duration::from_millis(2),
        CommitTime: Duration::from_millis(3),
        WriteKeys: 5,
        WriteSize: 20,
        ..Default::default()
    };
    commit.PrewriteRegionNum.store(2, Ordering::Relaxed);
    commit
        .ResolveLock
        .ResolveLockTime
        .store(1_000_000, Ordering::Relaxed);
    {
        let mut mu = commit.Mu.Lock();
        mu.CommitBackoffTime = 4_000_000;
        mu.PrewriteBackoffTypes = vec![
            "txnLock".to_owned(),
            "regionMiss".to_owned(),
            "txnLock".to_owned(),
        ];
    }
    let stats = RuntimeStatsWithCommit {
        Commit: Some(commit),
        TxnCnt: 2,
        ..Default::default()
    };
    assert_eq!(
        stats.String(),
        "commit_txn: {count: 2, prewrite:2ms, commit:3ms, backoff: {time: 4ms, prewrite type: [regionMiss txnLock]}, resolve_lock: 1ms, region_num:2, write_keys:5, write_byte:20}"
    );
}

#[test]
/// 加锁 RPC 耗时应按 Go 的纳秒单位换算展示。
fn lock_rpc_duration_uses_go_duration_units() {
    let stats = RuntimeStatsWithCommit {
        LockKeys: Some(util::LockKeysDetails {
            LockRPCTime: 1_500_000,
            LockRPCCount: 2,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(stats.String(), "lock_keys: {, lock_rpc:1.5ms, rpc_count:2}");
}

#[test]
fn shared_runtime_stats_collector_accepts_finish_commit_stats_by_plan_id() {
    let coll = std::sync::Arc::new(NewRuntimeStatsColl(None));
    let shared = coll.clone();
    shared.RegisterStatsShared(
        42,
        Box::new(RuntimeStatsWithCommit {
            Commit: Some(util::CommitDetails {
                WriteSize: 58,
                WriteKeys: 2,
                ..Default::default()
            }),
            ..Default::default()
        }),
    );
    assert!(
        coll.GetRootStatsStringShared(42)
            .contains("write_keys:2, write_byte:58")
    );
    drop(shared);
    let mut owned = std::sync::Arc::try_unwrap(coll).ok().unwrap();
    assert!(
        owned
            .GetRootStats(42)
            .String()
            .contains("write_keys:2, write_byte:58")
    );
}

#[test]
fn shared_and_owned_root_stats_merge_same_runtime_type_before_reading() {
    let mut coll = NewRuntimeStatsColl(None);
    coll.RegisterStats(42, Box::new(MergeableStats(2)));
    let coll = std::sync::Arc::new(coll);
    coll.RegisterStatsShared(42, Box::new(MergeableStats(3)));
    assert_eq!(coll.GetRootStatsStringShared(42), "custom:5");
    let mut owned = std::sync::Arc::try_unwrap(coll).ok().unwrap();
    assert_eq!(owned.GetRootStats(42).String(), "custom:5");
}

#[test]
fn fair_locking_details_clone_and_merge_preserve_all_go_counters() {
    let mut details = util::LockKeysDetails {
        AggressiveLockNewCount: 1,
        AggressiveLockDerivedCount: 2,
        LockedWithConflictCount: 3,
        ..Default::default()
    };
    let cloned = details.clone();
    details.Merge(&cloned);
    assert_eq!(details.AggressiveLockNewCount, 2);
    assert_eq!(details.AggressiveLockDerivedCount, 4);
    assert_eq!(details.LockedWithConflictCount, 6);
}

#[test]
fn ru_runtime_stats_v1_clone_and_merge_follow_go_behavior() {
    let mut stats = RURuntimeStats {
        RUDetails: Some(util::RUDetails {
            read_ru: 1.5,
            write_ru: 2.5,
            tikv_ru_v2: 3.0,
            tiflash_ru: 1.0,
            ..Default::default()
        }),
    };
    assert_eq!(stats.String(), "RU:4.00");
    let cloned = stats.Clone();
    assert_eq!(cloned.String(), "RU:4.00");
    stats.MergeRURuntimeStats(&RURuntimeStats {
        RUDetails: Some(util::RUDetails {
            read_ru: 1.0,
            ..Default::default()
        }),
    });
    assert_eq!(stats.String(), "RU:5.00");
}

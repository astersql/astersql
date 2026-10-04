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

// 语句摘要淘汰逻辑单测：对应 Go `evicted_test.go`。
//
// 覆盖 map 淘汰计数 Datum、窗口插入/合并、元素边界与 addInfo 指标合并。

use super::statement_summary_test::exec_info;
use super::*;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

/// 构造带默认 minLatency 边界的摘要元素。
fn new_induce_ssbde(begin: i64, end: i64) -> stmtSummaryByDigestElement {
    stmtSummaryByDigestElement {
        beginTime: begin,
        endTime: end,
        stmtSummaryStats: stmtSummaryStats {
            minLatency: Duration::from_nanos(i64::MAX as u64),
            ..Default::default()
        },
    }
}

/// 构造含单个历史窗口的 digest。
fn new_induce_ssbd(begin: i64, end: i64) -> stmtSummaryByDigest {
    let mut summary = stmtSummaryByDigest::default();
    summary.history.push_back(new_induce_ssbde(begin, end));
    summary
}

/// 按 schema 初始化 digest key 并附带时间窗口摘要。
fn key_value(schema: &str, begin: i64, end: i64) -> (StmtDigestKey, stmtSummaryByDigest) {
    let mut key = StmtDigestKey::default();
    key.Init(schema, "", "", "", "", "");
    (key, new_induce_ssbd(begin, end))
}

/// 渲染淘汰历史为 (begin, end, count)，最新在前。
fn rendered(history: &stmtSummaryByDigestEvicted) -> Vec<(i64, i64, i64)> {
    history
        .history
        .iter()
        .rev()
        .map(|item| (item.beginTime, item.endTime, item.count))
        .collect()
}

/// LRU 容量淘汰后 ToEvictedCountDatum 行数与 count 正确。
#[test]
fn test_map_to_evicted_count_datum() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetRefreshInterval(60).unwrap();
    summaries.SetMaxStmtCount(1).unwrap();
    summaries.SetHistorySize(100).unwrap();
    summaries.set_now_for_test(Some(120));

    let mut first = exec_info("digest", "user", 120);
    first.SchemaName = "I'll occupy this cache! :(".into();
    summaries.AddStatement(&first);
    let mut second = exec_info("digest", "user", 120);
    second.SchemaName = "sorry, it's mine now. =)".into();
    summaries.AddStatement(&second);
    let rows = summaries.ToEvictedCountDatum();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].len(), 3);
    assert_eq!(rows[0][2].GetInt64(), 1);

    summaries.Clear();
    for index in 0..50 {
        summaries.set_now_for_test(Some(120 + index * 120));
        summaries.AddStatement(&exec_info("digest", "user", 120));
    }
    assert_eq!(summaries.Len(), 1);
    assert_eq!(summaries.Summaries()[0].history.len(), 50);

    summaries.SetHistorySize(25).unwrap();
    summaries.set_now_for_test(Some(6_240));
    let mut bandit = exec_info("digest", "user", 120);
    bandit.SchemaName = "Kick you out >:(".into();
    summaries.AddStatement(&bandit);
    assert_eq!(summaries.ToEvictedCountDatum().len(), 25);

    bandit.SchemaName = "Yet another kicker".into();
    summaries.AddStatement(&bandit);
    let rows = summaries.ToEvictedCountDatum();
    assert_eq!(rows.len(), 25);
    assert_eq!(rows[0][2].GetInt64(), 1);
}

/// 基础 AddEvicted：nil 参数、合并同窗、跨窗插入与裁剪。
#[test]
fn test_simple_stmt_summary_by_digest_evicted() {
    let (mut key, mut value) = key_value("a", 1, 2);
    let mut evicted = newStmtSummaryByDigestEvicted();
    evicted.AddEvicted(None, None, 10);
    assert!(evicted.history.is_empty());
    evicted.AddEvicted(None, Some(&value), 10);
    assert_eq!(rendered(&evicted), vec![(1, 2, 0)]);
    evicted.Clear();
    evicted.AddEvicted(Some(&key), None, 10);
    assert!(evicted.history.is_empty());
    evicted.AddEvicted(Some(&key), Some(&value), 0);
    assert!(evicted.history.is_empty());

    evicted.AddEvicted(Some(&key), Some(&value), 1);
    evicted.AddEvicted(Some(&key), Some(&value), 1);
    (key, value) = key_value("b", 1, 2);
    evicted.AddEvicted(Some(&key), Some(&value), 1);
    assert_eq!(rendered(&evicted), vec![(1, 2, 3)]);

    (key, value) = key_value("b", 5, 6);
    evicted.AddEvicted(Some(&key), Some(&value), 2);
    (key, value) = key_value("b", 3, 4);
    evicted.AddEvicted(Some(&key), Some(&value), 3);
    assert_eq!(rendered(&evicted), vec![(5, 6, 1), (3, 4, 1), (1, 2, 3)]);

    evicted.Clear();
    (key, value) = key_value("a", 1, 2);
    value.history.push_back(new_induce_ssbde(2, 3));
    value.history.push_back(new_induce_ssbde(5, 6));
    value.history.push_back(new_induce_ssbde(8, 9));
    evicted.AddEvicted(Some(&key), Some(&value), 3);
    assert_eq!(rendered(&evicted), vec![(8, 9, 1), (5, 6, 1), (2, 3, 1)]);

    key.Init("b", "", "", "", "", "");
    evicted.AddEvicted(Some(&key), Some(&value), 4);
    assert_eq!(
        rendered(&evicted),
        vec![(8, 9, 2), (5, 6, 2), (2, 3, 2), (1, 2, 1)]
    );

    (key, value) = key_value("c", 4, 5);
    value.history.push_back(new_induce_ssbde(5, 6));
    value.history.push_back(new_induce_ssbde(7, 8));
    evicted.AddEvicted(Some(&key), Some(&value), 4);
    assert_eq!(
        rendered(&evicted),
        vec![(8, 9, 2), (7, 8, 1), (5, 6, 3), (4, 5, 1)]
    );

    (key, value) = key_value("d", 7, 8);
    evicted.AddEvicted(Some(&key), Some(&value), 4);
    assert_eq!(
        rendered(&evicted),
        vec![(8, 9, 2), (7, 8, 2), (5, 6, 3), (4, 5, 1)]
    );

    (key, value) = key_value("d", 0, 1);
    value.history.push_back(new_induce_ssbde(1, 2));
    value.history.push_back(new_induce_ssbde(2, 3));
    value.history.push_back(new_induce_ssbde(4, 5));
    evicted.AddEvicted(Some(&key), Some(&value), 4);
    assert_eq!(
        rendered(&evicted),
        vec![(8, 9, 2), (7, 8, 2), (5, 6, 3), (4, 5, 2)]
    );

    (key, value) = key_value("d", 1, 2);
    value.history.push_back(new_induce_ssbde(9, 10));
    evicted.AddEvicted(Some(&key), Some(&value), 4);
    assert_eq!(
        rendered(&evicted),
        vec![(9, 10, 1), (8, 9, 2), (7, 8, 2), (5, 6, 3)]
    );
}

/// 淘汰元素 matchAndAdd / addEvicted 边界行为。
#[test]
fn test_stmt_summary_by_digest_evicted_element() {
    let (key, value) = key_value("alpha", 0, 1);
    let digest = value.history.back().unwrap();
    let mut record = newStmtSummaryByDigestEvictedElement(0, 1);
    record.addEvicted(None, None);
    assert_eq!(record.count, 0);
    record.addEvicted(None, Some(digest));
    assert_eq!(record.count, 0);
    record.addEvicted(Some(&key), Some(digest));
    record.addEvicted(Some(&key), Some(digest));
    assert_eq!(record.count, 2);

    let (other_key, other_value) = key_value("bravo", 0, 1);
    record.addEvicted(Some(&other_key), Some(other_value.history.back().unwrap()));
    assert_eq!(record.count, 3);
    assert_eq!(
        record.matchAndAdd(Some(&key), Some(&new_induce_ssbde(-2, -1))),
        isTooOld
    );
    assert_eq!(
        record.matchAndAdd(Some(&key), Some(&new_induce_ssbde(1, 2))),
        isTooYoung
    );
}

/// 详细淘汰计数：多 key 同窗聚合与 Datum 列。
#[test]
fn test_evicted_count_detailed() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetRefreshInterval(60).unwrap();
    summaries.SetHistorySize(100).unwrap();
    summaries.SetMaxStmtCount(1).unwrap();
    for index in 0..100 {
        summaries.set_now_for_test(Some(60 + index * 60));
        summaries.AddStatement(&exec_info("digest", "user", 60));
        assert_eq!(summaries.Summaries()[0].history.len(), index as usize + 1);
    }

    let mut bandit = exec_info("digest", "user", 60);
    bandit.SchemaName = "kick you out >:(".into();
    summaries.AddStatement(&bandit);
    let rows = summaries.ToEvictedCountDatum();
    assert_eq!(rows.len(), 100);
    assert!(rows.iter().all(|row| row[2].GetInt64() == 1));

    bandit.SchemaName = "Yet another kicker".into();
    summaries.AddStatement(&bandit);
    assert_eq!(summaries.ToEvictedCountDatum()[0][2].GetInt64(), 2);

    summaries.Clear();
    summaries.other.AddEvicted(
        Some(&StmtDigestKey::default()),
        Some(&stmtSummaryByDigest::default()),
        100,
    );
    assert!(summaries.other.history.is_empty());
}

/// 构造函数初始化 begin/end 与 otherSummary 边界。
#[test]
fn test_new_stmt_summary_by_digest_evicted_element() {
    let element = newStmtSummaryByDigestEvictedElement(100, 160);
    assert_eq!(
        (element.beginTime, element.endTime, element.count),
        (100, 160, 0)
    );
    assert_eq!(element.otherSummary.beginTime, 100);
    assert_eq!(element.otherSummary.endTime, 160);
    assert_eq!(
        element.otherSummary.stmtSummaryStats.minLatency,
        Duration::from_nanos(i64::MAX as u64)
    );
    assert_eq!(
        element.otherSummary.stmtSummaryStats.firstSeen,
        SystemTime::UNIX_EPOCH + Duration::from_secs(160)
    );
}

/// 多窗口淘汰历史收集与 Clear。
#[test]
fn test_stmt_summary_by_digest_evicted() {
    let mut evicted = newStmtSummaryByDigestEvicted();
    assert!(evicted.history.is_empty());
    let (key, value) = key_value("a", 1, 2);
    evicted.AddEvicted(Some(&key), Some(&value), 1);
    assert_eq!(evicted.history.len(), 1);
    evicted.Clear();
    assert!(evicted.history.is_empty());
}

/// 构造带齐全指标字段的合并测试夹具。
fn merge_fixture(first_seen: u64, last_seen: u64) -> stmtSummaryByDigestElement {
    stmtSummaryByDigestElement {
        stmtSummaryStats: stmtSummaryStats {
            authUsers: HashSet::from(["a".into()]),
            execCount: 3,
            sumWarnings: 8,
            sumLatency: Duration::from_nanos(8),
            maxLatency: Duration::from_nanos(5),
            minLatency: Duration::from_nanos(1),
            sumParseLatency: Duration::from_nanos(3),
            maxParseLatency: Duration::from_nanos(2),
            sumCompileLatency: Duration::from_nanos(3),
            maxCompileLatency: Duration::from_nanos(2),
            sumNumCopTasks: 4,
            maxCopProcessTime: Duration::from_nanos(4),
            maxCopProcessAddress: "19.19.8.10".into(),
            maxCopWaitTime: Duration::from_nanos(4),
            maxCopWaitAddress: "19.19.8.10".into(),
            sumProcessTime: Duration::from_nanos(1),
            maxProcessTime: Duration::from_nanos(1),
            sumWaitTime: Duration::from_nanos(2),
            maxWaitTime: Duration::from_nanos(1),
            sumBackoffTime: Duration::from_nanos(2),
            maxBackoffTime: Duration::from_nanos(2),
            sumTotalKeys: 3,
            maxTotalKeys: 2,
            sumProcessedKeys: 8,
            maxProcessedKeys: 4,
            sumRocksdbDeleteSkippedCount: 8,
            maxRocksdbDeleteSkippedCount: 2,
            sumRocksdbKeySkippedCount: 8,
            maxRocksdbKeySkippedCount: 3,
            sumRocksdbBlockCacheHitCount: 8,
            maxRocksdbBlockCacheHitCount: 3,
            sumRocksdbBlockReadCount: 3,
            maxRocksdbBlockReadCount: 3,
            sumRocksdbBlockReadByte: 4,
            maxRocksdbBlockReadByte: 4,
            commitCount: 8,
            sumPrewriteTime: Duration::from_nanos(3),
            maxPrewriteTime: Duration::from_nanos(3),
            sumCommitTime: Duration::from_nanos(8),
            maxCommitTime: Duration::from_nanos(5),
            sumGetCommitTsTime: Duration::from_nanos(8),
            maxGetCommitTsTime: Duration::from_nanos(8),
            sumCommitBackoffTime: 8,
            maxCommitBackoffTime: 8,
            sumResolveLockTime: 8,
            maxResolveLockTime: 8,
            sumLocalLatchTime: Duration::from_nanos(8),
            maxLocalLatchTime: Duration::from_nanos(8),
            sumWriteKeys: 8,
            maxWriteKeys: 8,
            sumWriteSize: 8,
            maxWriteSize: 8,
            sumPrewriteRegionNum: 8,
            maxPrewriteRegionNum: 8,
            sumTxnRetry: 8,
            maxTxnRetry: 8,
            sumBackoffTimes: 8,
            backoffTypes: HashMap::from([("txnlock".into(), 2)]),
            planCacheHits: 8,
            sumAffectedRows: 8,
            sumMem: 8,
            maxMem: 8,
            sumDisk: 8,
            maxDisk: 8,
            sumMemArbitration: 11.0,
            maxMemArbitration: 11.0,
            firstSeen: SystemTime::UNIX_EPOCH + Duration::from_secs(first_seen),
            lastSeen: SystemTime::UNIX_EPOCH + Duration::from_secs(last_seen),
            execRetryCount: 8,
            execRetryTime: Duration::from_nanos(8),
            sumKVTotal: Duration::from_nanos(2),
            sumPDTotal: Duration::from_nanos(2),
            sumBackoffTotal: Duration::from_nanos(2),
            sumWriteSQLRespTotal: Duration::from_nanos(100),
            sumErrors: 8,
            StmtRUSummary: StmtRUSummary {
                SumRRU: 1.0,
                MaxRRU: 1.0,
                ..Default::default()
            },
            resourceGroupName: "rg-a".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

/// addInfo 全字段合并与极值/集合语义。
#[test]
fn test_add_info() {
    let mut add_to = merge_fixture(90, 92);
    let mut add_with = merge_fixture(80, 100);
    add_with.stmtSummaryStats.authUsers.insert("b".into());
    add_with.stmtSummaryStats.maxCopProcessTime = Duration::from_nanos(15);
    add_with.stmtSummaryStats.maxCopProcessAddress = "1.14.5.14".into();
    add_with.stmtSummaryStats.sumMemArbitration = 13.0;
    add_with.stmtSummaryStats.maxMemArbitration = 17.0;
    add_with.stmtSummaryStats.resourceGroupName = "rg-b".into();
    addInfo(&mut add_to, &add_with);
    let stats = &add_to.stmtSummaryStats;

    assert_eq!(stats.authUsers, HashSet::from(["a".into(), "b".into()]));
    assert_eq!(
        (stats.execCount, stats.sumWarnings, stats.sumErrors),
        (6, 16, 16)
    );
    assert_eq!(
        (stats.sumLatency, stats.maxLatency, stats.minLatency),
        (
            Duration::from_nanos(16),
            Duration::from_nanos(5),
            Duration::from_nanos(1)
        )
    );
    assert_eq!(
        (stats.sumParseLatency, stats.sumCompileLatency),
        (Duration::from_nanos(6), Duration::from_nanos(6))
    );
    assert_eq!(
        (
            stats.sumNumCopTasks,
            stats.maxCopProcessTime,
            stats.maxCopProcessAddress.as_str()
        ),
        (8, Duration::from_nanos(15), "1.14.5.14")
    );
    assert_eq!(
        (
            stats.sumTotalKeys,
            stats.maxTotalKeys,
            stats.sumProcessedKeys,
            stats.maxProcessedKeys
        ),
        (6, 2, 16, 4)
    );
    assert_eq!(
        (
            stats.sumRocksdbDeleteSkippedCount,
            stats.sumRocksdbKeySkippedCount,
            stats.sumRocksdbBlockCacheHitCount
        ),
        (16, 16, 16)
    );
    assert_eq!(
        (
            stats.commitCount,
            stats.sumCommitBackoffTime,
            stats.sumWriteKeys,
            stats.sumWriteSize
        ),
        (16, 16, 16, 16)
    );
    assert_eq!(stats.backoffTypes.get("txnlock"), Some(&4));
    assert_eq!(
        (
            stats.planCacheHits,
            stats.sumAffectedRows,
            stats.sumMem,
            stats.maxMem,
            stats.sumDisk,
            stats.maxDisk
        ),
        (16, 16, 16, 8, 16, 8)
    );
    assert_eq!(
        (stats.sumMemArbitration, stats.maxMemArbitration),
        (24.0, 17.0)
    );
    assert_eq!(
        stats.firstSeen,
        SystemTime::UNIX_EPOCH + Duration::from_secs(80)
    );
    assert_eq!(
        stats.lastSeen,
        SystemTime::UNIX_EPOCH + Duration::from_secs(100)
    );
    assert_eq!(
        (stats.execRetryCount, stats.execRetryTime),
        (16, Duration::from_nanos(16))
    );
    assert_eq!(
        (
            stats.sumKVTotal,
            stats.sumPDTotal,
            stats.sumBackoffTotal,
            stats.sumWriteSQLRespTotal
        ),
        (
            Duration::from_nanos(4),
            Duration::from_nanos(4),
            Duration::from_nanos(4),
            Duration::from_nanos(200)
        )
    );
    assert_eq!(
        (stats.StmtRUSummary.SumRRU, stats.StmtRUSummary.MaxRRU),
        (2.0, 1.0)
    );
    assert_eq!(stats.resourceGroupName, "rg-b");
}

#[test]
fn evicted_count_rows_are_safe_during_concurrent_statement_adds() {
    let mut summaries = newStmtSummaryByDigestMap();
    summaries.SetMaxStmtCount(1).unwrap();
    summaries.set_now_for_test(Some(120));
    let summaries = std::sync::Arc::new(std::sync::Mutex::new(summaries));
    std::thread::scope(|scope| {
        let writer = summaries.clone();
        scope.spawn(move || {
            for index in 0..200 {
                let mut info = exec_info("digest", "user", 120);
                info.SchemaName = format!("schema_{index}");
                writer.lock().unwrap().AddStatement(&info);
            }
        });
        scope.spawn(|| {
            for _ in 0..200 {
                for row in summaries.lock().unwrap().ToEvictedCountDatum() {
                    assert_eq!(row.len(), 3);
                    assert!((1..=199).contains(&row[2].GetInt64()));
                }
            }
        });
    });
    let rows = summaries.lock().unwrap().ToEvictedCountDatum();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][2].GetInt64(), 199);
}

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

// 淘汰聚合迁移期单元测试：窗口匹配、裁剪、nil key、addInfo 与 reader 辅助函数。
//
// 对照 Go `evicted` 语义，验证区间插入顺序、计数 Datum 与列工厂输出。

use super::*;
use std::collections::HashMap;
use std::time::{Duration, UNIX_EPOCH};

/// 构造仅含 begin/end 的摘要元素。
fn element(begin: i64, end: i64) -> stmtSummaryByDigestElement {
    stmtSummaryByDigestElement {
        beginTime: begin,
        endTime: end,
        ..Default::default()
    }
}

/// 构造含单个历史窗口的 digest 摘要。
fn digest(begin: i64, end: i64) -> stmtSummaryByDigest {
    let mut value = stmtSummaryByDigest::default();
    value.history.push_back(element(begin, end));
    value
}

/// 将淘汰历史渲染为 (begin, end, count)，最新在前。
fn rendered(history: &stmtSummaryByDigestEvicted) -> Vec<(i64, i64, i64)> {
    history
        .history
        .iter()
        .rev()
        .map(|item| (item.beginTime, item.endTime, item.count))
        .collect()
}

/// 验证 AddEvicted 的匹配、排序与 historySize 裁剪与 Go 一致。
#[test]
fn add_evicted_matches_orders_and_trims_like_go() {
    let key = StmtDigestKey::default();
    let mut evicted = newStmtSummaryByDigestEvicted();

    evicted.AddEvicted(Some(&key), Some(&digest(1, 2)), 1);
    evicted.AddEvicted(Some(&key), Some(&digest(1, 2)), 1);
    assert_eq!(rendered(&evicted), vec![(1, 2, 2)]);

    evicted.AddEvicted(Some(&key), Some(&digest(5, 6)), 2);
    evicted.AddEvicted(Some(&key), Some(&digest(3, 4)), 3);
    assert_eq!(rendered(&evicted), vec![(5, 6, 1), (3, 4, 1), (1, 2, 2)]);

    evicted.AddEvicted(Some(&key), Some(&digest(8, 9)), 2);
    assert_eq!(rendered(&evicted), vec![(8, 9, 1), (5, 6, 1)]);
}

/// nil key 只刷新窗口不增 count；nil value / historySize=0 不保留历史。
#[test]
fn nil_key_refreshes_windows_without_incrementing_count() {
    let mut evicted = newStmtSummaryByDigestEvicted();
    evicted.AddEvicted(None, Some(&digest(10, 20)), 4);
    assert_eq!(rendered(&evicted), vec![(10, 20, 0)]);
    evicted.AddEvicted(Some(&StmtDigestKey::default()), None, 4);
    assert_eq!(rendered(&evicted), vec![(10, 20, 0)]);
    evicted.AddEvicted(Some(&StmtDigestKey::default()), Some(&digest(30, 40)), 0);
    assert!(evicted.history.is_empty());
}

/// 多区间 digest 按 historySize 保留最新若干窗口。
#[test]
fn multi_interval_digest_keeps_newest_history_size() {
    let key = StmtDigestKey::default();
    let mut value = stmtSummaryByDigest::default();
    for (begin, end) in [(1, 2), (2, 3), (5, 6), (8, 9)] {
        value.history.push_back(element(begin, end));
    }
    let mut evicted = newStmtSummaryByDigestEvicted();
    evicted.AddEvicted(Some(&key), Some(&value), 3);
    assert_eq!(rendered(&evicted), vec![(8, 9, 1), (5, 6, 1), (2, 3, 1)]);
}

/// matchAndAdd 边界：落入窗口 / 过旧 / 过新 / 空值。
#[test]
fn match_and_add_preserves_go_boundary_rules() {
    let key = StmtDigestKey::default();
    let mut window = newStmtSummaryByDigestEvictedElement(10, 20);
    assert_eq!(
        window.matchAndAdd(Some(&key), Some(&element(12, 18))),
        isMatch
    );
    assert_eq!(window.count, 1);
    assert_eq!(
        window.matchAndAdd(Some(&key), Some(&element(1, 10))),
        isTooOld
    );
    assert_eq!(
        window.matchAndAdd(Some(&key), Some(&element(20, 21))),
        isTooYoung
    );
    assert_eq!(window.matchAndAdd(Some(&key), None), isTooYoung);
}

/// addInfo 合并求和、极值、集合与覆盖型字段。
#[test]
fn add_info_merges_sums_extrema_sets_and_identity_fields() {
    let mut add_to = element(1, 2);
    add_to.stmtSummaryStats.execCount = 2;
    add_to.stmtSummaryStats.sumLatency = Duration::from_nanos(10);
    add_to.stmtSummaryStats.maxLatency = Duration::from_nanos(7);
    add_to.stmtSummaryStats.minLatency = Duration::from_nanos(4);
    add_to.stmtSummaryStats.authUsers.insert("alice".into());
    add_to.stmtSummaryStats.backoffTypes = HashMap::from([("rpc".into(), 2)]);
    add_to.stmtSummaryStats.firstSeen = UNIX_EPOCH + Duration::from_secs(20);
    add_to.stmtSummaryStats.lastSeen = UNIX_EPOCH + Duration::from_secs(30);

    let mut add_with = element(1, 2);
    add_with.stmtSummaryStats.execCount = 3;
    add_with.stmtSummaryStats.sumLatency = Duration::from_nanos(30);
    add_with.stmtSummaryStats.maxLatency = Duration::from_nanos(12);
    add_with.stmtSummaryStats.minLatency = Duration::from_nanos(2);
    add_with.stmtSummaryStats.authUsers.insert("bob".into());
    add_with.stmtSummaryStats.backoffTypes = HashMap::from([("rpc".into(), 5), ("txn".into(), 1)]);
    add_with.stmtSummaryStats.firstSeen = UNIX_EPOCH + Duration::from_secs(10);
    add_with.stmtSummaryStats.lastSeen = UNIX_EPOCH + Duration::from_secs(40);
    add_with.stmtSummaryStats.resourceGroupName = "rg2".into();
    add_with.stmtSummaryStats.StmtRUSummary.SumRRU = 1.25;

    addInfo(&mut add_to, &add_with);
    let stats = &add_to.stmtSummaryStats;
    assert_eq!(stats.execCount, 5);
    assert_eq!(stats.sumLatency, Duration::from_nanos(40));
    assert_eq!(stats.maxLatency, Duration::from_nanos(12));
    assert_eq!(stats.minLatency, Duration::from_nanos(2));
    assert_eq!(stats.authUsers.len(), 2);
    assert_eq!(
        stats.backoffTypes,
        HashMap::from([("rpc".into(), 7), ("txn".into(), 1)])
    );
    assert_eq!(stats.firstSeen, UNIX_EPOCH + Duration::from_secs(10));
    assert_eq!(stats.lastSeen, UNIX_EPOCH + Duration::from_secs(40));
    assert_eq!(stats.resourceGroupName, "rg2");
    assert_eq!(stats.StmtRUSummary.SumRRU, 1.25);
}

/// ToEvictedCountDatum 最新区间在前。
#[test]
fn count_datums_are_returned_newest_first() {
    let key = StmtDigestKey::default();
    let mut evicted = newStmtSummaryByDigestEvicted();
    evicted.AddEvicted(Some(&key), Some(&digest(100, 110)), 4);
    evicted.AddEvicted(Some(&key), Some(&digest(200, 210)), 4);
    let rows = evicted.ToEvictedCountDatum();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][2].GetInt64(), 1);
    assert_eq!(rows[1][2].GetInt64(), 1);
}

/// reader 辅助：除零与 backoff 类型格式化规则。
#[test]
fn reader_helpers_match_go_zero_and_sorting_rules() {
    assert_eq!(avgInt(9, 2), 4);
    assert_eq!(avgInt(9, 0), 0);
    assert_eq!(avgFloat(9, 2), 4.5);
    assert_eq!(avgFloat(9, 0), 0.0);
    assert_eq!(avgSumFloat(1.5, 2), 0.75);
    assert_eq!(
        formatBackoffTypes(&HashMap::from([("txn".into(), 1), ("rpc".into(), 3)])),
        Some("rpc:3,txn:1".to_owned())
    );
}

/// 列工厂按列名产出真实 Datum（实例、schema、平均延迟）。
#[test]
fn reader_column_factories_emit_real_datums() {
    let mut instance = model::ColumnInfo::default();
    instance.Name.O = ClusterTableInstanceColumnNameStr.into();
    let mut schema = model::ColumnInfo::default();
    schema.Name.O = SchemaNameStr.into();
    let mut latency = model::ColumnInfo::default();
    latency.Name.O = AvgLatencyStr.into();

    let reader = NewStmtSummaryReader(
        None,
        false,
        vec![instance, schema, latency],
        "127.0.0.1:4000".into(),
        chrono_tz::UTC,
    );
    let mut summary = stmtSummaryByDigest::default();
    summary.schemaName = "test".into();
    let mut stats = stmtSummaryStats::default();
    stats.sumLatency = Duration::from_nanos(9);
    stats.execCount = 2;

    let values: Vec<_> = reader
        .columnValueFactories
        .iter()
        .map(|factory| factory(&reader, None, Some(&summary), &stats).into_datum())
        .collect();
    assert_eq!(values[0].GetString(), "127.0.0.1:4000");
    assert_eq!(values[1].GetString(), "test");
    assert_eq!(values[2].GetInt64(), 4);
}

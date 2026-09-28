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

// RU 窗口聚合器单元测试。
//
// 覆盖上报粒度（15/30/60）、Top-N 压缩、窗口只 take 一次、handover 丢弃、
// 并发压力、迟到数据移位/丢弃，以及超容量时保留热点键等行为。
// RU（Request Unit）是 TiDB 资源计量单位。

#![allow(non_snake_case, static_mut_refs)]

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Barrier};
use std::thread;

use reporter_metrics::{metrics as parent_metrics, reporter_metrics as global_metrics};
use topsql_reporter::stmtstats::{self, BinaryDigest, RUIncrement, RUIncrementMap, RUKey};
use topsql_reporter::*;

/// 构造聚合键。
fn key(user: &str, sql: &str, plan: &str) -> RUKey {
    RUKey {
        User: user.to_owned(),
        SQLDigest: BinaryDigest::from(sql.as_bytes()),
        PlanDigest: BinaryDigest::from(plan.as_bytes()),
    }
}

/// 构造仅关心 TotalRU 的增量（ExecCount/Duration 固定为 1）。
fn increment(total: f64) -> RUIncrement {
    RUIncrement {
        TotalRU: total,
        ExecCount: 1,
        ExecDuration: 1,
    }
}

/// 单键批次。
fn singleton(record_key: RUKey, total: f64) -> RUIncrementMap {
    HashMap::from([(record_key, increment(total))])
}

/// 以默认 RU 版本写入批次。
fn add(aggregator: &RUWindowAggregator, timestamp: u64, data: RUIncrementMap) {
    aggregator.add_batch(timestamp, data, stmtstats::DEFAULT_RU_VERSION);
}

/// 按用户与 digest 查找记录。
fn find_record<'a>(
    records: &'a [tipb_protobuf::TopRuRecord],
    user: &str,
    sql: &str,
    plan: &str,
) -> Option<&'a tipb_protobuf::TopRuRecord> {
    records.iter().find(|record| {
        record.get_user() == user
            && record.get_sql_digest() == sql.as_bytes()
            && record.get_plan_digest() == plan.as_bytes()
    })
}

/// 汇总单条记录全部时间点的 RU。
fn record_total(record: &tipb_protobuf::TopRuRecord) -> f64 {
    record
        .get_items()
        .iter()
        .map(|item| item.get_total_ru())
        .sum()
}

/// 汇总全部记录的 RU。
fn total(records: &[tipb_protobuf::TopRuRecord]) -> f64 {
    records.iter().map(record_total).sum()
}

/// 构造递减 RU 的大规模用户×SQL 批次，便于验证 Top-N。
fn make_ru_batch(num_users: usize, num_sqls_per_user: usize) -> RUIncrementMap {
    let mut batch = HashMap::with_capacity(num_users * num_sqls_per_user);
    for user in 0..num_users {
        for sql in 0..num_sqls_per_user {
            batch.insert(
                key(
                    &format!("u{user:04}"),
                    &format!("sql{user:04}_{sql:04}"),
                    "plan",
                ),
                increment((num_users * num_sqls_per_user - user * num_sqls_per_user - sql) as f64),
            );
        }
    }
    batch
}

/// 断言记录条目的时间戳与 RU 序列。
fn assert_items(record: &tipb_protobuf::TopRuRecord, expected: &[(u64, f64)]) {
    assert_eq!(record.get_items().len(), expected.len());
    for (item, (timestamp, total)) in record.get_items().iter().zip(expected) {
        assert_eq!(item.get_timestamp_sec(), *timestamp);
        assert!((item.get_total_ru() - total).abs() < 1e-9);
    }
}

#[test]
/// 验证 itemInterval=15/30/60 时时间点合并粒度正确。
fn test_ru_window_aggregator_report_granularity() {
    for (interval, expected) in [
        (15, vec![(0, 1.0), (15, 2.0), (30, 3.0), (45, 4.0)]),
        (30, vec![(0, 3.0), (30, 7.0)]),
        (60, vec![(0, 10.0)]),
    ] {
        let aggregator = RUWindowAggregator::new();
        for (timestamp, ru) in [(1, 1.0), (16, 2.0), (31, 3.0), (46, 4.0)] {
            add(
                &aggregator,
                timestamp,
                singleton(key("u1", "sql1", "plan1"), ru),
            );
        }
        let records = aggregator.take_report_records(60, interval, b"ks".to_vec());
        assert_items(
            find_record(&records, "u1", "sql1", "plan1").unwrap(),
            &expected,
        );
    }
}

#[test]
/// 验证用户过多时压缩到上报 Top-N，并保留 others 线标签。
fn test_ru_window_aggregator_compact_to_200() {
    let aggregator = RUWindowAggregator::new();
    let mut batch = HashMap::with_capacity(250);
    let expected_total: f64 = (1..=250).map(f64::from).sum();
    for user in 0..250 {
        batch.insert(
            key(&format!("u{user:03}"), "sql", "plan"),
            increment((250 - user) as f64),
        );
    }
    add(&aggregator, 1, batch);
    add(&aggregator, 16, singleton(key("next", "sql", "plan"), 1.0));
    let records = aggregator.take_report_records(60, 60, b"ks".to_vec());
    let real_users = records
        .iter()
        .filter(|record| record.get_user() != othersUserWireLabel)
        .map(|record| record.get_user())
        .collect::<HashSet<_>>();
    assert!(real_users.len() <= ruReportTopNUsers);
    assert!(
        records
            .iter()
            .any(|record| record.get_user() == othersUserWireLabel)
    );
    assert!((total(&records) - (expected_total + 1.0)).abs() < 1e-6);
}

#[test]
/// 验证同一上报窗口只成功 take 一次。
fn test_ru_window_aggregator_take_once_per_window() {
    let aggregator = RUWindowAggregator::new();
    add(&aggregator, 1, singleton(key("u1", "sql1", "plan1"), 1.0));
    assert!(
        aggregator
            .take_report_records(59, 60, b"ks".to_vec())
            .is_empty()
    );
    assert!(
        !aggregator
            .take_report_records(60, 60, b"ks".to_vec())
            .is_empty()
    );
    assert!(
        aggregator
            .take_report_records(61, 60, b"ks".to_vec())
            .is_empty()
    );
}

#[test]
/// 验证 handover 丢弃未完成窗口数据，且错误版本被忽略。
fn test_ru_window_aggregator_reset_current_window_drops_until_boundary() {
    const VERSION_TWO: stmtstats::RUVersion = 2;
    let initial = RUWindowAggregator::new();
    initial.add_batch(
        1,
        singleton(key("u-init", "sql-init", "plan-init"), 1.0),
        VERSION_TWO,
    );
    assert!(
        find_record(
            &initial.take_report_records(60, 60, b"ks".to_vec()),
            "u-init",
            "sql-init",
            "plan-init"
        )
        .is_some()
    );

    let aggregator = RUWindowAggregator::new();
    aggregator.reset_for_handover(VERSION_TWO, 73);
    aggregator.add_batch(
        76,
        singleton(key("dropped", "sql", "plan"), 1.0),
        VERSION_TWO,
    );
    assert!(
        aggregator
            .take_report_records(120, 60, b"ks".to_vec())
            .is_empty()
    );
    aggregator.add_batch(121, singleton(key("kept", "sql", "plan"), 2.0), VERSION_TWO);
    aggregator.add_batch(122, singleton(key("wrong-version", "sql", "plan"), 3.0), 1);
    let records = aggregator.take_report_records(180, 60, b"ks".to_vec());
    assert!(find_record(&records, "kept", "sql", "plan").is_some());
    assert!(find_record(&records, "wrong-version", "sql", "plan").is_none());
}

#[test]
/// 验证多线程并发写入后 RU 总量与 Top SQL 上限仍成立。
fn test_ru_window_aggregator_concurrent_pressure() {
    let aggregator = Arc::new(RUWindowAggregator::new());
    let barrier = Arc::new(Barrier::new(5));
    let mut handles = Vec::new();
    for worker in 0..4 {
        let aggregator = Arc::clone(&aggregator);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for sql in 0..250 {
                add(
                    &aggregator,
                    1,
                    singleton(key("user", &format!("sql-{worker}-{sql}"), "plan"), 1.0),
                );
            }
        }));
    }
    barrier.wait();
    for handle in handles {
        handle.join().unwrap();
    }
    let records = aggregator.take_report_records(60, 60, b"ks".to_vec());
    assert!((total(&records) - 1000.0).abs() < 1e-9);
    assert!(records.len() <= ruReportTopNSQLsPerUser + 1);
}

#[test]
/// 验证迟到数据可挪入下一窗口；目标桶已关闭则计入丢弃指标。
fn test_ru_window_aggregator_shifts_late_data_after_window_reported() {
    let aggregator = RUWindowAggregator::new();
    add(
        &aggregator,
        1,
        singleton(key("u1", "sql-a", "plan-a"), 10.0),
    );
    let first = aggregator.take_report_records(60, 60, b"ks".to_vec());
    assert!(find_record(&first, "u1", "sql-a", "plan-a").is_some());

    add(
        &aggregator,
        10,
        singleton(key("u1", "sql-late", "plan-late"), 999.0),
    );
    add(
        &aggregator,
        61,
        singleton(key("u1", "sql-cur", "plan-cur"), 1.0),
    );
    let second = aggregator.take_report_records(120, 60, b"ks".to_vec());
    assert_eq!(
        record_total(find_record(&second, "u1", "sql-late", "plan-late").unwrap()),
        999.0
    );
    assert_eq!(
        record_total(find_record(&second, "u1", "sql-cur", "plan-cur").unwrap()),
        1.0
    );
    assert_eq!(total(&second), 1000.0);

    parent_metrics::init_parent_metrics();
    global_metrics::InitMetricsVars();
    let (before_global_keys, before_global_ru) = unsafe {
        (
            global_metrics::IgnoreLateCompactedRUKeysCounter
                .as_ref()
                .unwrap()
                .get(),
            global_metrics::IgnoreLateCompactedRUTotalCounter
                .as_ref()
                .unwrap()
                .get(),
        )
    };
    let dropped = RUWindowAggregator::new();
    add(&dropped, 1, singleton(key("u1", "sql-a", "plan-a"), 10.0));
    dropped.take_report_records(60, 60, b"ks".to_vec());
    add(
        &dropped,
        61,
        singleton(key("u1", "sql-cur", "plan-cur"), 1.0),
    );
    add(
        &dropped,
        76,
        singleton(key("u1", "sql-cur-2", "plan-cur-2"), 2.0),
    );
    add(
        &dropped,
        10,
        singleton(key("u1", "sql-late", "plan-late"), 999.0),
    );
    assert_eq!(dropped.dropped_late_keys(), 1);
    assert_eq!(dropped.dropped_late_ru(), 999.0);
    unsafe {
        assert_eq!(
            global_metrics::IgnoreLateCompactedRUKeysCounter
                .as_ref()
                .unwrap()
                .get()
                - before_global_keys,
            1.0
        );
        assert_eq!(
            global_metrics::IgnoreLateCompactedRUTotalCounter
                .as_ref()
                .unwrap()
                .get()
                - before_global_ru,
            999.0
        );
    }
}

#[test]
/// 验证并发上报与迟到写入下，迟到 RU = 已上报 + 丢弃。
fn test_late_data_under_concurrent_reporting() {
    let aggregator = Arc::new(RUWindowAggregator::new());
    for timestamp in [1, 16, 31, 46] {
        add(
            &aggregator,
            timestamp,
            singleton(key("u-a", "sql-a", "plan-a"), 1.0),
        );
    }
    for timestamp in [61, 76, 91, 106] {
        add(
            &aggregator,
            timestamp,
            singleton(key("u-b", "sql-b", "plan-b"), 2.0),
        );
    }
    aggregator.take_report_records(60, 60, b"ks".to_vec());

    let barrier = Arc::new(Barrier::new(3));
    let writer = {
        let aggregator = Arc::clone(&aggregator);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            for _ in 0..200 {
                add(
                    &aggregator,
                    10,
                    singleton(key("u-late", "sql-late", "plan-late"), 999.0),
                );
            }
        })
    };
    let reporter = {
        let aggregator = Arc::clone(&aggregator);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            barrier.wait();
            aggregator.take_report_records(120, 60, b"ks".to_vec())
        })
    };
    barrier.wait();
    writer.join().unwrap();
    let second = reporter.join().unwrap();
    assert_eq!(
        record_total(find_record(&second, "u-b", "sql-b", "plan-b").unwrap()),
        8.0
    );
    assert!(
        aggregator
            .take_report_records(120, 60, b"ks".to_vec())
            .is_empty()
    );
    for timestamp in [121, 136, 151, 166] {
        add(
            &aggregator,
            timestamp,
            singleton(key("u-c", "sql-c", "plan-c"), 3.0),
        );
    }
    let third = aggregator.take_report_records(180, 60, b"ks".to_vec());
    assert!(find_record(&third, "u-c", "sql-c", "plan-c").is_some());
    let reported_late = find_record(&second, "u-late", "sql-late", "plan-late")
        .map_or(0.0, record_total)
        + find_record(&third, "u-late", "sql-late", "plan-late").map_or(0.0, record_total);
    assert!((reported_late + aggregator.dropped_late_ru() - 200.0 * 999.0).abs() < 1e-6);
    assert!(
        (aggregator.dropped_late_ru() / 999.0 - aggregator.dropped_late_keys() as f64).abs() < 1e-9
    );
}

#[test]
/// 验证最终报告用户数与每用户 SQL 数不超过 100×100。
fn test_ru_window_aggregator_final_report_capped_to_100x100() {
    let aggregator = RUWindowAggregator::new();
    let batch = make_ru_batch(120, 120);
    add(&aggregator, 1, batch);
    let records = aggregator.take_report_records(60, 60, b"ks".to_vec());
    assert!(!records.is_empty());
    let mut real_users: HashMap<&str, usize> = HashMap::new();
    let mut others_total = 0.0;
    for record in &records {
        if record.get_user() == othersUserWireLabel {
            assert!(record.get_sql_digest().is_empty() && record.get_plan_digest().is_empty());
            others_total += record_total(record);
        } else if !record.get_sql_digest().is_empty() || !record.get_plan_digest().is_empty() {
            *real_users.entry(record.get_user()).or_default() += 1;
        } else {
            real_users.entry(record.get_user()).or_default();
        }
    }
    assert!(real_users.len() <= ruReportTopNUsers);
    assert!(
        real_users
            .values()
            .all(|count| *count <= ruReportTopNSQLsPerUser)
    );
    assert!(others_total > 0.0);
}

#[test]
/// 验证稀疏桶重分组不产生虚假时间点。
fn test_ru_window_aggregator_regroup_sparse_buckets_no_phantom_points() {
    for (interval, expected) in [(30, vec![(0, 2.0), (30, 3.0)]), (60, vec![(0, 5.0)])] {
        let aggregator = RUWindowAggregator::new();
        add(
            &aggregator,
            1,
            singleton(key("u-sparse", "sql-sparse", "plan-sparse"), 2.0),
        );
        add(
            &aggregator,
            31,
            singleton(key("u-sparse", "sql-sparse", "plan-sparse"), 3.0),
        );
        let records = aggregator.take_report_records(60, interval, b"ks".to_vec());
        assert_eq!(records.len(), 1);
        assert_items(
            find_record(&records, "u-sparse", "sql-sparse", "plan-sparse").unwrap(),
            &expected,
        );
        assert_eq!(total(&records), 5.0);
    }
}

#[test]
/// 验证超容量时高 RU 热点键仍被保留，且不泄漏到下一窗口。
fn test_ru_window_aggregator_over_cap_behavior_keeps_hot_keys() {
    const HOT_RU: f64 = 1e9;
    let aggregator = RUWindowAggregator::new();
    let mut batch = make_ru_batch(130, 130);
    batch.insert(key("u-hot", "sql-hot", "plan-hot"), increment(HOT_RU));
    for timestamp in [1, 16, 31, 46] {
        add(&aggregator, timestamp, batch.clone());
    }
    let records = aggregator.take_report_records(60, 60, b"ks".to_vec());
    let mut users: HashMap<&str, usize> = HashMap::new();
    for record in &records {
        if record.get_user() == othersUserWireLabel {
            assert!(record.get_sql_digest().is_empty() && record.get_plan_digest().is_empty());
        } else if !record.get_sql_digest().is_empty() || !record.get_plan_digest().is_empty() {
            *users.entry(record.get_user()).or_default() += 1;
        } else {
            users.entry(record.get_user()).or_default();
        }
    }
    assert!(users.len() <= ruReportTopNUsers);
    assert!(
        users
            .values()
            .all(|count| *count <= ruReportTopNSQLsPerUser)
    );
    assert_eq!(
        record_total(find_record(&records, "u-hot", "sql-hot", "plan-hot").unwrap()),
        HOT_RU * 4.0
    );

    add(
        &aggregator,
        61,
        singleton(key("u-next", "sql-next", "plan-next"), 7.0),
    );
    let next = aggregator.take_report_records(120, 60, b"ks".to_vec());
    assert!(find_record(&next, "u-hot", "sql-hot", "plan-hot").is_none());
    assert_eq!(
        record_total(find_record(&next, "u-next", "sql-next", "plan-next").unwrap()),
        7.0
    );
}

// Rust stable has no built-in benchmark harness equivalent to testing.B. Keep the
// Go benchmark fixtures executable by criterion-style callers without weakening
// the ten behavioral tests above.
#[allow(dead_code)]
/// 基准矩阵参数占位，供 criterion 风格调用方复用。
fn benchmark_matrix_cases() -> [(usize, usize, u64); 2] {
    [(200, 200, 60), (1000, 500, 60)]
}

#[allow(dead_code)]
/// 构造热点主导 + 长尾键的批次（基准辅助）。
fn make_hotspot_dominated_batch(num_tail_keys: usize, hot_ru: f64) -> RUIncrementMap {
    let mut batch = HashMap::with_capacity(num_tail_keys + 1);
    batch.insert(key("u-hot", "sql-hot", "plan-hot"), increment(hot_ru));
    for index in 0..num_tail_keys {
        batch.insert(
            key(
                &format!("u-tail-{:03}", index % 200),
                &format!("sql-tail-{index:05}"),
                "plan-tail",
            ),
            increment(1.0),
        );
    }
    batch
}

#[allow(dead_code)]
/// 在普通批次上叠加若干热点用户（基准辅助）。
fn make_long_tail_batch(
    num_users: usize,
    sqls_per_user: usize,
    hot_users: usize,
    hot_ru: f64,
) -> RUIncrementMap {
    let mut batch = make_ru_batch(num_users, sqls_per_user);
    for index in 0..hot_users {
        batch.insert(
            key(&format!("u-hot-{index:02}"), "sql-hot", "plan-hot"),
            increment(hot_ru - index as f64),
        );
    }
    batch
}

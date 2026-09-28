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

// RU 数据模型单元测试。
//
// 覆盖 `ruItem` / `ruRecord` / `userRUCollecting` / `ruCollecting` 的累加、合并、
// Top-N 截断、others（其余）折叠，以及压缩上报为 tipb `TopRuRecord` 的一致性。
// RU（Request Unit）是 TiDB 资源计量单位；digest 是 SQL/执行计划的哈希指纹。

#![allow(non_snake_case)]

use std::collections::HashMap;

use topsql_reporter::stmtstats::{BinaryDigest, RUIncrement, RUKey};
use topsql_reporter::*;

/// 由原始字节构造 SQL/执行计划 digest。
fn digest(value: impl AsRef<[u8]>) -> BinaryDigest {
    BinaryDigest::from(value.as_ref())
}

/// 构造一次 RU 增量（总量、执行次数、执行时长）。
fn increment(total: f64, count: u64, duration: u64) -> RUIncrement {
    RUIncrement {
        TotalRU: total,
        ExecCount: count,
        ExecDuration: duration,
    }
}

/// 构造 (用户, SQL digest, 计划 digest) 聚合键。
fn key(user: &str, sql: &str, plan: &str) -> RUKey {
    RUKey {
        User: user.to_owned(),
        SQLDigest: digest(sql),
        PlanDigest: digest(plan),
    }
}

/// 判断 protobuf 记录是否匹配给定用户与 digest。
fn is_record(record: &tipb_protobuf::TopRuRecord, user: &str, sql: &str, plan: &str) -> bool {
    record.get_user() == user
        && record.get_sql_digest() == sql.as_bytes()
        && record.get_plan_digest() == plan.as_bytes()
}

#[test]
/// 验证 `ruItem` 字段完整映射到 tipb `TopRuRecordItem`。
fn test_ru_item_to_proto() {
    let item = ruItem {
        timestamp: 1000,
        totalRU: 100.5,
        execCount: 10,
        execDuration: 5000,
    };
    let proto = item.toProto();
    assert_eq!(proto.get_timestamp_sec(), 1000);
    assert_eq!(proto.get_total_ru(), 100.5);
    assert_eq!(proto.get_exec_count(), 10);
    assert_eq!(proto.get_exec_duration(), 5000);
}

#[test]
/// 验证同时间戳累加、跨时间戳新增条目，以及 totalRU 汇总。
fn test_ru_record_add() {
    let mut record = newRURecord(digest("sql1"), digest("plan1"));
    record.add(1000, 10.0, 1, 100);
    assert_eq!(record.items.len(), 1);
    assert_eq!(record.totalRU, 10.0);
    assert_eq!(record.items[0].timestamp, 1000);

    record.add(1000, 5.0, 2, 50);
    assert_eq!(record.items.len(), 1);
    assert_eq!(record.totalRU, 15.0);
    assert_eq!(record.items[0].totalRU, 15.0);
    assert_eq!(record.items[0].execCount, 3);

    record.add(1001, 20.0, 1, 200);
    assert_eq!(record.items.len(), 2);
    assert_eq!(record.totalRU, 35.0);
}

#[test]
/// 验证两条 `ruRecord` 按时间戳合并条目并累加 totalRU。
fn test_ru_record_merge() {
    let mut first = newRURecord(digest("sql1"), digest("plan1"));
    first.add(1000, 10.0, 1, 100);
    first.add(1001, 20.0, 2, 200);
    let mut second = newRURecord(digest("sql1"), digest("plan1"));
    second.add(1000, 5.0, 1, 50);
    second.add(1002, 15.0, 1, 150);

    first.merge(Some(&second));
    assert_eq!(first.totalRU, 50.0);
    assert_eq!(first.items.len(), 3);
    assert_eq!(first.items[0].totalRU, 15.0);
    assert_eq!(first.items[0].execCount, 2);
}

#[test]
/// 与 Go `mergeRecord` 一致：没有时间序列项的源记录不得创建记录或累计 totalRU。
fn test_user_ru_collecting_merge_record_ignores_empty_source() {
    let mut user = newUserRUCollectingWithCap("user", 10);
    let mut source = newRURecord(digest("sql1"), digest("plan1"));
    source.totalRU = 7.0;

    user.mergeRecord(
        makeKey(digest("sql1"), digest("plan1")),
        Some(&source),
        1000,
        false,
    );

    assert!(user.records.is_empty());
    assert!(user.othersRec.is_none());
    assert_eq!(user.totalRU, 0.0);
}

#[test]
/// 验证按 RU 取 Top-N SQL，溢出部分折叠进空 digest 的 others 记录。
fn test_ru_records_top_n() {
    let mut user = newUserRUCollectingWithCap("user", 10);
    for i in 0..5 {
        user.add(
            1000,
            digest(format!("sql{i}")),
            BinaryDigest::default(),
            Some(&increment(((i + 1) * 10) as f64, 0, 0)),
        );
    }
    let records = user.getReportRecordsWithLimit(3);
    assert_eq!(records.len(), 4);
    let top_ru: f64 = records
        .iter()
        .filter(|record| !record.sqlDigest.as_bytes().is_empty())
        .map(|record| record.totalRU)
        .sum();
    assert_eq!(top_ru, 120.0);
    assert_eq!(
        records
            .iter()
            .find(|record| record.sqlDigest.as_bytes().is_empty())
            .unwrap()
            .totalRU,
        30.0
    );
}

#[test]
/// 验证用户侧 SQL Top-N 上限，超出部分计入 others。
fn test_user_ru_collecting_top_n_sqls() {
    let mut user = newUserRUCollecting("user1");
    let num_sqls = maxTopSQLsPerUser + 10;
    for i in 0..num_sqls {
        user.add(
            1000,
            digest(format!("sql{i}")),
            BinaryDigest::default(),
            Some(&increment((i + 1) as f64, 1, 100)),
        );
    }
    assert_eq!(user.records.len(), num_sqls);
    let report = user.getReportRecordsWithLimit(maxTopSQLsPerUser);
    assert_eq!(report.len(), maxTopSQLsPerUser + 1);
    let others = report
        .iter()
        .find(|record| record.sqlDigest.as_bytes().is_empty())
        .unwrap();
    assert_eq!(others.totalRU, 55.0);
}

#[test]
/// 验证预聚合阶段 SQL 容量上限，超出立即折入 othersRec。
fn test_user_ru_collecting_pre_top_n_sql_cap() {
    let mut user = newUserRUCollecting("user1");
    for i in 0..maxPreTopNSQLsPerUser + 5 {
        user.add(
            1000,
            digest(format!("sql{i}")),
            BinaryDigest::default(),
            Some(&increment(1.0, 1, 10)),
        );
    }
    assert_eq!(user.records.len(), maxPreTopNSQLsPerUser);
    assert_eq!(user.othersRec.as_ref().unwrap().totalRU, 5.0);
}

#[test]
/// 验证 others 哨兵键为空 digest，且 `isOthersKey` 判定正确。
fn test_others_key_sentinel() {
    assert_eq!(
        *othersKey,
        makeKey(BinaryDigest::default(), BinaryDigest::default())
    );
    assert_ne!(*othersKey, makeKey(digest("sql"), BinaryDigest::default()));
    assert_ne!(*othersKey, makeKey(BinaryDigest::default(), digest("plan")));
    assert!(isOthersKey(&othersKey));
    assert!(!isOthersKey(&makeKey(
        digest("sql"),
        BinaryDigest::default()
    )));
}

#[test]
/// 验证空 digest 直接进入 othersRec，不占用 records。
fn test_user_ru_collecting_empty_digests_go_to_others_rec() {
    let mut user = newUserRUCollectingWithCap("user1", 10);
    user.add(
        1000,
        BinaryDigest::default(),
        BinaryDigest::default(),
        Some(&increment(3.0, 1, 10)),
    );
    assert!(user.records.is_empty());
    assert_eq!(user.othersRec.as_ref().unwrap().totalRU, 3.0);
    user.add(
        1001,
        digest("sql1"),
        BinaryDigest::default(),
        Some(&increment(2.0, 1, 10)),
    );
    assert_eq!(user.records.len(), 1);
}

#[test]
/// 验证 `addOthers` 将遗留 othersKey 条目折叠进 othersRec。
fn test_user_ru_collecting_add_others_folds_legacy_others_key() {
    let mut user = newUserRUCollectingWithCap("user1", 10);
    let mut legacy = newOthersRURecord();
    legacy.add(1000, 3.0, 1, 10);
    user.totalRU = legacy.totalRU;
    user.records.insert((*othersKey).clone(), legacy);
    user.addOthers(1001, Some(&increment(2.0, 1, 10)));
    assert!(!user.records.contains_key(&*othersKey));
    assert_eq!(user.othersRec.as_ref().unwrap().totalRU, 5.0);
}

#[test]
/// 验证全局用户 Top-N 压缩后出现 others 线标签，并写入 keyspace。
fn test_ru_collecting_hybrid_top_n() {
    let mut collecting = newRUCollecting();
    for user in 0..maxTopUsers + 5 {
        for sql in 0..3 {
            collecting.add(
                1000,
                key(&format!("user{user}"), &format!("sql{sql}"), ""),
                Some(&increment((user + 1) as f64, 1, 100)),
            );
        }
    }
    assert_eq!(collecting.users.len(), maxTopUsers + 5);
    let mut compacted = collecting
        .compactWithLimits(maxTopUsers, maxTopSQLsPerUser)
        .unwrap();
    let records = compacted.toTopRURecords(b"test-keyspace".to_vec());
    assert!(
        records
            .iter()
            .any(|record| record.get_user() == othersUserWireLabel)
    );
    assert!(
        records
            .iter()
            .all(|record| record.get_keyspace_name() == b"test-keyspace")
    );
}

#[test]
/// 验证预聚合用户容量上限，溢出用户累加到 othersUser。
fn test_ru_collecting_pre_top_n_user_cap() {
    let mut collecting = newRUCollecting();
    for user in 0..maxPreTopNUsers + 5 {
        collecting.add(
            1000,
            key(&format!("user{user}"), "sql", ""),
            Some(&increment(1.0, 0, 0)),
        );
    }
    assert_eq!(collecting.users.len(), maxPreTopNUsers);
    assert_eq!(collecting.othersUser.as_ref().unwrap().totalRU, 5.0);
}

/// 断言同时存在指定用户的 per-user others 与全局 others 线标签记录。
fn assert_per_user_and_global_others(mut collecting: Box<ruCollecting>, runtime_user: &str) {
    let records = collecting.toTopRURecords(b"ks".to_vec());
    assert!(
        records
            .iter()
            .any(|record| is_record(record, runtime_user, "", ""))
    );
    assert!(
        records
            .iter()
            .any(|record| is_record(record, othersUserWireLabel, "", ""))
    );
}

#[test]
/// 验证全局 others 线标签不与形如 `app@ip` 的运行时用户冲突。
fn test_ru_collecting_others_wire_label_no_collision_with_runtime_user_shape() {
    let runtime_user = "app@127.0.0.1";
    let mut collecting = newRUCollectingWithCaps(1, 1);
    collecting.add(
        1000,
        key(runtime_user, "sql-top", "plan-top"),
        Some(&increment(10.0, 1, 10)),
    );
    collecting.add(
        1001,
        key(runtime_user, "sql-overflow", "plan-overflow"),
        Some(&increment(8.0, 1, 10)),
    );
    collecting.add(
        1002,
        key(
            "other@127.0.0.1",
            "sql-global-overflow",
            "plan-global-overflow",
        ),
        Some(&increment(7.0, 1, 10)),
    );
    assert_per_user_and_global_others(collecting, runtime_user);
}

#[test]
/// 验证空用户名与全局 others 在上报中保持可区分。
fn test_ru_collecting_empty_user_and_global_others_remain_distinct() {
    let mut collecting = newRUCollectingWithCaps(1, 1);
    collecting.add(
        1000,
        key("", "sql-empty-top", "plan-empty-top"),
        Some(&increment(10.0, 1, 10)),
    );
    collecting.add(
        1001,
        key("", "sql-empty-overflow", "plan-empty-overflow"),
        Some(&increment(8.0, 1, 10)),
    );
    collecting.add(
        1002,
        key(
            "other@127.0.0.1",
            "sql-global-overflow",
            "plan-global-overflow",
        ),
        Some(&increment(7.0, 1, 10)),
    );
    assert_per_user_and_global_others(collecting, "");
}

#[test]
/// 验证 `mergeFrom` 后空用户与全局 others 仍不合并。
fn test_ru_collecting_merge_from_keeps_empty_user_distinct_from_global_others() {
    let mut destination = newRUCollectingWithCaps(1, 1);
    destination.add(
        1000,
        key("", "sql-empty-top", "plan-empty-top"),
        Some(&increment(10.0, 1, 10)),
    );
    destination.add(
        1001,
        key("", "sql-empty-overflow", "plan-empty-overflow"),
        Some(&increment(8.0, 1, 10)),
    );
    let mut source = newRUCollectingWithCaps(1, 1);
    source.add(
        1002,
        key("other@127.0.0.1", "sql-other-top", "plan-other-top"),
        Some(&increment(7.0, 1, 10)),
    );
    source.add(
        1003,
        key(
            "other2@127.0.0.1",
            "sql-other-overflow",
            "plan-other-overflow",
        ),
        Some(&increment(6.0, 1, 10)),
    );
    destination.mergeFrom(Some(&source), 0, false);
    assert_per_user_and_global_others(destination, "");
}

#[test]
/// 验证 `addBatch` 按用户分桶累加 RU。
fn test_ru_collecting_add_batch() {
    let mut collecting = newRUCollecting();
    collecting.addBatch(
        1000,
        HashMap::from([
            (key("user1", "sql1", ""), increment(10.0, 0, 0)),
            (key("user1", "sql2", ""), increment(20.0, 0, 0)),
            (key("user2", "sql1", ""), increment(30.0, 0, 0)),
        ]),
    );
    assert_eq!(collecting.users.len(), 2);
    assert_eq!(collecting.users["user1"].totalRU, 30.0);
    assert_eq!(collecting.users["user2"].totalRU, 30.0);
}

#[test]
/// 验证 `take` 移出收集结果并清空原容器。
fn test_ru_collecting_take() {
    let mut collecting = newRUCollecting();
    collecting.add(1000, key("user1", "sql1", ""), Some(&increment(10.0, 0, 0)));
    let taken = collecting.take();
    assert_eq!(taken.users.len(), 1);
    assert!(collecting.users.is_empty());
}

/// 将 TopRuRecord 规范化为可排序字符串，便于一致性断言。
fn normalized(records: &[tipb_protobuf::TopRuRecord]) -> Vec<String> {
    let mut output = records
        .iter()
        .map(|record| {
            let mut items = record
                .get_items()
                .iter()
                .map(|item| {
                    format!(
                        "{}|{:.6}|{}|{}",
                        item.get_timestamp_sec(),
                        item.get_total_ru(),
                        item.get_exec_count(),
                        item.get_exec_duration()
                    )
                })
                .collect::<Vec<_>>();
            items.sort();
            format!(
                "{}|{:x?}|{:x?}|{:x?}|{}",
                record.get_user(),
                record.get_sql_digest(),
                record.get_plan_digest(),
                record.get_keyspace_name(),
                items.join(",")
            )
        })
        .collect::<Vec<_>>();
    output.sort();
    output
}

#[test]
/// 验证压缩后 `toTopRURecords` 产出非空规范化结果。
fn test_ru_collecting_compact_and_report_consistency() {
    let mut collecting = newRUCollecting();
    for (user, sql, plan, timestamp, total) in [
        ("u1", "s1", "p1", 0, 100.0),
        ("u1", "s2", "p2", 15, 80.0),
        ("u2", "s1", "p1", 0, 70.0),
        ("u2", "s2", "p2", 30, 60.0),
        ("u3", "s1", "p1", 0, 50.0),
        ("u4", "s1", "p1", 0, 10.0),
    ] {
        collecting.add(
            timestamp,
            key(user, sql, plan),
            Some(&increment(total, 1, 10)),
        );
    }
    let mut compacted = collecting.compactWithLimits(2, 1).unwrap();
    assert!(!normalized(&compacted.toTopRURecords(b"ks".to_vec())).is_empty());
}

#[test]
/// 覆盖用户/SQL Top-N、预存在 others、仅 others、单用户、遗留 othersKey 等压缩路径。
fn test_compact_with_limits() {
    let mut first = newRUCollectingWithCaps(10, 10);
    let mut user = newUserRUCollectingWithCap("u1", 10);
    user.add(
        1000,
        digest("sql-top"),
        digest("plan-top"),
        Some(&increment(100.0, 1, 10)),
    );
    user.add(
        1001,
        digest("sql-evicted"),
        digest("plan-evicted"),
        Some(&increment(40.0, 1, 10)),
    );
    user.addOthers(1002, Some(&increment(7.0, 1, 10)));
    first.users.insert("u1".to_owned(), user);
    let compacted = first.compactWithLimits(1, 1).unwrap();
    let u1 = &compacted.users["u1"];
    assert_eq!(u1.records.len(), 1);
    assert_eq!(u1.othersRec.as_ref().unwrap().totalRU, 47.0);

    let mut second = newRUCollectingWithCaps(10, 10);
    for (user, total) in [("u1", 100.0), ("u2", 30.0)] {
        let mut collecting = newUserRUCollectingWithCap(user, 10);
        collecting.add(
            2000,
            digest(format!("sql-{user}")),
            digest("plan"),
            Some(&increment(total, 1, 10)),
        );
        second.users.insert(user.to_owned(), collecting);
    }
    let mut pre_others = newOthersUserRUCollectingWithCap(10);
    pre_others.add(
        2000,
        digest("sql-pre-others"),
        digest("plan"),
        Some(&increment(6.0, 1, 10)),
    );
    pre_others.addOthers(2001, Some(&increment(4.0, 1, 10)));
    second.othersUser = Some(pre_others);
    let compacted = second.compactWithLimits(1, 1).unwrap();
    assert_eq!(
        compacted
            .othersUser
            .as_ref()
            .unwrap()
            .othersRec
            .as_ref()
            .unwrap()
            .totalRU,
        40.0
    );

    let mut only_others = newRUCollectingWithCaps(10, 10);
    let mut others = newOthersUserRUCollectingWithCap(10);
    others.addOthers(3000, Some(&increment(11.0, 1, 10)));
    only_others.othersUser = Some(others);
    assert_eq!(
        only_others
            .compactWithLimits(1, 1)
            .unwrap()
            .othersUser
            .unwrap()
            .othersRec
            .unwrap()
            .totalRU,
        11.0
    );

    let mut single = newRUCollectingWithCaps(10, 10);
    single.add(
        4000,
        key("u1", "sql-only", "plan-only"),
        Some(&increment(88.0, 1, 10)),
    );
    let compacted = single.compactWithLimits(1, 1).unwrap();
    assert_eq!(compacted.users.len(), 1);
    assert!(compacted.othersUser.is_none());

    let mut legacy = newRUCollectingWithCaps(10, 10);
    let mut legacy_user = newOthersUserRUCollectingWithCap(10);
    let mut legacy_record = newOthersRURecord();
    legacy_record.add(5000, 13.0, 2, 30);
    legacy_user.totalRU = legacy_record.totalRU;
    legacy_user
        .records
        .insert((*othersKey).clone(), legacy_record);
    legacy.othersUser = Some(legacy_user);
    let compacted = legacy.compactWithLimits(1, 1).unwrap();
    assert_eq!(
        compacted.othersUser.unwrap().othersRec.unwrap().totalRU,
        13.0
    );
}

#[test]
/// 验证 `ruItem` 可按 timestamp 排序。
fn test_ru_items_sort() {
    let mut items = vec![
        ruItem {
            timestamp: 1002,
            ..Default::default()
        },
        ruItem {
            timestamp: 1000,
            ..Default::default()
        },
        ruItem {
            timestamp: 1001,
            ..Default::default()
        },
    ];
    items.sort_by_key(|item| item.timestamp);
    assert_eq!(
        items.iter().map(|item| item.timestamp).collect::<Vec<_>>(),
        vec![1000, 1001, 1002]
    );
}

#[test]
/// 验证一组 `ruItem` 批量转为 protobuf。
fn test_ru_items_to_proto() {
    let items = [
        ruItem {
            timestamp: 1000,
            totalRU: 10.0,
            execCount: 1,
            execDuration: 100,
        },
        ruItem {
            timestamp: 1001,
            totalRU: 20.0,
            execCount: 2,
            execDuration: 200,
        },
    ];
    let proto = items.iter().map(ruItem::toProto).collect::<Vec<_>>();
    assert_eq!(proto.len(), 2);
    assert_eq!(proto[0].get_timestamp_sec(), 1000);
    assert_eq!(proto[1].get_timestamp_sec(), 1001);
}

#[test]
/// 验证同一时间桶、同一键的多次增量会累加到同一条目。
fn test_ru_collecting_same_bucket_same_key_accumulates() {
    let mut collecting = newRUCollecting();
    let record_key = key("u1", "sql1", "plan1");
    collecting.addBatch(
        1000,
        HashMap::from([(record_key.clone(), increment(10.0, 1, 100))]),
    );
    collecting.addBatch(1000, HashMap::from([(record_key, increment(7.0, 0, 40))]));
    let mut compacted = collecting
        .compactWithLimits(maxTopUsers, maxTopSQLsPerUser)
        .unwrap();
    let records = compacted.toTopRURecords(b"ks".to_vec());
    assert_eq!(records.len(), 1);
    let item = &records[0].get_items()[0];
    assert!(is_record(&records[0], "u1", "sql1", "plan1"));
    assert_eq!(item.get_timestamp_sec(), 1000);
    assert_eq!(item.get_total_ru(), 17.0);
    assert_eq!(item.get_exec_count(), 1);
    assert_eq!(item.get_exec_duration(), 140);
}

#[test]
/// 验证空收集器压缩返回 None。
fn test_empty_ru_collecting() {
    let collecting = newRUCollecting();
    assert!(
        collecting
            .compactWithLimits(maxTopUsers, maxTopSQLsPerUser)
            .is_none()
    );
}

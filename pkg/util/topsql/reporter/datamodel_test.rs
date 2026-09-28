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

// TopSQL reporter 数据模型单测：时间序列项、记录合并、TopN 与规范化元数据。
//
// 覆盖 `TsItem`/`Record`/`Collecting`/`NormalizedSqlMap`/`NormalizedPlanMap` 等
// 结构的排序、合并、淘汰（evict）与 protobuf 序列化；TopSQL 指按 CPU/耗时
// 聚合后的“最耗资源 SQL”上报。

use std::collections::HashMap;

use super::*;

/// 构造仅含执行次数与总耗时的语句统计项。
fn stats(exec_count: u64, duration: u64) -> StatementStatsItem {
    StatementStatsItem {
        ExecCount: exec_count,
        SumDurationNs: duration,
        ..Default::default()
    }
}

/// 构造仅含时间戳与 CPU 毫秒的时间序列点。
fn ts_item(timestamp: u64, cpu_time_ms: u32) -> TsItem {
    TsItem {
        timestamp,
        cpu_time_ms,
        ..TsItem::zero()
    }
}

/// 验证单个时间序列点字段完整映射到 protobuf。
#[test]
fn test_ts_item_to_proto() {
    let item = TsItem {
        timestamp: 1,
        cpu_time_ms: 2,
        stmt_stats: StatementStatsItem {
            ExecCount: 3,
            SumDurationNs: 50_000,
            DurationCount: 2,
            KvStatsItem: KvStatementStatsItem {
                KvExecCount: Some(HashMap::from([(String::new(), 4)])),
            },
            ..Default::default()
        },
    };
    let proto = item.to_proto();
    assert_eq!(proto.get_timestamp_sec(), 1);
    assert_eq!(proto.get_cpu_time_ms(), 2);
    assert_eq!(proto.get_stmt_exec_count(), 3);
    assert_eq!(proto.get_stmt_duration_sum_ns(), 50_000);
    assert_eq!(proto.get_stmt_duration_count(), 2);
    assert_eq!(proto.get_stmt_kv_exec_count()[""], 4);
}

/// 验证时间序列点可按 timestamp 升序排序。
#[test]
fn test_ts_items_sort() {
    let mut items: Vec<TsItem> = Vec::new();
    assert!(
        items
            .windows(2)
            .all(|pair| pair[0].timestamp <= pair[1].timestamp)
    );
    items = vec![ts_item(2, 0), ts_item(3, 0), ts_item(1, 0)];
    assert!(
        !items
            .windows(2)
            .all(|pair| pair[0].timestamp <= pair[1].timestamp)
    );
    // 乱序输入经 sort_by_key 后应严格升序。
    items.sort_by_key(|item| item.timestamp);
    assert_eq!(
        items.iter().map(|item| item.timestamp).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

/// 验证一批时间序列点均可转成 protobuf。
#[test]
fn test_ts_items_to_proto() {
    let items = [TsItem::zero(), TsItem::zero(), TsItem::zero()];
    assert_eq!(items.iter().map(TsItem::to_proto).count(), 3);
}

/// 验证 Record 排序后重建 timestamp→下标索引。
#[test]
fn test_record_sort() {
    let mut record = Record {
        ts_items: vec![ts_item(2, 0), ts_item(3, 0), ts_item(1, 0)],
        ts_index: HashMap::from([(2, 0), (3, 1), (1, 2)]),
        ..Default::default()
    };
    record.sort_and_rebuild();
    assert_eq!(record.timestamps(), vec![1, 2, 3]);
    assert_eq!(record.ts_index, HashMap::from([(1, 0), (2, 1), (3, 2)]));
}

/// 验证交错追加 CPU 与语句统计后，时间轴对齐且总量正确。
#[test]
fn test_record_append() {
    let mut record = Record::new(Vec::new(), Vec::new());
    // 同一 timestamp 可分别追加 CPU 与语句耗时，最终按时间戳对齐。
    record.append_cpu_time(1, 1);
    record.append_stmt_stats_item(1, stats(1, 10_000));
    record.append_cpu_time(2, 1);
    record.append_cpu_time(3, 1);
    record.append_stmt_stats_item(3, stats(1, 30_000));
    record.append_stmt_stats_item(2, stats(1, 20_000));

    assert_eq!(record.timestamps(), vec![1, 2, 3]);
    assert_eq!(record.cpu_times_ms(), vec![1, 1, 1]);
    assert_eq!(record.total_cpu_time_ms(), 3);
    assert_eq!(
        record
            .statement_stats()
            .iter()
            .map(|item| item.ExecCount)
            .collect::<Vec<_>>(),
        vec![1, 1, 1]
    );
    assert_eq!(
        record
            .statement_stats()
            .iter()
            .map(|item| item.SumDurationNs)
            .collect::<Vec<_>>(),
        vec![10_000, 20_000, 30_000]
    );
}

/// 验证两段 Record 合并后时间戳拼接且 CPU 累加。
#[test]
fn test_record_merge() {
    let mut first = Record::new(Vec::new(), Vec::new());
    for (timestamp, cpu) in [(1, 1), (2, 2), (3, 3)] {
        first.append_cpu_time(timestamp, cpu);
    }
    let mut second = Record::new(Vec::new(), Vec::new());
    for (timestamp, cpu) in [(6, 6), (5, 5), (4, 4)] {
        second.append_cpu_time(timestamp, cpu);
    }
    // merge 会吸收 second 的点并重排 first。
    first.merge(Some(&mut second));
    assert_eq!(second.timestamps(), vec![4, 5, 6]);
    assert_eq!(first.timestamps(), vec![1, 2, 3, 4, 5, 6]);
    assert_eq!(first.total_cpu_time_ms(), 21);
}

/// 验证空/非空 ts_items 下重建索引的行为。
#[test]
fn test_record_rebuild_ts_index() {
    let mut record = Record {
        ts_index: HashMap::from([(1, 1)]),
        ..Default::default()
    };
    // 无 ts_items 时索引应清空。
    record.rebuild_ts_index();
    assert!(record.ts_index.is_empty());
    record.ts_items = vec![ts_item(1, 1), ts_item(2, 2), ts_item(3, 3)];
    record.rebuild_ts_index();
    assert_eq!(record.ts_index, HashMap::from([(1, 0), (2, 1), (3, 2)]));
}

/// 验证 Record 转 protobuf 时带上 keyspace、digest 与 items。
#[test]
fn test_record_to_proto() {
    let record = Record {
        sql_digest: b"SQL-1".to_vec(),
        plan_digest: b"PLAN-1".to_vec(),
        total_cpu_time_ms: 123,
        ts_items: vec![TsItem::zero(), TsItem::zero(), TsItem::zero()],
        ..Default::default()
    };
    let proto = record.to_proto(b"123".to_vec());
    assert_eq!(proto.get_keyspace_name(), b"123");
    assert_eq!(proto.get_sql_digest(), b"SQL-1");
    assert_eq!(proto.get_plan_digest(), b"PLAN-1");
    assert_eq!(proto.get_items().len(), 3);
}

/// 验证 Records 按总 CPU 降序排序。
#[test]
fn test_records_sort() {
    let mut records = Records(vec![
        Record {
            total_cpu_time_ms: 1,
            ..Default::default()
        },
        Record {
            total_cpu_time_ms: 3,
            ..Default::default()
        },
        Record {
            total_cpu_time_ms: 2,
            ..Default::default()
        },
    ]);
    records
        .0
        .sort_by_key(|record| std::cmp::Reverse(record.total_cpu_time_ms));
    assert_eq!(
        records
            .iter()
            .map(Record::total_cpu_time_ms)
            .collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
}

/// 验证 TopN 保留 CPU 最高的 N 条，其余进入淘汰列表。
#[test]
fn test_records_top_n() {
    let records = Records(vec![
        Record {
            total_cpu_time_ms: 1,
            ..Default::default()
        },
        Record {
            total_cpu_time_ms: 3,
            ..Default::default()
        },
        Record {
            total_cpu_time_ms: 2,
            ..Default::default()
        },
    ]);
    let (top, evicted) = records.top_n(2);
    let evicted = evicted.unwrap();
    assert_eq!(
        top.iter()
            .map(Record::total_cpu_time_ms)
            .collect::<Vec<_>>(),
        vec![3, 2]
    );
    assert_eq!(
        evicted
            .iter()
            .map(Record::total_cpu_time_ms)
            .collect::<Vec<_>>(),
        vec![1]
    );
}

/// 验证 Records 批量转 protobuf 的条数。
#[test]
fn test_records_to_proto() {
    assert_eq!(
        Records(vec![Record::default(), Record::default()])
            .to_proto(Vec::new())
            .len(),
        2
    );
}

/// 同一 SQL/Plan digest 应返回同一 Record 指针。
#[test]
fn test_collecting_get_or_create_record() {
    let mut collecting = Collecting::new();
    let first = collecting.get_or_create_record(b"SQL-1", b"PLAN-1") as *mut Record;
    let second = collecting.get_or_create_record(b"SQL-1", b"PLAN-1") as *mut Record;
    assert_eq!(first, second);
    assert_eq!(collecting.record_count(), 1);
}

/// 验证按时间戳+digest 标记淘汰后可正确查询。
#[test]
fn test_collecting_mark_as_evicted_has_evicted() {
    let mut collecting = Collecting::new();
    collecting.mark_as_evicted(1, b"SQL-1", b"PLAN-1");
    assert!(collecting.has_evicted(1, b"SQL-1", b"PLAN-1"));
    assert!(!collecting.has_evicted(1, b"SQL-2", b"PLAN-2"));
    assert!(!collecting.has_evicted(2, b"SQL-1", b"PLAN-1"));
}

/// 验证“others”汇总桶累计被淘汰 SQL 的 CPU 与语句统计。
#[test]
fn test_collecting_append_others() {
    let mut collecting = Collecting::new();
    collecting.append_others_cpu_time(1, 1);
    collecting.append_others_cpu_time(2, 2);
    collecting.append_others_stmt_stats_item(1, stats(1, 1_000));
    collecting.append_others_stmt_stats_item(2, stats(2, 2_000));
    let record = &collecting.records[KEY_OTHERS];
    assert_eq!(record.timestamps(), vec![1, 2]);
    assert_eq!(record.cpu_times_ms(), vec![1, 2]);
    assert_eq!(
        record
            .statement_stats()
            .iter()
            .map(|item| item.ExecCount)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        record
            .statement_stats()
            .iter()
            .map(|item| item.SumDurationNs)
            .collect::<Vec<_>>(),
        vec![1_000, 2_000]
    );
}

/// 验证上报列表含各 SQL 记录，且 others 桶排在末尾。
#[test]
fn test_collecting_get_report_records() {
    let mut collecting = Collecting::new();
    for (sql, plan, cpu) in [
        (b"SQL-1".as_slice(), b"PLAN-1".as_slice(), 1),
        (b"SQL-2", b"PLAN-2", 2),
        (b"SQL-3", b"PLAN-3", 3),
    ] {
        collecting
            .get_or_create_record(sql, plan)
            .append_cpu_time(1, cpu);
    }
    collecting.append_others_cpu_time(1, 10);
    let records = collecting.report_records();
    assert_eq!(records.len(), 4);
    assert_eq!(records.last().unwrap().cpu_times_ms(), vec![10]);
    assert_eq!(records.last().unwrap().total_cpu_time_ms(), 10);
}

/// 验证 take 抽走全部记录且 key_buf 缓冲区独立。
#[test]
fn test_collecting_take() {
    let mut first = Collecting::new();
    first
        .get_or_create_record(b"SQL-1", b"PLAN-1")
        .append_cpu_time(1, 1);
    let second = first.take();
    assert!(first.records.is_empty());
    assert_eq!(second.records.len(), 1);
    // key_buf 为内部复用缓冲，take 后应是不同分配。
    assert_ne!(first.key_buf.as_ptr(), second.key_buf.as_ptr());
}

/// 验证 CpuRecords 按 CPUTimeMs 降序排序。
#[test]
fn test_cpu_records_sort() {
    let mut records = CpuRecords(vec![
        SQLCPUTimeRecord {
            CPUTimeMs: 1,
            ..Default::default()
        },
        SQLCPUTimeRecord {
            CPUTimeMs: 3,
            ..Default::default()
        },
        SQLCPUTimeRecord {
            CPUTimeMs: 2,
            ..Default::default()
        },
    ]);
    records
        .0
        .sort_by_key(|record| std::cmp::Reverse(record.CPUTimeMs));
    assert_eq!(
        records
            .0
            .iter()
            .map(|record| record.CPUTimeMs)
            .collect::<Vec<_>>(),
        vec![3, 2, 1]
    );
}

/// 验证 CpuRecords 的 TopN 与淘汰集合。
#[test]
fn test_cpu_records_top_n() {
    let records = CpuRecords(vec![
        SQLCPUTimeRecord {
            CPUTimeMs: 1,
            ..Default::default()
        },
        SQLCPUTimeRecord {
            CPUTimeMs: 3,
            ..Default::default()
        },
        SQLCPUTimeRecord {
            CPUTimeMs: 2,
            ..Default::default()
        },
    ]);
    let (top, evicted) = records.top_n(2);
    assert_eq!(
        top.0
            .iter()
            .map(|record| record.CPUTimeMs)
            .collect::<Vec<_>>(),
        vec![3, 2]
    );
    assert_eq!(evicted.unwrap().0[0].CPUTimeMs, 1);
}

/// 验证规范化 SQL 映射容量上限：超出后 register 失败。
#[test]
fn test_normalized_sql_map_register() {
    let map = NormalizedSqlMap::new(2);
    assert!(map.register(b"SQL-1", "SQL-1".into(), true));
    assert!(map.register(b"SQL-2", "SQL-2".into(), false));
    // 容量为 2，第三条应被拒绝。
    assert!(!map.register(b"SQL-3", "SQL-3".into(), true));
    assert_eq!(map.len(), 2);
    let data = map.data.lock().unwrap();
    assert_eq!(data[b"SQL-1".as_slice()].normalized_sql, "SQL-1");
    assert!(data[b"SQL-1".as_slice()].is_internal);
    assert_eq!(data[b"SQL-2".as_slice()].normalized_sql, "SQL-2");
    assert!(!data[b"SQL-2".as_slice()].is_internal);
    assert!(!data.contains_key(b"SQL-3".as_slice()));
}

/// 验证 take 抽走全部规范化 SQL 元数据。
#[test]
fn test_normalized_sql_map_take() {
    let first = NormalizedSqlMap::new(999);
    for digest in ["SQL-1", "SQL-2", "SQL-3"] {
        assert!(first.register(digest.as_bytes(), digest.into(), true));
    }
    let second = first.take();
    assert!(first.is_empty());
    assert_eq!(second.len(), 3);
    let data = second.data.lock().unwrap();
    assert!(
        ["SQL-1", "SQL-2", "SQL-3"]
            .iter()
            .all(|digest| data.contains_key(digest.as_bytes()))
    );
}

/// 验证规范化 SQL 转 protobuf 时携带 keyspace 与 is_internal 标志。
#[test]
fn test_normalized_sql_map_to_proto() {
    let map = NormalizedSqlMap::new(999);
    for (digest, internal) in [("SQL-1", true), ("SQL-2", false), ("SQL-3", true)] {
        assert!(map.register(digest.as_bytes(), digest.into(), internal));
    }
    let mut protos = map.to_proto(b"12345".to_vec());
    protos.sort_by(|left, right| left.get_sql_digest().cmp(right.get_sql_digest()));
    assert_eq!(protos.len(), 3);
    for (proto, (digest, internal)) in
        protos
            .iter()
            .zip([("SQL-1", true), ("SQL-2", false), ("SQL-3", true)])
    {
        assert_eq!(proto.get_keyspace_name(), b"12345");
        assert_eq!(proto.get_sql_digest(), digest.as_bytes());
        assert_eq!(proto.get_normalized_sql(), digest);
        assert_eq!(proto.get_is_internal_sql(), internal);
    }
}

/// 验证规范化执行计划映射容量与 is_large 标记。
#[test]
fn test_normalized_plan_map_register() {
    let map = NormalizedPlanMap::new(2);
    assert!(map.register(b"PLAN-1", "PLAN-1".into(), false));
    assert!(map.register(b"PLAN-2", "PLAN-2".into(), true));
    assert!(!map.register(b"PLAN-3", "PLAN-3".into(), false));
    assert_eq!(map.len(), 2);
    let data = map.data.lock().unwrap();
    assert_eq!(data[b"PLAN-1".as_slice()].binary_normalized_plan, "PLAN-1");
    assert!(!data[b"PLAN-1".as_slice()].is_large);
    assert_eq!(data[b"PLAN-2".as_slice()].binary_normalized_plan, "PLAN-2");
    assert!(data[b"PLAN-2".as_slice()].is_large);
    assert!(!data.contains_key(b"PLAN-3".as_slice()));
}

/// 验证 take 抽走全部规范化 Plan 元数据。
#[test]
fn test_normalized_plan_map_take() {
    let first = NormalizedPlanMap::new(999);
    for digest in ["PLAN-1", "PLAN-2", "PLAN-3"] {
        assert!(first.register(digest.as_bytes(), digest.into(), false));
    }
    let second = first.take();
    assert!(first.is_empty());
    assert_eq!(second.len(), 3);
    let data = second.data.lock().unwrap();
    assert!(
        ["PLAN-1", "PLAN-2", "PLAN-3"]
            .iter()
            .all(|digest| data.contains_key(digest.as_bytes()))
    );
}

/// 验证大计划走压缩编码路径，普通计划走解码文本路径。
#[test]
fn test_normalized_plan_map_to_proto() {
    let map = NormalizedPlanMap::new(999);
    for (digest, large) in [("PLAN-1", false), ("PLAN-2", true), ("PLAN-3", false)] {
        assert!(map.register(digest.as_bytes(), digest.into(), large));
    }
    // is_large=true 使用 encode 回调，否则使用 decode 回调。
    let mut protos = map.to_proto(
        b"12345".to_vec(),
        |plan| Ok(format!("[decoded] {plan}")),
        |plan| format!("[encoded] {}", String::from_utf8_lossy(plan)),
    );
    protos.sort_by(|left, right| left.get_plan_digest().cmp(right.get_plan_digest()));
    assert_eq!(protos.len(), 3);
    assert_eq!(protos[0].get_normalized_plan(), "[decoded] PLAN-1");
    assert_eq!(protos[1].get_encoded_normalized_plan(), "[encoded] PLAN-2");
    assert_eq!(protos[2].get_normalized_plan(), "[decoded] PLAN-3");
    assert!(
        protos
            .iter()
            .all(|proto| proto.get_keyspace_name() == b"12345")
    );
}

/// 验证 SQL digest 与 Plan digest 拼接成内部 map 键。
#[test]
fn test_encode_key() {
    let mut buffer = Vec::with_capacity(64);
    assert_eq!(encode_key(&mut buffer, b"S", b"P"), b"SP");
}

/// 验证空 plan digest 的记录合并进同 SQL 的有效计划，避免无效键残留。
#[test]
fn test_remove_invalid_plan_record() {
    let mut collecting = Collecting::new();
    let input = [
        ("SQL-1", "PLAN-1", vec![1, 2, 3, 5]),
        ("SQL-1", "PLAN-2", vec![1, 2, 5, 6]),
        ("SQL-2", "PLAN-1", vec![1, 2, 3, 5]),
        ("SQL-2", "", vec![1, 2, 3, 4, 6]),
        ("SQL-3", "", vec![2, 3, 5]),
        ("SQL-3", "PLAN-1", vec![1, 2, 3, 4, 6]),
    ];
    for (sql, plan, timestamps) in input {
        let record = collecting.get_or_create_record(sql.as_bytes(), plan.as_bytes());
        for timestamp in timestamps {
            record.append_cpu_time(timestamp, 1);
        }
    }
    collecting.remove_invalid_plan_record();

    let expected = [
        ("SQL-1", "PLAN-1", vec![1, 2, 3, 5], vec![1, 1, 1, 1]),
        ("SQL-1", "PLAN-2", vec![1, 2, 5, 6], vec![1, 1, 1, 1]),
        (
            "SQL-2",
            "PLAN-1",
            vec![1, 2, 3, 4, 5, 6],
            vec![2, 2, 2, 1, 1, 1],
        ),
        (
            "SQL-3",
            "PLAN-1",
            vec![1, 2, 3, 4, 5, 6],
            vec![1, 2, 2, 1, 1, 1],
        ),
    ];
    assert_eq!(collecting.records.len(), expected.len());
    for (sql, plan, timestamps, cpu_times) in expected {
        let key = encode_key(&mut collecting.key_buf, sql.as_bytes(), plan.as_bytes());
        let record = &collecting.records[&key];
        assert_eq!(record.sql_digest(), sql.as_bytes());
        assert_eq!(record.plan_digest(), plan.as_bytes());
        assert_eq!(record.timestamps(), timestamps);
        assert_eq!(record.cpu_times_ms(), cpu_times);
    }
}

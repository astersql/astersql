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

// 事务信息（TxnInfo）与历史摘要记录器的迁移对齐单测。
//
// 校验 `TrxHistoryRecorder` 的 LRU 摘要、物理时间戳阈值过滤，
// 以及 `TxnInfo::ToDatum` / 指标访问器与 Go 行为一致。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::summary::newTrxHistoryRecorder;
use super::txn_info::{
    AllSQLDigestsStr, BlockStartTime, CurrentSQLDigestStr, DBStr, IDStr, MemBufferBytesStr,
    MemBufferKeysStr, ProcessInfo, RelatedTableIDsStr, SessionIDStr, StartTimeStr, StateStr,
    TxnDurationHistogram, TxnIdle, TxnInfo, TxnLockAcquiring, TxnRunning, TxnStatusEnteringCounter,
    UserStr, WaitingStartTimeStr, WaitingTimeStr,
};
use chrono::{DateTime, Datelike, Local, Timelike};
use prometheus::core::Collector;

/// 将系统时间转为类 TSO 的 StartTS：毫秒时间戳左移 18 位（物理时间戳部分）。
fn start_ts(time: SystemTime) -> u64 {
    let millis = time
        .duration_since(UNIX_EPOCH)
        .expect("test timestamps follow the Unix epoch")
        .as_millis() as u64;
    millis << 18
}

/// 计算 SQL digest 序列的 FNV-1a 64 位哈希，并以十六进制字符串返回。
fn fnv64a(digests: &[&str]) -> String {
    let mut hash = 0xcbf29ce484222325_u64;
    for digest in digests {
        for byte in digest.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("{hash:x}")
}

/// 构造仅填充 StartTS 与 AllSQLDigests 的测试用事务信息。
fn info_at(start: SystemTime, digests: &[&str]) -> TxnInfo {
    TxnInfo {
        StartTS: start_ts(start),
        AllSQLDigests: digests.iter().map(|value| (*value).to_owned()).collect(),
        ..TxnInfo::default()
    }
}

#[test]
/// 验证摘要 LRU：命中提升、容量收缩淘汰、Clean 后可再记录。
fn summaries_match_go_lru_digest_json_and_resize_behavior() {
    // 容量为 2：第三次不同 digest 会挤掉最久未用项。
    let recorder = newTrxHistoryRecorder(2);
    recorder.SetMinDuration(Duration::ZERO);
    let old = SystemTime::now() - Duration::from_secs(5);

    recorder.OnTrxEnd(&info_at(old, &["begin", "select"]));
    recorder.OnTrxEnd(&info_at(old, &["update"]));
    recorder.OnTrxEnd(&info_at(old, &["begin", "select"]));

    let rows = recorder.DumpTrxSummary();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0].GetString(), fnv64a(&["begin", "select"]));
    assert_eq!(rows[0][1].GetString(), r#"["begin","select"]"#);
    assert_eq!(rows[1][0].GetString(), fnv64a(&["update"]));

    // 收缩容量后只保留最近命中的摘要。
    recorder.ResizeSummaries(1);
    let rows = recorder.DumpTrxSummary();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1].GetString(), r#"["begin","select"]"#);

    recorder.Clean();
    assert!(recorder.DumpTrxSummary().is_empty());
    recorder.OnTrxEnd(&info_at(old, &["begin", "select"]));
    assert!(recorder.DumpTrxSummary().is_empty());
    recorder.OnTrxEnd(&info_at(old, &["after-clean"]));
    assert_eq!(recorder.DumpTrxSummary().len(), 1);
}

#[test]
/// 验证 Go 的零容量边界，以及记录、导出和调整容量共享同一互斥状态。
fn recorder_matches_go_zero_capacity_and_concurrent_access() {
    let zero_capacity = newTrxHistoryRecorder(0);
    zero_capacity.SetMinDuration(Duration::ZERO);
    zero_capacity.OnTrxEnd(&info_at(
        SystemTime::now() - Duration::from_secs(1),
        &["discarded"],
    ));
    assert!(zero_capacity.DumpTrxSummary().is_empty());

    let recorder = Arc::new(newTrxHistoryRecorder(8));
    recorder.SetMinDuration(Duration::ZERO);
    let old = SystemTime::now() - Duration::from_secs(1);
    let workers: Vec<_> = (0..8)
        .map(|index| {
            let recorder = Arc::clone(&recorder);
            std::thread::spawn(move || {
                let digest = format!("digest-{index}");
                recorder.OnTrxEnd(&info_at(old, &[&digest]));
                let _ = recorder.DumpTrxSummary();
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("recorder worker completes");
    }

    let rows = recorder.DumpTrxSummary();
    assert_eq!(rows.len(), 8);
    let mut summaries: Vec<String> = rows
        .into_iter()
        .map(|row| row[1].GetString().to_owned())
        .collect();
    summaries.sort_unstable();
    assert_eq!(
        summaries,
        (0..8)
            .map(|index| format!(r#"["digest-{index}"]"#))
            .collect::<Vec<_>>()
    );
}

#[test]
/// 验证最小持续时长阈值：过短事务不进入摘要。
fn recorder_applies_go_physical_timestamp_duration_threshold() {
    let recorder = newTrxHistoryRecorder(4);
    recorder.SetMinDuration(Duration::from_secs(2));

    recorder.OnTrxEnd(&info_at(SystemTime::now(), &["too-fast"]));
    recorder.OnTrxEnd(&info_at(
        SystemTime::now() - Duration::from_secs(3),
        &["slow-enough"],
    ));

    let rows = recorder.DumpTrxSummary();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1].GetString(), r#"["slow-enough"]"#);

    let future = newTrxHistoryRecorder(1);
    future.SetMinDuration(Duration::ZERO);
    future.OnTrxEnd(&info_at(
        SystemTime::now() + Duration::from_secs(1),
        &["future"],
    ));
    assert!(future.DumpTrxSummary().is_empty());
}

#[test]
/// 验证 ToDatum 各列形状、枚举值与空事务默认值。
fn txn_info_columns_match_go_datum_shapes_and_defaults() {
    // 构造阻塞等待场景，便于校验 WAITING_* 列。
    let block_start = SystemTime::now() - Duration::from_millis(1250);
    let mut related = HashMap::new();
    related.insert(42, ());
    related.insert(-7, ());
    let info = TxnInfo {
        StartTS: 1_621_516_710_123_u64 << 18,
        CurrentSQLDigest: "digest-1".to_owned(),
        AllSQLDigests: vec!["digest-1".to_owned(), "digest-2".to_owned()],
        State: TxnLockAcquiring,
        BlockStartTime: BlockStartTime {
            Valid: true,
            Time: block_start,
        },
        EntriesCount: 9,
        ProcessInfo: Some(ProcessInfo {
            ConnectionID: 88,
            Username: "root".to_owned(),
            CurrentDB: "test".to_owned(),
            RelatedTableIDs: related,
        }),
        ..TxnInfo::default()
    };

    assert_eq!(info.ToDatum(IDStr).GetUint64(), info.StartTS);
    let start = info.ToDatum(StartTimeStr).GetMysqlTime();
    let start_instant = UNIX_EPOCH + Duration::from_millis(info.StartTS >> 18);
    let local: DateTime<Local> = start_instant.into();
    assert_eq!(
        (
            start.Year(),
            start.Month(),
            start.Day(),
            start.Hour(),
            start.Minute(),
            start.Second(),
        ),
        (
            local.year(),
            local.month() as i32,
            local.day() as i32,
            local.hour() as i32,
            local.minute() as i32,
            local.second() as i32,
        )
    );
    assert_eq!(info.ToDatum(CurrentSQLDigestStr).GetString(), "digest-1");
    let state = info.ToDatum(StateStr).GetMysqlEnum();
    assert_eq!((state.Name.as_str(), state.Value), ("LockWaiting", 3));
    let waiting_start = info.ToDatum(WaitingStartTimeStr).GetMysqlTime();
    assert_eq!(waiting_start.Type(), parser_mysql::r#type::TypeTimestamp);
    assert_eq!(info.ToDatum(MemBufferKeysStr).GetUint64(), 9);
    assert!(info.ToDatum(MemBufferBytesStr).IsNull());
    assert_eq!(info.ToDatum(SessionIDStr).GetUint64(), 88);
    assert_eq!(info.ToDatum(UserStr).GetString(), "root");
    assert_eq!(info.ToDatum(DBStr).GetString(), "test");
    assert_eq!(
        info.ToDatum(AllSQLDigestsStr).GetString(),
        r#"["digest-1","digest-2"]"#
    );
    let mut ids: Vec<i64> = info
        .ToDatum(RelatedTableIDsStr)
        .GetString()
        .split(',')
        .map(|value| value.parse().expect("related table ID is numeric"))
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![-7, 42]);
    assert!(info.ToDatum(WaitingTimeStr).GetFloat64() >= 1.0);
    assert!(info.ToDatum("UNKNOWN").IsNull());

    let empty = TxnInfo::default();
    assert!(empty.ToDatum(CurrentSQLDigestStr).IsNull());
    assert!(empty.ToDatum(WaitingStartTimeStr).IsNull());
    assert!(empty.ToDatum(WaitingTimeStr).IsNull());
    assert_eq!(empty.ToDatum(SessionIDStr).GetUint64(), 0);
    assert_eq!(empty.ToDatum(UserStr).GetString(), "");
    assert_eq!(empty.ToDatum(DBStr).GetString(), "");
    assert_eq!(empty.ToDatum(AllSQLDigestsStr).GetString(), "[]");
    assert_eq!(empty.ToDatum(RelatedTableIDsStr).GetString(), "");
}

#[test]
/// 验证进入状态计数器与时长直方图按状态/是否持锁选取指标。
fn metric_accessors_select_the_go_state_and_lock_labels() {
    let global_counter =
        metrics::TxnStatusEnteringCounterVec().with_label_values(&["executing_sql"]);
    let counter = TxnStatusEnteringCounter(TxnRunning);
    let before = global_counter.get();
    counter.inc();
    assert_eq!(global_counter.get(), before + 1.0);

    let global_histogram = metrics::TxnDurationHistogramVec().with_label_values(&["idle", "false"]);
    let idle_without_lock = TxnDurationHistogram(TxnIdle, false);
    let before = global_histogram.collect()[0].get_metric()[0]
        .get_histogram()
        .get_sample_count();
    idle_without_lock.observe(0.25);
    let after = global_histogram.collect()[0].get_metric()[0]
        .get_histogram()
        .get_sample_count();
    assert_eq!(after, before + 1);
}

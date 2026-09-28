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

// Aster 迁移补充单测：环缓冲/Clear/Resize/Datum/错误转换与并发 Push。
//
// 相对 Go 原测试，额外覆盖零容量跳过 ID 分配、非法 tag 保留空 digest、
// 多线程 Push 后 ID 集合完整性。死锁指事务互相等待对方持有的锁。

use std::sync::Arc;
use std::thread;

use chrono::{Datelike, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use protobuf::{Message, RepeatedField};

use super::*;

/// 固定日期上按秒偏移构造 OccurTime，便于区分记录。
fn occur_time(second: u32) -> chrono::DateTime<Tz> {
    chrono_tz::UTC
        .with_ymd_and_hms(2021, 5, 14, 15, 28, second)
        .single()
        .unwrap()
        .with_nanosecond(123_456_000)
        .unwrap()
}

/// 空等待链记录，秒字段写入 OccurTime 以区分。
fn record(second: u32) -> Box<DeadlockRecord> {
    Box::new(DeadlockRecord {
        OccurTime: occur_time(second),
        WaitChain: Vec::new(),
        ID: 0,
        IsRetryable: false,
    })
}

/// 断言环缓冲覆盖策略与 Clear 不重置 ID 分配器。
#[test]
fn collection_matches_go_ring_buffer_and_clear_behavior() {
    let history = NewDeadlockHistory(3);
    assert_eq!(history.Len(), 0);
    assert_eq!(history.Head(), 0);

    history.Push(record(1));
    history.Push(record(2));
    history.Push(record(3));
    assert_eq!(
        history
            .GetAll()
            .iter()
            .map(|item| item.ID)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );

    // 继续 Push 覆盖最旧，最终保留 ID 7/8/9。
    for second in 4..=9 {
        history.Push(record(second));
    }
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 3);
    assert_eq!(
        history
            .GetAll()
            .iter()
            .map(|item| item.ID)
            .collect::<Vec<_>>(),
        vec![7, 8, 9]
    );

    history.Clear();
    assert!(history.GetAll().is_empty());
    assert_eq!(history.Head(), 0);

    history.Push(record(10));
    assert_eq!(
        history.GetAll()[0].ID,
        10,
        "Clear must not reset the ID allocator"
    );
}

/// 断言 Resize 保最新；容量 0 时 Push 不分配 ID，恢复后 ID 继续。
#[test]
fn resize_keeps_newest_records_and_zero_capacity_skips_id_allocation() {
    let history = NewDeadlockHistory(2);
    history.Push(record(1));
    history.Push(record(2));
    history.Push(record(3));
    assert_eq!(history.Head(), 1);
    assert_eq!(
        history
            .GetAll()
            .iter()
            .map(|item| item.ID)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );

    history.Resize(3);
    assert_eq!(history.Head(), 0);
    history.Push(record(4));
    assert_eq!(
        history
            .GetAll()
            .iter()
            .map(|item| item.ID)
            .collect::<Vec<_>>(),
        vec![2, 3, 4]
    );

    history.Resize(2);
    assert_eq!(
        history
            .GetAll()
            .iter()
            .map(|item| item.ID)
            .collect::<Vec<_>>(),
        vec![3, 4]
    );

    // 零容量 Push 丢弃且不消耗 ID；扩回后下一 ID 仍为 5。
    history.Resize(0);
    history.Push(record(5));
    assert!(history.GetAll().is_empty());
    history.Resize(1);
    history.Push(record(6));
    assert_eq!(history.GetAll()[0].ID, 5);
}

/// 断言 ToDatum 列值、空字段 NULL、未知列名 NULL。
#[test]
fn datum_conversion_matches_go_columns_and_nulls() {
    let time = occur_time(30);
    let record = DeadlockRecord {
        OccurTime: time,
        WaitChain: vec![
            WaitChainItem {
                SQLDigest: "sql1".to_owned(),
                Key: b"k1".to_vec(),
                AllSQLDigests: vec!["sql1".to_owned(), "sql2".to_owned()],
                TryLockTxn: 101,
                TxnHoldingLock: 102,
            },
            WaitChainItem {
                SQLDigest: String::new(),
                Key: Vec::new(),
                AllSQLDigests: Vec::new(),
                TryLockTxn: 102,
                TxnHoldingLock: 101,
            },
        ],
        ID: 7,
        IsRetryable: true,
    };

    assert_eq!(record.ToDatum(0, ColDeadlockIDStr).GetUint64(), 7);
    assert_eq!(record.ToDatum(0, ColRetryableStr).GetInt64(), 1);
    assert_eq!(record.ToDatum(0, ColTryLockTrxIDStr).GetUint64(), 101);
    assert_eq!(
        record.ToDatum(0, ColCurrentSQLDigestStr).GetString(),
        "sql1"
    );
    assert_eq!(record.ToDatum(0, ColKeyStr).GetString(), "6B31");
    assert_eq!(record.ToDatum(0, ColTrxHoldingLockStr).GetUint64(), 102);
    assert_eq!(
        record.ToDatum(0, ColOccurTimeStr).GetMysqlTime(),
        types::NewTime(
            types::FromDate(
                time.year(),
                time.month() as i32,
                time.day() as i32,
                time.hour() as i32,
                time.minute() as i32,
                time.second() as i32,
                time.nanosecond() as i32 / 1_000,
            ),
            mysql::r#type::TypeTimestamp,
            types::MaxFsp,
        )
    );

    assert_eq!(
        record.ToDatum(1, ColCurrentSQLDigestStr).Kind(),
        types::KindNull
    );
    assert_eq!(record.ToDatum(1, ColKeyStr).Kind(), types::KindNull);
    assert_eq!(
        record.ToDatum(0, ColCurrentSQLDigestTextStr).Kind(),
        types::KindNull
    );
    assert_eq!(record.ToDatum(0, ColKeyInfoStr).Kind(), types::KindNull);
    assert_eq!(record.ToDatum(0, "UNKNOWN").Kind(), types::KindNull);
}

/// 构造 wait_chain 中的 WaitForEntry。
fn wait_for_entry(
    txn: u64,
    wait_for_txn: u64,
    key: &[u8],
    tag: Vec<u8>,
) -> resourcegrouptag::kvproto::deadlock::WaitForEntry {
    let mut entry = resourcegrouptag::kvproto::deadlock::WaitForEntry::new();
    entry.set_txn(txn);
    entry.set_wait_for_txn(wait_for_txn);
    entry.set_key(key.to_vec());
    entry.set_resource_group_tag(tag);
    entry
}

/// 验证合法 tag 解码 digest，非法 tag 保留空 digest 且不丢弃等待项。
#[test]
fn err_deadlock_conversion_decodes_digest_and_keeps_invalid_entries() {
    let digest = b"aabbccdd".to_vec();
    let mut tag = resourcegrouptag::tipb::ResourceGroupTag::new();
    tag.set_sql_digest(digest.clone());

    let mut deadlock = resourcegrouptag::kvproto::kvrpcpb::Deadlock::new();
    deadlock.set_wait_chain(RepeatedField::from_vec(vec![
        wait_for_entry(100, 101, b"k2", tag.write_to_bytes().unwrap()),
        wait_for_entry(101, 100, b"k1", vec![0xff]),
    ]));
    let before = Utc::now();
    let converted = ErrDeadlockToDeadlockRecord(&ErrDeadlock {
        Deadlock: deadlock,
        IsRetryable: true,
    });

    assert!(converted.IsRetryable);
    assert_eq!(converted.WaitChain.len(), 2);
    assert_eq!(converted.WaitChain[0].TryLockTxn, 100);
    assert_eq!(converted.WaitChain[0].TxnHoldingLock, 101);
    assert_eq!(converted.WaitChain[0].Key, b"k2");
    assert_eq!(converted.WaitChain[0].SQLDigest, hex::encode(digest));
    assert!(converted.WaitChain[1].SQLDigest.is_empty());
    assert!(converted.OccurTime.with_timezone(&Utc) >= before);
}

/// 多线程并发 Push，断言最终 ID 集合为 1..=容量（线程安全）。
#[test]
fn public_apis_are_thread_safe() {
    let history = Arc::new(NewDeadlockHistory(100));
    let threads = (0..4)
        .map(|worker| {
            let history = Arc::clone(&history);
            thread::spawn(move || {
                for offset in 0..25 {
                    history.Push(record((worker * 25 + offset) % 60));
                }
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }

    let mut ids = history
        .GetAll()
        .iter()
        .map(|item| item.ID)
        .collect::<Vec<_>>();
    ids.sort_unstable();
    assert_eq!(ids, (1..=100).collect::<Vec<_>>());
}

/// Go `uint` follows the target pointer width; the Rust API must accept `usize`
/// rather than narrowing capacities to `u32`.
#[test]
fn history_capacity_uses_platform_uint_width() {
    let capacity: usize = 2;
    let history = NewDeadlockHistory(capacity);
    assert_eq!(history.Capacity(), capacity);
    history.Resize(capacity + 1);
    assert_eq!(history.Capacity(), capacity + 1);
}

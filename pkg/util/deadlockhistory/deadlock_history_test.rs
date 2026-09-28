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

// 死锁历史环形缓冲、Datum 投影与 TiKV 错误转换的 Go 对齐单元测试。
//
// 覆盖 Push/Clear 覆盖策略、INFORMATION_SCHEMA 列 Datum、resource group tag
// 解码，以及 Resize 保最新记录。死锁指事务互相等待对方持有的锁。

use std::sync::Arc;

use chrono::{Duration, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use deadlockhistory::*;
use meta_model::group_1::{ColumnInfo, ast};
use protobuf::{Message, RepeatedField};

/// 调用公共 setup，并断言只执行一次。
fn setup() {
    crate::main_test::setup_for_common_test();
    assert_eq!(crate::main_test::setup_count(), 1);
}

/// 构造固定 UTC 时间点，便于 Datum/时间断言可复现。
fn fixed_time(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    nanos: u32,
) -> chrono::DateTime<Tz> {
    chrono_tz::UTC
        .with_ymd_and_hms(year, month, day, hour, minute, second)
        .single()
        .unwrap()
        .with_nanosecond(nanos)
        .unwrap()
}

/// 构造 OccurTime=now、空等待链的占位死锁记录。
fn now_record() -> Box<DeadlockRecord> {
    Box::new(DeadlockRecord {
        OccurTime: Utc::now().with_timezone(&chrono_tz::UTC),
        WaitChain: Vec::new(),
        ID: 0,
        IsRetryable: false,
    })
}

/// 将历史中每条等待链项投影为 INFORMATION_SCHEMA 风格的 Datum 行。
fn get_all_datum(history: &DeadlockHistory, columns: &[ColumnInfo]) -> Vec<Vec<types::Datum>> {
    let records = history.GetAll();
    let row_count = records.iter().map(|record| record.WaitChain.len()).sum();
    let mut rows = Vec::with_capacity(row_count);
    for record in records {
        for wait_chain_index in 0..record.WaitChain.len() {
            rows.push(
                columns
                    .iter()
                    .map(|column| record.ToDatum(wait_chain_index, &column.Name.O))
                    .collect(),
            );
        }
    }
    rows
}

/// 按列名构造最小 ColumnInfo，供 ToDatum 按 Name.O 匹配。
fn column(name: &str) -> ColumnInfo {
    ColumnInfo::New(0, ast::NewCIStr(name))
}

/// 验证容量 1/3 下 Push 覆盖、Arc 身份、ID 递增与 Clear 清空。
#[test]
fn test_deadlock_history_collection() {
    setup();

    let history = NewDeadlockHistory(1);
    assert!(history.GetAll().is_empty());
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 0);

    history.Push(now_record());
    let mut result = history.GetAll();
    assert_eq!(result.len(), 1);
    let stored_record1 = Arc::clone(&result[0]);
    assert!(Arc::ptr_eq(&history.GetAll()[0], &stored_record1));
    assert_eq!(result[0].ID, 1);
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 1);

    // 容量 1：第二次 Push 覆盖第一条，ID 变为 2。
    history.Push(now_record());
    result = history.GetAll();
    assert_eq!(result.len(), 1);
    let stored_record2 = Arc::clone(&result[0]);
    assert!(Arc::ptr_eq(&history.GetAll()[0], &stored_record2));
    assert!(!Arc::ptr_eq(&stored_record1, &stored_record2));
    assert_eq!(result[0].ID, 2);
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 1);

    history.Clear();
    assert!(history.GetAll().is_empty());

    // 容量 3：填满后继续 Push，head 环移，始终保留最新 3 条。
    let history = NewDeadlockHistory(3);
    history.Push(now_record());
    history.Push(now_record());
    history.Push(now_record());
    result = history.GetAll();
    let mut expected_items = result.clone();
    assert_eq!(result.len(), 3);
    assert_eq!(
        result.iter().map(|item| item.ID).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 3);

    let mut expected_ids = vec![1_u64, 2, 3];
    let mut expected_head = 0;
    for _ in 0..6 {
        history.Push(now_record());
        result = history.GetAll();
        expected_items.remove(0);
        expected_items.push(Arc::clone(result.last().unwrap()));
        for id in &mut expected_ids {
            *id += 1;
        }
        expected_head = (expected_head + 1) % 3;

        result = history.GetAll();
        assert_eq!(result.len(), 3);
        for (index, item) in result.iter().enumerate() {
            assert!(Arc::ptr_eq(item, &expected_items[index]));
            assert_eq!(item.ID, expected_ids[index]);
        }
        assert_eq!(history.Head(), expected_head);
        assert_eq!(history.Len(), 3);
    }

    history.Clear();
    assert!(history.GetAll().is_empty());
}

/// 验证 ToDatum 对各列（含空 digest/空键 → NULL、键十六进制）投影正确。
#[test]
fn test_get_datum() {
    setup();

    let time1 = fixed_time(2021, 5, 14, 15, 28, 30, 123_456_000);
    let time2 = fixed_time(2022, 6, 15, 16, 29, 31, 123_457_000);
    let history = NewDeadlockHistory(10);
    history.Push(Box::new(DeadlockRecord {
        OccurTime: time1,
        IsRetryable: false,
        WaitChain: vec![
            WaitChainItem {
                TryLockTxn: 101,
                SQLDigest: "sql1".to_owned(),
                Key: b"k1".to_vec(),
                AllSQLDigests: vec!["sql1".to_owned(), "sql2".to_owned()],
                TxnHoldingLock: 102,
            },
            WaitChainItem {
                TryLockTxn: 102,
                SQLDigest: String::new(),
                Key: Vec::new(),
                AllSQLDigests: Vec::new(),
                TxnHoldingLock: 101,
            },
        ],
        ID: 0,
    }));
    history.Push(Box::new(DeadlockRecord {
        OccurTime: time2,
        IsRetryable: true,
        WaitChain: vec![
            WaitChainItem {
                TryLockTxn: 201,
                SQLDigest: String::new(),
                Key: Vec::new(),
                AllSQLDigests: Vec::new(),
                TxnHoldingLock: 202,
            },
            WaitChainItem {
                TryLockTxn: 202,
                SQLDigest: String::new(),
                Key: Vec::new(),
                AllSQLDigests: vec!["sql1".to_owned()],
                TxnHoldingLock: 201,
            },
        ],
        ID: 0,
    }));
    // 空等待链的记录不产生 Datum 行。
    history.Push(Box::new(DeadlockRecord {
        OccurTime: Utc::now().with_timezone(&chrono_tz::UTC),
        IsRetryable: false,
        WaitChain: Vec::new(),
        ID: 0,
    }));

    let columns = [
        column(ColDeadlockIDStr),
        column(ColOccurTimeStr),
        column(ColRetryableStr),
        column(ColTryLockTrxIDStr),
        column(ColCurrentSQLDigestStr),
        column(ColCurrentSQLDigestTextStr),
        column(ColKeyStr),
        column(ColKeyInfoStr),
        column(ColTrxHoldingLockStr),
    ];
    let result = get_all_datum(&history, &columns);

    assert_eq!(result.len(), 4);
    assert!(result.iter().all(|row| row.len() == 9));
    let to_go_time = |datum: &types::Datum| datum.GetMysqlTime().GoTime(chrono_tz::UTC).unwrap();

    assert_eq!(result[0][0].GetUint64(), 1);
    assert_eq!(to_go_time(&result[0][1]), time1);
    assert_eq!(result[0][2].GetInt64(), 0);
    assert_eq!(result[0][3].GetUint64(), 101);
    assert_eq!(result[0][4].GetString(), "sql1");
    assert_eq!(result[0][5].Kind(), types::KindNull);
    assert_eq!(result[0][6].GetString(), "6B31");
    assert_eq!(result[0][8].GetUint64(), 102);

    assert_eq!(result[1][0].GetUint64(), 1);
    assert_eq!(to_go_time(&result[1][1]), time1);
    assert_eq!(result[1][2].GetInt64(), 0);
    assert_eq!(result[1][3].GetUint64(), 102);
    assert_eq!(result[1][4].Kind(), types::KindNull);
    assert_eq!(result[1][5].Kind(), types::KindNull);
    assert_eq!(result[1][6].Kind(), types::KindNull);
    assert_eq!(result[1][8].GetUint64(), 101);

    assert_eq!(result[2][0].GetUint64(), 2);
    assert_eq!(to_go_time(&result[2][1]), time2);
    assert_eq!(result[2][2].GetInt64(), 1);
    assert_eq!(result[2][3].GetUint64(), 201);
    assert_eq!(result[2][8].GetUint64(), 202);

    assert_eq!(result[3][0].GetUint64(), 2);
    assert_eq!(to_go_time(&result[3][1]), time2);
    assert_eq!(result[3][2].GetInt64(), 1);
    assert_eq!(result[3][3].GetUint64(), 202);
    assert_eq!(result[3][8].GetUint64(), 201);
}

/// 构造 wait_chain 中的一项 WaitForEntry（txn / wait_for_txn / key / tag）。
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

/// 验证 ErrDeadlockToDeadlockRecord 解码 digest、保留键与可重试标志。
#[test]
fn test_err_deadlock_to_deadlock_record() {
    setup();

    let digest1 = parser::digester_impl::NewDigest(b"aabbccdd".to_vec());
    let digest2 = parser::digester_impl::NewDigest(b"ddccbbaa".to_vec());
    let mut tag1 = resourcegrouptag::tipb::ResourceGroupTag::new();
    let mut tag2 = resourcegrouptag::tipb::ResourceGroupTag::new();
    tag1.set_sql_digest(digest1.Bytes().to_vec());
    tag2.set_sql_digest(digest2.Bytes().to_vec());

    let mut deadlock = resourcegrouptag::kvproto::kvrpcpb::Deadlock::new();
    deadlock.set_lock_ts(101);
    deadlock.set_lock_key(b"k1".to_vec());
    deadlock.set_deadlock_key_hash(1_234_567);
    deadlock.set_wait_chain(RepeatedField::from_vec(vec![
        wait_for_entry(100, 101, b"k2", tag1.write_to_bytes().unwrap()),
        wait_for_entry(101, 100, b"k1", tag2.write_to_bytes().unwrap()),
    ]));
    let error = ErrDeadlock {
        Deadlock: deadlock,
        IsRetryable: true,
    };

    let mut expected = DeadlockRecord {
        OccurTime: fixed_time(1970, 1, 1, 0, 0, 0, 0),
        WaitChain: vec![
            WaitChainItem {
                TryLockTxn: 100,
                SQLDigest: digest1.String().to_owned(),
                Key: b"k2".to_vec(),
                AllSQLDigests: Vec::new(),
                TxnHoldingLock: 101,
            },
            WaitChainItem {
                TryLockTxn: 101,
                SQLDigest: digest2.String().to_owned(),
                Key: b"k1".to_vec(),
                AllSQLDigests: Vec::new(),
                TxnHoldingLock: 100,
            },
        ],
        ID: 0,
        IsRetryable: true,
    };

    let record = ErrDeadlockToDeadlockRecord(&error);
    // OccurTime 取转换时刻，允许与 now 有数毫秒偏差。
    assert!(
        Utc::now().signed_duration_since(record.OccurTime.with_timezone(&Utc))
            < Duration::milliseconds(5)
    );
    expected.OccurTime = record.OccurTime;
    assert_eq!(record, expected);
}

/// 固定时间的空等待链记录，供 Resize 用例使用。
fn dummy_record() -> Box<DeadlockRecord> {
    Box::new(DeadlockRecord {
        OccurTime: fixed_time(1970, 1, 1, 0, 0, 0, 0),
        WaitChain: Vec::new(),
        ID: 0,
        IsRetryable: false,
    })
}

/// 验证 Resize 扩/缩/清零容量时保留最新记录且 ID 分配器连续。
#[test]
fn test_resize() {
    setup();

    let history = NewDeadlockHistory(2);
    history.Push(dummy_record());
    history.Push(dummy_record());
    history.Push(dummy_record());
    assert_eq!(history.Head(), 1);
    assert_eq!(history.Len(), 2);
    assert_eq!(history.GetAll().len(), 2);
    assert_eq!(history.GetAll()[0].ID, 2);
    assert_eq!(history.GetAll()[1].ID, 3);

    history.Resize(3);
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 2);
    history.Push(dummy_record());
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 3);
    assert_eq!(history.GetAll().len(), 3);
    assert_eq!(history.GetAll()[0].ID, 2);
    assert_eq!(history.GetAll()[1].ID, 3);
    assert_eq!(history.GetAll()[2].ID, 4);

    history.Resize(2);
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 2);
    assert_eq!(history.GetAll().len(), 2);
    assert_eq!(history.GetAll()[0].ID, 3);
    assert_eq!(history.GetAll()[1].ID, 4);

    history.Resize(0);
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 0);
    assert!(history.GetAll().is_empty());

    history.Resize(2);
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 0);
    history.Push(dummy_record());
    assert_eq!(history.Head(), 0);
    assert_eq!(history.Len(), 1);
}

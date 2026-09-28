// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// `TxStructure` 字符串、列表与哈希在可写事务与只读快照上的行为单测。

use super::migration_aster_unit_test::writable;
use super::{
    ErrInvalidHashKeyFlag, ErrInvalidListIndex, ErrInvalidListMetaData, ErrWriteOnSnapshot,
    NewHashReverseIter, NewHashReverseIterBeginWithField, mysql,
};
use dbterror_dependency::terror;

/// 覆盖缺失键返回 0、Inc 自增/自减以及 Get 读回十进制字节。
#[test]
fn TestStringIntegerAndMissingValues() {
    let (mut tx, _) = writable(&[7]);
    assert_eq!(0, tx.GetInt64(b"missing").unwrap());
    assert_eq!(10, tx.Inc(b"counter", 10).unwrap());
    assert_eq!(7, tx.Inc(b"counter", -3).unwrap());
    assert_eq!(Some(b"7".to_vec()), tx.Get(b"counter").unwrap());
}

/// 只读快照上的 List/Hash 写操作必须失败并报告 `ErrWriteOnSnapshot`。
#[test]
fn TestListAndHashSnapshotWritesFail() {
    let (tx, store) = writable(&[8]);
    // 复用同一 store 构造无 Mutator 的快照结构。
    let mut snapshot = super::NewStructure(Box::new(store), None, vec![8]);

    let list_error = snapshot
        .LPush(b"list", &[b"value".to_vec()])
        .expect_err("snapshot list write must fail");
    assert!(ErrWriteOnSnapshot.Equal(Some(&list_error)));

    let hash_error = snapshot
        .HInc(b"hash", b"field", 1)
        .expect_err("snapshot hash write must fail");
    assert!(ErrWriteOnSnapshot.Equal(Some(&hash_error)));

    assert_eq!(None, tx.Get(b"missing").unwrap());
}

/// 空值等价于缺失，以及哈希反向迭代器与带起点字段的迭代。
#[test]
fn TestHashNilEquivalentAndReverseIterator() {
    let (mut tx, _) = writable(&[9]);
    // 空字节值按协议视为不存在（与 Go nil 语义对齐）。
    tx.HSet(b"hash", b"empty", b"").unwrap();
    assert_eq!(None, tx.HGet(b"hash", b"empty").unwrap());

    tx.HSet(b"hash", b"a", b"1").unwrap();
    tx.HSet(b"hash", b"b", b"2").unwrap();
    tx.HSet(b"hash", b"c", b"3").unwrap();

    let mut iterator = NewHashReverseIter(&tx, b"hash").unwrap();
    assert!(iterator.Valid());
    assert_eq!(b"3".to_vec(), iterator.Value());
    iterator.Next().unwrap();
    assert_eq!(b"b", iterator.Key());
    assert_eq!(b"2".to_vec(), iterator.Value());
    iterator.Close();

    // 从字段 "b" 开始的反向迭代应先看到 b 再落到 a。
    let mut bounded = NewHashReverseIterBeginWithField(&tx, b"hash", b"b").unwrap();
    assert!(bounded.Valid());
    assert_eq!(b"2".to_vec(), bounded.Value());
    bounded.Next().unwrap();
    assert_eq!(b"a", bounded.Key());
}

/// 所有 structure 错误都必须保留注册的 MySQL 错误码。
#[test]
fn TestError() {
    for error in [
        &*ErrInvalidHashKeyFlag,
        &*ErrInvalidListIndex,
        &*ErrInvalidListMetaData,
        &*ErrWriteOnSnapshot,
    ] {
        let sql_error = terror::ToSQLError(error);
        assert_ne!(sql_error.Code, mysql::ErrUnknown);
        assert_eq!(sql_error.Code, error.Code() as u16);
    }
}

// Copyright 2019-present PingCAP, Inc.
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

// MVCC 编解码与快照从 Go 迁移到 Rust 的行为对齐单测。
//
// 覆盖锁二进制布局往返、LockInfo 字段映射、用户元/额外事务状态键、
// Write CF / Lock CF 编码以及 DBSnapshot 共享锁存储语义。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::kvproto::kvrpcpb::Op;
use crate::{codec, lockstore};

use super::db_writer::{DBBundle, DBSnapshotSource, NewDBSnapshot};
use super::mvcc::{
    DecodeExtraTxnStatusKey, DecodeKeyTS, DecodeLock, EncodeExtraTxnStatusKey, Lock, LockHdr,
    LockUserMetaDelete, LockUserMetaNone, NewDBUserMeta, mvccLockHdrSize,
};
use super::tikv::{
    EncodeLockCFValue, EncodeWriteCFValue, LockTypeDelete, LockTypeLock, LockTypePessimistic,
    LockTypePut, ParseWriteCFValue, ShortValueMaxLen, WriteTypeDelete, WriteTypeLock, WriteTypePut,
    WriteTypeRollback,
};

/// 构造带完整可变字段的样例锁，便于编解码断言。
fn sample_lock() -> Lock {
    Lock {
        LockHdr: LockHdr {
            StartTS: 0x0102_0304_0506_0708,
            ForUpdateTS: 11,
            MinCommitTS: 13,
            TTL: 17,
            Op: Op::Put as u8,
            HasOldVer: true,
            PrimaryLen: 2,
            UseAsyncCommit: true,
            SecondaryNum: 2,
        },
        Primary: b"pk".to_vec(),
        Value: b"value".to_vec(),
        Secondaries: vec![b"s1".to_vec(), b"secondary-2".to_vec()],
    }
}

#[test]
/// 验证 Lock 二进制布局与 Go 一致，且可变字段为拷贝而非借用。
fn lock_binary_round_trip_matches_go_layout_and_copies_variable_fields() {
    let lock = sample_lock();
    // 固定头 40 字节后跟 Primary/Secondaries/Value，字段小端布局对齐 Go。
    let encoded = lock.MarshalBinary();

    assert_eq!(mvccLockHdrSize, 40);
    assert_eq!(&encoded[0..8], &lock.LockHdr.StartTS.to_le_bytes());
    assert_eq!(&encoded[24..28], &lock.LockHdr.TTL.to_le_bytes());
    assert_eq!(encoded[28], Op::Put as u8);
    assert_eq!(encoded[29], 1);
    assert_eq!(&encoded[30..32], &2_u16.to_le_bytes());
    assert_eq!(encoded[32], 1);
    assert_eq!(&encoded[33..36], &[0, 0, 0]);
    assert_eq!(&encoded[36..40], &2_u32.to_le_bytes());

    let mut decoded = DecodeLock(&encoded);
    assert_eq!(decoded.LockHdr.StartTS, lock.LockHdr.StartTS);
    assert_eq!(decoded.LockHdr.ForUpdateTS, 11);
    assert_eq!(decoded.LockHdr.MinCommitTS, 13);
    assert_eq!(decoded.LockHdr.TTL, 17);
    assert!(decoded.LockHdr.HasOldVer);
    assert!(decoded.LockHdr.UseAsyncCommit);
    assert_eq!(decoded.Primary, b"pk");
    assert_eq!(decoded.Secondaries, lock.Secondaries);
    assert_eq!(decoded.Value, b"value");

    // 修改解码结果不应回写到原始编码缓冲。
    decoded.Primary[0] = b'X';
    assert_eq!(encoded[mvccLockHdrSize], b'p');
}

#[test]
/// 验证 ToLockInfo / String 字段映射与 Go 输出一致。
fn lock_info_and_string_keep_go_field_mapping() {
    let lock = sample_lock();
    let info = lock.ToLockInfo(b"row-key");

    assert_eq!(info.primary_lock, b"pk");
    assert_eq!(info.lock_version, lock.LockHdr.StartTS);
    assert_eq!(info.key, b"row-key");
    assert_eq!(info.lock_ttl, 17);
    assert_eq!(info.lock_type, Op::Put);
    assert_eq!(info.lock_for_update_ts, 11);
    assert!(info.use_async_commit);
    assert_eq!(info.min_commit_ts, 13);
    assert_eq!(
        info.secondaries.into_vec(),
        vec![b"s1".to_vec(), b"secondary-2".to_vec()]
    );
    assert_eq!(
        lock.String(),
        "Lock { Type: Put, StartTS: 72623859790382856,  ForUpdateTS: 11, Primary: 706b, UseAsyncCommit: true }"
    );
}

#[test]
/// 验证 DBUserMeta、键时间戳解码与额外事务状态键编解码。
fn user_meta_key_timestamp_and_extra_status_key_match_go() {
    let meta = NewDBUserMeta(0x0102_0304_0506_0708, 0x1112_1314_1516_1718);
    assert_eq!(&meta.0[..8], &0x0102_0304_0506_0708_u64.to_le_bytes());
    assert_eq!(meta.StartTS(), 0x0102_0304_0506_0708);
    assert_eq!(meta.CommitTS(), 0x1112_1314_1516_1718);
    assert_eq!(LockUserMetaNone, &[0]);
    assert_eq!(LockUserMetaDelete, &[2]);

    let encoded_key = codec::EncodeUintDesc(b"key".to_vec(), 42);
    assert_eq!(DecodeKeyTS(&encoded_key), 42);

    let extra = EncodeExtraTxnStatusKey(b"key", 99);
    assert_eq!(extra[0], b'k' + 1);
    assert_eq!(DecodeExtraTxnStatusKey(&extra), Some(b"key".to_vec()));
    assert_eq!(DecodeKeyTS(&extra), 99);
    assert!(DecodeExtraTxnStatusKey(b"123456789").is_none());
}

#[test]
/// 验证 Write CF 各类型往返，并拒绝非法字节序列。
fn write_cf_round_trips_all_go_types_and_rejects_invalid_data() {
    for write_type in [
        WriteTypePut,
        WriteTypeDelete,
        WriteTypeLock,
        WriteTypeRollback,
    ] {
        let encoded = EncodeWriteCFValue(write_type, 300, b"abc");
        let decoded = ParseWriteCFValue(&encoded).unwrap();
        assert_eq!(decoded.Type, write_type);
        assert_eq!(decoded.StartTS, 300);
        assert_eq!(decoded.ShortVal, b"v\x03abc");
    }

    assert!(ParseWriteCFValue(&[]).is_err());
    assert!(ParseWriteCFValue(b"X\x01").is_err());
    assert!(ParseWriteCFValue(b"P\x80").is_err());
}

#[test]
/// 验证 Lock CF 短值内联与长值分离编码符合 TiKV 格式。
fn lock_cf_short_and_long_values_match_tikv_encoding() {
    assert_eq!(
        [
            LockTypePut,
            LockTypeDelete,
            LockTypeLock,
            LockTypePessimistic
        ],
        [b'P', b'D', b'L', b'S']
    );

    let mut lock = sample_lock();
    lock.LockHdr.StartTS = 7;
    lock.LockHdr.TTL = 9;
    lock.Value = b"abc".to_vec();
    let (encoded, long_value) = EncodeLockCFValue(&lock);
    assert_eq!(
        encoded,
        vec![
            b'P', 4, b'p', b'k', 7, 9, b'v', 3, b'a', b'b', b'c', b'f', 0, 0, 0, 0, 0, 0, 0, 11,
            b'm', 0, 0, 0, 0, 0, 0, 0, 13,
        ]
    );
    assert!(long_value.is_empty());

    lock.LockHdr.Op = Op::Del as u8;
    lock.LockHdr.ForUpdateTS = 0;
    lock.LockHdr.MinCommitTS = 0;
    lock.Value = vec![b'x'; ShortValueMaxLen + 1];
    let (encoded, long_value) = EncodeLockCFValue(&lock);
    assert_eq!(encoded, vec![b'D', 4, b'p', b'k', 7, 9]);
    assert_eq!(long_value, vec![b'x'; ShortValueMaxLen + 1]);
}

#[derive(Clone)]
/// 计数型假数据库：每次 NewReadSnapshot 递增，用于断言快照创建次数。
struct CountingDb(Arc<AtomicUsize>);

impl DBSnapshotSource for CountingDb {
    type Snapshot = usize;

    fn NewReadSnapshot(&self) -> Self::Snapshot {
        self.0.fetch_add(1, Ordering::SeqCst) + 1
    }
}

#[test]
/// 验证 NewDBSnapshot 创建独立读视图且共享同一 LockStore。
fn db_snapshot_creates_read_view_and_shares_lock_store() {
    let counter = Arc::new(AtomicUsize::new(0));
    let lock_store: Arc<lockstore::MemStore> = Arc::from(lockstore::MemStore::NewMemStore(256));
    let bundle = DBBundle {
        DB: CountingDb(Arc::clone(&counter)),
        LockStore: Arc::clone(&lock_store),
        MemStoreMu: std::sync::Mutex::new(()),
        StateTS: 12,
    };

    let first = NewDBSnapshot(&bundle);
    let second = NewDBSnapshot(&bundle);
    assert_eq!(first.Txn, 1);
    assert_eq!(second.Txn, 2);
    assert!(Arc::ptr_eq(&first.LockStore, &lock_store));
    assert!(Arc::ptr_eq(&second.LockStore, &lock_store));
}

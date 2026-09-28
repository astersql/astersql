// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// kverrors 从 Go 迁移的错误格式对齐单测。
//
// 校验锁错误、可重试错误、死锁/写冲突等消息字符串与十六进制
// 格式化与 Go 侧一致，避免迁移后客户端解析行为漂移。

use super::deadlockpb::WaitForEntry;
use super::kverrors::*;
use super::kvrpcpb::{Assertion, Op, WriteConflictReason};
use super::mvcc::{Lock, LockHdr};

/// 构造带典型字段的样例 MVCC Lock，供错误格式化断言使用。
fn sample_lock() -> Lock {
    Lock {
        LockHdr: LockHdr {
            StartTS: 42,
            ForUpdateTS: 43,
            MinCommitTS: 44,
            TTL: 3000,
            Op: Op::Put as u8,
            HasOldVer: true,
            PrimaryLen: 7,
            UseAsyncCommit: true,
            SecondaryNum: 0,
        },
        Primary: b"primary".to_vec(),
        Value: b"value".to_vec(),
        Secondaries: Vec::new(),
    }
}

/// 断言 `std::error::Error` 的 Display 文本与期望一致。
fn assert_std_error(error: &(dyn std::error::Error + 'static), expected: &str) {
    assert_eq!(error.to_string(), expected);
}

#[test]
/// ErrLocked 保留 Key/Lock 载荷，且 Error() 与 Go hex 格式一致。
fn locked_error_preserves_payload_and_matches_go_format() {
    let key = vec![0x00, 0xab, 0xff];
    let lock = sample_lock();
    let error = BuildLockErr(key.clone(), Box::new(lock.clone()));

    assert_eq!(error.Key, key);
    assert_eq!(*error.Lock, lock);
    assert_eq!(
        error.Error(),
        "key is locked, key: 00abff, lock: Lock { Type: Put, StartTS: 42,  ForUpdateTS: 43, Primary: 7072696d617279, UseAsyncCommit: true }"
    );
    assert_std_error(error.as_ref(), &error.Error());
}

#[test]
/// 可重试错误与简单错误的固定消息对齐 Go。
fn retryable_and_simple_errors_match_go_strings() {
    let custom = ErrRetryable::from("write conflict while acquiring lock");
    assert_eq!(
        custom.Error(),
        "retryable: write conflict while acquiring lock"
    );
    assert_eq!(ErrLockNotFound.Error(), "retryable: lock not found");
    assert_eq!(ErrAlreadyRollback.Error(), "retryable: already rollback");
    assert_eq!(
        ErrReplaced.Error(),
        "retryable: replaced by another transaction"
    );

    let invalid = ErrInvalidOp { Op: Op::Del };
    assert_eq!(invalid.Error(), "invalid op: Del");
    assert_std_error(&invalid, "invalid op: Del");

    let committed = ErrAlreadyCommitted(123);
    assert_eq!(committed.0, 123);
    assert_std_error(&committed, "txn already committed");

    let exists = ErrKeyAlreadyExists {
        Key: b"row-key".to_vec(),
    };
    assert_eq!(exists.Key, b"row-key");
    assert_std_error(&exists, "key already exists");
}

#[test]
/// 死锁/写冲突/提交过期/事务未找到的载荷与固定文案。
fn transaction_error_payloads_and_fixed_messages_match_go() {
    let deadlock = ErrDeadlock {
        LockKey: b"locked-key".to_vec(),
        LockTS: 11,
        DeadlockKeyHash: 12,
        WaitChain: vec![WaitForEntry {
            txn: 11,
            wait_for_txn: 13,
            key_hash: 12,
            key: b"locked-key".to_vec(),
            ..Default::default()
        }],
    };
    assert_eq!(deadlock.WaitChain[0].wait_for_txn, 13);
    assert_std_error(&deadlock, "deadlock");

    let conflict = ErrConflict {
        StartTS: 20,
        ConflictTS: 21,
        ConflictCommitTS: 22,
        Key: b"conflict-key".to_vec(),
        Reason: WriteConflictReason::RcCheckTs,
    };
    assert_eq!(conflict.Reason, WriteConflictReason::RcCheckTs);
    assert_std_error(&conflict, "write conflict");

    let expired = ErrCommitExpire {
        StartTs: 30,
        CommitTs: 31,
        MinCommitTs: 32,
        Key: b"commit-key".to_vec(),
    };
    assert_eq!(expired.MinCommitTs, 32);
    assert_std_error(&expired, "commit expired");

    let missing = ErrTxnNotFound {
        StartTS: 40,
        PrimaryKey: b"primary-key".to_vec(),
    };
    assert_eq!(missing.PrimaryKey, b"primary-key");
    assert_std_error(&missing, "txn not found");
}

#[test]
/// AssertionFailed / PrimaryMismatch 的详细格式化对齐 Go。
fn detailed_errors_match_go_hex_and_enum_formatting() {
    let assertion = ErrAssertionFailed {
        StartTS: 7,
        Key: vec![0x00, 0xff],
        Assertion: Assertion::Exist,
        ExistingStartTS: 8,
        ExistingCommitTS: 9,
    };
    assert_eq!(
        assertion.Error(),
        "AssertionFailed { StartTS: 7, Key: 00ff, Assertion: Exist, ExistingStartTS: 8, ExistingCommitTS: 9 }"
    );
    assert_std_error(&assertion, &assertion.Error());

    let mismatch = ErrPrimaryMismatch {
        Key: vec![0x12, 0x34],
        Lock: Box::new(sample_lock()),
    };
    assert_eq!(
        mismatch.Error(),
        "primary mismatch, key: 1234, lock: Lock { Type: Put, StartTS: 42,  ForUpdateTS: 43, Primary: 7072696d617279, UseAsyncCommit: true }"
    );
    assert_std_error(&mismatch, &mismatch.Error());
}

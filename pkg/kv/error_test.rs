// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// kv::error 预定义错误的 SQL 错误码一致性测试。

use kv_dependency as kv;

#[test]
fn test_not_found_accepts_nil_like_go() {
    assert!(!kv::IsErrNotFound(None));
    let missing = kv::ErrNotExist.FastGenByArgs(&[]);
    assert!(kv::IsErrNotFound(&missing));
    assert!(kv::IsErrNotFound(Some(&missing)));
    let invalid = kv::ErrInvalidTxn.FastGenByArgs(&[]);
    assert!(!kv::IsErrNotFound(&invalid));
}

#[test]
fn test_retryable_errors_match_go() {
    assert!(!kv::IsTxnRetryableError(None));
    for (prototype, retryable) in [
        (&*kv::ErrTxnRetryable, true),
        (&*kv::ErrWriteConflict, true),
        (&*kv::ErrWriteConflictInTiDB, true),
        (&*kv::ErrNotExist, false),
        (&*kv::ErrInvalidTxn, false),
        (&*kv::ErrLockExpire, false),
        (&*kv::ErrAssertionFailed, false),
    ] {
        let error = prototype.FastGenByArgs(&[]);
        assert_eq!(kv::IsTxnRetryableError(Some(&error)), retryable);
    }
}

#[test]
fn test_key_exists_join_matches_go() {
    for (columns, joined) in [
        (vec![], ""),
        (vec![""], ""),
        (vec!["a", "", "雪-b"], "a--雪-b"),
    ] {
        let columns = columns.into_iter().map(String::from).collect::<Vec<_>>();
        let error = kv::GenKeyExistsErr(&columns, "idx");
        let expected = kv::ErrKeyExists.FastGenByArgs(&[joined.into(), "idx".into()]);
        assert_eq!(error.to_string(), expected.to_string());
        assert!(kv::ErrKeyExists.Equal(Some(&error)));
    }
}

/// 校验各 KV 错误原型均可映射为非 Unknown 的 SQL 错误码，且与内部 Code 一致。
#[test]
fn test_error() {
    let errors = [
        &*kv::ErrNotExist,
        &*kv::ErrTxnRetryable,
        &*kv::ErrCannotSetNilValue,
        &*kv::ErrInvalidTxn,
        &*kv::ErrTxnTooLarge,
        &*kv::ErrEntryTooLarge,
        &*kv::ErrNotImplemented,
        &*kv::ErrWriteConflict,
        &*kv::ErrWriteConflictInTiDB,
        &*kv::ErrSharedLockLost,
    ];

    for error in errors {
        // ToSQLError 把内部 terror 转为 MySQL 协议可见的 SQLState/Code。
        let code = dbterror_dependency::terror::ToSQLError(error.as_ref()).Code;
        assert_ne!(kv::errno::ErrUnknown, code);
        assert_eq!(error.Code() as u16, code);
    }
}

#[test]
fn go_merge_4_shared_lock_lost_has_sql_error_code() {
    let error = &*kv::ErrSharedLockLost;
    let code = dbterror_dependency::terror::ToSQLError(error.as_ref()).Code;
    assert_eq!(9015, code);
}

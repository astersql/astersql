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

// Aster 迁移对照：TiKV/PD 错误到 TiDB 错误的全分支单元测试。
//
// 覆盖 nil、包装后的 ResultUndetermined、超限消息、哨兵类错误、带字段参数、
// sqlkiller 信号、GC / 资源组，以及官方 tikv-client 未映射错误保留原消息与堆栈。

use super::{
    ErrLockAcquireFailAndNoWaitSet, ErrLockWaitTimeout, ErrPDServerTimeout, ErrQueryInterrupted,
    ErrRegionUnavailable, ErrResolveLockTimeout, ErrResourceGroupConfigUnavailable,
    ErrResourceGroupNotExists, ErrResourceGroupThrottled, ErrTiFlashServerBusy,
    ErrTiFlashServerTimeout, ErrTiKVMaxTimestampNotSynced, ErrTiKVServerBusy, ErrTiKVServerTimeout,
    ErrTiKVStaleCommand, ErrTokenLimit, ErrTxnAbortedByGC, ErrUnknown, PdError, TiKvError,
    ToTiDBErr, errors, exeerrors, kv, register_tikv_returned_errors, sqlkiller, terror,
};

/// 便捷：将 TiKvError 经 ToTiDBErr 转为 SharedError。
fn convert_tikv(error: TiKvError) -> errors::SharedError {
    ToTiDBErr(Some(errors::SharedError::new(error))).expect("non-nil errors stay non-nil")
}

/// 断言两个错误在 terror Equal 语义下相等。
fn assert_equal(expected: &errors::Error, actual: &errors::SharedError) {
    assert!(
        expected.Equal(Some(actual)),
        "expected {expected}, got {actual}"
    );
}

/// None 保持 nil；重复注册安全；包装后的 ResultUndetermined 仍映射正确。
#[test]
fn nil_and_wrapped_undetermined_match_go_behavior() {
    register_tikv_returned_errors();
    // 第二次调用应被 Once 短路，不重复注册。
    register_tikv_returned_errors();
    assert!(ToTiDBErr(None).is_none());

    let wrappers: [fn(errors::SharedError) -> errors::SharedError; 4] = [
        |error| error,
        |error| errors::Trace(Some(error)).unwrap(),
        |error| errors::WithStack(Some(error)).unwrap(),
        |error| errors::Wrap(Some(error), "dummy").unwrap(),
    ];
    for wrap in wrappers {
        let error = wrap(errors::SharedError::new(TiKvError::ResultUndetermined));
        let converted = ToTiDBErr(Some(error)).unwrap();
        assert_equal(&terror::ErrResultUndetermined, &converted);
    }
}

/// Txn/Entry/KeyTooLarge 转换后保留 Go 客户端消息文本。
#[test]
fn memory_buffer_oversize_errors_preserve_go_messages() {
    let cases = [
        (
            TiKvError::TxnTooLarge { size: 100 },
            "Transaction is too large, size: 100",
        ),
        (
            TiKvError::EntryTooLarge {
                limit: 10,
                size: 20,
            },
            "entry too large, the max entry size is 10, the size of data is 20",
        ),
        (
            TiKvError::KeyTooLarge { key_size: 65_536 },
            "key is too large, the size of given key is 65536",
        ),
    ];
    for (error, expected) in cases {
        let converted = convert_tikv(error);
        assert!(converted.to_string().contains(expected), "{converted}");
    }
}

/// 无额外字段的哨兵错误映射到与 Go 相同的 TiDB 错误类。
#[test]
fn sentinel_errors_map_to_the_same_tidb_classes_as_go() {
    let cases: [(TiKvError, &errors::Error); 14] = [
        (TiKvError::NotFound, &kv::ErrNotExist),
        (TiKvError::CannotSetNilValue, &kv::ErrCannotSetNilValue),
        (TiKvError::InvalidTxn, &kv::ErrInvalidTxn),
        (TiKvError::TiKVServerTimeout, &ErrTiKVServerTimeout),
        (TiKvError::TiFlashServerTimeout, &ErrTiFlashServerTimeout),
        (TiKvError::TiKVServerBusy, &ErrTiKVServerBusy),
        (TiKvError::TiFlashServerBusy, &ErrTiFlashServerBusy),
        (TiKvError::TiKVStaleCommand, &ErrTiKVStaleCommand),
        (
            TiKvError::TiKVMaxTimestampNotSynced,
            &ErrTiKVMaxTimestampNotSynced,
        ),
        (
            TiKvError::LockAcquireFailAndNoWaitSet,
            &ErrLockAcquireFailAndNoWaitSet,
        ),
        (TiKvError::ResolveLockTimeout, &ErrResolveLockTimeout),
        (TiKvError::LockWaitTimeout, &ErrLockWaitTimeout),
        (TiKvError::RegionUnavailable, &ErrRegionUnavailable),
        (TiKvError::Unknown, &ErrUnknown),
    ];
    for (source, expected) in cases {
        assert_equal(expected, &convert_tikv(source));
    }
}

/// 带字段的错误保留参数；sqlkiller 信号映射到对应 exeerrors / QueryInterrupted。
#[test]
fn field_errors_and_query_signals_keep_go_arguments() {
    let cases = [
        (TiKvError::WriteConflictInLatch { start_ts: 42 }, "42"),
        (
            TiKvError::PdServerTimeout {
                message: "request timed out".to_owned(),
            },
            "request timed out",
        ),
        (TiKvError::TokenLimit { store_id: 88 }, "88"),
    ];
    for (source, expected) in cases {
        let converted = convert_tikv(source);
        assert!(converted.to_string().contains(expected), "{converted}");
    }
    assert_equal(
        &ErrPDServerTimeout,
        &convert_tikv(TiKvError::PdServerTimeout {
            message: "request timed out".to_owned(),
        }),
    );
    assert_equal(
        &ErrTokenLimit,
        &convert_tikv(TiKvError::TokenLimit { store_id: 88 }),
    );

    // (信号, 期望错误类, 消息中应包含的参数片段)
    let signal_cases: [(u32, &errors::Error, &str); 5] = [
        (sqlkiller::QueryInterrupted, &ErrQueryInterrupted, ""),
        (
            sqlkiller::MaxExecTimeExceeded,
            &exeerrors::ErrMaxExecTimeExceeded,
            "",
        ),
        (
            sqlkiller::QueryMemoryExceeded,
            &exeerrors::ErrMemoryExceedForQuery,
            "-1",
        ),
        (
            sqlkiller::ServerMemoryExceeded,
            &exeerrors::ErrMemoryExceedForInstance,
            "-1",
        ),
        (
            sqlkiller::RunawayQueryExceeded,
            &exeerrors::ErrResourceGroupQueryRunawayInterrupted,
            "exceed tidb side",
        ),
    ];
    for (signal, expected, argument) in signal_cases {
        let converted = convert_tikv(TiKvError::QueryInterruptedWithSignal { signal });
        assert_equal(expected, &converted);
        assert!(converted.to_string().contains(argument), "{converted}");
    }
}

/// GC 中止 / GcTooEarly 参数与 PD 资源组错误映射对齐 Go。
#[test]
fn gc_and_pd_resource_group_errors_keep_go_arguments() {
    let aborted = convert_tikv(TiKvError::TxnAbortedByGC {
        txn_start_ts: 11,
        txn_start_time: "start-time".to_owned(),
        txn_safe_point: 22,
        txn_safe_point_time: "safe-point-time".to_owned(),
    });
    assert_equal(&ErrTxnAbortedByGC, &aborted);
    for value in ["11", "start-time", "22", "safe-point-time"] {
        assert!(aborted.to_string().contains(value), "{aborted}");
    }

    // GcTooEarly 复用 ErrTxnAbortedByGC，未知时间戳填 "<unknown>"。
    let old = convert_tikv(TiKvError::GcTooEarly {
        txn_start_time: "old-start".to_owned(),
        gc_safe_point: "old-safe-point".to_owned(),
    });
    assert_equal(&ErrTxnAbortedByGC, &old);
    for value in ["<unknown>", "old-start", "old-safe-point"] {
        assert!(old.to_string().contains(value), "{old}");
    }

    let missing = ToTiDBErr(Some(errors::SharedError::new(
        PdError::ClientGetResourceGroup {
            resource_group_name: "analytics".to_owned(),
        },
    )))
    .unwrap();
    assert_equal(&ErrResourceGroupNotExists, &missing);
    assert!(missing.to_string().contains("analytics"), "{missing}");

    let config = ToTiDBErr(Some(errors::SharedError::new(
        PdError::ClientResourceGroupConfigUnavailable,
    )))
    .unwrap();
    assert_equal(&ErrResourceGroupConfigUnavailable, &config);

    let throttled = ToTiDBErr(Some(errors::SharedError::new(
        PdError::ClientResourceGroupThrottled,
    )))
    .unwrap();
    assert_equal(&ErrResourceGroupThrottled, &throttled);
}

/// 官方 client Undetermined 可映射；未映射错误保留原文并带堆栈。
#[test]
fn official_tikv_client_and_unknown_errors_are_not_simplified() {
    let undetermined = tikv_client::Error::UndeterminedError(Box::new(
        tikv_client::Error::StringError("network".to_owned()),
    ));
    let converted = ToTiDBErr(Some(errors::SharedError::new(undetermined))).unwrap();
    assert_equal(&terror::ErrResultUndetermined, &converted);

    let original = errors::SharedError::new(tikv_client::Error::StringError(
        "unmapped official error".to_owned(),
    ));
    let converted = ToTiDBErr(Some(original)).unwrap();
    assert!(converted.to_string().contains("unmapped official error"));
    assert!(errors::HasStack(&converted));
}

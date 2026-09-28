// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 可重试数据库错误判定。
//
// 对应 Go `pkg/util/dbutil` 的 `IsRetryableError`：按错误码列表，或对旧版 TiDB
// 将可重试错误包装成 1105（ErrUnknown）时的消息子串做二次匹配。

// Retryable1105Msgs list the error messages of some retryable error with `1105` code (`ErrUnknown`).
// Older TiDB versions wrapped some retryable errors as 1105.

use crate::interface::DbError;

/// 错误码为 1105 且消息包含这些子串时视为可重试（信息 schema 过期/变更）。
pub const Retryable1105Msgs: &[&str] = &[
    "Information schema is out of date",
    "Information schema is changed",
];
/// 始终可重试的 MySQL/TiDB 错误码（死锁、PD/TiKV 忙、写冲突、schema 变更等）。
pub const RETRYABLE_ERROR_CODES: &[u16] = &[
    1213, // ErrLockDeadlock
    9001, // ErrPDServerTimeout
    9003, // ErrTiKVServerBusy
    9004, // ErrResolveLockTimeout
    8027, // ErrInfoSchemaExpired
    8028, // ErrInfoSchemaChanged
    8005, // ErrWriteConflictInTiDB
    8022, // ErrTxnRetryable
    9007, // ErrWriteConflict
    8245, // ErrColumnInChange
];

/// 判断错误是否适合由上层自动重试。
pub fn IsRetryableError(error: &DbError) -> bool {
    // 先查固定错误码；再对 1105 做消息子串匹配以兼容旧包装。
    RETRYABLE_ERROR_CODES.contains(&error.code)
        || (error.code == 1105
            && Retryable1105Msgs
                .iter()
                .any(|message| error.message.contains(message)))
}

// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Backup error handling ported from `br/pkg/utils/error_handling.go`.
//!
//! 将备份 RPC/存储错误映射为「重试 / 放弃」策略，供备份客户端退避循环使用。
//! 先识别 protobuf 结构化错误，再对未知消息做子串启发式与按 store 的次数限制。
//! 与 Go 一致：上下文取消、缺文件/权限/凭证类错误直接放弃；可重试存储错误立即重试。
//! Reason 字符串面向运维，改动需同步 Go 测试期望，避免提示文案漂移。
//! ErrorContext 按 store uuid 分桶计数，多 store 互不影响配额。
//! StrategyUnknown 仅作中间态，最终应对调用方展开为 Retry 或 GiveUp。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::kvproto::brpb;
use astersql_br_pkg_logutil::{Field, log};

/// 可重试存储/网络错误子串表；匹配时不计入 unknown 重试配额。
pub static retryableErrorMsg: &[&str] = &[
    "server closed",
    "connection refused",
    "connection reset by peer",
    "channel closed",
    "error trying to connect",
    "connection closed before message completed",
    "body write aborted",
    "error during dispatch",
    "put object timeout",
    "timeout after",
    "internalerror",
    "not read from or written to within the timeout period",
    "<code>requesttimeout</code>",
    "<code>invalidpart</code>",
    "end of file before message length reached",
];

/// I/O 类错误关键字（小写匹配）。
pub const ioMsg: &str = "io";
/// 与 ioMsg 联用判定 NotFound 存储错误。
pub const notFoundMsg: &str = "notfound";
/// 权限拒绝关键字。
pub const permissionDeniedMsg: &str = "permissiondenied";
/// 对象存储凭证缺失关键字。
pub const credentialNotFoundMsg: &str = "credential info not found";

/// nil 错误时的兜底重试原因（正常路径不应到达）。
pub const unreachableRetryMsg: &str = "unreachable retry";
/// KvError → 重试。
pub const retryOnKvErrorMsg: &str = "retry on kv error";
/// RegionError → 重试（调度/epoch 变更常见）。
pub const retryOnRegionErrorMsg: &str = "retry on region error";
/// ClusterIdError → 放弃（连错集群不可恢复）。
pub const clusterIdMismatchMsg: &str = "cluster id mismatch";
/// protobuf 未识别错误的占位原因。
pub const unknownErrorMsg: &str = "unknown error";
/// context 取消子串；命中则放弃。
pub const contextCancelledMsg: &str = "context canceled";
/// unknown 错误尚在配额内时的重试原因。
pub const retryOnUnknownErrorMsg: &str = "unknown error, retry it for a few times";
/// unknown 错误超过配额后的放弃原因。
pub const noRetryOnUnknownErrorMsg: &str = "unknown error, retried too many times, give up";
/// 命中可重试存储子串时的原因文案。
pub const retryableStorageErrorMsg: &str = "retryable storage error";

/// 策略决策结果：策略枚举 + 人类可读原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorHandlingResult {
    pub Strategy: ErrorHandlingStrategy,
    pub Reason: String,
}

/// 备份错误处理策略，数值与 Go iota 对齐。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorHandlingStrategy {
    StrategyRetry = 0,
    StrategyGiveUp = 1,
    StrategyUnknown = 2,
}

/// 按 store uuid 统计 unknown 错误次数，并带场景描述。
#[derive(Clone, Debug)]
pub struct ErrorContext {
    encounterTimes: Arc<Mutex<HashMap<u64, i32>>>,
    encounterTimesLimitation: i32,
    description: String,
}

/// 创建错误上下文；`limitation` 为每个 uuid 允许的 unknown 重试次数上限。
pub fn NewErrorContext(scenario: impl Into<String>, limitation: i32) -> ErrorContext {
    ErrorContext {
        description: scenario.into(),
        encounterTimes: Arc::new(Mutex::new(HashMap::new())),
        encounterTimesLimitation: limitation,
    }
}

/// 默认场景、配额为 1。
pub fn NewDefaultContext() -> ErrorContext {
    NewErrorContext("default", 1)
}

/// 零重试配额：unknown 错误第一次即放弃。
pub fn NewZeroRetryContext(scenario: impl Into<String>) -> ErrorContext {
    NewErrorContext(scenario, 0)
}

/// 备份错误总入口：nil → 重试；结构化未知且有 msg → 走未知消息分支。
pub fn HandleBackupError(
    err: Option<&brpb::Error>,
    store_id: u64,
    ec: &mut ErrorContext,
) -> ErrorHandlingResult {
    let Some(err) = err else {
        return ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyRetry,
            Reason: unreachableRetryMsg.to_string(),
        };
    };

    let res = handleBackupProtoError(err);
    // StrategyUnknown 且带 msg 时继续启发式，否则直接返回结构化结果。
    if res.Strategy == ErrorHandlingStrategy::StrategyUnknown && !err.get_msg().is_empty() {
        return HandleUnknownBackupError(err.get_msg(), store_id, ec);
    }
    res
}

/// 仅根据 protobuf oneof 字段决策；均未设置则返回 Unknown。
pub fn handleBackupProtoError(e: &brpb::Error) -> ErrorHandlingResult {
    if e.has_kv_error() {
        return ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyRetry,
            Reason: retryOnKvErrorMsg.to_string(),
        };
    }
    if e.has_region_error() {
        return ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyRetry,
            Reason: retryOnRegionErrorMsg.to_string(),
        };
    }
    if e.has_cluster_id_error() {
        return ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyGiveUp,
            Reason: clusterIdMismatchMsg.to_string(),
        };
    }
    ErrorHandlingResult {
        Strategy: ErrorHandlingStrategy::StrategyUnknown,
        Reason: unknownErrorMsg.to_string(),
    }
}

/// 对未知错误消息做子串分类，并按 uuid 累计 unknown 重试次数。
pub fn HandleUnknownBackupError(
    msg: &str,
    uuid: u64,
    ec: &mut ErrorContext,
) -> ErrorHandlingResult {
    // 缺文件：BR 与 TiKV 存储路径/uid 不一致时常见，不可靠重试。
    if messageIsNotFoundStorageError(msg) {
        return ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyGiveUp,
            Reason: format!(
                "File or directory not found on TiKV Node (store id: {uuid}). workaround: please ensure br and tikv nodes share a same storage and the user of br and tikv has same uid."
            ),
        };
    }
    if messageIsPermissionDeniedStorageError(msg) {
        return ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyGiveUp,
            Reason: format!(
                "I/O permission denied error occurs on TiKV Node(store id: {uuid}). workaround: please ensure tikv has permission to read from & write to the storage."
            ),
        };
    }
    if messageIsCredentialNotFoundError(msg) {
        return ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyGiveUp,
            Reason: format!(
                "Credential info not found on TiKV Node (store id: {uuid}). workaround: please ensure the credential/access key is correctly configured for the storage."
            ),
        };
    }

    let msg_lower = msg.to_ascii_lowercase();
    if msg_lower.contains(contextCancelledMsg) {
        return ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyGiveUp,
            Reason: contextCancelledMsg.to_string(),
        };
    }

    // 瞬时存储/网络错误：立即重试，不消耗 unknown 配额。
    if MessageIsRetryableStorageError(msg) {
        log::Warn(
            retryableStorageErrorMsg,
            [
                Field::string("description", ec.description.clone()),
                Field::string("error", msg.to_string()),
            ],
        );
        return ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyRetry,
            Reason: retryableStorageErrorMsg.to_string(),
        };
    }

    // 其余 unknown：按 store 计数，超过 limitation 则放弃。
    let mut encounter_times = ec.encounterTimes.lock().expect("mutex poisoned");
    let times = encounter_times.entry(uuid).or_insert(0);
    *times += 1;
    if *times <= ec.encounterTimesLimitation {
        return ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyRetry,
            Reason: retryOnUnknownErrorMsg.to_string(),
        };
    }
    ErrorHandlingResult {
        Strategy: ErrorHandlingStrategy::StrategyGiveUp,
        Reason: noRetryOnUnknownErrorMsg.to_string(),
    }
}

/// 小写后同时包含 io 与 notfound。
pub fn messageIsNotFoundStorageError(msg: &str) -> bool {
    let msg_lower = msg.to_ascii_lowercase();
    msg_lower.contains(ioMsg) && msg_lower.contains(notFoundMsg)
}

/// 小写后包含 permissiondenied。
pub fn messageIsPermissionDeniedStorageError(msg: &str) -> bool {
    msg.to_ascii_lowercase().contains(permissionDeniedMsg)
}

/// 小写后包含 credential info not found（大小写不敏感）。
pub fn messageIsCredentialNotFoundError(msg: &str) -> bool {
    msg.to_ascii_lowercase().contains(credentialNotFoundMsg)
}

/// 消息是否命中可重试存储错误子串表。
pub fn MessageIsRetryableStorageError(msg: &str) -> bool {
    let msg_lower = msg.to_ascii_lowercase();
    retryableErrorMsg
        .iter()
        .any(|needle| msg_lower.contains(needle))
}

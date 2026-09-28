// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// 可重试错误判定：识别 Lightning 导入过程中遇临时故障时应自动重试的错误。
//
// 覆盖 URL/网络、MySQL 错误码、TiKV/PD 错误 ID、gRPC 状态码、HTTP 状态码及消息子串匹配。
// Region：TiKV 中数据分片单位；EpochNotMatch / NotLeader 等表示路由或领导者过期，通常可重试。

use crate::CommonError;

/// 错误消息中出现这些子串时视为可重试（大小写不敏感匹配前会先 lower）。
pub static retryableErrorMsgList: &[&str] = &[
    "coprocessor task terminated due to exceeding the deadline",
    "rate: wait",
    "injected random error",
];

/// 已知可重试的 TiKV/PD 等错误 ID 列表（对应 Go 侧 error ID）。
pub static retryableErrorIDs: &[&str] = &[
    "ErrKVEpochNotMatch",
    "ErrKVNotLeader",
    "ErrNoLeader",
    "ErrKVRegionNotFound",
    "ErrKVServerIsBusy",
    "ErrKVReadIndexNotReady",
    "ErrKVIngestFailed",
    "ErrKVRaftProposalDropped",
    "ErrCreatePDClient",
    "ErrRegionUnavailable",
    "ErrTiKVStaleCommand",
    "ErrTiKVServerTimeout",
    "ErrTiKVServerBusy",
    "ErrPDServerTimeout",
    "ErrUnknown",
];

/// 构造“写过慢”错误：通常表示 gRPC 长时间阻塞，视为可重试。
pub fn ErrWriteTooSlow() -> CommonError {
    CommonError::new(
        "write-too-slow",
        "write too slow, maybe gRPC is blocked forever",
    )
}

/// 根据错误消息子串判断是否可重试。
pub fn isRetryableFromErrorMessage(error: &CommonError) -> bool {
    let message = error.Message.to_ascii_lowercase();
    retryableErrorMsgList
        .iter()
        .any(|candidate| message.contains(candidate))
}

/// 判断 `url.Error` 内层错误是否可重试：无内层或 EOF 可重试；显式 request canceled 不可重试。
pub fn isRetryableURLInnerError(error: Option<&CommonError>) -> bool {
    let Some(error) = error else { return true };
    if error.Kind == "eof" {
        return true;
    }
    !error.Message.contains("net/http: request canceled")
}

/// 递归检查因果链中是否存在可重试的系统调用类错误（拒绝连接、重置、broken pipe）。
fn has_retryable_syscall_cause(error: &CommonError) -> bool {
    error.Causes.iter().any(|cause| {
        matches!(
            cause.Kind.as_str(),
            "connection-refused" | "connection-reset" | "broken-pipe"
        ) || has_retryable_syscall_cause(cause)
    })
}

/// 判断单个（非 multi）错误是否可重试；按 Kind、MySQL Code、错误 ID、HTTP/gRPC 码与消息依次匹配。
pub fn isSingleRetryableError(error: &CommonError) -> bool {
    // url.Error is special: Go checks it before Cause unwrap, and EOF inside url is retryable.
    // url 错误在 unwrap Cause 之前单独处理
    if error.Kind == "url" {
        return isRetryableURLInnerError(error.Causes.first());
    }

    // Match Go's errors.Cause unwrap for annotated/wrapped errors.
    // 沿 Causes 解包到根因，遇到 multi/url 则停止
    let mut error = error;
    while let Some(next) = error.Causes.first() {
        if error.Kind == "multi" || error.Kind == "url" {
            break;
        }
        error = next;
    }

    // 明确不可重试的 Kind
    if matches!(error.Kind.as_str(), "cancelled" | "eof" | "no-rows") {
        return false;
    }
    // 连接/超时等临时性 Kind 直接可重试
    if matches!(
        error.Kind.as_str(),
        "invalid-connection"
            | "bad-connection"
            | "write-too-slow"
            | "timeout"
            | "temporary"
            | "connection-refused"
            | "connection-reset"
            | "broken-pipe"
    ) {
        return true;
    }
    // DNS/网络类需看内层 syscall 是否可重试
    if matches!(error.Kind.as_str(), "dns" | "net" | "addr" | "op") {
        return has_retryable_syscall_cause(error);
    }
    // MySQL/TiDB 错误码：死锁、锁等待超时、Region 相关等
    if let Some(code) = error.Code {
        if [
            1105, 1213, 1205, 8005, 8022, 8027, 8028, 9001, 9002, 9003, 9004, 9005, 9007,
        ]
        .contains(&code)
        {
            return true;
        } else {
            return false;
        }
    }
    if retryableErrorIDs.contains(&error.ID.as_str()) {
        return true;
    }
    // HTTP：400/404 不可重试，其它状态码可重试
    if let Some(status) = error.StatusCode {
        return status != 400 && status != 404;
    }
    // gRPC 状态码：部分 Unknown 消息（磁盘空间不足、键序错误）不可重试
    if let Some(code) = &error.RpcCode {
        return match code.as_str() {
            "DeadlineExceeded" | "NotFound" | "AlreadyExists" | "PermissionDenied"
            | "ResourceExhausted" | "Aborted" | "OutOfRange" | "Unavailable" | "DataLoss" => true,
            "Unknown" => {
                !error.Message.contains("DiskSpaceNotEnough")
                    && !error
                        .Message
                        .contains("Keys must be added in strict ascending order")
            }
            _ => false,
        };
    }
    isRetryableFromErrorMessage(error)
}

/// 对外入口：`None` 不可重试；`multi` 要求全部子错误均可重试；否则走 `isSingleRetryableError`。
pub fn IsRetryableError(error: Option<&CommonError>) -> bool {
    let Some(error) = error else {
        return false;
    };
    // multierr：非空且每个子错误都可重试时才整体可重试
    if error.Kind == "multi" {
        return !error.Causes.is_empty() && error.Causes.iter().all(isSingleRetryableError);
    }
    isSingleRetryableError(error)
}

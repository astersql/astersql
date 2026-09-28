// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/utils/error_handling_test.go` (`package utils`).
//!
//! 覆盖 HandleBackupError 结构化分支与 HandleUnknownBackupError 启发式/配额语义。
//! 断言策略与 Reason 文案与 Go 测试期望字符串一致，防止运维提示漂移。
//! 每个用例独立构造 ErrorContext，避免跨用例污染 encounterTimes。
//! 结构化错误优先于消息启发式，确保 ClusterId 等不会被误判为可重试。

use crate::error_handling::{
    ErrorHandlingResult, ErrorHandlingStrategy, HandleBackupError, HandleUnknownBackupError,
    NewErrorContext, clusterIdMismatchMsg, noRetryOnUnknownErrorMsg, retryOnKvErrorMsg,
    retryOnRegionErrorMsg, retryOnUnknownErrorMsg, retryableStorageErrorMsg, unreachableRetryMsg,
};
use crate::kvproto::brpb;

#[test]
fn test_handle_error() {
    let mut ec = NewErrorContext("test", 3);
    // Test case 1: Error is nil
    // nil 错误：与 Go 一致返回 unreachable 重试。
    // store_id 在 nil 路径不参与决策，仅占位。
    let result = HandleBackupError(None, 123, &mut ec);
    assert_eq!(
        result,
        ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyRetry,
            Reason: unreachableRetryMsg.to_string(),
        }
    );

    // Test case 2: Error is KvError
    // KvError → StrategyRetry。
    // 模拟瞬时 KV 层错误，期望进入退避重试。
    let mut kv_err = brpb::Error::new();
    kv_err.set_kv_error(true);
    let result = HandleBackupError(Some(&kv_err), 123, &mut ec);
    assert_eq!(
        result,
        ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyRetry,
            Reason: retryOnKvErrorMsg.to_string(),
        }
    );

    // Test case 3: Error is RegionError
    // RegionError → StrategyRetry。
    // epoch/调度变更类错误同样可重试。
    let mut region_err = brpb::Error::new();
    region_err.set_region_error(true);
    let result = HandleBackupError(Some(&region_err), 123, &mut ec);
    assert_eq!(
        result,
        ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyRetry,
            Reason: retryOnRegionErrorMsg.to_string(),
        }
    );

    // Test case 4: Error is ClusterIdError
    // ClusterIdError → StrategyGiveUp，避免连错集群空转。
    // 与前三类形成对照：不可恢复配置错误。
    let mut cluster_err = brpb::Error::new();
    cluster_err.set_cluster_id_error(true);
    let result = HandleBackupError(Some(&cluster_err), 123, &mut ec);
    assert_eq!(
        result,
        ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyGiveUp,
            Reason: clusterIdMismatchMsg.to_string(),
        }
    );
}

#[test]
fn test_handle_error_msg() {
    let mut ec = NewErrorContext("test", 3);

    // NotFound：放弃并带 store id 与 workaround 文案。
    // 文案中的 store id 必须等于入参 uuid。
    let msg = "IO: files Notfound error";
    let uuid = 456u64;
    let expected_reason = "File or directory not found on TiKV Node (store id: 456). workaround: please ensure br and tikv nodes share a same storage and the user of br and tikv has same uid.";
    let actual = HandleUnknownBackupError(msg, uuid, &mut ec);
    assert_eq!(
        actual,
        ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyGiveUp,
            Reason: expected_reason.to_string(),
        }
    );

    // PermissionDenied：放弃。
    // 关键字匹配去掉空格差异后的 permissiondenied。
    let msg = "I/O permissiondenied error occurs on TiKV Node(store id: 456).";
    let expected_reason = "I/O permission denied error occurs on TiKV Node(store id: 456). workaround: please ensure tikv has permission to read from & write to the storage.";
    let actual = HandleUnknownBackupError(msg, uuid, &mut ec);
    assert_eq!(
        actual,
        ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyGiveUp,
            Reason: expected_reason.to_string(),
        }
    );

    // 可重试存储错误子串命中 → 立即重试，不消耗配额。
    // “server closed” 位于 retryableErrorMsg 表中。
    let msg = "server closed";
    let actual = HandleUnknownBackupError(msg, uuid, &mut ec);
    assert_eq!(
        actual,
        ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyRetry,
            Reason: retryableStorageErrorMsg.to_string(),
        }
    );

    // unknown：前 limitation 次重试，超额放弃（本测 limitation=3）。
    // 同一 uuid 连续 unknown 才会累加。
    let msg = "unknown error";
    let actual = HandleUnknownBackupError(msg, uuid, &mut ec);
    assert_eq!(
        actual,
        ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyRetry,
            Reason: retryOnUnknownErrorMsg.to_string(),
        }
    );

    // 第 2、3 次仍重试；第 4 次超过配额后 GiveUp。
    // 第 1 次已在上一断言消耗，此处再 2 次后第 4 次触发放弃。
    let _ = HandleUnknownBackupError(msg, uuid, &mut ec);
    let _ = HandleUnknownBackupError(msg, uuid, &mut ec);
    let actual = HandleUnknownBackupError(msg, uuid, &mut ec);
    assert_eq!(
        actual,
        ErrorHandlingResult {
            Strategy: ErrorHandlingStrategy::StrategyGiveUp,
            Reason: noRetryOnUnknownErrorMsg.to_string(),
        }
    );
}

#[test]
fn test_handle_credential_not_found_error() {
    // 凭证缺失：大小写不敏感匹配，策略均为放弃。
    // Azure 文案变体验证 to_ascii_lowercase 路径。
    let mut ec = NewErrorContext("test", 3);
    let uuid = 789u64;
    let expected_reason = "Credential info not found on TiKV Node (store id: 789). workaround: please ensure the credential/access key is correctly configured for the storage.";
    let expected = ErrorHandlingResult {
        Strategy: ErrorHandlingStrategy::StrategyGiveUp,
        Reason: expected_reason.to_string(),
    };

    let azure_msg = "External storage error: credential info not found";
    assert_eq!(HandleUnknownBackupError(azure_msg, uuid, &mut ec), expected);

    // 大小写变体仍应命中同一分支。
    let azure_msg_upper = "External storage error: Credential Info Not Found";
    assert_eq!(
        HandleUnknownBackupError(azure_msg_upper, uuid, &mut ec),
        expected
    );
}

#[test]
fn cloned_error_context_shares_unknown_retry_quota() {
    let ec = NewErrorContext("shared", 1);
    let mut first_handle = ec.clone();
    let mut second_handle = ec;

    assert_eq!(
        HandleUnknownBackupError("unknown", 42, &mut first_handle).Strategy,
        ErrorHandlingStrategy::StrategyRetry
    );
    assert_eq!(
        HandleUnknownBackupError("unknown", 42, &mut second_handle).Strategy,
        ErrorHandlingStrategy::StrategyGiveUp
    );
}

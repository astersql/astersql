// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// `IsRetryableError` 单元测试：覆盖 URL、网络、HTTP、KV、MySQL、gRPC、multierr 与消息匹配等分支。

use crate::{CommonError, ErrCreateKVClient, ErrWriteTooSlow, IsRetryableError};

/// 构造指定 Kind 与消息的简单错误。
fn err(kind: &str, message: &str) -> CommonError {
    CommonError::new(kind, message)
}

/// 构造带错误 ID 的 RFC 风格错误。
fn with_id(id: &str, message: &str) -> CommonError {
    CommonError::rfc(id, message)
}

/// 构造带 MySQL/TiDB 错误码的错误。
fn with_code(code: u16) -> CommonError {
    let mut error = CommonError::new("mysql", "mysql error");
    error.Code = Some(code);
    error
}

/// 构造带 HTTP 状态码的错误。
fn with_status(status: u16) -> CommonError {
    let mut error = CommonError::new("http", "http status");
    error.StatusCode = Some(status);
    error
}

/// 构造带 gRPC RpcCode 的错误。
fn with_rpc(code: &str, message: &str) -> CommonError {
    let mut error = CommonError::new("grpc", message);
    error.RpcCode = Some(code.to_owned());
    error
}

/// 构造 multi 组合错误，Causes 为子错误列表。
fn multi(errors: Vec<CommonError>) -> CommonError {
    let mut error = CommonError::new("multi", "combined");
    error.Causes = errors;
    error
}

/// 全面断言各类错误的可重试性，与 Go 侧测试用例对齐。
#[test]
fn test_is_retryable_error() {
    // url errors
    // URL 错误：无内层可重试；EOF 可重试；request canceled 不可重试
    assert!(IsRetryableError(Some(&err("url", "url error"))));
    assert!(IsRetryableError(Some(
        &err("url", "url error").wrap(err("eof", "EOF"))
    )));
    assert!(!IsRetryableError(Some(
        &err("url", "url error").wrap(err("cancel", "net/http: request canceled"))
    )));
    assert!(!IsRetryableError(Some(&err("url", "url error").wrap(err(
        "cancel",
        "net/http: request canceled while waiting for connection"
    )))));
    assert!(IsRetryableError(Some(
        &err("url", "url error").wrap(err("other", "dummy error"))
    )));
    assert!(IsRetryableError(Some(
        &err("url", "url error").wrap(err("other", "use of closed network connection"))
    )));

    // cancelled 不可重试；timeout / WriteTooSlow / 包装后的超时可重试
    assert!(!IsRetryableError(Some(&err(
        "cancelled",
        "context canceled"
    ))));
    assert!(IsRetryableError(Some(&err(
        "timeout",
        "context deadline exceeded"
    ))));
    assert!(IsRetryableError(Some(
        &ErrCreateKVClient
            .clone()
            .wrap(err("timeout", "context deadline exceeded"))
            .gen_with_stack("create kv client error")
    )));
    assert!(IsRetryableError(Some(&ErrWriteTooSlow())));
    assert!(!IsRetryableError(Some(&err("eof", "EOF"))));
    assert!(!IsRetryableError(Some(&err("addr", "address error"))));
    assert!(!IsRetryableError(Some(&err("dns", "dns error"))));
    assert!(IsRetryableError(Some(&err("timeout", "dns timeout"))));
    assert!(IsRetryableError(Some(&err("temporary", "dns temporary"))));

    // inner syscall errors
    // DNS 包装可重试 syscall（拒绝连接/broken-pipe/重置）可重试；ENETDOWN 不可
    assert!(IsRetryableError(Some(
        &err("dns", "dns").wrap(err("connection-refused", "ECONNREFUSED"))
    )));
    assert!(IsRetryableError(Some(
        &err("dns", "dns").wrap(err("broken-pipe", "EPIPE"))
    )));
    assert!(IsRetryableError(Some(
        &err("dns", "dns").wrap(err("connection-reset", "ECONNRESET"))
    )));
    assert!(!IsRetryableError(Some(
        &err("dns", "dns").wrap(err("net-down", "ENETDOWN"))
    )));

    // request error
    // HTTP 400/404 不可重试，500 可重试
    assert!(!IsRetryableError(Some(&with_status(400))));
    assert!(!IsRetryableError(Some(&with_status(404))));
    assert!(IsRetryableError(Some(&with_status(500))));

    // kv errors
    // TiKV 常见可重试错误 ID；磁盘满不可重试
    assert!(IsRetryableError(Some(
        &with_id("ErrNoLeader", "no leader").annotate("when write to tikv")
    )));
    for id in [
        "ErrKVNotLeader",
        "ErrKVEpochNotMatch",
        "ErrKVServerIsBusy",
        "ErrKVRegionNotFound",
        "ErrKVReadIndexNotReady",
        "ErrKVIngestFailed",
        "ErrKVRaftProposalDropped",
    ] {
        assert!(IsRetryableError(Some(&with_id(id, "kv"))));
        assert!(IsRetryableError(Some(
            &with_id(id, "kv").gen_with_stack("test")
        )));
    }
    assert!(!IsRetryableError(Some(
        &with_id("ErrKVDiskFull", "disk full").gen_with_stack("test")
    )));

    for id in [
        "ErrRegionUnavailable",
        "ErrTiKVStaleCommand",
        "ErrTiKVServerTimeout",
        "ErrTiKVServerBusy",
        "ErrPDServerTimeout",
        "ErrUnknown",
    ] {
        assert!(IsRetryableError(Some(
            &with_id(id, "tidb").annotate("failed")
        )));
    }

    // net: connection refused
    assert!(IsRetryableError(Some(&err(
        "connection-refused",
        "connection refused"
    ))));
    assert!(IsRetryableError(Some(
        &err("url", "post").wrap(err("connection-refused", "connection refused"))
    )));

    // MySQL Errors
    // 错误码 0 不可重试；死锁/锁等待/Region 等码可重试
    assert!(!IsRetryableError(Some(&with_code(0))));
    for err_number in [
        1105_u16, 1213, 1205, 8005, 8022, 8027, 8028, 9001, 9002, 9003, 9004, 9005, 9007,
    ] {
        assert!(IsRetryableError(Some(&with_code(err_number))));
    }
    // ErrBadNumber is adjacent to the retryable InfoSchema codes but is permanent.
    assert!(!IsRetryableError(Some(&with_code(8029))));

    // gRPC Errors
    // Canceled 不可重试；常见状态码与部分 Unknown 可重试
    assert!(!IsRetryableError(Some(&with_rpc("Canceled", ""))));
    assert!(IsRetryableError(Some(&with_rpc(
        "Unknown",
        "region 1234 is not fully replicated"
    ))));
    assert!(IsRetryableError(Some(&with_rpc(
        "Unknown",
        "No such file or directory: while stat a file for size: /...../write.sst: No such file or directory"
    ))));
    for code in [
        "DeadlineExceeded",
        "NotFound",
        "AlreadyExists",
        "PermissionDenied",
        "ResourceExhausted",
        "Aborted",
        "OutOfRange",
        "Unavailable",
        "DataLoss",
    ] {
        assert!(IsRetryableError(Some(&with_rpc(code, ""))));
    }

    // sqlmock errors
    assert!(!IsRetryableError(Some(&err(
        "sqlmock",
        "call to database Close was not expected"
    ))));

    // stderr
    assert!(IsRetryableError(Some(&err(
        "invalid-connection",
        "invalid connection"
    ))));
    assert!(IsRetryableError(Some(&err(
        "bad-connection",
        "bad connection"
    ))));
    assert!(!IsRetryableError(Some(&err("other", "error"))));

    // multierr
    // multi：全部可重试才为真；混入 cancelled 则为假
    assert!(!IsRetryableError(Some(&multi(vec![
        err("cancelled", "context canceled"),
        err("cancelled", "context canceled"),
    ]))));
    assert!(IsRetryableError(Some(&multi(vec![
        err("timeout", "dns timeout"),
        err("timeout", "dns timeout"),
    ]))));
    assert!(!IsRetryableError(Some(&multi(vec![
        err("cancelled", "context canceled"),
        err("timeout", "dns timeout"),
    ]))));

    // 消息子串：coprocessor deadline / rate limiter
    assert!(IsRetryableError(Some(&err(
        "other",
        "other error: Coprocessor task terminated due to exceeding the deadline"
    ))));

    // error from limiter
    assert!(IsRetryableError(Some(&err(
        "other",
        "rate: Wait(n=10) would exceed context deadline"
    ))));
}

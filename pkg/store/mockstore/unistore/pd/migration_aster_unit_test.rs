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

// PD 客户端辅助逻辑的 Aster 迁移单测。
//
// 覆盖 URL 规范化、成员地址排序、重试策略、心跳待发送队列，
// 以及 PD 响应头错误传播；确保与 Go 侧行为一致。

use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tikv_client_proto::pdpb;

/// 构造带指定 member_id 与 client_urls 的 PD Member 测试夹具。
fn member(id: u64, urls: &[&str]) -> pdpb::Member {
    pdpb::Member {
        member_id: id,
        client_urls: urls.iter().map(|url| (*url).to_owned()).collect(),
        ..Default::default()
    }
}

/// 无 scheme 的地址补 `http://`，已有 https 的保持不变。
#[test]
fn normalize_pd_urls_adds_only_missing_scheme_aster() {
    assert_eq!(
        normalize_pd_urls(vec!["127.0.0.1:2379".into(), "https://pd:2379".into()]),
        vec!["http://127.0.0.1:2379", "https://pd:2379"]
    );
}

/// 成员 URL 排序：非 leader 在前，leader 的全部 URL 排在末尾。
#[test]
fn ordered_member_urls_puts_leader_last_aster() {
    let leader = member(2, &["http://leader-1", "http://leader-2"]);
    let members = vec![
        member(2, &["http://leader-1", "http://leader-2"]),
        member(1, &["http://follower"]),
    ];
    assert_eq!(
        ordered_member_urls(&members, &leader),
        vec!["http://follower", "http://leader-1", "http://leader-2"]
    );
}

/// 持续失败时重试次数上限为 MAX_RETRY_COUNT（10）。
#[test]
fn retry_runs_at_most_ten_attempts_aster() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&attempts);
    let result: Result<()> = retry_with_policy(
        MAX_RETRY_COUNT,
        Duration::ZERO,
        || {
            observed.fetch_add(1, Ordering::SeqCst);
            Err(PdError::TooManyRetries)
        },
        |_| {},
    );
    assert!(matches!(result, Err(PdError::TooManyRetries)));
    assert_eq!(attempts.load(Ordering::SeqCst), 10);
}

/// 成功则提前结束；每次失败都回调通知，成功前通知次数 = 失败次数。
#[test]
fn retry_stops_on_success_and_notifies_each_failure_aster() {
    let mut attempts = 0;
    let mut notifications = 0;
    let value = retry_with_policy(
        MAX_RETRY_COUNT,
        Duration::ZERO,
        || {
            attempts += 1;
            if attempts == 3 {
                Ok(42)
            } else {
                Err(PdError::TooManyRetries)
            }
        },
        |_| notifications += 1,
    )
    .unwrap();
    assert_eq!(value, 42);
    assert_eq!(attempts, 3);
    assert_eq!(notifications, 2);
}

/// Region 心跳（RegionHeartbeat）：restore 的失败请求优先于通道中排队请求。
#[test]
fn failed_heartbeat_is_returned_before_queued_requests_aster() {
    let (tx, rx) = crossbeam_channel::bounded(2);
    let mut pending = PendingHeartbeat::default();
    let first = pdpb::RegionHeartbeatRequest::default();
    let second = pdpb::RegionHeartbeatRequest {
        bytes_written: 7,
        ..Default::default()
    };
    // 通道先放入 second，再 restore first，next 应先吐出 first。
    tx.send(second).unwrap();
    pending.restore(first);

    assert_eq!(pending.next(&rx).unwrap().bytes_written, 0);
    assert_eq!(pending.next(&rx).unwrap().bytes_written, 7);
}

/// 响应头携带 Error 时，check_response_header 应原样传播错误信息。
#[test]
fn response_header_error_is_propagated_aster() {
    let header = pdpb::ResponseHeader {
        error: Some(pdpb::Error {
            message: "not bootstrapped".into(),
            ..Default::default()
        })
        .into(),
        ..Default::default()
    };
    let err = check_response_header(&header).unwrap_err();
    assert!(err.to_string().contains("not bootstrapped"));

    // Go returns `errors.New(herr.String())`, so an error carrying only its
    // protocol type must not collapse to an empty diagnostic.
    let mut typed_error = pdpb::Error::default();
    typed_error.set_type(pdpb::ErrorType::NotBootstrapped);
    let typed_header = pdpb::ResponseHeader {
        error: Some(typed_error).into(),
        ..Default::default()
    };
    let typed_diagnostic = check_response_header(&typed_header)
        .unwrap_err()
        .to_string();
    assert!(
        typed_diagnostic.contains("NOT_BOOTSTRAPPED"),
        "PD error type was lost: {typed_diagnostic}"
    );
}

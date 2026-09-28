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

// Executor failpoint（故障注入点）测试。
//
// Failpoint 在关键路径注入延迟、错误或 panic，用于验证可重复读二次读取、
// Shuffle worker 恢复边界、BatchCop 重试，以及 unistore RPC 超时等行为。
//
// 下方测试直接调用生产 failpoint 入口。

// ----- 以下为已接线的 Rust failpoint 测试 -----

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use astersql_store_copr::{
    Backoffer, BatchError, BatchRequest, BatchResult, CancellationToken, RegionBatchRequestSender,
    RegionFailureHandler, RegionInfo, RegionStore, RpcClient, RpcContext, RpcResponse,
};

#[test]
/// PointGet 可重复读路径：step1/step2 两个 failpoint 必须在二次读边界各触发一次。
fn point_get_failpoints_are_consumed_at_the_canonical_second_read_boundary() {
    let calls = Arc::new(AtomicUsize::new(0));
    let step_one = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/executor/pointGetRepeatableReadTest-step1",
        {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
            }
        },
    );
    let step_two = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/executor/pointGetRepeatableReadTest-step2",
        {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
            }
        },
    );
    // 触发生产路径上的两个 step failpoint。
    crate::point_get::point_get_repeatable_read_failpoint();
    assert_eq!(calls.load(Ordering::Acquire), 2);
    drop((step_one, step_two));
}

#[test]
/// BatchPointGet：索引快照读后的 step1/step2 failpoint 各消费一次。
fn batch_point_get_failpoints_are_consumed_after_index_snapshot_read() {
    let calls = Arc::new(AtomicUsize::new(0));
    let step_one = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/executor/batchPointGetRepeatableReadTest-step1",
        {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
            }
        },
    );
    let step_two = astersql_testkit_testfailpoint::enable_call(
        "github.com/pingcap/tidb/pkg/executor/batchPointGetRepeatableReadTest-step2",
        {
            let calls = Arc::clone(&calls);
            move || {
                calls.fetch_add(1, Ordering::AcqRel);
            }
        },
    );
    // 触发 BatchPointGet 可重复读边界上的两个 step failpoint。
    crate::batch_point_get::batch_point_get_repeatable_read_failpoint();
    assert_eq!(calls.load(Ordering::Acquire), 2);
    drop((step_one, step_two));
}

#[test]
/// Shuffle worker 在可恢复边界内 panic，外层 catch_unwind 应捕获。
fn shuffle_worker_failpoint_panics_inside_the_recovered_worker_boundary() {
    let guard = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/executor/shuffleWorkerRun",
        "panic(shuffle-worker)",
    );
    // failpoint 配置为 panic(shuffle-worker)，应在 worker 恢复边界内爆发。
    let panic = std::panic::catch_unwind(crate::shuffle::shuffle_worker_failpoint);
    assert!(panic.is_err());
    drop(guard);
}

/// 故意 panic 的 RPC 客户端：注入响应必须绕过真实传输层。
struct UnexpectedRpcClient;

impl RpcClient for UnexpectedRpcClient {
    fn send_request(
        &self,
        _address: &str,
        _request: &BatchRequest,
        _timeout: Duration,
        _cancellation: &CancellationToken,
    ) -> BatchResult<RpcResponse> {
        panic!("injected BatchCop response must bypass the transport")
    }
}

#[derive(Default)]
/// 记录 BatchCop 发送失败回调次数，用于断言重试路径。
struct FailureRecorder {
    calls: AtomicUsize,
}

impl RegionFailureHandler for FailureRecorder {
    /// 断言 reload_region 且错误为 OtherResponse，并累加调用计数。
    fn on_send_fail_for_batch_regions(
        &self,
        _store: Option<&RegionStore>,
        _regions: &[RegionInfo],
        reload_region: bool,
        error: &BatchError,
    ) {
        assert!(reload_region);
        assert!(matches!(error, BatchError::OtherResponse(_)));
        self.calls.fetch_add(1, Ordering::AcqRel);
    }
}

#[test]
/// mockBatchCopResponseError 注入后，sender 走真实重试并回调 FailureRecorder。
fn batch_cop_response_failpoint_uses_the_real_sender_retry_path() {
    let guard = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/store/copr/mockBatchCopResponseError",
        "return(true)",
    );
    let recorder = Arc::new(FailureRecorder::default());
    let mut sender = RegionBatchRequestSender::new(
        recorder.clone(),
        Arc::new(UnexpectedRpcClient),
        true,
        false,
        Arc::new(AtomicBool::new(false)),
    );
    let mut request = BatchRequest::default();
    let mut backoff = Backoffer::new(1);
    // UnexpectedRpcClient 不应被调用；响应由 failpoint 注入。
    let result = sender.send_req_to_addr(
        &mut backoff,
        &RpcContext {
            address: "tiflash0".to_owned(),
            ..Default::default()
        },
        &[RegionInfo::default()],
        &mut request,
        Duration::from_secs(1),
    );
    assert!(result.retry);
    assert!(result.response.is_none());
    assert_eq!(recorder.calls.load(Ordering::Acquire), 1);
    assert!(matches!(
        sender.last_rpc_error,
        Some(BatchError::OtherResponse(_))
    ));
    drop(guard);
}

#[test]
/// unistoreRPCDeadlineExceeded 注入后返回真实 Deadline is exceeded RPC 错误。
fn unistore_deadline_failpoint_returns_the_real_rpc_error() {
    use astersql_store_mockstore_unistore::{Request, RpcError};

    let (client, _, _) =
        astersql_store_mockstore_unistore::New("", Vec::new(), 0, Vec::new()).unwrap();
    let guard = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/store/mockstore/unistore/unistoreRPCDeadlineExceeded",
        "return(true)",
    );
    let error = match client.send_request("unused", Request::Empty, Duration::from_millis(10)) {
        Err(error) => error,
        Ok(_) => panic!("deadline failpoint unexpectedly returned a response"),
    };
    assert_eq!(error, RpcError::Server("Deadline is exceeded".to_owned()));
    drop(guard);
    client.close().unwrap();
}

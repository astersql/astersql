// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Port of `pkg/meta/autoid/autoid_service_test.go`.
//
// AutoID 远程服务相关单元测试：用 FakeDiscovery/FakeClient 验证
// 上下文取消时 alloc/rebase 快速返回，以及 Backoffer 对 Context 的感知。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use crate::*;

/// 固定返回 Leader 地址的假发现器。
#[derive(Default)]
struct FakeDiscovery;

impl LeaderDiscovery for FakeDiscovery {
    fn leader(&self, _ctx: &Context, _path: &str) -> Result<Option<String>> {
        Ok(Some("leader:1234".into()))
    }
}

/// 按队列弹出预设结果的假 RPC 客户端，并统计调用次数。
#[derive(Default)]
struct FakeClient {
    alloc_results: Mutex<VecDeque<Result<AutoIdResponse>>>,
    rebase_results: Mutex<VecDeque<Result<RebaseResponse>>>,
    alloc_requests: Mutex<Vec<AutoIdRequest>>,
    rebase_requests: Mutex<Vec<RebaseRequest>>,
    alloc_calls: AtomicUsize,
    rebase_calls: AtomicUsize,
}

impl AutoIdClient for FakeClient {
    fn alloc_auto_id(&self, _ctx: &Context, request: AutoIdRequest) -> Result<AutoIdResponse> {
        self.alloc_requests.lock().unwrap().push(request);
        self.alloc_calls.fetch_add(1, Ordering::SeqCst);
        self.alloc_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(AutoIdResponse {
                min: 0,
                max: 1,
                errmsg: String::new(),
            }))
    }

    fn rebase(&self, _ctx: &Context, request: RebaseRequest) -> Result<RebaseResponse> {
        self.rebase_requests.lock().unwrap().push(request);
        self.rebase_calls.fetch_add(1, Ordering::SeqCst);
        self.rebase_results
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(RebaseResponse {
                errmsg: String::new(),
            }))
    }
}

/// 空操作连接，关闭即成功。
#[derive(Default)]
struct FakeConnection;

impl ClientConnection for FakeConnection {
    fn close(&self) -> Result<()> {
        Ok(())
    }
}

/// 始终连到注入的 FakeClient。
struct FakeConnector {
    client: Arc<FakeClient>,
}

impl AutoIdClientConnector for FakeConnector {
    fn connect(
        &self,
        _address: &str,
    ) -> Result<(Arc<dyn AutoIdClient>, Arc<dyn ClientConnection>)> {
        Ok((self.client.clone(), Arc::new(FakeConnection)))
    }
}

/// 构造已注入 mock 客户端的单点分配器。
fn new_test_single_point_alloc(mock: Arc<FakeClient>) -> SinglePointAllocator {
    let discover = Arc::new(ClientDiscover::new(
        Arc::new(FakeDiscovery),
        Arc::new(FakeConnector {
            client: mock.clone(),
        }),
    ));
    discover.seed_client_for_test(mock);
    SinglePointAllocator::new(1, 1, false, NULLSPACE_ID, discover)
}

#[test]
fn go_merge_5_alloc_tracks_maximum_and_rebase_is_monotonic() {
    let mock = Arc::new(FakeClient::default());
    mock.alloc_results
        .lock()
        .unwrap()
        .push_back(Ok(AutoIdResponse {
            min: 10,
            max: 20,
            errmsg: String::new(),
        }));
    let alloc = new_test_single_point_alloc(mock);
    assert_eq!(
        alloc.alloc(&Context::background(), 10, 1, 1).unwrap(),
        (10, 20)
    );
    assert_eq!(alloc.base(), 20);
    alloc.rebase(&Context::background(), 5, false).unwrap();
    assert_eq!(alloc.base(), 20);
}

#[test]
fn go_merge_5_transfer_refreshes_source_and_rolls_back_failure() {
    let mock = Arc::new(FakeClient::default());
    mock.alloc_results
        .lock()
        .unwrap()
        .push_back(Ok(AutoIdResponse {
            min: 0,
            max: 42,
            errmsg: String::new(),
        }));
    mock.rebase_results
        .lock()
        .unwrap()
        .push_back(Err(AutoIdError::Service("denied".into())));
    let alloc = new_test_single_point_alloc(mock.clone());
    assert!(alloc.transfer(2, 3).is_err());
    assert_eq!(alloc.base(), 42);
    assert_eq!(mock.alloc_calls.load(Ordering::SeqCst), 1);
    mock.alloc_results
        .lock()
        .unwrap()
        .push_back(Ok(AutoIdResponse {
            min: 42,
            max: 43,
            errmsg: String::new(),
        }));
    alloc.alloc(&Context::background(), 1, 1, 1).unwrap();
    assert_eq!(alloc.base(), 43);
}

#[test]
fn go_merge_5_retry_limit_requires_count_and_duration() {
    let mock = Arc::new(FakeClient::default());
    for _ in 0..4 {
        mock.alloc_results
            .lock()
            .unwrap()
            .push_back(Err(AutoIdError::Rpc("rpc error: unavailable".into())));
    }
    let mut alloc = new_test_single_point_alloc(mock.clone());
    alloc.set_retry_policy_for_test(2, Duration::from_millis(15));
    let error = alloc.alloc(&Context::background(), 1, 1, 1).unwrap_err();
    assert!(is_rpc_retry_limit_error(&error));
    assert!(
        error
            .to_string()
            .contains("check AutoID service availability")
    );
    assert!(mock.alloc_calls.load(Ordering::SeqCst) >= 2);
}

#[test]
fn go_merge_5_rebase_retry_limit_is_marked() {
    let mock = Arc::new(FakeClient::default());
    mock.rebase_results
        .lock()
        .unwrap()
        .push_back(Err(AutoIdError::Rpc("rpc error: unavailable".into())));
    let mut alloc = new_test_single_point_alloc(mock.clone());
    alloc.set_retry_policy_for_test(1, Duration::ZERO);
    let error = alloc.rebase(&Context::background(), 10, false).unwrap_err();
    assert!(is_rpc_retry_limit_error(&error));
    assert_eq!(mock.rebase_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn go_merge_5_force_rebase_can_lower_base() {
    let mock = Arc::new(FakeClient::default());
    mock.alloc_results
        .lock()
        .unwrap()
        .push_back(Ok(AutoIdResponse {
            min: 1,
            max: 50,
            errmsg: String::new(),
        }));
    let alloc = new_test_single_point_alloc(mock);
    alloc.alloc(&Context::background(), 1, 1, 1).unwrap();
    alloc.force_rebase(5).unwrap();
    assert_eq!(alloc.base(), 5);
}

#[test]
fn go_merge_5_transfer_uses_authoritative_base_and_unsigned_order() {
    let mock = Arc::new(FakeClient::default());
    mock.alloc_results
        .lock()
        .unwrap()
        .push_back(Ok(AutoIdResponse {
            min: 0,
            max: 42,
            errmsg: String::new(),
        }));
    let alloc = new_test_single_point_alloc(mock.clone());
    alloc.transfer(2, 3).unwrap();
    assert_eq!(mock.alloc_calls.load(Ordering::SeqCst), 1);
    assert_eq!(mock.rebase_calls.load(Ordering::SeqCst), 1);
    assert_eq!(alloc.base(), 42);
    alloc.transfer(2, 3).unwrap();
    assert_eq!(mock.alloc_calls.load(Ordering::SeqCst), 1);

    let discover = Arc::new(ClientDiscover::new(
        Arc::new(FakeDiscovery),
        Arc::new(FakeConnector {
            client: mock.clone(),
        }),
    ));
    discover.seed_client_for_test(mock.clone());
    let unsigned = SinglePointAllocator::new(1, 1, true, NULLSPACE_ID, discover);
    mock.alloc_results
        .lock()
        .unwrap()
        .push_back(Ok(AutoIdResponse {
            min: 0,
            max: -2,
            errmsg: String::new(),
        }));
    unsigned.alloc(&Context::background(), 1, 1, 1).unwrap();
    unsigned.rebase(&Context::background(), 3, false).unwrap();
    assert_eq!(unsigned.base(), -2);
    unsigned.force_rebase(3).unwrap();
    assert_eq!(unsigned.base(), 3);
}

struct OutOfOrderClient {
    calls: AtomicUsize,
    started: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl AutoIdClient for OutOfOrderClient {
    fn alloc_auto_id(&self, _ctx: &Context, _request: AutoIdRequest) -> Result<AutoIdResponse> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            self.started.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
        }
        Ok(AutoIdResponse {
            min: call as i64,
            max: call as i64 + 1,
            errmsg: String::new(),
        })
    }

    fn rebase(&self, _ctx: &Context, _request: RebaseRequest) -> Result<RebaseResponse> {
        Ok(RebaseResponse::default())
    }
}

#[test]
fn go_merge_5_out_of_order_responses_keep_highest_base() {
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let client = Arc::new(OutOfOrderClient {
        calls: AtomicUsize::new(0),
        started: started_tx,
        release: Mutex::new(release_rx),
    });
    let fallback = Arc::new(FakeClient::default());
    let discover = Arc::new(ClientDiscover::new(
        Arc::new(FakeDiscovery),
        Arc::new(FakeConnector { client: fallback }),
    ));
    discover.seed_client_for_test(client);
    let alloc = Arc::new(SinglePointAllocator::new(
        1,
        1,
        false,
        NULLSPACE_ID,
        discover,
    ));
    let first = alloc.clone();
    let first_done = thread::spawn(move || first.alloc(&Context::background(), 1, 1, 1));
    started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(alloc.alloc(&Context::background(), 1, 1, 1).unwrap().1, 2);
    release_tx.send(()).unwrap();
    first_done.join().unwrap().unwrap();
    assert_eq!(alloc.base(), 2);
}

/// Corresponds to Go `TestAllocCanceledRPCReturnsQuickly`.
/// 上下文已取消时，RPC 错误应立即映射为 Canceled，不进入长退避。
#[test]
fn test_alloc_canceled_rpc_returns_quickly() {
    let mock = Arc::new(FakeClient::default());
    mock.alloc_results
        .lock()
        .unwrap()
        .push_back(Err(AutoIdError::Rpc(
            "rpc error: code = Canceled desc = context canceled".into(),
        )));
    let alloc = new_test_single_point_alloc(mock.clone());
    let ctx = Context::background();
    ctx.cancel();

    let started = Instant::now();
    let err = alloc.alloc(&ctx, 1, 1, 1).unwrap_err();
    assert!(matches!(err, AutoIdError::Canceled));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(mock.alloc_calls.load(Ordering::SeqCst), 1);
}

/// Corresponds to Go `TestRebaseCanceledRPCReturnsQuickly`.
/// rebase 路径同样在取消后快速返回。
#[test]
fn test_rebase_canceled_rpc_returns_quickly() {
    let mock = Arc::new(FakeClient::default());
    mock.rebase_results
        .lock()
        .unwrap()
        .push_back(Err(AutoIdError::Rpc(
            "rpc error: code = Canceled desc = context canceled".into(),
        )));
    let alloc = new_test_single_point_alloc(mock.clone());
    let ctx = Context::background();
    ctx.cancel();

    let started = Instant::now();
    let err = alloc.rebase(&ctx, 100, false).unwrap_err();
    assert!(matches!(err, AutoIdError::Canceled));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(mock.rebase_calls.load(Ordering::SeqCst), 1);
}

/// Corresponds to Go `TestBackoffCtxAware`.
/// 无 Context 时真实休眠；已取消或中途取消时迅速失败。
#[test]
fn test_backoff_ctx_aware() {
    let mut bo = Backoffer::default();

    // 无 Context：走 thread::sleep，至少等待 BACKOFF_MIN*2。
    let started = Instant::now();
    bo.backoff(None).unwrap();
    assert!(started.elapsed() >= Duration::from_millis(10));

    bo.reset();
    let ctx = Context::background();
    ctx.cancel();
    let started = Instant::now();
    assert!(matches!(bo.backoff(Some(&ctx)), Err(AutoIdError::Canceled)));
    assert!(started.elapsed() < Duration::from_millis(10));

    // 等待中被取消也应尽快返回。
    bo.reset();
    let ctx = Context::background();
    let cancel_ctx = ctx.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(5));
        cancel_ctx.cancel();
    });
    let started = Instant::now();
    let _ = bo.backoff(Some(&ctx));
    assert!(started.elapsed() < Duration::from_millis(50));
}

#[test]
fn go_merge_12_rpc_retry_recovers_and_service_error_does_not_retry() {
    let transient = Arc::new(FakeClient::default());
    transient
        .alloc_results
        .lock()
        .unwrap()
        .push_back(Err(AutoIdError::Rpc("unavailable".into())));
    transient
        .alloc_results
        .lock()
        .unwrap()
        .push_back(Ok(AutoIdResponse {
            min: 100,
            max: 101,
            errmsg: String::new(),
        }));
    let alloc = new_test_single_point_alloc(transient.clone());
    assert_eq!(
        alloc.alloc(&Context::background(), 1, 1, 1).unwrap(),
        (100, 101)
    );
    assert_eq!(transient.alloc_calls.load(Ordering::SeqCst), 2);

    let service = Arc::new(FakeClient::default());
    service
        .alloc_results
        .lock()
        .unwrap()
        .push_back(Err(AutoIdError::Service("denied".into())));
    let alloc = new_test_single_point_alloc(service.clone());
    assert!(matches!(
        alloc.alloc(&Context::background(), 1, 1, 1),
        Err(AutoIdError::Service(_))
    ));
    assert_eq!(service.alloc_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn rpc_retry_limit_recovery_and_logging_follow_request_outcome() {
    const SCENARIO: &str = "ASTERSQL_AUTOID_RETRY_SCENARIO";
    if let Ok(scenario) = std::env::var(SCENARIO) {
        let (operation, outcome) = scenario.split_once(':').unwrap();
        let client = Arc::new(FakeClient::default());
        let failures = if outcome == "limit" {
            3
        } else if matches!(outcome, "recover" | "failed" | "canceled") {
            2
        } else {
            0
        };
        for index in 0..failures {
            let error = AutoIdError::Rpc(format!("rpc error: mixed failure {index} at 100%"));
            if operation == "alloc" {
                client.alloc_results.lock().unwrap().push_back(Err(error));
            } else {
                client.rebase_results.lock().unwrap().push_back(Err(error));
            }
        }
        if matches!(outcome, "failed" | "canceled") {
            let error = if outcome == "canceled" {
                AutoIdError::Canceled
            } else {
                AutoIdError::Service("local validation failed".into())
            };
            if operation == "alloc" {
                client.alloc_results.lock().unwrap().push_back(Err(error));
            } else {
                client.rebase_results.lock().unwrap().push_back(Err(error));
            }
        }
        let ctx = Context::background();
        if outcome.starts_with("local") {
            if outcome == "local-canceled" {
                ctx.cancel();
            }
            let error = AutoIdError::Service("local validation failed".into());
            client
                .alloc_results
                .lock()
                .unwrap()
                .push_back(Err(error.clone()));
            client.rebase_results.lock().unwrap().push_back(Err(error));
        }
        let discover = Arc::new(ClientDiscover::new(
            Arc::new(FakeDiscovery),
            Arc::new(FakeConnector {
                client: client.clone(),
            }),
        ));
        discover.seed_client_for_test(client.clone());
        let mut alloc = SinglePointAllocator::new(11, 22, false, NULLSPACE_ID, discover.clone());
        alloc.set_retry_policy_for_test(3, Duration::ZERO);
        if matches!(outcome, "recover" | "success") {
            client
                .alloc_results
                .lock()
                .unwrap()
                .push_back(Ok(AutoIdResponse {
                    min: 100,
                    max: 101,
                    errmsg: String::new(),
                }));
        }
        let result = if operation == "alloc" {
            let result = alloc.alloc(&ctx, 1, 1, 1);
            if matches!(outcome, "recover" | "success") {
                assert_eq!(result, Ok((100, 101)));
            }
            result.map(|_| ())
        } else {
            alloc.rebase(&ctx, 100, false)
        };
        if outcome == "limit" {
            let error = result.unwrap_err();
            assert!(is_rpc_retry_limit_error(&error));
            assert!(error.to_string().contains("3 RPC errors"));
            assert!(error.to_string().contains("mixed failure 2 at 100%"));
            assert!(error.to_string().contains("db_id=11, table_id=22"));
            assert!(error.to_string().contains("then retry the statement"));
        } else if outcome == "canceled" {
            assert_eq!(result, Err(AutoIdError::Canceled));
        } else if outcome == "failed" || outcome.starts_with("local") {
            assert_eq!(
                result,
                Err(AutoIdError::Service("local validation failed".into()))
            );
        } else {
            result.unwrap();
        }
        let calls = if operation == "alloc" {
            client.alloc_calls.load(Ordering::SeqCst)
        } else {
            client.rebase_calls.load(Ordering::SeqCst)
        };
        assert_eq!(calls, if outcome == "limit" { 3 } else { failures + 1 });

        assert_eq!(discover.version(), failures as u64);
        if matches!(
            outcome,
            "limit" | "failed" | "canceled" | "local" | "local-canceled"
        ) {
            assert_eq!(alloc.base(), 0);
        } else {
            assert_eq!(alloc.base(), if operation == "alloc" { 101 } else { 100 });
        }
        if operation == "alloc" {
            assert!(
                client
                    .alloc_requests
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|request| request
                        == &AutoIdRequest {
                            database_id: 11,
                            table_id: 22,
                            n: 1,
                            increment: 1,
                            offset: 1,
                            is_unsigned: false,
                            keyspace_id: NULLSPACE_ID,
                        })
            );
        } else {
            assert!(
                client
                    .rebase_requests
                    .lock()
                    .unwrap()
                    .iter()
                    .all(|request| request
                        == &RebaseRequest {
                            database_id: 11,
                            table_id: 22,
                            base: 100,
                            force: false,
                            is_unsigned: false,
                        })
            );
        }
        return;
    }
    for operation in ["alloc", "rebase"] {
        for outcome in [
            "limit",
            "recover",
            "failed",
            "canceled",
            "local",
            "local-canceled",
            "success",
        ] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "autoid_service_test::rpc_retry_limit_recovery_and_logging_follow_request_outcome", "--nocapture"])
                .env(SCENARIO, format!("{operation}:{outcome}"))
                .output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let logs = String::from_utf8(output.stderr).unwrap();
            assert_eq!(
                logs.matches("autoid request entered RPC retry").count(),
                usize::from(matches!(
                    outcome,
                    "limit" | "recover" | "failed" | "canceled"
                ))
            );
            assert_eq!(
                logs.matches("autoid request stopped after reaching RPC retry limit")
                    .count(),
                usize::from(outcome == "limit")
            );
            assert_eq!(
                logs.matches("autoid request completed after RPC retry")
                    .count(),
                usize::from(matches!(outcome, "recover" | "failed" | "canceled"))
            );
            if matches!(outcome, "limit" | "recover" | "failed" | "canceled") {
                assert!(logs.contains(&format!("operation={operation}")));
                assert!(logs.contains("keyspace-id=4294967295 db-id=11 table-id=22"));
                assert_eq!(logs.matches("autoid-request-id=1").count(), 2);
                assert!(logs.contains(match outcome {
                    "limit" => "outcome=fast-failed",
                    "failed" => "outcome=failed",
                    "canceled" => "outcome=context-canceled",
                    _ => "outcome=recovered",
                }));
                if outcome == "failed" {
                    assert!(logs.contains("error=autoid service error: local validation failed"));
                }
                if outcome == "canceled" {
                    assert!(logs.contains("error=context canceled"));
                }
            }
        }
    }
}

#[test]
fn rpc_retry_policy_requires_both_count_and_elapsed_duration() {
    use crate::autoid_service::{RpcRetryPolicy, RpcRetryState};
    let mut allocator = new_test_single_point_alloc(Arc::new(FakeClient::default()));
    for min_errors in [10, 0] {
        if min_errors == 0 {
            allocator.set_retry_policy_for_test(0, Duration::ZERO);
        }
        let defaults = allocator.effective_retry_policy();
        assert_eq!(defaults.min_errors, 10);
        assert_eq!(defaults.min_duration, Duration::from_secs(15));
    }
    let policy = RpcRetryPolicy {
        min_errors: 3,
        min_duration: Duration::from_secs(2),
    };
    let start = Instant::now();
    let mut state = RpcRetryState::default();
    assert!(!state.observe(start, policy));
    assert!(!state.observe(start + Duration::from_secs(1), policy));
    assert!(state.observe(start + Duration::from_secs(2), policy));
    assert_eq!(state.errors, 3);
    assert_eq!(state.first_error, Some(start));
    let mut count_only = RpcRetryState::default();
    for _ in 0..3 {
        assert!(!count_only.observe(start, policy));
    }
    let mut duration_only = RpcRetryState::default();
    assert!(!duration_only.observe(start, policy));
    assert!(!duration_only.observe(start + Duration::from_secs(3), policy));
}

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
use std::sync::{Arc, Mutex};
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
    alloc_calls: AtomicUsize,
    rebase_calls: AtomicUsize,
}

impl AutoIdClient for FakeClient {
    fn alloc_auto_id(&self, _ctx: &Context, _request: AutoIdRequest) -> Result<AutoIdResponse> {
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

    fn rebase(&self, _ctx: &Context, _request: RebaseRequest) -> Result<RebaseResponse> {
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

    // 无 Context：走 thread::sleep，间隔约 BACKOFF_MIN*2。
    let started = Instant::now();
    bo.backoff(None).unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(4));
    assert!(elapsed < Duration::from_millis(80));

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

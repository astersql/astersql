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

//! Go-equivalent tests from `import_mode_switcher_test.go`.
//!
//! Mapping:
//! - `TestRestorePreWork` → `test_restore_pre_work`
//!
//! Go spins a real ImportSST gRPC server and fake PD HTTP client. Darwin restore
//! crate has no kvproto/grpcio; the same SwitchMode boundary is exercised via
//! `RecordingImportSstSwitcher` + `MemPdClient` + `MemConnMgr` (same call order:
//! import-mode switch → remove schedulers → wait for ≥3 SwitchMode → post-work
//! restores Normal mode and clears pause state).
//!
//! 模块职责：对照 Go TestRestorePreWork，验证离线 PreWork 切 Import、摘 scheduler，
//! 后台刷新至少 3 次 SwitchMode，PostWork 切回 Normal。
//! 约束：无真实 gRPC；用 Recording 桩记录调用顺序与地址。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use crate::import_mode_switcher::{NewImportModeSwitcher, RestorePostWork, RestorePreWork};
use crate::stubs::{
    ClusterConfig, Context, MemConnMgr, MemPdClient, RecordingImportSstSwitcher, import_sstpb,
    metapb,
};

/// `test_restore_pre_work` ↔ Go `TestRestorePreWork`.
/// 单 TiKV store + 200ms 刷新间隔，覆盖初始 Import、周期性刷新与收尾 Normal。
#[test]
fn test_restore_pre_work() {
    let ctx = Context::Background();

    // Go: one TiKV store behind a mock ImportSST server (count=3 SwitchMode).
    // 单 store 无 TiFlash 标签，SkipTiFlash 不会过滤它。
    // 地址固定 20160，后续断言 SwitchMode 目标地址。
    let stores = vec![metapb::Store {
        Id: 1,
        Address: "127.0.0.1:20160".into(),
        Labels: vec![],
    }];
    let pd = Arc::new(MemPdClient::new(stores));
    // Recording 桩记录 (addr, mode) 调用序列供轮询断言。
    let switcher_impl = Arc::new(RecordingImportSstSwitcher::new());
    // Go: NewImportModeSwitcher(pdClient, 200ms, nil)
    // 200ms 间隔使测试在数秒内观察到多次刷新。
    let mut switcher = NewImportModeSwitcher(
        pd.clone(),
        Duration::from_millis(200),
        switcher_impl.clone(),
    );

    // Go: mgr.PdController with SchedulerPauseTTL + RemoveSchedulersWithConfig.
    // origin 预置三类 balance scheduler，供快照断言。
    let mgr = MemConnMgr {
        origin: std::sync::Mutex::new(ClusterConfig {
            Schedulers: vec![
                "balance-leader-scheduler".into(),
                "balance-hot-region-scheduler".into(),
                "balance-region-scheduler".into(),
            ],
            RuleID: String::new(),
        }),
        ..Default::default()
    };

    // is_online=false, switch_to_import=true：完整离线预热路径。
    let (undo, cfg) = RestorePreWork(&ctx, &mgr, &mut switcher, false, true)
        .expect("Go require.NoError RestorePreWork");

    // check the cfg — Go asserts paused schedulers / schedule cfg snapshot.
    // 返回的 ClusterConfig 调度器列表应来自 origin 快照。
    {
        let cfg = cfg.expect("offline RestorePreWork returns ClusterConfig");
        assert_eq!(cfg.Schedulers.len(), 3);
        for key in &cfg.Schedulers {
            assert!(
                mgr.origin.lock().unwrap().Schedulers.contains(key),
                "scheduler {key} must be from origin snapshot"
            );
        }
        assert!(mgr.remove_called.load(Ordering::SeqCst));
        // Initial SwitchMode(Import) for the single non-tiflash store.
        // PreWork 返回前必须已对 20160 发过至少一次 Import。
        let calls = switcher_impl.calls.lock().unwrap().clone();
        assert!(
            calls
                .iter()
                .any(|(a, m)| a == "127.0.0.1:20160" && *m == import_sstpb::SwitchMode::Import),
            "expected initial Import switch, got {calls:?}"
        );
    }

    // Go: `<-ch` after mock server receives 3 SwitchMode RPCs (initial + refresh).
    // 等待初始 + 至少两次刷新，证明后台循环在跑。
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let n = switcher_impl
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, m)| *m == import_sstpb::SwitchMode::Import)
            .count();
        // 达到 3 次 Import 即认为刷新循环健康。
        if n >= 3 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for 3 Import SwitchMode calls (Go mock count=3)"
        );
        // 短睡轮询，避免忙等占满 CPU。
        thread::sleep(Duration::from_millis(20));
    }

    // 收尾：停刷新、切 Normal，并调用 undo（MemConnMgr 可为 nop）。
    RestorePostWork(ctx.clone(), &mut switcher, undo, false);

    // check the cfg done — Go asserts schedule cfg restored and delay schedulers cleared.
    // PostWork 后调用记录中必须出现 Normal；remove_called 仍为 true。
    {
        let calls = switcher_impl.calls.lock().unwrap().clone();
        assert!(
            calls
                .iter()
                .any(|(a, m)| a == "127.0.0.1:20160" && *m == import_sstpb::SwitchMode::Normal),
            "expected Normal switch after RestorePostWork, got {calls:?}"
        );
        // Undo from MemConnMgr is nop but must be invokable without error (Go undo runs).
        // 桩 undo 不改状态，但 PreWork 阶段的 remove 标记应保留。
        // Post-work already invoked undo; verify remove was recorded once during pre-work.
        assert!(mgr.remove_called.load(Ordering::SeqCst));
    }
}

/// Go's ticker `select` wakes as soon as `SwitchToNormalMode` cancels the
/// refresh context; it must not wait for the whole refresh interval.
#[test]
fn switch_to_normal_wakes_sleeping_refresh_immediately() {
    let ctx = Context::Background();
    let pd = Arc::new(MemPdClient::new(vec![metapb::Store {
        Id: 1,
        Address: "127.0.0.1:20160".into(),
        Labels: vec![],
    }]));
    let switcher_impl = Arc::new(RecordingImportSstSwitcher::new());
    let mut switcher = NewImportModeSwitcher(pd, Duration::from_secs(2), switcher_impl);

    switcher
        .GoSwitchToImportMode(&ctx)
        .expect("initial import switch succeeds");
    thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    switcher
        .SwitchToNormalMode(&ctx)
        .expect("normal switch succeeds");

    assert!(
        started.elapsed() < Duration::from_millis(500),
        "cancellation should wake the refresh loop immediately; elapsed {:?}",
        started.elapsed()
    );
}

#[test]
fn test_restore_post_work_online_skips_normal_mode() {
    let ctx = Context::Background();
    let pd = Arc::new(MemPdClient::new(vec![metapb::Store {
        Id: 1,
        Address: "store-online".into(),
        Labels: vec![],
    }]));
    let transport = Arc::new(RecordingImportSstSwitcher::new());
    let mut mode = NewImportModeSwitcher(pd, Duration::from_secs(3600), transport.clone());
    mode.GoSwitchToImportMode(&ctx).unwrap();
    let restored = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = restored.clone();
    RestorePostWork(
        ctx.clone(),
        &mut mode,
        Arc::new(move |ctx: &Context| {
            assert!(ctx.Err().is_none());
            observed.store(true, Ordering::SeqCst);
            Ok(())
        }),
        true,
    );
    let calls = transport.calls.lock().unwrap().clone();
    mode.SwitchToNormalMode(&ctx).unwrap();
    assert!(restored.load(Ordering::SeqCst));
    assert_eq!(
        calls,
        vec![("store-online".into(), import_sstpb::SwitchMode::Import)]
    );
}

#[derive(Clone)]
struct NetworkModeServer(Arc<std::sync::Mutex<Vec<kvproto::import_sstpb::SwitchMode>>>);
impl kvproto::import_sstpb_grpc::ImportSst for NetworkModeServer {
    fn switch_mode(
        &mut self,
        ctx: grpcio::RpcContext,
        request: kvproto::import_sstpb::SwitchModeRequest,
        sink: grpcio::UnarySink<kvproto::import_sstpb::SwitchModeResponse>,
    ) {
        self.0.lock().unwrap().push(request.get_mode());
        ctx.spawn(async move {
            sink.success(kvproto::import_sstpb::SwitchModeResponse::default())
                .await
                .unwrap();
        });
    }
}

#[test]
fn grpc_mode_transport_sends_import_and_normal_to_real_server() {
    use crate::import_mode_switcher::GrpcImportSstSwitcher;
    use crate::stubs::ImportSstSwitcher;
    let environment = Arc::new(grpcio::Environment::new(1));
    let modes = Arc::new(std::sync::Mutex::new(vec![]));
    let service = kvproto::import_sstpb_grpc::create_import_sst(NetworkModeServer(modes.clone()));
    let mut server = grpcio::ServerBuilder::new(environment.clone())
        .register_service(service)
        .build()
        .unwrap();
    let port = server
        .add_listening_port("127.0.0.1:0", grpcio::ServerCredentials::insecure())
        .unwrap();
    server.start();
    let transport = GrpcImportSstSwitcher {
        environment,
        credentials: None,
    };
    let address = format!("127.0.0.1:{port}");
    transport
        .SwitchMode(
            &Context::Background(),
            &address,
            import_sstpb::SwitchMode::Import,
        )
        .unwrap();
    transport
        .SwitchMode(
            &Context::Background(),
            &address,
            import_sstpb::SwitchMode::Normal,
        )
        .unwrap();
    assert_eq!(
        *modes.lock().unwrap(),
        vec![
            kvproto::import_sstpb::SwitchMode::Import,
            kvproto::import_sstpb::SwitchMode::Normal
        ]
    );
    drop(transport);
    drop(server);
}

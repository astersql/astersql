// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent of `br/pkg/mock/mock_cluster_test.go`.
//!
//! Go package is external `mock_test`; Rust wires this as an in-crate test
//! module. `Stop` is checked to close Domain/Storage/Server.
//!
//! 冒烟契约：对齐 Go `TestSmoke`——NewCluster → Start → Stop 无错，
//! 并断言 Domain/Storage/Server/HttpServer 在 Stop 后已关闭。

use std::sync::atomic::Ordering;

use crate::NewCluster;
use crate::stubs;

/// `TestSmoke` — create mock cluster, Start, Stop; no error and resources close.
///
/// 验证 mock 集群主路径与资源清理；钩子 reset/skip_sleep 避免真实等待干扰。
#[test]
fn test_smoke() {
    // 干净钩子 + 跳过 sleep；先 offline，Start 内再置 online。
    stubs::reset_test_hooks();
    stubs::set_skip_sleep(true);
    // Start() sets online before waitUntilServerOnline probes succeed.
    stubs::set_cluster_online(false);

    // NewCluster 后应已有 Storage/Domain/PD 与已 bootstrap 的 Cluster。
    let mut m = NewCluster().expect("Go require.NoError NewCluster");
    assert!(m.Storage.is_some());
    assert!(m.Domain.is_some());
    assert!(m.PDClient.is_some());
    assert!(m.PDHTTPCli.is_some());
    assert!(
        m.Cluster.as_ref().unwrap().bootstrapped,
        "BootstrapWithSingleStore must run"
    );

    // Start 后 DSN 非空，且 Server/驱动已挂接。
    m.Start().expect("Go require.NoError Start");
    assert!(!m.DSN.is_empty(), "Start must set DSN after server online");
    assert!(m.Server.is_some());
    assert!(m.TiDBDriver.is_some());

    // 提前克隆 closed 标志，Stop 后仍可断言（结构体字段可能被消费式清理）。
    let domain_closed = m.Domain.as_ref().unwrap().closed.clone();
    let storage_closed = m.Storage.as_ref().unwrap().closed.clone();
    let server_closed = m.Server.as_ref().unwrap().closed.clone();
    let http_closed = m.HttpServer.as_ref().map(|h| h.closed.clone());

    // Explicit cleanup is the observable resource-lifecycle contract.
    // Stop 必须关闭 Domain/Storage/Server，以及进程级 pprof HttpServer。
    m.Stop();
    assert!(
        domain_closed.load(Ordering::SeqCst),
        "Stop must close Domain"
    );
    assert!(
        storage_closed.load(Ordering::SeqCst),
        "Stop must close Storage"
    );
    assert!(
        server_closed.load(Ordering::SeqCst),
        "Stop must close Server"
    );
    if let Some(http_closed) = http_closed {
        assert!(
            http_closed.load(Ordering::SeqCst),
            "Stop must close HttpServer (pprof)"
        );
    }

    // 恢复默认钩子，避免污染同进程后续用例。
    stubs::reset_test_hooks();
}

/// Go's `pprofOnce.Do` closure assigns the HTTP server only to the `Cluster`
/// instance which actually performs the one-time initialization.
#[test]
fn pprof_server_is_not_shared_by_later_clusters() {
    let mut first = NewCluster().expect("first NewCluster");
    let mut second = NewCluster().expect("second NewCluster");

    assert!(
        first.HttpServer.is_none() || second.HttpServer.is_none(),
        "sync.Once must not attach the same pprof server to every Cluster"
    );

    first.Stop();
    second.Stop();
}

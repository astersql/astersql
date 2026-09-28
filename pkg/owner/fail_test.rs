// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Owner 选举失败路径测试：模拟 etcd 传输层断开后，新建 session / Campaign 应失败。
//
// 对应 Go failpoint 关闭 client / gRPC：分别覆盖连接拒绝与 accept 后立即关闭两种场景。

use std::time::Duration;

use etcd_client::{Client, ConnectOptions};
use owner::{Context, NewOwnerManager, OwnerError};
use tokio::net::TcpListener;

/// 启动本地假监听：可选在 accept 后立刻 drop 连接，并返回已连上的 etcd Client。
async fn disconnected_client(close_after_accept: bool) -> (Client, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        if close_after_accept {
            // 接受连接后立即关闭，模拟 gRPC 层被掐断。
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        }
    });
    let options = ConnectOptions::new()
        .with_connect_timeout(Duration::from_millis(300))
        .with_timeout(Duration::from_millis(300));
    let client = Client::connect([endpoint], Some(options)).await.unwrap();
    (client, server)
}

/// 在传输已坏的前提下竞选 Owner，期望在有限重试内返回 Etcd 错误。
async fn assert_new_session_fails_after_transport_close(close_after_accept: bool) {
    let (client, server) = disconnected_client(close_after_accept).await;
    if !close_after_accept {
        // 未 accept 即 abort：后续请求表现为连接拒绝。
        server.abort();
    }
    let manager = NewOwnerManager(
        Context::new(),
        client,
        "fail_new_session",
        "node-1",
        "/task-547/fail-new-session",
    );
    let result = tokio::time::timeout(Duration::from_secs(5), manager.CampaignOwner(&[]))
        .await
        .expect("session retry must be bounded");
    assert!(matches!(result, Err(OwnerError::Etcd(_))), "{result:?}");
    manager.Close().await;
    server.abort();
}

/// 同时覆盖连接拒绝与 accept-then-close 两条失败路径。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_fail_new_session() {
    // Go's closeClient and closeGrpc failpoints close two different transport
    // layers. Exercise both connection-refused and accepted-then-closed paths.
    // 对应 Go 的 closeClient / closeGrpc 两个 failpoint。
    assert_new_session_fails_after_transport_close(false).await;
    assert_new_session_fails_after_transport_close(true).await;
}

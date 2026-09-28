// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// TopSQL PubSub 的轻量 mock 服务端。
//
// 在回环地址绑定动态端口，用 watch 通道模拟 gRPC Server 启停；
// 仅接受 TCP 连接而不实现完整 protobuf，供集成测试验证生命周期。

#![allow(non_snake_case, non_camel_case_types)]

use std::future::Future;
use std::io;
use std::net::TcpListener;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio_stream::wrappers::TcpListenerStream;

use crate::tipb::top_sql_pub_sub_server::{TopSqlPubSub, TopSqlPubSubServer};

type ServeFuture = Pin<Box<dyn Future<Output = Result<(), tonic::transport::Error>> + Send>>;
type ServiceFactory =
    Box<dyn FnOnce(tokio::net::TcpListener, watch::Receiver<bool>) -> ServeFuture + Send>;

/// A clonable handle corresponding to Go's `*grpc.Server` for lifecycle use.
/// 可克隆句柄，对应 Go 的 `*grpc.Server`，用于判断/触发停止。
#[derive(Clone)]
pub struct MockGrpcServerHandle {
    stop: watch::Sender<bool>,
    service: Arc<Mutex<Option<ServiceFactory>>>,
}

impl MockGrpcServerHandle {
    /// 若已收到停止信号则返回 true。
    pub fn is_stopped(&self) -> bool {
        *self.stop.borrow()
    }

    /// Register the generated TopSQL PubSub service before `Serve`, matching
    /// Go's `tipb.RegisterTopSQLPubSubServer(server.Server(), service)` step.
    pub fn register_top_sql_pub_sub<T>(&self, service: T) -> io::Result<()>
    where
        T: TopSqlPubSub,
    {
        let factory: ServiceFactory = Box::new(move |listener, mut stopped| {
            Box::pin(async move {
                tonic::transport::Server::builder()
                    .add_service(TopSqlPubSubServer::new(service))
                    .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async move {
                        if *stopped.borrow() {
                            return;
                        }
                        while stopped.changed().await.is_ok() {
                            if *stopped.borrow() {
                                return;
                            }
                        }
                    })
                    .await
            })
        });
        let mut slot = self
            .service
            .lock()
            .map_err(|_| io::Error::other("mock pubsub service lock poisoned"))?;
        if slot.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "TopSQL PubSub service already registered",
            ));
        }
        *slot = Some(factory);
        Ok(())
    }

    /// Stops the serving loop. Cloned handles retain the Go `*grpc.Server`
    /// ability to stop the server independently of the wrapper.
    pub fn Stop(&self) {
        let _ = self.stop.send(true);
    }
}

/// Test server bound to an OS-assigned loopback port.
/// 绑定 OS 分配回环端口的测试用 publisher 服务。
pub struct mockPubSubServer {
    listen: Option<TcpListener>,
    grpcServer: MockGrpcServerHandle,
    addr: String,
    serving: bool,
}

/// Creates a mock publisher server and reserves its listening address.
/// 创建 mock publisher 并预留监听地址（`127.0.0.1:0`）。
pub fn NewMockPubSubServer() -> io::Result<mockPubSubServer> {
    let listen = TcpListener::bind("127.0.0.1:0")?;
    listen.set_nonblocking(true)?;
    let addr = listen.local_addr()?.to_string();
    let (stop, _) = watch::channel(false);
    let service = Arc::new(Mutex::new(None));
    Ok(mockPubSubServer {
        listen: Some(listen),
        grpcServer: MockGrpcServerHandle { stop, service },
        addr,
        serving: false,
    })
}

impl mockPubSubServer {
    /// Starts the listener loop. As in Go, errors after startup are not returned.
    /// 启动监听循环；与 Go 一样，启动后的 accept 错误不向外返回。
    pub fn Serve(&mut self) -> io::Result<()> {
        if self.serving {
            return Ok(());
        }
        let Some(listen) = self.listen.take() else {
            return Ok(());
        };
        let listener = tokio::net::TcpListener::from_std(listen)?;
        let stopped = self.grpcServer.stop.subscribe();
        let service = self
            .grpcServer
            .service
            .lock()
            .map_err(|_| io::Error::other("mock pubsub service lock poisoned"))?
            .take();
        if let Some(factory) = service {
            tokio::spawn(async move {
                if let Err(error) = factory(listener, stopped).await {
                    log::warn!(target: "top-sql", "mock pubsub server serve failed: {error}");
                }
            });
        } else {
            let mut stopped = stopped;
            // An unregistered Go grpc.Server still accepts connections. Keep
            // that lifecycle behavior for callers that only exercise binding.
            tokio::spawn(async move {
                if *stopped.borrow() {
                    return;
                }
                loop {
                    tokio::select! {
                        changed = stopped.changed() => {
                            if changed.is_err() || *stopped.borrow() {
                                break;
                            }
                        }
                        accepted = listener.accept() => {
                            if accepted.is_err() {
                                break;
                            }
                        }
                    }
                }
            });
        }
        self.serving = true;
        Ok(())
    }

    /// 返回可克隆的 gRPC 生命周期句柄。
    pub fn Server(&self) -> MockGrpcServerHandle {
        self.grpcServer.clone()
    }

    /// 返回监听地址字符串。
    pub fn Address(&self) -> String {
        self.addr.clone()
    }

    /// 发送停止信号并释放 listener。
    pub fn Stop(&mut self) {
        self.grpcServer.Stop();
        self.listen.take();
    }
}

impl Drop for mockPubSubServer {
    fn drop(&mut self) {
        self.Stop();
    }
}

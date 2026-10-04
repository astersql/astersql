// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 外部工作负载控制器 gRPC 客户端的集成测试。
//
// 启动内存 tonic stub server，验证：
// - 各 RPC 往返字段与请求头（keyspace / tidb_pool）；
// - 一元拦截器执行次数；
// - 控制器 Paused / Unknown 错误映射；
// - 选项校验与地址规范化。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Client, ClientError, Context, Option as ClientOption, UnaryClientInterceptor, pb};
use pb::external_workload_controller_server::{
    ExternalWorkloadController, ExternalWorkloadControllerServer,
};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status};

/// stub server 记录的各 RPC 请求体，供断言字段透传。
#[derive(Default)]
struct Received {
    register_gc: Option<pb::RegisterGcv2Request>,
    recycle_gc: Option<pb::RecycleGcv2Request>,
    update_gc: Option<pb::UpdateGcLifeTimeRequest>,
    register_ttl: Option<pb::RegisterTtlTaskRequest>,
    delete_ttl: Option<pb::DeleteTtlTableInfoRequest>,
    recycle_ttl: Option<pb::RecycleTtlTaskRequest>,
    update_ttl: Option<pb::UpdateTtlJobEnableRequest>,
    register_analyze: Option<pb::RegisterAutoAnalyzeRequest>,
    recycle_analyze: Option<pb::RecycleAutoAnalyzeRequest>,
}

/// 可注入 Ping 错误、并记录收到请求的控制器 stub。
#[derive(Default)]
struct StubServer {
    /// 各状态变更 RPC 的最近一次请求。
    received: Arc<Mutex<Received>>,
    /// Ping 响应中返回的业务错误（Paused 等）。
    ping_error: Arc<Mutex<Option<pb::Error>>>,
}

/// 构造空成功响应。
fn ok_response() -> Result<Response<pb::Response>, Status> {
    Ok(Response::new(pb::Response::default()))
}

#[tonic::async_trait]
impl ExternalWorkloadController for StubServer {
    async fn ping(&self, _: Request<pb::PingRequest>) -> Result<Response<pb::Response>, Status> {
        Ok(Response::new(pb::Response {
            error: self.ping_error.lock().unwrap().clone(),
        }))
    }

    async fn register_gcv2(
        &self,
        request: Request<pb::RegisterGcv2Request>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().register_gc = Some(request.into_inner());
        ok_response()
    }

    async fn recycle_gcv2(
        &self,
        request: Request<pb::RecycleGcv2Request>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().recycle_gc = Some(request.into_inner());
        ok_response()
    }

    async fn update_gc_life_time(
        &self,
        request: Request<pb::UpdateGcLifeTimeRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().update_gc = Some(request.into_inner());
        ok_response()
    }

    async fn register_ttl_task(
        &self,
        request: Request<pb::RegisterTtlTaskRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().register_ttl = Some(request.into_inner());
        ok_response()
    }

    async fn delete_ttl_table_info(
        &self,
        request: Request<pb::DeleteTtlTableInfoRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().delete_ttl = Some(request.into_inner());
        ok_response()
    }

    async fn recycle_ttl_task(
        &self,
        request: Request<pb::RecycleTtlTaskRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().recycle_ttl = Some(request.into_inner());
        ok_response()
    }

    async fn update_ttl_job_enable(
        &self,
        request: Request<pb::UpdateTtlJobEnableRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().update_ttl = Some(request.into_inner());
        ok_response()
    }

    async fn register_auto_analyze(
        &self,
        request: Request<pb::RegisterAutoAnalyzeRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().register_analyze = Some(request.into_inner());
        ok_response()
    }

    async fn recycle_auto_analyze(
        &self,
        request: Request<pb::RecycleAutoAnalyzeRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().recycle_analyze = Some(request.into_inner());
        ok_response()
    }
}

/// 已启动的 stub server 与已连接客户端的句柄集合。
struct RunningStub {
    /// 指向 stub 的客户端。
    client: Box<dyn Client>,
    /// 与 stub 共享的请求记录。
    received: Arc<Mutex<Received>>,
    /// 与 stub 共享的 Ping 错误注入点。
    ping_error: Arc<Mutex<Option<pb::Error>>>,
    /// 后台 server 任务，测试结束需 abort。
    server: JoinHandle<()>,
}

/// 在随机端口启动 stub，并按给定拦截器构造客户端。
async fn start_stub_server(interceptors: Vec<UnaryClientInterceptor>) -> RunningStub {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stub = StubServer::default();
    let received = stub.received.clone();
    let ping_error = stub.ping_error.clone();
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ExternalWorkloadControllerServer::new(stub))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    let client = extworkload_client::New(Some(&ClientOption {
        KeyspaceID: 42,
        KeyspaceName: "starter-ks".into(),
        TiDBPool: "starter-pool".into(),
        ControllerAddr: format!("http://{address}"),
        TLSConfig: None,
        Interceptors: interceptors,
    }))
    .unwrap();
    RunningStub {
        client,
        received,
        ping_error,
        server,
    }
}

/// 测试用短超时 context。
fn new_test_context() -> Context {
    Context::with_timeout(Duration::from_secs(3))
}

/// 断言请求头携带 stub 启动时写入的 keyspace / pool 身份。
fn require_header(header: &Option<pb::RequestHeader>) {
    let header = header.as_ref().expect("RPC request header");
    assert_eq!(
        header.keyspace,
        Some(pb::request_header::Keyspace::KeyspaceId(42))
    );
    assert_eq!(header.keyspace_name, "starter-ks");
    assert_eq!(header.tidb_pool, "starter-pool");
}

/// 全量 RPC 往返：校验请求体字段与公共请求头。
#[tokio::test]
async fn test_client_round_trip() {
    let mut running = start_stub_server(Vec::new()).await;
    let ctx = new_test_context();
    running.client.Ping(&ctx).await.unwrap();
    running.client.RegisterGCV2(&ctx, 12, 600).await.unwrap();
    running.client.RecycleGCV2(&ctx, 1234).await.unwrap();
    running.client.UpdateGCLifeTime(&ctx, 3600).await.unwrap();
    running
        .client
        .RegisterTTLTask(&ctx, 11, true)
        .await
        .unwrap();
    running.client.DeleteTTLTableInfo(&ctx, 12).await.unwrap();
    running.client.RecycleTTLTask(&ctx, 99).await.unwrap();
    running
        .client
        .UpdateTTLJobEnable(&ctx, false)
        .await
        .unwrap();
    running.client.RegisterAutoAnalyze(&ctx, 7).await.unwrap();
    running.client.RecycleAutoAnalyze(&ctx, 8).await.unwrap();

    let got = running.received.lock().unwrap();
    let request = got.register_gc.as_ref().unwrap();
    require_header(&request.header);
    assert_eq!((request.safe_point, request.gc_life_time), (12, 600));
    let request = got.recycle_gc.as_ref().unwrap();
    require_header(&request.header);
    assert_eq!(request.safe_point, 1234);
    let request = got.update_gc.as_ref().unwrap();
    require_header(&request.header);
    assert_eq!(request.gc_life_time, 3600);
    let request = got.register_ttl.as_ref().unwrap();
    require_header(&request.header);
    assert_eq!((request.table_id, request.ttl_job_enable), (11, true));
    let request = got.delete_ttl.as_ref().unwrap();
    require_header(&request.header);
    assert_eq!(request.table_id, 12);
    let request = got.recycle_ttl.as_ref().unwrap();
    require_header(&request.header);
    assert_eq!(request.completed_job_create_time, 99);
    let request = got.update_ttl.as_ref().unwrap();
    require_header(&request.header);
    assert!(!request.ttl_job_enable);
    let request = got.register_analyze.as_ref().unwrap();
    require_header(&request.header);
    assert_eq!(request.task_id, 7);
    let request = got.recycle_analyze.as_ref().unwrap();
    require_header(&request.header);
    assert_eq!(request.task_id, 8);
    drop(got);

    running.client.Close().unwrap();
    running.server.abort();
}

/// 一元拦截器应在每次 RPC 前执行一次。
#[tokio::test]
async fn test_client_interceptor() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let interceptor: UnaryClientInterceptor = Arc::new(move |request| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(request)
    });
    let mut running = start_stub_server(vec![interceptor]).await;
    running.client.Ping(&new_test_context()).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    running.client.Close().unwrap();
    running.server.abort();
}

/// Paused 映射为 ControllerPaused；其它业务错误保留消息文本。
#[tokio::test]
async fn test_client_error_mapping() {
    let mut running = start_stub_server(Vec::new()).await;
    *running.ping_error.lock().unwrap() = Some(pb::Error {
        r#type: pb::ErrorType::Paused as i32,
        message: "paused".into(),
    });
    assert!(matches!(
        running.client.Ping(&new_test_context()).await,
        Err(ClientError::ControllerPaused)
    ));

    *running.ping_error.lock().unwrap() = Some(pb::Error {
        r#type: pb::ErrorType::Unknown as i32,
        message: "boom".into(),
    });
    let error = running.client.Ping(&new_test_context()).await.unwrap_err();
    assert!(!matches!(error, ClientError::ControllerPaused));
    assert!(error.to_string().contains("boom"));
    running.client.Close().unwrap();
    running.server.abort();
}

/// 空响应应映射为含 "empty response" 的错误。
#[test]
fn test_map_response_nil_response() {
    let error = extworkload_client::mapResponse("Ping", Ok(None)).unwrap_err();
    assert!(error.to_string().contains("empty response"));
}

/// New 拒绝 None、空地址与非法 URL。
#[test]
fn test_new_client_validation() {
    assert!(extworkload_client::New(None).is_err());
    assert!(extworkload_client::New(Some(&ClientOption::with_addr(""))).is_err());
    assert!(extworkload_client::New(Some(&ClientOption::with_addr("://bad"))).is_err());
}

/// normalizeAddr 剥离 http(s) 方案并校验主机端口形态。
#[test]
fn test_normalize_addr() {
    assert_eq!(
        extworkload_client::normalizeAddr("http://127.0.0.1:1234").unwrap(),
        "127.0.0.1:1234"
    );
    assert_eq!(
        extworkload_client::normalizeAddr("127.0.0.1:1234").unwrap(),
        "127.0.0.1:1234"
    );
    assert_eq!(
        extworkload_client::normalizeAddr("http://controller.example:80").unwrap(),
        "controller.example:80"
    );
    assert!(extworkload_client::normalizeAddr("http://").is_err());
}

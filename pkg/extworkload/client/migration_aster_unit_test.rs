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

// 客户端与 Go 行为对齐的迁移回归测试。
//
// 覆盖请求字段透传、拦截器与错误映射、选项校验及地址规范化，
// 确保机械迁移后的 Rust 客户端语义与原 Go 包一致。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::{Client, ClientError, Context, Option as ClientOption, UnaryClientInterceptor, pb};
use pb::external_workload_controller_server::{
    ExternalWorkloadController, ExternalWorkloadControllerServer,
};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status};

/// stub 记录的各 RPC 请求，用于字段级对齐断言。
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

/// 迁移测试用控制器 stub：记录请求并可注入 Ping 错误。
#[derive(Default)]
struct Stub {
    /// 最近一次各 RPC 请求体。
    received: Arc<Mutex<Received>>,
    /// Ping 响应中的业务错误。
    ping_error: Arc<Mutex<Option<pb::Error>>>,
}

#[tonic::async_trait]
impl ExternalWorkloadController for Stub {
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
        Ok(Response::new(pb::Response::default()))
    }
    async fn recycle_gcv2(
        &self,
        request: Request<pb::RecycleGcv2Request>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().recycle_gc = Some(request.into_inner());
        Ok(Response::new(pb::Response::default()))
    }
    async fn update_gc_life_time(
        &self,
        request: Request<pb::UpdateGcLifeTimeRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().update_gc = Some(request.into_inner());
        Ok(Response::new(pb::Response::default()))
    }
    async fn register_ttl_task(
        &self,
        request: Request<pb::RegisterTtlTaskRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().register_ttl = Some(request.into_inner());
        Ok(Response::new(pb::Response::default()))
    }
    async fn delete_ttl_table_info(
        &self,
        request: Request<pb::DeleteTtlTableInfoRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().delete_ttl = Some(request.into_inner());
        Ok(Response::new(pb::Response::default()))
    }
    async fn recycle_ttl_task(
        &self,
        request: Request<pb::RecycleTtlTaskRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().recycle_ttl = Some(request.into_inner());
        Ok(Response::new(pb::Response::default()))
    }
    async fn update_ttl_job_enable(
        &self,
        request: Request<pb::UpdateTtlJobEnableRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().update_ttl = Some(request.into_inner());
        Ok(Response::new(pb::Response::default()))
    }
    async fn register_auto_analyze(
        &self,
        request: Request<pb::RegisterAutoAnalyzeRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().register_analyze = Some(request.into_inner());
        Ok(Response::new(pb::Response::default()))
    }
    async fn recycle_auto_analyze(
        &self,
        request: Request<pb::RecycleAutoAnalyzeRequest>,
    ) -> Result<Response<pb::Response>, Status> {
        self.received.lock().unwrap().recycle_analyze = Some(request.into_inner());
        Ok(Response::new(pb::Response::default()))
    }
}

/// 启动 stub server 并返回已连接客户端与共享状态句柄。
async fn start_stub() -> (
    Box<dyn Client>,
    Arc<Mutex<Received>>,
    Arc<Mutex<Option<pb::Error>>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stub = Stub::default();
    let received = stub.received.clone();
    let ping_error = stub.ping_error.clone();
    tokio::spawn(async move {
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
        Interceptors: Vec::new(),
    }))
    .unwrap();
    (client, received, ping_error)
}

/// 断言状态变更 RPC 携带与 Go 一致的身份请求头。
fn assert_header(header: &Option<pb::RequestHeader>) {
    let header = header.as_ref().expect("state-changing RPC header");
    assert_eq!(header.keyspace_id, 42);
    assert_eq!(header.keyspace_name, "starter-ks");
    assert_eq!(header.tidb_pool, "starter-pool");
}

/// 全量 RPC：请求字段与 header 应与 Go 客户端一致。
#[tokio::test]
async fn migration_round_trip_preserves_all_go_request_fields() {
    let (mut client, received, _) = start_stub().await;
    let context = Context::with_timeout(Duration::from_secs(3));
    client.Ping(&context).await.unwrap();
    client.RegisterGCV2(&context, 12, 600).await.unwrap();
    client.RecycleGCV2(&context, 1234).await.unwrap();
    client.UpdateGCLifeTime(&context, 3600).await.unwrap();
    client.RegisterTTLTask(&context, 11, true).await.unwrap();
    client.DeleteTTLTableInfo(&context, 12).await.unwrap();
    client.RecycleTTLTask(&context, 99).await.unwrap();
    client.UpdateTTLJobEnable(&context, false).await.unwrap();
    client.RegisterAutoAnalyze(&context, 7).await.unwrap();
    client.RecycleAutoAnalyze(&context, 8).await.unwrap();

    let got = received.lock().unwrap();
    let request = got.register_gc.as_ref().unwrap();
    assert_header(&request.header);
    assert_eq!((request.safe_point, request.gc_life_time), (12, 600));
    let request = got.recycle_gc.as_ref().unwrap();
    assert_header(&request.header);
    assert_eq!(request.safe_point, 1234);
    let request = got.update_gc.as_ref().unwrap();
    assert_header(&request.header);
    assert_eq!(request.gc_life_time, 3600);
    let request = got.register_ttl.as_ref().unwrap();
    assert_header(&request.header);
    assert_eq!((request.table_id, request.ttl_job_enable), (11, true));
    let request = got.delete_ttl.as_ref().unwrap();
    assert_header(&request.header);
    assert_eq!(request.table_id, 12);
    let request = got.recycle_ttl.as_ref().unwrap();
    assert_header(&request.header);
    assert_eq!(request.completed_job_create_time, 99);
    let request = got.update_ttl.as_ref().unwrap();
    assert_header(&request.header);
    assert!(!request.ttl_job_enable);
    let request = got.register_analyze.as_ref().unwrap();
    assert_header(&request.header);
    assert_eq!(request.task_id, 7);
    let request = got.recycle_analyze.as_ref().unwrap();
    assert_header(&request.header);
    assert_eq!(request.task_id, 8);
}

/// 拦截器调用次数与 Paused/Unknown 错误映射应对齐 Go。
#[tokio::test]
async fn migration_interceptor_and_error_mapping_match_go() {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let interceptor: UnaryClientInterceptor = Arc::new(move |request| {
        counter.fetch_add(1, Ordering::SeqCst);
        Ok(request)
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stub = Stub::default();
    let ping_error = stub.ping_error.clone();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(ExternalWorkloadControllerServer::new(stub))
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    let mut client = extworkload_client::New(Some(&ClientOption {
        KeyspaceID: 0,
        KeyspaceName: String::new(),
        TiDBPool: String::new(),
        ControllerAddr: format!("http://{address}"),
        TLSConfig: None,
        Interceptors: vec![interceptor],
    }))
    .unwrap();
    let context = Context::with_timeout(Duration::from_secs(3));
    client.Ping(&context).await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    *ping_error.lock().unwrap() = Some(pb::Error {
        r#type: pb::ErrorType::Paused as i32,
        message: "paused".into(),
    });
    assert!(matches!(
        client.Ping(&context).await,
        Err(ClientError::ControllerPaused)
    ));
    *ping_error.lock().unwrap() = Some(pb::Error {
        r#type: pb::ErrorType::Unknown as i32,
        message: "boom".into(),
    });
    let error = client.Ping(&context).await.unwrap_err();
    assert!(error.to_string().contains("Ping") && error.to_string().contains("boom"));
}

/// 选项校验、地址规范化（含 IPv6）与空响应错误应对齐 Go。
#[test]
fn migration_validates_options_and_normalizes_addresses() {
    assert!(extworkload_client::New(None).is_err());
    assert!(extworkload_client::New(Some(&ClientOption::with_addr(""))).is_err());
    assert!(extworkload_client::New(Some(&ClientOption::with_addr("://bad"))).is_err());
    assert_eq!(
        extworkload_client::normalizeAddr(" http://127.0.0.1:1234 ").unwrap(),
        "127.0.0.1:1234"
    );
    assert_eq!(
        extworkload_client::normalizeAddr("127.0.0.1:1234").unwrap(),
        "127.0.0.1:1234"
    );
    assert_eq!(
        extworkload_client::normalizeAddr("http://[::1]:1234").unwrap(),
        "[::1]:1234"
    );
    assert!(extworkload_client::normalizeAddr("http://").is_err());
    let error = extworkload_client::mapResponse("Ping", Ok(None)).unwrap_err();
    assert!(error.to_string().contains("empty response"));
}

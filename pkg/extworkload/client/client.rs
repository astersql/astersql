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

// gRPC client for the external workload controller.
//
// 外部工作负载控制器的 gRPC 客户端。
// 负责与控制器通信以注册/回收 GC v2、TTL 任务与自动分析（auto analyze）任务，
// 并将控制器返回的暂停（Paused）错误映射为 `ClientError::ControllerPaused`。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};
use tonic::{Request, Response, Status};

/// 由 build 脚本从 protobuf 生成的外部工作负载 RPC 消息与客户端桩。
pub mod pb {
    tonic::include_proto!("externalworkload");
}

/// Error returned when the controller pauses a worker or an RPC fails.
/// 控制器暂停 worker 或 RPC 失败时返回的错误。
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("external workload controller: worker paused")]
    ControllerPaused,
    #[error("{0}")]
    Message(String),
}

/// Go's package-level sentinel is represented by a matchable Rust variant.
/// 对应 Go 包级哨兵错误 `ErrControllerPaused`。
#[allow(non_upper_case_globals)]
pub const ErrControllerPaused: ClientError = ClientError::ControllerPaused;

/// A tonic metadata interceptor. Multiple interceptors run in declaration order.
/// 一元 RPC 元数据拦截器；多个拦截器按声明顺序链式执行。
pub type UnaryClientInterceptor =
    Arc<dyn Fn(Request<()>) -> Result<Request<()>, Status> + Send + Sync + 'static>;

/// Configuration for a controller client.
/// 控制器客户端配置：keyspace、TiDB 池、地址、TLS 与拦截器。
/// Keyspace 是多租户隔离命名空间；TiDBPool 标识所属 TiDB 实例池。
#[allow(non_snake_case)]
#[derive(Clone)]
pub struct Option {
    pub KeyspaceID: u32,
    pub KeyspaceName: String,
    pub TiDBPool: String,
    pub ControllerAddr: String,
    pub TLSConfig: std::option::Option<ClientTlsConfig>,
    pub Interceptors: Vec<UnaryClientInterceptor>,
}

impl Option {
    /// 仅指定控制器地址的便捷构造；其余字段取默认空值。
    pub fn with_addr(address: impl Into<String>) -> Self {
        Self {
            KeyspaceID: 0,
            KeyspaceName: String::new(),
            TiDBPool: String::new(),
            ControllerAddr: address.into(),
            TLSConfig: None,
            Interceptors: Vec::new(),
        }
    }
}

/// Per-call context. Tonic translates its timeout into the grpc-timeout header.
/// 单次 RPC 调用上下文；超时会写入 grpc-timeout 请求头。
#[derive(Clone, Copy, Debug, Default)]
pub struct Context {
    timeout: std::option::Option<Duration>,
}

impl Context {
    /// 无超时的后台上下文。
    pub fn background() -> Self {
        Self::default()
    }

    /// 带超时的调用上下文。
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout: Some(timeout),
        }
    }

    /// 将消息包装为 tonic Request，并按需设置超时。
    fn request<T>(&self, message: T) -> Request<T> {
        let mut request = Request::new(message);
        if let Some(timeout) = self.timeout {
            request.set_timeout(timeout);
        }
        request
    }
}

/// 控制器客户端总接口：关闭、Ping，并组合 GCv2 / TTL / AutoAnalyze 子接口。
#[async_trait]
#[allow(non_snake_case)]
pub trait Client: gcv2Client + ttlClient + autoAnalyzeClient + Send {
    fn Close(&mut self) -> Result<(), ClientError>;
    async fn Ping(&mut self, context: &Context) -> Result<(), ClientError>;
}

/// GC v2 相关 RPC：注册、回收安全点（safe point）与更新 GC 生命周期。
/// Safe point 是 MVCC 垃圾回收可安全删除的时间戳下界。
#[async_trait]
#[allow(non_camel_case_types, non_snake_case)]
pub trait gcv2Client: Send {
    async fn RegisterGCV2(
        &mut self,
        context: &Context,
        safePoint: u64,
        gcLifeTime: i64,
    ) -> Result<(), ClientError>;
    async fn RecycleGCV2(&mut self, context: &Context, safePoint: u64) -> Result<(), ClientError>;
    async fn UpdateGCLifeTime(
        &mut self,
        context: &Context,
        gcLifeTime: i64,
    ) -> Result<(), ClientError>;
}

/// TTL（Time-To-Live）表任务相关 RPC：注册、删除、回收与开关。
#[async_trait]
#[allow(non_camel_case_types, non_snake_case)]
pub trait ttlClient: Send {
    async fn RegisterTTLTask(
        &mut self,
        context: &Context,
        tableID: i64,
        ttlJobEnable: bool,
    ) -> Result<(), ClientError>;
    async fn DeleteTTLTableInfo(
        &mut self,
        context: &Context,
        tableID: i64,
    ) -> Result<(), ClientError>;
    async fn RecycleTTLTask(
        &mut self,
        context: &Context,
        completedJobCreateTime: u64,
    ) -> Result<(), ClientError>;
    async fn UpdateTTLJobEnable(
        &mut self,
        context: &Context,
        ttlJobEnable: bool,
    ) -> Result<(), ClientError>;
}

/// 自动分析（auto analyze）任务相关 RPC：注册与回收。
/// Auto analyze 根据统计信息过期情况自动触发 ANALYZE。
#[async_trait]
#[allow(non_camel_case_types, non_snake_case)]
pub trait autoAnalyzeClient: Send {
    async fn RegisterAutoAnalyze(
        &mut self,
        context: &Context,
        taskID: u64,
    ) -> Result<(), ClientError>;
    async fn RecycleAutoAnalyze(
        &mut self,
        context: &Context,
        taskID: u64,
    ) -> Result<(), ClientError>;
}

/// 将多个一元拦截器按顺序串联的 tonic Interceptor。
#[derive(Clone)]
struct ChainInterceptor {
    interceptors: Vec<UnaryClientInterceptor>,
}

impl Interceptor for ChainInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        for interceptor in &self.interceptors {
            request = interceptor(request)?;
        }
        Ok(request)
    }
}

/// 带拦截器的 ExternalWorkloadController 客户端桩类型别名。
type Stub = pb::external_workload_controller_client::ExternalWorkloadControllerClient<
    InterceptedService<Channel, ChainInterceptor>,
>;

/// 基于 tonic 的具体客户端实现。
#[allow(non_camel_case_types)]
struct grpcClient {
    opt: Option,
    stub: std::option::Option<Stub>,
}

/// Creates a lazy tonic channel, matching grpc.NewClient's non-blocking dial behavior.
/// 创建惰性 tonic 通道，对齐 Go grpc.NewClient 的非阻塞拨号语义。
#[allow(non_snake_case)]
pub fn New(option: std::option::Option<&Option>) -> Result<Box<dyn Client>, ClientError> {
    let option = option
        .ok_or_else(|| ClientError::Message("external workload client: nil option".to_owned()))?;
    let host = normalizeAddr(&option.ControllerAddr)?;
    // 有 TLS 配置则用 https，否则 http。
    let scheme = if option.TLSConfig.is_some() {
        "https"
    } else {
        "http"
    };
    let uri = format!("{scheme}://{host}");
    let mut endpoint = Endpoint::from_shared(uri).map_err(|error| {
        ClientError::Message(format!(
            "create external workload controller client: {error}"
        ))
    })?;
    if let Some(tls) = option.TLSConfig.clone() {
        endpoint = endpoint.tls_config(tls).map_err(|error| {
            ClientError::Message(format!(
                "create external workload controller client: {error}"
            ))
        })?;
    }
    // connect_lazy：首次 RPC 时再真正建连。
    let channel = endpoint.connect_lazy();
    let stub =
        pb::external_workload_controller_client::ExternalWorkloadControllerClient::with_interceptor(
            channel,
            ChainInterceptor {
                interceptors: option.Interceptors.clone(),
            },
        );
    Ok(Box::new(grpcClient {
        opt: option.clone(),
        stub: Some(stub),
    }))
}

/// Trims an address and returns Go url.URL.Host semantics, including the port.
/// 规范化控制器地址：去空白；若带 scheme 则解析出 host:port，语义对齐 Go url.URL.Host。
#[allow(non_snake_case)]
pub fn normalizeAddr(address: &str) -> Result<String, ClientError> {
    let address = address.trim();
    if address.is_empty() {
        return Err(ClientError::Message(
            "external workload client: empty controller address".to_owned(),
        ));
    }
    // 无 scheme 时原样返回（已 trim）。
    if !address.contains("://") {
        return Ok(address.to_owned());
    }
    let parsed = url::Url::parse(address).map_err(|error| {
        ClientError::Message(format!("parse controller address {address:?}: {error}"))
    })?;
    parsed.host().ok_or_else(|| {
        ClientError::Message(format!(
            "external workload client: controller address {address:?} has no host"
        ))
    })?;
    // `url::Url` normalizes explicit default ports away, while Go's `url.URL.Host`
    // preserves the original authority (including `:80` / `:443`). Return the
    // validated authority without userinfo to retain those Go semantics.
    let authority = address
        .split_once("://")
        .expect("scheme separator checked above")
        .1
        .split(['/', '?', '#'])
        .next()
        .expect("split always yields the authority");
    Ok(authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
        .to_owned())
}

impl grpcClient {
    /// 取得可变桩引用；已 Close 则报错。
    fn stub(&mut self) -> Result<&mut Stub, ClientError> {
        self.stub.as_mut().ok_or_else(|| {
            ClientError::Message("external workload controller client is closed".to_owned())
        })
    }

    /// 从 Option 组装每次 RPC 共用的 RequestHeader。
    fn header(&self) -> pb::RequestHeader {
        pb::RequestHeader {
            keyspace_id: self.opt.KeyspaceID,
            keyspace_name: self.opt.KeyspaceName.clone(),
            tidb_pool: self.opt.TiDBPool.clone(),
        }
    }
}

#[async_trait]
#[allow(non_snake_case)]
impl Client for grpcClient {
    fn Close(&mut self) -> Result<(), ClientError> {
        // 丢弃桩，后续 RPC 将失败。
        self.stub.take();
        Ok(())
    }

    async fn Ping(&mut self, context: &Context) -> Result<(), ClientError> {
        let request = context.request(pb::PingRequest {});
        mapTonicResponse("Ping", self.stub()?.ping(request).await)
    }
}

#[async_trait]
#[allow(non_snake_case)]
impl gcv2Client for grpcClient {
    async fn RegisterGCV2(
        &mut self,
        context: &Context,
        safePoint: u64,
        gcLifeTime: i64,
    ) -> Result<(), ClientError> {
        let request = context.request(pb::RegisterGcv2Request {
            header: Some(self.header()),
            safe_point: safePoint,
            gc_life_time: gcLifeTime,
        });
        mapTonicResponse("RegisterGCV2", self.stub()?.register_gcv2(request).await)
    }

    async fn RecycleGCV2(&mut self, context: &Context, safePoint: u64) -> Result<(), ClientError> {
        let request = context.request(pb::RecycleGcv2Request {
            header: Some(self.header()),
            safe_point: safePoint,
        });
        mapTonicResponse("RecycleGCV2", self.stub()?.recycle_gcv2(request).await)
    }

    async fn UpdateGCLifeTime(
        &mut self,
        context: &Context,
        gcLifeTime: i64,
    ) -> Result<(), ClientError> {
        let request = context.request(pb::UpdateGcLifeTimeRequest {
            header: Some(self.header()),
            gc_life_time: gcLifeTime,
        });
        mapTonicResponse(
            "UpdateGCLifeTime",
            self.stub()?.update_gc_life_time(request).await,
        )
    }
}

#[async_trait]
#[allow(non_snake_case)]
impl ttlClient for grpcClient {
    async fn RegisterTTLTask(
        &mut self,
        context: &Context,
        tableID: i64,
        ttlJobEnable: bool,
    ) -> Result<(), ClientError> {
        let request = context.request(pb::RegisterTtlTaskRequest {
            header: Some(self.header()),
            table_id: tableID,
            ttl_job_enable: ttlJobEnable,
        });
        mapTonicResponse(
            "RegisterTTLTask",
            self.stub()?.register_ttl_task(request).await,
        )
    }

    async fn DeleteTTLTableInfo(
        &mut self,
        context: &Context,
        tableID: i64,
    ) -> Result<(), ClientError> {
        let request = context.request(pb::DeleteTtlTableInfoRequest {
            header: Some(self.header()),
            table_id: tableID,
        });
        mapTonicResponse(
            "DeleteTTLTableInfo",
            self.stub()?.delete_ttl_table_info(request).await,
        )
    }

    async fn RecycleTTLTask(
        &mut self,
        context: &Context,
        completedJobCreateTime: u64,
    ) -> Result<(), ClientError> {
        let request = context.request(pb::RecycleTtlTaskRequest {
            header: Some(self.header()),
            completed_job_create_time: completedJobCreateTime,
        });
        mapTonicResponse(
            "RecycleTTLTask",
            self.stub()?.recycle_ttl_task(request).await,
        )
    }

    async fn UpdateTTLJobEnable(
        &mut self,
        context: &Context,
        ttlJobEnable: bool,
    ) -> Result<(), ClientError> {
        let request = context.request(pb::UpdateTtlJobEnableRequest {
            header: Some(self.header()),
            ttl_job_enable: ttlJobEnable,
        });
        mapTonicResponse(
            "UpdateTTLJobEnable",
            self.stub()?.update_ttl_job_enable(request).await,
        )
    }
}

#[async_trait]
#[allow(non_snake_case)]
impl autoAnalyzeClient for grpcClient {
    async fn RegisterAutoAnalyze(
        &mut self,
        context: &Context,
        taskID: u64,
    ) -> Result<(), ClientError> {
        let request = context.request(pb::RegisterAutoAnalyzeRequest {
            header: Some(self.header()),
            task_id: taskID,
        });
        mapTonicResponse(
            "RegisterAutoAnalyze",
            self.stub()?.register_auto_analyze(request).await,
        )
    }

    async fn RecycleAutoAnalyze(
        &mut self,
        context: &Context,
        taskID: u64,
    ) -> Result<(), ClientError> {
        let request = context.request(pb::RecycleAutoAnalyzeRequest {
            header: Some(self.header()),
            task_id: taskID,
        });
        mapTonicResponse(
            "RecycleAutoAnalyze",
            self.stub()?.recycle_auto_analyze(request).await,
        )
    }
}

/// 将 tonic Response 解包后交给 `mapResponse`。
#[allow(non_snake_case)]
fn mapTonicResponse(
    method: &str,
    response: Result<Response<pb::Response>, Status>,
) -> Result<(), ClientError> {
    mapResponse(method, response.map(|response| Some(response.into_inner())))
}

/// Applies the Go response mapping order, including its defensive nil-response branch.
/// 按 Go 侧顺序映射响应：传输错误 → 空响应 → 业务 ErrorType（Ok / Paused / 其它消息）。
#[allow(non_snake_case)]
pub fn mapResponse(
    method: &str,
    response: Result<std::option::Option<pb::Response>, Status>,
) -> Result<(), ClientError> {
    let response = response.map_err(|error| {
        ClientError::Message(format!("external workload rpc {method}: {error}"))
    })?;
    let response = response.ok_or_else(|| {
        ClientError::Message(format!("external workload rpc {method}: empty response"))
    })?;
    let Some(error) = response.error else {
        return Ok(());
    };
    match error.r#type() {
        pb::ErrorType::Ok => Ok(()),
        pb::ErrorType::Paused => Err(ClientError::ControllerPaused),
        _ => Err(ClientError::Message(format!(
            "external workload rpc {method}: {}",
            error.message
        ))),
    }
}

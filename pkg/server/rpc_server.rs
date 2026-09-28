// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// gRPC/RPC 服务端：协处理器（Coprocessor）、批量命令与 MPP 任务状态上报。
//
// Coprocessor 在 TiKV 侧执行下推计算；MPP（Massively Parallel Processing）
// 负责任务状态协调。本模块将 panic 捕获为 other_error，避免拖垮 RPC 连接。

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use crate::server::{Domain, StatusConfig};

#[derive(Clone, Debug, Default)]
/// 协处理器请求：请求 ID、序列化载荷与可选对端地址。
pub struct CoprocessorRequest {
    /// 请求标识，用于与响应配对。
    pub request_id: u64,
    /// 序列化的协处理器请求体。
    pub payload: Vec<u8>,
    /// 对端地址，写入会话供诊断。
    pub peer_address: Option<String>,
}

#[derive(Clone, Debug, Default)]
/// 协处理器响应：成功载荷或 other_error 文本。
pub struct CoprocessorResponse {
    pub payload: Vec<u8>,
    /// 非业务错误（含 panic 文本），对应 Go 的 OtherError。
    pub other_error: Option<String>,
}

#[derive(Clone, Debug)]
/// 批量命令流中的单条命令：协处理器或空探测。
pub enum BatchCommand {
    Coprocessor(CoprocessorRequest),
    Empty { test_id: u64 },
}

#[derive(Clone, Debug, Default)]
/// 一批命令请求，含 request_id 列表与命令体。
pub struct BatchRequest {
    pub request_ids: Vec<u64>,
    pub requests: Vec<BatchCommand>,
}

#[derive(Clone, Debug)]
/// 批量响应中的单项，与 BatchCommand 对应。
pub enum BatchResponseItem {
    Coprocessor(CoprocessorResponse),
    Empty { test_id: u64 },
}

#[derive(Clone, Debug, Default)]
/// 一批命令的完整响应。
pub struct BatchResponse {
    pub request_ids: Vec<u64>,
    pub responses: Vec<BatchResponseItem>,
}

#[derive(Clone, Debug, Default)]
/// MPP 任务状态上报请求（task_id + 状态字符串）。
pub struct MppTaskStatusRequest {
    pub task_id: u64,
    pub state: String,
}

#[derive(Clone, Debug, Default)]
/// MPP 任务状态上报结果。
pub struct MppTaskStatusResponse {
    pub accepted: bool,
    pub error: Option<String>,
}

/// RPC 会话：绑定对端地址、初始化内存跟踪并在结束时 close。
pub trait RpcSession: Send {
    fn set_peer_address(&mut self, address: Option<String>);
    fn initialize_memory_tracker(&mut self);
    fn detach_memory_tracker(&mut self);
    fn close(&mut self);
}

/// 按 Domain 创建 RPC 会话。
pub trait RpcSessionFactory: Send + Sync {
    fn create(&self, domain: Arc<dyn Domain>) -> Result<Box<dyn RpcSession>, String>;
}

/// 协处理器执行器：单次或流式执行请求。
pub trait CoprocessorExecutor: Send + Sync {
    fn execute(
        &self,
        session: &mut dyn RpcSession,
        request: &CoprocessorRequest,
    ) -> Result<CoprocessorResponse, String>;

    fn execute_stream(
        &self,
        session: &mut dyn RpcSession,
        request: &CoprocessorRequest,
        stream: &mut dyn CoprocessorStream,
    ) -> Result<(), String> {
        stream.send(self.execute(session, request)?)
    }
}

/// 协处理器流式响应通道。
pub trait CoprocessorStream {
    fn send(&mut self, response: CoprocessorResponse) -> Result<(), String>;
}

/// 双向批量命令流：收 BatchRequest、发 BatchResponse。
pub trait BatchCommandStream {
    fn receive(&mut self) -> Result<Option<BatchRequest>, String>;
    fn send(&mut self, response: BatchResponse) -> Result<(), String>;
}

/// MPP 协调器：接收任务状态报告。
pub trait MppCoordinator: Send + Sync {
    fn report_status(&self, request: &MppTaskStatusRequest) -> MppTaskStatusResponse;
}

#[derive(Clone, Debug)]
/// gRPC 保活、并发流、窗口与消息大小等传输参数。
pub struct RpcServerConfig {
    pub keep_alive_time: Duration,
    pub keep_alive_timeout: Duration,
    pub minimum_ping_interval: Duration,
    pub max_concurrent_streams: u32,
    pub initial_window_size: u32,
    pub max_send_message_size: usize,
}

/// 从状态服务 StatusConfig 映射 gRPC 相关字段。
impl From<&StatusConfig> for RpcServerConfig {
    fn from(status: &StatusConfig) -> Self {
        Self {
            keep_alive_time: status.grpc_keep_alive,
            keep_alive_timeout: status.grpc_keep_alive_timeout,
            minimum_ping_interval: Duration::from_secs(5),
            max_concurrent_streams: status.grpc_concurrent_streams,
            initial_window_size: status.grpc_initial_window_size,
            max_send_message_size: status.grpc_max_send_message_size,
        }
    }
}

/// RAII 会话守卫：Drop 时调用 session.close()。
struct SessionGuard {
    session: Option<Box<dyn RpcSession>>,
    detach_memory_tracker: bool,
}

impl SessionGuard {
    fn session(&mut self) -> &mut dyn RpcSession {
        self.session
            .as_deref_mut()
            .expect("session guard must contain session")
    }

    fn detach_memory_tracker_on_drop(&mut self) {
        self.detach_memory_tracker = true;
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Some(session) = &mut self.session {
            if self.detach_memory_tracker {
                session.detach_memory_tracker();
            }
            session.close();
        }
    }
}

/// RPC 服务：持有 Domain、会话工厂、协处理器与 MPP 协调器。
pub struct RpcServer {
    pub config: RpcServerConfig,
    pub diagnostic_log_file: Option<String>,
    domain: Arc<dyn Domain>,
    sessions: Arc<dyn RpcSessionFactory>,
    executor: Arc<dyn CoprocessorExecutor>,
    mpp: Arc<dyn MppCoordinator>,
}

impl RpcServer {
    /// 由 StatusConfig 与各依赖组装 RpcServer。
    pub fn new(
        status: &StatusConfig,
        diagnostic_log_file: Option<String>,
        domain: Arc<dyn Domain>,
        sessions: Arc<dyn RpcSessionFactory>,
        executor: Arc<dyn CoprocessorExecutor>,
        mpp: Arc<dyn MppCoordinator>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config: RpcServerConfig::from(status),
            diagnostic_log_file,
            domain,
            sessions,
            executor,
            mpp,
        })
    }

    /// 创建会话并初始化内存跟踪器。
    fn create_session(&self) -> Result<SessionGuard, String> {
        let mut session = self.sessions.create(Arc::clone(&self.domain))?;
        session.initialize_memory_tracker();
        Ok(SessionGuard {
            session: Some(session),
            detach_memory_tracker: false,
        })
    }

    /// 处理单次协处理器请求；捕获 panic 写入 other_error。
    pub fn coprocessor(&self, request: &CoprocessorRequest) -> CoprocessorResponse {
        // 与 Go 一致：协处理器 panic 不得撕裂 RPC，转为 OtherError。
        let result = catch_unwind(AssertUnwindSafe(|| self.handle_coprocessor(request)));
        match result {
            Ok(Ok(response)) => response,
            Ok(Err(error)) => CoprocessorResponse {
                other_error: Some(error),
                ..CoprocessorResponse::default()
            },
            Err(payload) => CoprocessorResponse {
                other_error: Some(format!(
                    "panic while handling coprocessor: {}",
                    panic_text(payload)
                )),
                ..CoprocessorResponse::default()
            },
        }
    }

    /// 创建会话后委托 CoprocessorExecutor 执行。
    fn handle_coprocessor(
        &self,
        request: &CoprocessorRequest,
    ) -> Result<CoprocessorResponse, String> {
        let mut session = self.create_session()?;
        if let Some(peer) = &request.peer_address {
            session.session().set_peer_address(Some(peer.clone()));
        }
        session.detach_memory_tracker_on_drop();
        self.executor.execute(session.session(), request)
    }

    /// 流式协处理器；panic 时向流发送带 other_error 的响应。
    pub fn coprocessor_stream(
        &self,
        request: &CoprocessorRequest,
        stream: &mut dyn CoprocessorStream,
    ) -> Result<(), String> {
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut session = match self.create_session() {
                Ok(session) => session,
                Err(error) => {
                    return stream.send(CoprocessorResponse {
                        other_error: Some(error),
                        ..CoprocessorResponse::default()
                    });
                }
            };
            self.executor
                .execute_stream(session.session(), request, stream)
        }));
        match result {
            Ok(result) => result,
            Err(payload) => stream.send(CoprocessorResponse {
                other_error: Some(format!(
                    "panic while handling coprocessor stream: {}",
                    panic_text(payload)
                )),
                ..CoprocessorResponse::default()
            }),
        }
    }

    /// 循环接收批量请求，逐条执行并回写响应。
    pub fn batch_commands(&self, stream: &mut dyn BatchCommandStream) -> Result<(), String> {
        let result = catch_unwind(AssertUnwindSafe(|| {
            // 流结束时 receive 返回 None，正常退出循环。
            while let Some(batch) = stream.receive()? {
                let responses = batch
                    .requests
                    .into_iter()
                    .map(|request| match request {
                        BatchCommand::Coprocessor(request) => {
                            BatchResponseItem::Coprocessor(self.coprocessor(&request))
                        }
                        BatchCommand::Empty { test_id } => BatchResponseItem::Empty { test_id },
                    })
                    .collect();
                stream.send(BatchResponse {
                    request_ids: batch.request_ids,
                    responses,
                })?;
            }
            Ok(())
        }));
        match result {
            Ok(result) => result,
            // Go's deferred recover logs the panic and returns the zero error value.
            Err(_payload) => Ok(()),
        }
    }

    /// 将 MPP 任务状态转交协调器。
    pub fn report_mpp_task_status(&self, request: &MppTaskStatusRequest) -> MppTaskStatusResponse {
        self.mpp.report_status(request)
    }
}

/// 从 catch_unwind 载荷提取可读 panic 文本。
fn panic_text(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).into()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic payload".into()
    }
}

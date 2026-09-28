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

// 协处理器 RPC 处理层：对接 CmdCop / BatchCop 请求与会话上下文。
//
// 提供 `CoprRPCHandler` 抽象及默认实现：校验 Region（键空间分片）错误、
// 派发到 `coprHandler`，并用后台线程监控 BatchCop 流式租约超时。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::copr_handler::{
    BatchRequest, BatchResponse, CopError, KvReader, MemoryReader, Request, Response, coprHandler,
    mockBatchCopDataClient,
};

/// 单次 RPC 会话：绑定 KV 读取器与可选的 Region 错误。
pub struct RPCSession {
    pub reader: Arc<dyn KvReader>,
    pub region_error: Option<CopError>,
}

impl RPCSession {
    /// 用指定 KV 读取器创建会话。
    pub fn new(reader: Arc<dyn KvReader>) -> Self {
        Self {
            reader,
            region_error: None,
        }
    }
    /// 若会话已携带 Region 错误则直接失败，否则通过。
    pub fn CheckRequestContext(&self) -> Result<(), CopError> {
        self.region_error.clone().map_or(Ok(()), Err)
    }
}

/// BatchCop 流式响应的租约：到期或取消后 Recv 会报超时。
#[derive(Clone)]
pub struct Lease {
    pub deadline: Instant,
    pub cancelled: Arc<AtomicBool>,
}

impl Lease {
    /// 从现在起 `timeout` 后到期。
    pub fn new(timeout: Duration) -> Self {
        Self {
            deadline: Instant::now() + timeout,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
    /// 标记租约已取消（超时线程或主动关闭时调用）。
    pub fn Cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }
    /// 查询租约是否已取消。
    pub fn IsCancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// BatchCop 流式响应包装：内含客户端、首包、租约与超时时长。
pub struct BatchCopStreamResponse {
    pub client: BatchCopClient,
    pub BatchResponse: Option<BatchResponse>,
    pub Lease: Lease,
    pub Timeout: Duration,
}

impl BatchCopStreamResponse {
    /// 接收下一包；租约已取消则返回超时错误。
    pub fn Recv(&mut self) -> Result<BatchResponse, CopError> {
        if self.Lease.IsCancelled() {
            return Err(CopError::InvalidRequest(
                "batch coprocessor stream timed out".into(),
            ));
        }
        self.client.Recv()
    }
}

/// BatchCop 客户端流：正常路径逐包返回数据，Region 错误路径持续返回错误响应。
pub enum BatchCopClient {
    Data(mockBatchCopDataClient),
    RegionError(CopError),
}

impl BatchCopClient {
    fn Recv(&mut self) -> Result<BatchResponse, CopError> {
        match self {
            Self::Data(client) => client.Recv(),
            Self::RegionError(error) => Ok(BatchResponse {
                responses: Vec::new(),
                other_error: Some(error.to_string()),
            }),
        }
    }
}

/// 协处理器 RPC 处理器接口（流式 / 命令式 / 批量）。
pub trait CoprRPCHandler: Send {
    fn HandleCopStream(
        &mut self,
        session: &RPCSession,
        request: &Request,
        timeout: Duration,
    ) -> Result<Response, CopError>;
    fn HandleCmdCop(&mut self, session: &RPCSession, request: &Request) -> Response;
    fn HandleBatchCop(
        &mut self,
        session: &RPCSession,
        request: &BatchRequest,
        timeout: Duration,
    ) -> Result<BatchCopStreamResponse, CopError>;
    fn Close(&mut self);
}

/// 默认 RPC 处理器：持有会话、租约通道与超时监控线程。
pub struct coprRPCHandler {
    pub session: RPCSession,
    streamTimeout: mpsc::SyncSender<Lease>,
    done: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}

/// 使用内存 KV 读取器创建处理器（测试常用入口）。
pub fn NewCoprRPCHandler() -> Box<dyn CoprRPCHandler> {
    Box::new(coprRPCHandler::new(Arc::new(MemoryReader::default())))
}

/// 使用指定 KV 读取器创建处理器。
pub fn NewCoprRPCHandlerWithReader(reader: Arc<dyn KvReader>) -> coprRPCHandler {
    coprRPCHandler::new(reader)
}

impl coprRPCHandler {
    /// 启动超时监控线程并绑定读取器会话。
    pub fn new(reader: Arc<dyn KvReader>) -> Self {
        let (timeout_sender, timeout_receiver) = mpsc::sync_channel::<Lease>(1024);
        let (done_sender, done_receiver) = mpsc::channel();
        let worker =
            std::thread::spawn(move || check_stream_timeout_loop(timeout_receiver, done_receiver));
        Self {
            session: RPCSession::new(reader),
            streamTimeout: timeout_sender,
            done: Some(done_sender),
            worker: Some(worker),
        }
    }

    /// 旧版 CopStream API，已废弃，调用即 panic。
    pub fn HandleCopStream(
        &mut self,
        _session: &RPCSession,
        _request: &Request,
        _timeout: Duration,
    ) -> Result<Response, CopError> {
        panic!("CopStream API is deprecated")
    }

    /// 处理单次 CmdCop：校验上下文后交给 `coprHandler`，取首个响应。
    pub fn HandleCmdCop(&mut self, session: &RPCSession, request: &Request) -> Response {
        if let Err(error) = session.CheckRequestContext() {
            return Response {
                region_error: Some(error),
                ..Response::default()
            };
        }
        let handler = coprHandler {
            reader: session.reader.clone(),
            region_error: session.region_error.clone(),
        };
        handler
            .handle_request(request)
            .responses
            .into_iter()
            .next()
            .unwrap_or_default()
    }

    /// 处理 BatchCop：注册租约、预取首包并返回流式响应。
    pub fn HandleBatchCop(
        &mut self,
        session: &RPCSession,
        request: &BatchRequest,
        timeout: Duration,
    ) -> Result<BatchCopStreamResponse, CopError> {
        if let Err(error) = session.CheckRequestContext() {
            let mut response = BatchCopStreamResponse {
                client: BatchCopClient::RegionError(error),
                BatchResponse: None,
                Lease: Lease::new(Duration::ZERO),
                Timeout: Duration::ZERO,
            };
            response.BatchResponse = Some(response.Recv()?);
            return Ok(response);
        }
        let handler = coprHandler {
            reader: session.reader.clone(),
            region_error: session.region_error.clone(),
        };
        let client = handler.handleBatchCopRequest(request)?;
        let lease = Lease::new(timeout);
        // 将租约交给后台超时循环监控。
        self.streamTimeout
            .send(lease.clone())
            .map_err(|_| CopError::InvalidRequest("stream timeout loop has stopped".into()))?;
        let mut response = BatchCopStreamResponse {
            client: BatchCopClient::Data(client),
            BatchResponse: None,
            Lease: lease,
            Timeout: timeout,
        };
        response.BatchResponse = Some(response.Recv()?);
        Ok(response)
    }

    /// 通知超时线程退出并 join。
    pub fn Close(&mut self) {
        if let Some(done) = self.done.take() {
            let _ = done.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl CoprRPCHandler for coprRPCHandler {
    fn HandleCopStream(
        &mut self,
        session: &RPCSession,
        request: &Request,
        timeout: Duration,
    ) -> Result<Response, CopError> {
        self.HandleCopStream(session, request, timeout)
    }
    fn HandleCmdCop(&mut self, session: &RPCSession, request: &Request) -> Response {
        self.HandleCmdCop(session, request)
    }
    fn HandleBatchCop(
        &mut self,
        session: &RPCSession,
        request: &BatchRequest,
        timeout: Duration,
    ) -> Result<BatchCopStreamResponse, CopError> {
        self.HandleBatchCop(session, request, timeout)
    }
    fn Close(&mut self) {
        self.Close();
    }
}

impl Drop for coprRPCHandler {
    fn drop(&mut self) {
        self.Close();
    }
}

/// 后台循环：收集租约，到期则 Cancel，收到 done 信号则退出。
fn check_stream_timeout_loop(
    timeout_receiver: mpsc::Receiver<Lease>,
    done_receiver: mpsc::Receiver<()>,
) {
    let mut leases = Vec::new();
    loop {
        if done_receiver.try_recv().is_ok() {
            return;
        }
        while let Ok(lease) = timeout_receiver.try_recv() {
            leases.push(lease);
        }
        let now = Instant::now();
        // 清理已取消或已到期的租约。
        leases.retain(|lease| {
            if lease.IsCancelled() {
                return false;
            }
            if lease.deadline <= now {
                lease.Cancel();
                return false;
            }
            true
        });
        match done_receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// MPP（Massively Parallel Processing，大规模并行处理）任务隧道与 DAG 调度。
//
// MPP 把查询计划拆成多任务，经 Exchange（数据交换）在任务间传递行数据。
// 本模块实现 `ExchangerTunnel`（同步通道隧道）、`MppTaskHandler`（隧道注册/建连/取消）
// 与 `MppExecBuilder`（在执行器树中处理 ExchangeSender/Receiver）。

use crate::cop_handler::{
    CopError, DagRequest, ExchangeType, Executor, KeyRange, KvReader, Request, RequestPayload,
    Response, Row, handle_cop_request,
};
use crate::mpp_exec::{ExecutionOutput, execute_executor};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};

/// MPP 协议版本号。
pub const MPP_VERSION: i64 = 1;
/// Exchange 隧道同步通道缓冲容量。
pub const TUNNEL_BUFFER: usize = 10;

#[derive(Clone, Debug, Default, Eq, PartialEq, Hash)]
/// MPP 任务元数据：任务 ID 与通信地址。
pub struct TaskMeta {
    pub task_id: i64,
    pub address: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 隧道中传递的数据包：行集或错误信息。
pub struct MppDataPacket {
    pub data: Vec<Row>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
/// 隧道键：由发送方与接收方 task_id 唯一标识一条 Exchange 通道。
pub struct TunnelKey {
    pub sender_task_id: i64,
    pub receiver_task_id: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 建立隧道连接的请求：指定发送方与接收方任务。
pub struct EstablishRequest {
    pub sender: TaskMeta,
    pub receiver: TaskMeta,
}

/// Exchange 隧道：带连接握手、激活与关闭的同步通道封装。
pub struct ExchangerTunnel {
    sender: Mutex<Option<SyncSender<MppDataPacket>>>,
    receiver: Mutex<Receiver<MppDataPacket>>,
    connected: (Mutex<bool>, Condvar),
    active: AtomicBool,
    closed: AtomicBool,
}

impl ExchangerTunnel {
    /// 创建带缓冲的同步通道隧道。
    pub fn new() -> Arc<Self> {
        let (sender, receiver) = mpsc::sync_channel(TUNNEL_BUFFER);
        Arc::new(Self {
            sender: Mutex::new(Some(sender)),
            receiver: Mutex::new(receiver),
            connected: (Mutex::new(false), Condvar::new()),
            active: AtomicBool::new(false),
            closed: AtomicBool::new(false),
        })
    }

    /// 标记隧道已连接并唤醒等待方。
    pub fn connect(&self) {
        let mut connected = self
            .connected
            .0
            .lock()
            .expect("tunnel connection lock poisoned");
        *connected = true;
        self.connected.1.notify_all();
    }

    /// 阻塞直到隧道连接或已关闭；关闭则返回 Cancelled。
    pub fn wait_connected(&self) -> Result<(), CopError> {
        let mut connected = self
            .connected
            .0
            .lock()
            .expect("tunnel connection lock poisoned");
        while !*connected && !self.closed.load(Ordering::Acquire) {
            connected = self
                .connected
                .1
                .wait(connected)
                .expect("tunnel connection lock poisoned");
        }
        if self.closed.load(Ordering::Acquire) {
            Err(CopError::Cancelled)
        } else {
            Ok(())
        }
    }

    /// 等待连接后发送数据包；对端关闭则视为取消。
    pub fn send(&self, packet: MppDataPacket) -> Result<(), CopError> {
        self.wait_connected()?;
        self.sender
            .lock()
            .expect("tunnel sender lock poisoned")
            .as_ref()
            .ok_or(CopError::Cancelled)?
            .send(packet)
            .map_err(|_| CopError::Cancelled)
    }

    /// 接收一块数据；包内 error 转为 Tunnel 错误，通道关闭返回 None。
    pub fn recv_chunk(&self) -> Result<Option<MppDataPacket>, CopError> {
        match self
            .receiver
            .lock()
            .expect("tunnel receiver lock poisoned")
            .recv()
        {
            Ok(packet) => {
                if let Some(error) = &packet.error {
                    Err(CopError::Tunnel(error.clone()))
                } else {
                    Ok(Some(packet))
                }
            }
            Err(_) if self.closed.load(Ordering::Acquire) => Ok(None),
            Err(_) => Err(CopError::Cancelled),
        }
    }

    /// 一次性激活隧道，防止同一隧道被重复占用。
    pub fn activate(&self) -> Result<(), CopError> {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| CopError::Tunnel("tunnel already active".into()))
    }

    /// 关闭隧道：置关闭标志、丢弃 sender 并唤醒等待者。
    pub fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.sender
            .lock()
            .expect("tunnel sender lock poisoned")
            .take();
        self.connected.1.notify_all();
    }
}

#[derive(Default)]
/// MPP 任务侧隧道表与取消标志。
pub struct MppTaskHandler {
    tunnels: Mutex<HashMap<TunnelKey, Arc<ExchangerTunnel>>>,
    cancelled: AtomicBool,
}

impl MppTaskHandler {
    /// 注册隧道；同一 TunnelKey 不可重复注册。
    pub fn register_tunnel(
        &self,
        key: TunnelKey,
        tunnel: Arc<ExchangerTunnel>,
    ) -> Result<(), CopError> {
        let mut tunnels = self.tunnels.lock().expect("MPP tunnel map poisoned");
        if tunnels.contains_key(&key) {
            return Err(CopError::Tunnel("tunnel already registered".into()));
        }
        tunnels.insert(key, tunnel);
        Ok(())
    }

    /// 按 EstablishRequest 查找隧道、激活并完成连接握手。
    pub fn establish_conn(
        &self,
        request: &EstablishRequest,
    ) -> Result<Arc<ExchangerTunnel>, CopError> {
        let key = TunnelKey {
            sender_task_id: request.sender.task_id,
            receiver_task_id: request.receiver.task_id,
        };
        let tunnel = self
            .tunnels
            .lock()
            .expect("MPP tunnel map poisoned")
            .get(&key)
            .cloned()
            .ok_or_else(|| CopError::Tunnel(format!("tunnel {key:?} not found")))?;
        tunnel.activate()?;
        tunnel.connect();
        Ok(tunnel)
    }

    /// 按发送/接收 task_id 查询已注册隧道。
    pub fn tunnel(&self, sender: i64, receiver: i64) -> Option<Arc<ExchangerTunnel>> {
        self.tunnels
            .lock()
            .expect("MPP tunnel map poisoned")
            .get(&TunnelKey {
                sender_task_id: sender,
                receiver_task_id: receiver,
            })
            .cloned()
    }

    /// 取消任务：置取消标志并关闭全部隧道。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        for tunnel in self
            .tunnels
            .lock()
            .expect("MPP tunnel map poisoned")
            .values()
        {
            tunnel.close();
        }
    }

    /// 是否已取消。
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// 执行 Exchange 时绑定的任务元数据与任务处理器。
pub struct MppContext<'a> {
    pub task: TaskMeta,
    pub handler: &'a MppTaskHandler,
}

/// 带可选 MPP 上下文的执行器构建/执行器。
pub struct MppExecBuilder<'a> {
    reader: &'a dyn KvReader,
    ranges: &'a [KeyRange],
    start_ts: u64,
    context: Option<MppContext<'a>>,
}

impl<'a> MppExecBuilder<'a> {
    /// 构造无 MPP 上下文的执行构建器（start_ts 为 MVCC 读时间戳）。
    pub fn new(reader: &'a dyn KvReader, ranges: &'a [KeyRange], start_ts: u64) -> Self {
        Self {
            reader,
            ranges,
            start_ts,
            context: None,
        }
    }
    /// 附加 MPP 上下文以支持 ExchangeSender/Receiver。
    pub fn with_context(mut self, context: MppContext<'a>) -> Self {
        self.context = Some(context);
        self
    }

    /// 递归执行执行器树；Exchange 算子走隧道收发，其余委托 `execute_executor`。
    pub fn build_and_execute(&self, executor: &Executor) -> Result<ExecutionOutput, CopError> {
        if self
            .context
            .as_ref()
            .is_some_and(|context| context.handler.cancelled())
        {
            return Err(CopError::Cancelled);
        }
        match executor {
            Executor::ExchangeReceiver { source_task_ids } => self.receive(source_task_ids),
            Executor::ExchangeSender {
                exchange,
                partition_keys,
                child,
            } => {
                let mut output = self.build_and_execute(child)?;
                self.send(exchange, partition_keys, &output.rows)?;
                output.intermediate.push(output.rows.clone());
                Ok(output)
            }
            _ => execute_executor(self.reader, self.ranges, self.start_ts, executor),
        }
    }

    /// 按 Exchange 类型（Broadcast/PassThrough/Hash）向本任务发出隧道发送行。
    fn send(
        &self,
        exchange: &ExchangeType,
        partition_keys: &[crate::cop_handler::Expr],
        rows: &[Row],
    ) -> Result<(), CopError> {
        let Some(context) = &self.context else {
            return Ok(());
        };
        let tunnels = context
            .handler
            .tunnels
            .lock()
            .expect("MPP tunnel map poisoned")
            .iter()
            .filter(|(key, _)| key.sender_task_id == context.task.task_id)
            .map(|(_, tunnel)| tunnel.clone())
            .collect::<Vec<_>>();
        if tunnels.is_empty() {
            return Ok(());
        }
        match exchange {
            // 广播：每个下游隧道各发一份完整行集后关闭。
            ExchangeType::Broadcast => {
                for tunnel in tunnels {
                    tunnel.send(MppDataPacket {
                        data: rows.to_vec(),
                        error: None,
                    })?;
                    tunnel.close();
                }
            }
            // 直通：只向第一条隧道发送。
            ExchangeType::PassThrough => {
                tunnels[0].send(MppDataPacket {
                    data: rows.to_vec(),
                    error: None,
                })?;
                tunnels[0].close();
            }
            // 哈希分区：按行哈希取模分发到各隧道。
            ExchangeType::Hash => {
                let mut partitions = vec![Vec::new(); tunnels.len()];
                for row in rows {
                    let key = partition_keys
                        .iter()
                        .map(|expression| expression.eval(row))
                        .collect::<Result<Vec<_>, _>>()?;
                    let slot = row_hash(&key) as usize % partitions.len();
                    partitions[slot].push(row.clone());
                }
                for (tunnel, rows) in tunnels.into_iter().zip(partitions) {
                    tunnel.send(MppDataPacket {
                        data: rows,
                        error: None,
                    })?;
                    tunnel.close();
                }
            }
        }
        Ok(())
    }

    /// 从各源任务隧道拉取全部数据块并合并为输出行。
    fn receive(&self, sources: &[i64]) -> Result<ExecutionOutput, CopError> {
        let context = self.context.as_ref().ok_or(CopError::Unsupported(
            "exchange receiver without MPP context",
        ))?;
        let mut rows = Vec::new();
        for source in sources {
            let tunnel = context
                .handler
                .tunnel(*source, context.task.task_id)
                .ok_or_else(|| CopError::Tunnel(format!("source tunnel {source} not found")))?;
            tunnel.connect();
            while let Some(packet) = tunnel.recv_chunk()? {
                rows.extend(packet.data);
            }
        }
        Ok(ExecutionOutput {
            rows,
            ..ExecutionOutput::default()
        })
    }
}

/// 处理带 MPP 上下文的 DAG 请求：根执行器走 `MppExecBuilder`，否则回退普通 Cop 请求。
pub fn handle_mpp_dag_request(
    reader: &dyn KvReader,
    request: &Request,
    context: MppContext<'_>,
) -> Response {
    let RequestPayload::Dag(DagRequest {
        root: Some(root), ..
    }) = &request.payload
    else {
        return handle_cop_request(reader, request);
    };
    match MppExecBuilder::new(reader, &request.ranges, request.start_ts)
        .with_context(context)
        .build_and_execute(root)
    {
        Ok(output) => Response {
            chunks: output
                .rows
                .chunks(64)
                .map(|rows| crate::cop_handler::Chunk {
                    rows: rows.to_vec(),
                })
                .collect(),
            range_counts: output.range_counts,
            ndvs: output.ndvs,
            summaries: output.summaries,
            ..Response::default()
        },
        Err(error) => Response {
            other_error: Some(error.to_string()),
            ..Response::default()
        },
    }
}

/// 对行 Datum 做 FNV-1a 哈希，供 Hash Exchange 分区选槽。
fn row_hash(row: &[crate::cop_handler::Datum]) -> u64 {
    let mut bytes = Vec::new();
    for datum in row {
        datum.encode(&mut bytes);
    }
    // FNV-1a 64 位初始偏移与素数乘法。
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

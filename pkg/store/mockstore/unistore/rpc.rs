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

// unistore 进程内 RPC 客户端：将 Request 分发到 Server / RawHandler。
//
// 覆盖 TiKV KV（含两阶段提交 Prewrite/Commit）、Raw KV、Coprocessor、
// MPP 与调试类命令；支持 failpoint 模拟超时，以及非持久模式下关闭时清理路径。

use crate::cluster::Cluster;
use crate::raw_handler::{KvPair as RawKvPair, RawGetResponse, RawHandler};
use crate::tikv::mock_region::Region;
use crate::tikv::mvcc::{
    KvPair, Lock, MAX_SYSTEM_TS, MvccInfo, MvccStore, PessimisticLockRequest,
    PessimisticLockResult, PrewriteRequest, PrewriteResult, SecondaryLocksStatus, TxnStatus,
};
use crate::tikv::server::{RpcContext, RpcResponse, Server};
use std::collections::VecDeque;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// RPC 命令类型枚举，与 Request 变体一一对应。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandType {
    Get,
    Scan,
    Prewrite,
    PessimisticLock,
    PessimisticRollback,
    Commit,
    Cleanup,
    CheckTxnStatus,
    CheckSecondaryLocks,
    TxnHeartBeat,
    BatchGet,
    BatchRollback,
    ScanLock,
    ResolveLock,
    Gc,
    DeleteRange,
    RawGet,
    RawBatchGet,
    RawPut,
    RawBatchPut,
    RawDelete,
    RawBatchDelete,
    RawDeleteRange,
    RawScan,
    Cop,
    CopStream,
    BatchCop,
    MppConn,
    MppTask,
    MppCancel,
    MppAlive,
    MvccGetByKey,
    MvccGetByStartTs,
    SplitRegion,
    DebugGetRegionProperties,
    StoreSafeTs,
    UnsafeDestroyRange,
    Flush,
    BufferBatchGet,
    Empty,
}

/// 统一请求体：MVCC/事务、Raw、Coprocessor、MPP 与元数据调试命令。
pub enum Request {
    Get {
        context: RpcContext,
        key: Vec<u8>,
        version: u64,
    },
    Scan {
        context: RpcContext,
        start: Vec<u8>,
        end: Vec<u8>,
        version: u64,
        limit: usize,
        reverse: bool,
        key_only: bool,
    },
    Prewrite {
        context: RpcContext,
        request: PrewriteRequest,
    },
    PessimisticLock {
        context: RpcContext,
        request: PessimisticLockRequest,
    },
    PessimisticRollback {
        context: RpcContext,
        keys: Vec<Vec<u8>>,
        start_ts: u64,
        for_update_ts: u64,
    },
    Commit {
        context: RpcContext,
        keys: Vec<Vec<u8>>,
        start_ts: u64,
        commit_ts: u64,
    },
    Cleanup {
        context: RpcContext,
        key: Vec<u8>,
        start_ts: u64,
        current_ts: u64,
    },
    CheckTxnStatus {
        context: RpcContext,
        primary: Vec<u8>,
        start_ts: u64,
        caller_start_ts: u64,
        current_ts: u64,
        rollback_if_not_exist: bool,
    },
    CheckSecondaryLocks {
        context: RpcContext,
        keys: Vec<Vec<u8>>,
        start_ts: u64,
    },
    TxnHeartBeat {
        context: RpcContext,
        primary: Vec<u8>,
        start_ts: u64,
        ttl: u64,
    },
    BatchGet {
        context: RpcContext,
        keys: Vec<Vec<u8>>,
        version: u64,
    },
    BatchRollback {
        context: RpcContext,
        keys: Vec<Vec<u8>>,
        start_ts: u64,
    },
    ScanLock {
        context: RpcContext,
        start: Vec<u8>,
        end: Vec<u8>,
        max_ts: u64,
        limit: usize,
    },
    ResolveLock {
        context: RpcContext,
        start_ts: u64,
        commit_ts: u64,
    },
    Gc {
        context: RpcContext,
        safe_point: u64,
    },
    DeleteRange {
        context: RpcContext,
        start: Vec<u8>,
        end: Vec<u8>,
    },
    RawGet {
        key: Vec<u8>,
    },
    RawBatchGet {
        keys: Vec<Vec<u8>>,
    },
    RawPut {
        key: Vec<u8>,
        value: Vec<u8>,
    },
    RawBatchPut {
        pairs: Vec<RawKvPair>,
    },
    RawDelete {
        key: Vec<u8>,
    },
    RawBatchDelete {
        keys: Vec<Vec<u8>>,
    },
    RawDeleteRange {
        start: Vec<u8>,
        end: Vec<u8>,
    },
    RawScan {
        start: Vec<u8>,
        end: Vec<u8>,
        limit: usize,
        reverse: bool,
    },
    Cop {
        context: RpcContext,
        data: Vec<u8>,
        start_key: Vec<u8>,
        start_ts: u64,
    },
    CopStream {
        context: RpcContext,
        data: Vec<u8>,
        start_key: Vec<u8>,
        start_ts: u64,
    },
    BatchCop {
        context: RpcContext,
        requests: Vec<Vec<u8>>,
        start_ts: u64,
    },
    MppConn {
        store_id: u64,
        task_id: i64,
    },
    MppTask {
        store_id: u64,
        task_id: i64,
        payload: Vec<u8>,
    },
    MppCancel {
        store_id: u64,
        task_id: i64,
    },
    MppAlive,
    MvccGetByKey {
        context: RpcContext,
        key: Vec<u8>,
    },
    MvccGetByStartTs {
        context: RpcContext,
        start_ts: u64,
    },
    SplitRegion {
        context: RpcContext,
        keys: Vec<Vec<u8>>,
    },
    DebugGetRegionProperties {
        region_id: u64,
    },
    StoreSafeTs,
    UnsafeDestroyRange,
    Flush {
        context: RpcContext,
        request: PrewriteRequest,
    },
    BufferBatchGet {
        context: RpcContext,
        keys: Vec<Vec<u8>>,
        version: u64,
    },
    Empty,
}

impl Request {
    /// 返回本请求对应的 CommandType。
    pub fn command_type(&self) -> CommandType {
        match self {
            Self::Get { .. } => CommandType::Get,
            Self::Scan { .. } => CommandType::Scan,
            Self::Prewrite { .. } => CommandType::Prewrite,
            Self::PessimisticLock { .. } => CommandType::PessimisticLock,
            Self::PessimisticRollback { .. } => CommandType::PessimisticRollback,
            Self::Commit { .. } => CommandType::Commit,
            Self::Cleanup { .. } => CommandType::Cleanup,
            Self::CheckTxnStatus { .. } => CommandType::CheckTxnStatus,
            Self::CheckSecondaryLocks { .. } => CommandType::CheckSecondaryLocks,
            Self::TxnHeartBeat { .. } => CommandType::TxnHeartBeat,
            Self::BatchGet { .. } => CommandType::BatchGet,
            Self::BatchRollback { .. } => CommandType::BatchRollback,
            Self::ScanLock { .. } => CommandType::ScanLock,
            Self::ResolveLock { .. } => CommandType::ResolveLock,
            Self::Gc { .. } => CommandType::Gc,
            Self::DeleteRange { .. } => CommandType::DeleteRange,
            Self::RawGet { .. } => CommandType::RawGet,
            Self::RawBatchGet { .. } => CommandType::RawBatchGet,
            Self::RawPut { .. } => CommandType::RawPut,
            Self::RawBatchPut { .. } => CommandType::RawBatchPut,
            Self::RawDelete { .. } => CommandType::RawDelete,
            Self::RawBatchDelete { .. } => CommandType::RawBatchDelete,
            Self::RawDeleteRange { .. } => CommandType::RawDeleteRange,
            Self::RawScan { .. } => CommandType::RawScan,
            Self::Cop { .. } => CommandType::Cop,
            Self::CopStream { .. } => CommandType::CopStream,
            Self::BatchCop { .. } => CommandType::BatchCop,
            Self::MppConn { .. } => CommandType::MppConn,
            Self::MppTask { .. } => CommandType::MppTask,
            Self::MppCancel { .. } => CommandType::MppCancel,
            Self::MppAlive => CommandType::MppAlive,
            Self::MvccGetByKey { .. } => CommandType::MvccGetByKey,
            Self::MvccGetByStartTs { .. } => CommandType::MvccGetByStartTs,
            Self::SplitRegion { .. } => CommandType::SplitRegion,
            Self::DebugGetRegionProperties { .. } => CommandType::DebugGetRegionProperties,
            Self::StoreSafeTs => CommandType::StoreSafeTs,
            Self::UnsafeDestroyRange => CommandType::UnsafeDestroyRange,
            Self::Flush { .. } => CommandType::Flush,
            Self::BufferBatchGet { .. } => CommandType::BufferBatchGet,
            Self::Empty => CommandType::Empty,
        }
    }
}

/// 统一响应体：按命令包装 RpcResponse、流式 MockStream 或 Raw 结果。
pub enum Response {
    Get(RpcResponse<Option<Vec<u8>>>),
    Scan(RpcResponse<Vec<KvPair>>),
    Prewrite(RpcResponse<PrewriteResult>),
    PessimisticLock(RpcResponse<PessimisticLockResult>),
    Unit(RpcResponse<()>),
    Timestamp(RpcResponse<u64>),
    TxnStatus(RpcResponse<TxnStatus>),
    SecondaryLocks(RpcResponse<SecondaryLocksStatus>),
    BatchGet(RpcResponse<Vec<KvPair>>),
    Locks(RpcResponse<Vec<(Vec<u8>, Lock)>>),
    RawGet(RawGetResponse),
    RawPairs(Vec<RawKvPair>),
    Cop(RpcResponse<Vec<u8>>),
    CopStream(MockStream<RpcResponse<Vec<u8>>>),
    BatchCop(MockStream<RpcResponse<Vec<u8>>>),
    MppStream(MockStream<Vec<u8>>),
    MppAlive(bool),
    MvccInfo(RpcResponse<MvccInfo>),
    MvccByStartTs(RpcResponse<Option<(Vec<u8>, MvccInfo)>>),
    Regions(RpcResponse<Vec<Region>>),
    RegionProperties(Vec<(String, String)>),
    SafeTs(u64),
    Empty,
}

/// RPC 层错误：取消、不支持、Region/Server/IO 与流结束。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RpcError {
    Cancelled,
    Unsupported(CommandType),
    Region(String),
    Server(String),
    Io(String),
    EndOfStream,
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for RpcError {}
pub type Result<T> = std::result::Result<T, RpcError>;

/// 进程内 RPC 客户端：持有 Server、Cluster、路径与独立 RawHandler。
pub struct RPCClient {
    server: Arc<Server>,
    cluster: Arc<Cluster>,
    path: PathBuf,
    raw_handler: RawHandler,
    persistent: bool,
    closed: AtomicBool,
}

impl RPCClient {
    /// 构造客户端；raw_handler 初始为空，closed 为 false。
    pub fn new(
        server: Arc<Server>,
        cluster: Arc<Cluster>,
        path: PathBuf,
        persistent: bool,
    ) -> Self {
        Self {
            server,
            cluster,
            path,
            raw_handler: RawHandler::new(),
            persistent,
            closed: AtomicBool::new(false),
        }
    }

    /// 同步发送：校验地址、可选 failpoint 超时，再 dispatch。
    pub fn send_request(
        &self,
        address: &str,
        request: Request,
        timeout: Duration,
    ) -> Result<Response> {
        if self.closed.load(Ordering::Acquire) {
            return Err(RpcError::Cancelled);
        }
        // 短超时 + failpoint 命中时模拟 DeadlineExceeded。
        if timeout < Duration::from_secs(1)
            && fail::eval(
                "github.com/pingcap/tidb/pkg/store/mockstore/unistore/unistoreRPCDeadlineExceeded",
                |_| true,
            )
            .unwrap_or(false)
        {
            return Err(RpcError::Server("Deadline is exceeded".to_owned()));
        }
        self.server
            .get_store_id_by_address(address)
            .map_err(|error| RpcError::Region(error.to_string()))?;
        self.dispatch(request)
    }

    /// 在独立线程中异步发送，完成后调用 callback。
    pub fn send_request_async(
        self: &Arc<Self>,
        address: String,
        request: Request,
        callback: impl FnOnce(Result<Response>) + Send + 'static,
    ) {
        let client = Arc::clone(self);
        std::thread::spawn(move || {
            callback(client.send_request(&address, request, Duration::from_secs(2)));
        });
    }

    /// 按 Request 变体路由到 Server.kv_* / RawHandler / Coprocessor / MPP。
    fn dispatch(&self, request: Request) -> Result<Response> {
        Ok(match request {
            Request::Get {
                context,
                key,
                version,
            } => Response::Get(self.server.kv_get(&context, key, version)),
            Request::Scan {
                context,
                start,
                end,
                version,
                limit,
                reverse,
                key_only,
            } => Response::Scan(
                self.server
                    .kv_scan(&context, &start, &end, version, limit, reverse, key_only),
            ),
            Request::Prewrite { context, request } => {
                // Prewrite（两阶段提交第一阶段）前注入 Cluster 一次性延迟。
                self.cluster
                    .handle_delay(request.start_ts, context.region.region_id);
                Response::Prewrite(self.server.kv_prewrite(&context, &request))
            }
            Request::PessimisticLock { context, request } => {
                self.cluster
                    .handle_delay(request.start_ts, context.region.region_id);
                Response::PessimisticLock(self.server.kv_pessimistic_lock(&context, &request))
            }
            Request::PessimisticRollback {
                context,
                keys,
                start_ts,
                for_update_ts,
            } => Response::Unit(self.server.kv_pessimistic_rollback(
                &context,
                &keys,
                start_ts,
                for_update_ts,
            )),
            Request::Commit {
                context,
                keys,
                start_ts,
                commit_ts,
            } => Response::Unit(self.server.kv_commit(&context, &keys, start_ts, commit_ts)),
            Request::Cleanup {
                context,
                key,
                start_ts,
                current_ts,
            } => Response::Unit(self.server.kv_cleanup(&context, &key, start_ts, current_ts)),
            Request::CheckTxnStatus {
                context,
                primary,
                start_ts,
                caller_start_ts,
                current_ts,
                rollback_if_not_exist,
            } => Response::TxnStatus(self.server.kv_check_txn_status(
                &context,
                &primary,
                start_ts,
                caller_start_ts,
                current_ts,
                rollback_if_not_exist,
            )),
            Request::CheckSecondaryLocks {
                context,
                keys,
                start_ts,
            } => Response::SecondaryLocks(
                self.server
                    .kv_check_secondary_locks(&context, &keys, start_ts),
            ),
            Request::TxnHeartBeat {
                context,
                primary,
                start_ts,
                ttl,
            } => Response::Timestamp(
                self.server
                    .kv_txn_heartbeat(&context, &primary, start_ts, ttl),
            ),
            Request::BatchGet {
                context,
                keys,
                version,
            }
            | Request::BufferBatchGet {
                context,
                keys,
                version,
            } => Response::BatchGet(self.server.kv_batch_get(&context, &keys, version)),
            Request::BatchRollback {
                context,
                keys,
                start_ts,
            } => Response::Unit(self.server.kv_batch_rollback(&context, &keys, start_ts)),
            Request::ScanLock {
                context,
                start,
                end,
                max_ts,
                limit,
            } => Response::Locks(
                self.server
                    .kv_scan_lock(&context, &start, &end, max_ts, limit),
            ),
            Request::ResolveLock {
                context,
                start_ts,
                commit_ts,
            } => Response::Unit(self.server.kv_resolve_lock(&context, start_ts, commit_ts)),
            Request::Gc {
                context,
                safe_point,
            } => Response::Unit(self.server.kv_gc(&context, safe_point)),
            Request::DeleteRange {
                context,
                start,
                end,
            } => Response::Unit(self.server.kv_delete_range(&context, &start, &end)),
            Request::RawGet { key } => Response::RawGet(self.raw_handler.raw_get(&key)),
            Request::RawBatchGet { keys } => {
                Response::RawPairs(self.raw_handler.raw_batch_get(&keys))
            }
            // Raw 写路径走独立 RawHandler，不经 MVCC。
            Request::RawPut { key, value } => {
                self.raw_handler.raw_put(key, value);
                Response::Empty
            }
            Request::RawBatchPut { pairs } => {
                self.raw_handler.raw_batch_put(&pairs);
                Response::Empty
            }
            Request::RawDelete { key } => {
                self.raw_handler.raw_delete(&key);
                Response::Empty
            }
            Request::RawBatchDelete { keys } => {
                self.raw_handler.raw_batch_delete(&keys);
                Response::Empty
            }
            Request::RawDeleteRange { start, end } => {
                self.raw_handler.raw_delete_range(&start, &end);
                Response::Empty
            }
            Request::RawScan {
                start,
                end,
                limit,
                reverse,
            } => Response::RawPairs(self.raw_handler.raw_scan(&start, &end, limit, reverse)),
            Request::Cop {
                context,
                data,
                start_ts,
                ..
            } => Response::Cop(self.server.coprocessor(&context, &data, start_ts)),
            Request::CopStream {
                context,
                data,
                start_ts,
                ..
            } => {
                // 流式 Cop：单次执行结果包装为仅含一条的 MockStream。
                let response = self.server.coprocessor(&context, &data, start_ts);
                Response::CopStream(MockStream::new(vec![response]))
            }
            Request::BatchCop {
                context,
                requests,
                start_ts,
            } => {
                let responses = requests
                    .into_iter()
                    .map(|request| self.server.coprocessor(&context, &request, start_ts))
                    .collect();
                Response::BatchCop(MockStream::new(responses))
            }
            Request::MppConn { store_id, task_id } => {
                let packets = self
                    .server
                    .establish_mpp_connection(store_id, task_id)
                    .map_err(RpcError::Server)?;
                Response::MppStream(MockStream::new(packets))
            }
            Request::MppTask {
                store_id,
                task_id,
                payload,
            } => {
                self.server
                    .create_mpp_task(store_id, task_id, payload)
                    .map_err(RpcError::Server)?;
                Response::Empty
            }
            Request::MppCancel { store_id, task_id } => {
                self.server
                    .cancel_mpp_task(store_id, task_id)
                    .map_err(RpcError::Server)?;
                Response::Empty
            }
            Request::MppAlive => Response::MppAlive(true),
            Request::MvccGetByKey { context, key } => {
                Response::MvccInfo(self.server.mvcc_get_by_key(&context, &key))
            }
            Request::MvccGetByStartTs { context, start_ts } => {
                Response::MvccByStartTs(self.server.mvcc_get_by_start_ts(&context, start_ts))
            }
            Request::SplitRegion { context, keys } => {
                Response::Regions(self.server.split_region(&context, keys))
            }
            Request::DebugGetRegionProperties { region_id } => {
                let region = self
                    .cluster
                    .region_manager()
                    .get_region(region_id)
                    .ok_or_else(|| RpcError::Region(format!("region {region_id} not found")))?;
                let start = decode_bytes(&region.start_key)?;
                let end = decode_bytes(&region.end_key)?;
                let rows = self.server.mvcc_store().scan(
                    &start,
                    &end,
                    MAX_SYSTEM_TS,
                    u32::MAX as usize,
                    false,
                    false,
                    &[],
                );
                Response::RegionProperties(vec![("mvcc.num_rows".into(), rows.len().to_string())])
            }
            Request::StoreSafeTs => Response::SafeTs(0),
            Request::UnsafeDestroyRange => Response::Empty,
            Request::Flush { context, request } => {
                self.cluster
                    .handle_delay(request.start_ts, context.region.region_id);
                Response::Prewrite(self.server.kv_flush(&context, &request))
            }
            Request::Empty => return Err(RpcError::Unsupported(CommandType::Empty)),
        })
    }

    /// 幂等关闭：停 Server；非 persistent 时删除数据目录。
    pub fn close(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.server.stop().map_err(RpcError::Server)?;
        if !self.persistent && self.path.exists() {
            fs::remove_dir_all(&self.path).map_err(|error| RpcError::Io(error.to_string()))?;
        }
        Ok(())
    }

    /// 按地址关闭连接的占位实现（进程内无真实连接）。
    pub fn close_addr(&self, _address: &str) -> Result<()> {
        Ok(())
    }

    /// 返回内部 RawHandler 引用。
    pub fn raw_handler(&self) -> &RawHandler {
        &self.raw_handler
    }

    /// 返回 Server 持有的 MvccStore。
    pub fn mvcc_store(&self) -> Arc<MvccStore> {
        self.server.mvcc_store()
    }

    /// 克隆 Server 的 Arc。
    pub fn server(&self) -> Arc<Server> {
        Arc::clone(&self.server)
    }
}

impl Drop for RPCClient {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

/// Decode TiDB's memcomparable byte encoding used by Region boundaries.
fn decode_bytes(encoded: &[u8]) -> Result<Vec<u8>> {
    const GROUP: usize = 8;
    if encoded.is_empty() {
        return Ok(Vec::new());
    }
    if !encoded.len().is_multiple_of(GROUP + 1) {
        return Err(RpcError::Server("invalid encoded region boundary".into()));
    }
    let mut decoded = Vec::with_capacity(encoded.len());
    for group in encoded.chunks_exact(GROUP + 1) {
        let marker = group[GROUP];
        let padding = 0xff_u8
            .checked_sub(marker)
            .filter(|padding| *padding <= GROUP as u8)
            .ok_or_else(|| RpcError::Server("invalid encoded region boundary marker".into()))?
            as usize;
        if padding > 0 && group[GROUP - padding..GROUP].iter().any(|byte| *byte != 0) {
            return Err(RpcError::Server(
                "invalid encoded region boundary padding".into(),
            ));
        }
        decoded.extend_from_slice(&group[..GROUP - padding]);
        if padding > 0 {
            return Ok(decoded);
        }
    }
    Err(RpcError::Server(
        "unterminated encoded region boundary".into(),
    ))
}

/// 内存中的简易响应流，用于 CopStream/BatchCop/MPP。
pub struct MockStream<T> {
    responses: VecDeque<T>,
}

impl<T> MockStream<T> {
    /// 用给定响应序列构造流。
    pub fn new(responses: Vec<T>) -> Self {
        Self {
            responses: responses.into(),
        }
    }

    /// 弹出下一条响应；空则返回 EndOfStream。
    pub fn recv(&mut self) -> Result<T> {
        self.responses.pop_front().ok_or(RpcError::EndOfStream)
    }
}

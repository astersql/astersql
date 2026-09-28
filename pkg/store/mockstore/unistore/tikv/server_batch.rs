// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// Batch 命令分发：把批量 KV/事务/Raw 请求映射到 `Server` RPC。
//
// 批处理在线程作用域内并行执行各子请求，再按完成顺序组装响应；每条响应
// 与其 request_id 保持同一位置，以模拟 TiKV BatchCommands 的乱序完成语义。

use crate::mvcc::{KvPair, PessimisticLockRequest, PrewriteRequest};
use crate::server::{RpcContext, Server};
use std::sync::mpsc;

/// 批请求通道容量。
pub const REQUEST_CHANNEL_SIZE: usize = 1024;
/// 批响应通道容量。
pub const RESPONSE_CHANNEL_SIZE: usize = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单条批命令：覆盖点查、扫描、悲观锁、两阶段提交与 Raw KV。
pub enum BatchCommand {
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
    Prewrite {
        context: RpcContext,
        request: PrewriteRequest,
    },
    Commit {
        context: RpcContext,
        keys: Vec<Vec<u8>>,
        start_ts: u64,
        commit_ts: u64,
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
    ResolveLock {
        context: RpcContext,
        start_ts: u64,
        commit_ts: u64,
    },
    RawGet {
        key: Vec<u8>,
        now_ts: u64,
    },
    RawPut {
        key: Vec<u8>,
        value: Vec<u8>,
        ttl: Option<u64>,
        now_ts: u64,
    },
    RawDelete {
        key: Vec<u8>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 单条批命令的响应：值、键值对列表、空成功或错误串。
pub enum BatchCommandResponse {
    Value(Option<Vec<u8>>),
    Pairs(Vec<KvPair>),
    Empty,
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 带 request_id 的单条响应，便于保持完成顺序及响应关联。
pub struct ResponseIdPair {
    pub request_id: u64,
    pub response: BatchCommandResponse,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 整批响应：与请求一一对应的 id 与结果列表。
pub struct BatchResponse {
    pub request_ids: Vec<u64>,
    pub responses: Vec<BatchCommandResponse>,
}
impl ResponseIdPair {
    /// 将本对追加到批量响应中。
    pub fn append_to(self, response: &mut BatchResponse) {
        response.request_ids.push(self.request_id);
        response.responses.push(self.response);
    }
}

/// 持有 `Server` 引用的批请求处理器。
pub struct BatchRequestHandler<'a> {
    server: &'a Server,
}
impl<'a> BatchRequestHandler<'a> {
    /// 创建处理器。
    pub fn new(server: &'a Server) -> Self {
        Self { server }
    }
    /// 并行分发批请求，按完成顺序组装 `BatchResponse`。
    pub fn dispatch_batch(&self, requests: Vec<(u64, BatchCommand)>) -> BatchResponse {
        let (sender, receiver) = mpsc::sync_channel(RESPONSE_CHANNEL_SIZE);
        // 在作用域内为每条请求起线程，结束后按响应到达顺序收集。
        std::thread::scope(|scope| {
            for (id, request) in requests {
                let sender = sender.clone();
                let server = self.server;
                scope.spawn(move || {
                    let response = handle_batch_request(server, request);
                    let _ = sender.send(ResponseIdPair {
                        request_id: id,
                        response,
                    });
                });
            }
            drop(sender);
            collect_batch_response(receiver)
        })
    }
}

pub(crate) fn collect_batch_response(
    pairs: impl IntoIterator<Item = ResponseIdPair>,
) -> BatchResponse {
    let mut response = BatchResponse::default();
    for pair in pairs {
        pair.append_to(&mut response);
    }
    response
}

/// 将单条 `BatchCommand` 转调到对应的 `Server` 方法。
pub fn handle_batch_request(server: &Server, request: BatchCommand) -> BatchCommandResponse {
    match request {
        BatchCommand::Get {
            context,
            key,
            version,
        } => {
            let response = server.kv_get(&context, key, version);
            match response.value {
                Some(value) => BatchCommandResponse::Value(value),
                None => error(response.key_error, response.region_error),
            }
        }
        BatchCommand::Scan {
            context,
            start,
            end,
            version,
            limit,
            reverse,
            key_only,
        } => {
            let response =
                server.kv_scan(&context, &start, &end, version, limit, reverse, key_only);
            match response.value {
                Some(value) => BatchCommandResponse::Pairs(value),
                None => error(response.key_error, response.region_error),
            }
        }
        BatchCommand::PessimisticLock { context, request } => {
            let response = server.kv_pessimistic_lock(&context, &request);
            unit_rpc(response.key_error, response.region_error)
        }
        BatchCommand::PessimisticRollback {
            context,
            keys,
            start_ts,
            for_update_ts,
        } => {
            let response = server.kv_pessimistic_rollback(&context, &keys, start_ts, for_update_ts);
            unit_rpc(response.key_error, response.region_error)
        }
        BatchCommand::Prewrite { context, request } => {
            let response = server.kv_prewrite(&context, &request);
            unit_rpc(response.key_error, response.region_error)
        }
        BatchCommand::Commit {
            context,
            keys,
            start_ts,
            commit_ts,
        } => {
            let response = server.kv_commit(&context, &keys, start_ts, commit_ts);
            unit_rpc(response.key_error, response.region_error)
        }
        BatchCommand::BatchGet {
            context,
            keys,
            version,
        } => {
            let response = server.kv_batch_get(&context, &keys, version);
            response
                .value
                .map(BatchCommandResponse::Pairs)
                .unwrap_or_else(|| error(response.key_error, response.region_error))
        }
        BatchCommand::BatchRollback {
            context,
            keys,
            start_ts,
        } => {
            let response = server.kv_batch_rollback(&context, &keys, start_ts);
            unit_rpc(response.key_error, response.region_error)
        }
        BatchCommand::ResolveLock {
            context,
            start_ts,
            commit_ts,
        } => {
            let response = server.kv_resolve_lock(&context, start_ts, commit_ts);
            unit_rpc(response.key_error, response.region_error)
        }
        BatchCommand::RawGet { key, now_ts } => {
            BatchCommandResponse::Value(server.raw_get(&key, now_ts))
        }
        BatchCommand::RawPut {
            key,
            value,
            ttl,
            now_ts,
        } => {
            server.raw_put(key, value, ttl, now_ts);
            BatchCommandResponse::Empty
        }
        BatchCommand::RawDelete { key } => {
            server.raw_delete(&key);
            BatchCommandResponse::Empty
        }
    }
}

/// 无返回值 RPC：无错误则 Empty，否则转为 Error 响应。
fn unit_rpc(
    key_error: Option<crate::server::KeyError>,
    region_error: Option<crate::mock_region::RegionError>,
) -> BatchCommandResponse {
    if key_error.is_none() && region_error.is_none() {
        BatchCommandResponse::Empty
    } else {
        error(key_error, region_error)
    }
}
/// 优先取 KeyError 消息，否则 RegionError，都无则占位串。
fn error(
    key_error: Option<crate::server::KeyError>,
    region_error: Option<crate::mock_region::RegionError>,
) -> BatchCommandResponse {
    BatchCommandResponse::Error(
        key_error
            .map(|error| error.message)
            .or_else(|| region_error.map(|error| error.to_string()))
            .unwrap_or_else(|| "empty response".into()),
    )
}

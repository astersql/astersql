// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// AutoID 远程服务客户端与单点分配器。
//
// 当 `auto_id_cache = 1`（单点模式）时，节点不再本地批量缓存 ID，而是通过 gRPC
// 向 AutoID Leader 服务申请。本模块提供：etcd Leader 路径解析、客户端发现
// （[`ClientDiscover`]）、指数退避（[`Backoffer`]），以及实现 [`Allocator`] 的
// [`SinglePointAllocator`]。RPC 失败时按版本号重置连接并重试；上下文取消则快速返回。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use crate::autoid::{Allocator, AllocatorType, Context, valid_increment_and_offset};
use crate::errors::{AutoIdError, Result, autoinc_read_failed, invalid_increment_and_offset};

/// etcd 中 AutoID Leader 的默认键路径。
pub const AUTO_ID_LEADER_PATH: &str = "tidb/autoid/leader";
/// 表示“无独立 keyspace”的哨兵 ID（对应 Go 的 NullspaceID）。
pub const NULLSPACE_ID: u32 = u32::MAX;

/// 按 keyspace 生成 etcd Leader 路径；Nullspace 用裸路径，否则加前导 `/`。
pub fn get_auto_id_service_leader_etcd_path(keyspace_id: u32) -> String {
    if keyspace_id == NULLSPACE_ID {
        AUTO_ID_LEADER_PATH.to_owned()
    } else {
        format!("/{AUTO_ID_LEADER_PATH}")
    }
}

/// 向 AutoID 服务申请一批 ID 的请求参数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AutoIdRequest {
    pub database_id: i64,
    pub table_id: i64,
    pub n: u64,
    pub increment: i64,
    pub offset: i64,
    pub is_unsigned: bool,
    pub keyspace_id: u32,
}

/// 分配结果：可用区间为 `(min, max]`；`errmsg` 非空表示服务端业务错误。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AutoIdResponse {
    pub min: i64,
    pub max: i64,
    pub errmsg: String,
}

/// 将全局 AutoID 水位 rebase（抬高）到指定 base 的请求。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebaseRequest {
    pub database_id: i64,
    pub table_id: i64,
    pub base: i64,
    pub force: bool,
    pub is_unsigned: bool,
}

/// Rebase 响应；`errmsg` 非空表示失败原因。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RebaseResponse {
    pub errmsg: String,
}

/// 从 etcd（或等价注册中心）解析当前 AutoID Leader 地址。
pub trait LeaderDiscovery: Send + Sync {
    fn leader(&self, ctx: &Context, path: &str) -> Result<Option<String>>;
}

/// 与 AutoID Leader 通信的 RPC 客户端抽象。
pub trait AutoIdClient: Send + Sync {
    fn alloc_auto_id(&self, ctx: &Context, request: AutoIdRequest) -> Result<AutoIdResponse>;
    fn rebase(&self, ctx: &Context, request: RebaseRequest) -> Result<RebaseResponse>;
}

/// 底层连接句柄，用于重置时异步关闭。
pub trait ClientConnection: Send + Sync {
    fn close(&self) -> Result<()>;
}

/// 按 Leader 地址建立客户端与连接。
pub trait AutoIdClientConnector: Send + Sync {
    fn connect(&self, address: &str) -> Result<(Arc<dyn AutoIdClient>, Arc<dyn ClientConnection>)>;
}

/// 已缓存的客户端与连接。
#[derive(Default)]
struct ClientState {
    client: Option<Arc<dyn AutoIdClient>>,
    connection: Option<Arc<dyn ClientConnection>>,
}

/// 客户端发现器：懒加载连接、按版本重置、带退避的 Leader 轮询。
pub struct ClientDiscover {
    discovery: Arc<dyn LeaderDiscovery>,
    connector: Arc<dyn AutoIdClientConnector>,
    state: RwLock<ClientState>,
    version: AtomicU64,
}

impl ClientDiscover {
    /// 用 Leader 发现与连接器构造发现器。
    pub fn new(
        discovery: Arc<dyn LeaderDiscovery>,
        connector: Arc<dyn AutoIdClientConnector>,
    ) -> Self {
        Self {
            discovery,
            connector,
            state: RwLock::new(ClientState::default()),
            version: AtomicU64::new(0),
        }
    }

    /// 当前连接世代版本；重置连接时递增，用于避免过期重置。
    pub fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Corresponds to Go tests that assign `ClientDiscover.mu.AutoIDAllocClient` directly.
    /// 测试用：直接注入已有客户端，跳过 Leader 发现。
    pub fn seed_client_for_test(&self, client: Arc<dyn AutoIdClient>) {
        let mut state = self.state.write().unwrap();
        state.client = Some(client);
        state.connection = None;
    }

    /// 获取可用客户端；双检锁缓存，未命中则轮询 Leader 并连接。
    pub fn get_client(
        &self,
        ctx: &Context,
        keyspace_id: u32,
    ) -> Result<(Arc<dyn AutoIdClient>, u64)> {
        // 快路径：读锁下已有客户端。
        if let Some(client) = self.state.read().unwrap().client.clone() {
            return Ok((client, self.version()));
        }

        let mut state = self.state.write().unwrap();
        // 写锁下再次检查，避免并发重复建连。
        if let Some(client) = state.client.clone() {
            return Ok((client, self.version()));
        }

        let path = get_auto_id_service_leader_etcd_path(keyspace_id);
        let mut backoffer = Backoffer::default();
        // Leader 尚未选出时退避重试，直到拿到地址或上下文取消。
        let address = loop {
            ctx.check()?;
            if let Some(address) = self.discovery.leader(ctx, &path)? {
                break address;
            }
            backoffer.backoff(Some(ctx))?;
        };
        backoffer.reset();
        let (client, connection) = self.connector.connect(&address)?;
        state.client = Some(client.clone());
        state.connection = Some(connection);
        Ok((client, self.version()))
    }

    /// 仅当版本仍匹配时重置连接，防止过期失败误清新连接。
    pub fn reset_conn_if_version(&self, version: u64) {
        if self
            .version
            .compare_exchange(version, version + 1, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        self.reset_conn();
    }

    /// 清空缓存客户端，并延迟关闭旧连接，避免与进行中的 RPC 竞态。
    pub fn reset_conn(&self) {
        let connection = {
            let mut state = self.state.write().unwrap();
            state.client = None;
            state.connection.take()
        };
        if let Some(connection) = connection {
            // 延迟关闭：给 in-flight RPC 一点收尾时间。
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(200));
                let _ = connection.close();
            });
        }
    }
}

/// 指数退避状态：等待间隔在 [BACKOFF_MIN, BACKOFF_MAX] 内倍增。
#[derive(Default)]
pub struct Backoffer {
    pub duration: Duration,
}

/// 退避下限（5ms）。
const BACKOFF_MIN: Duration = Duration::from_millis(5);
/// 退避上限（100ms）。
const BACKOFF_MAX: Duration = Duration::from_millis(100);

impl Backoffer {
    /// 将间隔重置为最小值。
    pub fn reset(&mut self) {
        self.duration = BACKOFF_MIN;
    }

    /// 倍增间隔后休眠；若提供 Context 则使用可取消等待。
    pub fn backoff(&mut self, ctx: Option<&Context>) -> Result<()> {
        if self.duration.is_zero() {
            self.duration = BACKOFF_MIN;
        }
        self.duration = (self.duration * 2).min(BACKOFF_MAX);
        if let Some(ctx) = ctx {
            ctx.wait(self.duration)
        } else {
            thread::sleep(self.duration);
            Ok(())
        }
    }
}

/// 单点分配器的本地表身份与最近一次分配下限。
struct SinglePointState {
    database_id: i64,
    table_id: i64,
    last_allocated: i64,
}

/// 远程单点 AutoID 分配器：每次分配都走 Leader RPC，不本地批量缓存。
pub struct SinglePointAllocator {
    state: Mutex<SinglePointState>,
    is_unsigned: bool,
    discover: Arc<ClientDiscover>,
    keyspace_id: u32,
}

impl SinglePointAllocator {
    /// 绑定库表、是否无符号、keyspace 与客户端发现器。
    pub fn new(
        database_id: i64,
        table_id: i64,
        is_unsigned: bool,
        keyspace_id: u32,
        discover: Arc<ClientDiscover>,
    ) -> Self {
        Self {
            state: Mutex::new(SinglePointState {
                database_id,
                table_id,
                last_allocated: 0,
            }),
            is_unsigned,
            discover,
            keyspace_id,
        }
    }

    /// 内部 rebase：RPC 失败则重置连接并退避重试，直到成功或取消。
    fn rebase_inner(&self, ctx: &Context, new_base: i64, force: bool) -> Result<()> {
        let mut backoffer = Backoffer::default();
        loop {
            let (client, version) = self.discover.get_client(ctx, self.keyspace_id)?;
            let state = self.state.lock().unwrap();
            let request = RebaseRequest {
                database_id: state.database_id,
                table_id: state.table_id,
                base: new_base,
                force,
                is_unsigned: self.is_unsigned,
            };
            drop(state);
            match client.rebase(ctx, request) {
                Ok(response) => {
                    backoffer.reset();
                    if !response.errmsg.is_empty() {
                        return Err(AutoIdError::Service(response.errmsg));
                    }
                    self.state.lock().unwrap().last_allocated = new_base;
                    return Ok(());
                }
                Err(AutoIdError::Rpc(_message)) => {
                    // 上下文已取消则立即退出，避免长时间退避。
                    if ctx.is_canceled() {
                        return Err(AutoIdError::Canceled);
                    }
                    self.discover.reset_conn_if_version(version);
                    backoffer.backoff(Some(ctx))?;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

impl Allocator for SinglePointAllocator {
    fn alloc(&self, ctx: &Context, n: u64, increment: i64, offset: i64) -> Result<(i64, i64)> {
        if !valid_increment_and_offset(increment, offset) {
            return Err(invalid_increment_and_offset(increment, offset));
        }
        let mut backoffer = Backoffer::default();
        // 与 rebase_inner 相同的 RPC 重试环。
        loop {
            let (client, version) = self.discover.get_client(ctx, self.keyspace_id)?;
            let state = self.state.lock().unwrap();
            let request = AutoIdRequest {
                database_id: state.database_id,
                table_id: state.table_id,
                n,
                increment,
                offset,
                is_unsigned: self.is_unsigned,
                keyspace_id: self.keyspace_id,
            };
            drop(state);
            match client.alloc_auto_id(ctx, request) {
                Ok(response) => {
                    backoffer.reset();
                    if !response.errmsg.is_empty() {
                        return Err(AutoIdError::Service(response.errmsg));
                    }
                    self.state.lock().unwrap().last_allocated = response.min;
                    return Ok((response.min, response.max));
                }
                Err(AutoIdError::Rpc(_message)) => {
                    if ctx.is_canceled() {
                        return Err(AutoIdError::Canceled);
                    }
                    self.discover.reset_conn_if_version(version);
                    backoffer.backoff(Some(ctx))?;
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn alloc_seq_cache(&self) -> Result<(i64, i64, i64)> {
        Err(AutoIdError::NotImplemented(
            "AllocSeqCache not implemented".into(),
        ))
    }

    fn rebase(&self, ctx: &Context, new_base: i64, _allocate_ids: bool) -> Result<()> {
        self.rebase_inner(ctx, new_base, false)
    }

    fn force_rebase(&self, new_base: i64) -> Result<()> {
        if new_base == -1 {
            return Err(autoinc_read_failed(
                "Cannot force rebase the next global ID to '0'",
            ));
        }
        self.rebase_inner(&Context::background(), new_base, true)
    }

    fn rebase_seq(&self, _new_base: i64) -> Result<(i64, bool)> {
        Err(AutoIdError::NotImplemented(
            "RebaseSeq not implemented".into(),
        ))
    }

    fn transfer(&self, database_id: i64, table_id: i64) -> Result<()> {
        // 切换绑定的库表后，把下一全局水位同步到新身份。
        let next_base = {
            let mut state = self.state.lock().unwrap();
            if state.database_id == database_id && state.table_id == table_id {
                return Ok(());
            }
            state.database_id = database_id;
            state.table_id = table_id;
            state.last_allocated.wrapping_add(1)
        };
        self.rebase(&Context::background(), next_base, false)
    }

    fn base(&self) -> i64 {
        self.state.lock().unwrap().last_allocated
    }

    fn end(&self) -> i64 {
        self.state.lock().unwrap().last_allocated
    }

    fn next_global_auto_id(&self) -> Result<i64> {
        let (_, maximum) = self.alloc(&Context::background(), 0, 1, 1)?;
        Ok(maximum.wrapping_add(1))
    }

    fn get_type(&self) -> AllocatorType {
        AllocatorType::AutoIncrement
    }
}

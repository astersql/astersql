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

// 自增 ID 分配服务（autoid service）模块。
//
// 本模块实现了一个集中式的自增 ID（AUTO_INCREMENT）分配服务：
// 集群中通过 etcd 选举产生唯一的 leader（owner），由 leader 统一
// 从底层存储中按批（默认每批 4000 个）预取 ID 区间并缓存在内存中，
// 各节点通过 gRPC 请求向 leader 申请 ID 区段，从而避免每次插入
// 都访问存储，显著降低分配开销。
//
// 核心流程：
// - `Service::allocate`：处理分配请求，按需从存储事务中批量取号；
// - `Service::rebase_ids`：处理 rebase（重设基准值）请求，把存储中
//   记录的当前最大值抬高到指定基准之上；
// - `OwnerListener`：监听 leader 切换事件，成为新 owner 时清空
//   内存缓存以保证 ID 不重复。
//
// 术语说明：
// - 事务（transaction）：一组要么全部成功、要么全部回滚的存储操作，
//   这里用于原子地读取并递增存储中的 ID 计数器；
// - rebase：将自增计数器的基准值强制抬高（或指定）为某个值，
//   常见于用户显式插入了较大的 ID 之后，保证后续分配不会重复；
// - owner/leader：通过 etcd 分布式选举产生的唯一服务实例，
//   只有它有权执行分配，避免多节点并发写同一计数器。

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Instant;

use autoid_dependency::{
    AutoIdClient, AutoIdError, AutoIdKey, AutoIdKeyKind, AutoIdRequest as ClientAutoIdRequest,
    AutoIdResponse as ClientAutoIdResponse, Context, IdStore, RebaseRequest as ClientRebaseRequest,
    RebaseResponse as ClientRebaseResponse, Result,
};
use etcd_client::{Client as EtcdClient, ConnectOptions};
use grpcio::{RpcContext, RpcStatus, RpcStatusCode, UnarySink};
use kvproto::autoid::{AutoIdAlloc, AutoIdRequest, AutoIdResponse, RebaseRequest, RebaseResponse};
use owner_dependency::{Listener, Manager, NewOwnerManager, OwnerError};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

/// etcd 中用于 autoid leader 选举的键路径。
pub const AUTO_ID_LEADER_PATH: &str = "tidb/autoid/leader";
/// 每次从存储中批量预取的 ID 数量（步长下限）。
/// 预取越多，访问存储的次数越少，但节点故障时浪费的 ID 也越多。
pub const BATCH: i64 = 4000;
/// 自增操作失败时返回的错误消息文本。
const AUTO_INCREMENT_ACTION_FAILED: &str = "auto increment action failed";
/// MySQL 1467 标准错误；Go 的 `autoid.ErrAutoincReadFailed` 在无符号空间耗尽时返回此文案。
const AUTO_INCREMENT_READ_FAILED: &str =
    "[autoid:1467]Failed to read auto-increment value from storage engine";
/// 表信息版本 5，用于构造自增 ID 在存储中的键（不同版本的键编码不同）。
const TABLE_INFO_VERSION_5: u16 = 5;

/// 服务初始化阶段可能出现的错误：etcd 连接失败或 owner 选举失败。
#[derive(Debug, Error)]
pub enum InitError {
    #[error(transparent)]
    Etcd(#[from] etcd_client::Error),
    #[error(transparent)]
    Owner(#[from] OwnerError),
}

/// Storage surface used by the service. The transactional ID operations come
/// from `pkg/meta/autoid`; the remaining fields correspond to Go's Storage
/// codec and UUID methods.
///
/// 服务依赖的存储抽象：事务性 ID 读写能力继承自 `IdStore`，
/// 其余方法提供存储实例标识（UUID）、keyspace（多租户的键空间隔离
/// 单元）ID 以及 etcd 命名空间前缀。
pub trait AutoIdStorage: IdStore {
    /// 返回底层存储实例的唯一标识。
    fn uuid(&self) -> &str;
    /// 返回当前存储所属的 keyspace ID，用于校验请求归属。
    fn keyspace_id(&self) -> u32;
    /// 返回 etcd 键的命名空间前缀，用于隔离不同集群的选举路径。
    fn etcd_namespace(&self) -> String;
}

/// 内存缓存的键：以（数据库 ID，表 ID）唯一定位一个自增分配器。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct AutoIdCacheKey {
    database_id: i64,
    table_id: i64,
}

/// 单张表的自增 ID 分配器状态。
///
/// `base` 是已分配出去的最大值，`end` 是当前从存储中预取到的区间上界；
/// 区间 (base, end] 内的 ID 可以直接在内存中分配，无需访问存储。
struct AutoIdValue {
    /// 已分配出去的最大 ID（内存中的当前基准值）。
    base: i64,
    /// 已从存储预取的区间上界，`base` 追上 `end` 时需再取一批。
    end: i64,
    #[allow(dead_code)]
    is_unsigned: bool,
    #[allow(dead_code)]
    token: (SyncSender<()>, Receiver<()>),
}

impl AutoIdValue {
    /// 创建一个空的分配器状态，`base`/`end` 均为 0，首次分配时会从存储加载。
    fn new(is_unsigned: bool) -> Self {
        Self {
            base: 0,
            end: 0,
            is_unsigned,
            token: sync_channel(1),
        }
    }

    /// 构造该表自增计数器在存储中的键（使用表信息版本 5 的编码）。
    fn key(database_id: i64, table_id: i64) -> AutoIdKey {
        AutoIdKey {
            database_id,
            table_id,
            kind: AutoIdKeyKind::IncrementId(TABLE_INFO_VERSION_5),
        }
    }

    /// 为无符号自增列分配 `n` 个 ID，返回半开区间 `(min, max]`。
    ///
    /// `increment`/`offset` 对应 MySQL 的 auto_increment_increment 与
    /// auto_increment_offset，用于控制步长与起始偏移。所有比较与运算均
    /// 按 u64 语义进行，以正确处理超过 i64::MAX 的取值。
    fn allocate_unsigned(
        &mut self,
        store: &dyn AutoIdStorage,
        database_id: i64,
        table_id: i64,
        n: u64,
        increment: i64,
        offset: i64,
    ) -> Result<(i64, i64)> {
        // 若起始偏移超过当前基准，先把基准抬高到 offset-1，保证首个 ID 不小于 offset。
        if offset.wrapping_sub(1) as u64 > self.base as u64 {
            self.rebase_unsigned(store, database_id, table_id, offset.wrapping_sub(1) as u64)?;
        }
        // 结合步长与偏移计算本次实际需要占用的 ID 数量。
        let mut needed = calc_needed_batch_size(self.base, n as i64, increment, offset, true);
        // 剩余空间不足以容纳本次分配，视为溢出错误。
        if u64::MAX.wrapping_sub(self.base as u64) <= needed as u64 {
            return Err(standard_read_failed());
        }

        // 内存缓存区间不够（或尚未初始化）时，通过存储事务再预取一批。
        if (self.base as u64).wrapping_add(needed as u64) > self.end as u64 || self.base == 0 {
            let key = Self::key(database_id, table_id);
            let mut new_base = 0;
            let mut new_end = 0;
            let mut next_step = BATCH;
            let from_base = self.base;
            // 在存储事务中原子地读取当前计数器并递增，保证多副本间不重号。
            store.run_in_transaction(&mut |transaction| {
                new_base = transaction.get(key)?;
                // 缓存未初始化或存储值与缓存脱节（如被其他节点 rebase 过）时，
                // 以存储中的最新值为准重置缓存并重算所需数量。
                if self.base == 0 || new_base != self.end {
                    self.base = new_base;
                    self.end = new_base;
                    needed = calc_needed_batch_size(new_base, n as i64, increment, offset, true);
                }
                next_step = next_step.max(needed);
                // 步长受 u64::MAX 上限约束，防止计数器溢出。
                let step = u64::MAX.wrapping_sub(new_base as u64).min(next_step as u64) as i64;
                if step < needed {
                    return Err(read_failed());
                }
                new_end = transaction.inc(key, step)?;
                Ok(())
            })?;
            if new_base as u64 == u64::MAX {
                return Err(read_failed());
            }
            log::info!(
                target: "autoid_service",
                "alloc unsigned db={database_id} table={table_id} from_base={from_base} from_end={} to_base={new_base} to_end={new_end}",
                self.end,
            );
            self.end = new_end;
        }
        // 在内存中推进 base，返回本次分配的区间 (min, base]。
        let min = self.base;
        self.base = (self.base as u64).wrapping_add(needed as u64) as i64;
        Ok((min, self.base))
    }

    /// 为有符号自增列分配 `n` 个 ID，返回半开区间 `(min, max]`。
    ///
    /// 逻辑与 `allocate_unsigned` 相同，但所有比较与溢出检查按 i64 语义进行。
    fn allocate_signed(
        &mut self,
        store: &dyn AutoIdStorage,
        database_id: i64,
        table_id: i64,
        n: u64,
        increment: i64,
        offset: i64,
    ) -> Result<(i64, i64)> {
        // 若起始偏移超过当前基准，先把基准抬高到 offset-1。
        if offset.wrapping_sub(1) > self.base {
            self.rebase_signed(store, database_id, table_id, offset.wrapping_sub(1))?;
        }
        let mut needed = calc_needed_batch_size(self.base, n as i64, increment, offset, false);
        // 剩余空间不足，视为溢出错误。
        if i64::MAX.wrapping_sub(self.base) <= needed {
            return Err(read_failed());
        }

        // 缓存区间不足（或尚未初始化）时，通过存储事务再预取一批。
        if self.base.wrapping_add(needed) > self.end || self.base == 0 {
            let key = Self::key(database_id, table_id);
            let mut new_base = 0;
            let mut new_end = 0;
            let mut next_step = BATCH;
            let from_base = self.base;
            // 在存储事务中原子地读取并递增计数器。
            store.run_in_transaction(&mut |transaction| {
                new_base = transaction.get(key)?;
                // 缓存与存储脱节时以存储值为准重置缓存并重算所需数量。
                if self.base == 0 || new_base != self.end {
                    self.base = new_base;
                    self.end = new_base;
                    needed = calc_needed_batch_size(new_base, n as i64, increment, offset, false);
                }
                next_step = next_step.max(needed);
                // 步长受 i64::MAX 上限约束，防止溢出。
                let step = i64::MAX.wrapping_sub(new_base).min(next_step);
                if step < needed {
                    return Err(read_failed());
                }
                new_end = transaction.inc(key, step)?;
                Ok(())
            })?;
            if new_base == i64::MAX {
                return Err(read_failed());
            }
            log::info!(
                target: "autoid_service",
                "alloc signed db={database_id} table={table_id} from_base={from_base} from_end={} to_base={new_base} to_end={new_end}",
                self.end,
            );
            self.end = new_end;
        }
        // 在内存中推进 base，返回本次分配的区间 (min, base]。
        let min = self.base;
        self.base = self.base.wrapping_add(needed);
        Ok((min, self.base))
    }

    /// 将无符号自增计数器的基准值抬高到不小于 `required_base`。
    ///
    /// rebase 用于用户显式写入较大 ID 之后，保证后续分配的 ID 不与
    /// 已有数据冲突。抬高只会向前，不会回退。
    fn rebase_unsigned(
        &mut self,
        store: &dyn AutoIdStorage,
        database_id: i64,
        table_id: i64,
        required_base: u64,
    ) -> Result<()> {
        // 目标基准已在当前缓存区间内，只需更新内存状态，无需访问存储。
        if required_base <= self.base as u64 {
            return Ok(());
        }
        if required_base <= self.end as u64 {
            self.base = required_base as i64;
            return Ok(());
        }

        let started = Instant::now();
        let key = Self::key(database_id, table_id);
        let mut old_value = 0;
        let mut new_base = 0_u64;
        let mut new_end = 0_u64;
        // 在存储事务中把计数器抬高到 max(当前值, required_base) 再多预留一批。
        let result = store.run_in_transaction(&mut |transaction| {
            let current_end = transaction.get(key)?;
            old_value = current_end;
            let unsigned_end = current_end as u64;
            new_base = unsigned_end.max(required_base);
            new_end = new_base.min(u64::MAX - BATCH as u64) + BATCH as u64;
            transaction.inc(key, new_end.wrapping_sub(unsigned_end) as i64)?;
            Ok(())
        });
        record_rebase(started, &result);
        result?;
        log::info!(
            target: "autoid_service",
            "rebase unsigned db={database_id} table={table_id} from={old_value} to={new_end}",
        );
        self.base = new_base as i64;
        self.end = new_end as i64;
        Ok(())
    }

    /// 将有符号自增计数器的基准值抬高到不小于 `required_base`。
    ///
    /// 逻辑与 `rebase_unsigned` 相同，但按 i64 语义进行比较与运算。
    fn rebase_signed(
        &mut self,
        store: &dyn AutoIdStorage,
        database_id: i64,
        table_id: i64,
        required_base: i64,
    ) -> Result<()> {
        // 目标基准已在当前缓存区间内，只需更新内存状态。
        if required_base <= self.base {
            return Ok(());
        }
        if required_base <= self.end {
            self.base = required_base;
            return Ok(());
        }

        let started = Instant::now();
        let key = Self::key(database_id, table_id);
        let mut old_value = 0;
        let mut new_base = 0;
        let mut new_end = 0;
        // 在存储事务中把计数器抬高到 max(当前值, required_base) 再多预留一批。
        let result = store.run_in_transaction(&mut |transaction| {
            let current_end = transaction.get(key)?;
            old_value = current_end;
            new_base = current_end.max(required_base);
            new_end = new_base.min(i64::MAX - BATCH) + BATCH;
            transaction.inc(key, new_end.wrapping_sub(current_end))?;
            Ok(())
        });
        record_rebase(started, &result);
        result?;
        log::info!(
            target: "autoid_service",
            "rebase signed db={database_id} table={table_id} from={old_value} to={new_end}",
        );
        self.base = new_base;
        self.end = new_end;
        Ok(())
    }

    /// 强制把计数器设为 `required_base`，允许向后回退（普通 rebase 只能向前）。
    ///
    /// 常用于 ALTER TABLE ... AUTO_INCREMENT = N 之类的强制重置场景。
    fn force_rebase(
        &mut self,
        store: &dyn AutoIdStorage,
        database_id: i64,
        table_id: i64,
        required_base: i64,
        is_unsigned: bool,
    ) -> Result<()> {
        let key = Self::key(database_id, table_id);
        let mut old_value = 0;
        // 计算差值并通过事务把存储中的计数器直接调整到目标值。
        store.run_in_transaction(&mut |transaction| {
            let current_end = transaction.get(key)?;
            old_value = current_end;
            let step = if is_unsigned {
                (required_base as u64).wrapping_sub(current_end as u64) as i64
            } else {
                required_base.wrapping_sub(current_end)
            };
            transaction.inc(key, step)?;
            Ok(())
        })?;
        log::info!(
            target: "autoid_service",
            "force rebase db={database_id} table={table_id} from={old_value} to={required_base} unsigned={is_unsigned}",
        );
        self.base = required_base;
        self.end = required_base;
        Ok(())
    }
}

/// 服务的共享内部状态，通过 `Arc` 在多个克隆间共享。
struct ServiceInner {
    /// 按（库 ID，表 ID）缓存的每表分配器；外层锁保护映射结构，
    /// 内层锁保护单个表的分配状态，两级锁降低不同表之间的争用。
    auto_id_map: Mutex<HashMap<AutoIdCacheKey, Arc<Mutex<AutoIdValue>>>>,
    /// owner 选举管理器；mock 模式下为 `None`（视自身始终为 owner）。
    manager: Option<Arc<dyn Manager>>,
    /// 底层持久化存储。
    store: Arc<dyn AutoIdStorage>,
}

/// 自增 ID 分配服务的对外句柄，可低成本克隆并跨线程共享。
#[derive(Clone)]
pub struct Service {
    inner: Arc<ServiceInner>,
}

impl Service {
    /// 连接给定的 etcd endpoints 并创建服务（含 owner 选举）。
    pub async fn new(
        self_address: impl Into<String>,
        endpoints: &[String],
        store: Arc<dyn AutoIdStorage>,
        options: Option<ConnectOptions>,
    ) -> std::result::Result<Self, InitError> {
        let client = EtcdClient::connect(endpoints, options).await?;
        Self::new_with_client(self_address, client, store).await
    }

    /// 使用已建立的 etcd 客户端创建服务：注册 leader 切换监听器
    /// 并发起 owner 竞选（campaign）。
    pub async fn new_with_client(
        self_address: impl Into<String>,
        client: EtcdClient,
        store: Arc<dyn AutoIdStorage>,
    ) -> std::result::Result<Self, InitError> {
        let self_address = self_address.into();
        let election_path = format!("{}{}", store.etcd_namespace(), AUTO_ID_LEADER_PATH);
        let manager = NewOwnerManager(
            CancellationToken::new(),
            client,
            "autoid",
            self_address.clone(),
            election_path,
        );
        let service = Self::with_manager(store, manager.clone());
        manager
            .SetListener(Arc::new(OwnerListener {
                service: Arc::downgrade(&service.inner),
                self_address,
            }))
            .await;
        manager.CampaignOwner(&[10]).await?;
        Ok(service)
    }

    /// 使用外部提供的 owner 管理器构造服务（不发起竞选）。
    pub fn with_manager(store: Arc<dyn AutoIdStorage>, manager: Arc<dyn Manager>) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                auto_id_map: Mutex::new(HashMap::new()),
                manager: Some(manager),
                store,
            }),
        }
    }

    /// 创建无选举的 mock 服务（测试用），自身始终视为 owner。
    pub fn new_mock(store: Arc<dyn AutoIdStorage>) -> Self {
        Self {
            inner: Arc::new(ServiceInner {
                auto_id_map: Mutex::new(HashMap::new()),
                manager: None,
                store,
            }),
        }
    }

    /// 关闭服务，退出 owner 选举。
    pub async fn close(&self) {
        if let Some(manager) = &self.inner.manager {
            manager.Close().await;
        }
    }

    /// 判断当前实例是否为 autoid 服务的 owner（leader）。
    pub fn is_owner(&self) -> bool {
        self.inner
            .manager
            .as_ref()
            .is_some_and(|manager| manager.IsOwner())
    }

    /// 成为 owner 时清空内存缓存，强制之后的分配从存储重新加载，
    /// 避免使用旧 owner 遗留的过期区间造成 ID 重复。
    pub fn on_become_owner(&self) {
        self.inner.auto_id_map.lock().unwrap().clear();
    }

    /// 获取（或惰性创建）指定表的分配器。
    fn get_allocator(
        &self,
        database_id: i64,
        table_id: i64,
        is_unsigned: bool,
    ) -> Arc<Mutex<AutoIdValue>> {
        let key = AutoIdCacheKey {
            database_id,
            table_id,
        };
        let mut allocators = self.inner.auto_id_map.lock().unwrap();
        allocators
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(AutoIdValue::new(is_unsigned))))
            .clone()
    }

    /// 校验当前实例是 owner，否则返回 "not leader" 错误让客户端重试其他节点。
    fn ensure_owner(&self) -> Result<()> {
        if self
            .inner
            .manager
            .as_ref()
            .is_some_and(|manager| !manager.IsOwner())
        {
            Err(AutoIdError::Rpc("not leader".to_owned()))
        } else {
            Ok(())
        }
    }

    /// 处理一次自增 ID 分配请求。
    ///
    /// 特殊约定：`n == 0` 表示只查询当前值而不分配；此时若缓存为空
    /// 则从存储读取一次。正常分配返回区间 `(min, max]`。
    pub fn allocate(&self, request: AutoIdRequest) -> Result<AutoIdResponse> {
        // keyspace 不匹配说明请求发错了服务实例，按非 leader 处理。
        if request.keyspace_id != self.inner.store.keyspace_id() {
            log::info!(
                target: "autoid_service",
                "request keyspace {} does not match service keyspace {}",
                request.keyspace_id,
                self.inner.store.keyspace_id(),
            );
            return Err(AutoIdError::Rpc("not leader".to_owned()));
        }
        self.ensure_owner()?;
        fail::fail_point!("mockErr", |_| {
            Err(AutoIdError::Rpc("mock reload failed".to_owned()))
        });

        let allocator = self.get_allocator(request.db_id, request.tbl_id, request.is_unsigned);
        let mut allocator = allocator.lock().unwrap();
        // n == 0：只读查询当前值，不推进计数器。
        if request.n == 0 {
            if allocator.base != 0 {
                return Ok(AutoIdResponse {
                    min: allocator.base,
                    max: allocator.base,
                    errmsg: Vec::new(),
                    ..Default::default()
                });
            }
            let key = AutoIdValue::key(request.db_id, request.tbl_id);
            let mut current_end = 0;
            // 缓存尚未初始化，从存储读取当前值并同步到缓存。
            let result = self.inner.store.run_in_transaction(&mut |transaction| {
                current_end = transaction.get(key)?;
                allocator.base = current_end;
                allocator.end = current_end;
                Ok(())
            });
            return Ok(match result {
                Ok(()) => AutoIdResponse {
                    min: current_end,
                    max: current_end,
                    errmsg: Vec::new(),
                    ..Default::default()
                },
                Err(error) => response_error(error),
            });
        }

        // 根据列的符号属性走对应的分配路径。
        let result = if request.is_unsigned {
            allocator.allocate_unsigned(
                self.inner.store.as_ref(),
                request.db_id,
                request.tbl_id,
                request.n,
                request.increment,
                request.offset,
            )
        } else {
            allocator.allocate_signed(
                self.inner.store.as_ref(),
                request.db_id,
                request.tbl_id,
                request.n,
                request.increment,
                request.offset,
            )
        };
        Ok(match result {
            Ok((min, max)) => AutoIdResponse {
                min,
                max,
                errmsg: Vec::new(),
                ..Default::default()
            },
            Err(error) => response_error(error),
        })
    }

    /// 处理 rebase 请求：把该表自增计数器的基准值调整到 `request.base`。
    /// `force` 为真时先执行强制重置（允许回退），随后再做常规抬高。
    pub fn rebase_ids(&self, request: RebaseRequest) -> Result<RebaseResponse> {
        self.ensure_owner()?;
        let allocator = self.get_allocator(request.db_id, request.tbl_id, request.is_unsigned);
        let mut allocator = allocator.lock().unwrap();
        if request.force {
            if let Err(error) = allocator.force_rebase(
                self.inner.store.as_ref(),
                request.db_id,
                request.tbl_id,
                request.base,
                request.is_unsigned,
            ) {
                return Ok(rebase_error(error));
            }
        }
        let result = if request.is_unsigned {
            allocator.rebase_unsigned(
                self.inner.store.as_ref(),
                request.db_id,
                request.tbl_id,
                request.base as u64,
            )
        } else {
            allocator.rebase_signed(
                self.inner.store.as_ref(),
                request.db_id,
                request.tbl_id,
                request.base,
            )
        };
        Ok(match result {
            Ok(()) => RebaseResponse {
                errmsg: Vec::new(),
                ..Default::default()
            },
            Err(error) => rebase_error(error),
        })
    }
}

/// 进程内客户端接口实现：请求方与服务在同一进程时直接调用，
/// 免去 gRPC 序列化开销。
impl AutoIdClient for Service {
    fn alloc_auto_id(
        &self,
        context: &Context,
        request: ClientAutoIdRequest,
    ) -> Result<ClientAutoIdResponse> {
        context.check()?;
        let response = self.allocate(AutoIdRequest {
            db_id: request.database_id,
            tbl_id: request.table_id,
            is_unsigned: request.is_unsigned,
            n: request.n,
            increment: request.increment,
            offset: request.offset,
            keyspace_id: request.keyspace_id,
            ..Default::default()
        })?;
        Ok(ClientAutoIdResponse {
            min: response.min,
            max: response.max,
            errmsg: String::from_utf8_lossy(&response.errmsg).into_owned(),
        })
    }

    fn rebase(
        &self,
        context: &Context,
        request: ClientRebaseRequest,
    ) -> Result<ClientRebaseResponse> {
        context.check()?;
        let response = self.rebase_ids(RebaseRequest {
            db_id: request.database_id,
            tbl_id: request.table_id,
            is_unsigned: request.is_unsigned,
            base: request.base,
            force: request.force,
            ..Default::default()
        })?;
        Ok(ClientRebaseResponse {
            errmsg: String::from_utf8_lossy(&response.errmsg).into_owned(),
        })
    }
}

/// gRPC 服务端接口实现：把远程请求转发到本地的分配 / rebase 逻辑，
/// 并异步写回响应。
impl AutoIdAlloc for Service {
    fn alloc_auto_id(
        &mut self,
        context: RpcContext,
        request: AutoIdRequest,
        sink: UnarySink<AutoIdResponse>,
    ) {
        spawn_grpc(context, sink, self.allocate(request));
    }

    fn rebase(
        &mut self,
        context: RpcContext,
        request: RebaseRequest,
        sink: UnarySink<RebaseResponse>,
    ) {
        spawn_grpc(context, sink, self.rebase_ids(request));
    }
}

/// 把 `Service` 包装成可注册到 gRPC 服务器的服务定义。
pub fn create_grpc_service(service: Service) -> grpcio::Service {
    kvproto::autoid::create_auto_id_alloc(service)
}

/// 测试用的全局 mock 服务注册表，按存储 UUID 复用同一实例。
static MOCK_SERVICES: OnceLock<Mutex<HashMap<String, Service>>> = OnceLock::new();

/// 按存储 UUID 获取（或创建）测试用 mock 服务，同一存储共享同一服务实例。
pub fn mock_for_test(store: Arc<dyn AutoIdStorage>) -> Service {
    let mut services = MOCK_SERVICES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap();
    services
        .entry(store.uuid().to_owned())
        .or_insert_with(|| Service::new_mock(store))
        .clone()
}

/// owner 选举事件监听器：持有服务内部状态的弱引用（Weak），
/// 避免与 `Service` 形成引用循环导致内存无法释放。
struct OwnerListener {
    service: Weak<ServiceInner>,
    self_address: String,
}

impl Listener for OwnerListener {
    /// 当选 owner 回调：清空分配器缓存，保证从存储重新取号。
    fn OnBecomeOwner(&self) {
        if let Some(service) = self.service.upgrade() {
            service.auto_id_map.lock().unwrap().clear();
        }
        log::info!(
            target: "autoid_service",
            "leader change: {} became autoid owner",
            self.self_address,
        );
    }

    fn OnRetireOwner(&self) {}
}

/// 计算大于 `base` 的第一个满足 `id ≡ offset (mod increment)` 的有符号 ID。
pub fn seek_to_first_auto_id_signed(base: i64, increment: i64, offset: i64) -> i64 {
    let quotient = base.wrapping_add(increment).wrapping_sub(offset) / increment;
    quotient.wrapping_mul(increment).wrapping_add(offset)
}

/// 计算大于 `base` 的第一个满足 `id ≡ offset (mod increment)` 的无符号 ID。
pub fn seek_to_first_auto_id_unsigned(base: u64, increment: u64, offset: u64) -> u64 {
    let quotient = base.wrapping_add(increment).wrapping_sub(offset) / increment;
    quotient.wrapping_mul(increment).wrapping_add(offset)
}

/// 计算从 `base` 开始分配 `n` 个符合步长（increment）与偏移（offset）
/// 规则的 ID 需要占用的计数器区间大小。
pub fn calc_needed_batch_size(
    base: i64,
    n: i64,
    increment: i64,
    offset: i64,
    is_unsigned: bool,
) -> i64 {
    // 步长为 1 时无空洞，恰好需要 n 个。
    if increment == 1 {
        return n;
    }
    // 步长大于 1 时需对齐到第一个合法 ID，再按步长跳跃到最后一个 ID。
    if is_unsigned {
        let first = seek_to_first_auto_id_unsigned(base as u64, increment as u64, offset as u64);
        let last = first.wrapping_add((n as u64).wrapping_sub(1).wrapping_mul(increment as u64));
        return last.wrapping_sub(base as u64) as i64;
    }
    let first = seek_to_first_auto_id_signed(base, increment, offset);
    let last = first.wrapping_add(n.wrapping_sub(1).wrapping_mul(increment));
    last.wrapping_sub(base)
}

/// 构造统一的"自增读取失败"错误（如计数器溢出等场景）。
fn read_failed() -> AutoIdError {
    AutoIdError::AutoIncrementReadFailed(AUTO_INCREMENT_ACTION_FAILED.to_owned())
}

/// 构造 Go `autoid.ErrAutoincReadFailed` 对应的 MySQL 标准错误文案。
fn standard_read_failed() -> AutoIdError {
    AutoIdError::AutoIncrementReadFailed(AUTO_INCREMENT_READ_FAILED.to_owned())
}

/// 把错误编码进分配响应的 errmsg 字段（业务错误通过响应体返回而非 RPC 失败）。
fn response_error(error: AutoIdError) -> AutoIdResponse {
    let errmsg = match error {
        // Go 的 errAutoincReadFailed 是 errors.New 创建的普通错误，响应只携带原始文本。
        AutoIdError::AutoIncrementReadFailed(message) => message.into_bytes(),
        error => error.to_string().into_bytes(),
    };
    AutoIdResponse {
        min: 0,
        max: 0,
        errmsg,
        ..Default::default()
    }
}

/// 把错误编码进 rebase 响应的 errmsg 字段。
fn rebase_error(error: AutoIdError) -> RebaseResponse {
    RebaseResponse {
        errmsg: error.to_string().into_bytes(),
        ..Default::default()
    }
}

/// 上报 rebase 耗时直方图指标（按成功 / 失败区分标签）。
fn record_rebase(started: Instant, result: &Result<()>) {
    metrics::histogram!(
        "tidb_autoid_rebase_seconds",
        "result" => if result.is_ok() { "ok" } else { "error" }
    )
    .record(started.elapsed().as_secs_f64());
}

/// 在 gRPC 上下文中异步发送响应：成功走 success，失败转换为 UNKNOWN 状态码。
fn spawn_grpc<T>(context: RpcContext, sink: UnarySink<T>, result: Result<T>)
where
    T: Send + 'static,
{
    let future = async move {
        let sent = match result {
            Ok(response) => sink.success(response).await,
            Err(error) => {
                sink.fail(RpcStatus::with_message(
                    RpcStatusCode::UNKNOWN,
                    error.to_string(),
                ))
                .await
            }
        };
        if let Err(error) = sent {
            log::error!(target: "autoid_service", "failed to send gRPC response: {error}");
        }
    };
    context.spawn(future);
}

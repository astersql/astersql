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

// unistore 内存 TiKV Server：对外暴露 KV/事务/Raw/MPP/死锁检测 RPC。
//
// 写路径通过 Region Latch 串行化同一键上的冲突；读路径校验 Region 后直访 MVCC。
// 两阶段提交、悲观锁、GC、Coprocessor 与 MPP 任务均在此聚合调度。

use crate::deadlock::{DeadlockRequest, DeadlockResponse, DetectorServer};
use crate::inner_server::InnerServer;
use crate::mock_region::{Region, RegionError};
use crate::mvcc::{
    KvPair, Lock, MvccError, MvccInfo, MvccStore, PessimisticLockRequest, PessimisticLockResult,
    PrewriteRequest, PrewriteResult, SecondaryLocksStatus, TxnStatus,
};
use crate::region::{RegionContext, RegionManager, RequestContext};
use crate::util::keys_to_hash_values;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 事务隔离级别：快照隔离、读已提交、以及带 TS 检查的 RC。
pub enum IsolationLevel {
    #[default]
    SnapshotIsolation,
    ReadCommitted,
    RcCheckTs,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// RPC 请求上下文：Region 定位、隔离级别与已解析/已提交锁列表。
pub struct RpcContext {
    /// Request policy delivered by the transaction client.
    pub priority: i32,
    /// Request-local marker; unrelated background requests remain unmarked.
    pub request_marker: Option<u64>,
    pub region: RequestContext,
    pub isolation: IsolationLevel,
    pub resolved_locks: Vec<u64>,
    pub committed_locks: Vec<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 键级错误：锁冲突、写冲突等，可标记可重试或中止。
pub struct KeyError {
    pub message: String,
    pub deadlock: Option<MvccError>,
    pub locked: Option<Lock>,
    pub conflict: Option<MvccError>,
    pub retryable: bool,
    pub abort: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 统一 RPC 响应：成功值、键错误或 Region 错误三选一。
pub struct RpcResponse<T> {
    pub value: Option<T>,
    pub key_error: Option<KeyError>,
    pub region_error: Option<RegionError>,
}

impl<T> RpcResponse<T> {
    /// 构造成功响应。
    fn ok(value: T) -> Self {
        Self {
            value: Some(value),
            key_error: None,
            region_error: None,
        }
    }
    /// 将 MVCC 错误转为键错误响应。
    fn from_mvcc(error: MvccError) -> Self {
        Self {
            value: None,
            key_error: Some(convert_to_key_error(error)),
            region_error: None,
        }
    }
    /// 将 Region 错误转为 Region 错误响应。
    fn from_region(error: RegionError) -> Self {
        Self {
            value: None,
            key_error: None,
            region_error: Some(error),
        }
    }
}

/// Coprocessor（下推计算）处理接口。
pub trait CoprocessorHandler: Send + Sync {
    fn handle(
        &self,
        request: &[u8],
        start_ts: u64,
        region: &RegionContext,
    ) -> Result<Vec<u8>, String>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// MPP（大规模并行处理）任务的内存态：载荷、取消标记与数据包。
pub struct MppTask {
    pub task_id: i64,
    pub store_id: u64,
    pub payload: Vec<u8>,
    pub cancelled: bool,
    pub packets: Vec<Vec<u8>>,
}

/// 内存 mock TiKV Server：聚合 Region、MVCC、Raw、MPP 与死锁检测。
pub struct Server {
    region_manager: Arc<dyn RegionManager>,
    store: Arc<MvccStore>,
    inner_server: Arc<dyn InnerServer>,
    coprocessor: Option<Arc<dyn CoprocessorHandler>>,
    raw: RwLock<BTreeMap<Vec<u8>, (Vec<u8>, Option<u64>)>>,
    mpp_tasks: Mutex<HashMap<(u64, i64), MppTask>>,
    detector: DetectorServer,
    stopped: AtomicBool,
}

impl Server {
    /// 创建 Server（不含 Coprocessor）。
    pub fn new(
        region_manager: Arc<dyn RegionManager>,
        store: Arc<MvccStore>,
        inner_server: Arc<dyn InnerServer>,
    ) -> Self {
        Self {
            region_manager,
            store,
            inner_server,
            coprocessor: None,
            raw: RwLock::new(BTreeMap::new()),
            mpp_tasks: Mutex::new(HashMap::new()),
            detector: DetectorServer::new(),
            stopped: AtomicBool::new(false),
        }
    }
    /// 挂载 Coprocessor 处理器。
    pub fn with_coprocessor(mut self, coprocessor: Arc<dyn CoprocessorHandler>) -> Self {
        self.coprocessor = Some(coprocessor);
        self
    }

    /// 返回底层 MVCC 存储句柄。
    pub fn mvcc_store(&self) -> Arc<MvccStore> {
        Arc::clone(&self.store)
    }

    /// 停止 Server：关闭 MVCC、RegionManager 与 InnerServer。
    pub fn stop(&self) -> Result<(), String> {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.store.close();
        self.region_manager.close();
        self.inner_server.stop()
    }
    /// 按地址解析 Store ID。
    pub fn get_store_id_by_address(&self, address: &str) -> Result<u64, RegionError> {
        self.region_manager.get_store_id_by_address(address)
    }
    /// 按 Store ID 解析地址。
    pub fn get_store_address_by_id(&self, store_id: u64) -> Result<String, RegionError> {
        self.region_manager.get_store_address_by_id(store_id)
    }

    /// 从请求上下文解析并校验 Region。
    fn request_region(&self, context: &RpcContext) -> Result<Arc<RegionContext>, RegionError> {
        self.region_manager.get_region_from_context(&context.region)
    }
    /// 在键对应 Latch 保护下执行写操作，统一转换错误。
    fn with_latches<T>(
        &self,
        context: &RpcContext,
        keys: &[Vec<u8>],
        operation: impl FnOnce() -> Result<T, MvccError>,
    ) -> RpcResponse<T> {
        let region = match self.request_region(context) {
            Ok(region) => region,
            Err(error) => return RpcResponse::from_region(error),
        };
        // 先获取 Latch，再执行业务，最后释放，保证同键写互斥。
        let hashes = keys_to_hash_values(keys);
        region.acquire_latches(&hashes);
        let result = operation();
        region.release_latches(&hashes);
        match result {
            Ok(value) => RpcResponse::ok(value),
            Err(error) => RpcResponse::from_mvcc(error),
        }
    }

    /// 点查：按版本读取键值（尊重已解析锁）。
    pub fn kv_get(
        &self,
        context: &RpcContext,
        key: Vec<u8>,
        version: u64,
    ) -> RpcResponse<Option<Vec<u8>>> {
        if let Err(error) = self.request_region(context) {
            return RpcResponse::from_region(error);
        }
        match self.store.get(&key, version, &context.resolved_locks) {
            Ok(value) => RpcResponse::ok(value),
            Err(error) => RpcResponse::from_mvcc(error),
        }
    }
    /// 范围扫描（可逆序、仅键）。
    pub fn kv_scan(
        &self,
        context: &RpcContext,
        start: &[u8],
        end: &[u8],
        version: u64,
        limit: usize,
        reverse: bool,
        key_only: bool,
    ) -> RpcResponse<Vec<KvPair>> {
        if let Err(error) = self.request_region(context) {
            return RpcResponse::from_region(error);
        }
        RpcResponse::ok(self.store.scan(
            start,
            end,
            version,
            limit,
            reverse,
            key_only,
            &context.resolved_locks,
        ))
    }
    /// 悲观锁加锁。
    pub fn kv_pessimistic_lock(
        &self,
        context: &RpcContext,
        request: &PessimisticLockRequest,
    ) -> RpcResponse<PessimisticLockResult> {
        if let Some(error) = injected_pessimistic_deadlock(
            request
                .mutations
                .first()
                .map(|mutation| mutation.key.as_slice()),
            request.start_ts,
        ) {
            return RpcResponse::from_mvcc(error);
        }
        let keys = request
            .mutations
            .iter()
            .map(|mutation| mutation.key.clone())
            .collect::<Vec<_>>();
        self.with_latches(context, &keys, || self.store.pessimistic_lock(request))
    }
    /// 悲观锁回滚。
    pub fn kv_pessimistic_rollback(
        &self,
        context: &RpcContext,
        keys: &[Vec<u8>],
        start_ts: u64,
        for_update_ts: u64,
    ) -> RpcResponse<()> {
        self.with_latches(context, keys, || {
            self.store
                .pessimistic_rollback(keys, start_ts, for_update_ts);
            Ok(())
        })
    }
    /// 事务心跳：延长主键锁 TTL。
    pub fn kv_txn_heartbeat(
        &self,
        context: &RpcContext,
        primary: &[u8],
        start_ts: u64,
        ttl: u64,
    ) -> RpcResponse<u64> {
        self.with_latches(context, &[primary.to_vec()], || {
            self.store.txn_heartbeat(primary, start_ts, ttl)
        })
    }
    /// 检查事务状态（可能推高 min_commit_ts 或回滚过期锁）。
    pub fn kv_check_txn_status(
        &self,
        context: &RpcContext,
        primary: &[u8],
        start_ts: u64,
        caller_start_ts: u64,
        current_ts: u64,
        rollback_if_not_exist: bool,
    ) -> RpcResponse<TxnStatus> {
        self.with_latches(context, &[primary.to_vec()], || {
            self.store.check_txn_status(
                primary,
                start_ts,
                caller_start_ts,
                current_ts,
                rollback_if_not_exist,
            )
        })
    }
    /// 检查二级锁状态（异步提交相关）。
    pub fn kv_check_secondary_locks(
        &self,
        context: &RpcContext,
        keys: &[Vec<u8>],
        start_ts: u64,
    ) -> RpcResponse<SecondaryLocksStatus> {
        if let Err(error) = self.request_region(context) {
            return RpcResponse::from_region(error);
        }
        RpcResponse::ok(self.store.check_secondary_locks(keys, start_ts))
    }
    /// 两阶段提交 Prewrite。
    pub fn kv_prewrite(
        &self,
        context: &RpcContext,
        request: &PrewriteRequest,
    ) -> RpcResponse<PrewriteResult> {
        let keys = request
            .mutations
            .iter()
            .map(|mutation| mutation.key.clone())
            .collect::<Vec<_>>();
        self.with_latches(context, &keys, || self.store.prewrite(request))
    }
    /// Flush 语义的预写（pipelined DML 路径）。
    pub fn kv_flush(
        &self,
        context: &RpcContext,
        request: &PrewriteRequest,
    ) -> RpcResponse<PrewriteResult> {
        let keys = request
            .mutations
            .iter()
            .map(|mutation| mutation.key.clone())
            .collect::<Vec<_>>();
        self.with_latches(context, &keys, || self.store.flush(request))
    }
    /// 两阶段提交 Commit。
    pub fn kv_commit(
        &self,
        context: &RpcContext,
        keys: &[Vec<u8>],
        start_ts: u64,
        commit_ts: u64,
    ) -> RpcResponse<()> {
        self.with_latches(context, keys, || {
            self.store.commit(keys, start_ts, commit_ts)
        })
    }
    /// 清理残留锁（过期或已回滚场景）。
    pub fn kv_cleanup(
        &self,
        context: &RpcContext,
        key: &[u8],
        start_ts: u64,
        current_ts: u64,
    ) -> RpcResponse<()> {
        self.with_latches(context, &[key.to_vec()], || {
            self.store.cleanup(key, start_ts, current_ts)
        })
    }
    /// 批量点查。
    pub fn kv_batch_get(
        &self,
        context: &RpcContext,
        keys: &[Vec<u8>],
        version: u64,
    ) -> RpcResponse<Vec<KvPair>> {
        if let Err(error) = self.request_region(context) {
            return RpcResponse::from_region(error);
        }
        RpcResponse::ok(self.store.batch_get(keys, version, &context.resolved_locks))
    }
    /// 批量回滚。
    pub fn kv_batch_rollback(
        &self,
        context: &RpcContext,
        keys: &[Vec<u8>],
        start_ts: u64,
    ) -> RpcResponse<()> {
        self.with_latches(context, keys, || self.store.rollback(keys, start_ts))
    }
    /// 扫描锁表。
    pub fn kv_scan_lock(
        &self,
        context: &RpcContext,
        start: &[u8],
        end: &[u8],
        max_ts: u64,
        limit: usize,
    ) -> RpcResponse<Vec<(Vec<u8>, Lock)>> {
        if let Err(error) = self.request_region(context) {
            return RpcResponse::from_region(error);
        }
        RpcResponse::ok(self.store.scan_lock(start, end, max_ts, limit))
    }
    /// 按 start_ts 解析锁（提交或回滚）。
    pub fn kv_resolve_lock(
        &self,
        context: &RpcContext,
        start_ts: u64,
        commit_ts: u64,
    ) -> RpcResponse<()> {
        if let Err(error) = self.request_region(context) {
            return RpcResponse::from_region(error);
        }
        match self.store.resolve_lock(start_ts, commit_ts) {
            Ok(()) => RpcResponse::ok(()),
            Err(error) => RpcResponse::from_mvcc(error),
        }
    }
    /// 更新 GC safe point 并触发垃圾回收。
    pub fn kv_gc(&self, context: &RpcContext, safe_point: u64) -> RpcResponse<()> {
        if let Err(error) = self.request_region(context) {
            return RpcResponse::from_region(error);
        }
        self.store.update_safe_point(safe_point);
        self.store.gc();
        RpcResponse::ok(())
    }
    /// 删除范围内的数据文件/版本。
    pub fn kv_delete_range(
        &self,
        context: &RpcContext,
        start: &[u8],
        end: &[u8],
    ) -> RpcResponse<()> {
        if let Err(error) = self.request_region(context) {
            return RpcResponse::from_region(error);
        }
        self.store.delete_file_in_range(start, end);
        RpcResponse::ok(())
    }
    /// 按键导出 MVCC 调试信息。
    pub fn mvcc_get_by_key(&self, context: &RpcContext, key: &[u8]) -> RpcResponse<MvccInfo> {
        if let Err(error) = self.request_region(context) {
            return RpcResponse::from_region(error);
        }
        RpcResponse::ok(self.store.mvcc_get_by_key(key))
    }
    /// 按 start_ts 查找对应键与 MVCC 信息。
    pub fn mvcc_get_by_start_ts(
        &self,
        context: &RpcContext,
        start_ts: u64,
    ) -> RpcResponse<Option<(Vec<u8>, MvccInfo)>> {
        if let Err(error) = self.request_region(context) {
            return RpcResponse::from_region(error);
        }
        RpcResponse::ok(self.store.mvcc_get_by_start_ts(start_ts))
    }

    /// Raw KV 点查（带 TTL 过期过滤）。
    pub fn raw_get(&self, key: &[u8], now_ts: u64) -> Option<Vec<u8>> {
        self.raw
            .read()
            .expect("raw store poisoned")
            .get(key)
            // 过滤已过期的 Raw 条目。
            .filter(|(_, expires)| expires.is_none_or(|expires| expires > now_ts))
            .map(|(value, _)| value.clone())
    }
    /// 查询 Raw 键剩余 TTL。
    pub fn raw_get_key_ttl(&self, key: &[u8], now_ts: u64) -> Option<u64> {
        self.raw
            .read()
            .expect("raw store poisoned")
            .get(key)
            .and_then(|(_, expires)| {
                expires.and_then(|expires| (expires > now_ts).then_some(expires - now_ts))
            })
    }
    /// Raw KV 写入（可选 TTL）。
    pub fn raw_put(&self, key: Vec<u8>, value: Vec<u8>, ttl: Option<u64>, now_ts: u64) {
        self.raw
            .write()
            .expect("raw store poisoned")
            .insert(key, (value, ttl.map(|ttl| now_ts.saturating_add(ttl))));
    }
    /// Raw KV 删除。
    pub fn raw_delete(&self, key: &[u8]) {
        self.raw.write().expect("raw store poisoned").remove(key);
    }
    /// 比较并交换：仅当旧值匹配 `previous` 时写入。
    pub fn raw_compare_and_swap(
        &self,
        key: Vec<u8>,
        previous: Option<&[u8]>,
        value: Vec<u8>,
    ) -> (bool, Option<Vec<u8>>) {
        let mut raw = self.raw.write().expect("raw store poisoned");
        let old = raw.get(&key).map(|(value, _)| value.clone());
        let matches = old.as_deref() == previous;
        if matches {
            raw.insert(key, (value, None));
        }
        (matches, old)
    }
    /// Raw 范围扫描（过滤过期、可逆序）。
    pub fn raw_scan(
        &self,
        start: &[u8],
        end: &[u8],
        limit: usize,
        reverse: bool,
        key_only: bool,
        now_ts: u64,
    ) -> Vec<(Vec<u8>, Vec<u8>)> {
        let raw = self.raw.read().expect("raw store poisoned");
        let keys = if end.is_empty() {
            raw.range(start.to_vec()..)
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>()
        } else {
            raw.range(start.to_vec()..end.to_vec())
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>()
        };
        let mut values = keys
            .into_iter()
            .filter_map(|key| {
                let (value, expires) = raw.get(&key)?;
                if expires.is_some_and(|expires| expires <= now_ts) {
                    return None;
                }
                Some((key, value.clone()))
            })
            .map(|(key, value)| (key.clone(), if key_only { Vec::new() } else { value }))
            .collect::<Vec<_>>();
        if reverse {
            values.reverse();
        }
        values.truncate(limit);
        values
    }
    /// Raw 范围删除。
    pub fn raw_delete_range(&self, start: &[u8], end: &[u8]) {
        let mut raw = self.raw.write().expect("raw store poisoned");
        let keys = raw
            .range(start.to_vec()..end.to_vec())
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        for key in keys {
            raw.remove(&key);
        }
    }
    /// 计算范围内校验和、行数与字节数。
    pub fn raw_checksum(&self, start: &[u8], end: &[u8], now_ts: u64) -> (u64, u64, u64) {
        let rows = self.raw_scan(start, end, usize::MAX, false, false, now_ts);
        let mut checksum = 0;
        let mut bytes = 0;
        for (key, value) in &rows {
            checksum ^= hash(key) ^ hash(value);
            bytes += (key.len() + value.len()) as u64;
        }
        (checksum, rows.len() as u64, bytes)
    }

    /// 执行 Coprocessor 请求；未挂载处理器时返回中止错误。
    pub fn coprocessor(
        &self,
        context: &RpcContext,
        request: &[u8],
        start_ts: u64,
    ) -> RpcResponse<Vec<u8>> {
        let region = match self.request_region(context) {
            Ok(region) => region,
            Err(error) => return RpcResponse::from_region(error),
        };
        let Some(handler) = &self.coprocessor else {
            return RpcResponse {
                value: None,
                key_error: Some(KeyError {
                    message: "coprocessor unavailable".into(),
                    abort: true,
                    ..KeyError::default()
                }),
                region_error: None,
            };
        };
        match handler.handle(request, start_ts, &region) {
            Ok(value) => RpcResponse::ok(value),
            Err(message) => RpcResponse {
                value: None,
                key_error: Some(KeyError {
                    message,
                    abort: true,
                    ..KeyError::default()
                }),
                region_error: None,
            },
        }
    }

    /// 按切分键分裂 Region。
    pub fn split_region(
        &self,
        context: &RpcContext,
        keys: Vec<Vec<u8>>,
    ) -> RpcResponse<Vec<Region>> {
        match self
            .region_manager
            .split_region(context.region.region_id, keys)
        {
            Ok(regions) => RpcResponse::ok(regions),
            Err(error) => RpcResponse::from_region(error),
        }
    }
    /// 创建 MPP 任务。
    pub fn create_mpp_task(
        &self,
        store_id: u64,
        task_id: i64,
        payload: Vec<u8>,
    ) -> Result<(), String> {
        let mut tasks = self.mpp_tasks.lock().expect("MPP tasks poisoned");
        if tasks.contains_key(&(store_id, task_id)) {
            return Err("MPP task already exists".into());
        }
        tasks.insert(
            (store_id, task_id),
            MppTask {
                task_id,
                store_id,
                payload,
                cancelled: false,
                packets: Vec::new(),
            },
        );
        Ok(())
    }
    /// 向 MPP 任务投递数据包。
    pub fn dispatch_mpp_packet(
        &self,
        store_id: u64,
        task_id: i64,
        packet: Vec<u8>,
    ) -> Result<(), String> {
        let mut tasks = self.mpp_tasks.lock().expect("MPP tasks poisoned");
        let task = tasks
            .get_mut(&(store_id, task_id))
            .ok_or("MPP task not found")?;
        if task.cancelled {
            return Err("MPP task cancelled".into());
        }
        task.packets.push(packet);
        Ok(())
    }
    /// 取消 MPP 任务。
    pub fn cancel_mpp_task(&self, store_id: u64, task_id: i64) -> Result<(), String> {
        self.mpp_tasks
            .lock()
            .expect("MPP tasks poisoned")
            .get_mut(&(store_id, task_id))
            .ok_or("MPP task not found".into())
            .map(|task| task.cancelled = true)
    }
    /// 移除并返回 MPP 任务。
    pub fn remove_mpp_task(&self, store_id: u64, task_id: i64) -> Option<MppTask> {
        self.mpp_tasks
            .lock()
            .expect("MPP tasks poisoned")
            .remove(&(store_id, task_id))
    }
    /// 建立 MPP 连接并返回已缓冲的数据包。
    pub fn establish_mpp_connection(
        &self,
        store_id: u64,
        task_id: i64,
    ) -> Result<Vec<Vec<u8>>, String> {
        let tasks = self.mpp_tasks.lock().expect("MPP tasks poisoned");
        let task = tasks
            .get(&(store_id, task_id))
            .ok_or("MPP task not found")?;
        if task.cancelled {
            Err("MPP task cancelled".into())
        } else {
            Ok(task.packets.clone())
        }
    }
    /// 死锁检测。
    pub fn detect_deadlock(&self, request: &DeadlockRequest) -> Option<DeadlockResponse> {
        self.detector.detect(request)
    }
    /// 转发 Raft 消息到 InnerServer。
    pub fn raft(&self) -> Result<(), String> {
        self.inner_server.raft()
    }
    /// 批量转发 Raft 消息。
    pub fn batch_raft(&self) -> Result<(), String> {
        self.inner_server.batch_raft()
    }
    /// 处理 Snapshot 请求。
    pub fn snapshot(&self) -> Result<(), String> {
        self.inner_server.snapshot()
    }
}

/// 将 MVCC 错误映射为带重试/中止语义的 KeyError。
pub fn convert_to_key_error(error: MvccError) -> KeyError {
    match error.clone() {
        MvccError::Deadlock { .. } => KeyError {
            message: "deadlock".into(),
            deadlock: Some(error),
            ..KeyError::default()
        },
        // 遇锁：可重试，附带锁信息。
        MvccError::KeyLocked { lock, .. } => KeyError {
            message: error.to_string(),
            deadlock: None,
            locked: Some(lock),
            conflict: None,
            retryable: true,
            abort: false,
        },
        // 写冲突/提交时间戳过期：可重试。
        MvccError::WriteConflict { .. } | MvccError::CommitTsExpired { .. } => KeyError {
            message: error.to_string(),
            conflict: Some(error),
            retryable: true,
            ..KeyError::default()
        },
        // 已存在或已提交：中止事务。
        MvccError::AlreadyExists(_) | MvccError::AlreadyCommitted(_) => KeyError {
            message: error.to_string(),
            conflict: Some(error),
            abort: true,
            ..KeyError::default()
        },
        _ => KeyError {
            message: error.to_string(),
            conflict: Some(error),
            retryable: false,
            abort: true,
            ..KeyError::default()
        },
    }
}

/// FNV-1a 哈希，供 Raw checksum 使用。
fn hash(value: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325;
    for byte in value {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// The same mock RPC fault boundary is used by native in-process sessions.
/// Evaluate before region validation, including requests with no mutations.
pub fn injected_pessimistic_deadlock(first_key: Option<&[u8]>, start_ts: u64) -> Option<MvccError> {
    let enabled = fail::eval("pessimisticLockReturnDeadlock", |value| {
        value.as_deref() == Some("true")
    })
    .unwrap_or(false);
    if !enabled {
        return None;
    }
    let key = first_key?;
    Some(MvccError::Deadlock {
        lock_key: key.to_vec(),
        lock_ts: start_ts.wrapping_add(1),
        deadlock_key_hash: keys_to_hash_values(&[key.to_vec()])[0],
    })
}

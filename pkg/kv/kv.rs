// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// KV 核心抽象：读写接口、事务、存储、请求与资源组标签。
//
// 定义 Getter/Retriever/Mutator、MemBuffer、Transaction、Storage、Client 等 trait，
// 以及 KeyRanges、Request、隔离级别（IsoLevel）与 ResourceGroupTagBuilder。
// 对齐 pkg/kv/kv.go 的核心接口、请求结构、范围处理及资源组标签逻辑。

// 对齐 pkg/kv/kv.go 的核心接口、请求结构、范围处理及资源组标签逻辑。

use std::any::Any;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// 更新未改变索引值时，以字节 '1' 标记该索引键值无需提交。
// 更新未改变索引值时，以字节 '1' 标记该索引键值无需提交。
pub const UnCommitIndexKVFlag: u8 = b'1';

// 单条与整个事务大小限制对应 Go atomic.Uint64；默认值来自全局配置。
pub static TxnEntrySizeLimit: AtomicU64 = AtomicU64::new(config::DefTxnEntrySizeLimit);
pub static TxnTotalSizeLimit: AtomicU64 = AtomicU64::new(config::DefTxnTotalSizeLimit);

// 以下别名直接复用 client-go 的值与 Get/BatchGet 选项语义。
pub type ValueEntry = tikvstore::ValueEntry;
pub type GetOption = tikvstore::GetOption;
pub type BatchGetOption = tikvstore::BatchGetOption;
pub type GetOptions = tikvstore::GetOptions;
pub type BatchGetOptions = tikvstore::BatchGetOptions;

/// BatchGet 的 Rust 接口暂以 `String` 承载 Go `map[string]` 的原始字节键。
/// 十六进制编码保证任意二进制 TiDB key 一一对应，避免 UTF-8 有损转换碰撞。
pub fn KeyMapName(key: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(key.len() * 2);
    for byte in key {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

// NewValueEntry 把值字节和提交时间戳交给底层客户端构造。
pub fn NewValueEntry(value: Vec<u8>, commit_ts: u64) -> ValueEntry {
    tikvstore::NewValueEntry(value, commit_ts)
}

// BatchGetToGetOptions 保持空输入返回 None 的 Go nil 语义。
pub fn BatchGetToGetOptions(options: Vec<BatchGetOption>) -> Option<Vec<GetOption>> {
    if options.is_empty() {
        None
    } else {
        Some(tikvstore::BatchGetToGetOptions(options))
    }
}

// WithReturnCommitTS 要求读取结果附带实际提交时间戳。
pub fn WithReturnCommitTS() -> tikvstore::GetOrBatchGetOption {
    tikvstore::WithReturnCommitTS()
}

// Getter 对应基础 Get 接口；未命中由实现返回 ErrNotExist。
pub trait Getter {
    fn Get(
        &self,
        ctx: &context::Context,
        k: Key,
        options: &[GetOption],
    ) -> Result<ValueEntry, errors::SharedError>;
}

// GetValue 丢弃 ValueEntry 的额外元数据，只返回值字节。
pub fn GetValue(
    ctx: &context::Context,
    getter: &dyn Getter,
    k: Key,
) -> Result<Vec<u8>, errors::SharedError> {
    Ok(getter.Get(ctx, k, &[])?.Value)
}

// Retriever 在 Getter 上增加正向和反向有界迭代；调用方必须关闭迭代器。
pub trait Retriever: Getter {
    fn Iter(
        &self,
        k: Key,
        upper_bound: Option<Key>,
    ) -> Result<Box<dyn Iterator>, errors::SharedError>;
    fn IterReverse(
        &self,
        k: Option<Key>,
        lower_bound: Option<Key>,
    ) -> Result<Box<dyn Iterator>, errors::SharedError>;
}

// EmptyIterator 永远无有效条目，用于空 Retriever 的无分配语义占位。
pub struct EmptyIterator;

impl Iterator for EmptyIterator {
    fn Valid(&self) -> bool {
        false
    }
    fn Key(&self) -> Key {
        Key::default()
    }
    fn Value(&self) -> Vec<u8> {
        Vec::new()
    }
    fn Next(&mut self) -> Result<(), errors::SharedError> {
        Err(errors::New("iterator is invalid"))
    }
    fn Close(&mut self) {}
}

// EmptyRetriever 的所有读取均为空：Get 返回 ErrNotExist，迭代返回 EmptyIterator。
pub struct EmptyRetriever;

impl Getter for EmptyRetriever {
    fn Get(
        &self,
        _: &context::Context,
        _: Key,
        _: &[GetOption],
    ) -> Result<ValueEntry, errors::SharedError> {
        Err(ErrNotExist.FastGenByArgs(&[]))
    }
}

impl Retriever for EmptyRetriever {
    fn Iter(&self, _: Key, _: Option<Key>) -> Result<Box<dyn Iterator>, errors::SharedError> {
        Ok(Box::new(EmptyIterator))
    }
    fn IterReverse(
        &self,
        _: Option<Key>,
        _: Option<Key>,
    ) -> Result<Box<dyn Iterator>, errors::SharedError> {
        Ok(Box::new(EmptyIterator))
    }
}

// Mutator 对应 Set/Delete；Set 的值不得为 nil 或空，否则实现应返回 ErrCannotSetNilValue。
pub trait Mutator {
    fn Set(&mut self, k: Key, v: Vec<u8>) -> Result<(), errors::SharedError>;
    fn Delete(&mut self, k: Key) -> Result<(), errors::SharedError>;
}

// StagingHandle 引用 MemBuffer 内的临时写入层。
pub type StagingHandle = i32;
pub const InvalidStagingHandle: StagingHandle = 0;
pub const LastActiveStagingHandle: StagingHandle = -1;

pub trait RetrieverMutator: Retriever + Mutator {}

// MemBuffer 是事务内存 KV 集合。读锁用于 UnionScan 等多 goroutine 读取场景。
pub trait MemBuffer: RetrieverMutator {
    fn RLock(&self);
    fn RUnlock(&self);
    fn GetFlags(&self, key: &Key) -> Result<KeyFlags, errors::SharedError>;
    fn SetWithFlags(
        &mut self,
        key: Key,
        value: Vec<u8>,
        ops: &[FlagsOp],
    ) -> Result<(), errors::SharedError>;
    fn UpdateFlags(&mut self, key: Key, ops: &[FlagsOp]);
    fn UpdateAssertionFlags(&mut self, key: Key, op: AssertionOp);
    fn DeleteWithFlags(&mut self, key: Key, ops: &[FlagsOp]) -> Result<(), errors::SharedError>;

    // Staging 创建临时层；Release 发布到上层，Cleanup 丢弃未发布资源。
    fn Staging(&mut self) -> StagingHandle;
    fn Release(&mut self, handle: StagingHandle);
    fn Cleanup(&mut self, handle: StagingHandle);
    fn InspectStage(&self, handle: StagingHandle, f: &mut dyn FnMut(Key, KeyFlags, Vec<u8>));

    fn SnapshotGetter(&self) -> Box<dyn Getter>;
    fn SnapshotIter(&self, k: Key, upper_bound: Option<Key>) -> Box<dyn Iterator>;
    fn SnapshotIterReverse(&self, k: Option<Key>, lower_bound: Option<Key>) -> Box<dyn Iterator>;
    fn Len(&self) -> usize;
    fn Size(&self) -> usize;
    fn RemoveFromBuffer(&mut self, key: Key);
    fn GetLocal(&self, ctx: &context::Context, key: &[u8]) -> Result<Vec<u8>, errors::SharedError>;
    fn BatchGet(
        &self,
        ctx: &context::Context,
        keys: &[Vec<u8>],
        options: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, errors::SharedError>;
}

// FindKeysInStage 用 InspectStage 遍历指定临时层，只收集谓词接受的键。
pub fn FindKeysInStage(
    m: &dyn MemBuffer,
    h: StagingHandle,
    mut predicate: impl FnMut(&Key, KeyFlags, &[u8]) -> bool,
) -> Vec<Key> {
    let mut result = Vec::new();
    m.InspectStage(h, &mut |k, flags, value| {
        if predicate(&k, flags, &value) {
            result.push(k);
        }
    });
    result
}

pub type LockCtx = tikvstore::LockCtx;

// Transaction 汇总事务内读写、锁、选项、内存和流水线 DML 能力；与 Go 一样不保证线程安全。
pub trait Transaction: RetrieverMutator + FairLockingController {
    /// A statement mutation checkpoint. The implementation must restore both
    /// the local mem-buffer and any remote staged writes on cleanup.
    fn StageStatement(&mut self) -> Result<StagingHandle, errors::SharedError> {
        Err(ErrNotImplemented.FastGenByArgs(&[]))
    }
    fn ReleaseStatement(&mut self, _handle: StagingHandle) -> Result<(), errors::SharedError> {
        Err(ErrNotImplemented.FastGenByArgs(&[]))
    }
    fn CleanupStatement(&mut self, _handle: StagingHandle) -> Result<(), errors::SharedError> {
        Err(ErrNotImplemented.FastGenByArgs(&[]))
    }
    fn Size(&self) -> usize;
    fn Mem(&self) -> u64;
    fn SetMemoryFootprintChangeHook(&mut self, hook: Box<dyn Fn(u64)>);
    fn MemHookSet(&self) -> bool;
    fn Len(&self) -> usize;
    fn Commit(&mut self, ctx: &context::Context) -> Result<(), errors::SharedError>;
    fn Rollback(&mut self) -> Result<(), errors::SharedError>;
    fn String(&self) -> String;
    fn LockKeys(
        &mut self,
        ctx: &context::Context,
        lock_ctx: &mut LockCtx,
        keys: &[Key],
    ) -> Result<(), errors::SharedError>;
    fn LockKeysFunc(
        &mut self,
        ctx: &context::Context,
        lock_ctx: &mut LockCtx,
        f: &mut dyn FnMut(),
        keys: &[Key],
    ) -> Result<(), errors::SharedError>;
    fn SetOption(&mut self, opt: i32, val: Option<Box<dyn Any>>);
    fn GetOption(&self, opt: i32) -> Option<&dyn Any>;
    fn IsReadOnly(&self) -> bool;
    fn StartTS(&self) -> u64;
    fn CommitTS(&self) -> u64;
    fn Valid(&self) -> bool;
    fn GetMemBuffer(&self) -> &dyn MemBuffer;
    fn GetSnapshot(&self) -> &dyn Snapshot;
    fn SetVars(&mut self, vars: Box<dyn Any>);
    fn GetVars(&self) -> &dyn Any;
    fn BatchGet(
        &self,
        ctx: &context::Context,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, errors::SharedError>;
    fn IsPessimistic(&self) -> bool;
    fn CacheTableInfo(&mut self, id: i64, info: model::TableInfo);
    fn GetTableInfo(&self, id: i64) -> Option<&model::TableInfo>;
    fn SetDiskFullOpt(&mut self, level: kvrpcpb::DiskFullOpt);
    fn ClearDiskFullOpt(&mut self);
    fn GetMemDBCheckpoint(&self) -> &tikv::MemDBCheckpoint;
    fn RollbackMemDBToCheckpoint(&mut self, checkpoint: &tikv::MemDBCheckpoint);
    fn IsPipelined(&self) -> bool;
    fn MayFlush(&mut self) -> Result<(), errors::SharedError>;
}

// FairLockingController 保留公平加锁四阶段及模式查询接口。
pub trait FairLockingController {
    fn StartFairLocking(&mut self) -> Result<(), errors::SharedError>;
    fn RetryFairLocking(&mut self, ctx: &context::Context) -> Result<(), errors::SharedError>;
    fn CancelFairLocking(&mut self, ctx: &context::Context) -> Result<(), errors::SharedError>;
    fn DoneFairLocking(&mut self, ctx: &context::Context) -> Result<(), errors::SharedError>;
    fn IsInFairLockingMode(&self) -> bool;
}

// Client 向 KV 层发送请求，并报告请求类型支持情况。
pub trait Client {
    fn Send(
        &self,
        ctx: &context::Context,
        req: &Request,
        vars: &dyn Any,
        option: &ClientSendOption,
    ) -> Option<Box<dyn Response>>;
    fn IsRequestTypeSupported(&self, req_type: i64, sub_type: i64) -> bool;
}

// ClientSendOption 聚合单次 Send 的内存、事件、限流与 TiFlash 副本选项。
pub struct ClientSendOption {
    pub SessionMemTracker: Option<memory::Tracker>,
    pub EnabledRateLimitAction: bool,
    pub EventCb: Option<trxevents::EventCallback>,
    pub EnableCollectExecutionInfo: bool,
    pub TiFlashReplicaRead: tiflash::ReplicaRead,
    pub AppendWarning: Option<Box<dyn Fn(errors::SharedError)>>,
    pub TryCopLiteWorker: Option<AtomicU32>,
}

// 请求主类型与子类型编号保持 Go 常量值，供存储能力探测使用。
pub const ReqTypeSelect: i64 = 101;
pub const ReqTypeIndex: i64 = 102;
pub const ReqTypeDAG: i64 = 103;
pub const ReqTypeAnalyze: i64 = 104;
pub const ReqTypeChecksum: i64 = 105;
pub const ReqSubTypeBasic: i64 = 0;
pub const ReqSubTypeDesc: i64 = 10000;
pub const ReqSubTypeGroupBy: i64 = 10001;
pub const ReqSubTypeTopN: i64 = 10002;
pub const ReqSubTypeSignature: i64 = 10003;
pub const ReqSubTypeAnalyzeIdx: i64 = 10004;
pub const ReqSubTypeAnalyzeCol: i64 = 10005;

// StoreType 对应请求目标存储引擎；UnSpecified 明确使用 255。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum StoreType {
    TiKV = 0,
    TiFlash = 1,
    TiDB = 2,
    UnSpecified = 255,
}

impl StoreType {
    // Name 返回日志与协议中使用的小写名称。
    pub fn Name(self) -> &'static str {
        match self {
            StoreType::TiFlash => "tiflash",
            StoreType::TiDB => "tidb",
            StoreType::TiKV => "tikv",
            StoreType::UnSpecified => "unspecified",
        }
    }
}

// KeyRanges 按分区保存范围和行数提示，确保请求先按分区再按 region 发送。
pub struct KeyRanges {
    ranges: Vec<Vec<KeyRange>>,
    rowCountHints: Vec<Vec<i32>>,
    isPartitioned: bool,
}

pub fn NewPartitionedKeyRanges(ranges: Vec<Vec<KeyRange>>) -> KeyRanges {
    NewPartitionedKeyRangesWithHints(ranges, Vec::new())
}

pub fn NewNonPartitionedKeyRanges(ranges: Vec<KeyRange>) -> KeyRanges {
    NewNonParitionedKeyRangesWithHint(ranges, None)
}

pub fn NewPartitionedKeyRangesWithHints(
    ranges: Vec<Vec<KeyRange>>,
    hints: Vec<Vec<i32>>,
) -> KeyRanges {
    KeyRanges {
        ranges,
        rowCountHints: hints,
        isPartitioned: true,
    }
}

// NewNonParitionedKeyRangesWithHint 保留 Go 名称中的 Paritioned 拼写。
pub fn NewNonParitionedKeyRangesWithHint(
    ranges: Vec<KeyRange>,
    hints: Option<Vec<i32>>,
) -> KeyRanges {
    KeyRanges {
        ranges: vec![ranges],
        rowCountHints: hints.map(|h| vec![h]).unwrap_or_default(),
        isPartitioned: false,
    }
}

impl KeyRanges {
    // FirstPartitionRange 为空时返回空切片，否则返回第一分区，供非分区路径回退。
    pub fn FirstPartitionRange(&self) -> &[KeyRange] {
        self.ranges.first().map(Vec::as_slice).unwrap_or(&[])
    }

    // SetToNonPartitioned 拒绝把多分区范围伪装成单分区。
    pub fn SetToNonPartitioned(&mut self) -> Result<(), errors::SharedError> {
        if self.ranges.len() > 1 {
            return Err(errors::New(
                "you want to change the partitioned ranges to non-partitioned ranges",
            ));
        }
        self.isPartitioned = false;
        Ok(())
    }

    // AppendSelfTo 依分区顺序展平到调用方已有向量尾部。
    pub fn AppendSelfTo(&self, mut ranges: Vec<KeyRange>) -> Vec<KeyRange> {
        for partition in &self.ranges {
            ranges.extend(partition.iter().cloned());
        }
        ranges
    }

    // SortByFunc 先按每个分区首范围排序分区，再排序分区内部；已排序部分不重复处理。
    pub fn SortByFunc(&mut self, sort_func: impl Fn(&KeyRange, &KeyRange) -> Ordering + Copy) {
        if !self.ranges.is_sorted_by(|a, b| {
            if a.is_empty() || b.is_empty() {
                true
            } else {
                sort_func(&a[0], &b[0]) != Ordering::Greater
            }
        }) {
            self.ranges.sort_by(|a, b| match (a.first(), b.first()) {
                (None, _) => Ordering::Less,
                (_, None) => Ordering::Greater,
                (Some(a), Some(b)) => sort_func(a, b),
            });
        }
        for ranges in &mut self.ranges {
            if !ranges.is_sorted_by(|a, b| sort_func(a, b) != Ordering::Greater) {
                ranges.sort_by(sort_func);
            }
        }
    }

    // ForEachPartitionWithErr 把对应行数提示交给回调，首个错误立即返回。
    pub fn ForEachPartitionWithErr(
        &self,
        mut f: impl FnMut(&[KeyRange], &[i32]) -> Result<(), errors::SharedError>,
    ) -> Result<(), errors::SharedError> {
        for (i, ranges) in self.ranges.iter().enumerate() {
            let hints = self.rowCountHints.get(i).map(Vec::as_slice).unwrap_or(&[]);
            f(ranges, hints)?;
        }
        Ok(())
    }

    pub fn ForEachPartition(&self, mut f: impl FnMut(&[KeyRange])) {
        for ranges in &self.ranges {
            f(ranges);
        }
    }
    pub fn PartitionNum(&self) -> usize {
        self.ranges.len()
    }

    // IsFullySorted 同时检查分区首键顺序与每个分区内部 StartKey 顺序。
    pub fn IsFullySorted(&self) -> bool {
        let partitions_sorted = self.ranges.is_sorted_by(|a, b| {
            if a.is_empty() || b.is_empty() {
                true
            } else {
                a[0].StartKey.0 <= b[0].StartKey.0
            }
        });
        partitions_sorted
            && self
                .ranges
                .iter()
                .all(|ranges| ranges.is_sorted_by(|a, b| a.StartKey.0 <= b.StartKey.0))
    }

    pub fn TotalRangeNum(&self) -> usize {
        self.ranges.iter().map(Vec::len).sum()
    }
}

// Paging 保存分页开关、行数上下界及每页字节预算。
#[derive(Default)]
pub struct Paging {
    pub Enable: bool,
    pub MinPagingSize: u64,
    pub MaxPagingSize: u64,
    pub PagingSizeBytes: u64,
}

// Request 是完整 KV/coprocessor 请求结构，字段顺序与 Go 保持一致。
/// Limits the aggregate number of active coprocessor request attempts.
pub struct CoprRequestLimiter {
    in_flight: Mutex<usize>,
    available: Condvar,
    capacity: usize,
}

/// Create a limiter, or None when the requested capacity is nonpositive.
pub fn NewCoprRequestLimiter(capacity: i32) -> Option<Arc<CoprRequestLimiter>> {
    (capacity > 0).then(|| {
        Arc::new(CoprRequestLimiter {
            in_flight: Mutex::new(0),
            available: Condvar::new(),
            capacity: capacity as usize,
        })
    })
}

impl CoprRequestLimiter {
    /// Return true when cancellation prevents admission; false means Release is required.
    pub fn AcquireWithContext(&self, ctx: &Context, done: &CancellationToken) -> bool {
        let mut in_flight = self.in_flight.lock().unwrap();
        loop {
            if ctx.is_cancelled() || done.is_cancelled() {
                return true;
            }
            if *in_flight < self.capacity {
                *in_flight += 1;
                return false;
            }
            // CancellationToken cannot wake a std::Condvar; check it periodically.
            in_flight = self
                .available
                .wait_timeout(in_flight, Duration::from_millis(10))
                .unwrap()
                .0;
        }
    }

    /// Attempt admission without waiting.
    pub fn TryAcquire(&self) -> bool {
        let mut in_flight = self.in_flight.lock().unwrap();
        if *in_flight >= self.capacity {
            return false;
        }
        *in_flight += 1;
        true
    }

    /// Release one attempt; redundant release is a programming error.
    pub fn Release(&self) {
        let mut in_flight = self.in_flight.lock().unwrap();
        assert!(*in_flight > 0, "release a redundant cop request token");
        *in_flight -= 1;
        self.available.notify_one();
    }

    pub fn Capacity(&self) -> usize {
        self.capacity
    }
}

/// Query-scoped collection of independent per-store limiters.
pub struct QueryCopStoreLimiter {
    limit: i32,
    stores: Mutex<HashMap<u64, Arc<CoprRequestLimiter>>>,
}

pub fn NewQueryCopStoreLimiter(limit: i32) -> Option<Arc<QueryCopStoreLimiter>> {
    (limit > 0).then(|| {
        Arc::new(QueryCopStoreLimiter {
            limit,
            stores: Mutex::new(HashMap::new()),
        })
    })
}

impl QueryCopStoreLimiter {
    pub fn GetStoreLimiter(&self, store_id: u64) -> Option<Arc<CoprRequestLimiter>> {
        if store_id == 0 {
            return None;
        }
        let mut stores = self.stores.lock().unwrap();
        Some(
            stores
                .entry(store_id)
                .or_insert_with(|| NewCoprRequestLimiter(self.limit).unwrap())
                .clone(),
        )
    }

    pub fn Capacity(&self) -> i32 {
        self.limit
    }
}

pub struct Request {
    pub Tp: i64,
    pub StartTs: u64,
    pub Data: Vec<u8>,
    pub KeyRanges: Option<KeyRanges>,
    pub PartitionIDAndRanges: Vec<PartitionIDAndRanges>,
    pub Concurrency: i32,
    pub CoprRequestLimiter: Option<Arc<CoprRequestLimiter>>,
    pub QueryCopStoreLimiter: Option<Arc<QueryCopStoreLimiter>>,
    pub IsolationLevel: IsoLevel,
    pub Priority: i32,
    pub MemTracker: Option<memory::Tracker>,
    pub KeepOrder: bool,
    pub Desc: bool,
    pub NotFillCache: bool,
    pub ReplicaRead: ReplicaReadType,
    pub StoreType: StoreType,
    pub Cacheable: bool,
    pub SchemaVar: i64,
    pub BatchCop: bool,
    pub TaskID: u64,
    pub TiDBServerID: u64,
    pub TxnScope: String,
    pub ReadReplicaScope: String,
    pub IsStaleness: bool,
    pub ClosestReplicaReadAdjuster: Option<CoprRequestAdjuster>,
    pub MatchStoreLabels: Vec<metapb::StoreLabel>,
    pub ResourceGroupTagger: Option<ResourceGroupTagBuilder>,
    pub Paging: Paging,
    pub RequestSource: util::RequestSource,
    pub StoreBatchSize: i32,
    pub AllowBatchTaskDataMerge: bool,
    pub ExecuteBatchTasksSerially: bool,
    pub ResourceGroupName: String,
    pub LimitSize: u64,
    pub StoreBusyThreshold: Duration,
    pub TiKVClientReadTimeout: u64,
    pub MaxExecutionTime: u64,
    pub MaxKeysRead: u64,
    pub MaxKeysReadCounter: Option<AtomicU64>,
    pub RunawayChecker: Option<resourcegroup::SharedRunawayChecker>,
    pub ResourceControlInterceptor: Option<resourcegroup::SharedCopRUInterceptor>,
    pub ConnID: u64,
    pub ConnAlias: String,
}

// CoprRequestAdjuster 返回 true 表示按规则修改了请求。
pub type CoprRequestAdjuster = fn(&mut Request, i32) -> bool;

// PartitionIDAndRanges 用于 TiFlash PartitionTableScan 的分区范围绑定。
pub struct PartitionIDAndRanges {
    pub ID: i64,
    pub KeyRanges: Vec<KeyRange>,
}

pub const GlobalReplicaScope: &str = oracle::GlobalTxnScope;

pub use execdetails_group1::util::PoolTaskDetails;

// ResultSubset 表示单个存储单元返回的一段结果及其内存/耗时元数据。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CopRuntimeEvidence {
    pub total_keys: u64,
    pub processed_keys: u64,
    pub processed_bytes: u64,
    pub tikv_response_bytes: Option<u64>,
}

pub trait ResultSubset {
    /// Read-pool diagnostics retained by this completed response.
    fn ReadPoolTaskDetails(&self) -> Option<PoolTaskDetails> {
        None
    }
    fn CopRuntimeEvidence(&self) -> Option<CopRuntimeEvidence> {
        None
    }
    fn GetData(&self) -> &[u8];
    fn GetStartKey(&self) -> Key;
    fn MemSize(&self) -> i64;
    fn RespTime(&self) -> Duration;
}

// Response::Next 在结果耗尽时返回 Ok(None)，Close 负责释放底层请求资源。
pub trait Response {
    fn Next(
        &mut self,
        ctx: &context::Context,
    ) -> Result<Option<Box<dyn ResultSubset>>, errors::SharedError>;
    fn Close(&mut self) -> Result<(), errors::SharedError>;
}

pub trait Snapshot: Retriever {
    fn BatchGet(
        &self,
        ctx: &context::Context,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, errors::SharedError>;
    fn SetOption(&mut self, opt: i32, val: Option<Box<dyn Any>>);

    /// Number of point-read entries retained by the snapshot cache.
    ///
    /// Backends without an observable cache keep the Go-compatible fallback
    /// of zero; mockstore overrides this for clustered-index regressions.
    fn SnapCacheSize(&self) -> usize {
        0
    }
}

// SnapshotInterceptor 可分别拦截单读、批量读和正反向迭代。
pub trait SnapshotInterceptor {
    fn OnGet(
        &self,
        ctx: &context::Context,
        snap: &dyn Snapshot,
        k: Key,
        options: &[GetOption],
    ) -> Result<ValueEntry, errors::SharedError>;
    fn OnBatchGet(
        &self,
        ctx: &context::Context,
        snap: &dyn Snapshot,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, errors::SharedError>;
    fn OnIter(
        &self,
        snap: &dyn Snapshot,
        k: Key,
        upper_bound: Option<Key>,
    ) -> Result<Box<dyn Iterator>, errors::SharedError>;
    fn OnIterReverse(
        &self,
        snap: &dyn Snapshot,
        k: Option<Key>,
        lower_bound: Option<Key>,
    ) -> Result<Box<dyn Iterator>, errors::SharedError>;
}

pub trait BatchGetter {
    fn BatchGet(
        &self,
        ctx: &context::Context,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, errors::SharedError>;
}

// BatchGetValue 把批量 ValueEntry 映射转换为纯 value 映射，任一读取错误原样返回。
pub fn BatchGetValue(
    ctx: &context::Context,
    getter: &dyn BatchGetter,
    keys: &[Key],
) -> Result<HashMap<String, Vec<u8>>, errors::SharedError> {
    let entries = getter.BatchGet(ctx, keys, &[])?;
    Ok(entries
        .into_iter()
        .map(|(k, entry)| (k, entry.Value))
        .collect())
}

pub trait Driver {
    // Open 的 path 使用具体存储驱动定义的格式。
    fn Open(&self, path: &str) -> Result<Box<dyn Storage>, errors::SharedError>;
}

// Storage 对应至少提供快照隔离的 KV 存储总接口。
#[derive(Clone, Debug, Default)]
pub struct SSTImportStats {
    pub keys: usize,
    pub bytes: usize,
    pub write_rpcs: usize,
    pub ingest_rpcs: usize,
}

/// Shared per-store limiter used at actual SST WriteBatch transport boundaries.
/// Implementations must stop waiting when the import context is cancelled.
pub trait SSTWriteLimiter: Send + Sync {
    fn WaitN(&self, context: &context::Context, store_id: u64, bytes: usize) -> Result<(), errors::SharedError>;
}
#[derive(Clone, Default)]
pub struct SSTImportOptions {
    pub context: context::Context,
    pub write_limiter: Option<Arc<dyn SSTWriteLimiter>>,
}

pub trait Storage {
    /// Read the first available TiKV coprocessor split configuration through PD
    /// store metadata. Non-PD stores return None; query failures remain errors so
    /// the range planner can apply its documented defaults.
    fn DDLRegionSplitConfig(
        &self,
        _context: &context::Context,
    ) -> Result<Option<(i64, i64)>, errors::SharedError> {
        Ok(None)
    }
    /// Number of live TiKV stores for TTL scan splitting. Non-TiKV stores
    /// return `None` and retain the default split count.
    fn TTLStoreCount(&self) -> Result<Option<usize>, errors::SharedError> {
        Ok(None)
    }
    /// Return TiKV Region boundaries for TTL record keys. Non-Region stores
    /// return `None`, which schedules one full-table scan range.
    fn TTLRegionRanges(
        &self,
        _start: &[u8],
        _end: &[u8],
    ) -> Result<Option<Vec<(Vec<u8>, Vec<u8>)>>, errors::SharedError> {
        Ok(None)
    }
    /// Return actual Region boundaries for a DDL backfill key interval.
    /// Stores without Regions return None; the planner then uses one interval.
    fn DDLRegionRanges(
        &self,
        _start: &[u8],
        _end: &[u8],
    ) -> Result<Option<Vec<(Vec<u8>, Vec<u8>)>>, errors::SharedError> {
        Ok(None)
    }
    /// Publish a TiFlash learner placement rule for a physical table. Stores
    /// without PD (for example the in-memory test store) have no rule target.
    fn PublishTiFlashPlacementRule(
        &self,
        _table_id: i64,
        _count: u64,
        _location_labels: &[String],
    ) -> Result<(), errors::SharedError> {
        Ok(())
    }
    /// Return observed physical replica progress, or None when this storage
    /// has no PD/TiFlash status source.
    fn ObserveTiFlashReplicaProgress(
        &self,
        _table_id: i64,
        _replica_count: u64,
    ) -> Result<Option<f64>, errors::SharedError> {
        Ok(None)
    }
    /// Physically ingest sorted unique logical KV pairs at a PD commit TSO.
    /// Unsupported stores must report an error, never silently use SQL writes.
    fn ImportSST(
        &self,
        _commit_ts: u64,
        _pairs: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<SSTImportStats, errors::SharedError> {
        Err(errors::New("storage does not support physical SST import"))
    }
    /// Preserve older store implementations while letting TiKV observe the
    /// same shared limiter and cancellation throughout an import.
    fn ImportSSTWithOptions(&self, commit_ts: u64, pairs: Vec<(Vec<u8>, Vec<u8>)>, options: SSTImportOptions) -> Result<SSTImportStats, errors::SharedError> {
        if options.context.is_cancelled() { return Err(errors::New("SST import cancelled")); }
        if let Some(limiter) = options.write_limiter {
            let bytes = pairs.iter().map(|(key, value)| key.len() + value.len()).sum();
            limiter.WaitN(&options.context, 0, bytes)?;
        }
        self.ImportSST(commit_ts, pairs)
    }
    fn Begin(&self, opts: &[tikv::TxnOption]) -> Result<Box<dyn Transaction>, errors::SharedError>;
    fn GetSnapshot(&self, ver: Version) -> Box<dyn Snapshot>;
    fn GetClient(&self) -> &dyn Client;
    fn GetMPPClient(&self) -> &dyn MPPClient;
    fn Close(&mut self) -> Result<(), errors::SharedError>;
    fn UUID(&self) -> String;
    fn CurrentVersion(&self, txn_scope: &str) -> Result<Version, errors::SharedError>;
    fn GetOracle(&self) -> &dyn oracle::Oracle;
    /// Request a timestamp through the oracle, retaining synchronous store compatibility.
    fn TimestampFuture(&self, scope: &str, low_resolution: bool) -> Box<dyn oracle::Future> {
        let future = if low_resolution {
            self.GetOracle().GetLowResolutionTimestampAsync(scope)
        } else {
            self.GetOracle().GetTimestampAsync(scope)
        };
        future.unwrap_or_else(|| {
            Box::new(oracle::ReadyFuture(self.CurrentVersion(scope).map(|v| v.Ver)))
        })
    }
    fn SupportDeleteRange(&self) -> bool;
    fn Name(&self) -> String;
    fn Describe(&self) -> String;
    fn ShowStatus(
        &self,
        ctx: &context::Context,
        key: &str,
    ) -> Result<Box<dyn Any>, errors::SharedError>;
    fn GetMemCache(&self) -> &dyn MemManager;
    fn GetMinSafeTS(&self, txn_scope: &str) -> u64;
    fn GetLockWaits(&self) -> Result<Vec<deadlockpb::WaitForEntry>, errors::SharedError>;
    fn GetCodec(&self) -> tikv::Codec;
    fn SetOption(&self, key: Box<dyn Any>, value: Box<dyn Any>);
    fn GetOption(&self, key: &dyn Any) -> Option<&dyn Any>;
    fn GetClusterID(&self) -> u64;
    fn DDLPDEndpoints(&self) -> Result<Vec<String>, errors::SharedError> {
        Err(errors::New("DDL PD endpoints unavailable"))
    }
    fn DDLKeyspaceID(&self) -> Result<u32, errors::SharedError> {
        if self.GetKeyspace().is_empty() {
            Ok(u32::MAX)
        } else {
            Err(errors::New("DDL keyspace ID unavailable"))
        }
    }
    /// Encode PD region ranges with this store's actual API/keyspace codec.
    fn EncodeDDLRegionRange(
        &self,
        start: &[u8],
        end: &[u8],
    ) -> Result<(Vec<u8>, Vec<u8>), errors::SharedError> {
        if !self.GetKeyspace().is_empty() {
            return Err(errors::New("DDL keyspace codec unavailable"));
        }
        Ok((
            codec_dependency::EncodeBytes(Vec::new(), start),
            codec_dependency::EncodeBytes(Vec::new(), end),
        ))
    }
    fn GetKeyspace(&self) -> String;
    /// Test-only observation of successful oracle timestamp requests.
    fn TSORequestCountForTest(&self) -> u64 {
        0
    }
}

// EtcdBackend 由真实 TiKV 存储实现，暴露地址、TLS 与 GC worker 生命周期。
pub trait EtcdBackend {
    fn EtcdAddrs(&self) -> Result<Vec<String>, errors::SharedError>;
    fn GetPDAddrs(&self) -> Result<Vec<String>, errors::SharedError>;
    fn TLSConfig(&self) -> Option<&tls::Config>;
    fn StartGCWorker(&self) -> Result<(), errors::SharedError>;
}

pub trait StorageWithPD {
    fn GetPDClient(&self) -> &dyn pd::Client;
    fn GetPDHTTPClient(&self) -> &dyn pdhttp::Client;
}

pub type FnKeyCmp = fn(Key) -> bool;

// Iterator 对应 KV 游标；Next 可能失败，Close 必须由调用方执行。
pub trait Iterator {
    fn Valid(&self) -> bool;
    fn Key(&self) -> Key;
    fn Value(&self) -> Vec<u8>;
    fn Next(&mut self) -> Result<(), errors::SharedError>;
    fn Close(&mut self);
}

pub trait SplittableStore {
    fn SplitRegions(
        &self,
        ctx: &context::Context,
        split_keys: &[Vec<u8>],
        scatter: bool,
        table_id: Option<i64>,
    ) -> Result<Vec<u64>, errors::SharedError>;
    fn WaitScatterRegionFinish(
        &self,
        ctx: &context::Context,
        region_id: u64,
        back_off: i32,
    ) -> Result<(), errors::SharedError>;
    fn CheckRegionInScattering(&self, region_id: u64) -> Result<bool, errors::SharedError>;
}

pub const PriorityNormal: i32 = 0;
pub const PriorityLow: i32 = 1;
pub const PriorityHigh: i32 = 2;

// IsoLevel 保持 SI、RC、RCCheckTS 的 Go iota 顺序。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum IsoLevel {
    SI = 0,
    RC = 1,
    RCCheckTS = 2,
}

// ResourceGroupTagBuilder 缓存 SQL/计划摘要和 keyspace，按请求首键补表 ID 与标签。
pub struct ResourceGroupTagBuilder {
    sqlDigest: Option<parser::Digest>,
    planDigest: Option<parser::Digest>,
    keyspaceName: Vec<u8>,
}

pub fn NewResourceGroupTagBuilder(keyspace_name: Vec<u8>) -> ResourceGroupTagBuilder {
    ResourceGroupTagBuilder {
        sqlDigest: None,
        planDigest: None,
        keyspaceName: keyspace_name,
    }
}

impl ResourceGroupTagBuilder {
    pub fn SetSQLDigest(&mut self, digest: parser::Digest) -> &mut Self {
        self.sqlDigest = Some(digest);
        self
    }
    pub fn SetPlanDigest(&mut self, digest: parser::Digest) -> &mut Self {
        self.planDigest = Some(digest);
        self
    }

    // BuildProtoTagger 返回的闭包只描述 Go 回调形状；真正写请求仍由调用方触发。
    pub fn BuildProtoTagger(&self) -> impl Fn(&mut tikvrpc::Request) + '_ {
        move |req| self.Build(req)
    }

    // EncodeTagWithKey mirrors the gogo protobuf wire output used by Go.
    pub fn EncodeTagWithKey(&self, key: &[u8]) -> Option<Vec<u8>> {
        let mut tag = tipb::ResourceGroupTag::new();
        if !self.keyspaceName.is_empty() {
            tag.set_keyspace_name(self.keyspaceName.clone());
        }
        if let Some(digest) = &self.sqlDigest {
            if !digest.Bytes().is_empty() {
                tag.set_sql_digest(digest.Bytes());
            }
        }
        if let Some(digest) = &self.planDigest {
            if !digest.Bytes().is_empty() {
                tag.set_plan_digest(digest.Bytes());
            }
        }
        tag.set_table_id(if key.is_empty() {
            0
        } else {
            decodeTableID(Key(key.to_vec()))
        });
        if !key.is_empty() {
            tag.set_label(resourcegrouptag::GetResourceGroupLabelByKey(key));
        }
        tag.write_to_bytes().ok()
    }

    // Build 防御空请求，并且只在编码结果非空时覆盖 ResourceGroupTag。
    pub fn Build(&self, req: &mut tikvrpc::Request) {
        let key = resourcegrouptag::GetFirstKeyFromRequest(req);
        if let Some(encoded) = self.EncodeTagWithKey(&key) {
            if !encoded.is_empty() {
                req.ResourceGroupTag = encoded;
            }
        }
    }
}

// DecodeTableIDFunc 通过回调打破 kv 与 tablecodec 的 import cycle。
pub static mut DecodeTableIDFunc: Option<fn(Key) -> i64> = None;

fn decodeTableID(key: Key) -> i64 {
    // 读取可变全局函数指针需要 unsafe；这里保留 Go 可替换回调语义。
    unsafe { DecodeTableIDFunc.map(|decode| decode(key)).unwrap_or(0) }
}

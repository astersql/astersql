// Copyright 2026 AsterSQL.

// KV 包入口：键值存储抽象、事务接口与子模块汇总。
//
// 本 crate 对应 TiDB `pkg/kv`，为上层会话/执行器提供：
// - Storage / Transaction / Snapshot 等读写与事务抽象；
// - Key、错误码、MPP 客户端、事务选项等支撑类型；
// - 通过 `include!` / `#[path]` 汇入各实现文件。
//
// 文件前半多为迁移期依赖桩（stub）与外部 crate 再导出；后半为正式子模块与测试夹具。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

extern crate self as kv_dependency;

use std::sync::Arc;

/// 请求上下文：可携带内部请求来源（RequestSource）。
pub mod context {
    use crate::option_impl::RequestSource;
    use std::ops::Deref;
    use tokio_util::sync::CancellationToken;

    #[derive(Clone, Debug)]
    pub struct Context {
        cancellation: CancellationToken,
        request_source: Option<RequestSource>,
    }

    impl Default for Context {
        fn default() -> Self {
            Self {
                cancellation: CancellationToken::new(),
                request_source: None,
            }
        }
    }

    impl PartialEq for Context {
        fn eq(&self, other: &Self) -> bool {
            self.request_source == other.request_source
                && self.is_cancelled() == other.is_cancelled()
        }
    }

    impl Eq for Context {}

    impl Deref for Context {
        type Target = CancellationToken;

        fn deref(&self) -> &Self::Target {
            &self.cancellation
        }
    }

    impl Context {
        /// 构造新的可取消 Context。
        pub fn new() -> Self {
            Self::default()
        }

        /// 构造占位 Context（对应 Go `context.TODO()` 用法）。
        pub fn todo() -> Self {
            Self::default()
        }
        /// 返回已挂载的请求来源。
        pub fn RequestSource(&self) -> Option<&RequestSource> {
            self.request_source.as_ref()
        }
        /// 内部：写入请求来源并返回新 Context。
        pub(crate) fn with_request_source(mut self, value: RequestSource) -> Self {
            self.request_source = Some(value);
            self
        }
    }
}
pub use context::Context;

/// 错误类型再导出。
pub mod errors {
    pub use dbterror_dependency::errors::*;
}
/// 编解码基础类型再导出。
pub mod types {
    pub use codec_dependency::types::*;
}
/// codec 包再导出。
pub mod codec {
    pub use codec_dependency::*;
    pub type Error = codec_dependency::errors::SharedError;
}
/// 大小计算工具再导出。
pub mod size {
    pub use size_dependency::*;
}
/// hack/map ABI 再导出。
pub mod hack {
    pub use hack_dependency::map_abi::{MemAwareMap, NewMemAwareMap};
}
/// dbterror 再导出。
pub mod dbterror {
    pub use dbterror_dependency::dbterror::*;
}
/// errno 再导出。
pub mod errno {
    pub use dbterror_dependency::errno::*;
}
/// MySQL 错误名再导出。
pub mod parser_mysql {
    pub use ::parser_mysql::errname::{ErrMessage, Message};
}
/// tipb / Resource Group Tag 相关再导出。
pub mod tipb {
    pub use resourcegrouptag_dependency::tipb::*;
}
/// 全局配置与默认事务大小限制。
pub mod config {
    pub use config_dependency::*;
    /// 默认单条 mutation 大小上限（6 MiB）。
    pub const DefTxnEntrySizeLimit: u64 = 6 * 1024 * 1024;
    /// 默认事务总大小上限（100 MiB）。
    pub const DefTxnTotalSizeLimit: u64 = 100 * 1024 * 1024;
}
/// TiKV store 侧值条目与 Get/BatchGet 选项桩。
pub mod tikvstore {
    /// 带提交时间戳的 value 条目。
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    pub struct ValueEntry {
        pub Value: Vec<u8>,
        /// 提交时间戳（CommitTS）；0 表示未知或未提交快照值。
        pub CommitTs: u64,
    }
    #[derive(Clone, Debug, Default)]
    pub struct GetOption;
    #[derive(Clone, Debug, Default)]
    pub struct BatchGetOption;
    pub type GetOptions = Vec<GetOption>;
    pub type BatchGetOptions = Vec<BatchGetOption>;
    #[derive(Clone, Debug, Default)]
    pub struct GetOrBatchGetOption;
    /// 构造 ValueEntry。
    pub fn NewValueEntry(value: Vec<u8>, commit_ts: u64) -> ValueEntry {
        ValueEntry {
            Value: value,
            CommitTs: commit_ts,
        }
    }
    /// 将 BatchGet 选项降级为逐键 Get 选项列表。
    pub fn BatchGetToGetOptions(options: Vec<BatchGetOption>) -> Vec<GetOption> {
        options.into_iter().map(|_| GetOption).collect()
    }
    /// 请求返回 CommitTS 的选项标记。
    pub fn WithReturnCommitTS() -> GetOrBatchGetOption {
        GetOrBatchGetOption
    }
    #[derive(Clone, Debug, Default)]
    pub struct LockCtx {
        /// TiKV's server-side pessimistic lock wait budget in milliseconds.
        /// -1 selects NOWAIT, positive values bound the service-side wait.
        pub WaitTimeoutMs: i64,
        /// Select TiKV SharedPessimisticLock mutation for FK parent checks.
        pub Shared: bool,
        /// Fair-lock counters populated from TiKV per-key lock results.
        pub AggressiveLockNewCount: i32,
        pub AggressiveLockDerivedCount: i32,
        pub LockedWithConflictCount: i32,
    }
}
/// TiKV 事务相关桩类型。
pub mod tikv {
    /// Options applied while opening a transaction.
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    pub enum TxnOption {
        /// Allocate the transaction timestamp through the storage oracle.
        #[default]
        Default,
        /// Open a read transaction at an already selected timestamp.
        StartTS(u64),
    }
    #[derive(Clone, Debug, Default)]
    pub struct MemDBCheckpoint;
    #[derive(Clone, Debug, Default)]
    pub struct Codec;
}
/// SQL digest 包装。
pub mod parser {
    #[derive(Clone, Debug, Default)]
    pub struct Digest(Vec<u8>);
    impl Digest {
        pub fn new(value: Vec<u8>) -> Self {
            Self(value)
        }
        pub fn Bytes(&self) -> Vec<u8> {
            self.0.clone()
        }
    }
}
/// Resource Group Tag 工具再导出。
pub mod resourcegrouptag {
    pub use resourcegrouptag_dependency::resource_group_tag::GetResourceGroupLabelByKey;
    pub fn GetFirstKeyFromRequest(req: &crate::tikvrpc::Request) -> Vec<u8> {
        resourcegrouptag_dependency::resource_group_tag::GetFirstKeyFromRequest(Some(&req.inner))
            .unwrap_or_default()
            .to_vec()
    }
}
/// TiKV RPC 请求桩：携带 ResourceGroupTag。
pub mod tikvrpc {
    pub struct Request {
        pub inner: resourcegrouptag_dependency::resource_group_tag::Request,
        pub ResourceGroupTag: Vec<u8>,
    }
    impl Default for Request {
        fn default() -> Self {
            Self {
                inner: resourcegrouptag_dependency::resource_group_tag::Request {
                    payload: resourcegrouptag_dependency::resource_group_tag::RequestPayload::Other,
                },
                ResourceGroupTag: Vec::new(),
            }
        }
    }
}
/// 生成仅含空结构体的占位模块。
macro_rules! empty_struct_module {
    ($module:ident, $name:ident) => {
        pub mod $module {
            #[derive(Clone, Debug, Default)]
            pub struct $name;
        }
    };
}
empty_struct_module!(memory, Tracker);
empty_struct_module!(model, TableInfo);
empty_struct_module!(metapb, StoreLabel);
pub mod resourcegroup {
    use std::sync::Arc;

    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    pub struct CopRPCRequestInfo {
        pub resource_group_name: String,
        pub request_type: String,
        pub region_id: u64,
        pub store_address: String,
        pub data_bytes: usize,
        /// MVCC read-byte estimate used for request-side RU pre-charge.
        pub predicted_read_bytes: u64,
        pub priority_low: bool,
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct CopRPCResponseInfo {
        pub region_id: u64,
        pub data_bytes: usize,
        pub processed_keys: u64,
        /// Storage-engine MVCC bytes, independent of the response payload size.
        pub read_bytes: u64,
        pub kv_cpu_ms: f64,
        pub error: Option<String>,
    }

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub enum RunawayAction {
        #[default]
        None,
        CoolDown,
        Kill,
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct CopRequest {
        pub priority_low: bool,
        pub resource_group_name: String,
        pub max_execution_duration_ms: u64,
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct RUDetails {
        pub read_ru: f64,
        pub write_ru: f64,
    }

    pub trait RunawayChecker: Send + Sync {
        fn BeforeCopRequest(&self, request: &mut CopRequest) -> Result<(), String>;
        fn CheckThresholds(
            &self,
            ru: Option<&RUDetails>,
            processed_keys: u64,
            original_error: Option<&str>,
        ) -> Result<(), String>;
        fn CheckAction(&self) -> RunawayAction;
        fn ResetTotalProcessedKeys(&self);
    }

    pub type SharedRunawayChecker = Arc<dyn RunawayChecker>;

    pub trait CopRUInterceptor: std::fmt::Debug + Send + Sync {
        fn OnRequestWait(&self, request: &CopRPCRequestInfo) -> Result<RUDetails, String>;
        fn OnRequestWaitCancellable(
            &self,
            request: &CopRPCRequestInfo,
            _cancelled: Option<&std::sync::atomic::AtomicBool>,
        ) -> Result<RUDetails, String> {
            self.OnRequestWait(request)
        }
        fn OnResponseWait(
            &self,
            request: &CopRPCRequestInfo,
            response: &CopRPCResponseInfo,
        ) -> Result<RUDetails, String>;
    }

    pub type SharedCopRUInterceptor = Arc<dyn CopRUInterceptor>;
}
empty_struct_module!(tls, Config);
pub mod deadlockpb {
    #[derive(Clone, Debug, Default, Eq, PartialEq)]
    pub struct WaitForEntry {
        pub txn: u64,
        pub wait_for_txn: u64,
        pub key_hash: u64,
        pub key: Vec<u8>,
        pub resource_group_tag: Vec<u8>,
        pub wait_time: u64,
    }
}
pub mod kvrpcpb {
    /// 磁盘空间保护级别；数值与 kvproto `kvrpcpb.DiskFullOpt` 保持一致。
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    #[repr(i32)]
    pub enum DiskFullOpt {
        /// 磁盘达到 AlmostFull/Full 时拒绝写入。
        #[default]
        NotAllowedOnFull = 0,
        /// 允许 AlmostFull，达到 Full 时仍拒绝。
        AllowedOnAlmostFull = 1,
        /// 即使达到 Full 也允许写入。
        AllowedOnAlreadyFull = 2,
    }
}
pub mod tiflash {
    #[derive(Clone, Copy, Debug, Default)]
    pub struct ReplicaRead;
}
pub mod tiflashcompute {
    #[derive(Clone, Copy, Debug, Default)]
    pub struct DispatchPolicy;
}
pub mod trxevents {
    pub type EventCallback = fn();
}
pub mod util {
    #[derive(Clone, Debug, Default)]
    pub struct RateLimit;
    #[derive(Clone, Debug, Default)]
    pub struct RequestSource;
}
/// 时间戳预言机（Oracle）相关桩。
pub mod oracle {
    pub const GlobalTxnScope: &str = "global";
    /// A timestamp request whose wait preserves the original result and error.
    pub trait Future: Send {
        fn Wait(&mut self) -> Result<u64, crate::errors::SharedError>;
    }
    pub struct ReadyFuture(pub Result<u64, crate::errors::SharedError>);
    impl Future for ReadyFuture {
        fn Wait(&mut self) -> Result<u64, crate::errors::SharedError> {
            self.0.clone()
        }
    }
    pub trait Oracle: Send + Sync {
        /// None lets stores without an asynchronous oracle use CurrentVersion.
        fn GetTimestampAsync(&self, _scope: &str) -> Option<Box<dyn Future>> {
            None
        }
        fn GetLowResolutionTimestampAsync(&self, scope: &str) -> Option<Box<dyn Future>> {
            self.GetTimestampAsync(scope)
        }
    }
}
/// PD 客户端桩。
pub mod pd {
    pub trait Client {}
}
/// PD HTTP 客户端桩。
pub mod pdhttp {
    pub trait Client {}
}
/// 事务 RPC 请求种类（用于拦截器匹配）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestKind {
    /// 两阶段提交的预写阶段（Prewrite）。
    Prewrite,
    /// 两阶段提交的提交阶段（Commit）。
    Commit,
    /// 悲观锁请求。
    PessimisticLock,
    Other,
}
/// 拦截器可见的精简 RPC 请求描述。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RpcRequest {
    pub Kind: RequestKind,
    pub ResourceGroupName: String,
}
/// RPC 拦截器类型别名。
pub type RpcInterceptor = Arc<dyn Fn(&mut RpcRequest) -> Result<(), Error> + Send + Sync>;
/// 退避器（Backoffer）占位。
#[derive(Default)]
pub struct Backoffer;

/// The packet source behind an established TiFlash MPP connection.
///
/// Keeping the receive operation on a trait preserves the streaming RPC
/// contract: callers consume one packet at a time instead of materialising an
/// entire query result before returning from dispatch.
///
/// 已建立的 TiFlash MPP 连接背后的数据包源；按包接收以保持流式 RPC 契约。
pub trait MPPDataPacketStream: Send {
    fn Recv(&mut self) -> Result<Option<kvproto::mpp::MppDataPacket>, Error>;
    fn Close(&mut self) -> Result<(), Error>;
}

/// An established MPP stream, including the packet returned together with the
/// connection response by TiKV/TiFlash.
///
/// 已建立的 MPP 流：含建连时首包与后续流式接收通道。
#[derive(Default)]
pub struct MPPStreamResponse {
    first_packet: Option<kvproto::mpp::MppDataPacket>,
    stream: Option<Box<dyn MPPDataPacketStream>>,
}

impl MPPStreamResponse {
    /// 用首包与底层流构造响应。
    pub fn New(
        first_packet: Option<kvproto::mpp::MppDataPacket>,
        stream: Box<dyn MPPDataPacketStream>,
    ) -> Self {
        Self {
            first_packet,
            stream: Some(stream),
        }
    }

    /// 先消费首包，再从底层流接收。
    pub fn Recv(&mut self) -> Result<Option<kvproto::mpp::MppDataPacket>, Error> {
        if self.first_packet.is_some() {
            return Ok(self.first_packet.take());
        }
        match self.stream.as_mut() {
            Some(stream) => stream.Recv(),
            None => Ok(None),
        }
    }

    /// 关闭底层流。
    pub fn Close(&mut self) -> Result<(), Error> {
        match self.stream.as_mut() {
            Some(stream) => stream.Close(),
            None => Ok(()),
        }
    }
}

/// Key 标志位（见 keyflags.rs）。
pub mod keyflags {
    use crate::*;
    include!("keyflags.rs");
}
pub use keyflags::*;
#[cfg(test)]
#[path = "keyflags_test.rs"]
mod keyflags_test;
/// 断言级别相关（见 assertion.rs）。
pub mod assertion {
    use crate::*;
    include!("assertion.rs");
}
pub use assertion::*;
/// Key 编解码与区间（见 key.rs）。
pub mod key {
    use crate::*;
    include!("key.rs");
}
pub use key::*;
/// KV 核心接口与常量（见 kv.rs）。
pub mod kv {
    use crate::*;
    use protobuf::Message;
    include!("kv.rs");
}
pub use kv::*;
/// Schema 检查器（见 checker.rs）。
pub mod checker {
    use crate::*;
    include!("checker.rs");
}
pub use checker::*;
/// KV 错误定义（见 error.rs）。
pub mod error {
    use crate::*;
    include!("error.rs");
}
pub use error::*;
/// 缓存 DB（见 cachedb.rs）。
pub mod cachedb {
    use crate::*;
    include!("cachedb.rs");
    #[cfg(test)]
    mod cachedb_test {
        include!("cachedb_test.rs");
    }
}
pub use cachedb::*;
/// 故障注入（见 fault_injection.rs）。
pub mod fault_injection {
    use crate::*;
    include!("fault_injection.rs");
}
pub use fault_injection::*;
/// 迭代器抽象（见 iter.rs）。
pub mod iter {
    use crate::*;
    include!("iter.rs");
}
pub use iter::*;
#[cfg(test)]
#[path = "iter_test.rs"]
mod iter_test;

/// MPP 任务与客户端（见 mpp.rs）。
#[path = "mpp.rs"]
mod mpp_impl;
pub use mpp_impl::*;
/// 事务选项与请求来源（见 option.rs）。
#[path = "option.rs"]
mod option_impl;
pub use option_impl::*;
/// 事务作用域变量（见 txn_scope_var.rs）。
#[path = "txn_scope_var.rs"]
mod txn_scope_var_impl;
pub use txn_scope_var_impl::*;
/// 内部事务 RunInNewTxn 等（见 txn.rs）。
#[path = "txn.rs"]
mod txn_impl;
pub use txn_impl::*;
/// UniStore 相关（见 unistore.rs）。
#[path = "unistore.rs"]
mod unistore_impl;
pub use unistore_impl::*;
/// 通用工具函数（见 utils.rs）。
#[path = "utils.rs"]
mod utils_impl;
pub use utils_impl::*;
/// 客户端 Variables（见 variables.rs）。
#[path = "variables.rs"]
mod variables_impl;
pub use variables_impl::*;
#[cfg(test)]
#[path = "variables_test.rs"]
mod variables_test;
/// Version 类型（见 version.rs）。
#[path = "version.rs"]
mod version_impl;
pub use version_impl::*;

#[cfg(test)]
#[path = "assertion_1_aster_unit_test.rs"]
mod assertion_1_aster_unit_test;
#[cfg(test)]
#[path = "mpp_2_aster_unit_test.rs"]
mod mpp_2_aster_unit_test;

/// test_fixtures 收敛测试专用的最小 Storage/Transaction/Retriever 实现，
/// 对应 Go 测试文件里散落定义的 mockTxn/mockStorage/mockSnapshot/mockMap。
#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_fixtures {
    use crate::*;
    use std::any::Any;
    use std::collections::HashMap;

    /// NullClient 只在 Storage::GetClient 未被实际调用时充当占位返回值，
    /// 对应 Go mockStorage.GetClient 返回的 nil Client。
    struct NullClient;

    impl Client for NullClient {
        fn Send(
            &self,
            _ctx: &Context,
            _req: &Request,
            _vars: &dyn Any,
            _option: &ClientSendOption,
        ) -> Option<Box<dyn Response>> {
            panic!("NullClient.Send is unused by the kv test fixtures")
        }
        fn IsRequestTypeSupported(&self, _req_type: i64, _sub_type: i64) -> bool {
            false
        }
    }

    /// NullOracle 对应 Go mockStorage.GetOracle 返回的 nil oracle.Oracle。
    struct NullOracle;

    impl oracle::Oracle for NullOracle {}

    /// MockTxn 对应 Go `mockTxn`：Commit 始终返回可重试错误；本地写集合用于事务回归。
    pub struct MockTxn {
        opts: HashMap<i32, Box<dyn Any>>,
        writes: HashMap<Key, Vec<u8>>,
        valid: bool,
        startTs: u64,
        checkpoint: tikv::MemDBCheckpoint,
    }

    impl Default for MockTxn {
        fn default() -> Self {
            Self {
                opts: HashMap::new(),
                writes: HashMap::new(),
                valid: true,
                startTs: 0,
                checkpoint: tikv::MemDBCheckpoint::default(),
            }
        }
    }

    impl MockTxn {
        /// Pending KV mutations visible to transaction-focused tests.
        pub fn pending_writes(&self) -> &HashMap<Key, Vec<u8>> {
            &self.writes
        }
        /// with_start_ts 对应 Go 测试里通过 tikv.WithStartTS 间接设置的起始时间戳。
        pub fn with_start_ts(startTs: u64) -> Self {
            Self {
                startTs,
                ..Self::default()
            }
        }

        /// Reset 对应 Go mockTxn.Reset：仅把事务标记为无效。
        pub fn Reset(&mut self) {
            self.valid = false;
        }
    }

    impl Getter for MockTxn {
        fn Get(
            &self,
            _ctx: &Context,
            _k: Key,
            _options: &[GetOption],
        ) -> Result<ValueEntry, Error> {
            Ok(ValueEntry::default())
        }
    }

    impl Retriever for MockTxn {
        fn Iter(&self, _k: Key, _upper_bound: Option<Key>) -> Result<Box<dyn Iterator>, Error> {
            Ok(Box::new(EmptyIterator))
        }
        fn IterReverse(
            &self,
            _k: Option<Key>,
            _lower_bound: Option<Key>,
        ) -> Result<Box<dyn Iterator>, Error> {
            Ok(Box::new(EmptyIterator))
        }
    }

    impl Mutator for MockTxn {
        fn Set(&mut self, k: Key, v: Vec<u8>) -> Result<(), Error> {
            self.writes.insert(k, v);
            Ok(())
        }
        fn Delete(&mut self, k: Key) -> Result<(), Error> {
            self.writes.remove(&k);
            Ok(())
        }
    }

    impl RetrieverMutator for MockTxn {}

    impl FairLockingController for MockTxn {
        fn StartFairLocking(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn RetryFairLocking(&mut self, _ctx: &Context) -> Result<(), Error> {
            Ok(())
        }
        fn CancelFairLocking(&mut self, _ctx: &Context) -> Result<(), Error> {
            Ok(())
        }
        fn DoneFairLocking(&mut self, _ctx: &Context) -> Result<(), Error> {
            Ok(())
        }
        fn IsInFairLockingMode(&self) -> bool {
            false
        }
    }

    impl Transaction for MockTxn {
        fn Size(&self) -> usize {
            0
        }
        fn Mem(&self) -> u64 {
            0
        }
        fn SetMemoryFootprintChangeHook(&mut self, _hook: Box<dyn Fn(u64)>) {}
        fn MemHookSet(&self) -> bool {
            false
        }
        fn Len(&self) -> usize {
            0
        }
        // Commit 始终返回可重试错误，对应 Go mockTxn.Commit。
        fn Commit(&mut self, _ctx: &Context) -> Result<(), Error> {
            Err(ErrTxnRetryable.FastGenByArgs(&[]))
        }
        fn Rollback(&mut self) -> Result<(), Error> {
            self.valid = false;
            self.writes.clear();
            Ok(())
        }
        fn String(&self) -> String {
            String::new()
        }
        fn LockKeys(
            &mut self,
            _ctx: &Context,
            _lock_ctx: &mut LockCtx,
            _keys: &[Key],
        ) -> Result<(), Error> {
            Ok(())
        }
        fn LockKeysFunc(
            &mut self,
            _ctx: &Context,
            _lock_ctx: &mut LockCtx,
            f: &mut dyn FnMut(),
            _keys: &[Key],
        ) -> Result<(), Error> {
            f();
            Ok(())
        }
        fn SetOption(&mut self, opt: i32, val: Option<Box<dyn Any>>) {
            match val {
                Some(value) => {
                    self.opts.insert(opt, value);
                }
                None => {
                    self.opts.remove(&opt);
                }
            }
        }
        fn GetOption(&self, opt: i32) -> Option<&dyn Any> {
            self.opts.get(&opt).map(|value| value.as_ref())
        }
        fn IsReadOnly(&self) -> bool {
            true
        }
        fn StartTS(&self) -> u64 {
            self.startTs
        }
        fn CommitTS(&self) -> u64 {
            0
        }
        fn Valid(&self) -> bool {
            self.valid
        }
        fn GetMemBuffer(&self) -> &dyn MemBuffer {
            panic!("MockTxn.GetMemBuffer is unused by the kv test fixtures")
        }
        fn GetSnapshot(&self) -> &dyn Snapshot {
            panic!("MockTxn.GetSnapshot is unused by the kv test fixtures")
        }
        fn SetVars(&mut self, _vars: Box<dyn Any>) {}
        fn GetVars(&self) -> &dyn Any {
            &()
        }
        fn BatchGet(
            &self,
            _ctx: &Context,
            _keys: &[Key],
            _options: &[BatchGetOption],
        ) -> Result<HashMap<String, ValueEntry>, Error> {
            Ok(HashMap::new())
        }
        fn IsPessimistic(&self) -> bool {
            false
        }
        fn CacheTableInfo(&mut self, _id: i64, _info: model::TableInfo) {}
        fn GetTableInfo(&self, _id: i64) -> Option<&model::TableInfo> {
            None
        }
        fn SetDiskFullOpt(&mut self, _level: kvrpcpb::DiskFullOpt) {}
        fn ClearDiskFullOpt(&mut self) {}
        fn GetMemDBCheckpoint(&self) -> &tikv::MemDBCheckpoint {
            &self.checkpoint
        }
        fn RollbackMemDBToCheckpoint(&mut self, _checkpoint: &tikv::MemDBCheckpoint) {}
        fn IsPipelined(&self) -> bool {
            false
        }
        fn MayFlush(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }

    /// MockStorage 对应 Go `mockStorage`：Begin 总是返回新 MockTxn，Snapshot 依托空 MockMap。
    pub struct MockStorage {
        startTs: u64,
        keyspace: String,
    }

    impl Default for MockStorage {
        fn default() -> Self {
            Self {
                startTs: 0,
                keyspace: String::new(),
            }
        }
    }

    impl MockStorage {
        pub fn with_start_ts(startTs: u64) -> Self {
            Self {
                startTs,
                ..Self::default()
            }
        }

        /// 指定 keyspace 名称构造 MockStorage。
        pub fn with_keyspace(keyspace: impl Into<String>) -> Self {
            Self {
                keyspace: keyspace.into(),
                ..Self::default()
            }
        }
    }

    impl Storage for MockStorage {
        fn Begin(&self, _opts: &[tikv::TxnOption]) -> Result<Box<dyn Transaction>, Error> {
            Ok(Box::new(MockTxn::with_start_ts(self.startTs)))
        }
        fn GetSnapshot(&self, _ver: Version) -> Box<dyn Snapshot> {
            Box::new(MockSnapshot {
                store: Box::new(MockMap::default()),
            })
        }
        fn GetClient(&self) -> &dyn Client {
            &NullClient
        }
        fn GetMPPClient(&self) -> &dyn MPPClient {
            panic!("MockStorage.GetMPPClient is unused by the kv test fixtures")
        }
        fn Close(&mut self) -> Result<(), Error> {
            Ok(())
        }
        fn UUID(&self) -> String {
            String::new()
        }
        // CurrentVersion 返回当前最大已提交版本，对应 Go mockStorage.CurrentVersion。
        fn CurrentVersion(&self, _txn_scope: &str) -> Result<Version, Error> {
            Ok(NewVersion(1))
        }
        fn GetOracle(&self) -> &dyn oracle::Oracle {
            &NullOracle
        }
        fn SupportDeleteRange(&self) -> bool {
            false
        }
        fn Name(&self) -> String {
            "KVMockStorage".to_owned()
        }
        fn Describe(&self) -> String {
            "KVMockStorage is a mock Store implementation, only for unittests in KV package"
                .to_owned()
        }
        fn ShowStatus(&self, _ctx: &Context, _key: &str) -> Result<Box<dyn Any>, Error> {
            Ok(Box::new(()))
        }
        fn GetMemCache(&self) -> &dyn MemManager {
            panic!("MockStorage.GetMemCache is unused by the kv test fixtures")
        }
        fn GetMinSafeTS(&self, _txn_scope: &str) -> u64 {
            0
        }
        fn GetLockWaits(&self) -> Result<Vec<deadlockpb::WaitForEntry>, Error> {
            Ok(Vec::new())
        }
        fn GetCodec(&self) -> tikv::Codec {
            tikv::Codec
        }
        fn SetOption(&self, _key: Box<dyn Any>, _value: Box<dyn Any>) {}
        fn GetOption(&self, _key: &dyn Any) -> Option<&dyn Any> {
            None
        }
        fn GetClusterID(&self) -> u64 {
            1
        }
        fn GetKeyspace(&self) -> String {
            self.keyspace.clone()
        }
    }

    /// MockSnapshot 对应 Go `mockSnapshot`：所有读操作委托给内部 Retriever。
    pub struct MockSnapshot {
        store: Box<dyn Retriever>,
    }

    impl Getter for MockSnapshot {
        fn Get(&self, ctx: &Context, k: Key, options: &[GetOption]) -> Result<ValueEntry, Error> {
            self.store.Get(ctx, k, options)
        }
    }

    impl Retriever for MockSnapshot {
        fn Iter(&self, k: Key, upper_bound: Option<Key>) -> Result<Box<dyn Iterator>, Error> {
            self.store.Iter(k, upper_bound)
        }
        fn IterReverse(
            &self,
            k: Option<Key>,
            lower_bound: Option<Key>,
        ) -> Result<Box<dyn Iterator>, Error> {
            self.store.IterReverse(k, lower_bound)
        }
    }

    impl Snapshot for MockSnapshot {
        // BatchGet 逐键读取并跳过未命中项，对应 Go mockSnapshot.BatchGet。
        fn BatchGet(
            &self,
            ctx: &Context,
            keys: &[Key],
            options: &[BatchGetOption],
        ) -> Result<HashMap<String, ValueEntry>, Error> {
            let mut result = HashMap::with_capacity(keys.len());
            let get_options = BatchGetToGetOptions(options.to_vec()).unwrap_or_default();
            for key in keys {
                match self.store.Get(ctx, key.clone(), &get_options) {
                    Ok(value) => {
                        result.insert(KeyMapName(&key.0), value);
                    }
                    Err(error) if IsErrNotFound(&error) => continue,
                    Err(error) => return Err(error),
                }
            }
            Ok(result)
        }
        fn SetOption(&mut self, _opt: i32, _val: Option<Box<dyn Any>>) {}
    }

    /// MockMap 对应 Go `mockMap`：以并行 index/value 向量线性查找模拟 KV 表。
    #[derive(Default)]
    pub struct MockMap {
        index: Vec<Key>,
        value: Vec<Vec<u8>>,
    }

    impl Getter for MockMap {
        fn Get(&self, _ctx: &Context, k: Key, _options: &[GetOption]) -> Result<ValueEntry, Error> {
            for (i, key) in self.index.iter().enumerate() {
                if *key == k {
                    return Ok(NewValueEntry(self.value[i].clone(), 0));
                }
            }
            Err(ErrNotExist.FastGenByArgs(&[]))
        }
    }

    impl Retriever for MockMap {
        fn Iter(&self, _k: Key, _upper_bound: Option<Key>) -> Result<Box<dyn Iterator>, Error> {
            Ok(Box::new(EmptyIterator))
        }
        fn IterReverse(
            &self,
            _k: Option<Key>,
            _lower_bound: Option<Key>,
        ) -> Result<Box<dyn Iterator>, Error> {
            Ok(Box::new(EmptyIterator))
        }
    }

    impl Mutator for MockMap {
        fn Set(&mut self, k: Key, v: Vec<u8>) -> Result<(), Error> {
            for (i, key) in self.index.iter().enumerate() {
                if *key == k {
                    self.value[i] = v;
                    return Ok(());
                }
            }
            self.index.push(k);
            self.value.push(v);
            Ok(())
        }
        fn Delete(&mut self, k: Key) -> Result<(), Error> {
            if let Some(i) = self.index.iter().position(|key| *key == k) {
                self.index.remove(i);
                self.value.remove(i);
            }
            Ok(())
        }
    }

    impl RetrieverMutator for MockMap {}
}

#[cfg(test)]
#[path = "checker_test.rs"]
mod checker_test;
#[cfg(test)]
#[path = "error_test.rs"]
mod error_test;
#[cfg(test)]
#[path = "fault_injection_test.rs"]
mod fault_injection_test;
#[cfg(test)]
#[path = "interface_mock_test.rs"]
mod interface_mock_test;
#[cfg(test)]
#[path = "key_test.rs"]
mod key_test;
/// Resource Group Tag 编码测试。
#[cfg(test)]
#[path = "kv_test.rs"]
mod kv_test;
/// 测试入口公共配置检查。
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
/// Mock 接口冒烟测试。
#[cfg(test)]
#[path = "mock_test.rs"]
mod mock_test;
/// 事务选项 TxnSource 测试。
#[cfg(test)]
#[path = "option_test.rs"]
mod option_test;
#[cfg(test)]
#[path = "txn_test.rs"]
mod txn_test;
#[cfg(test)]
#[path = "utils_test.rs"]
mod utils_test;
#[cfg(test)]
#[path = "version_test.rs"]
mod version_test;

#[cfg(test)]
#[path = "assertion_test.rs"]
mod assertion_test;

pub mod paging_resource_control;

#[cfg(test)]
#[path = "paging_resource_control_test.rs"]
mod paging_resource_control_test;

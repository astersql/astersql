// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.
//
// Local stand-ins for PD / TiKV / kvproto / domain / distsql / metautil / utils
// boundaries (darwin-safe; no heavy workspace deps).

//! 本文件是 `br/pkg/backup` 的**依赖边界桩集合**（darwin/arm64 友好）。
//!
//! 目的：在不拉取 PD/TiKV/kvproto/domain/distsql 等重依赖的前提下，
//! 让 backup 客户端、store 收发与 schema 路径可编译、可单测。
//! 各 `pub mod` 仅保留调用方用到的类型/函数子集；许多方法是空操作、
//! 内存实现或恒定返回，**不能**当作生产级 PD/对象存储/checkpoint 实现。
//! 与 Go 的对齐点在于符号名、关键字段与错误文案，而非完整行为等价。
//!
//! 组织：顶层提供 Error/Context/限流无关的 WorkerPool；子模块按上游包名镜像。
//! failpoint 与 skip_sleep 开关专供测试竞态注入。

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 统一 Result 别名，错误类型为本地 Error。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 轻量错误：仅消息字符串，Trace/Annotate 模拟 pingcap/errors 链式标注。
pub struct Error {
    pub msg: String,
}

impl Error {
    /// 由消息字符串构造轻量 Error。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// 透传错误，模拟 pingcap/errors::Trace（本桩不做栈叠加）。
    pub fn Trace(err: Self) -> Self {
        err
    }

    /// 在错误消息前附加上下文前缀，对齐 errors.Annotate。
    pub fn Annotate(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
        }
    }

    /// 带格式化参数的 Annotate，前缀含 fmt 与 Display 参数。
    pub fn Annotatef(err: Self, fmt: impl AsRef<str>, args: impl fmt::Display) -> Self {
        Self {
            msg: format!("{}: {}: {}", fmt.as_ref(), args, err.msg),
        }
    }

    /// 返回错误引用本身；桩环境无深层 cause 链。
    pub fn Cause(err: &Self) -> &Self {
        err
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// 等价 errors.Errorf，构造带格式消息的 Error。
pub fn errors_Errorf(msg: impl Into<String>) -> Error {
    Error::new(msg)
}

/// Cancellation token approximating Go context.Context.
#[derive(Clone, Default)]
/// Go context.Context 近似：用共享 Mutex 保存取消原因。
pub struct Context {
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl Context {
    /// 返回未取消的根 Context，对齐 context.Background。
    pub fn Background() -> Self {
        Self::default()
    }

    /// 返回未取消 Context，对齐 context.TODO。
    pub fn TODO() -> Self {
        Self::default()
    }

    /// 派生可取消子 Context，并返回 CancelFunc。
    pub fn WithCancel(parent: &Self) -> (Self, CancelFunc) {
        let child = Self {
            cancelled: Arc::new(Mutex::new(parent.Err())),
        };
        let cancel = CancelFunc {
            cancelled: child.cancelled.clone(),
        };
        (child, cancel)
    }

    /// 派生可取消子 Context，并返回可带 cause 的取消句柄。
    pub fn WithCancelCause(parent: &Self) -> (Self, CancelCauseFunc) {
        let child = Self {
            cancelled: Arc::new(Mutex::new(parent.Err())),
        };
        let cancel = CancelCauseFunc {
            cancelled: child.cancelled.clone(),
        };
        (child, cancel)
    }

    /// 写入调用方提供的取消原因。
    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    /// 读取当前取消原因；未取消则 None。
    pub fn Err(&self) -> Option<Error> {
        self.cancelled.lock().unwrap().clone()
    }

    /// 是否已取消（Err 非空）。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

#[derive(Clone)]
/// 无 cause 取消句柄；cancel/call 写入 "context canceled"。
pub struct CancelFunc {
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl CancelFunc {
    /// 写入默认取消原因 "context canceled"。
    pub fn cancel(self) {
        *self.cancelled.lock().unwrap() = Some(Error::new("context canceled"));
    }

    /// 与 cancel 相同，供可复用引用调用。
    pub fn call(&self) {
        *self.cancelled.lock().unwrap() = Some(Error::new("context canceled"));
    }
}

#[derive(Clone)]
/// 带可选 cause 的取消；None 时回落默认取消文案。
pub struct CancelCauseFunc {
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl CancelCauseFunc {
    /// 写入可选 cause；缺省回落 "context canceled"。
    pub fn cancel(&self, cause: Option<Error>) {
        *self.cancelled.lock().unwrap() =
            Some(cause.unwrap_or_else(|| Error::new("context canceled")));
    }
}

/// kvproto/brpb 备份协议的本地精简镜像：请求/响应/文件元数据与流式客户端 trait；非完整 protobuf 生成物。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod backuppb {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 备份目标 URI 包装（非完整 StorageBackend oneof）。
    pub struct StorageBackend {
        pub uri: String,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 加密密钥材料占位，供 checkpoint/元数据路径传参。
    pub struct CipherInfo {
        pub cipher_key: Vec<u8>,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
    /// SST/备份文件元数据：名、键范围、crc 与统计。
    pub struct File {
        pub name: String,
        pub start_key: Vec<u8>,
        pub end_key: Vec<u8>,
        pub crc64xor: u64,
        pub total_kvs: u64,
        pub total_bytes: u64,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 统计文件索引条目：小统计可内联，大统计使用 name 引用。
    pub struct StatsFileIndex {
        pub name: String,
        pub InlineData: Vec<u8>,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 放置策略原始 Info 字节。
    pub struct PlacementPolicy {
        pub Info: Vec<u8>,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 库表 schema 与 checksum/统计索引；含 merge 选项标志。
    pub struct Schema {
        pub Db: Vec<u8>,
        pub Table: Vec<u8>,
        pub Crc64Xor: u64,
        pub TotalKvs: u64,
        pub TotalBytes: u64,
        pub Stats: Vec<u8>,
        pub StatsIndex: Vec<StatsFileIndex>,
        pub IsMergeOptionAllowed: bool,
        pub PartitionMergeOptionAllowed: std::collections::HashMap<String, bool>,
    }

    #[derive(Clone, Debug, Default)]
    /// 半开键区间 [StartKey, EndKey)。
    pub struct KeyRange {
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
    }

    #[derive(Clone, Debug, Default)]
    /// 备份请求内嵌锁上下文（Resolved/Committed），非取消用 Context。
    pub struct Context {
        pub ResolvedLocks: Vec<u64>,
        pub CommittedLocks: Vec<u64>,
    }

    #[derive(Clone, Debug, Default)]
    /// Backup RPC 请求：键范围、子区间与起止版本。
    pub struct BackupRequest {
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub SubRanges: Vec<KeyRange>,
        pub StartVersion: u64,
        pub EndVersion: u64,
        pub Context: Option<Context>,
    }

    #[derive(Clone, Debug, Default)]
    /// KV 错误细节，可挂 Locked 锁信息。
    pub struct KvError {
        pub Locked: Option<LockInfo>,
    }

    #[derive(Clone, Debug, Default)]
    /// 事务锁关键字段，供锁解析路径构造 Lock。
    pub struct LockInfo {
        pub Key: Vec<u8>,
        pub Primary: Vec<u8>,
        pub TTL: u64,
        pub TxnVersion: u64,
    }

    #[derive(Clone, Debug)]
    /// Backup 错误细节：KvError / RegionError / None。
    pub enum ErrorDetail {
        KvError {
            KvError: KvError,
        },
        RegionError {
            RegionError: crate::stubs::errorpb::Error,
        },
        None,
    }

    impl Default for ErrorDetail {
        fn default() -> Self {
            Self::None
        }
    }

    #[derive(Clone, Debug, Default)]
    /// backuppb.Error：Msg + Detail（Kv/Region/None）。
    pub struct Error {
        pub Msg: String,
        pub Detail: ErrorDetail,
    }

    #[derive(Clone, Debug, Default)]
    /// Backup RPC 响应：区间、文件列表、API 版本与错误。
    pub struct BackupResponse {
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub Files: Vec<File>,
        pub ApiVersion: i32,
        pub Error: Option<Error>,
    }

    impl BackupResponse {
        /// 返回可选 BackupResponse.Error。
        pub fn GetError(&self) -> Option<&Error> {
            self.Error.as_ref()
        }
        /// 返回 StartKey 切片视图。
        pub fn GetStartKey(&self) -> &[u8] {
            &self.StartKey
        }
        /// 返回 EndKey 切片视图。
        pub fn GetEndKey(&self) -> &[u8] {
            &self.EndKey
        }
    }

    /// gRPC Backup 客户端边界；需注入 mock 实现流式备份。
    pub trait BackupClient: Send + Sync {
        fn Backup(
            &self,
            ctx: &crate::stubs::Context,
            req: &BackupRequest,
        ) -> crate::stubs::Result<Box<dyn BackupStream>>;
    }

    /// Backup 响应流：Recv/CloseSend。
    pub trait BackupStream: Send {
        fn Recv(&mut self) -> crate::stubs::Result<Option<BackupResponse>>;
        fn CloseSend(&mut self) -> crate::stubs::Result<()>;
    }
}

/// errorpb.Error 精简桩，仅保留 Message，供 RegionError 细节挂载。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod errorpb {
    #[derive(Clone, Debug, Default)]
    /// errorpb.Error：仅 Message 字段，挂到 RegionError。
    pub struct Error {
        pub Message: String,
    }
}

/// metapb.Store/StoreLabel 精简镜像，供 PD 列举与存活检查使用。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod metapb {
    #[derive(Clone, Debug, Default)]
    /// PD store 标签键值。
    pub struct StoreLabel {
        pub Key: String,
        pub Value: String,
    }

    #[derive(Clone, Debug, Default)]
    /// PD store：Id/地址/状态/标签。
    pub struct Store {
        pub Id: u64,
        pub Address: String,
        pub Labels: Vec<StoreLabel>,
        pub State: i32,
    }

    impl Store {
        /// 返回 Store.Id。
        pub fn GetId(&self) -> u64 {
            self.Id
        }
    }
}

/// APIVersion 枚举桩，对齐 TiKV API v1/v1ttl/v2 取值。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod kvrpcpb {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    /// TiKV API 版本枚举桩（v1/v1ttl/v2 取值对齐）。
    pub enum APIVersion {
        #[default]
        V1 = 0,
        V1TTL = 1,
        V2 = 2,
    }
}

/// TiDB model 包精简：CIStr、表/库/DDL Job/Placement 等备份 schema 路径所需字段。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod model {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    /// 大小写不敏感字符串：O 原始、L 小写。
    pub struct CIStr {
        pub O: String,
        pub L: String,
    }

    impl CIStr {
        /// 由任意字符串构造：O 保留原样，L 转小写。
        pub fn new(s: impl Into<String>) -> Self {
            let O = s.into();
            let L = O.to_lowercase();
            Self { O, L }
        }
        /// 返回原始大小写 O 字段。
        pub fn String(&self) -> &str {
            &self.O
        }
    }

    impl fmt::Display for CIStr {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.O)
        }
    }

    use std::fmt;

    /// 与 Go `StatePublic` 对齐的本地常量。
    pub const StatePublic: i32 = 5;
    /// 与 Go `TableCacheStatusDisable` 对齐的本地常量。
    pub const TableCacheStatusDisable: i32 = 0;

    /// 与 Go `ActionCreatePlacementPolicy` 对齐的本地常量。
    pub const ActionCreatePlacementPolicy: i32 = 64;
    /// 与 Go `ActionAlterPlacementPolicy` 对齐的本地常量。
    pub const ActionAlterPlacementPolicy: i32 = 65;
    /// 与 Go `ActionDropPlacementPolicy` 对齐的本地常量。
    pub const ActionDropPlacementPolicy: i32 = 66;
    /// 与 Go `ActionAlterTablePartitionPlacement` 对齐的本地常量。
    pub const ActionAlterTablePartitionPlacement: i32 = 67;
    /// 与 Go `ActionModifySchemaDefaultPlacement` 对齐的本地常量。
    pub const ActionModifySchemaDefaultPlacement: i32 = 68;
    /// 与 Go `ActionAlterTablePlacement` 对齐的本地常量。
    pub const ActionAlterTablePlacement: i32 = 69;
    /// 与 Go `ActionAlterTableAttributes` 对齐的本地常量。
    pub const ActionAlterTableAttributes: i32 = 70;
    /// 与 Go `ActionAlterTablePartitionAttributes` 对齐的本地常量。
    pub const ActionAlterTablePartitionAttributes: i32 = 71;

    /// 与 Go `JobStateDone` 对齐的本地常量。
    pub const JobStateDone: i32 = 5;
    /// 与 Go `JobStateSynced` 对齐的本地常量。
    pub const JobStateSynced: i32 = 6;

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// `IndexInfo` 本地精简结构：含 ID/Name/State；非完整上游类型。
    pub struct IndexInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub State: i32,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// `PartitionDefinition` 本地精简结构：含 ID/Name；非完整上游类型。
    pub struct PartitionDefinition {
        pub ID: i64,
        pub Name: CIStr,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// `PartitionInfo` 本地精简结构：含 Definitions；非完整上游类型。
    pub struct PartitionInfo {
        pub Definitions: Vec<PartitionDefinition>,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// `PlacementPolicyRef` 本地精简结构：含 Name；非完整上游类型。
    pub struct PlacementPolicyRef {
        pub Name: CIStr,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 表元数据子集：ID、名、自增/随机位、分区与放置引用。
    pub struct TableInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub Version: i64,
        pub Indices: Vec<IndexInfo>,
        pub Partition: Option<PartitionInfo>,
        pub AutoIncID: i64,
        pub AutoIncIDExtra: i64,
        pub AutoRandID: i64,
        pub PlacementPolicyRef: Option<PlacementPolicyRef>,
        pub TableCacheStatusType: i32,
        pub IsSequence: bool,
        pub IsView: bool,
        pub HasAutoInc: bool,
        pub SepAutoInc: bool,
        pub AutoRandomBits: u64,
    }

    impl TableInfo {
        /// `IsSequence` 判定辅助，对齐 Go utils。
        pub fn IsSequence(&self) -> bool {
            self.IsSequence
        }
        /// `IsView` 判定辅助，对齐 Go utils。
        pub fn IsView(&self) -> bool {
            self.IsView
        }
        /// 是否配置了分离自增列（SepAutoIncCols 非空）。
        pub fn SepAutoInc(&self) -> bool {
            self.SepAutoInc
        }
        /// AutoRandomBits 是否非零。
        pub fn ContainsAutoRandomBits(&self) -> bool {
            self.AutoRandomBits > 0
        }
        /// 清空 PlacementPolicyRef，备份 schema 时忽略放置策略。
        pub fn ClearPlacement(&mut self) {
            self.PlacementPolicyRef = None;
        }
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 库元数据子集：ID、名与放置策略引用。
    pub struct DBInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub PlacementPolicyRef: Option<PlacementPolicyRef>,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// DDL binlog 附带的 schema 版本与库表快照。
    pub struct HistoryInfo {
        pub SchemaVersion: i64,
        pub DBInfo: Option<DBInfo>,
        pub TableInfo: Option<TableInfo>,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// DDL Job 子集：类型/状态与 Binlog HistoryInfo。
    pub struct Job {
        pub ID: i64,
        pub Type: i32,
        pub State: i32,
        pub BinlogInfo: Option<HistoryInfo>,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// Placement Policy 名称与 ID。
    pub struct PolicyInfo {
        pub ID: i64,
        pub Name: CIStr,
    }
}

/// TSO 物理/逻辑位运算工具，对齐 tidb/pkg/store/mockstore/unistore/oracle 常用函数。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod oracle {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    /// TSO 物理部分左移位数（18），对齐 oracle。
    pub const PHYSICAL_SHIFT_BITS: u64 = 18;

    /// 物理时间左移后与逻辑部分相加，合成 TSO。
    pub fn ComposeTS(physical: i64, logical: i64) -> u64 {
        ((physical as u64) << PHYSICAL_SHIFT_BITS) + (logical as u64)
    }

    /// 右移取出 TSO 的物理毫秒部分。
    pub fn ExtractPhysical(ts: u64) -> i64 {
        (ts >> PHYSICAL_SHIFT_BITS) as i64
    }

    /// 由 TSO 物理部分还原为 SystemTime（毫秒精度）。
    pub fn GetTimeFromTS(ts: u64) -> SystemTime {
        let ms = ExtractPhysical(ts);
        UNIX_EPOCH + Duration::from_millis(ms as u64)
    }

    /// SystemTime 相对 UNIX_EPOCH 的毫秒物理时间。
    pub fn GetPhysical(t: SystemTime) -> i64 {
        t.duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
    }
}

/// 事务锁解析边界：Lock 结构、LockResolver trait 与 DummyBackoffer；Resolve 实现由调用方注入。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod txnlock {
    use crate::stubs::backuppb::LockInfo;

    #[derive(Clone, Debug, Default)]
    /// txnlock.Lock：由 LockInfo 拷贝而来。
    pub struct Lock {
        pub Key: Vec<u8>,
        pub Primary: Vec<u8>,
        pub TTL: u64,
        pub TxnVersion: u64,
    }

    /// 从 backuppb.LockInfo 拷贝构造 txnlock.Lock。
    pub fn NewLock(info: &LockInfo) -> Lock {
        Lock {
            Key: info.Key.clone(),
            Primary: info.Primary.clone(),
            TTL: info.TTL,
            TxnVersion: info.TxnVersion,
        }
    }

    /// 锁解析接口；ResolveLocksForRead 由调用方实现。
    pub trait LockResolver: Send + Sync {
        fn ResolveLocksForRead(
            &self,
            _bo: &dyn Backoffer,
            _startTS: u64,
            locks: &[Lock],
            _forRead: bool,
        ) -> crate::stubs::Result<(Vec<u64>, Vec<u64>, Vec<u64>)>;
    }

    /// 退避接口占位，供锁解析签名满足。
    pub trait Backoffer: Send {}

    #[derive(Default)]
    /// 空 Backoffer，满足 ResolveLocks 签名。
    pub struct DummyBackoffer;
    impl Backoffer for DummyBackoffer {}
}

/// PD 客户端最小接口：集群 ID、TSO、单 store/全 store 查询。
pub trait PdClient: Send + Sync {
    fn GetClusterID(&self, _ctx: &Context) -> u64;
    fn GetTS(&self, _ctx: &Context) -> Result<(i64, i64)>;
    fn GetStore(&self, _ctx: &Context, storeID: u64) -> Result<metapb::Store>;
    fn GetAllStores(&self, _ctx: &Context) -> Result<Vec<metapb::Store>>;
}

/// 键编码接口；默认恒等，IdentityCodec 无变换。
pub trait Codec: Send + Sync {
    fn EncodeRange(&self, start: &[u8], end: &[u8]) -> (Vec<u8>, Vec<u8>) {
        (start.to_vec(), end.to_vec())
    }
}

#[derive(Default)]
/// 恒等 Codec 实现，起止键原样返回。
pub struct IdentityCodec;
impl Codec for IdentityCodec {}

/// KV Snapshot 空 trait，占位满足 Storage 返回类型。
pub trait Snapshot: Send + Sync {}

/// KV 客户端空 trait，checksum 等路径仅持有类型。
pub trait KvClient: Send + Sync {}

/// TiKV Storage 抽象：快照/客户端/编解码/当前版本。
pub trait Storage: Send + Sync {
    fn GetSnapshot(&self, _ver: Version) -> Box<dyn Snapshot>;
    fn GetClient(&self) -> Box<dyn KvClient>;
    fn GetCodec(&self) -> Box<dyn Codec>;
    fn CurrentVersion(&self, _scope: &str) -> Result<Version>;
}

#[derive(Clone, Copy, Debug, Default)]
/// 存储版本包装，Ver 为内部 u64。
pub struct Version {
    pub Ver: u64,
}

impl Version {
    /// 用 u64 包装为 Version。
    pub fn New(v: u64) -> Self {
        Self { Ver: v }
    }
}

/// 全局事务 scope 空串，对齐 kv.GlobalTxnScope。
pub const GlobalTxnScope: &str = "";

/// 外部存储读写/遍历/URI，backup 元数据与 checkpoint 共用。
pub trait ExternalStorage: Send + Sync {
    fn WriteFile(&self, _ctx: &Context, name: &str, data: &[u8]) -> Result<()>;
    fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool>;
    fn WalkDir(
        &self,
        _ctx: &Context,
        _opt: &WalkOption,
        f: &mut dyn FnMut(&str, i64) -> Result<()>,
    ) -> Result<()>;
    fn URI(&self) -> String;
    fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>>;
}

#[derive(Default)]
/// WalkDir 选项占位，当前字段为空。
pub struct WalkOption {}

/// 外部存储 Options 与 Storage 别名，对齐 br/pkg/storage 调用约定。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod storeapi {
    /// 再导出存储别名，调用方可按 storeapi::Storage 书写。
    pub use super::{ExternalStorage as Storage, WalkOption};

    #[derive(Clone, Debug, Default)]
    /// 对象存储打开选项；send_credentials 对齐 Go。
    pub struct Options {
        pub send_credentials: bool,
    }
}

/// 对象存储工厂与内存 MemStorage：测试/darwin 路径用，非真实 S3/GCS 客户端。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod objstore {
    use crate::stubs::{Context, Error, Result, backuppb, storeapi};

    /// 工厂：打开 URI 对应的 MemStorage（忽略凭证选项）。
    pub fn New(
        _ctx: &Context,
        backend: &backuppb::StorageBackend,
        _opts: &storeapi::Options,
    ) -> Result<std::sync::Arc<dyn storeapi::Storage>> {
        Ok(std::sync::Arc::new(MemStorage::new(backend.uri.clone())))
    }

    #[derive(Default)]
    /// 进程内 HashMap 外部存储，供单测替代 S3/GCS。
    pub struct MemStorage {
        uri: String,
        files: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
    }

    impl MemStorage {
        /// 创建空内存 KV 文件表并记录 URI。
        pub fn new(uri: String) -> Self {
            Self {
                uri,
                files: std::sync::Mutex::new(std::collections::HashMap::new()),
            }
        }
        /// 预置内存文件，便于单测构造已有对象。
        pub fn insert(&self, name: &str, data: Vec<u8>) {
            self.files.lock().unwrap().insert(name.to_string(), data);
        }
    }

    impl storeapi::Storage for MemStorage {
        fn WriteFile(&self, _ctx: &Context, name: &str, data: &[u8]) -> Result<()> {
            self.files
                .lock()
                .unwrap()
                .insert(name.to_string(), data.to_vec());
            Ok(())
        }
        fn FileExists(&self, _ctx: &Context, name: &str) -> Result<bool> {
            Ok(self.files.lock().unwrap().contains_key(name))
        }
        fn WalkDir(
            &self,
            _ctx: &Context,
            _opt: &storeapi::WalkOption,
            f: &mut dyn FnMut(&str, i64) -> Result<()>,
        ) -> Result<()> {
            let files: Vec<_> = self
                .files
                .lock()
                .unwrap()
                .iter()
                .map(|(k, v)| (k.clone(), v.len() as i64))
                .collect();
            for (path, size) in files {
                f(&path, size)?;
            }
            Ok(())
        }
        fn URI(&self) -> String {
            self.uri.clone()
        }
        fn ReadFile(&self, _ctx: &Context, name: &str) -> Result<Vec<u8>> {
            self.files
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .ok_or_else(|| Error::new(format!("file not found: {name}")))
        }
    }
}

/// backupmeta 写入抽象：Append 操作、MetaWriter、StatsWriter；MemMetaWriter 仅内存收集。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod metautil {
    use crate::stubs::backuppb;
    use crate::stubs::{Context, Result};
    use std::sync::Mutex;

    /// 与 Go `LockFile` 对齐的本地常量。
    pub const LockFile: &str = "backup.lock";
    /// 与 Go `MetaFile` 对齐的本地常量。
    pub const MetaFile: &str = "backupmeta";

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    /// `AppendOp` 枚举取值对齐 Go，供分支判断。
    pub enum AppendOp {
        AppendSchema,
        AppendDDL,
        AppendDataFile,
    }

    /// 与 Go `AppendSchema` 对齐的本地常量。
    pub const AppendSchema: AppendOp = AppendOp::AppendSchema;
    /// 与 Go `AppendDDL` 对齐的本地常量。
    pub const AppendDDL: AppendOp = AppendOp::AppendDDL;
    /// 与 Go `AppendDataFile` 对齐的本地常量。
    pub const AppendDataFile: AppendOp = AppendOp::AppendDataFile;

    #[derive(Clone, Debug, Default)]
    /// `ChecksumStats` 本地精简结构：含 Crc64Xor/TotalKvs/TotalBytes；非完整上游类型。
    pub struct ChecksumStats {
        pub Crc64Xor: u64,
        pub TotalKvs: u64,
        pub TotalBytes: u64,
    }

    /// backupmeta 追加写抽象。
    pub trait MetaWriter: Send + Sync {
        fn StartWriteMetasAsync(&self, _ctx: &Context, _op: AppendOp);
        fn Send(&self, data: MetaPayload, _op: AppendOp) -> Result<()>;
        fn FinishWriteMetas(&self, _ctx: &Context, _op: AppendOp) -> Result<()>;
        fn NewStatsWriter(&self) -> StatsWriter;
    }

    #[derive(Clone, Debug)]
    /// `MetaPayload` 枚举取值对齐 Go，供分支判断。
    pub enum MetaPayload {
        Schema(backuppb::Schema),
        Bytes(Vec<u8>),
        Files(Vec<backuppb::File>),
    }

    #[derive(Default)]
    /// `MemMetaWriter` 本地精简结构：含 schemas/ddls/files/started；非完整上游类型。
    pub struct MemMetaWriter {
        pub schemas: Mutex<Vec<backuppb::Schema>>,
        pub ddls: Mutex<Vec<Vec<u8>>>,
        pub files: Mutex<Vec<backuppb::File>>,
        pub started: Mutex<bool>,
        pub finished: Mutex<bool>,
    }

    impl MetaWriter for MemMetaWriter {
        fn StartWriteMetasAsync(&self, _ctx: &Context, _op: AppendOp) {
            *self.started.lock().unwrap() = true;
        }
        fn Send(&self, data: MetaPayload, op: AppendOp) -> Result<()> {
            match (op, data) {
                (AppendOp::AppendSchema, MetaPayload::Schema(s)) => {
                    self.schemas.lock().unwrap().push(s);
                }
                (AppendOp::AppendDDL, MetaPayload::Bytes(b)) => {
                    self.ddls.lock().unwrap().push(b);
                }
                (AppendOp::AppendDataFile, MetaPayload::Files(f)) => {
                    self.files.lock().unwrap().extend(f);
                }
                _ => {}
            }
            Ok(())
        }
        fn FinishWriteMetas(&self, _ctx: &Context, _op: AppendOp) -> Result<()> {
            *self.finished.lock().unwrap() = true;
            Ok(())
        }
        fn NewStatsWriter(&self) -> StatsWriter {
            StatsWriter::default()
        }
    }

    #[derive(Default)]
    /// `StatsWriter` 本地精简结构，字段对齐 backup 调用子集。
    pub struct StatsWriter {
        indexes: Mutex<Vec<backuppb::StatsFileIndex>>,
    }

    impl StatsWriter {
        /// 写入小体量统计的内联索引；多次写入聚合到同一索引。
        pub fn BackupStats(
            &self,
            _db: &str,
            _table: &crate::stubs::model::TableInfo,
            stats: &[u8],
        ) -> Result<()> {
            let mut indexes = self.indexes.lock().unwrap();
            if let Some(index) = indexes.first_mut()
                && index.name.is_empty()
            {
                index.InlineData.extend_from_slice(stats);
            } else {
                indexes.push(backuppb::StatsFileIndex {
                    name: String::new(),
                    InlineData: stats.to_vec(),
                });
            }
            Ok(())
        }
        /// 返回已收集的 StatsFileIndex 列表（可为空）。
        pub fn BackupStatsDone(&self, _ctx: &Context) -> Result<Vec<backuppb::StatsFileIndex>> {
            Ok(self.indexes.lock().unwrap().clone())
        }
    }
}

/// 备份 checkpoint 元数据读写与 Runner 桩；Walk/Append 多数为空操作，勿当作完整 checkpoint 实现。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod checkpoint {
    use crate::stubs::backuppb::{CipherInfo, File};
    use crate::stubs::{Context, ExternalStorage, PdClient, Result};
    use serde::{Deserialize, Serialize};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// 备份 checkpoint 元数据相对路径。
    pub const CheckpointMetaPathForBackup: &str = "checkpoints/checkpoint.meta";

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 单表 checkpoint checksum 三项。
    pub struct ChecksumItem {
        pub TableID: i64,
        pub Crc64xor: u64,
        pub TotalKvs: u64,
        pub TotalBytes: u64,
    }

    #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// 备份 checkpoint 元：GC 服务、配置哈希、BackupTS。
    pub struct CheckpointMetadataForBackup {
        pub GCServiceId: String,
        pub ConfigHash: Vec<u8>,
        pub BackupTS: u64,
        #[serde(skip)]
        pub CheckpointChecksum: Option<HashMap<i64, ChecksumItem>>,
        #[serde(skip)]
        pub LoadCheckpointDataMap: bool,
    }

    #[derive(Clone, Debug, Default)]
    /// checkpoint 值：区间与已备份文件。
    pub struct BackupValueType {
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub Files: Vec<File>,
    }

    /// checkpoint 键类型别名（String），锚定 Go BackupKeyType。
    pub type BackupKeyType = String;

    /// 内存 checksum 缓存与 finished 标志，非异步 runner。
    pub struct CheckpointRunner {
        checksums: Mutex<HashMap<i64, ChecksumItem>>,
        finished: Mutex<bool>,
    }

    impl CheckpointRunner {
        /// 创建空的内存 CheckpointRunner（无后台刷写线程）。
        pub fn new() -> Arc<Self> {
            Arc::new(Self {
                checksums: Mutex::new(HashMap::new()),
                finished: Mutex::new(false),
            })
        }
        /// 按 table_id 缓存 checksum 三项，供后续元数据合并。
        pub fn FlushChecksum(
            &self,
            _ctx: &Context,
            table_id: i64,
            crc: u64,
            kvs: u64,
            bytes: u64,
        ) -> Result<()> {
            self.checksums.lock().unwrap().insert(
                table_id,
                ChecksumItem {
                    TableID: table_id,
                    Crc64xor: crc,
                    TotalKvs: kvs,
                    TotalBytes: bytes,
                },
            );
            Ok(())
        }
        /// 标记 runner 已结束；无后台刷盘线程。
        pub fn WaitForFinish(&self, _ctx: &Context, _flush: bool) {
            *self.finished.lock().unwrap() = true;
        }
    }

    /// 从固定路径读 JSON checkpoint 元数据。
    pub fn LoadCheckpointMetadata(
        ctx: &Context,
        storage: &dyn ExternalStorage,
    ) -> Result<CheckpointMetadataForBackup> {
        let data = storage.ReadFile(ctx, CheckpointMetaPathForBackup)?;
        serde_json::from_slice(&data).map_err(|e| crate::stubs::Error::new(e.to_string()))
    }

    /// 将 checkpoint 元数据序列化写回固定路径。
    pub fn SaveCheckpointMetadata(
        ctx: &Context,
        storage: &dyn ExternalStorage,
        meta: &CheckpointMetadataForBackup,
    ) -> Result<()> {
        let data = serde_json::to_vec(meta).map_err(|e| crate::stubs::Error::new(e.to_string()))?;
        storage.WriteFile(ctx, CheckpointMetaPathForBackup, &data)
    }

    /// 返回内存 CheckpointRunner，不启动真实异步刷写。
    pub fn StartCheckpointRunnerForBackup(
        _ctx: &Context,
        _storage: &dyn ExternalStorage,
        _cipher: Option<&CipherInfo>,
        _pd: &dyn PdClient,
    ) -> Result<Arc<CheckpointRunner>> {
        Ok(CheckpointRunner::new())
    }

    /// 追加 range/files 占位：本桩为空操作成功。
    pub fn AppendForBackup(
        _ctx: &Context,
        _runner: &CheckpointRunner,
        _start: &[u8],
        _end: &[u8],
        _files: &[File],
    ) -> Result<()> {
        Ok(())
    }

    /// 遍历 checkpoint 文件占位：立即返回零耗时。
    pub fn WalkCheckpointFileForBackup<F>(
        _ctx: &Context,
        _storage: &dyn ExternalStorage,
        _cipher: Option<&CipherInfo>,
        mut _f: F,
    ) -> Result<Duration>
    where
        F: FnMut(&str, BackupValueType) -> Result<()>,
    {
        Ok(Duration::ZERO)
    }
}

/// 进度区间树：按 Origin 记录已完成子区间，并重算未完成洞位。
pub mod rtree {
    use crate::stubs::backuppb::File;
    use crate::stubs::metautil::{AppendDataFile, ChecksumStats, MetaPayload, MetaWriter};
    use crate::stubs::{Error, Result};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug, Default)]
    /// 半开键区间 [StartKey, EndKey)。
    pub struct KeyRange {
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
    }

    #[derive(Clone, Default)]
    /// 按 PhysicalID 收集已完成区间与文件列表。
    pub struct RangeTree {
        pub PhysicalID: i64,
        ranges: Arc<Mutex<Vec<(Vec<u8>, Vec<u8>, Vec<File>)>>>,
    }

    impl RangeTree {
        fn overlaps(a_start: &[u8], a_end: &[u8], b_start: &[u8], b_end: &[u8]) -> bool {
            (a_end.is_empty() || b_start < a_end) && (b_end.is_empty() || a_start < b_end)
        }

        /// 强制写入一段 (start,end,files)，删除所有重叠旧段。
        pub fn Put(&self, start: Vec<u8>, end: Vec<u8>, files: Vec<File>) {
            let mut ranges = self.ranges.lock().unwrap();
            ranges.retain(|(old_start, old_end, _)| {
                !Self::overlaps(&start, &end, old_start, old_end)
            });
            ranges.push((start, end, files));
            ranges.sort_by(|a, b| a.0.cmp(&b.0));
        }
        /// force=false 且存在重叠时拒绝；否则覆盖重叠段后写入。
        pub fn PutForce(
            &self,
            start: Vec<u8>,
            end: Vec<u8>,
            files: Option<Vec<File>>,
            force: bool,
        ) -> bool {
            let mut ranges = self.ranges.lock().unwrap();
            let has_overlap = ranges
                .iter()
                .any(|(old_start, old_end, _)| Self::overlaps(&start, &end, old_start, old_end));
            if has_overlap && !force {
                return false;
            }
            ranges.retain(|(old_start, old_end, _)| {
                !Self::overlaps(&start, &end, old_start, old_end)
            });
            ranges.push((start, end, files.unwrap_or_default()));
            ranges.sort_by(|a, b| a.0.cmp(&b.0));
            true
        }
    }

    #[derive(Clone)]
    /// 进度单元：结果树 + 原始区间。
    pub struct ProgressRange {
        pub Res: RangeTree,
        pub Origin: KeyRange,
    }

    /// 未完成区间、checksum 映射与可选完成回调。
    pub struct ProgressRangeTree {
        ranges: Mutex<Vec<ProgressRange>>,
        checksum: Mutex<HashMap<i64, ChecksumStats>>,
        callback: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
        skip_checksum: bool,
        #[allow(dead_code)]
        meta_writer: Option<std::sync::Arc<dyn MetaWriter>>,
    }

    impl ProgressRangeTree {
        /// 构建进度树；可挂 MetaWriter 并选择跳过 checksum。
        pub fn new(meta: Option<std::sync::Arc<dyn MetaWriter>>, skip_checksum: bool) -> Self {
            Self {
                ranges: Mutex::new(Vec::new()),
                checksum: Mutex::new(HashMap::new()),
                callback: Mutex::new(None),
                skip_checksum,
                meta_writer: meta,
            }
        }

        /// 将 Origin 区间记入进度树；与已有 Origin 重叠时报错。
        pub fn Insert(&self, pr: ProgressRange) -> Result<()> {
            let mut ranges = self.ranges.lock().unwrap();
            if ranges.iter().any(|existing| {
                pr.Origin.StartKey < existing.Origin.EndKey
                    && existing.Origin.StartKey < pr.Origin.EndKey
            }) {
                return Err(Error::new(
                    "failed to insert the progress range into range tree, because there is a overlapping range",
                ));
            }
            ranges.push(pr);
            ranges.sort_by(|a, b| a.Origin.StartKey.cmp(&b.Origin.StartKey));
            Ok(())
        }

        /// 注册完成回调，在 clear_incomplete 时触发。
        pub fn SetCallBack<F: Fn() + Send + Sync + 'static>(&self, f: F) {
            *self.callback.lock().unwrap() = Some(Box::new(f));
        }

        /// 根据每个 Origin 的已完成子区间重算洞位；完成项写 meta 并移除。
        pub fn GetIncompleteRanges(&self) -> Result<Vec<crate::stubs::backuppb::KeyRange>> {
            let mut entries = self.ranges.lock().unwrap();
            let mut incomplete = Vec::new();
            let mut completed = Vec::new();

            for (idx, pr) in entries.iter().enumerate() {
                let mut segments = pr.Res.ranges.lock().unwrap().clone();
                segments.sort_by(|a, b| a.0.cmp(&b.0));
                let mut cursor = pr.Origin.StartKey.clone();
                let mut has_gap = false;
                for (start, end, _) in &segments {
                    if end.as_slice() <= cursor.as_slice()
                        || start.as_slice() >= pr.Origin.EndKey.as_slice()
                    {
                        continue;
                    }
                    if start.as_slice() > cursor.as_slice() {
                        has_gap = true;
                        incomplete.push(crate::stubs::backuppb::KeyRange {
                            StartKey: cursor.clone(),
                            EndKey: start.clone().min(pr.Origin.EndKey.clone()),
                        });
                    }
                    if end.as_slice() > cursor.as_slice() {
                        cursor = end.clone().min(pr.Origin.EndKey.clone());
                    }
                    if cursor >= pr.Origin.EndKey {
                        break;
                    }
                }
                if cursor < pr.Origin.EndKey {
                    has_gap = true;
                    incomplete.push(crate::stubs::backuppb::KeyRange {
                        StartKey: cursor,
                        EndKey: pr.Origin.EndKey.clone(),
                    });
                } else if !has_gap {
                    completed.push(idx);
                }
            }

            for idx in completed.into_iter().rev() {
                let pr = entries.remove(idx);
                let segments = pr.Res.ranges.lock().unwrap();
                let mut files = Vec::new();
                let (mut crc, mut kvs, mut bytes) = (0, 0, 0);
                for (_, _, range_files) in segments.iter() {
                    for file in range_files {
                        crc ^= file.crc64xor;
                        kvs += file.total_kvs;
                        bytes += file.total_bytes;
                    }
                    files.extend(range_files.clone());
                }
                if let Some(writer) = &self.meta_writer {
                    writer.Send(MetaPayload::Files(files), AppendDataFile)?;
                }
                self.UpdateChecksum(pr.Res.PhysicalID, crc, kvs, bytes);
                if let Some(cb) = self.callback.lock().unwrap().as_ref() {
                    cb();
                }
            }
            Ok(incomplete)
        }

        /// 在未完成区间中查找完全包含 [start,end) 的项。
        /// 若 start 已落入某个 Origin 但 end 越界，与 Go 一样返回错误。
        pub fn FindContained(
            &self,
            start: &[u8],
            end: &[u8],
        ) -> Result<Option<std::sync::Arc<ProgressRange>>> {
            let ranges = self.ranges.lock().unwrap();
            for pr in ranges.iter() {
                let r = &pr.Origin;
                if start >= r.StartKey.as_slice() && start < r.EndKey.as_slice() {
                    if end > r.EndKey.as_slice() {
                        return Err(Error::new(format!(
                            "The given region is not contained in the found progress range. The region start key is {:?}; The progress range start key is {:?}, end key is {:?}.",
                            start, r.StartKey, r.EndKey
                        )));
                    }
                    return Ok(Some(std::sync::Arc::new(pr.clone())));
                }
            }
            let _ = start;
            let _ = end;
            Ok(None)
        }

        /// 按 PhysicalID 累加/覆盖 checksum 统计。
        pub fn UpdateChecksum(&self, physical_id: i64, crc: u64, kvs: u64, bytes: u64) {
            if self.skip_checksum {
                return;
            }
            self.checksum.lock().unwrap().insert(
                physical_id,
                ChecksumStats {
                    Crc64Xor: crc,
                    TotalKvs: kvs,
                    TotalBytes: bytes,
                },
            );
        }

        /// 未完成区间个数。
        pub fn Len(&self) -> usize {
            self.ranges.lock().unwrap().len()
        }

        /// 返回 physical_id → ChecksumStats 快照。
        pub fn GetChecksumMap(&self) -> HashMap<i64, ChecksumStats> {
            self.checksum.lock().unwrap().clone()
        }

        /// Mark all incomplete ranges finished (used by tests / successful backup path).
        /// 清空未完成区间并触发完成回调（成功备份/测试路径）。
        pub fn clear_incomplete(&self) {
            self.ranges.lock().unwrap().clear();
            if let Some(cb) = self.callback.lock().unwrap().as_ref() {
                cb();
            }
        }

        /// 测试注入：直接覆盖未完成区间列表。
        pub fn set_incomplete(&self, ranges: Vec<KeyRange>) {
            *self.ranges.lock().unwrap() = ranges
                .into_iter()
                .map(|Origin| ProgressRange {
                    Res: RangeTree::default(),
                    Origin,
                })
                .collect();
        }
    }

    /// 构造 ProgressRangeTree；可挂 MetaWriter 并选择跳过 checksum。
    pub fn NewProgressRangeTree(
        meta: Option<std::sync::Arc<dyn MetaWriter>>,
        skip_checksum: bool,
    ) -> ProgressRangeTree {
        ProgressRangeTree::new(meta, skip_checksum)
    }

    /// 按 physical_id 新建空 RangeTree；FreeListG 参数忽略。
    pub fn NewRangeTreeWithFreeListG(_physical_id: i64, _fl: &FreeListG) -> RangeTree {
        RangeTree {
            PhysicalID: _physical_id,
            ranges: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[derive(Default)]
    /// `FreeListG` 本地精简结构，字段对齐 backup 调用子集。
    pub struct FreeListG;
    impl FreeListG {
        /// 空 FreeList 占位；容量参数忽略。
        pub fn new(_cap: usize) -> Self {
            Self
        }
    }
}

/// GC safepoint 管理器接口与检查；MemGCManager 供单测注入 safepoint。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod gc {
    use crate::stubs::{Context, Error, Result};
    use std::sync::atomic::{AtomicU64, Ordering};

    /// 与 Go `DefaultBRGCSafePointTTL` 对齐的本地常量。
    pub const DefaultBRGCSafePointTTL: i64 = 120;

    /// `Manager` 抽象边界；实现由测试或上层注入。
    pub trait Manager: Send + Sync {
        fn GetGCSafePoint(&self, _ctx: &Context) -> Result<u64>;
    }

    #[derive(Default)]
    /// `MemGCManager` 本地精简结构，字段对齐 backup 调用子集。
    pub struct MemGCManager {
        safe_point: AtomicU64,
    }

    impl MemGCManager {
        /// 以初始 safepoint 构造内存 GC 管理器。
        pub fn new(sp: u64) -> Self {
            Self {
                safe_point: AtomicU64::new(sp),
            }
        }
    }

    impl Manager for MemGCManager {
        fn GetGCSafePoint(&self, _ctx: &Context) -> Result<u64> {
            Ok(self.safe_point.load(Ordering::SeqCst))
        }
    }

    /// `CheckGCSafePoint` 判定辅助，对齐 Go utils。
    pub fn CheckGCSafePoint(ctx: &Context, mgr: &dyn Manager, backupTS: u64) -> Result<()> {
        let sp = mgr.GetGCSafePoint(ctx)?;
        if backupTS < sp {
            return Err(Error::new(format!(
                "backupTS {backupTS} is earlier than GCSafePoint {sp}"
            )));
        }
        Ok(())
    }

    static SAFE_POINT_SEQ: AtomicU64 = AtomicU64::new(1);

    /// 生成递增 safepoint 服务 ID：`br-<seq>`。
    pub fn MakeSafePointID() -> String {
        format!("br-{}", SAFE_POINT_SEQ.fetch_add(1, Ordering::SeqCst))
    }
}

/// TiDB Glue/Session 抽象，UseOneShotSession 由上层注入真实或 mock 实现。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod glue {
    use crate::stubs::{Result, Storage};
    use std::sync::atomic::{AtomicI64, Ordering};

    /// `Progress` 抽象边界；实现由测试或上层注入。
    pub trait Progress: Send + Sync {
        fn Inc(&self);
    }

    #[derive(Default)]
    /// `AtomicProgress` 本地精简结构：含 n；非完整上游类型。
    pub struct AtomicProgress {
        pub n: AtomicI64,
    }

    impl Progress for AtomicProgress {
        fn Inc(&self) {
            self.n.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Glue Session 边界。
    pub trait Session: Send {
        fn GetSessionCtx(&self) -> &dyn SessionCtx;
    }

    /// `SessionCtx` 抽象边界；实现由测试或上层注入。
    pub trait SessionCtx: Send {}

    /// TiDB Glue：UseOneShotSession 等。
    pub trait Glue: Send + Sync {
        fn UseOneShotSession(
            &self,
            _store: &dyn Storage,
            _needDomain: bool,
            f: &mut dyn FnMut(&dyn Session) -> Result<()>,
        ) -> Result<()>;
    }
}

/// 备份工具函数：系统库判定、临时库名、错误策略、WithRetry/退避与文件 checksum 汇总。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod utils {
    use crate::stubs::backuppb::{self, File};
    use crate::stubs::metapb::Store;
    use crate::stubs::model::{CIStr, TableInfo};
    use crate::stubs::txnlock::{Backoffer, DummyBackoffer};
    use crate::stubs::{Context, Error, Result};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    /// Label Rule 批处理大小（64）。
    pub const LabelRuleBatchSize: usize = 64;

    /// 是否系统库（mysql/sys/metrics_schema）。
    pub fn IsSysDB(name_l: &str) -> bool {
        matches!(name_l, "mysql" | "sys" | "metrics_schema")
    }

    /// 生成 BR 临时库名 `__TiDB_BR_Temporary_<name>`。
    pub fn TemporaryDBName(name: &str) -> CIStr {
        CIStr::new(format!("__TiDB_BR_Temporary_{name}"))
    }

    /// 是否信息类模板库（information/performance/metrics_schema）。
    pub fn IsTemplateSysDB(name: &CIStr) -> bool {
        name.L == "information_schema"
            || name.L == "performance_schema"
            || name.L == "metrics_schema"
    }

    /// 表是否需要分配 AutoID（有自增或非视图）。
    pub fn NeedAutoID(t: &TableInfo) -> bool {
        t.HasAutoInc || !t.IsView()
    }

    /// Up(0) 与 Offline(1) 都可备份；只有 Tombstone/其他状态不可用。
    pub fn CheckStoreLiveness(store: &Store) -> Result<()> {
        if store.State != 0 && store.State != 1 {
            return Err(Error::new(format!("store {} not alive", store.Id)));
        }
        Ok(())
    }

    #[derive(Clone, Debug)]
    /// 错误处理上下文：名称与允许的最大错误数。
    pub struct ErrorContext {
        pub name: String,
        pub max: usize,
        encounter_times: Arc<Mutex<HashMap<u64, usize>>>,
    }

    impl ErrorContext {
        /// 构造错误上下文：资源名与最大错误计数。
        pub fn new(name: &str, max: usize) -> Self {
            Self {
                name: name.to_string(),
                max,
                encounter_times: Arc::new(Mutex::new(HashMap::new())),
            }
        }
    }

    /// 构造 ErrorContext(name, max)。
    pub fn NewErrorContext(name: &str, max: usize) -> ErrorContext {
        ErrorContext::new(name, max)
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    /// 备份错误策略：GiveUp / Retry / Ignore。
    pub enum Strategy {
        GiveUp,
        Retry,
        Ignore,
    }

    #[derive(Clone, Debug)]
    /// 备份错误处理结果：策略与原因。
    pub struct HandleResult {
        pub Strategy: Strategy,
        pub Reason: String,
    }

    /// 按 Go `HandleBackupError` 的结构化错误与 unknown 错误配额分类。
    pub fn HandleBackupError(
        err: &backuppb::Error,
        storeID: u64,
        ctx: &ErrorContext,
    ) -> HandleResult {
        match &err.Detail {
            backuppb::ErrorDetail::KvError { .. } => {
                return HandleResult {
                    Strategy: Strategy::Retry,
                    Reason: "retry on kv error".into(),
                };
            }
            backuppb::ErrorDetail::RegionError { .. } => {
                return HandleResult {
                    Strategy: Strategy::Retry,
                    Reason: "retry on region error".into(),
                };
            }
            backuppb::ErrorDetail::None => {}
        }

        let msg_lower = err.Msg.to_ascii_lowercase();
        if (msg_lower.contains("io") && msg_lower.contains("notfound"))
            || msg_lower.contains("permissiondenied")
            || msg_lower.contains("permission denied")
            || msg_lower.contains("credential info not found")
            || msg_lower.contains("context canceled")
            || msg_lower.contains("giveup")
        {
            return HandleResult {
                Strategy: Strategy::GiveUp,
                Reason: err.Msg.clone(),
            };
        }

        const RETRYABLE_MESSAGES: &[&str] = &[
            "server closed",
            "connection refused",
            "connection reset by peer",
            "channel closed",
            "error trying to connect",
            "connection closed before message completed",
            "body write aborted",
            "error during dispatch",
            "put object timeout",
            "timeout after",
            "internalerror",
            "not read from or written to within the timeout period",
            "<code>requesttimeout</code>",
            "<code>invalidpart</code>",
            "end of file before message length reached",
        ];
        if RETRYABLE_MESSAGES
            .iter()
            .any(|message| msg_lower.contains(message))
        {
            return HandleResult {
                Strategy: Strategy::Retry,
                Reason: "retryable storage error".into(),
            };
        }

        let mut encounter_times = ctx.encounter_times.lock().unwrap();
        let encounters = encounter_times.entry(storeID).or_insert(0);
        *encounters += 1;
        if *encounters <= ctx.max {
            return HandleResult {
                Strategy: Strategy::Retry,
                Reason: "unknown error, retry it for a few times".into(),
            };
        }
        HandleResult {
            Strategy: Strategy::GiveUp,
            Reason: "unknown error, retried too many times, give up".into(),
        }
    }

    static SKIP_BACKOFF: AtomicBool = AtomicBool::new(false);

    /// 测试注入：设置 `skip_backoff_sleep` 开关/值。
    pub fn set_skip_backoff_sleep(skip: bool) {
        SKIP_BACKOFF.store(skip, Ordering::SeqCst);
    }

    /// 最多重试 3 次；可被 skip_backoff 跳过 sleep，尊重 Context 取消。
    pub fn WithRetry<F>(ctx: &Context, mut f: F, _strategy: BackoffStrategy) -> Result<()>
    where
        F: FnMut() -> Result<()>,
    {
        let mut last = None;
        for _ in 0..3 {
            if ctx.Done() {
                return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
            }
            match f() {
                Ok(()) => return Ok(()),
                Err(e) => {
                    last = Some(e);
                    if !SKIP_BACKOFF.load(Ordering::SeqCst) {
                        thread::sleep(Duration::from_millis(1));
                    }
                }
            }
        }
        Err(last.unwrap_or_else(|| Error::new("retry exhausted")))
    }

    #[derive(Default)]
    /// 退避策略占位类型。
    pub struct BackoffStrategy;

    /// 返回默认 BackoffStrategy 占位实例。
    pub fn NewBackupSSTBackoffStrategy() -> BackoffStrategy {
        BackoffStrategy
    }

    /// 返回包装 DummyBackoffer 的适配器，忽略 maxSleep。
    pub fn AdaptTiKVBackoffer(
        _ctx: &Context,
        _maxSleepMs: u64,
        _err: Error,
    ) -> TiKVBackofferAdapter {
        TiKVBackofferAdapter
    }

    /// 包装 DummyBackoffer 的适配器。
    pub struct TiKVBackofferAdapter;
    impl TiKVBackofferAdapter {
        /// 暴露内部 DummyBackoffer。
        pub fn Inner(&self) -> &dyn Backoffer {
            &DummyBackoffer
        }
    }

    /// 对文件列表做 crc64xor 异或与 kvs/bytes 求和。
    pub fn SummaryFiles(files: &[File]) -> (u64, u64, u64) {
        let mut crc = 0u64;
        let mut kvs = 0u64;
        let mut bytes = 0u64;
        for f in files {
            crc ^= f.crc64xor;
            kvs += f.total_kvs;
            bytes += f.total_bytes;
        }
        (crc, kvs, bytes)
    }
}

/// PD store 拓扑观察：Step 对比新增、断连与可观测重启并触发回调。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod storewatch {
    use crate::stubs::metapb::Store;
    use crate::stubs::{Context, PdClient, Result};

    /// storewatch 三类回调（reboot/disconnect/new）。
    pub struct Callbacks {
        pub on_reboot: Option<Box<dyn Fn(&Store) + Send + Sync>>,
        pub on_disconnect: Option<Box<dyn Fn(&Store) + Send + Sync>>,
        pub on_new: Option<Box<dyn Fn(&Store) + Send + Sync>>,
    }

    /// 组装 storewatch Callbacks 三元组。
    pub fn MakeCallback(
        on_reboot: Option<Box<dyn Fn(&Store) + Send + Sync>>,
        on_disconnect: Option<Box<dyn Fn(&Store) + Send + Sync>>,
        on_new: Option<Box<dyn Fn(&Store) + Send + Sync>>,
    ) -> Callbacks {
        Callbacks {
            on_reboot,
            on_disconnect,
            on_new,
        }
    }

    /// 装箱 reboot 回调供 MakeCallback 使用。
    pub fn WithOnReboot<F: Fn(&Store) + Send + Sync + 'static>(
        f: F,
    ) -> Option<Box<dyn Fn(&Store) + Send + Sync>> {
        Some(Box::new(f))
    }
    /// 装箱 disconnect 回调供 MakeCallback 使用。
    pub fn WithOnDisconnect<F: Fn(&Store) + Send + Sync + 'static>(
        f: F,
    ) -> Option<Box<dyn Fn(&Store) + Send + Sync>> {
        Some(Box::new(f))
    }
    /// 装箱新 store 注册回调供 MakeCallback 使用。
    pub fn WithOnNewStoreRegistered<F: Fn(&Store) + Send + Sync + 'static>(
        f: F,
    ) -> Option<Box<dyn Fn(&Store) + Send + Sync>> {
        Some(Box::new(f))
    }

    /// 基于 PD GetAllStores 的轮询观察者。
    pub struct Watcher {
        pd: std::sync::Arc<dyn PdClient>,
        #[allow(dead_code)]
        cb: Callbacks,
        seen: std::sync::Mutex<std::collections::HashMap<u64, Store>>,
    }

    impl Watcher {
        /// 持有 PD 客户端与回调，seen 表跟踪已观察 store。
        pub fn new(pd: std::sync::Arc<dyn PdClient>, cb: Callbacks) -> Self {
            Self {
                pd,
                cb,
                seen: std::sync::Mutex::new(std::collections::HashMap::new()),
            }
        }
        /// 拉取全量 store，按上一轮快照触发 new/disconnect/reboot 回调。
        pub fn Step(&self, ctx: &Context) -> Result<()> {
            let stores = self.pd.GetAllStores(ctx)?;
            let mut seen = self.seen.lock().unwrap();
            let mut recorded = std::collections::HashSet::new();
            for s in stores {
                recorded.insert(s.Id);
                match seen.get(&s.Id) {
                    None => {
                        if let Some(cb) = &self.cb.on_new {
                            cb(&s);
                        }
                    }
                    Some(last) => {
                        if last.State == 0
                            && s.State == 1
                            && let Some(cb) = &self.cb.on_disconnect
                        {
                            cb(&s);
                        }
                        // This local metapb boundary has no StartTimestamp. A transition back
                        // to Up, or an address change for the same ID, is its reboot signal.
                        if (last.State != 0 && s.State == 0 || last.Address != s.Address)
                            && let Some(cb) = &self.cb.on_reboot
                        {
                            cb(&s);
                        }
                    }
                }
                seen.insert(s.Id, s);
            }
            seen.retain(|id, _| recorded.contains(id));
            Ok(())
        }
    }

    /// 构造本地实例，对齐 Go 同名 New 辅助。
    pub fn New(pd: std::sync::Arc<dyn PdClient>, cb: Callbacks) -> Watcher {
        Watcher::new(pd, cb)
    }
}

/// 耗时汇总收集器，测试可 take_durations 取走记录。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod summary {
    use std::sync::Mutex;
    use std::time::Duration;

    static DURATIONS: Mutex<Vec<(String, Duration)>> = Mutex::new(Vec::new());
    static ADJUST: Mutex<Option<Duration>> = Mutex::new(None);

    /// 记录命名耗时，供测试 take_durations 取走。
    pub fn CollectDuration(name: &str, d: Duration) {
        DURATIONS.lock().unwrap().push((name.to_string(), d));
    }

    /// 记录“起始时间前移”调整量（测试可观测）。
    pub fn AdjustStartTimeToEarlierTime(d: Duration) {
        *ADJUST.lock().unwrap() = Some(d);
    }

    /// 取走并清空已收集的耗时列表。
    pub fn take_durations() -> Vec<(String, Duration)> {
        std::mem::take(&mut *DURATIONS.lock().unwrap())
    }
}

/// 备份支持的 TableInfo 版本常量。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod version {
    /// 与 Go `CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION` 对齐的本地常量。
    pub const CURRENT_BACKUP_SUPPORT_TABLE_INFO_VERSION: i64 = 5;
}

/// 库表过滤接口：AllFilter 全放行，AllowList 按名单匹配。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod filter {
    /// `Filter` 抽象边界；实现由测试或上层注入。
    pub trait Filter: Send + Sync {
        fn MatchSchema(&self, schema: &str) -> bool;
        fn MatchTable(&self, schema: &str, table: &str) -> bool;
    }

    #[derive(Default)]
    /// `AllFilter` 本地精简结构，字段对齐 backup 调用子集。
    pub struct AllFilter;
    impl Filter for AllFilter {
        fn MatchSchema(&self, _schema: &str) -> bool {
            true
        }
        fn MatchTable(&self, _schema: &str, _table: &str) -> bool {
            true
        }
    }

    /// `AllowList` 本地精简结构：含 schemas/tables；非完整上游类型。
    pub struct AllowList {
        pub schemas: Vec<String>,
        pub tables: Vec<(String, String)>,
    }

    impl Filter for AllowList {
        fn MatchSchema(&self, schema: &str) -> bool {
            self.schemas.is_empty() || self.schemas.iter().any(|s| s.eq_ignore_ascii_case(schema))
        }
        fn MatchTable(&self, schema: &str, table: &str) -> bool {
            if self.tables.is_empty() {
                return true;
            }
            self.tables
                .iter()
                .any(|(s, t)| s.eq_ignore_ascii_case(schema) && t.eq_ignore_ascii_case(table))
        }
    }
}

/// 元数据辅助：判定 information_schema/performance_schema 等内存库。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod metadef {
    /// `IsMemDB` 判定辅助，对齐 Go utils。
    pub fn IsMemDB(name_l: &str) -> bool {
        name_l == "information_schema" || name_l == "performance_schema"
    }
}

/// meta.Reader 精简：列举策略/库表与 AutoID；MemMeta 为内存实现。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod meta {
    use crate::stubs::model::{DBInfo, Job, PolicyInfo, TableInfo};
    use crate::stubs::{Result, Snapshot};
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// `Reader` 抽象边界；实现由测试或上层注入。
    pub trait Reader: Send + Sync {
        fn ListPolicies(&self) -> Result<Vec<PolicyInfo>>;
        fn ListDatabases(&self) -> Result<Vec<DBInfo>>;
        fn IterTables(
            &self,
            db_id: i64,
            f: &mut dyn FnMut(&mut TableInfo) -> Result<()>,
        ) -> Result<()>;
        fn GetAutoIDAccessors(&self, db_id: i64, table_id: i64) -> AutoIDAccessors;
        fn GetSchemaVersionWithNonEmptyDiff(&self) -> Result<i64>;
        fn HistoryDDLJobs(&self) -> Result<Vec<Job>> {
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    /// `MemMeta` 本地精简结构：含 policies/dbs/tables/schema_version；非完整上游类型。
    pub struct MemMeta {
        pub policies: Mutex<Vec<PolicyInfo>>,
        pub dbs: Mutex<Vec<DBInfo>>,
        pub tables: Mutex<HashMap<i64, Vec<TableInfo>>>,
        pub schema_version: Mutex<i64>,
        pub auto_ids: Mutex<HashMap<(i64, i64), i64>>,
        pub history_jobs: Mutex<Vec<Job>>,
    }

    impl Reader for MemMeta {
        fn ListPolicies(&self) -> Result<Vec<PolicyInfo>> {
            Ok(self.policies.lock().unwrap().clone())
        }
        fn ListDatabases(&self) -> Result<Vec<DBInfo>> {
            Ok(self.dbs.lock().unwrap().clone())
        }
        fn IterTables(
            &self,
            db_id: i64,
            f: &mut dyn FnMut(&mut TableInfo) -> Result<()>,
        ) -> Result<()> {
            let mut tables = self
                .tables
                .lock()
                .unwrap()
                .get(&db_id)
                .cloned()
                .unwrap_or_default();
            for t in &mut tables {
                f(t)?;
            }
            Ok(())
        }
        fn GetAutoIDAccessors(&self, db_id: i64, table_id: i64) -> AutoIDAccessors {
            let v = *self
                .auto_ids
                .lock()
                .unwrap()
                .get(&(db_id, table_id))
                .unwrap_or(&0);
            AutoIDAccessors { value: v }
        }
        fn GetSchemaVersionWithNonEmptyDiff(&self) -> Result<i64> {
            Ok(*self.schema_version.lock().unwrap())
        }
        fn HistoryDDLJobs(&self) -> Result<Vec<Job>> {
            Ok(self.history_jobs.lock().unwrap().clone())
        }
    }

    /// 忽略 snapshot，返回默认空 MemMeta。
    pub fn NewReader(_snap: Box<dyn Snapshot>) -> MemMeta {
        MemMeta::default()
    }

    /// Hook used by production BuildBackup* to obtain a reader from storage snapshot.
    /// `MetaFromStorage` 抽象边界；实现由测试或上层注入。
    pub trait MetaFromStorage: Send + Sync {
        fn reader_at(&self, backup_ts: u64) -> Box<dyn Reader>;
    }

    /// `AutoIDAccessors` 本地精简结构，字段对齐 backup 调用子集。
    pub struct AutoIDAccessors {
        value: i64,
    }

    impl AutoIDAccessors {
        /// 返回序列值 getter（本桩恒为内部 value）。
        pub fn SequenceValue(&self) -> IDGetter {
            IDGetter { v: self.value }
        }
        /// 返回自增 ID getter；版本参数忽略。
        pub fn IncrementID(&self, _ver: i64) -> IDGetter {
            IDGetter { v: self.value }
        }
        /// 返回 RowID getter。
        pub fn RowID(&self) -> IDGetter {
            IDGetter { v: self.value }
        }
        /// 返回 RandomID getter。
        pub fn RandomID(&self) -> IDGetter {
            IDGetter { v: self.value }
        }
    }

    /// 延迟读取 AutoID 的 getter 包装。
    pub struct IDGetter {
        v: i64,
    }

    impl IDGetter {
        /// 返回内部 i64 值。
        pub fn Get(&self) -> Result<i64> {
            Ok(self.v)
        }
    }
}

/// BuildTableRanges 用表前缀编码构造整表 key range，非完整 distsql。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod distsql {
    use crate::stubs::Result;
    use crate::stubs::model::TableInfo;
    use crate::stubs::rtree::KeyRange;

    /// 用 `t`+tableID 前缀构造整表 [start, nextTable) 单区间。
    pub fn BuildTableRanges(table: &TableInfo) -> Result<Vec<KeyRange>> {
        let start = encode_table_prefix(table.ID);
        let mut end = encode_table_prefix(table.ID);
        // end = start of next table id
        if let Some(last) = end.last_mut() {
            *last = last.wrapping_add(1);
        }
        Ok(vec![KeyRange {
            StartKey: start,
            EndKey: end,
        }])
    }

    fn encode_table_prefix(id: i64) -> Vec<u8> {
        let mut v = vec![0x74]; // 't'
        v.extend_from_slice(&id.to_be_bytes());
        v
    }
}

/// 从 key 解码 table ID（t + 8 字节大端），非法 key 返回 0。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod tablecodec {
    /// 解码 `t`+8 字节大端 table ID；非法 key 返回 0。
    pub fn DecodeTableID(key: &[u8]) -> i64 {
        if key.len() >= 9 && key[0] == 0x74 {
            let mut buf = [0u8; 8];
            buf.copy_from_slice(&key[1..9]);
            i64::from_be_bytes(buf)
        } else {
            0
        }
    }
}

/// DDL history 读取边界：会话活跃任务与 meta 历史分页分别提供。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod ddl {
    use crate::stubs::glue::SessionCtx;
    use crate::stubs::meta::Reader;
    use crate::stubs::model::Job;
    use crate::stubs::{Context, Result};

    /// 与 Go `DefNumHistoryJobs` 对齐的本地常量。
    pub const DefNumHistoryJobs: usize = 10;

    /// 默认会话无活跃 DDL job；历史 job 由 meta reader 提供。
    pub fn GetAllDDLJobs(_ctx: &Context, _se: &dyn SessionCtx) -> Result<Vec<Job>> {
        Ok(Vec::new())
    }

    /// `HistoryJobsIterator` 抽象边界；实现由测试或上层注入。
    pub trait HistoryJobsIterator: Send {
        fn GetLastJobs(&mut self, num: usize, cache: Vec<Job>) -> Result<Vec<Job>>;
    }

    /// 按新到旧顺序分页返回 DDL 历史。
    pub struct VecHistory {
        jobs: Vec<Job>,
        offset: usize,
    }
    impl HistoryJobsIterator for VecHistory {
        fn GetLastJobs(&mut self, num: usize, _cache: Vec<Job>) -> Result<Vec<Job>> {
            if self.offset >= self.jobs.len() {
                return Ok(Vec::new());
            }
            let end = (self.offset + num).min(self.jobs.len());
            let page = self.jobs[self.offset..end].to_vec();
            self.offset = end;
            Ok(page)
        }
    }

    /// 从 meta reader 取得历史 DDL 快照并构造分页迭代器。
    pub fn GetLastHistoryDDLJobsIterator(
        meta: &dyn Reader,
    ) -> Result<Box<dyn HistoryJobsIterator>> {
        Ok(Box::new(VecHistory {
            jobs: meta.HistoryDDLJobs()?,
            offset: 0,
        }))
    }
}

/// 经 PD 拉 TiKV store 列表；StoreBehavior 占位对齐 Go util.SkipTiFlash 等。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod conn {
    use crate::stubs::metapb::Store;
    use crate::stubs::{Context, PdClient, Result};
    use std::sync::Arc;

    /// 直接委托 pd.GetAllStores；忽略 StoreBehavior 与重试。
    pub fn GetAllTiKVStoresWithRetry(
        ctx: &Context,
        pd: &dyn PdClient,
        _store_behavior: StoreBehavior,
    ) -> Result<Vec<Store>> {
        pd.GetAllStores(ctx)
    }

    #[derive(Clone, Copy)]
    /// 拉 store 列表时的过滤行为占位（如 SkipTiFlash）。
    pub struct StoreBehavior;

    /// conn::util 子模块：导出 SkipTiFlash 等 StoreBehavior 常量别名。
    /// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
    pub mod util {
        /// 拉取 store 时跳过 TiFlash 的行为常量别名。
        pub const SkipTiFlash: super::StoreBehavior = super::StoreBehavior;
    }
}

/// Placement/Label Rule ID 生成，格式 schema/db/table[/partition]。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod label {
    use crate::stubs::Codec;

    #[derive(Clone, Debug, Default)]
    /// Placement Label 键值。
    pub struct Label {
        pub Key: String,
        pub Value: String,
    }

    #[derive(Clone, Debug, Default)]
    /// 一组 Labels 构成的规则。
    pub struct Rule {
        pub Labels: Vec<Label>,
    }

    /// 生成 schema/db/table[/partition] 形式的 Label Rule ID。
    pub fn NewRuleID(_codec: &dyn Codec, db: &str, table: &str, partition: &str) -> String {
        if partition.is_empty() {
            format!("schema/{db}/{table}")
        } else {
            format!("schema/{db}/{table}/{partition}")
        }
    }
}

/// 标签规则查询：由测试 set_label_rules 注入全局表，生产按 ID 过滤返回。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod infosync {
    use crate::stubs::label::Rule;
    use crate::stubs::{Context, Result};
    use std::collections::HashMap;
    use std::sync::Mutex;

    static RULES: Mutex<Option<HashMap<String, Rule>>> = Mutex::new(None);

    /// 测试注入：设置 `label_rules` 开关/值。
    pub fn set_label_rules(rules: HashMap<String, Rule>) {
        *RULES.lock().unwrap() = Some(rules);
    }

    /// 清空测试注入的全局 label rules。
    pub fn clear_label_rules() {
        *RULES.lock().unwrap() = None;
    }

    /// 按 ID 从测试注入表过滤返回规则；未注入则空 map。
    pub fn GetLabelRules(_ctx: &Context, ids: &[String]) -> Result<HashMap<String, Rule>> {
        let guard = RULES.lock().unwrap();
        let mut out = HashMap::new();
        if let Some(all) = guard.as_ref() {
            for id in ids {
                if let Some(r) = all.get(id) {
                    out.insert(id.clone(), r.clone());
                }
            }
        }
        Ok(out)
    }
}

/// 统计信息 Persist 接口与 MemStatsHandle；将 db.table 名写入回调载荷。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod statistics {
    use crate::stubs::metautil::StatsWriter;
    use crate::stubs::model::{CIStr, TableInfo};
    use crate::stubs::{Context, Result};

    #[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
    /// 统计 JSON 表名占位。
    pub struct JSONTable {
        pub name: String,
    }

    /// 统计 Persist 接口；快照路径写 backup_stats 回调。
    pub trait Handle: Send + Sync {
        fn PersistStatsBySnapshot(
            &self,
            _ctx: &Context,
            db: &str,
            table: &TableInfo,
            _backupTS: u64,
            backup_stats: &dyn Fn(&str, &TableInfo, &[u8]) -> Result<()>,
        ) -> Result<()>;
    }

    #[derive(Default)]
    /// 内存统计句柄：把 db.table 写入 backup_stats 回调。
    pub struct MemStatsHandle;
    impl Handle for MemStatsHandle {
        fn PersistStatsBySnapshot(
            &self,
            _ctx: &Context,
            db: &str,
            table: &TableInfo,
            _backupTS: u64,
            backup_stats: &dyn Fn(&str, &TableInfo, &[u8]) -> Result<()>,
        ) -> Result<()> {
            let payload = format!("{db}.{}", table.Name.O).into_bytes();
            backup_stats(db, table, &payload)
        }
    }

    // silence unused import
    /// 再导出 CIStr，消除未使用导入告警。
    pub type _CIStr = CIStr;
}

/// 表级 checksum Executor 构建器；可通过 inject 注入响应，默认用 table.ID 填充。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod checksum {
    use crate::stubs::model::TableInfo;
    use crate::stubs::{Context, KvClient, Result};
    use std::cell::RefCell;

    #[derive(Clone, Debug, Default)]
    /// 表级 checksum 执行结果三元组。
    pub struct ChecksumResponse {
        pub Checksum: u64,
        pub TotalKvs: u64,
        pub TotalBytes: u64,
    }

    /// 持有预设响应的 checksum 执行器。
    pub struct Executor {
        resp: ChecksumResponse,
    }

    impl Executor {
        /// 调用进度回调后返回预设 ChecksumResponse。
        pub fn Execute(
            &self,
            _ctx: &Context,
            _client: &dyn KvClient,
            update: impl Fn(),
        ) -> Result<ChecksumResponse> {
            update();
            Ok(self.resp.clone())
        }
    }

    /// 构建 Executor；支持注入响应与链式忽略项。
    pub struct ExecutorBuilder {
        table: TableInfo,
        resp: ChecksumResponse,
    }

    impl ExecutorBuilder {
        /// 链式配置请求来源类型；本桩忽略参数。
        pub fn SetExplicitRequestSourceType(mut self, _t: &str) -> Self {
            self
        }
        /// 链式配置并发度；本桩忽略参数。
        pub fn SetConcurrency(self, _c: u32) -> Self {
            self
        }
        /// 用已准备的响应构造 Executor。
        pub fn Build(self) -> Result<Executor> {
            Ok(Executor { resp: self.resp })
        }
        /// 覆盖默认/注入的 ChecksumResponse。
        pub fn with_response(mut self, resp: ChecksumResponse) -> Self {
            self.resp = resp;
            self
        }
    }

    thread_local! {
        static NEXT_RESP: RefCell<Option<ChecksumResponse>> = const { RefCell::new(None) };
    }

    /// 线程局部注入下一次 NewExecutorBuilder 使用的响应。
    pub fn inject_checksum_response(resp: ChecksumResponse) {
        NEXT_RESP.with(|next| *next.borrow_mut() = Some(resp));
    }

    /// 优先消费线程局部注入响应，否则用 table.ID 填默认值。
    pub fn NewExecutorBuilder(table: TableInfo, _backupTS: u64) -> ExecutorBuilder {
        let resp = NEXT_RESP
            .with(|next| next.borrow_mut().take())
            .unwrap_or(ChecksumResponse {
                Checksum: table.ID as u64,
                TotalKvs: 1,
                TotalBytes: 1,
            });
        ExecutorBuilder { table, resp }
    }
}

/// KV 请求来源类型常量，标记流量来自 BR。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod kvutil {
    /// KV 请求来源标记为 BR。
    pub const ExplicitTypeBR: &str = "br";
}

/// BR 领域错误构造函数，消息与 Go berrors 文案对齐。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod berrors {
    use crate::stubs::Error;

    /// 构造 "backup checksum mismatch"，对齐 berrors。
    pub fn ErrBackupChecksumMismatch() -> Error {
        Error::new("backup checksum mismatch")
    }

    /// 构造 "invalid argument"。
    pub fn ErrInvalidArgument() -> Error {
        Error::new("invalid argument")
    }

    /// 构造 "kv storage error"。
    pub fn ErrKVStorage() -> Error {
        Error::new("kv storage error")
    }

    /// 构造 "unknown"。
    pub fn ErrUnknown() -> Error {
        Error::new("unknown")
    }
}

/// Failpoint-style injectors used by store/client paths.
/// 进程内 failpoint 注入器：set/take 成对，供 store/client 测试路径读取。
/// 边界：仅解耦 backup 编译/单测；缺能力见函数体（常为空操作）。
pub mod failpoint {
    use std::sync::Mutex;

    static HINT_BACKUP_START: Mutex<Option<String>> = Mutex::new(None);
    static RESET_RETRYABLE: Mutex<Option<String>> = Mutex::new(None);
    static RESET_NOT_RETRYABLE: Mutex<bool> = Mutex::new(false);
    static BACKUP_TIMEOUT_ERR: Mutex<Option<String>> = Mutex::new(None);
    static BACKUP_STORAGE_ERR: Mutex<Option<String>> = Mutex::new(None);
    static TIKV_RW_ERR: Mutex<Option<String>> = Mutex::new(None);
    static TIKV_REGION_ERR: Mutex<Option<String>> = Mutex::new(None);
    static STORE_CHANGE_TICK: Mutex<bool> = Mutex::new(false);

    /// 测试注入：设置 `hint_backup_start` 开关/值。
    pub fn set_hint_backup_start(v: Option<String>) {
        *HINT_BACKUP_START.lock().unwrap() = v;
    }
    /// 读取 failpoint `take_hint_backup_start` 的当前注入值。
    pub fn take_hint_backup_start() -> Option<String> {
        HINT_BACKUP_START.lock().unwrap().clone()
    }

    /// 测试注入：设置 `reset_retryable` 开关/值。
    pub fn set_reset_retryable(v: Option<String>) {
        *RESET_RETRYABLE.lock().unwrap() = v;
    }
    /// 读取 failpoint `take_reset_retryable` 的当前注入值。
    pub fn take_reset_retryable() -> Option<String> {
        RESET_RETRYABLE.lock().unwrap().clone()
    }

    /// 测试注入：设置 `reset_not_retryable` 开关/值。
    pub fn set_reset_not_retryable(v: bool) {
        *RESET_NOT_RETRYABLE.lock().unwrap() = v;
    }
    /// 读取 failpoint `take_reset_not_retryable` 的当前注入值。
    pub fn take_reset_not_retryable() -> bool {
        *RESET_NOT_RETRYABLE.lock().unwrap()
    }

    /// 测试注入：设置 `backup_timeout_error` 开关/值。
    pub fn set_backup_timeout_error(v: Option<String>) {
        *BACKUP_TIMEOUT_ERR.lock().unwrap() = v;
    }
    /// 读取 failpoint `take_backup_timeout_error` 的当前注入值。
    pub fn take_backup_timeout_error() -> Option<String> {
        BACKUP_TIMEOUT_ERR.lock().unwrap().take()
    }

    /// 测试注入：设置 `backup_storage_error` 开关/值。
    pub fn set_backup_storage_error(v: Option<String>) {
        *BACKUP_STORAGE_ERR.lock().unwrap() = v;
    }
    /// 读取 failpoint `take_backup_storage_error` 的当前注入值。
    pub fn take_backup_storage_error() -> Option<String> {
        BACKUP_STORAGE_ERR.lock().unwrap().take()
    }

    /// 测试注入：设置 `tikv_rw_error` 开关/值。
    pub fn set_tikv_rw_error(v: Option<String>) {
        *TIKV_RW_ERR.lock().unwrap() = v;
    }
    /// 读取 failpoint `take_tikv_rw_error` 的当前注入值。
    pub fn take_tikv_rw_error() -> Option<String> {
        TIKV_RW_ERR.lock().unwrap().take()
    }

    /// 测试注入：设置 `tikv_region_error` 开关/值。
    pub fn set_tikv_region_error(v: Option<String>) {
        *TIKV_REGION_ERR.lock().unwrap() = v;
    }
    /// 读取 failpoint `take_tikv_region_error` 的当前注入值。
    pub fn take_tikv_region_error() -> Option<String> {
        TIKV_REGION_ERR.lock().unwrap().take()
    }

    /// 测试注入：设置 `backup_store_change_tick` 开关/值。
    pub fn set_backup_store_change_tick(v: bool) {
        *STORE_CHANGE_TICK.lock().unwrap() = v;
    }
    /// 读取 failpoint `take_backup_store_change_tick` 的当前注入值。
    pub fn take_backup_store_change_tick() -> bool {
        *STORE_CHANGE_TICK.lock().unwrap()
    }
}

/// Simple worker pool approximating tidbutil.NewWorkerPool + ApplyOnErrorGroup.
/// 近似 tidbutil.WorkerPool：限制并发后 spawn，再 wait_jobs 聚合错误。
pub struct WorkerPool {
    concurrency: usize,
}

impl WorkerPool {
    /// concurrency 至少为 1；name 仅占位对齐 Go 构造签名。
    pub fn new(concurrency: u32, _name: &str) -> Self {
        Self {
            concurrency: concurrency.max(1) as usize,
        }
    }

    /// 超出并发上限时先 join 再 spawn，近似 WorkerPool。
    pub fn ApplyOnErrorGroup<F>(&self, jobs: &mut Vec<std::thread::JoinHandle<Result<()>>>, f: F)
    where
        F: FnOnce() -> Result<()> + Send + 'static,
    {
        // Keep a full slot as a chained job.  Joining and dropping the handle here
        // would lose its error before `wait_jobs`, unlike Go's errgroup.
        if jobs.len() >= self.concurrency {
            let previous = jobs
                .pop()
                .expect("a full worker pool must contain a pending job");
            jobs.push(std::thread::spawn(move || {
                let previous_result = match previous.join() {
                    Ok(result) => result,
                    Err(_) => Err(Error::new("worker panicked")),
                };
                let current_result = f();
                match previous_result {
                    Err(err) => Err(err),
                    Ok(()) => current_result,
                }
            }));
        } else {
            jobs.push(std::thread::spawn(f));
        }
    }
}

/// join 全部任务，返回首个错误；panic 映射为 worker panicked。
pub fn wait_jobs(jobs: Vec<std::thread::JoinHandle<Result<()>>>) -> Result<()> {
    let mut first = None;
    for h in jobs {
        match h.join() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first.is_none() {
                    first = Some(e);
                }
            }
            Err(_) => {
                if first.is_none() {
                    first = Some(Error::new("worker panicked"));
                }
            }
        }
    }
    match first {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// 当前 Unix 毫秒时间戳。
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

static SKIP_ROUND_SLEEP: AtomicBool = AtomicBool::new(false);

/// 测试开关：跳过备份轮次 sleep。
pub fn set_skip_round_sleep(skip: bool) {
    SKIP_ROUND_SLEEP.store(skip, Ordering::SeqCst);
}

/// 读取是否跳过轮次 sleep。
pub fn should_skip_round_sleep() -> bool {
    SKIP_ROUND_SLEEP.load(Ordering::SeqCst)
}

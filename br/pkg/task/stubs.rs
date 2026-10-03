// Copyright 2026 AsterSQL.
//! Local stand-ins for glue/PD/TiKV/storage/domain/flag boundaries (arm64-safe).
//! `br/pkg/task` 的本地桩模块：在 arm64/无 TiKV 环境下替代 glue、PD、存储、flag 等外部依赖。
//! 真实生产路径应接入 `br/pkg/glue`、`br/pkg/storage`、PD client 等；此处仅保证 task 层可编译与单测。
//! 被 80+ 个 br/cmd 与 br/pkg 模块引用，改动需保持公开 API 与 Go 命名风格一致。
//! 内存实现（Mem*）供单元测试注入，不模拟真实集群 I/O 或 gRPC 语义。

//! task 桩模块：替身外部存储、PD 与加密依赖，支撑 backup/restore 任务单测。
//! 默认行为偏“可断言的最小实现”，复杂能力留给专用 mock。
//! 错误桩用于验证任务层重试、中止与进度汇报。
//! 保持与 Go task 测试替身同名语义，降低对照成本。
//! 注释只描述桩边界，不把未接线逻辑写成已完成功能。

//! 符号索引补充 1：公开 API 的约束优先于内部实现细节。
//! 数据流补充 2：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 3：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 4：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 5：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 6：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 桩补充 7：Mem* 仅服务单测注入，不模拟真实 gRPC/集群调度。
//! 命名补充 8：PascalCase 方法名保留以对齐 Go 调用点迁移成本。
//! 符号索引补充 9：公开 API 的约束优先于内部实现细节。
//! 数据流补充 10：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 11：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 12：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 13：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 14：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 桩补充 15：Mem* 仅服务单测注入，不模拟真实 gRPC/集群调度。
//! 命名补充 16：PascalCase 方法名保留以对齐 Go 调用点迁移成本。
//! 符号索引补充 17：公开 API 的约束优先于内部实现细节。
//! 数据流补充 18：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 19：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 20：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 21：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 22：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 桩补充 23：Mem* 仅服务单测注入，不模拟真实 gRPC/集群调度。
//! 命名补充 24：PascalCase 方法名保留以对齐 Go 调用点迁移成本。
//! 符号索引补充 25：公开 API 的约束优先于内部实现细节。
//! 数据流补充 26：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 27：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 28：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 29：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 30：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 桩补充 31：Mem* 仅服务单测注入，不模拟真实 gRPC/集群调度。
//! 命名补充 32：PascalCase 方法名保留以对齐 Go 调用点迁移成本。
//! 符号索引补充 33：公开 API 的约束优先于内部实现细节。
//! 数据流补充 34：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 35：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 36：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 37：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 38：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 桩补充 39：Mem* 仅服务单测注入，不模拟真实 gRPC/集群调度。
//! 命名补充 40：PascalCase 方法名保留以对齐 Go 调用点迁移成本。
//! 符号索引补充 41：公开 API 的约束优先于内部实现细节。
//! 数据流补充 42：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 43：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 44：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 45：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 46：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 桩补充 47：Mem* 仅服务单测注入，不模拟真实 gRPC/集群调度。
//! 命名补充 48：PascalCase 方法名保留以对齐 Go 调用点迁移成本。
//! 符号索引补充 49：公开 API 的约束优先于内部实现细节。
//! 数据流补充 50：谁产生状态、谁消费状态、失败时如何回滚或标注。
//! 边界补充 51：空输入、取消上下文、未知枚举值都应按 Go 方式处理。
//! 测试补充 52：断言固定契约，不把桩的简化实现误当成生产能力。
//! 对照补充 53：改动前先核对相邻 Go 文件同名符号的注释与测试。
//! 重试补充 54：可重试错误应消耗 backoff，不可重试错误应立即上抛。
//! 桩补充 55：Mem* 仅服务单测注入，不模拟真实 gRPC/集群调度。
//! 命名补充 56：PascalCase 方法名保留以对齐 Go 调用点迁移成本。
//! 符号索引补充 57：公开 API 的约束优先于内部实现细节。

// 统一 Result 别名，对应 Go `errors`/`berrors` 包装链的简化 Rust 版本。
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

// 轻量错误类型，对齐 Go `github.com/pingcap/errors` 的 Errorf/Annotate/Wrapf 调用面。
// 桩实现不做错误链与 stack trace；Cause 直接返回整段 msg 字符串。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
    // 对应 Go `errors.Errorf`，保持 PascalCase 以匹配 task 层现有调用。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }
    // 对应 Go `errors.Annotate`：将上下文消息前置到 base 之前。
    pub fn Annotate(base: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", msg.into(), base.into()),
        }
    }
    pub fn Annotatef(base: impl Into<String>, msg: impl Into<String>) -> Self {
        Self::Annotate(base, msg)
    }
    // 桩：Trace 为恒等，真实实现会附加 stack。
    pub fn Trace(err: Self) -> Self {
        err
    }
    // 对应 Go `errors.Wrapf`：在已有错误前追加前缀。
    pub fn Wrapf(err: Self, msg: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", msg.into(), err.msg),
        }
    }
    // 桩：无多层 Cause 链，直接暴露 msg。
    pub fn Cause(err: &Self) -> &str {
        &err.msg
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for Error {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

// 对应 `br/pkg/utils/berrors` 的 sentinel 字符串；桩仅保留 task 层常用子集。
pub mod berrors {
    // 参数非法；ParseBackend/ParseKey 等校验失败时使用。
    pub const ErrInvalidArgument: &str = "invalid argument";
    // 备份 range 与集群拓扑不一致。
    pub const ErrBackupInvalidRange: &str = "backup invalid range";
    // 全量/增量/PiTR 模式与 meta 不匹配。
    pub const ErrRestoreModeMismatch: &str = "restore mode mismatch";
    // 无法识别 storage scheme。
    pub const ErrStorageUnknown: &str = "storage unknown";
    // 未分类错误 sentinel。
    pub const ErrUnknown: &str = "unknown";
}

// --- task 层默认常量，与 Go `br/pkg/task` 包级变量对齐 ---
// DefChecksumTableConcurrency：restore 后 checksum 表并发上限。
pub const DefChecksumTableConcurrency: u32 = 4;
// DefaultBRGCSafePointTTL：BR GC safepoint 默认 TTL（秒）。
pub const DefaultBRGCSafePointTTL: i64 = 5 * 60;
// DefaultSchemaConcurrency：并行加载/应用 schema 的 goroutine 数。
pub const DefaultSchemaConcurrency: u32 = 64;
// RangesSentThreshold：进度汇报前累计发送 range 数阈值。
pub const RangesSentThreshold: i32 = 1024;
// DefaultMergeRegionSizeBytes：TiKV merge region 目标大小（96 MiB）。
pub const DefaultMergeRegionSizeBytes: u64 = 96 * 1024 * 1024;
// DefaultMergeRegionKeyCount：merge region 目标 key 数。
pub const DefaultMergeRegionKeyCount: u64 = 960000;
// MetaFile：备份 meta 对象名，写入 storage 根目录。
pub const MetaFile: &str = "backupmeta";
// MetaFileSize：meta 分片大小上限（128 KiB）。
pub const MetaFileSize: usize = 128 * 1024;
// UnitRange：进度单位字符串 "range"。
pub const UnitRange: &str = "range";
// UnitRegion：进度单位字符串 "region"。
pub const UnitRegion: &str = "region";
// BackupDataSize：summary 中备份数据量 metric 名。
pub const BackupDataSize: &str = "BackupDataSize";
// RestoreDataSize：summary 中恢复数据量 metric 名。
pub const RestoreDataSize: &str = "RestoreDataSize";
// FlagKeyspaceName：keyspace 相关 CLI flag 名。
pub const FlagKeyspaceName: &str = "keyspace-name";
// MiB：字节换算常量 1024*1024。
pub const MiB: u64 = 1024 * 1024;
// CrypterIvLen：AES-CTR IV 长度（字节）。
pub const CrypterIvLen: usize = 16;
// 流备份全局 checkpoint 相关对象路径前缀下的文件名。
// TruncateSafePointFileName：流备份 truncate safepoint 持久化路径。
pub const TruncateSafePointFileName: &str = "v1/global_checkpoint/ts";
// resumeStateFileName：流备份 resume 状态 JSON 路径。
pub const resumeStateFileName: &str = "v1/global_checkpoint/resume_state.json";
// streamShiftDurationSecs：流备份时间窗口偏移（秒）。
pub const streamShiftDurationSecs: i64 = 60;

// protobuf `encryptionpb` 最小 serde 镜像；不含真实 KMS 加解密逻辑。
pub mod encryptionpb {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub enum EncryptionMethod {
        // 未指定或 proto 默认值。
        UNKNOWN = 0,
        #[default]
        // 明文，不做文件级加密。
        PLAINTEXT = 1,
        AES128_CTR = 2,
        AES192_CTR = 3,
        AES256_CTR = 4,
    }

    // Go 风格别名，便于 task 层与 parity 测试逐字段对照。
    pub use EncryptionMethod::{
        AES128_CTR as EncryptionMethod_AES128_CTR, AES192_CTR as EncryptionMethod_AES192_CTR,
        AES256_CTR as EncryptionMethod_AES256_CTR, PLAINTEXT as EncryptionMethod_PLAINTEXT,
        UNKNOWN as EncryptionMethod_UNKNOWN,
    };

    // 本地文件 master key 配置；真实路径由 `br/pkg/kms` 解析。
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct MasterKeyFile {
        pub Path: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct AwsKms {
        pub AccessKey: String,
        pub SecretAccessKey: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct AzureKms {
        pub TenantId: String,
        pub ClientId: String,
        pub ClientSecret: String,
        pub KeyVaultUrl: String,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct GcpKms {
        pub Credential: String,
    }

    // 多云 KMS 统一配置壳；桩不发起任何云 API 调用。
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct MasterKeyKms {
        pub Vendor: String,
        pub KeyId: String,
        pub Region: String,
        pub Endpoint: String,
        pub AwsKms: Option<AwsKms>,
        pub AzureKms: Option<AzureKms>,
        pub GcpKms: Option<GcpKms>,
    }

    #[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
    pub enum MasterKeyBackend {
        File(MasterKeyFile),
        Kms(MasterKeyKms),
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct MasterKey {
        pub Backend: Option<MasterKeyBackend>,
    }
}

// protobuf `backuppb` 最小 serde 镜像；字段名保持 PascalCase 与 Go proto 生成码一致。
pub mod backuppb {
    use super::encryptionpb::EncryptionMethod;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub enum CompressionType {
        #[default]
        UNKNOWN = 0,
        LZ4 = 1,
        SNAPPY = 2,
        ZSTD = 3,
    }

    pub use CompressionType::{
        LZ4 as CompressionType_LZ4, SNAPPY as CompressionType_SNAPPY,
        UNKNOWN as CompressionType_UNKNOWN, ZSTD as CompressionType_ZSTD,
    };

    // 备份文件级 cipher 描述；桩不执行 CTR 加解密。
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct CipherInfo {
        pub CipherType: EncryptionMethod,
        pub CipherKey: Vec<u8>,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct MasterKeyConfig {
        pub EncryptionType: EncryptionMethod,
        pub MasterKeys: Vec<super::encryptionpb::MasterKey>,
    }

    // 存储后端 URI 解析结果；真实读写由 `br/pkg/storage` 负责。
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct StorageBackend {
        pub Scheme: String,
        pub Path: String,
        pub GcsPrefix: String,
    }

    // RawKV 备份范围；Cf 为 column family 名。
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct RawRange {
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub Cf: String,
    }

    // 单个备份 SST/文件元数据；Size_ 尾随下划线对齐 Go proto 生成字段名。
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct File {
        pub Name: String,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub Size_: u64,
        pub Cf: String,
    }

    // 备份 meta 汇总；桩 MetaWriter 在内存中维护此结构。
    #[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
    pub struct BackupMeta {
        pub ClusterId: u64,
        pub ClusterVersion: String,
        pub BrVersion: String,
        // MVCC 备份起止版本（TSO）。
        pub StartVersion: u64,
        pub EndVersion: u64,
        pub IsRawKv: bool,
        pub IsTxnKv: bool,
        pub RawRanges: Vec<RawRange>,
        pub Files: Vec<File>,
        // KV 编码 API 版本（V1/V2）。
        pub ApiVersion: i32,
    }

    // 单次 Backup RPC 请求参数壳；MemBackupClient 不真正下发到 TiKV。
    #[derive(Clone, Debug, Default)]
    pub struct BackupRequest {
        pub ClusterId: u64,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub StartVersion: u64,
        pub EndVersion: u64,
        pub RateLimit: u64,
        pub Concurrency: u32,
        pub StorageBackend: Option<StorageBackend>,
        pub IsRawKv: bool,
        pub Cf: String,
        pub CompressionType: CompressionType,
        pub CompressionLevel: i32,
        pub CipherInfo: Option<CipherInfo>,
    }

    // 流备份任务安全配置；对应 Go stream task security config。
    #[derive(Clone, Debug, Default)]
    pub struct StreamBackupTaskSecurityConfig {
        pub CipherInfo: Option<CipherInfo>,
        pub MasterKeyConfig: Option<MasterKeyConfig>,
    }
}

// PD `metapb` 最小 Region/Store 镜像；不含 peer/leader 等完整拓扑。
pub mod metapb {
    // Region 分片；桩不含 Epoch/Peers。
    #[derive(Clone, Debug, Default)]
    pub struct Region {
        pub Id: u64,
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
    }

    // TiKV store 节点；Address 为 gRPC 地址字符串。
    #[derive(Clone, Debug, Default)]
    pub struct Store {
        pub Id: u64,
        pub Address: String,
    }
}

// 存储后端 CLI 选项；桩 ParseFromFlags 恒成功且不读取 flag。
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct BackendOptions {
    #[serde(rename = "s3")]
    pub S3: HashMap<String, String>,
}

impl BackendOptions {
    // 桩：忽略 FlagSet，真实实现解析 S3 相关 flag 到 S3 map。
    pub fn ParseFromFlags(&mut self, _flags: &FlagSet) -> Result<()> {
        Ok(())
    }
}

// 存储读写选项；对应 `br/pkg/storage` ExternalStorage 行为开关。
#[derive(Clone, Debug, Default)]
pub struct StorageOptions {
    pub NoCredentials: bool,
    pub SendCredentials: bool,
    pub CheckS3ObjectLockOptions: bool,
}

// gRPC keepalive 参数；桩仅承载字段供 task 配置序列化。
#[derive(Clone, Debug, Default)]
pub struct KeepaliveParams {
    pub Time: Duration,
    pub Timeout: Duration,
    pub PermitWithoutStream: bool,
}

// PD TLS 证书路径三元组；桩不加载或校验证书。
#[derive(Clone, Debug, Default)]
pub struct PDSecurityOption {
    pub CAPath: String,
    pub CertPath: String,
    pub KeyPath: String,
}

// TiKV 集群 mTLS 配置；对应 Go `br/pkg/gluetikv` KVSecurity。
#[derive(Clone, Debug, Default)]
pub struct KVSecurity {
    pub ClusterSSLCA: String,
    pub ClusterSSLCert: String,
    pub ClusterSSLKey: String,
}

// TLS 启用标记；MemMgr 恒返回 None，真实 Mgr 可返回完整 TLS 配置。
#[derive(Clone, Debug, Default)]
pub struct TLSConfigInner {
    pub enabled: bool,
}

// 备份/恢复 key 范围；StartKey 含、EndKey 不含（TiKV 半开区间约定）。
#[derive(Clone, Debug, Default)]
pub struct KeyRange {
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
}

// 范围统计占位；真实实现由 TiKV scan/statistics 填充 Size/Count。
#[derive(Clone, Debug, Default)]
pub struct RangeStats {
    pub StartKey: Vec<u8>,
    pub EndKey: Vec<u8>,
    // 范围内估计数据字节数。
    pub Size: u64,
    // 范围内估计 key 数。
    pub Count: u64,
}

/// Minimal flag store matching pflag Get*/Lookup/Changed semantics used by task.
// 内存版 pflag 替代：覆盖 task 层 Define*/Get*/Changed/Visit 调用面。
// 不解析 argv、不支持 shorthand、不生成 usage 文本；仅服务单测与 arm64 编译。
#[derive(Clone, Debug, Default)]
pub struct FlagSet {
    values: HashMap<String, FlagValue>,
    changed: HashMap<String, bool>,
    hidden: Vec<String>,
}

// flag 值枚举；类型不匹配时 Get* 返回 Error 而非 panic。
#[derive(Clone, Debug)]
pub enum FlagValue {
    String(String),
    Bool(bool),
    Uint(u64),
    Uint32(u32),
    Int(i64),
    Int32(i32),
    Duration(Duration),
    StringSlice(Vec<String>),
    StringArray(Vec<String>),
}

impl FlagSet {
    pub fn new() -> Self {
        Self::default()
    }

    // 显式设值并标记 Changed；对应测试里手动注入 flag。
    pub fn Set(&mut self, name: &str, value: FlagValue) {
        self.values.insert(name.to_string(), value);
        self.changed.insert(name.to_string(), true);
    }

    pub fn DefineString(&mut self, name: &str, default: &str) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::String(default.to_string()));
    }

    // 注册 bool flag；已存在则不覆盖（与 pflag 首次 Define 语义一致）。
    pub fn DefineBool(&mut self, name: &str, default: bool) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Bool(default));
    }

    pub fn DefineUint64(&mut self, name: &str, default: u64) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Uint(default));
    }

    pub fn DefineUint(&mut self, name: &str, default: u64) {
        self.DefineUint64(name, default);
    }

    pub fn DefineUint32(&mut self, name: &str, default: u32) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Uint32(default));
    }

    pub fn DefineInt64(&mut self, name: &str, default: i64) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Int(default));
    }

    pub fn DefineInt32(&mut self, name: &str, default: i32) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Int32(default));
    }

    pub fn DefineInt(&mut self, name: &str, default: i64) {
        self.DefineInt64(name, default);
    }

    pub fn DefineDuration(&mut self, name: &str, default: Duration) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Duration(default));
    }

    pub fn DefineStringSlice(&mut self, name: &str, default: Vec<String>) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::StringSlice(default));
    }

    pub fn DefineStringArray(&mut self, name: &str, default: Vec<String>) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::StringArray(default));
    }

    pub fn MarkHidden(&mut self, name: &str) -> Result<()> {
        self.hidden.push(name.to_string());
        Ok(())
    }

    // 桩：MarkDeprecated 为 no-op，不打印 deprecation warning。
    pub fn MarkDeprecated(&mut self, _name: &str, _msg: &str) -> Result<()> {
        Ok(())
    }

    pub fn Lookup(&self, name: &str) -> Option<&FlagValue> {
        self.values.get(name)
    }

    // 仅 Set 过的 flag 视为 Changed；Define* 默认值不算变更。
    pub fn Changed(&self, name: &str) -> bool {
        self.changed.get(name).copied().unwrap_or(false)
    }

    pub fn GetString(&self, name: &str) -> Result<String> {
        match self.values.get(name) {
            Some(FlagValue::String(s)) => Ok(s.clone()),
            // 非 String 类型 fallback 到 Debug 格式，便于测试观测。
            Some(other) => Ok(format!("{other:?}")),
            None => Err(Error::new(format!("flag {name} not defined"))),
        }
    }

    pub fn GetBool(&self, name: &str) -> Result<bool> {
        match self.values.get(name) {
            Some(FlagValue::Bool(b)) => Ok(*b),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }

    pub fn GetUint64(&self, name: &str) -> Result<u64> {
        match self.values.get(name) {
            Some(FlagValue::Uint(v)) => Ok(*v),
            Some(FlagValue::Uint32(v)) => Ok(*v as u64),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }

    pub fn GetUint(&self, name: &str) -> Result<u32> {
        Ok(self.GetUint64(name)? as u32)
    }

    pub fn GetUint32(&self, name: &str) -> Result<u32> {
        match self.values.get(name) {
            Some(FlagValue::Uint32(v)) => Ok(*v),
            Some(FlagValue::Uint(v)) => Ok(*v as u32),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }

    pub fn GetInt64(&self, name: &str) -> Result<i64> {
        match self.values.get(name) {
            Some(FlagValue::Int(v)) => Ok(*v),
            Some(FlagValue::Int32(v)) => Ok(*v as i64),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }

    pub fn GetInt32(&self, name: &str) -> Result<i32> {
        Ok(self.GetInt64(name)? as i32)
    }

    pub fn GetInt(&self, name: &str) -> Result<i32> {
        Ok(self.GetInt64(name)? as i32)
    }

    pub fn GetDuration(&self, name: &str) -> Result<Duration> {
        match self.values.get(name) {
            Some(FlagValue::Duration(d)) => Ok(*d),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }

    pub fn GetStringSlice(&self, name: &str) -> Result<Vec<String>> {
        match self.values.get(name) {
            Some(FlagValue::StringSlice(v)) | Some(FlagValue::StringArray(v)) => Ok(v.clone()),
            Some(FlagValue::String(s)) => Ok(vec![s.clone()]),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }

    pub fn GetStringArray(&self, name: &str) -> Result<Vec<String>> {
        self.GetStringSlice(name)
    }

    // 仅遍历 Changed 的 flag；对应 Go pflag Changed 语义用于日志脱敏。
    pub fn Visit<F: FnMut(&str, &FlagValue)>(&self, mut f: F) {
        for (k, v) in &self.values {
            if self.Changed(k) {
                f(k, v);
            }
        }
    }
}

// 已解析 flag 快照；供 redact 测试比对 Name/Value。
#[derive(Clone, Debug)]
pub struct Flag {
    pub Name: String,
    pub Value: String,
}

impl FlagValue {
    // 将任意 FlagValue 转为字符串，供 Visit 与日志输出。
    pub fn as_string(&self) -> String {
        match self {
            FlagValue::String(s) => s.clone(),
            FlagValue::Bool(b) => b.to_string(),
            FlagValue::Uint(v) => v.to_string(),
            FlagValue::Uint32(v) => v.to_string(),
            FlagValue::Int(v) => v.to_string(),
            FlagValue::Int32(v) => v.to_string(),
            FlagValue::Duration(d) => format!("{:?}", d),
            FlagValue::StringSlice(v) | FlagValue::StringArray(v) => v.join(","),
        }
    }
}

/// Zap-like field for argument redaction tests.
// 对应 Go `zap.String` 字段；桩仅保留 Key/String 供 common_test 脱敏断言。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZapField {
    pub Key: String,
    pub String: String,
}

pub fn ZapString(key: impl Into<String>, value: impl Into<String>) -> ZapField {
    ZapField {
        Key: key.into(),
        String: value.into(),
    }
}

pub fn ZapStringer(key: impl Into<String>, value: impl Into<String>) -> ZapField {
    ZapString(key, value)
}

// 进度条 trait；对应 Go glue Progress 接口。
pub trait Progress: Send + Sync {
    fn Inc(&self);
    fn IncBy(&self, n: i64);
    fn GetCurrent(&self) -> i64;
    fn Close(&self);
}

// 内存进度计数器；StartProgress 默认返回此类型。
#[derive(Default)]
pub struct MemProgress {
    current: AtomicI64,
    closed: AtomicBool,
}

impl Progress for MemProgress {
    fn Inc(&self) {
        self.IncBy(1);
    }
    fn IncBy(&self, n: i64) {
        self.current.fetch_add(n, Ordering::SeqCst);
    }
    fn GetCurrent(&self) -> i64 {
        self.current.load(Ordering::SeqCst)
    }
    fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

// Glue 门面 trait；真实实现由 `br/pkg/glue` 提供 TiDB 控制台与 metrics 接入。
pub trait Glue: Send + Sync {
    fn GetRestoreLifecycle(
        &self,
        _kind: crate::restore_lifecycle::RestoreKind,
        _files: &[backuppb::File],
    ) -> Result<crate::restore_lifecycle::RestoreLifecycle> {
        Err(Error::new("restore PD/import lifecycle is not configured"))
    }
    fn GetVersion(&self) -> String;
    fn StartProgress(&self, _cmd: &str, total: i64, _log_progress: bool) -> Arc<dyn Progress>;
    fn Record(&self, key: &str, value: u64);
    fn ConsoleOutWrite(&self, msg: &[u8]) -> Result<()>;
}

// 内存 Glue：Record 写入 HashMap，ConsoleOutWrite 追加到 Vec<u8>。
#[derive(Default)]
pub struct MemGlue {
    pub version: String,
    pub records: Mutex<HashMap<String, u64>>,
    pub console: Mutex<Vec<u8>>,
}

impl Glue for MemGlue {
    fn GetVersion(&self) -> String {
        self.version.clone()
    }
    fn StartProgress(&self, _cmd: &str, _total: i64, _log_progress: bool) -> Arc<dyn Progress> {
        Arc::new(MemProgress::default())
    }
    fn Record(&self, key: &str, value: u64) {
        self.records.lock().unwrap().insert(key.to_string(), value);
    }
    fn ConsoleOutWrite(&self, msg: &[u8]) -> Result<()> {
        self.console.lock().unwrap().extend_from_slice(msg);
        Ok(())
    }
}

// 外部存储 trait；真实实现对接 S3/GCS/Azure/local 等 `br/pkg/storage` 后端。
pub trait Storage: Send + Sync {
    fn ReadFile(&self, name: &str) -> Result<Vec<u8>>;
    fn WriteFile(&self, name: &str, data: &[u8]) -> Result<()>;
    fn FileExists(&self, name: &str) -> Result<bool>;
    fn WalkDir(&self, sub_dir: &str, f: &mut dyn FnMut(&str, i64) -> Result<()>) -> Result<()>;
}

// 进程内 HashMap 文件系统；WalkDir 按前缀过滤，不含目录层级语义。
#[derive(Default, Clone, Debug)]
pub struct MemStorage {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
}

impl MemStorage {
    pub fn new() -> Self {
        Self::default()
    }
    // 测试辅助：直接向内存 map 写入文件内容。
    pub fn put(&self, name: &str, data: Vec<u8>) {
        self.files.lock().unwrap().insert(name.to_string(), data);
    }
}

impl Storage for MemStorage {
    // 按完整路径键读取；不存在返回 file not found 错误。
    fn ReadFile(&self, name: &str) -> Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new(format!("file not found: {name}")))
    }
    // 覆盖写入；不保留目录层级，路径即 HashMap key。
    fn WriteFile(&self, name: &str, data: &[u8]) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(name.to_string(), data.to_vec());
        Ok(())
    }
    fn FileExists(&self, name: &str) -> Result<bool> {
        Ok(self.files.lock().unwrap().contains_key(name))
    }
    // 前缀匹配 WalkDir；size 为字节长度而非 inode 元数据。
    fn WalkDir(&self, sub_dir: &str, f: &mut dyn FnMut(&str, i64) -> Result<()>) -> Result<()> {
        let entries: Vec<(String, i64)> = {
            let files = self.files.lock().unwrap();
            files
                .iter()
                .filter(|(path, _)| path.starts_with(sub_dir))
                .map(|(path, data)| (path.clone(), data.len() as i64))
                .collect()
        };
        for (path, size) in entries {
            f(&path, size)?;
        }
        Ok(())
    }
}

// restore 结束时需调用的 scheduler 恢复闭包；RemoveSchedulers 返回此类型。
pub type RestoreSchedulers = Box<dyn Fn() -> Result<()> + Send + Sync>;

// PD/TiDB 集群管理 trait；真实 Mgr 连接 PD 并操作 scheduler。
pub trait Mgr: Send + Sync {
    fn GetGCManager(&self) -> Option<Arc<dyn astersql_br_pkg_gc::Manager>> {
        None
    }
    fn Close(&self);
    fn GetClusterVersion(&self) -> Result<String>;
    fn GetRegionCount(&self, start: &[u8], end: &[u8]) -> Result<usize>;
    fn RemoveSchedulers(&self) -> Result<RestoreSchedulers>;
    fn GetTLSConfig(&self) -> Option<TLSConfigInner>;
    fn UpdatePDScheduleConfig(&self) -> Result<()>;
}

// 内存 Mgr：region_count 为固定注入值，RemoveSchedulers 返回空恢复闭包。
#[derive(Default)]
pub struct MemMgr {
    pub gc_manager: Option<Arc<dyn astersql_br_pkg_gc::Manager>>,
    pub cluster_version: String,
    pub region_count: usize,
    pub closed: AtomicBool,
    pub remove_called: AtomicBool,
    pub update_pd_schedule_called: AtomicBool,
}

impl Mgr for MemMgr {
    fn GetGCManager(&self) -> Option<Arc<dyn astersql_br_pkg_gc::Manager>> {
        self.gc_manager.clone()
    }
    fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    fn GetClusterVersion(&self) -> Result<String> {
        Ok(self.cluster_version.clone())
    }
    fn GetRegionCount(&self, _start: &[u8], _end: &[u8]) -> Result<usize> {
        Ok(self.region_count)
    }
    // 桩：返回 no-op 闭包并置 remove_called；不真正移除 PD scheduler。
    fn RemoveSchedulers(&self) -> Result<RestoreSchedulers> {
        self.remove_called.store(true, Ordering::SeqCst);
        Ok(Box::new(|| Ok(())))
    }
    fn GetTLSConfig(&self) -> Option<TLSConfigInner> {
        None
    }
    fn UpdatePDScheduleConfig(&self) -> Result<()> {
        self.update_pd_schedule_called.store(true, Ordering::SeqCst);
        Ok(())
    }
}

// 备份客户端 trait；真实实现经 gRPC 调用 TiKV backup 服务。
pub trait BackupClient: Send + Sync {
    /// Checkpoint-aware clients override this with the persisted safe point ID.
    fn GetSafePointID(&self) -> String {
        astersql_br_pkg_gc::MakeSafePointID()
    }

    fn GetClusterID(&self) -> u64;
    fn GetCurrentTS(&self) -> Result<u64>;
    fn GetStorageBackend(&self) -> Option<backuppb::StorageBackend>;
    fn GetApiVersion(&self) -> i32;
    fn SetStorageAndCheckNotInUse(
        &self,
        backend: &backuppb::StorageBackend,
        opts: &StorageOptions,
    ) -> Result<()>;
    /// Build ranges after applying the configured table filter.
    /// The default empty-bound range denotes the complete keyspace.
    fn BuildBackupRanges(
        &self,
        _filter: &[String],
        _backup_ts: u64,
        _is_full_backup: bool,
    ) -> Result<Vec<KeyRange>> {
        Ok(vec![KeyRange::default()])
    }
    fn BackupRanges(&self, ranges: &[KeyRange], req: &backuppb::BackupRequest) -> Result<u64>;
    fn GetStorage(&self) -> Arc<dyn Storage>;
}

// 内存 BackupClient：BackupRanges 仅置标志并返回预设 archive_size。
#[derive(Default)]
pub struct MemBackupClient {
    pub cluster_id: u64,
    pub current_ts: u64,
    pub api_version: i32,
    pub storage: MemStorage,
    pub archive_size: AtomicU64,
    pub backup_called: AtomicBool,
    pub ranges: Option<Vec<KeyRange>>,
}

impl BackupClient for MemBackupClient {
    fn GetClusterID(&self) -> u64 {
        self.cluster_id
    }
    // 桩：返回注入的 current_ts，不访问 PD TSO。
    fn GetCurrentTS(&self) -> Result<u64> {
        Ok(self.current_ts)
    }
    fn GetStorageBackend(&self) -> Option<backuppb::StorageBackend> {
        // 桩：固定返回 local:///tmp，不代表真实 storage 绑定状态。
        Some(backuppb::StorageBackend {
            Scheme: "local".into(),
            Path: "/tmp".into(),
            ..Default::default()
        })
    }
    fn GetApiVersion(&self) -> i32 {
        self.api_version
    }
    fn SetStorageAndCheckNotInUse(
        &self,
        _backend: &backuppb::StorageBackend,
        _opts: &StorageOptions,
    ) -> Result<()> {
        Ok(())
    }
    fn BuildBackupRanges(
        &self,
        _filter: &[String],
        _backup_ts: u64,
        _is_full_backup: bool,
    ) -> Result<Vec<KeyRange>> {
        Ok(self
            .ranges
            .clone()
            .unwrap_or_else(|| vec![KeyRange::default()]))
    }
    fn BackupRanges(&self, _ranges: &[KeyRange], _req: &backuppb::BackupRequest) -> Result<u64> {
        self.backup_called.store(true, Ordering::SeqCst);
        Ok(self.archive_size.load(Ordering::SeqCst))
    }
    fn GetStorage(&self) -> Arc<dyn Storage> {
        Arc::new(self.storage.clone())
    }
}

// 备份 meta 异步写入器桩；StartWriteMetasAsync 为空操作，meta 存于 Mutex。
#[derive(Clone, Debug, Default)]
pub struct MetaWriter {
    meta: Arc<Mutex<backuppb::BackupMeta>>,
    archive_size: Arc<AtomicU64>,
    finished: Arc<AtomicBool>,
    flushed: Arc<AtomicBool>,
}

impl MetaWriter {
    pub fn new() -> Self {
        Self::default()
    }
    // 桩：无后台 goroutine，真实实现会异步 flush meta 到 storage。
    pub fn StartWriteMetasAsync(&self) {}
    pub fn Update<F: FnOnce(&mut backuppb::BackupMeta)>(&self, f: F) {
        f(&mut self.meta.lock().unwrap());
    }
    pub fn FinishWriteMetas(&self) -> Result<()> {
        self.finished.store(true, Ordering::SeqCst);
        Ok(())
    }
    pub fn FlushBackupMeta(&self) -> Result<()> {
        self.flushed.store(true, Ordering::SeqCst);
        Ok(())
    }
    pub fn ArchiveSize(&self) -> u64 {
        self.archive_size.load(Ordering::SeqCst)
    }
    // 测试辅助：读取当前 meta 快照。
    pub fn snapshot(&self) -> backuppb::BackupMeta {
        self.meta.lock().unwrap().clone()
    }
    pub fn was_finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }
    pub fn was_flushed(&self) -> bool {
        self.flushed.load(Ordering::SeqCst)
    }
}

// 命令结束 summary 收集器；对应 Go glue SummaryCollector。
#[derive(Default)]
pub struct SummaryCollector {
    pub ints: Mutex<HashMap<String, i64>>,
    pub success: AtomicBool,
}

impl SummaryCollector {
    pub fn CollectInt(&self, name: &str, v: i64) {
        self.ints.lock().unwrap().insert(name.to_string(), v);
    }
    pub fn SetSuccessStatus(&self, ok: bool) {
        self.success.store(ok, Ordering::SeqCst);
    }
    // 桩：Summary 不输出日志，真实 glue 会打印汇总表。
    pub fn Summary(&self, _cmd: &str) {}
}

// thread_local 全局 summary；对应 Go 包级 summary 变量。
thread_local! {
    static SUMMARY: SummaryCollector = SummaryCollector::default();
}

pub fn CollectInt(name: &str, v: i64) {
    SUMMARY.with(|s| s.CollectInt(name, v));
}

pub fn SetSuccessStatus(ok: bool) {
    SUMMARY.with(|s| s.SetSuccessStatus(ok));
}

pub fn Summary(_cmd: &str) {
    SUMMARY.with(|s| s.Summary(_cmd));
}

pub fn TakeSuccessStatus() -> bool {
    SUMMARY.with(|s| s.success.load(Ordering::SeqCst))
}

// 解析 storage URI 为 StorageBackend；桩不校验 bucket 可达性也不读 BackendOptions。
pub fn ParseBackend(storage: &str, _opts: &BackendOptions) -> Result<backuppb::StorageBackend> {
    if storage.is_empty() {
        // 断言/错误路径：验证与 Go 测试相同的边界、计数或错误分类。
        return Err(Error::Annotate(
            berrors::ErrInvalidArgument,
            "storage is empty",
        ));
    }
    let (scheme, path) = if let Some(idx) = storage.find("://") {
        (&storage[..idx], &storage[idx + 3..])
    } else {
        ("local", storage)
    };
    Ok(backuppb::StorageBackend {
        Scheme: scheme.to_string(),
        Path: path.to_string(),
        ..Default::default()
    })
}

// 解析 CLI key 参数；支持 hex/raw/escaped，对应 Go `br/pkg/task` ParseKey。
pub fn ParseKey(format: &str, key: &str) -> Result<Vec<u8>> {
    if key.is_empty() {
        return Ok(Vec::new());
    }
    match format {
        "hex" => hex::decode(key).map_err(|e| Error::new(e.to_string())),
        "raw" => Ok(key.as_bytes().to_vec()),
        "escaped" => unescaped_key(key),
        _ => Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            "unknown format",
        )),
    }
}

fn unescaped_key(key: &str) -> Result<Vec<u8>> {
    let bytes = key.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let current = bytes[index];
        index += 1;
        if current != b'\\' {
            out.push(current);
            continue;
        }
        if index == bytes.len() {
            return Err(Error::new("unexpected EOF"));
        }
        let escaped = bytes[index];
        index += 1;
        if let Some(position) = b"abfnrtv\\'\"".iter().position(|value| *value == escaped) {
            out.push(b"\x07\x08\x0c\n\r\t\x0b\\'\""[position]);
            continue;
        }
        if escaped == b'x' {
            let end = (index + 2).min(bytes.len());
            let digits = std::str::from_utf8(&bytes[index..end])
                .map_err(|error| Error::new(error.to_string()))?;
            let value =
                u8::from_str_radix(digits, 16).map_err(|error| Error::new(error.to_string()))?;
            out.push(value);
            index = end;
            continue;
        }
        let end = (index + 2).min(bytes.len());
        let mut digits = Vec::with_capacity(3);
        digits.push(escaped);
        digits.extend_from_slice(&bytes[index..end]);
        let digits = std::str::from_utf8(&digits).map_err(|error| Error::new(error.to_string()))?;
        let value = u8::from_str_radix(digits, 8).map_err(|error| Error::new(error.to_string()))?;
        out.push(value);
        index = end;
    }
    Ok(out)
}

// 判断加密方法是否为有效 AES-CTR；PLAINTEXT/UNKNOWN 返回 false。
pub fn IsEffectiveEncryptionMethod(m: encryptionpb::EncryptionMethod) -> bool {
    matches!(
        m,
        encryptionpb::EncryptionMethod::AES128_CTR
            | encryptionpb::EncryptionMethod::AES192_CTR
            | encryptionpb::EncryptionMethod::AES256_CTR
    )
}

// 为标识符加反引号；对应 TiDB SQL 引用风格。
pub fn EncloseName(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

pub fn EncloseDBAndTable(db: &str, table: &str) -> String {
    format!("{}.{}", EncloseName(db), EncloseName(table))
}

/// TiDB TSO helpers matching `oracle` package physical/logical split.
// TSO 编解码桩；算法与 `github.com/tikv/pd/client/pkg/timestamp` 一致，无 PD 连接。
pub mod oracle {
    const PHYSICAL_SHIFT_BITS: u64 = 18;

    // 提取 TSO 高 46 位物理时间（毫秒）。
    pub fn ExtractPhysical(ts: u64) -> i64 {
        (ts >> PHYSICAL_SHIFT_BITS) as i64
    }

    // 提取 TSO 低 18 位逻辑计数。
    pub fn ExtractLogical(ts: u64) -> i64 {
        (ts & ((1u64 << PHYSICAL_SHIFT_BITS) - 1)) as i64
    }

    pub fn ComposeTS(physical: i64, logical: i64) -> u64 {
        ((physical as u64) << PHYSICAL_SHIFT_BITS) | (logical as u64)
    }

    // 将 Go time 毫秒时间戳转为 TSO（logical=0）。
    pub fn GoTimeToTS(millis: i64) -> u64 {
        ComposeTS(millis, 0)
    }
}

// 编码 TiDB table prefix：`t` + big-endian table_id。
pub fn EncodeTablePrefix(table_id: i64) -> Vec<u8> {
    let mut buf = vec![0x74]; // 't'
    buf.extend_from_slice(&((table_id as u64) ^ (1_u64 << 63)).to_be_bytes());
    buf
}

// memcomparable 编码；与 Go `pkg/util/codec.EncodeBytes` 的 8-byte group 格式一致。
pub fn EncodeBytes(data: &[u8]) -> Vec<u8> {
    const GROUP_SIZE: usize = 8;
    const MARKER: u8 = 0xff;
    let mut out = Vec::with_capacity((data.len() / GROUP_SIZE + 1) * (GROUP_SIZE + 1));
    for index in (0..=data.len()).step_by(GROUP_SIZE) {
        let remaining = data.len() - index;
        let group_len = remaining.min(GROUP_SIZE);
        out.extend_from_slice(&data[index..index + group_len]);
        let padding = GROUP_SIZE - group_len;
        out.resize(out.len() + padding, 0);
        out.push(MARKER - padding as u8);
    }
    out
}

pub fn GetStreamBackupGlobalCheckpointPrefix() -> &'static str {
    "v1/global_checkpoint"
}

// 累加备份文件 Size_ 字段；不含压缩前后差异计算。
pub fn ArchiveSize(files: &[backuppb::File]) -> u64 {
    files.iter().map(|f| f.Size_).sum()
}

// 表过滤规则；简化版 glob，并支持 Go filter.CaseInsensitive 的大小写语义。
#[derive(Clone, Debug, Default)]
pub struct TableFilter {
    pub patterns: Vec<String>,
    pub case_insensitive: bool,
}

impl TableFilter {
    // 匹配 db.table；支持 `*.*`、精确名、schema.* 前缀模式。
    pub fn MatchTable(&self, db: &str, table: &str) -> bool {
        if self.patterns.is_empty() || self.patterns.iter().any(|p| p == "*.*") {
            return true;
        }
        let (db, table) = if self.case_insensitive {
            (db.to_lowercase(), table.to_lowercase())
        } else {
            (db.to_string(), table.to_string())
        };
        let full = format!("{db}.{table}");
        for p in &self.patterns {
            let pattern = if self.case_insensitive {
                p.to_lowercase()
            } else {
                p.clone()
            };
            if pattern == "*.*" || pattern == full || pattern == format!("`{db}`.`{table}`") {
                return true;
            }
            if pattern.ends_with(".*") {
                let schema = pattern.trim_end_matches(".*").trim_matches('`');
                if schema == db {
                    return true;
                }
            }
        }
        false
    }
}

// 从 CLI filter 字符串构造 TableFilter；桩不做复杂 glob 语法校验。
pub fn ParseFilter(patterns: Vec<String>) -> Result<TableFilter> {
    Ok(TableFilter {
        patterns,
        case_insensitive: false,
    })
}

pub fn CaseInsensitive(mut f: TableFilter) -> TableFilter {
    f.case_insensitive = true;
    f
}

// 集群版本检查类型占位；真实 checker 会比对 TiDB/TiKV/PD 版本矩阵。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VersionCheckerType {
    NormalVersionChecker,
}

pub const NormalVersionChecker: VersionCheckerType = VersionCheckerType::NormalVersionChecker;

// 带来源标记的 u64；Modified 表示用户是否显式覆盖默认值。
#[derive(Clone, Debug, Default)]
pub struct ModifiedU64 {
    pub Value: u64,
    pub Modified: bool,
}

// TiKV merge region 配置项；restore 前可能 tweak 这些值。
#[derive(Clone, Debug, Default)]
pub struct KVConfig {
    pub MergeRegionSize: ModifiedU64,
    pub MergeRegionKeyCount: ModifiedU64,
}

// 流备份持久化状态；对应 checkpoint JSON 序列化字段。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PersistentState {
    pub LastCheckpoint: u64,
}

// 日志恢复任务进度快照；Progress 为枚举常量而非百分比。
#[derive(Clone, Debug, Default)]
pub struct TaskInfoForLogRestore {
    pub Progress: u8,
}

pub const LogRestoreProgressIdMapSaved: u8 = 1;

// restore schema 替换映射：old DB id -> 新 DB 及表/id 映射。
#[derive(Clone, Debug, Default)]
pub struct DBReplace {
    pub Name: String,
    pub FilteredOut: bool,
    pub TableMap: HashMap<i64, TableReplace>,
}

#[derive(Clone, Debug, Default)]
pub struct TableReplace {
    pub Name: String,
    pub TableID: i64,
    pub FilteredOut: bool,
    pub PartitionMap: HashMap<i64, i64>,
    pub IndexMap: HashMap<i64, i64>,
}

#[derive(Clone, Debug, Default)]
pub struct SchemasReplace {
    pub DbReplaceMap: HashMap<i64, DBReplace>,
}

// 表级 rewrite rule；桩忽略 index_map 与 collation，仅映射 table id。
#[derive(Clone, Debug, Default)]
pub struct RewriteRules {
    pub OldTableID: i64,
    pub NewTableID: i64,
}

pub fn GetRewriteRuleOfTable(
    old_id: i64,
    new_id: i64,
    _index_map: &HashMap<i64, i64>,
    _new_collation: bool,
) -> RewriteRules {
    RewriteRules {
        OldTableID: old_id,
        NewTableID: new_id,
    }
}

// 判断是否为 TiDB 系统库；与 Go 一样要求调用方传入规范名称。
pub fn IsSysOrTempSysDB(name: &str) -> bool {
    const TEMPORARY_PREFIX: &str = "__TiDB_BR_Temporary_";
    let name = name.strip_prefix(TEMPORARY_PREFIX).unwrap_or(name);
    matches!(name, "mysql" | "sys" | "workload_schema")
}

/// Local stand-in for `br/pkg/operation.Context` (keeps public fields testable).
// OperationContext 桩：对齐 Go operation.Context 的 ID/StartedAt/HintFields 语义。
// 真实路径由 `br/pkg/operation` 创建并注入 etcd；此处用 UUID 生成 ID。
#[derive(Clone, Debug)]
pub struct OperationContext {
    pub OperationID: String,
    pub StartedAt: std::time::SystemTime,
    hint_fields: HashMap<String, String>,
}

impl Default for OperationContext {
    fn default() -> Self {
        Self {
            OperationID: String::new(),
            StartedAt: std::time::SystemTime::UNIX_EPOCH,
            hint_fields: HashMap::new(),
        }
    }
}

impl OperationContext {
    // 空 key 忽略；空 value 删除已有 hint。
    pub fn SetHintField(&mut self, key: &str, value: &str) {
        if key.is_empty() {
            return;
        }
        if value.is_empty() {
            self.hint_fields.remove(key);
            return;
        }
        self.hint_fields.insert(key.to_string(), value.to_string());
    }

    pub fn HintFields(&self) -> &HashMap<String, String> {
        &self.hint_fields
    }
}

// 创建新 OperationContext；桩忽略 command 参数，不注册 etcd operation。
pub fn NewOperationContext(command: &str) -> Result<OperationContext> {
    let _ = command;
    Ok(OperationContext {
        OperationID: uuid::Uuid::new_v4().to_string(),
        StartedAt: std::time::SystemTime::now(),
        hint_fields: HashMap::new(),
    })
}

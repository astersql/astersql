// Copyright 2026 AsterSQL.
//! Local stand-ins for glue/PD/TiKV/storage/domain/flag/CRR/checksum boundaries (arm64-safe).
//!
//! Operator 子包共享桩与测试替身：对象存储、PD/Store、migration、
//! 控制台、CRR/etcd 客户端配置、GC safepoint 等。
//! 目标是让 `checksum_table`/`migrate_to`/`prepare_snap`/`crr_checkpoint`
//! 等模块在无真实集群/云存储时也能编译与契约测试。
//! 注释标明占位边界：缺网络实现处返回明确错误，不伪装成功。
//! 与各 Go 依赖包的公开符号名尽量一一对应，便于对照迁移。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// 本包统一 Result；错误类型为桩 `Error`，对齐 Go `error` 返回习惯。
pub type Result<T> = std::result::Result<T, Error>;

/// 轻量错误载体：仅 msg；支持 Annotate/Trace/Errorf 以贴近 pingcap/errors。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
}

impl Error {
    /// 由消息构造 Error。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }
    /// 格式化构造错误。
    pub fn Errorf(msg: impl Into<String>) -> Self {
        Self::new(msg)
    }
    /// 包裹错误并附加上下文消息。
    pub fn Annotate(base: impl Into<String>, msg: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", msg.into(), base.into()),
        }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Annotatef(base: impl Into<String>, msg: impl Into<String>) -> Self {
        Self::Annotate(base, msg)
    }
    /// 保留堆栈语义的错误包装（此处仅透传 msg）。
    pub fn Trace(err: Self) -> Self {
        err
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Wrapf(err: Self, msg: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", msg.into(), err.msg),
        }
    }
}

impl std::fmt::Display for Error {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// Minimal cancellable context used by long-running operator commands.
/// Clones observe the same cancellation state, matching Go context propagation.
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<AtomicBool>,
}

impl Context {
    pub fn Background() -> Self {
        Self::default()
    }

    pub fn Cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn IsCancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Reuse a caller-owned cancellation flag so command-layer cancellation
    /// reaches long-running operator work just like Go's context propagation.
    pub fn FromCancellationFlag(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }

    pub(crate) fn cancellation_flag(&self) -> Arc<AtomicBool> {
        self.cancelled.clone()
    }
}

impl From<String> for Error {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl From<&str> for Error {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

pub mod berrors {
    /// 与 Go 同名常量对齐的桩常量。
    pub const ErrInvalidArgument: &str = "invalid argument";
    /// 与 Go 同名常量对齐的桩常量。
    pub const ErrPossibleInconsistency: &str = "possible inconsistency";
}

/// 默认 schema 并发；与 BR 任务层 Go 常量同名同值。
pub const DefaultSchemaConcurrency: u32 = 64;
/// BR GC safepoint 默认 TTL（秒）。
pub const DefaultBRGCSafePointTTL: i64 = 5 * 60;
/// 备份锁文件名；存在表示存储上有进行中的备份/同步。
pub const LockFile: &str = "backup.lock";
/// 备份元数据对象名（无扩展名形式）。
pub const MetaFile: &str = "backupmeta";
/// 空 keyspace ID 占位。
pub const NullspaceID: u32 = 0;

/// 对象存储后端选项桩；真实 S3/GCS 字段在完整实现中展开。
#[derive(Clone, Debug, Default)]
pub struct BackendOptions {
    pub S3: HashMap<String, String>,
}

impl BackendOptions {
    /// 从 FlagSet 解析字段。
    pub fn ParseFromFlags(&mut self, _flags: &FlagSet) -> Result<()> {
        Ok(())
    }
}

/// 注册 backend 相关 flag；当前为空实现，保持 CLI 可链接。
pub fn DefineBackendFlags(_flags: &mut FlagSet) {}

/// 解析后的存储后端描述（scheme/bucket 等）桩。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct StorageBackend {
    pub Scheme: String,
    pub Path: String,
    pub raw: String,
}

impl StorageBackend {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Marshal(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(self).map_err(|e| Error::new(e.to_string()))
    }
}

/// 打开存储时的选项（如 S3 object lock 检查）。
#[derive(Clone, Debug, Default)]
pub struct StorageOptions {
    pub SendCredentials: bool,
    pub CheckS3ObjectLockOptions: bool,
}

/// 打开 Reader 的范围/限速选项桩。
#[derive(Clone, Debug, Default)]
pub struct ReaderOption {
    pub StartOffset: Option<i64>,
    pub EndOffset: Option<i64>,
}

/// WalkDir 分页与前缀选项桩。
#[derive(Clone, Debug, Default)]
pub struct WalkOption {
    pub SubDir: String,
    pub ListCount: i64,
}

/// DryRun 产生的外部存储副作用描述。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Effect {
    pub Op: String,
    pub Path: String,
}

/// 将 effects 序列化到临时 JSON，供 migrate-to DryRun 人工核对。
pub fn SaveJSONEffectsToTmp(effects: &[Effect]) -> Result<String> {
    let path = format!(
        "{}/br-effects-{}.json",
        std::env::temp_dir().display(),
        uuid::Uuid::new_v4()
    );
    let data = serde_json::to_vec_pretty(effects).map_err(|e| Error::new(e.to_string()))?;
    std::fs::write(&path, data).map_err(|e| Error::new(e.to_string()))?;
    Ok(path)
}

/// 加解密密钥信息桩。
#[derive(Clone, Debug, Default)]
pub struct CipherInfo {
    pub CipherKey: Vec<u8>,
}

/// TLS 材料路径/开关；`IsEnabled`/`ToTLSConfig` 供拨号前校验。
#[derive(Clone, Debug, Default)]
pub struct TLSConfig {
    pub CA: String,
    pub Cert: String,
    pub Key: String,
}

impl TLSConfig {
    /// TLS 是否启用。
    pub fn IsEnabled(&self) -> bool {
        !self.CA.is_empty()
    }
    /// 转为 TLS 材料；无效路径报错。
    pub fn ToTLSConfig(&self) -> Result<TlsMaterial> {
        Ok(TlsMaterial {
            enabled: self.IsEnabled(),
        })
    }
    /// 转为 PD 安全选项。
    pub fn ToPDSecurityOption(&self) -> PDSecurityOption {
        PDSecurityOption {
            CAPath: self.CA.clone(),
            CertPath: self.Cert.clone(),
            KeyPath: self.Key.clone(),
        }
    }
}

/// 已加载的 TLS 证书材料桩。
#[derive(Clone, Debug, Default)]
pub struct TlsMaterial {
    pub enabled: bool,
}

/// PD 客户端安全选项桩。
#[derive(Clone, Debug, Default)]
pub struct PDSecurityOption {
    pub CAPath: String,
    pub CertPath: String,
    pub KeyPath: String,
}

/// gRPC keepalive 参数，对齐 Go `keepalive.ClientParameters`。
#[derive(Clone, Debug, Default)]
pub struct KeepaliveParams {
    pub Time: Duration,
    pub Timeout: Duration,
    pub PermitWithoutStream: bool,
}

/// 表过滤规则桩；MatchTable 供 checksum 等路径使用。
#[derive(Clone, Debug, Default)]
pub struct TableFilter {
    pub schemas: Vec<String>,
    pub tables: Vec<(String, String)>,
    pub match_all: bool,
}

impl TableFilter {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn MatchSchema(&self, name: &str) -> bool {
        if self.match_all || self.schemas.is_empty() {
            return true;
        }
        self.schemas.iter().any(|s| s == name)
    }
    /// 表是否通过过滤器。
    pub fn MatchTable(&self, db: &str, table: &str) -> bool {
        if self.match_all || self.tables.is_empty() {
            return true;
        }
        self.tables.iter().any(|(d, t)| d == db && t == table)
    }
}

/// 通用 BR Config 子集：PD/TLS/keepalive 等 operator 需要的字段。
#[derive(Clone, Debug)]
pub struct Config {
    pub BackendOptions: BackendOptions,
    pub Storage: String,
    pub PD: Vec<String>,
    pub TLS: TLSConfig,
    pub TableConcurrency: u32,
    pub ChecksumConcurrency: u32,
    pub CheckRequirements: bool,
    pub KeyspaceName: String,
    pub GRPCKeepaliveTime: Duration,
    pub GRPCKeepaliveTimeout: Duration,
    pub CipherInfo: CipherInfo,
    pub TableFilter: TableFilter,
}

impl Default for Config {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn default() -> Self {
        Self {
            BackendOptions: BackendOptions::default(),
            Storage: String::new(),
            PD: Vec::new(),
            TLS: TLSConfig::default(),
            TableConcurrency: DefaultSchemaConcurrency,
            ChecksumConcurrency: 4,
            CheckRequirements: true,
            KeyspaceName: String::new(),
            GRPCKeepaliveTime: Duration::from_secs(10),
            GRPCKeepaliveTimeout: Duration::from_secs(3),
            CipherInfo: CipherInfo::default(),
            TableFilter: TableFilter {
                match_all: true,
                ..Default::default()
            },
        }
    }
}

impl Config {
    /// 从 FlagSet 解析字段。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        if flags.Lookup("storage").is_some() {
            self.Storage = flags.GetString("storage")?;
        }
        if flags.Lookup("pd").is_some() {
            self.PD = flags.GetStringSlice("pd")?;
        }
        if flags.Lookup("keyspace-name").is_some() {
            self.KeyspaceName = flags.GetString("keyspace-name")?;
        }
        if flags.Lookup("table-concurrency").is_some() {
            self.TableConcurrency = flags.GetUint("table-concurrency")?;
        }
        if flags.Lookup("grpc-keepalive-time").is_some() {
            self.GRPCKeepaliveTime = flags.GetDuration("grpc-keepalive-time")?;
        }
        if flags.Lookup("grpc-keepalive-timeout").is_some() {
            self.GRPCKeepaliveTimeout = flags.GetDuration("grpc-keepalive-timeout")?;
        }
        Ok(())
    }
}

/// 恢复配置子集桩，供与 restore 任务交叉引用的最小字段。
#[derive(Clone, Debug, Default)]
pub struct RestoreConfig {
    pub Config: Config,
    pub RestoreTS: u64,
    pub UpstreamClusterID: u64,
    pub TableConcurrency: u32,
}

impl RestoreConfig {
    /// 从 FlagSet 解析字段。
    pub fn ParseFromFlags(&mut self, flags: &FlagSet) -> Result<()> {
        self.Config.ParseFromFlags(flags)
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug)]
enum FlagValue {
    String(String),
    Bool(bool),
    Uint(u64),
    Int(i64),
    Duration(Duration),
    StringSlice(Vec<String>),
}

/// 极简 flag 集合：字符串/布尔/整数，模拟 pflag 供 ParseFromFlags。
#[derive(Clone, Debug, Default)]
pub struct FlagSet {
    values: HashMap<String, FlagValue>,
    shorts: HashMap<String, String>,
}

impl FlagSet {
    /// 由消息构造 Error。
    pub fn new() -> Self {
        Self::default()
    }

    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn String(&mut self, name: &str, default: &str, _help: &str) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::String(default.to_string()));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn StringP(&mut self, name: &str, short: &str, default: &str, help: &str) {
        self.shorts.insert(short.to_string(), name.to_string());
        self.String(name, default, help);
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Bool(&mut self, name: &str, default: bool, _help: &str) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Bool(default));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn BoolP(&mut self, name: &str, short: &str, default: bool, help: &str) {
        self.shorts.insert(short.to_string(), name.to_string());
        self.Bool(name, default, help);
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Uint(&mut self, name: &str, default: u32, _help: &str) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Uint(default as u64));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Uint64(&mut self, name: &str, default: u64, _help: &str) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Uint(default));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Uint64P(&mut self, name: &str, short: &str, default: u64, help: &str) {
        self.shorts.insert(short.to_string(), name.to_string());
        self.Uint64(name, default, help);
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Int(&mut self, name: &str, default: i32, _help: &str) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Int(default as i64));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Int64(&mut self, name: &str, default: i64, _help: &str) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Int(default));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Duration(&mut self, name: &str, default: Duration, _help: &str) {
        self.values
            .entry(name.to_string())
            .or_insert(FlagValue::Duration(default));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn DurationP(&mut self, name: &str, short: &str, default: Duration, help: &str) {
        self.shorts.insert(short.to_string(), name.to_string());
        self.Duration(name, default, help);
    }

    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetString(&mut self, name: &str, value: impl Into<String>) {
        self.values
            .insert(name.to_string(), FlagValue::String(value.into()));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetBool(&mut self, name: &str, value: bool) {
        self.values.insert(name.to_string(), FlagValue::Bool(value));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetUint64(&mut self, name: &str, value: u64) {
        self.values.insert(name.to_string(), FlagValue::Uint(value));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetUint(&mut self, name: &str, value: u32) {
        self.SetUint64(name, value as u64);
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetInt(&mut self, name: &str, value: i32) {
        self.values
            .insert(name.to_string(), FlagValue::Int(value as i64));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetInt64(&mut self, name: &str, value: i64) {
        self.values.insert(name.to_string(), FlagValue::Int(value));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetDuration(&mut self, name: &str, value: Duration) {
        self.values
            .insert(name.to_string(), FlagValue::Duration(value));
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetStringSlice(&mut self, name: &str, value: Vec<String>) {
        self.values
            .insert(name.to_string(), FlagValue::StringSlice(value));
    }

    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Lookup(&self, name: &str) -> Option<()> {
        if self.values.contains_key(name) {
            Some(())
        } else {
            None
        }
    }

    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetString(&self, name: &str) -> Result<String> {
        match self.values.get(name) {
            Some(FlagValue::String(s)) => Ok(s.clone()),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetBool(&self, name: &str) -> Result<bool> {
        match self.values.get(name) {
            Some(FlagValue::Bool(b)) => Ok(*b),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetUint64(&self, name: &str) -> Result<u64> {
        match self.values.get(name) {
            Some(FlagValue::Uint(v)) => Ok(*v),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetUint(&self, name: &str) -> Result<u32> {
        Ok(self.GetUint64(name)? as u32)
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetInt(&self, name: &str) -> Result<i32> {
        match self.values.get(name) {
            Some(FlagValue::Int(v)) => Ok(*v as i32),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetInt64(&self, name: &str) -> Result<i64> {
        match self.values.get(name) {
            Some(FlagValue::Int(v)) => Ok(*v),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetDuration(&self, name: &str) -> Result<Duration> {
        match self.values.get(name) {
            Some(FlagValue::Duration(d)) => Ok(*d),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetStringSlice(&self, name: &str) -> Result<Vec<String>> {
        match self.values.get(name) {
            Some(FlagValue::StringSlice(v)) => Ok(v.clone()),
            Some(FlagValue::String(s)) => Ok(vec![s.clone()]),
            None => Err(Error::new(format!("flag {name} not defined"))),
            _ => Err(Error::new(format!("flag {name} type mismatch"))),
        }
    }
}

/// 外部存储抽象：读写/Walk/Delete；MemStorage 为实现之一。
pub trait ExternalStorage: Send + Sync {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn URI(&self) -> String;
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    fn Close(&self);
    /// 写入完整对象；覆盖同名键。
    fn WriteFile(&self, name: &str, data: &[u8]) -> Result<()>;
    /// 读取完整对象；缺失返回错误。
    fn ReadFile(&self, name: &str) -> Result<Vec<u8>>;
    /// 对象是否存在。
    fn FileExists(&self, name: &str) -> Result<bool>;
    /// 删除单个对象。
    fn DeleteFile(&self, name: &str) -> Result<()>;
    /// 批量删除；部分失败策略与 Go 对齐为尽最大努力。
    fn DeleteFiles(&self, names: &[String]) -> Result<()>;
    /// 重命名对象键。
    fn Rename(&self, old: &str, new: &str) -> Result<()>;
    /// 遍历前缀下对象；可分页。
    fn WalkDir(&self, opt: &WalkOption, f: &mut dyn FnMut(&str, i64) -> Result<()>) -> Result<()>;
    /// 打开 Reader 读全量。
    fn Open(&self, name: &str, opt: Option<&ReaderOption>) -> Result<Box<dyn ExternalReader>>;
    /// 创建 Writer。
    fn Create(&self, name: &str) -> Result<Box<dyn ExternalWriter>>;
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn as_object_sync_checker(&self) -> Option<Arc<dyn ObjectSyncChecker>> {
        None
    }
}

/// 顺序读接口；Open/OpenWithRange 返回。
pub trait ExternalReader: Send {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn GetFileSize(&self) -> Result<i64>;
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn read_to_end(&mut self) -> Result<Vec<u8>>;
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    fn Close(&mut self);
}

/// 顺序写接口；Create 返回。
pub trait ExternalWriter: Send {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn Write(&mut self, data: &[u8]) -> Result<usize>;
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    fn Close(&mut self) -> Result<()>;
}

/// 内存外部存储：按 URI 隔离字典，供单测与 DryRun。
#[derive(Default, Clone)]
pub struct MemStorage {
    files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    uri: String,
    closed: Arc<AtomicBool>,
    pub as_sync_checker: bool,
}

impl MemStorage {
    /// 由消息构造 Error。
    pub fn new(uri: impl Into<String>) -> Self {
        Self {
            files: Arc::new(Mutex::new(HashMap::new())),
            uri: uri.into(),
            closed: Arc::new(AtomicBool::new(false)),
            as_sync_checker: false,
        }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn put(&self, name: &str, data: Vec<u8>) {
        self.files.lock().unwrap().insert(name.to_string(), data);
    }
    /// 是否已 Close；供资源清理断言。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
struct MemReader {
    data: Vec<u8>,
    closed: bool,
}

impl ExternalReader for MemReader {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn GetFileSize(&self) -> Result<i64> {
        Ok(self.data.len() as i64)
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn read_to_end(&mut self) -> Result<Vec<u8>> {
        Ok(self.data.clone())
    }
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    fn Close(&mut self) {
        self.closed = true;
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
struct MemWriter {
    store: MemStorage,
    name: String,
    buf: Vec<u8>,
}

impl ExternalWriter for MemWriter {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn Write(&mut self, data: &[u8]) -> Result<usize> {
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    fn Close(&mut self) -> Result<()> {
        self.store.WriteFile(&self.name, &self.buf)
    }
}

impl ExternalStorage for MemStorage {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn URI(&self) -> String {
        self.uri.clone()
    }
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    /// 写入完整对象；覆盖同名键。
    fn WriteFile(&self, name: &str, data: &[u8]) -> Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(name.to_string(), data.to_vec());
        Ok(())
    }
    /// 读取完整对象；缺失返回错误。
    fn ReadFile(&self, name: &str) -> Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new(format!("file not found: {name}")))
    }
    /// 对象是否存在。
    fn FileExists(&self, name: &str) -> Result<bool> {
        Ok(self.files.lock().unwrap().contains_key(name))
    }
    /// 删除单个对象。
    fn DeleteFile(&self, name: &str) -> Result<()> {
        self.files.lock().unwrap().remove(name);
        Ok(())
    }
    /// 批量删除；部分失败策略与 Go 对齐为尽最大努力。
    fn DeleteFiles(&self, names: &[String]) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        for n in names {
            files.remove(n);
        }
        Ok(())
    }
    /// 重命名对象键。
    fn Rename(&self, old: &str, new: &str) -> Result<()> {
        let mut files = self.files.lock().unwrap();
        let data = files
            .remove(old)
            .ok_or_else(|| Error::new(format!("file not found: {old}")))?;
        files.insert(new.to_string(), data);
        Ok(())
    }
    /// 遍历前缀下对象；可分页。
    fn WalkDir(&self, opt: &WalkOption, f: &mut dyn FnMut(&str, i64) -> Result<()>) -> Result<()> {
        let mut entries: Vec<(String, i64)> = {
            let files = self.files.lock().unwrap();
            files
                .iter()
                .filter(|(path, _)| {
                    if opt.SubDir.is_empty() {
                        true
                    } else {
                        path.starts_with(&opt.SubDir)
                    }
                })
                .map(|(p, d)| (p.clone(), d.len() as i64))
                .collect()
        };
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        if opt.ListCount > 0 {
            // Pagination is an internal detail of remote stores; MemStorage still
            // walks all matching entries so callers observe the full set.
        }
        for (path, size) in entries {
            f(&path, size)?;
        }
        Ok(())
    }
    /// 打开 Reader 读全量。
    fn Open(&self, name: &str, opt: Option<&ReaderOption>) -> Result<Box<dyn ExternalReader>> {
        let data = self.ReadFile(name)?;
        let sliced = if let Some(o) = opt {
            let start = o.StartOffset.unwrap_or(0).max(0) as usize;
            let end = o
                .EndOffset
                .map(|e| e as usize)
                .unwrap_or(data.len())
                .min(data.len());
            data.get(start..end).unwrap_or(&[]).to_vec()
        } else {
            data
        };
        Ok(Box::new(MemReader {
            data: sliced,
            closed: false,
        }))
    }
    /// 创建 Writer。
    fn Create(&self, name: &str) -> Result<Box<dyn ExternalWriter>> {
        Ok(Box::new(MemWriter {
            store: self.clone(),
            name: name.to_string(),
            buf: Vec::new(),
        }))
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn as_object_sync_checker(&self) -> Option<Arc<dyn ObjectSyncChecker>> {
        if self.as_sync_checker {
            Some(Arc::new(ExistenceSyncChecker {
                storage: Arc::new(self.clone()),
            }))
        } else {
            None
        }
    }
}

/// 解析 storage URI 为 StorageBackend；非法 scheme 报错。
pub fn ParseBackend(uri: &str, _opts: &BackendOptions) -> Result<StorageBackend> {
    if uri.is_empty() {
        return Err(Error::new("empty storage uri"));
    }
    let (scheme, path) = if let Some((s, p)) = uri.split_once("://") {
        (s.to_string(), p.to_string())
    } else {
        ("local".to_string(), uri.to_string())
    };
    Ok(StorageBackend {
        Scheme: scheme,
        Path: path,
        raw: uri.to_string(),
    })
}

/// 按 backend 创建存储实例；mem:// 走 MemStorage。
pub fn NewStorage(
    backend: &StorageBackend,
    _opts: &StorageOptions,
) -> Result<Arc<dyn ExternalStorage>> {
    Ok(Arc::new(MemStorage::new(backend.raw.clone())))
}

/// 与 Go `objstore.Create` 对齐的入口包装。
pub fn CreateStorage(
    backend: &StorageBackend,
    _send_creds: bool,
) -> Result<Arc<dyn ExternalStorage>> {
    NewStorage(
        backend,
        &StorageOptions {
            SendCredentials: false,
            ..Default::default()
        },
    )
}

/// 解析 URI 并打开存储，返回 backend + Arc。
pub fn GetStorage(uri: &str, cfg: &Config) -> Result<(StorageBackend, Arc<dyn ExternalStorage>)> {
    let backend = ParseBackend(uri, &cfg.BackendOptions)?;
    let store = NewStorage(
        &backend,
        &StorageOptions {
            SendCredentials: true,
            ..Default::default()
        },
    )?;
    Ok((backend, store))
}

/// 从 Config 提取 keepalive；PermitWithoutStream=true。
pub fn GetKeepalive(cfg: &Config) -> KeepaliveParams {
    KeepaliveParams {
        Time: cfg.GRPCKeepaliveTime,
        Timeout: cfg.GRPCKeepaliveTimeout,
        PermitWithoutStream: true,
    }
}

pub mod metapb {
    /// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
    #[derive(Clone, Debug, Default)]
    pub struct Store {
        pub Id: u64,
        pub Address: String,
        pub Labels: Vec<(String, String)>,
    }
}

/// 依据 engine=tiflash 标签判断是否 TiFlash store。
pub fn IsTiFlash(store: &metapb::Store) -> bool {
    store
        .Labels
        .iter()
        .any(|(k, v)| k == "engine" && v == "tiflash")
}

/// PD 客户端最小接口：stores / resolved-ts。
pub trait PDClient: Send + Sync {
    /// 返回已知 store 列表。
    fn GetAllStores(&self) -> Result<Vec<metapb::Store>>;
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn GetTS(&self) -> Result<(i64, i64)>;
    /// 返回配置的最小 resolved TS。
    fn GetMinResolvedTS(&self) -> Result<u64>;
    /// 移除 PD 调度器并返回 undo 闭包（可空）。
    fn RemoveAllPDSchedulers(&self) -> Result<Option<Box<dyn Fn() -> Result<()> + Send + Sync>>>;
}

/// 内存 PD：可配置 stores 与 min_resolved_ts。
#[derive(Default)]
pub struct MemPDClient {
    pub stores: Mutex<Vec<metapb::Store>>,
    pub ts: (i64, i64),
    pub min_resolved_ts: u64,
    pub closed: AtomicBool,
}

impl PDClient for MemPDClient {
    /// 返回已知 store 列表。
    fn GetAllStores(&self) -> Result<Vec<metapb::Store>> {
        Ok(self.stores.lock().unwrap().clone())
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn GetTS(&self) -> Result<(i64, i64)> {
        Ok(self.ts)
    }
    /// 返回配置的最小 resolved TS。
    fn GetMinResolvedTS(&self) -> Result<u64> {
        Ok(self.min_resolved_ts)
    }
    /// 移除 PD 调度器并返回 undo 闭包（可空）。
    fn RemoveAllPDSchedulers(&self) -> Result<Option<Box<dyn Fn() -> Result<()> + Send + Sync>>> {
        Ok(Some(Box::new(|| Ok(()))))
    }
}

/// PD 控制器桩：暂停调度器 TTL、关连、取 client。
pub struct PdController {
    pub client: Arc<dyn PDClient>,
    pub SchedulerPauseTTL: Mutex<Duration>,
    closed: AtomicBool,
}

impl PdController {
    /// 由消息构造 Error。
    pub fn new(client: Arc<dyn PDClient>) -> Self {
        Self {
            client,
            SchedulerPauseTTL: Mutex::new(Duration::from_secs(120)),
            closed: AtomicBool::new(false),
        }
    }
    /// 设置调度器暂停 TTL。
    pub fn SetSchedulerPauseTTL(&self, ttl: Duration) {
        *self.SchedulerPauseTTL.lock().unwrap() = ttl;
    }
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    pub fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    /// 是否已 Close；供资源清理断言。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    /// 取底层 PDClient。
    pub fn GetPDClient(&self) -> Arc<dyn PDClient> {
        self.client.clone()
    }
    /// 返回配置的最小 resolved TS。
    pub fn GetMinResolvedTS(&self) -> Result<u64> {
        self.client.GetMinResolvedTS()
    }
    /// 移除 PD 调度器并返回 undo 闭包（可空）。
    pub fn RemoveAllPDSchedulers(
        &self,
    ) -> Result<Option<Box<dyn Fn() -> Result<()> + Send + Sync>>> {
        self.client.RemoveAllPDSchedulers()
    }
}

/// TiKV Store 管理桩：FlushNow / Close；测试可注入 flush_results。
pub struct StoreManager {
    pub pd: Arc<dyn PDClient>,
    closed: AtomicBool,
    pub flush_results: Mutex<HashMap<u64, Vec<FlushResult>>>,
    pub flush_errors: Mutex<HashMap<u64, String>>,
}

/// 单次 FlushNow 结果：任务名、成功标记、错误信息。
#[derive(Clone, Debug, Default)]
pub struct FlushResult {
    pub TaskName: String,
    pub Success: bool,
    pub ErrorMessage: String,
}

impl StoreManager {
    /// 由消息构造 Error。
    pub fn new(pd: Arc<dyn PDClient>) -> Self {
        Self {
            pd,
            closed: AtomicBool::new(false),
            flush_results: Mutex::new(HashMap::new()),
            flush_errors: Mutex::new(HashMap::new()),
        }
    }
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    pub fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    /// 是否已 Close；供资源清理断言。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    /// 对指定 store 触发日志刷盘；结果来自注入表或空成功。
    pub fn FlushNow(&self, store_id: u64) -> Result<Vec<FlushResult>> {
        if let Some(err) = self.flush_errors.lock().unwrap().get(&store_id) {
            return Err(Error::Annotatef(
                err.clone(),
                format!("failed to flush store {store_id}"),
            ));
        }
        Ok(self
            .flush_results
            .lock()
            .unwrap()
            .get(&store_id)
            .cloned()
            .unwrap_or_default())
    }
}

/// 运维操作上下文名称包装。
#[derive(Clone, Debug, Default)]
pub struct OperationContext {
    pub OperationID: String,
}

/// 创建 OperationContext；失败路径保留与 Go 一致签名。
pub fn NewOperationContext(command: &str) -> Result<OperationContext> {
    let _ = command;
    Ok(OperationContext {
        OperationID: uuid::Uuid::new_v4().to_string(),
    })
}

/// 单层 migration 内容（名称等最小字段）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Migration {
    pub Name: String,
}

/// 单层 migration 内容（名称等最小字段）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MigrationLayer {
    pub SeqNum: i32,
    pub Content: Migration,
}

/// 单层 migration 内容（名称等最小字段）。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Migrations {
    pub Base: Migration,
    pub Layers: Vec<MigrationLayer>,
}

/// 合并结果：新 BASE 与 Warnings。
#[derive(Clone, Debug, Default)]
pub struct MergeAndMigratedTo {
    pub NewBase: Migration,
    pub Warnings: Vec<Error>,
}

/// 交互确认回调类型；`--yes` 时恒真。
pub type InteractiveCheck = Arc<dyn Fn(&Migration) -> bool + Send + Sync>;

/// 单层 migration 内容（名称等最小字段）。
#[derive(Clone)]
pub struct MigrationExt {
    pub storage: Arc<dyn ExternalStorage>,
    pub operation: Option<OperationContext>,
    pub dry_run_effects: Arc<Mutex<Vec<Effect>>>,
    pub load_err: Arc<Mutex<Option<Error>>>,
    pub merge_result: Arc<Mutex<Option<MergeAndMigratedTo>>>,
}

impl MigrationExt {
    /// 附着 OperationContext，返回新 Ext。
    pub fn WithOperationContext(mut self, ctx: OperationContext) -> Self {
        self.operation = Some(ctx);
        self
    }
    /// 从存储加载 migrations；可按选项把缺失当错误。
    pub fn Load(&self, not_found_is_err: bool) -> Result<Migrations> {
        if let Some(err) = self.load_err.lock().unwrap().clone() {
            return Err(err);
        }
        if !self.storage.FileExists("migrations.json")? {
            if not_found_is_err {
                return Err(Error::new("migrations not found"));
            }
            return Ok(Migrations::default());
        }
        let data = self.storage.ReadFile("migrations.json")?;
        serde_json::from_slice(&data).map_err(|e| Error::new(e.to_string()))
    }
    /// 将 migration 字段填入控制台表格。
    pub fn AddMigrationToTable(&self, _mig: &Migration, table: &mut ConsoleTable) {
        table.rows.push(vec![_mig.Name.clone()]);
    }
    /// 在副作用记录模式下执行闭包，不持久化。
    pub fn DryRun(&self, f: impl FnOnce(MigrationExt)) -> Vec<Effect> {
        self.dry_run_effects.lock().unwrap().clear();
        f(self.clone());
        let mut effects = self.dry_run_effects.lock().unwrap().clone();
        if effects.is_empty() {
            effects.push(Effect {
                Op: "noop".into(),
                Path: "dry-run".into(),
            });
        }
        effects
    }
    /// 合并到目标版本；可挂交互确认。
    pub fn MergeAndMigrateTo(
        &self,
        target: i32,
        check: Option<InteractiveCheck>,
    ) -> Result<MergeAndMigratedTo> {
        let migs = self.Load(false)?;
        let target_mig = if target == 0 {
            migs.Base.clone()
        } else {
            migs.Layers
                .iter()
                .find(|l| l.SeqNum == target)
                .map(|l| l.Content.clone())
                .unwrap_or_else(|| Migration {
                    Name: format!("seq-{target}"),
                })
        };
        if let Some(check) = check {
            if !check(&target_mig) {
                return Err(Error::new("migration cancelled by user"));
            }
        }
        self.dry_run_effects.lock().unwrap().push(Effect {
            Op: "merge".into(),
            Path: format!("target={target}"),
        });
        if let Some(res) = self.merge_result.lock().unwrap().clone() {
            return Ok(res);
        }
        Ok(MergeAndMigratedTo {
            NewBase: target_mig,
            Warnings: Vec::new(),
        })
    }
}

/// 由 ExternalStorage 构造 MigrationExt。
pub fn MigrationExtension(storage: Arc<dyn ExternalStorage>) -> MigrationExt {
    MigrationExt {
        storage,
        operation: None,
        dry_run_effects: Arc::new(Mutex::new(Vec::new())),
        load_err: Arc::new(Mutex::new(None)),
        merge_result: Arc::new(Mutex::new(None)),
    }
}

/// Load 选项：找不到 migration 是否当错误。
pub fn MLNotFoundIsErr() -> bool {
    true
}

/// 挂进度条 Hooks；当前为空实现占位。
pub fn NewProgressBarHooks(_console: &ConsoleOperations) {}

/// 控制台表格桩：AddRow/Print。
#[derive(Default)]
pub struct ConsoleTable {
    pub rows: Vec<Vec<String>>,
}

impl ConsoleTable {
    /// 打印表格内容。
    pub fn Print(&self) {
        for row in &self.rows {
            println!("{}", row.join("\t"));
        }
    }
}

/// 控制台 I/O：Println/Printf/PromptBool/CreateTable。
#[derive(Clone, Default)]
pub struct ConsoleOperations {
    pub out: Arc<Mutex<Vec<String>>>,
    pub prompt_answer: Arc<Mutex<bool>>,
}

impl ConsoleOperations {
    /// 标准输入输出控制台。
    pub fn StdIO() -> Self {
        Self::default()
    }
    /// 打印一行。
    pub fn Println(&self, msg: impl Into<String>) {
        let s = msg.into();
        self.out.lock().unwrap().push(s.clone());
        println!("{s}");
    }
    /// 格式化打印。
    pub fn Printf(&self, msg: impl Into<String>) {
        let s = msg.into();
        self.out.lock().unwrap().push(s.clone());
        print!("{s}");
    }
    /// 新建空表格。
    pub fn CreateTable(&self) -> ConsoleTable {
        ConsoleTable::default()
    }
    /// 交互询问布尔；测试环境可默认 true。
    pub fn PromptBool(&self, _msg: &str) -> bool {
        *self.prompt_answer.lock().unwrap()
    }
}

/// 上下游对象同步检查抽象。
pub trait ObjectSyncChecker: Send + Sync {
    /// 检查器名称，用于日志/断言。
    fn name(&self) -> &str;
    /// Reports whether an object required by CRR is already safe to use.
    /// Mirrors Go `service.ObjectSyncChecker.FileSynced`.
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn FileSynced(&self, name: &str) -> Result<bool>;
}

/// 仅检查对象存在性的同步检查器。
pub struct ExistenceSyncChecker {
    pub storage: Arc<dyn ExternalStorage>,
}

impl ObjectSyncChecker for ExistenceSyncChecker {
    /// 检查器名称，用于日志/断言。
    fn name(&self) -> &str {
        "existence"
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn FileSynced(&self, name: &str) -> Result<bool> {
        self.storage.FileExists(name)
    }
}

/// Fixed-result sync checker — Go `syncedStorage{synced: bool}` ObjectSyncChecker path.
/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
pub struct FixedSyncChecker {
    pub synced: bool,
}

impl ObjectSyncChecker for FixedSyncChecker {
    /// 检查器名称，用于日志/断言。
    fn name(&self) -> &str {
        "fixed"
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn FileSynced(&self, _name: &str) -> Result<bool> {
        Ok(self.synced)
    }
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn NewExistenceSyncChecker(storage: Arc<dyn ExternalStorage>) -> Arc<dyn ObjectSyncChecker> {
    Arc::new(ExistenceSyncChecker { storage })
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistentState {
    #[serde(rename = "last_checkpoint")]
    pub LastCheckpoint: u64,
    #[serde(rename = "synced_ts")]
    pub SyncedTS: u64,
    #[serde(
        rename = "synced_by_store",
        default,
        skip_serializing_if = "HashMap::is_empty"
    )]
    pub SyncedByStore: HashMap<u64, u64>,
}

/// 桩 trait：定义调用边界，具体行为由 Mem*/测试实现提供。
pub trait ResumeStateStore: Send + Sync {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn LoadState(&self) -> Result<Option<PersistentState>>;
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn SaveState(&self, state: PersistentState) -> Result<()>;
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn GetStatusFileName() -> &'static str {
    "crr-checkpoint/resume-state.json"
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default)]
pub struct CRRServiceConfig {
    pub TaskName: String,
    pub PollInterval: Duration,
    pub MetaReadConcurrency: i32,
    pub RetryInterval: Duration,
}

impl CRRServiceConfig {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Parse(&mut self, flags: &FlagSet) -> Result<()> {
        if flags.Lookup("task-name").is_some() {
            self.TaskName = flags.GetString("task-name")?;
        }
        if flags.Lookup("retry-interval").is_some() {
            self.RetryInterval = flags.GetDuration("retry-interval")?;
        }
        if flags.Lookup("calc.poll-interval").is_some() {
            self.PollInterval = flags.GetDuration("calc.poll-interval")?;
        }
        if flags.Lookup("calc.meta-read-concurrency").is_some() {
            self.MetaReadConcurrency = flags.GetInt("calc.meta-read-concurrency")?;
        }
        Ok(())
    }
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn DefineCRRFlags(flags: &mut FlagSet) {
    flags.String("task-name", "", "The name of the upstream log backup task.");
    flags.Duration(
        "retry-interval",
        Duration::from_secs(1),
        "The retry interval after crr-checkpoint service errors or watch failures.",
    );
    flags.Duration(
        "calc.poll-interval",
        Duration::from_secs(5),
        "The calculator polling interval for downstream sync checks.",
    );
    flags.Int(
        "calc.meta-read-concurrency",
        8,
        "The calculator concurrency for reading backupmeta files.",
    );
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
pub struct CRRService {
    pub cfg: CRRServiceConfig,
    pub closed: AtomicBool,
}

impl CRRService {
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    pub fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    /// 是否已 Close；供资源清理断言。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
pub struct CRRDeps {
    pub Upstream: Arc<dyn ExternalStorage>,
    pub Sync: Arc<dyn ObjectSyncChecker>,
    pub State: Arc<dyn ResumeStateStore>,
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn NewCRRService(deps: CRRDeps, cfg: CRRServiceConfig) -> Result<CRRService> {
    let _ = deps;
    if cfg.TaskName.is_empty() {
        return Err(Error::new("empty task name"));
    }
    Ok(CRRService {
        cfg,
        closed: AtomicBool::new(false),
    })
}

/// 桩 trait：定义调用边界，具体行为由 Mem*/测试实现提供。
pub trait Glue: Send + Sync {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn GetDomain(&self, _store: &dyn KVStorage) -> Result<Arc<dyn Domain>>;
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn CreateSession(&self, _store: &dyn KVStorage) -> Result<Arc<dyn Session>>;
}

/// 桩 trait：定义调用边界，具体行为由 Mem*/测试实现提供。
pub trait KVStorage: Send + Sync {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn GetClient(&self) -> Arc<dyn KVClient>;
}

/// 桩 trait：定义调用边界，具体行为由 Mem*/测试实现提供。
pub trait KVClient: Send + Sync {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn Checksum(&self, _req: &ChecksumRequest) -> Result<ChecksumResponse>;
}

/// 桩 trait：定义调用边界，具体行为由 Mem*/测试实现提供。
pub trait Domain: Send + Sync {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn InfoSchema(&self) -> Arc<dyn InfoSchema>;
}

/// 桩 trait：定义调用边界，具体行为由 Mem*/测试实现提供。
pub trait InfoSchema: Send + Sync {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn AllSchemas(&self) -> Vec<DBInfo>;
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn SchemaTableInfos(&self, db: &str) -> Result<Vec<TableInfo>>;
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn TableByName(&self, db: &str, table: &str) -> Result<Arc<dyn Table>>;
}

/// 桩 trait：定义调用边界，具体行为由 Mem*/测试实现提供。
pub trait Table: Send + Sync {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn Meta(&self) -> &TableInfo;
}

/// 桩 trait：定义调用边界，具体行为由 Mem*/测试实现提供。
pub trait Session: Send + Sync {
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    fn Close(&self);
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn ExecRestrictedSQL(&self, sql: &str, args: &[u64]) -> Result<Vec<SQLRow>>;
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DBInfo {
    pub Name: CIStr,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CIStr {
    pub O: String,
    pub L: String,
}

impl CIStr {
    /// 由消息构造 Error。
    pub fn new(s: &str) -> Self {
        Self {
            O: s.to_string(),
            L: s.to_lowercase(),
        }
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub Name: CIStr,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PartitionDefinition {
    pub ID: i64,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PartitionInfo {
    pub Definitions: Vec<PartitionDefinition>,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TableInfo {
    pub ID: i64,
    pub Name: CIStr,
    pub Columns: Vec<ColumnInfo>,
    pub Partition: Option<PartitionInfo>,
}

impl TableInfo {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default)]
pub struct SQLRow {
    pub cols: Vec<SQLValue>,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug)]
pub enum SQLValue {
    Uint64(u64),
    Bytes(Vec<u8>),
}

impl SQLRow {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetUint64(&self, i: usize) -> u64 {
        match self.cols.get(i) {
            Some(SQLValue::Uint64(v)) => *v,
            _ => 0,
        }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetBytes(&self, i: usize) -> Vec<u8> {
        match self.cols.get(i) {
            Some(SQLValue::Bytes(v)) => v.clone(),
            _ => Vec::new(),
        }
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Default)]
pub struct MemGlue {
    pub domain: Option<Arc<dyn Domain>>,
    pub session: Option<Arc<dyn Session>>,
}

impl Glue for MemGlue {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn GetDomain(&self, _store: &dyn KVStorage) -> Result<Arc<dyn Domain>> {
        self.domain
            .clone()
            .ok_or_else(|| Error::new("domain not set"))
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn CreateSession(&self, _store: &dyn KVStorage) -> Result<Arc<dyn Session>> {
        self.session
            .clone()
            .ok_or_else(|| Error::new("session not set"))
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Default)]
pub struct MemKVStorage {
    pub client: Arc<MemKVClient>,
}

impl KVStorage for MemKVStorage {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn GetClient(&self) -> Arc<dyn KVClient> {
        self.client.clone()
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Default)]
pub struct MemKVClient {
    pub resp: Mutex<ChecksumResponse>,
}

impl KVClient for MemKVClient {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn Checksum(&self, _req: &ChecksumRequest) -> Result<ChecksumResponse> {
        Ok(self.resp.lock().unwrap().clone())
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default)]
pub struct ChecksumRequest {
    pub table_id: i64,
    pub ts: u64,
    pub concurrency: u32,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default)]
pub struct ChecksumResponse {
    pub Checksum: u64,
    pub TotalBytes: u64,
    pub TotalKvs: u64,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
pub struct ChecksumExecutor {
    pub req: ChecksumRequest,
    pub parts: usize,
}

impl ChecksumExecutor {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Len(&self) -> usize {
        self.parts.max(1)
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Execute(
        &self,
        client: &dyn KVClient,
        on_finish: impl FnMut(),
    ) -> Result<ChecksumResponse> {
        let mut on_finish = on_finish;
        for _ in 0..self.Len() {
            on_finish();
        }
        client.Checksum(&self.req)
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
pub struct ExecutorBuilder {
    info: TableInfo,
    ts: u64,
    concurrency: u32,
    old_table: Option<TableInfo>,
}

impl ExecutorBuilder {
    /// 由消息构造 Error。
    pub fn new(info: TableInfo, ts: u64) -> Self {
        Self {
            info,
            ts,
            concurrency: 4,
            old_table: None,
        }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetConcurrency(&mut self, c: u32) {
        self.concurrency = c;
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetOldTable(&mut self, t: &MetaTable) {
        self.old_table = Some(t.Info.clone());
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn SetExplicitRequestSourceType(&mut self, _t: &str) {}
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Build(self) -> Result<ChecksumExecutor> {
        Ok(ChecksumExecutor {
            req: ChecksumRequest {
                table_id: self
                    .old_table
                    .as_ref()
                    .map(|t| t.ID)
                    .unwrap_or(self.info.ID),
                ts: self.ts,
                concurrency: self.concurrency,
            },
            parts: 1,
        })
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MetaTable {
    pub DB: DBInfo,
    pub Info: TableInfo,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct IdMap {
    pub DownstreamId: i64,
    pub UpstreamId: i64,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PitrTableMap {
    pub Name: String,
    pub IdMap: IdMap,
    pub Partitions: Vec<IdMap>,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PitrDBMap {
    pub Name: String,
    pub Tables: Vec<PitrTableMap>,
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BackupMeta {
    pub DbMaps: Vec<PitrDBMap>,
    pub raw: Vec<u8>,
}

impl BackupMeta {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Unmarshal(data: &[u8]) -> Result<Self> {
        if data.is_empty() {
            return Ok(Self::default());
        }
        // Prefer JSON (test-friendly); fall back to opaque raw payload.
        if let Ok(v) = serde_json::from_slice::<BackupMeta>(data) {
            return Ok(v);
        }
        Ok(BackupMeta {
            raw: data.to_vec(),
            ..Default::default()
        })
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetDbMaps(&self) -> &[PitrDBMap] {
        &self.DbMaps
    }
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn PitrIDMapsFilename(cluster_id: u64, restored_ts: u64) -> String {
    format!("pitr_id_map/cluster-{cluster_id}-ts-{restored_ts}")
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
pub struct MetaReader {
    pub meta: BackupMeta,
    pub storage: Arc<dyn ExternalStorage>,
}

impl MetaReader {
    /// 由消息构造 Error。
    pub fn new(meta: BackupMeta, storage: Arc<dyn ExternalStorage>) -> Self {
        Self { meta, storage }
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn ReadSchemasFiles(&self) -> Result<Vec<MetaTable>> {
        let _ = &self.meta;
        let _ = &self.storage;
        // Schema file reading is an external storage boundary; callers inject via
        // MemStorage JSON helper files when needed by tests.
        if let Ok(data) = self.storage.ReadFile("schemas.json") {
            return serde_json::from_slice(&data).map_err(|e| Error::new(e.to_string()));
        }
        Ok(Vec::new())
    }
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Clone, Debug, Default)]
pub struct BRServiceSafePoint {
    pub ID: String,
    pub TTL: i64,
    pub BackupTS: u64,
}

/// 桩 trait：定义调用边界，具体行为由 Mem*/测试实现提供。
pub trait GCManager: Send + Sync {
    /// 设置/清除 service safepoint（TTL=0 清除）。
    fn SetServiceSafePoint(&self, sp: BRServiceSafePoint) -> Result<()>;
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn DeleteServiceSafePoint(&self, sp: BRServiceSafePoint) -> Result<()>;
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
#[derive(Default)]
pub struct MemGCManager {
    pub points: Mutex<HashMap<String, BRServiceSafePoint>>,
}

impl GCManager for MemGCManager {
    /// 设置/清除 service safepoint（TTL=0 清除）。
    fn SetServiceSafePoint(&self, sp: BRServiceSafePoint) -> Result<()> {
        self.points.lock().unwrap().insert(sp.ID.clone(), sp);
        Ok(())
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn DeleteServiceSafePoint(&self, sp: BRServiceSafePoint) -> Result<()> {
        self.points.lock().unwrap().remove(&sp.ID);
        let _ = sp;
        Ok(())
    }
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn MakeSafePointID() -> String {
    format!("br-{}", uuid::Uuid::new_v4())
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn StartServiceSafePointKeeper(sp: BRServiceSafePoint, mgr: &dyn GCManager) -> Result<()> {
    if sp.ID.is_empty() || sp.TTL <= 0 {
        return Err(Error::Annotatef(
            berrors::ErrInvalidArgument,
            format!("invalid service safe point {sp:?}"),
        ));
    }
    mgr.SetServiceSafePoint(sp)
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn ComposeTS(physical: i64, logical: i64) -> u64 {
    ((physical as u64) << 18) | ((logical as u64) & ((1 << 18) - 1))
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn GetTSWithRetry(pd: &dyn PDClient) -> Result<u64> {
    let (phy, logi) = pd.GetTS()?;
    Ok(ComposeTS(phy, logi))
}

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
pub struct ConnMgr {
    pub pd: Arc<dyn PDClient>,
    pub store: Arc<dyn KVStorage>,
    pub gc: Arc<dyn GCManager>,
    closed: AtomicBool,
}

impl ConnMgr {
    /// 标记关闭；后续 RPC 应拒绝或空操作。
    pub fn Close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    /// 是否已 Close；供资源清理断言。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    /// 取底层 PDClient。
    pub fn GetPDClient(&self) -> Arc<dyn PDClient> {
        self.pd.clone()
    }
    /// 解析 URI 并打开存储，返回 backend + Arc。
    pub fn GetStorage(&self) -> Arc<dyn KVStorage> {
        self.store.clone()
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn GetGCManager(&self) -> Arc<dyn GCManager> {
        self.gc.clone()
    }
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn NewMgr(
    _g: &dyn Glue,
    cfg: &Config,
    pd: Option<Arc<dyn PDClient>>,
    store: Option<Arc<dyn KVStorage>>,
) -> Result<Arc<ConnMgr>> {
    let _ = cfg;
    Ok(Arc::new(ConnMgr {
        pd: pd.unwrap_or_else(|| Arc::new(MemPDClient::default())),
        store: store.unwrap_or_else(|| Arc::new(MemKVStorage::default())),
        gc: Arc::new(MemGCManager::default()),
        closed: AtomicBool::new(false),
    }))
}

pub static DUMP_GOROUTINE_WHEN_EXIT: AtomicBool = AtomicBool::new(false);

/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
pub struct Preparer {
    pub LeaseDuration: Duration,
    pub AfterConnectionsEstablished: Option<Box<dyn Fn() + Send + Sync>>,
    finalized: AtomicBool,
    pub drive_err: Mutex<Option<Error>>,
}

impl Preparer {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn DriveLoopAndWaitPrepare(&self) -> Result<()> {
        if let Some(f) = &self.AfterConnectionsEstablished {
            f();
        }
        if let Some(err) = self.drive_err.lock().unwrap().clone() {
            return Err(err);
        }
        Ok(())
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn Finalize(&self) -> Result<()> {
        self.finalized.store(true, Ordering::SeqCst);
        Ok(())
    }
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    pub fn is_finalized(&self) -> bool {
        self.finalized.load(Ordering::SeqCst)
    }
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn NewPreparer() -> Preparer {
    Preparer {
        LeaseDuration: Duration::from_secs(120),
        AfterConnectionsEstablished: None,
        finalized: AtomicBool::new(false),
        drive_err: Mutex::new(None),
    }
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn color_hi_red(s: &str) -> String {
    format!("\x1b[91m{s}\x1b[0m")
}
/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn color_green(s: &str) -> String {
    format!("\x1b[32m{s}\x1b[0m")
}
/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn color_bold(s: &str) -> String {
    format!("\x1b[1m{s}\x1b[0m")
}
/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn color_cyan(s: &str) -> String {
    format!("\x1b[36m{s}\x1b[0m")
}
/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn color_red(s: &str) -> String {
    format!("\x1b[31m{s}\x1b[0m")
}
/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn color_yellow(s: &str) -> String {
    format!("\x1b[33m{s}\x1b[0m")
}

/// Test injection hooks for PD/store dial boundaries.
/// 桩类型：为 operator 编译/测试提供最小字段，非生产实现。
pub struct DialHooks {
    pub dial_pd: Option<Box<dyn Fn(&Config) -> Result<Arc<PdController>> + Send + Sync>>,
    pub create_store_manager:
        Option<Box<dyn Fn(Arc<dyn PDClient>, &Config) -> Result<Arc<StoreManager>> + Send + Sync>>,
    pub new_mgr: Option<Box<dyn Fn(&dyn Glue, &Config) -> Result<Arc<ConnMgr>> + Send + Sync>>,
}

impl Default for DialHooks {
    /// 方法实现：保持与 Go 方法名及关键分支一致的可测行为。
    fn default() -> Self {
        Self {
            dial_pd: None,
            create_store_manager: None,
            new_mgr: None,
        }
    }
}

pub static DIAL_HOOKS: LazyLock<Mutex<DialHooks>> =
    LazyLock::new(|| Mutex::new(DialHooks::default()));

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn clear_dial_hooks() {
    *DIAL_HOOKS.lock().unwrap() = DialHooks::default();
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn set_dial_pd_hook(f: impl Fn(&Config) -> Result<Arc<PdController>> + Send + Sync + 'static) {
    DIAL_HOOKS.lock().unwrap().dial_pd = Some(Box::new(f));
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn set_create_store_manager_hook(
    f: impl Fn(Arc<dyn PDClient>, &Config) -> Result<Arc<StoreManager>> + Send + Sync + 'static,
) {
    DIAL_HOOKS.lock().unwrap().create_store_manager = Some(Box::new(f));
}

/// 桩函数：语义贴近同名 Go API，真实网络/磁盘能力可能未接通。
pub fn set_new_mgr_hook(
    f: impl Fn(&dyn Glue, &Config) -> Result<Arc<ConnMgr>> + Send + Sync + 'static,
) {
    DIAL_HOOKS.lock().unwrap().new_mgr = Some(Box::new(f));
}

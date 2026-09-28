// Copyright 2026 AsterSQL.
//! Local stand-ins for task / metautil / logutil / objstore / oracle / kvproto
//! boundaries (darwin arm64-safe; no heavy workspace deps).
//!
//! show 子命令本地桩模块：在 darwin arm64 等轻量构建下替代真实 workspace 依赖。
//! 占位能力：`Context`、`Error`、`MetaReader`、`ReadBackupMeta` hook、内存 Storage 等。
//! 真实边界未实现：kvproto 解码、对象存储 IO、metautil V2 schema 遍历、真实 PD/oracle。
//! 测试通过 hook 与 `MemStorage::with_tables` 注入数据，勿将桩行为当作生产已支持能力。
//! 公开符号命名与 Go 包对齐（PascalCase 字段、`ReadSchemasFiles` 等），便于 parity 对照。
//! `ReadBackupMeta` 无 hook 时返回配置错误，不代表真实 storage 后端可用。
//! 本模块仅供 show 包单元/parity 测试链接，不可作为生产 BR 依赖注入实现。
//! thread_local hook 非 Send：并行测试须各自清理，避免跨用例串扰。

use std::cell::RefCell;
#[cfg(unix)]
use std::ffi::{CStr, c_char, c_long, c_void};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 桩层统一 Result 别名，错误类型为本地 `Error`（非真实 berrors 栈）。
pub type Result<T> = std::result::Result<T, Error>;

/// 轻量错误载体，模拟 Go errors / berrors 的 msg 与 Annotate 链式包装。
/// 不含 error code、RFC 码或 metrics 标签；仅保证 Display 与 parity 断言可读。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub msg: String,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// 对应 Go `errors.Annotate`：在原有 msg 前追加上下文前缀。
    pub fn Annotate(err: Self, ctx: impl Into<String>) -> Self {
        Self {
            msg: format!("{}: {}", ctx.into(), err.msg),
        }
    }

    /// 桩实现：Trace 为恒等，不收集真实调用栈。
    pub fn Trace(err: Self) -> Self {
        err
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// Cancellation token approximating Go `context.Context`.
/// 近似 Go `context.Context` 的取消令牌；无 deadline/value，仅支持 cancel 传播。
#[derive(Clone, Default)]
pub struct Context {
    cancelled: Arc<Mutex<Option<Error>>>,
    parent: Option<Arc<Context>>,
}

impl Context {
    /// 对应 `context.Background()`：永不主动取消的根上下文。
    pub fn Background() -> Self {
        Self::default()
    }

    /// 对应 `context.WithCancel`：子上下文动态继承父级 Err，CancelFunc 触发本地 canceled。
    pub fn WithCancel(parent: &Self) -> (Self, CancelFunc) {
        let child = Self {
            cancelled: Arc::new(Mutex::new(None)),
            parent: Some(Arc::new(parent.clone())),
        };
        let cancel = CancelFunc {
            cancelled: child.cancelled.clone(),
        };
        (child, cancel)
    }

    pub fn cancel(&self, err: Error) {
        *self.cancelled.lock().unwrap() = Some(err);
    }

    /// 若已取消则返回 Some(Error)，否则 None。
    pub fn Err(&self) -> Option<Error> {
        self.cancelled
            .lock()
            .unwrap()
            .clone()
            .or_else(|| self.parent.as_ref().and_then(|parent| parent.Err()))
    }

    /// 对应 Go `ctx.Done()` 语义：Err 非空即视为 done。
    pub fn Done(&self) -> bool {
        self.Err().is_some()
    }
}

#[derive(Clone)]
pub struct CancelFunc {
    cancelled: Arc<Mutex<Option<Error>>>,
}

impl CancelFunc {
    /// 触发取消，错误文案固定为 "context canceled"（与 Go 默认一致）。
    pub fn cancel(self) {
        *self.cancelled.lock().unwrap() = Some(Error::new("context canceled"));
    }
}

pub mod berrors {
    use super::Error;

    /// Mirrors `berrors.ErrInvalidMetaFile.GenWithStackByArgs(...)`.
    /// 桩层 invalid metafile 工厂；真实 berrors 会附带 stack，此处仅格式化 msg。
    pub fn ErrInvalidMetaFile(arg: impl Into<String>) -> Error {
        Error::new(format!("invalid metafile: {}", arg.into()))
    }
}

pub mod encryptionpb {
    /// 加密方式枚举占位；与 kvproto encryptionpb 数值对齐，show 仅用到 PLAINTEXT。
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum EncryptionMethod {
        UNKNOWN = 0,
        #[default]
        PLAINTEXT = 1,
        AES128_CTR = 2,
        AES192_CTR = 3,
        AES256_CTR = 4,
    }
    // 桩不执行加解密；CipherInfo 仅随 Config 传递以满足类型签名。
}

pub mod backuppb {
    use super::encryptionpb::EncryptionMethod;

    /// backuppb.CipherInfo 最小字段；真实解码与 KMS 不在桩内。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct CipherInfo {
        pub CipherType: EncryptionMethod,
        pub CipherKey: Vec<u8>,
    }

    /// protobuf RawRange 投影；show 读取后经 convertRawRange 转为展示结构。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct RawRange {
        pub StartKey: Vec<u8>,
        pub EndKey: Vec<u8>,
        pub Cf: String,
    }

    /// Minimal `BackupMeta` fields used by `show`.
    /// show 用到的 BackupMeta 最小字段集；不含完整 backuppb 扩展字段。
    /// ClusterVersion 保留 Go 单测中的引号与换行，不做 JSON 规范化。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct BackupMeta {
        pub ClusterId: u64,
        pub ClusterVersion: String,
        pub BrVersion: String,
        pub Version: i32,
        pub StartVersion: u64,
        pub EndVersion: u64,
        pub IsRawKv: bool,
        pub RawRanges: Vec<RawRange>,
        /// Non-`None` means backup meta v2 raw-range index (unsupported by show).
        /// 非 None 表示 backup meta v2 的 RawRangeIndex；show 当前不支持，须报错。
        pub RawRangeIndex: Option<Vec<u8>>,
    }

    /// 存储后端 URI 分解结果占位；真实 objstore 解析不在此实现。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct StorageBackend {
        pub Scheme: String,
        pub Path: String,
    }
}

/// Hex-encoded byte wrapper mirroring `logutil.HexBytes`.
/// 十六进制展示用字节包装，Display 为小写无分隔拼接，对齐 Go logutil.HexBytes。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HexBytes(pub Vec<u8>);

impl HexBytes {
    /// 从字节切片构造；等价于 Go HexBytes 包装 raw key。
    pub fn from_slice(b: &[u8]) -> Self {
        Self(b.to_vec())
    }
}

impl fmt::Display for HexBytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

pub mod oracle {
    use super::*;

    /// Mirrors `oracle.GetTimeFromTS`: physical time = `ts >> 18` milliseconds.
    /// TSO 物理时间：毫秒 = ts >> 18，与 TiDB oracle 布局一致（桩不含 PD 时钟源）。
    pub fn GetTimeFromTS(ts: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ts >> 18)
    }

    /// Format matching Go `time.Format("Y06M01D02,15:03:04")` in `time.Local`.
    ///
    /// Note: Go's layout uses `15` (24h hour), `03` (12h hour), `04` (minute).
    /// 将 TSO 格式化为 show 用时间括号段；本地时区与 Go `time.Unix` 对齐。
    pub fn format_show_ts(ts: u64) -> String {
        #[cfg(unix)]
        if let Some(local) = format_local_show_ts(ts) {
            return local;
        }

        let t = GetTimeFromTS(ts);
        let dur = t.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO);
        let secs = dur.as_secs() as i64;
        let (year, month, day, hour, min, _sec) = civil_from_days(secs);
        let hour12 = {
            let h = hour % 12;
            if h == 0 { 12 } else { h }
        };
        format!(
            "Y{:02}M{:02}D{:02},{:02}:{:02}:{:02}",
            year % 100,
            month,
            day,
            hour,
            hour12,
            min
        )
    }

    #[cfg(unix)]
    fn format_local_show_ts(ts: u64) -> Option<String> {
        unsafe extern "C" {
            fn localtime_r(timep: *const c_long, result: *mut c_void) -> *mut c_void;
            fn strftime(
                s: *mut c_char,
                max: usize,
                format: *const c_char,
                tm: *const c_void,
            ) -> usize;
        }

        let seconds = ((ts >> 18) / 1_000) as c_long;
        // Keep `struct tm` opaque while providing ample aligned storage for
        // supported Unix implementations.
        let mut tm = [0_usize; 32];
        let mut output = [0_i8; 64];
        let written = unsafe {
            let tm = localtime_r(&seconds, tm.as_mut_ptr().cast());
            if tm.is_null() {
                return None;
            }
            strftime(
                output.as_mut_ptr(),
                output.len(),
                c"Y%yM%mD%d,%H:%I:%M".as_ptr(),
                tm,
            )
        };
        if written == 0 {
            return None;
        }
        unsafe { CStr::from_ptr(output.as_ptr()) }
            .to_str()
            .ok()
            .map(str::to_owned)
    }

    /// Civil date/time from Unix seconds (UTC), proleptic Gregorian.
    /// Unix 秒 → 民用日期时间（UTC）；算法来自 Howard Hinnant civil_from_days。
    fn civil_from_days(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
        let days = secs.div_euclid(86400);
        let tod = secs.rem_euclid(86400) as u32;
        let hour = tod / 3600;
        let min = (tod % 3600) / 60;
        let sec = tod % 60;

        // Algorithm from civil_from_days (Howard Hinnant), days since 1970-01-01.
        let z = days + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = (z - era * 146097) as u32;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = (yoe as i64) + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if m <= 2 { y + 1 } else { y };
        (y as i32, m, d, hour, min, sec)
        // 桩 civil_from_days 仅服务 format_show_ts；不暴露为公开 API。
    }
}

pub mod objstore {
    /// 对象存储 BackendOptions 空壳；真实 S3/GCS 选项未建模。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct BackendOptions {}
}

pub mod task {
    use super::backuppb::CipherInfo;
    use super::objstore::BackendOptions;

    /// Minimal `task.Config` fields required by `ReadBackupMeta` / `lameTaskConfig`.
    /// ReadBackupMeta / lameTaskConfig 所需的最小 task.Config；非完整 BR task 配置。
    #[derive(Clone, Debug, Default)]
    pub struct Config {
        pub Storage: String,
        pub BackendOptions: BackendOptions,
        pub CipherInfo: CipherInfo,
    }
}

/// CIStr-like name holder for DB / table names.
/// 模拟 parser/model CIStr：内部字段 O 存原始名字符串。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CIStr {
    pub O: String,
}

impl CIStr {
    pub fn new(s: impl Into<String>) -> Self {
        Self { O: s.into() }
    }

    /// 对应 Go CIStr.String()，返回 O 的克隆。
    pub fn String(&self) -> String {
        self.O.clone()
    }
}

/// 库级 schema 信息占位；仅 Name 字段供 show 展示库名。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DBInfo {
    pub Name: CIStr,
}

/// 表级 schema 信息占位；Info 为 None 时表示库级行（无表名）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableInfo {
    pub Name: CIStr,
}

/// `metautil.Table` projection used by show.
/// metautil.Table 在 show 侧的投影；KV 统计与 TiFlash 副本数为展示字段。
#[derive(Clone, Debug, Default)]
pub struct MetaTable {
    pub DB: DBInfo,
    pub Info: Option<TableInfo>,
    pub TotalKvs: u64,
    pub TotalBytes: u64,
    pub TiFlashReplicas: i32,
}

/// ReadSchemasFiles 选项回调类型；与 Go `ReadSchemaOption` 函数签名对齐。
pub type ReadSchemaOption = fn(&mut ReadSchemaConfig);

/// ReadSchemasFiles 行为开关；桩内 skipFiles/skipStats 仅记录，真实过滤在 metautil。
#[derive(Clone, Debug, Default)]
pub struct ReadSchemaConfig {
    pub skipFiles: bool,
    pub skipStats: bool,
}

pub fn SkipFiles(conf: &mut ReadSchemaConfig) {
    // 桩记录 skipFiles 标志；真实 metautil 会跳过 data/files 类 schema 文件。
    conf.skipFiles = true;
}

pub fn SkipStats(conf: &mut ReadSchemaConfig) {
    // 桩记录 skipStats 标志；真实实现会省略统计类 schema 条目。
    conf.skipStats = true;
}

/// backupmeta 文件名常量，与 Go metautil.MetaFile 一致。
pub const MetaFile: &str = "backupmeta";

/// External storage boundary (mocked in tests).
///
/// `schema_tables` lets tests inject metautil schema walk results without
/// kvproto / real object-store IO (darwin arm64 slim stubs).
/// 外部存储 trait 边界：测试经 `schema_tables` 注入 schema，无真实对象存储 IO。
/// 生产路径需 kvproto + objstore；桩默认 `schema_tables` 返回空 Vec。
pub trait Storage: Send + Sync {
    /// 存储实例标签，供 MetaReader 记录来源；桩内不参与 URI 解析。
    fn name(&self) -> &str;
    /// 预注入 schema 表；默认空，生产需 metautil 遍历 object store。
    fn schema_tables(&self) -> Vec<MetaTable> {
        Vec::new()
    }
}

/// 内存 Storage 桩：可预置 tables，供 MetaReader / ReadBackupMeta hook 路径使用。
#[derive(Clone, Debug, Default)]
pub struct MemStorage {
    pub label: String,
    pub tables: Vec<MetaTable>,
}

impl MemStorage {
    /// 空表列表的内存 storage；NewMetaReader 仍会从 storage.schema_tables() 取表。
    pub fn new(label: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            label: label.into(),
            tables: Vec::new(),
        })
    }

    /// Like [`Self::new`], but preloads schema tables for `CreateExec` → `Read`.
    /// 预加载 schema 表，模拟 ReadBackupMeta 后 storage 已携带 metautil 遍历结果。
    pub fn with_tables(label: impl Into<String>, tables: Vec<MetaTable>) -> Arc<Self> {
        Arc::new(Self {
            label: label.into(),
            tables,
        })
    }
}

impl Storage for MemStorage {
    fn name(&self) -> &str {
        &self.label
    }

    fn schema_tables(&self) -> Vec<MetaTable> {
        self.tables.clone()
    }
}

/// MetaReader stand-in: holds basic meta and optionally preloaded schemas.
///
/// Real V2 schema walking / storage IO is the mocked boundary; tests inject
/// `tables` to exercise `CmdExecutor::Read` collection semantics.
/// MetaReader 桩：持有 BackupMeta 与可选预加载 schema；不执行 V2 文件遍历。
/// 真实 metautil.MetaReader 会从 storage 读 schema 文件；此处由 `with_tables` 注入。
#[derive(Clone, Debug, Default)]
pub struct MetaReader {
    backup_meta: backuppb::BackupMeta,
    tables: Vec<MetaTable>,
    read_err: Option<Error>,
    #[allow(dead_code)]
    storage_name: String,
    #[allow(dead_code)]
    cipher: backuppb::CipherInfo,
}

impl MetaReader {
    /// 测试辅助：覆盖预注入 schema 表列表。
    pub fn with_tables(mut self, tables: Vec<MetaTable>) -> Self {
        self.tables = tables;
        self
    }

    /// 测试辅助：令 ReadSchemasFiles 返回指定错误，模拟 schema 读取失败。
    pub fn with_read_err(mut self, err: Error) -> Self {
        self.read_err = Some(err);
        self
    }

    /// 对应 Go MetaReader.GetBasic：返回内存中的 BackupMeta 副本。
    pub fn GetBasic(&self) -> backuppb::BackupMeta {
        self.backup_meta.clone()
    }

    /// Mirrors Go `ReadSchemasFiles`: pushes tables to `output`, returns error.
    /// 模拟 Go ReadSchemasFiles：按序 send 预加载表；ctx 取消或通道关闭时返回错误。
    pub fn ReadSchemasFiles(
        &self,
        ctx: &Context,
        output: &std::sync::mpsc::SyncSender<MetaTable>,
        opts: &[ReadSchemaOption],
    ) -> Result<()> {
        let mut cfg = ReadSchemaConfig::default();
        for opt in opts {
            opt(&mut cfg);
        }
        let _ = cfg; // skipFiles / skipStats applied at load time in real metautil
        // 真实 ReadSchemasFiles 会读 backupmeta 引用的 schema 文件；桩直接 replay tables。

        if let Some(err) = &self.read_err {
            return Err(err.clone());
        }
        for t in &self.tables {
            if ctx.Done() {
                // 与 Go 一致：schema  walk 中检测 ctx 取消并提前返回。
                return Err(ctx.Err().unwrap_or_else(|| Error::new("context canceled")));
            }
            output
                .send(t.clone())
                // 接收方 drop 通道时映射为 schema output channel closed。
                .map_err(|_| Error::new("schema output channel closed"))?;
        }
        Ok(())
    }
}

/// 工厂：从 BackupMeta + Storage 构造 MetaReader；tables 优先取自 storage.schema_tables()。
pub fn NewMetaReader(
    backupMeta: backuppb::BackupMeta,
    storage: Arc<dyn Storage>,
    cipher: &backuppb::CipherInfo,
) -> MetaReader {
    MetaReader {
        backup_meta: backupMeta,
        // Prefer tables carried by the storage mock (fixture / hook path).
        // storage 携带的 tables 优先于 MetaReader 内建空列表（hook 路径常见）。
        tables: storage.schema_tables(),
        read_err: None,
        storage_name: storage.name().to_string(),
        cipher: cipher.clone(),
    }
}

/// Hook type for `ReadBackupMeta` (storage / protobuf decode boundary).
/// ReadBackupMeta 可注入 hook：替代真实 storage 打开 + protobuf 解码边界。
pub type ReadBackupMetaHook = Box<
    dyn Fn(
            &Context,
            &str,
            &task::Config,
        ) -> Result<(
            backuppb::StorageBackend,
            Arc<dyn Storage>,
            backuppb::BackupMeta,
        )> + Send
        + Sync,
>;

thread_local! {
    // 每线程独立 hook 槽；set_read_backup_meta_hook(None) 为测试清理契约。
    static READ_BACKUP_META_HOOK: RefCell<Option<ReadBackupMetaHook>> =
        const { RefCell::new(None) };
}

/// 设置/清除 thread_local ReadBackupMeta hook；parity 测试须在结束时传 None 清理。
pub fn set_read_backup_meta_hook(hook: Option<ReadBackupMetaHook>) {
    READ_BACKUP_META_HOOK.with(|slot| *slot.borrow_mut() = hook);
}

/// Mirrors `task.ReadBackupMeta` shape used by show `CreateExec`.
///
/// Without a hook, returns an annotated error (storage/protobuf is mocked).
/// 对应 task.ReadBackupMeta 签名；无 hook 时返回未配置错误，非真实 IO 失败。
/// 有 hook 时完全由测试/集成层决定返回值，桩不解析 Storage URI。
pub fn ReadBackupMeta(
    ctx: &Context,
    fileName: &str,
    cfg: &task::Config,
) -> Result<(
    backuppb::StorageBackend,
    Arc<dyn Storage>,
    backuppb::BackupMeta,
)> {
    if let Some(result) = READ_BACKUP_META_HOOK
        .with(|slot| slot.borrow().as_ref().map(|hook| hook(ctx, fileName, cfg)))
    {
        // hook 完全接管 IO：返回 backend、storage 句柄与已解码 BackupMeta。
        return result;
    }
    // 默认错误路径：提醒测试须先 set_read_backup_meta_hook，非生产 storage 故障。
    Err(Error::new(format!(
        "ReadBackupMeta not configured for storage={} file={}",
        cfg.Storage, fileName
    )))
}

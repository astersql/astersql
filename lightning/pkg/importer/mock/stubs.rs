// Copyright 2026 AsterSQL.
//! Local stand-ins for mydump/objstore/model/filter/pd/errno/units boundaries
//! (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! 这个文件不尝试完整复刻真实依赖，只提供 mock 包和相关测试所需的最小公共形状。
//! 注释重点说明“为什么这里只做这么少”以及“这些替身给上层提供了什么契约”，
//! 以免后续维护者把它误当成生产实现继续叠加复杂逻辑。
//! 可以把它理解为一组“结构兼容适配器”：
//! 重点在字段、方法名和少量可观察行为对齐，
//! 而不在完整功能覆盖。
//! 一旦真实依赖接入，这些实现理论上都应被替换而不是扩展。

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// errors (pingcap/errors shape used by mock.go)
// ---------------------------------------------------------------------------
// 错误模型只保留字符串消息和最常用的构造/传递方式。
// 目标是兼容测试里的 `Error()`/`Errorf()`/`Trace()` 调用，而不是提供完整堆栈能力。

#[derive(Clone, Debug)]
pub struct Error {
    pub msg: String,
}
// `Error` 只承载消息文本，
// 没有额外错误码、堆栈或来源链。

impl Error {
    /// 以任意可转字符串的值构造错误。
    pub fn new(msg: impl Into<String>) -> Self {
        Self { msg: msg.into() }
    }

    /// 保留 Go 风格的 `Error()` 方法名，便于测试直接照搬原断言。
    pub fn Error(&self) -> String {
        self.msg.clone()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.msg)
    }
}

impl std::error::Error for Error {}

/// 整个 stub 文件统一使用这一结果类型。
pub type Result<T> = std::result::Result<T, Error>;

pub mod errors {
    use super::Error;
    use std::fmt;

    /// `Trace` 在 stub 中不追加上下文，只原样返回。
    pub fn Trace(err: Error) -> Error {
        err
    }

    /// 通过显示格式化快速构造错误，模拟 Go `errors.Errorf`。
    pub fn Errorf(msg: impl fmt::Display) -> Error {
        Error::new(msg.to_string())
    }
}

// ---------------------------------------------------------------------------
// context (unused beyond signature parity)
// ---------------------------------------------------------------------------
// context 只承担签名占位作用，不提供取消、deadline 等真实能力。

pub mod context {
    #[derive(Clone, Debug, Default)]
    pub struct Context;

    /// 返回一个空背景上下文，匹配 Go `context.Background()` 调用点。
    pub fn Background() -> Context {
        Context
    }
}

// ---------------------------------------------------------------------------
// filter.Table
// ---------------------------------------------------------------------------
// 这里的表名结构仅保留 schema/name 两个字段，足够支撑 mydump 元数据描述。

pub mod filter {
    /// 只描述表所属 schema 与表名。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct Table {
        pub Schema: String,
        pub Name: String,
    }
}

// ---------------------------------------------------------------------------
// mydump metadata shapes (Go field names)
// ---------------------------------------------------------------------------
// mydump 相关替身承担“文件元数据载体”职责，
// 让 `mock.rs` 可以像真实 importer 一样生成数据库/表/文件层级。

pub mod mydump {
    use super::filter;

    /// 源文件类型覆盖 schema、数据与视图等几类测试需要的分支。
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum SourceType {
        #[default]
        Ignore = 0,
        SchemaSchema,
        TableSchema,
        SQL,
        CSV,
        Parquet,
        ViewSchema,
    }

    /// 这些常量导出保留 Go 代码常见的命名风格，方便 parity 测试引用。
    pub const SourceTypeIgnore: SourceType = SourceType::Ignore;
    pub const SourceTypeSchemaSchema: SourceType = SourceType::SchemaSchema;
    pub const SourceTypeTableSchema: SourceType = SourceType::TableSchema;
    pub const SourceTypeSQL: SourceType = SourceType::SQL;
    pub const SourceTypeCSV: SourceType = SourceType::CSV;
    pub const SourceTypeParquet: SourceType = SourceType::Parquet;
    pub const SourceTypeViewSchema: SourceType = SourceType::ViewSchema;

    /// 压缩算法枚举主要用于后缀到元数据字段的映射验证。
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum Compression {
        #[default]
        None = 0,
        GZ,
        Lz4,
        Zstd,
        Xz,
        Lzo,
        Snappy,
    }

    /// 当前 mock 只真正依赖 `None` 和 `GZ`，其余枚举值保留作形状兼容。
    pub const CompressionNone: Compression = Compression::None;
    pub const CompressionGZ: Compression = Compression::GZ;

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct SourceFileMeta {
        pub Path: String,
        pub Type: SourceType,
        pub Compression: Compression,
        pub FileSize: i64,
        pub RealSize: i64,
    }

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct FileInfo {
        pub TableName: filter::Table,
        pub FileMeta: SourceFileMeta,
    }

    #[derive(Clone, Debug, Default)]
    pub struct MDDatabaseMeta {
        pub Name: String,
        pub SchemaFile: FileInfo,
        pub Tables: Vec<MDTableMeta>,
        pub CharSet: String,
    }

    /// 生成一个带固定字符集的数据库元数据壳。
    pub fn NewMDDatabaseMeta(char_set: &str) -> MDDatabaseMeta {
        MDDatabaseMeta {
            CharSet: char_set.to_string(),
            ..Default::default()
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct MDTableMeta {
        pub DB: String,
        pub Name: String,
        pub SchemaFile: FileInfo,
        pub DataFiles: Vec<FileInfo>,
        pub CharSet: String,
        pub TotalSize: i64,
    }

    /// 生成一个带固定字符集的表元数据壳。
    pub fn NewMDTableMeta(char_set: &str) -> MDTableMeta {
        MDTableMeta {
            CharSet: char_set.to_string(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// objstore MemStorage (WriteFile / ReadFile)
// ---------------------------------------------------------------------------
// 对象存储替身是这份 stub 中最有“行为”的部分，
// 因为 mock 测试确实会通过它写入和读回字节内容。

pub mod objstore {
    use super::{Error, Result, errors};
    use std::collections::HashMap;
    use std::fmt;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct MemStorage {
        files: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    }
    // 共享底层 `Arc<Mutex<_>>` 是为了让克隆句柄在源对象释放后仍可读。

    impl fmt::Debug for MemStorage {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            let n = self.files.lock().map(|g| g.len()).unwrap_or(0);
            f.debug_struct("MemStorage").field("files", &n).finish()
        }
    }

    /// 创建一份空的内存对象存储。
    pub fn NewMemStorage() -> MemStorage {
        MemStorage {
            files: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    impl MemStorage {
        /// 以路径为 key 覆盖写入文件内容。
        /// 不模拟权限、目录层级或部分写入失败。
        pub fn WriteFile(
            &self,
            _ctx: &super::context::Context,
            name: &str,
            data: &[u8],
        ) -> Result<()> {
            self.files
                .lock()
                .expect("mem storage lock")
                .insert(name.to_string(), data.to_vec());
            Ok(())
        }

        /// 读取指定路径的完整内容。
        /// 若路径不存在，返回带文件名的错误消息，方便测试直接匹配。
        pub fn ReadFile(&self, _ctx: &super::context::Context, name: &str) -> Result<Vec<u8>> {
            self.files
                .lock()
                .expect("mem storage lock")
                .get(name)
                .cloned()
                .ok_or_else(|| errors::Errorf(format!("cannot find the file: {name}")))
        }
    }

    // silence unused Error import warning via type alias use
    #[allow(dead_code)]
    type _E = Error;
}

pub mod storeapi {
    /// 提供与上层期望一致的 `Storage` 别名。
    pub use super::objstore::MemStorage as Storage;
}

// ---------------------------------------------------------------------------
// parser/ast CIStr + meta/model minimal shapes
// ---------------------------------------------------------------------------
// 这些类型只保留 importer mock 测试会访问的字段，
// 不追求覆盖 parser/model 真正的全部语义。

pub mod ast {
    /// `CIStr` 复刻大小写双份字段，匹配真实 parser/model 的常见用法。
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    pub struct CIStr {
        pub O: String,
        pub L: String,
    }

    /// 构造同时带原始大小写与小写副本的字符串。
    pub fn NewCIStr(s: impl Into<String>) -> CIStr {
        let o = s.into();
        let l = o.to_lowercase();
        CIStr { O: o, L: l }
    }
}

pub mod model {
    use super::ast::CIStr;

    /// 数据库信息只保留名称。
    #[derive(Clone, Debug, Default)]
    pub struct DBInfo {
        pub Name: CIStr,
    }

    /// 列信息只保留 ID、名称与偏移量。
    #[derive(Clone, Debug, Default)]
    pub struct ColumnInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub Offset: isize,
    }

    /// 表结构只覆盖 parity 测试会读取的字段。
    #[derive(Clone, Debug, Default)]
    pub struct TableInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub Columns: Vec<ColumnInfo>,
    }
}

// ---------------------------------------------------------------------------
// errno + dbterror ClassSchema.NewStd(ErrBadDB).FastGenByArgs
// ---------------------------------------------------------------------------
// 这里主要复刻“未知数据库”错误的构造路径，
// 因为 mock parity 测试会断言其错误消息和错误码片段。

pub mod errno {
    /// MySQL/TiDB 未知数据库错误码。
    pub const ErrBadDB: u16 = 1049;
}

pub mod dbterror {
    use super::Error;
    use super::errno;

    /// 错误类本身不存状态，只提供模板选择能力。
    pub struct ErrClass;

    /// schema 错误类的单例入口。
    pub const ClassSchema: ErrClass = ErrClass;

    pub struct StdError {
        code: u16,
        class: &'static str,
        template: &'static str,
    }
    // 模板对象只在生成最终消息前暂存错误码、类别与格式串。

    impl ErrClass {
        /// 只实现当前测试需要的少量错误模板。
        pub fn NewStd(&self, code: u16) -> StdError {
            let template = if code == errno::ErrBadDB {
                "Unknown database '%s'"
            } else {
                "error"
            };
            StdError {
                code,
                class: "schema",
                template,
            }
        }
    }

    impl StdError {
        /// 按 Go terror 风格把参数填入模板，再拼成统一错误字符串。
        pub fn FastGenByArgs(&self, arg: impl std::fmt::Display) -> Error {
            let msg = self.template.replace("%s", &arg.to_string());
            // pingcap/errors RFC style used by terror Error()
            Error::new(format!("[{}:{}]{}", self.class, self.code, msg))
        }
    }
}

// ---------------------------------------------------------------------------
// docker/go-units BytesSize
// ---------------------------------------------------------------------------
// `BytesSize` 负责把容量数字转成 Go 风格的人类可读文案，
// 这样 store 容量相关断言可以直接复用原字符串期望值。

pub mod units {
    /// Corresponds to Go `units.BytesSize` (base 1024, `%.4g` + unit).
    /// 输入来自 `StorageInfo` 的 `u64` 字段；实现遵循上游的 1024 进制与四位有效数字。
    pub fn BytesSize(size: f64) -> String {
        let sizes = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
        let base = 1024.0_f64;
        let mut size = size;
        let mut i = 0usize;
        let units_limit = sizes.len() - 1;
        while size >= base && i < units_limit {
            size /= base;
            i += 1;
        }
        format!("{}{}", format_g4(size), sizes[i])
    }

    /// 近似 Go `%.4g` 的格式化逻辑，优先服务当前测试样例。
    fn format_g4(v: f64) -> String {
        // Match Go fmt `%.4g` for the non-negative integer byte counts accepted by StorageInfo.
        if v == 0.0 {
            return "0".to_string();
        }
        let abs = v.abs();
        if abs >= 1e-4 && abs < 1e4 {
            // Up to 4 significant digits without forced scientific notation.
            let mut s = format!("{:.6}", v);
            if s.contains('.') {
                while s.ends_with('0') {
                    s.pop();
                }
                if s.ends_with('.') {
                    s.pop();
                }
            }
            // Trim to 4 significant digits when needed.
            let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() > 4 {
                let prec = if v.fract().abs() < 1e-12 {
                    0
                } else {
                    4usize.saturating_sub(format!("{}", v.trunc().abs() as i64).len())
                };
                let mut t = format!("{v:.prec$}");
                if t.contains('.') {
                    while t.ends_with('0') {
                        t.pop();
                    }
                    if t.ends_with('.') {
                        t.pop();
                    }
                }
                return t;
            }
            return s;
        }
        format!("{v:.4e}")
            .replace("e+", "e+")
            .replace("e-0", "e-")
            .replace("e+0", "e+")
    }
}

// ---------------------------------------------------------------------------
// pd/client/http store / region shapes
// ---------------------------------------------------------------------------
// PD HTTP 替身只保留 store/region 统计所需字段，
// 足够让目标端 mock 生成预检阶段要消费的结构。

pub mod pdhttp {
    /// store 基本信息只保留 ID 和状态名。
    #[derive(Clone, Debug, Default)]
    pub struct MetaStore {
        pub ID: i64,
        pub StateName: String,
    }

    /// store 运行时统计对应 importer 预检会读取的字段。
    #[derive(Clone, Debug, Default)]
    pub struct StoreStatus {
        pub Capacity: String,
        pub Available: String,
        pub RegionSize: i64,
        pub RegionCount: i64,
    }

    /// `StoreInfo` 把元信息和状态合并到一条记录中。
    #[derive(Clone, Debug, Default)]
    pub struct StoreInfo {
        pub Store: MetaStore,
        pub Status: StoreStatus,
    }

    /// store 列表外层结构，保留总数与明细。
    #[derive(Clone, Debug, Default)]
    pub struct StoresInfo {
        pub Count: usize,
        pub Stores: Vec<StoreInfo>,
    }

    /// region peer 只记录所属 store。
    #[derive(Clone, Debug, Default)]
    pub struct RegionPeer {
        pub StoreID: i64,
    }

    /// region 信息只保留 peer 列表，足够表达空 region 分布。
    #[derive(Clone, Debug, Default)]
    pub struct RegionInfo {
        pub Peers: Vec<RegionPeer>,
    }

    /// regions 列表外层结构，便于直接断言数量。
    #[derive(Clone, Debug, Default)]
    pub struct RegionsInfo {
        pub Count: i64,
        pub Regions: Vec<RegionInfo>,
    }
}

// Re-exports commonly used by mock.rs
// 这些 re-export 让 `mock.rs` 看起来更像在依赖一个真实边界包。
pub use context::Context;
pub use objstore::{MemStorage, NewMemStorage};
pub use pdhttp::{RegionInfo, RegionPeer, RegionsInfo, StoreInfo, StoresInfo};
pub use storeapi::Storage;

// Keep HashMap visible for stub-internal docs.
// 末尾两个空函数只是为了让公共类型在本文件内保持“被使用”状态。
#[allow(dead_code)]
fn _hashmap_ty() -> HashMap<(), ()> {
    HashMap::new()
}

#[allow(dead_code)]
fn _arc_mutex() -> Arc<Mutex<()>> {
    Arc::new(Mutex::new(()))
}

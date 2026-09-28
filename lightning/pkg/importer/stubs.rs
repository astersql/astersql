// Copyright 2026 AsterSQL.
//! Local stand-ins for SQL/config/common/log/encode/kv/tablecodec boundaries
//! (arm64-safe; no kv/domain/kvproto/grpcio).

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// errors (pingcap/errors shape)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
// 语义说明：`Error` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
pub struct Error {
    pub msg: String,
    pub not_found: bool,
    pub cause: Option<Box<Error>>,
    pub class: Option<&'static str>,
}

// 语义说明：这个 impl 块补齐 `Error` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
impl Error {
    // 语义说明：`new` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            not_found: false,
            cause: None,
            class: None,
        }
    }

    // 语义说明：`GenWithStackByArgs` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn GenWithStackByArgs(&self, args: impl std::fmt::Display) -> Error {
        Error {
            msg: format!("{}: {}", self.msg, args),
            not_found: self.not_found,
            cause: None,
            class: self.class,
        }
    }

    // 语义说明：`Error` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Error(&self) -> String {
        match &self.cause {
            Some(c) => format!("{}: {}", self.msg, c.Error()),
            None => self.msg.clone(),
        }
    }
}

// 语义说明：这个 impl 块补齐 `Error` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
impl fmt::Display for Error {
    // 语义说明：`fmt` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.Error())
    }
}

// 语义说明：这个 impl 块补齐 `Error` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
impl std::error::Error for Error {}

// 语义说明：这个 impl 块补齐 `Error` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
impl PartialEq for Error {
    // 语义说明：`eq` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    fn eq(&self, other: &Self) -> bool {
        self.Error() == other.Error()
    }
}

pub type Result<T> = std::result::Result<T, Error>;

// 语义说明：`errors` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod errors {
    use std::fmt;

    pub use super::{Error, Result};

    // 语义说明：`New` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn New(msg: impl Into<String>) -> Error {
        Error::new(msg)
    }

    // 语义说明：`Errorf` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Errorf(msg: impl fmt::Display) -> Error {
        Error::new(msg.to_string())
    }

    // 语义说明：`Trace` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Trace(err: Error) -> Error {
        err
    }

    // 语义说明：`Annotatef` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Annotatef(err: Error, msg: impl fmt::Display) -> Error {
        Error {
            msg: msg.to_string(),
            not_found: false,
            cause: Some(Box::new(err)),
            class: None,
        }
    }

    // 语义说明：`Cause` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Cause(err: &Error) -> &Error {
        match &err.cause {
            Some(c) => Cause(c),
            None => err,
        }
    }

    // 语义说明：`ErrorEqual` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn ErrorEqual(a: &Error, b: &Error) -> bool {
        a.class == b.class && a.class.is_some() || a.msg == b.msg
    }

    // 语义说明：`Normalize` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Normalize(msg: &'static str, _code: &str) -> Error {
        let mut e = Error::new(msg);
        e.class = Some(msg);
        e
    }

    // 语义说明：`Is` 保留布尔判定入口，供迁移代码沿用与 Go 相同的条件分支。
    pub fn Is(err: &Error, target: &Error) -> bool {
        if let Some(c) = target.class {
            if err.class == Some(c) {
                return true;
            }
        }
        err.msg.contains(&target.msg)
    }
}

// 语义说明：`multierr` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod multierr {
    use super::Error;

    // 语义说明：`Append` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
    pub fn Append(a: Error, b: Error) -> Error {
        Error::new(format!("{}; {}", a.Error(), b.Error()))
    }
}

// ---------------------------------------------------------------------------
// context
// ---------------------------------------------------------------------------

// 语义说明：`context` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod context {
    use std::any::Any;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[derive(Clone, Default)]
    // 语义说明：`Context` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Context {
        pub str_values: Arc<HashMap<String, Arc<dyn Any + Send + Sync>>>,
        pub cancelled: bool,
        pub(crate) cancellation_tokens: Vec<Arc<AtomicBool>>,
    }

    // 语义说明：这个 impl 块补齐 `Context` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl std::fmt::Debug for Context {
        // 语义说明：`fmt` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Context")
        }
    }

    // 语义说明：`Background` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Background() -> Context {
        Context::default()
    }

    // 语义说明：`WithCancel` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn WithCancel(parent: Context) -> (Context, CancelFunc) {
        let token = Arc::new(AtomicBool::new(false));
        let mut child = parent;
        child.cancellation_tokens.push(token.clone());
        (child, CancelFunc { token })
    }

    // 语义说明：`WithValue` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn WithValue(
        parent: Context,
        key: impl Into<String>,
        val: Arc<dyn Any + Send + Sync>,
    ) -> Context {
        let mut m = (*parent.str_values).clone();
        m.insert(key.into(), val);
        Context {
            str_values: Arc::new(m),
            cancelled: parent.cancelled,
            cancellation_tokens: parent.cancellation_tokens,
        }
    }

    // 语义说明：这个 impl 块补齐 `Context` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Context {
        // 语义说明：`Value` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Value(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
            self.str_values.get(key).cloned()
        }
        // 语义说明：`Err` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Err(&self) -> Option<super::Error> {
            if self.cancelled
                || self
                    .cancellation_tokens
                    .iter()
                    .any(|token| token.load(Ordering::SeqCst))
            {
                Some(super::Error::new("context canceled"))
            } else {
                None
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`CancelFunc` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct CancelFunc {
        token: Arc<AtomicBool>,
    }

    // 语义说明：这个 impl 块补齐 `CancelFunc` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl CancelFunc {
        // 语义说明：`cancel` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn cancel(self) {
            self.token.store(true, Ordering::SeqCst);
        }
    }
}

// ---------------------------------------------------------------------------
// atomic (go.uber.org/atomic shape)
// ---------------------------------------------------------------------------

// 语义说明：`atomic` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod atomic {
    use super::{AtomicBool, AtomicI32, AtomicI64, Ordering};

    #[derive(Debug)]
    // 语义说明：`Int64` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Int64(AtomicI64);

    // 语义说明：这个 impl 块补齐 `Int64` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Int64 {
        // 语义说明：`new` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn new(v: i64) -> Self {
            Self(AtomicI64::new(v))
        }
        // 语义说明：`Load` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn Load(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
        // 语义说明：`Store` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
        pub fn Store(&self, v: i64) {
            self.0.store(v, Ordering::SeqCst);
        }
        /// Returns the new value (Go uber atomic semantics).
        // 语义说明：`Add` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
        pub fn Add(&self, delta: i64) -> i64 {
            self.0.fetch_add(delta, Ordering::SeqCst) + delta
        }
        // 语义说明：`Sub` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Sub(&self, n: i64) -> i64 {
            self.Add(-n)
        }
        // 语义说明：`Dec` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Dec(&self) -> i64 {
            self.Add(-1)
        }
    }

    // 语义说明：这个 impl 块补齐 `Int64` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Clone for Int64 {
        // 语义说明：`clone` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn clone(&self) -> Self {
            Self::new(self.Load())
        }
    }

    // 语义说明：这个 impl 块补齐 `Int64` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Default for Int64 {
        // 语义说明：`default` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn default() -> Self {
            Self::new(0)
        }
    }

    #[derive(Debug)]
    // 语义说明：`Bool` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Bool(AtomicBool);

    // 语义说明：这个 impl 块补齐 `Bool` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Bool {
        // 语义说明：`new` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn new(v: bool) -> Self {
            Self(AtomicBool::new(v))
        }
        // 语义说明：`Load` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn Load(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        // 语义说明：`Store` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
        pub fn Store(&self, v: bool) {
            self.0.store(v, Ordering::SeqCst);
        }
        // 语义说明：`CompareAndSwap` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn CompareAndSwap(&self, old: bool, new: bool) -> bool {
            self.0
                .compare_exchange(old, new, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        }
    }

    // 语义说明：这个 impl 块补齐 `Bool` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Clone for Bool {
        // 语义说明：`clone` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn clone(&self) -> Self {
            Self::new(self.Load())
        }
    }

    // 语义说明：这个 impl 块补齐 `Bool` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Default for Bool {
        // 语义说明：`default` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn default() -> Self {
            Self::new(false)
        }
    }

    // 语义说明：`NewInt64` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewInt64(v: i64) -> Int64 {
        Int64::new(v)
    }

    // 语义说明：`NewBool` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewBool(v: bool) -> Bool {
        Bool::new(v)
    }

    #[derive(Debug)]
    // 语义说明：`Int32` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Int32(AtomicI32);
    // 语义说明：这个 impl 块补齐 `Int32` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Int32 {
        // 语义说明：`new` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn new(v: i32) -> Self {
            Self(AtomicI32::new(v))
        }
        // 语义说明：`Load` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn Load(&self) -> i32 {
            self.0.load(Ordering::SeqCst)
        }
        // 语义说明：`Store` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
        pub fn Store(&self, v: i32) {
            self.0.store(v, Ordering::SeqCst);
        }
        // 语义说明：`CompareAndSwap` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn CompareAndSwap(&self, old: i32, new: i32) -> bool {
            self.0
                .compare_exchange(old, new, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        }
    }
    // 语义说明：这个 impl 块补齐 `Int32` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Clone for Int32 {
        // 语义说明：`clone` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn clone(&self) -> Self {
            Self::new(self.Load())
        }
    }
    // 语义说明：这个 impl 块补齐 `Int32` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Default for Int32 {
        // 语义说明：`default` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn default() -> Self {
            Self::new(0)
        }
    }
    // 语义说明：`NewInt32` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewInt32(v: i32) -> Int32 {
        Int32::new(v)
    }
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

// 语义说明：`config` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod config {
    use super::atomic::Int64;

    // 语义说明：`BackendTiDB` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const BackendTiDB: &str = "tidb";
    // 语义说明：`BackendLocal` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const BackendLocal: &str = "local";
    pub const CheckpointDriverFile: &str = "file";

    pub type DuplicateResolutionAlgorithm = i32;
    // 语义说明：`NoneOnDup` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const NoneOnDup: DuplicateResolutionAlgorithm = 0;
    // 语义说明：`ReplaceOnDup` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const ReplaceOnDup: DuplicateResolutionAlgorithm = 1;
    // 语义说明：`IgnoreOnDup` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const IgnoreOnDup: DuplicateResolutionAlgorithm = 2;
    // 语义说明：`ErrorOnDup` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const ErrorOnDup: DuplicateResolutionAlgorithm = 3;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`MaxError` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct MaxError {
        pub Syntax: Int64,
        pub Charset: Int64,
        pub Type: Int64,
        pub Conflict: Int64,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Conflict` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Conflict {
        pub Strategy: DuplicateResolutionAlgorithm,
        pub PrecheckConflictBeforeImport: bool,
        pub Threshold: i64,
        pub MaxRecordRows: i64,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Lightning` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Lightning {
        pub TaskInfoSchemaName: String,
        pub CheckRequirements: bool,
        pub MaxError: MaxError,
        pub RegionConcurrency: i32,
        pub TableConcurrency: i32,
        pub IndexConcurrency: i32,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`TikvImporter` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct TikvImporter {
        pub Backend: String,
        pub Addr: String,
        pub SortedKVDir: String,
        pub AddIndexBySQL: bool,
        pub ParallelImport: bool,
        pub IncrementalImport: bool,
    }

    pub type OpLevel = i32;
    // 语义说明：`OpLevelOff` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const OpLevelOff: OpLevel = 0;
    // 语义说明：`OpLevelOptional` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const OpLevelOptional: OpLevel = 1;
    // 语义说明：`OpLevelRequired` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const OpLevelRequired: OpLevel = 2;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Security` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Security {
        pub TLSConfig: String,
        pub AllowFallbackToPlaintext: bool,
        pub ClusterSSLCA: String,
        pub ClusterSSLCert: String,
        pub ClusterSSLKey: String,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`DBStore` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct DBStore {
        pub Host: String,
        pub Port: i32,
        pub User: String,
        pub Psw: String,
        pub StrSQLMode: String,
        pub SQLMode: u64,
        pub MaxAllowedPacket: u64,
        pub Security: Security,
        pub UUID: String,
        pub PdAddr: String,
        pub BuildStatsConcurrency: i32,
        pub DistSQLScanConcurrency: i32,
        pub IndexSerialScanConcurrency: i32,
        pub ChecksumTableConcurrency: i32,
        pub Vars: Option<std::collections::HashMap<String, String>>,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Checkpoint` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Checkpoint {
        pub Enable: bool,
        pub Driver: String,
        pub DSN: String,
        pub Schema: String,
        pub KeepAfterSuccess: OpLevel,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`IgnoreColumnsCfg` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct IgnoreColumnsCfg {
        pub entries: Vec<(String, String, std::collections::HashSet<String>)>,
    }

    // 语义说明：这个 impl 块补齐 `IgnoreColumnsCfg` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl IgnoreColumnsCfg {
        // 语义说明：`GetIgnoreColumns` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn GetIgnoreColumns(
            &self,
            db: &str,
            table: &str,
            _case_sensitive: bool,
        ) -> super::Result<IgnoreColumnsSet> {
            for (d, t, cols) in &self.entries {
                if (d == "*" || d.eq_ignore_ascii_case(db))
                    && (t == "*" || t.eq_ignore_ascii_case(table))
                {
                    return Ok(IgnoreColumnsSet { cols: cols.clone() });
                }
            }
            Ok(IgnoreColumnsSet::default())
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`IgnoreColumnsSet` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct IgnoreColumnsSet {
        pub cols: std::collections::HashSet<String>,
    }

    // 语义说明：这个 impl 块补齐 `IgnoreColumnsSet` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl IgnoreColumnsSet {
        // 语义说明：`ColumnsMap` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn ColumnsMap(&self) -> std::collections::HashSet<String> {
            self.cols.clone()
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`CSVConfig` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct CSVConfig {
        pub Header: bool,
        pub HeaderSchemaMatch: bool,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`MydumperRuntime` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct MydumperRuntime {
        pub SourceDir: String,
        pub SourceID: String,
        pub StrictFormat: bool,
        pub CSV: CSVConfig,
        pub IgnoreColumns: IgnoreColumnsCfg,
        pub CaseSensitive: bool,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`PostRestore` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct PostRestore {
        pub Checksum: OpLevel,
        pub Analyze: OpLevel,
        pub ChecksumViaSQL: bool,
    }

    #[derive(Clone, Copy, Debug, Default)]
    // 语义说明：`DurationSecs` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct DurationSecs(pub u64);

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Cron` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Cron {
        pub SwitchMode: DurationSecs,
        pub LogProgress: DurationSecs,
        pub CheckDiskQuota: DurationSecs,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Config` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Config {
        pub TaskID: i64,
        pub App: Lightning,
        pub TikvImporter: TikvImporter,
        pub Conflict: Conflict,
        pub Checkpoint: Checkpoint,
        pub Mydumper: MydumperRuntime,
        pub TiDB: DBStore,
        pub PostRestore: PostRestore,
        pub Cron: Cron,
        pub Security: Security,
        pub Routes: Vec<table_router::TableRule>,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`GlobalConfig` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct GlobalConfig {
        pub Security: Security,
    }

    use std::sync::{Mutex, OnceLock};
    // 语义说明：`global_cfg` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    fn global_cfg() -> &'static Mutex<GlobalConfig> {
        static G: OnceLock<Mutex<GlobalConfig>> = OnceLock::new();
        G.get_or_init(|| Mutex::new(GlobalConfig::default()))
    }

    // 语义说明：`GetGlobalConfig` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
    pub fn GetGlobalConfig() -> GlobalConfig {
        global_cfg().lock().unwrap().clone()
    }

    // 语义说明：`StoreGlobalConfig` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
    pub fn StoreGlobalConfig(cfg: GlobalConfig) {
        *global_cfg().lock().unwrap() = cfg;
    }

    // 语义说明：这个 impl 块补齐 `Config` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Config {
        // 语义说明：`NewConfig` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
        pub fn NewConfig() -> Self {
            let mut cfg = Self::default();
            cfg.App.CheckRequirements = true;
            cfg.Checkpoint.Driver = CheckpointDriverFile.into();
            cfg
        }
    }
}

pub mod build {
    pub static ReleaseVersion: &str = "v0.0.0-astersql-stub";
}

// ---------------------------------------------------------------------------
// common
// ---------------------------------------------------------------------------

// 语义说明：`common` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod common {
    pub use super::common_ext::*;

    // 语义说明：`GetBackoffWeightFromDB` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
    pub fn GetBackoffWeightFromDB(
        _ctx: super::context::Context,
        db: &super::sql::DB,
    ) -> super::Result<i32> {
        match db.QueryRowString("SELECT @@tidb_backoff_weight") {
            Ok(s) => Ok(s.parse().unwrap_or(0)),
            Err(e) => Err(e),
        }
    }

    use super::Result;
    use super::context::Context;
    use super::log::Logger;
    use super::sql::{DB, SqlValue, Tx};
    use std::fmt::Write as _;

    // 语义说明：`EscapeIdentifier` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn EscapeIdentifier(identifier: &str) -> String {
        let mut builder = String::with_capacity(identifier.len() + 2);
        builder.push('`');
        for b in identifier.bytes() {
            if b == b'`' {
                builder.push_str("``");
            } else {
                builder.push(b as char);
            }
        }
        builder.push('`');
        builder
    }

    // 语义说明：`UniqueTable` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn UniqueTable(schema: &str, table: &str) -> String {
        format!("{}.{}", EscapeIdentifier(schema), EscapeIdentifier(table))
    }

    /// Go `fmt.Sprintf` with escaped identifiers; supports `%s` and `%%`.
    // 语义说明：`SprintfWithIdentifiers` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn SprintfWithIdentifiers(format: &str, identifiers: &[&str]) -> String {
        let escaped: Vec<String> = identifiers.iter().map(|s| EscapeIdentifier(s)).collect();
        sprintf_go(format, &escaped)
    }

    // 语义说明：`FprintfWithIdentifiers` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn FprintfWithIdentifiers(
        w: &mut String,
        format: &str,
        identifiers: &[&str],
    ) -> Result<usize> {
        let s = SprintfWithIdentifiers(format, identifiers);
        let n = s.len();
        w.push_str(&s);
        Ok(n)
    }

    // 语义说明：`sprintf_go` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    fn sprintf_go(format: &str, args: &[String]) -> String {
        let mut out = String::new();
        let bytes = format.as_bytes();
        let mut i = 0;
        let mut next = 0usize;
        while i < bytes.len() {
            if bytes[i] != b'%' {
                out.push(bytes[i] as char);
                i += 1;
                continue;
            }
            i += 1;
            if i >= bytes.len() {
                out.push('%');
                break;
            }
            if bytes[i] == b'%' {
                out.push('%');
                i += 1;
                continue;
            }
            if bytes[i] == b's' {
                if let Some(v) = args.get(next) {
                    out.push_str(v);
                } else {
                    out.push_str("%s");
                }
                next += 1;
                i += 1;
                continue;
            }
            out.push('%');
        }
        out
    }

    #[derive(Clone, Debug)]
    // 语义说明：`SQLWithRetry` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct SQLWithRetry {
        pub DB: super::sql::DB,
        pub Logger: Logger,
        pub HideQueryLog: bool,
    }

    // 语义说明：这个 impl 块补齐 `SQLWithRetry` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl SQLWithRetry {
        // 语义说明：`Exec` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Exec(&self, _ctx: Context, _name: &str, query: impl Into<String>) -> Result<()> {
            self.DB.Exec(query.into().as_str(), &[])
        }

        // 语义说明：`ExecArgs` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn ExecArgs(
            &self,
            _ctx: Context,
            _name: &str,
            query: String,
            args: &[SqlValue],
        ) -> Result<()> {
            self.DB.Exec(query.as_str(), args)
        }

        // 语义说明：`QueryStringRows` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn QueryStringRows(
            &self,
            _ctx: Context,
            _name: &str,
            query: &str,
        ) -> Result<Vec<Vec<String>>> {
            let mut rows = self.DB.QueryContext(Context::default(), query, &[])?;
            let mut out = Vec::new();
            while rows.Next() {
                // Best-effort: expose via Scan helpers when available; else empty.
                // For SHOW VARIABLES style, use internal current row through as_string on values.
                // Rows API in this stub does not expose generic scan; return empty on miss.
                let _ = rows.Err()?;
                out.push(Vec::new());
            }
            // If handlers provided raw rows, reconstruct from QueryContext internal by re-query via push handlers.
            // Prefer dedicated string-row helper on DB when present.
            if out.iter().all(|r| r.is_empty()) {
                if let Ok(raw) = self.DB.query_string_matrix(query) {
                    return Ok(raw);
                }
            }
            Ok(out)
        }

        // 语义说明：`QueryRow` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn QueryRow(
            &self,
            _ctx: Context,
            _name: &str,
            query: &str,
            dest: &mut String,
        ) -> Result<()> {
            *dest = self.DB.QueryRowString(query)?;
            Ok(())
        }

        // 语义说明：`Transact` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Transact<F>(&self, ctx: Context, _name: &str, f: F) -> Result<()>
        where
            F: FnOnce(Context, &Tx) -> Result<()>,
        {
            let tx = self.DB.Begin()?;
            match f(ctx, &tx) {
                Ok(()) => tx.Commit(),
                Err(e) => {
                    let _ = tx.Rollback();
                    Err(e)
                }
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`TLS` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct TLS;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Pauser` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Pauser {
        paused: std::sync::Arc<std::sync::Mutex<bool>>,
    }
    // 语义说明：这个 impl 块补齐 `Pauser` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Pauser {
        // 语义说明：`Pause` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Pause(&self) {
            *self.paused.lock().unwrap() = true;
        }
        // 语义说明：`Resume` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Resume(&self) {
            *self.paused.lock().unwrap() = false;
        }
        // 语义说明：`IsPaused` 保留布尔判定入口，供迁移代码沿用与 Go 相同的条件分支。
        pub fn IsPaused(&self) -> bool {
            *self.paused.lock().unwrap()
        }
    }
    // 语义说明：`NewPauser` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewPauser() -> Pauser {
        Pauser {
            paused: std::sync::Arc::new(std::sync::Mutex::new(false)),
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`MySQLConnectParam` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct MySQLConnectParam {
        pub Host: String,
        pub Port: i32,
        pub User: String,
        pub Password: String,
        pub SQLMode: String,
        pub MaxAllowedPacket: u64,
        pub TLSConfig: String,
        pub AllowFallbackToPlaintext: bool,
        pub Net: String,
        pub Vars: std::collections::HashMap<String, String>,
    }
    // 语义说明：这个 impl 块补齐 `MySQLConnectParam` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl MySQLConnectParam {
        // 语义说明：`Connect` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Connect(&self) -> super::Result<super::sql::DB> {
            Ok(super::sql::DB::new_memory())
        }
    }

    /// Helper used by tests / formatting.
    // 语义说明：`format_sql_values` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn format_sql_values(parts: &[&str]) -> String {
        let mut s = String::new();
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                let _ = write!(s, ",");
            }
            s.push_str(p);
        }
        s
    }
}

// ---------------------------------------------------------------------------
// log / zap / redact / logutil
// ---------------------------------------------------------------------------

// 语义说明：`log` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod log {
    use super::zap::{self, Field};
    use std::time::Duration;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Logger` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Logger {
        pub warns: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        pub fields: Vec<Field>,
    }

    // 语义说明：这个 impl 块补齐 `Logger` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Logger {
        // 语义说明：`With` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn With(mut self, field: Field) -> Self {
            self.fields.push(field);
            self
        }
        // 语义说明：`WithFields` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn WithFields(mut self, fields: &[Field]) -> Self {
            self.fields.extend_from_slice(fields);
            self
        }
        // 语义说明：`Warn` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Warn(&self, msg: impl Into<String>, _fields: &[Field]) {
            if let Ok(mut g) = self.warns.lock() {
                g.push(msg.into());
            }
        }
        // 语义说明：`Info` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Info(&self, _msg: &str, _fields: &[Field]) {}
        // 语义说明：`Error` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Error(&self, _msg: &str, _fields: &[Field]) {}
        // 语义说明：`Debug` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Debug(&self, _msg: &str, _fields: &[Field]) {}
        // 语义说明：`L` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn L() -> Self {
            Self::default()
        }
        // 语义说明：`Begin` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Begin(&self, _level: zap::Level, _msg: &str) -> Task {
            Task {
                logger: self.clone(),
            }
        }
    }

    // 语义说明：`Wrap` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Wrap(l: Logger) -> Logger {
        l
    }

    // 语义说明：`ShortError` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn ShortError(err: &super::Error) -> Field {
        zap::String("error", err.Error())
    }

    #[derive(Clone, Debug)]
    // 语义说明：`Task` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Task {
        logger: Logger,
    }

    // 语义说明：这个 impl 块补齐 `Task` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Task {
        // 语义说明：`End` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn End(&self, _level: zap::Level, _err: Option<&super::Error>) -> Duration {
            Duration::from_millis(1)
        }
        // 语义说明：`EndWith` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn EndWith(
            &self,
            _level: zap::Level,
            _err: Option<&super::Error>,
            _extra: &[Field],
        ) -> Duration {
            Duration::from_millis(1)
        }
        // 语义说明：`Error` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Error(&self, _msg: &str, _fields: &[Field]) {}
    }
}

// 语义说明：`zap` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod zap {
    pub type Level = i32;
    // 语义说明：`InfoLevel` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const InfoLevel: Level = 0;
    // 语义说明：`WarnLevel` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const WarnLevel: Level = 1;
    // 语义说明：`ErrorLevel` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const ErrorLevel: Level = 2;
    // 语义说明：`DebugLevel` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const DebugLevel: Level = 3;

    #[derive(Clone, Debug)]
    // 语义说明：`Field` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Field;

    // 语义说明：`String` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn String(_k: &str, _v: impl ToString) -> Field {
        Field
    }
    // 语义说明：`Int` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Int(_k: &str, _v: i64) -> Field {
        Field
    }
    // 语义说明：`Int64` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Int64(_k: &str, _v: i64) -> Field {
        Field
    }
    // 语义说明：`Uint64` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Uint64(_k: &str, _v: u64) -> Field {
        Field
    }
    // 语义说明：`Bool` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Bool(_k: &str, _v: bool) -> Field {
        Field
    }
    // 语义说明：`Binary` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Binary(_k: &str, _v: &[u8]) -> Field {
        Field
    }
    // 语义说明：`Stringer` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Stringer(_k: &str, _v: impl ToString) -> Field {
        Field
    }
    // 语义说明：`Error` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Error(_e: &super::Error) -> Field {
        Field
    }
}

// 语义说明：`redact` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod redact {
    // 语义说明：`Value` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Value(s: &str) -> String {
        s.to_string()
    }
    // 语义说明：`NeedRedact` 保留布尔判定入口，供迁移代码沿用与 Go 相同的条件分支。
    pub fn NeedRedact() -> bool {
        false
    }
}

// 语义说明：`logutil` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod logutil {
    // 语义说明：`Key` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Key(k: &str, v: &[u8]) -> super::zap::Field {
        let _ = (k, v);
        super::zap::Field
    }
    // 语义说明：`Logger` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Logger(_ctx: super::context::Context) -> super::log::Logger {
        super::log::Logger::L()
    }
}

// ---------------------------------------------------------------------------
// sql stub (in-memory mock)
// ---------------------------------------------------------------------------

// 语义说明：`sql` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod sql {
    use super::{Error, Result};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug)]
    // 语义说明：`SqlValue` 枚举把 Go 里的有限状态翻成 Rust 可匹配分支，方便维持相同判定语义。
    pub enum SqlValue {
        Null,
        Int64(i64),
        Bool(bool),
        String(String),
        Bytes(Vec<u8>),
    }

    // 语义说明：这个 impl 块补齐 `SqlValue` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl From<i64> for SqlValue {
        // 语义说明：`from` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn from(v: i64) -> Self {
            SqlValue::Int64(v)
        }
    }
    // 语义说明：这个 impl 块补齐 `SqlValue` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl From<bool> for SqlValue {
        // 语义说明：`from` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn from(v: bool) -> Self {
            SqlValue::Bool(v)
        }
    }
    // 语义说明：这个 impl 块补齐 `SqlValue` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl From<String> for SqlValue {
        // 语义说明：`from` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn from(v: String) -> Self {
            SqlValue::String(v)
        }
    }
    // 语义说明：这个 impl 块补齐 `SqlValue` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl From<&str> for SqlValue {
        // 语义说明：`from` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn from(v: &str) -> Self {
            SqlValue::String(v.to_string())
        }
    }
    // 语义说明：这个 impl 块补齐 `SqlValue` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl From<Vec<u8>> for SqlValue {
        // 语义说明：`from` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn from(v: Vec<u8>) -> Self {
            SqlValue::Bytes(v)
        }
    }
    // 语义说明：这个 impl 块补齐 `SqlValue` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl From<&[u8]> for SqlValue {
        // 语义说明：`from` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn from(v: &[u8]) -> Self {
            SqlValue::Bytes(v.to_vec())
        }
    }
    // 语义说明：这个 impl 块补齐 `SqlValue` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl From<Option<String>> for SqlValue {
        // 语义说明：`from` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn from(v: Option<String>) -> Self {
            match v {
                Some(s) => SqlValue::String(s),
                None => SqlValue::Null,
            }
        }
    }

    #[derive(Clone, Debug)]
    // 语义说明：`DB` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct DB {
        inner: Arc<Mutex<DbInner>>,
    }

    #[derive(Debug, Default)]
    // 语义说明：`DbInner` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    struct DbInner {
        pub closed: bool,
        pub exec_log: Vec<(String, Vec<SqlValue>)>,
        pub query_handlers: Vec<QueryHandler>,
        pub query_error_handlers: Vec<QueryErrorHandler>,
        pub next_affected: i64,
        pub delete_affected_queue: Vec<i64>,
    }

    #[derive(Clone, Debug)]
    // 语义说明：`QueryHandler` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct QueryHandler {
        pub match_substr: String,
        pub rows: Vec<Vec<SqlValue>>,
        pub times: i32, // how many times to return; -1 = forever
    }

    #[derive(Clone, Debug)]
    // 语义说明：`QueryErrorHandler` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct QueryErrorHandler {
        pub match_substr: String,
        pub err: Error,
        pub times: i32, // how many times to return; -1 = forever
    }

    // 语义说明：这个 impl 块补齐 `DB` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl DB {
        // 语义说明：`new_memory` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn new_memory() -> Self {
            Self {
                inner: Arc::new(Mutex::new(DbInner {
                    next_affected: 1,
                    ..DbInner::default()
                })),
            }
        }

        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Close(&self) -> Result<()> {
            let mut g = self.inner.lock().unwrap();
            g.closed = true;
            Ok(())
        }

        // 语义说明：`is_closed` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn is_closed(&self) -> bool {
            self.inner.lock().unwrap().closed
        }

        // 语义说明：`query_string_matrix` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn query_string_matrix(&self, query: &str) -> Result<Vec<Vec<String>>> {
            let mut g = self.inner.lock().unwrap();
            g.exec_log.push((format!("QUERY {query}"), Vec::new()));
            for h in &mut g.query_handlers {
                if query.contains(&h.match_substr) || h.match_substr.is_empty() {
                    let out = h
                        .rows
                        .iter()
                        .map(|row| row.iter().map(SqlValue::as_string).collect())
                        .collect();
                    if h.times > 0 {
                        h.times -= 1;
                    }
                    if h.times == 0 {
                        h.rows.clear();
                    }
                    return Ok(out);
                }
            }
            Ok(vec![])
        }

        // 语义说明：`QueryRowString` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn QueryRowString(&self, query: &str) -> Result<String> {
            if let Some(err) = self.take_query_error(query) {
                return Err(err);
            }
            let matrix = self.query_string_matrix(query)?;
            if matrix.is_empty() || matrix[0].is_empty() {
                return Err(Error {
                    msg: "sql: no rows in result set".into(),
                    not_found: true,
                    cause: None,
                    class: Some("ErrNoRows"),
                });
            }
            Ok(matrix[0][0].clone())
        }

        // 语义说明：`exec_log` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn exec_log(&self) -> Vec<(String, Vec<SqlValue>)> {
            self.inner.lock().unwrap().exec_log.clone()
        }

        // 语义说明：`push_query_rows` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn push_query_rows(&self, match_substr: &str, rows: Vec<Vec<SqlValue>>, times: i32) {
            let mut g = self.inner.lock().unwrap();
            g.query_handlers.push(QueryHandler {
                match_substr: match_substr.to_string(),
                rows,
                times,
            });
        }

        /// Inject a non-row error for `QueryRowString` / query paths (e.g. access denied).
        // 语义说明：`push_query_error` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn push_query_error(&self, match_substr: &str, err: Error, times: i32) {
            let mut g = self.inner.lock().unwrap();
            g.query_error_handlers.push(QueryErrorHandler {
                match_substr: match_substr.to_string(),
                err,
                times,
            });
        }

        // 语义说明：`take_query_error` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn take_query_error(&self, query: &str) -> Option<Error> {
            let mut g = self.inner.lock().unwrap();
            let mut found = None;
            let mut remove_idx = None;
            for (i, h) in g.query_error_handlers.iter_mut().enumerate() {
                if query.contains(&h.match_substr) || h.match_substr.is_empty() {
                    found = Some(h.err.clone());
                    if h.times > 0 {
                        h.times -= 1;
                    }
                    if h.times == 0 {
                        remove_idx = Some(i);
                    }
                    break;
                }
            }
            if let Some(i) = remove_idx {
                g.query_error_handlers.remove(i);
            }
            found
        }

        // 语义说明：`push_delete_affected` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn push_delete_affected(&self, affected: i64) {
            let mut g = self.inner.lock().unwrap();
            g.delete_affected_queue.push(affected);
        }

        // 语义说明：`Exec` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Exec(&self, query: &str, args: &[SqlValue]) -> Result<()> {
            let mut g = self.inner.lock().unwrap();
            g.exec_log.push((query.to_string(), args.to_vec()));
            Ok(())
        }

        // 语义说明：`ExecAffected` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn ExecAffected(&self, query: &str, args: &[SqlValue]) -> Result<i64> {
            let mut g = self.inner.lock().unwrap();
            g.exec_log.push((query.to_string(), args.to_vec()));
            let q = query.trim().to_ascii_uppercase();
            if q.starts_with("DELETE") {
                if !g.delete_affected_queue.is_empty() {
                    return Ok(g.delete_affected_queue.remove(0));
                }
                return Ok(0);
            }
            Ok(g.next_affected)
        }

        // 语义说明：`Begin` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Begin(&self) -> Result<Tx> {
            Ok(Tx { db: self.clone() })
        }

        // 语义说明：`QueryContext` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn QueryContext(
            &self,
            _ctx: super::context::Context,
            query: &str,
            _args: &[SqlValue],
        ) -> Result<Rows> {
            let mut g = self.inner.lock().unwrap();
            g.exec_log.push((format!("QUERY {query}"), _args.to_vec()));
            for h in g.query_handlers.iter_mut() {
                if query.contains(&h.match_substr) {
                    let rows = h.rows.clone();
                    if h.times > 0 {
                        h.times -= 1;
                    }
                    if h.times == 0 {
                        // exhausted: leave empty for subsequent
                        h.rows.clear();
                    }
                    return Ok(Rows { rows, idx: 0 });
                }
            }
            Ok(Rows {
                rows: vec![],
                idx: 0,
            })
        }
    }

    #[derive(Clone, Debug)]
    // 语义说明：`Tx` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Tx {
        db: DB,
    }

    // 语义说明：这个 impl 块补齐 `Tx` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Tx {
        // 语义说明：`Commit` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Commit(&self) -> Result<()> {
            Ok(())
        }
        // 语义说明：`Rollback` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Rollback(&self) -> Result<()> {
            Ok(())
        }
        // 语义说明：`ExecContext` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn ExecContext(
            &self,
            _ctx: super::context::Context,
            query: &str,
            args: &[SqlValue],
        ) -> Result<ExecResult> {
            let affected = self.db.ExecAffected(query, args)?;
            Ok(ExecResult { affected })
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`ExecResult` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct ExecResult {
        pub affected: i64,
    }

    // 语义说明：这个 impl 块补齐 `ExecResult` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl ExecResult {
        // 语义说明：`RowsAffected` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn RowsAffected(&self) -> Result<i64> {
            Ok(self.affected)
        }
    }

    #[derive(Clone, Debug)]
    // 语义说明：`Rows` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Rows {
        rows: Vec<Vec<SqlValue>>,
        idx: usize,
    }

    // 语义说明：这个 impl 块补齐 `Rows` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Rows {
        /// Advances to the next row (Go `database/sql.Rows.Next` semantics).
        // 语义说明：`Next` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Next(&mut self) -> bool {
            if self.idx >= self.rows.len() {
                return false;
            }
            self.idx += 1;
            true
        }

        // 语义说明：`current` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn current(&self) -> Result<&[SqlValue]> {
            if self.idx == 0 || self.idx > self.rows.len() {
                return Err(Error::new("no row"));
            }
            Ok(&self.rows[self.idx - 1])
        }

        // 语义说明：`ScanIndexConflict` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn ScanIndexConflict(&mut self) -> Result<(i64, Vec<u8>, String, Vec<u8>, Vec<u8>)> {
            let row = self.current()?;
            Ok((
                row[0].as_i64(),
                row[1].as_bytes(),
                row[2].as_string(),
                row[3].as_bytes(),
                row[4].as_bytes(),
            ))
        }

        // 语义说明：`ScanDataConflict` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn ScanDataConflict(&mut self) -> Result<(i64, Vec<u8>, Vec<u8>)> {
            let row = self.current()?;
            Ok((row[0].as_i64(), row[1].as_bytes(), row[2].as_bytes()))
        }

        // 语义说明：`Err` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Err(&self) -> Result<()> {
            Ok(())
        }

        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Close(&mut self) -> Result<()> {
            Ok(())
        }
    }

    // 语义说明：这个 impl 块补齐 `SqlValue` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl SqlValue {
        // 语义说明：`as_i64` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn as_i64(&self) -> i64 {
            match self {
                SqlValue::Int64(v) => *v,
                SqlValue::Bool(b) => {
                    if *b {
                        1
                    } else {
                        0
                    }
                }
                _ => 0,
            }
        }
        // 语义说明：`as_string` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn as_string(&self) -> String {
            match self {
                SqlValue::String(s) => s.clone(),
                SqlValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                SqlValue::Int64(v) => v.to_string(),
                SqlValue::Bool(b) => b.to_string(),
                SqlValue::Null => String::new(),
            }
        }
        // 语义说明：`as_bytes` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn as_bytes(&self) -> Vec<u8> {
            match self {
                SqlValue::Bytes(b) => b.clone(),
                SqlValue::String(s) => s.as_bytes().to_vec(),
                SqlValue::Null => Vec::new(),
                SqlValue::Int64(v) => v.to_string().into_bytes(),
                SqlValue::Bool(b) => {
                    if *b {
                        vec![1]
                    } else {
                        vec![0]
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// tablecodec / types / table / tables / encode / kv / mysql / util / tikverr
// ---------------------------------------------------------------------------

// 语义说明：`tablecodec` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod tablecodec {
    use super::{Result, errors};

    // 语义说明：`IsRecordKey` 保留布尔判定入口，供迁移代码沿用与 Go 相同的条件分支。
    pub fn IsRecordKey(k: &[u8]) -> bool {
        k.len() > 11 && k[0] == b't' && k[10] == b'r'
    }

    // 语义说明：`IsIndexKey` 保留布尔判定入口，供迁移代码沿用与 Go 相同的条件分支。
    pub fn IsIndexKey(k: &[u8]) -> bool {
        k.len() > 11 && k[0] == b't' && k[10] == b'i'
    }

    /// Decode index id from encoded index key (simplified Go DecodeIndexKey).
    // 语义说明：`DecodeIndexKey` 维持编码解析辅助函数的最小行为，避免上层在协议边界上失配。
    pub fn DecodeIndexKey(key: &[u8]) -> Result<(i64, i64, Vec<u8>)> {
        if !IsIndexKey(key) {
            return Err(errors::Errorf("not index key"));
        }
        if key.len() < 19 {
            return Err(errors::Errorf("invalid index key"));
        }
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&key[11..19]);
        let idx_id = i64::from_be_bytes(buf);
        Ok((0, idx_id, key[19..].to_vec()))
    }

    #[derive(Clone, Copy, Debug, Default)]
    // 语义说明：`Handle` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Handle(pub i64);

    // 语义说明：这个 impl 块补齐 `Handle` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Handle {
        // 语义说明：`IntValue` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn IntValue(self) -> i64 {
            self.0
        }
    }

    // 语义说明：`DecodeRowKey` 维持编码解析辅助函数的最小行为，避免上层在协议边界上失配。
    pub fn DecodeRowKey(key: &[u8]) -> Result<Handle> {
        if key.len() < 19 {
            return Err(errors::Errorf(format!(
                "invalid record key length {}",
                key.len()
            )));
        }
        // last 8 bytes big-endian handle (simplified)
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&key[key.len() - 8..]);
        Ok(Handle(i64::from_be_bytes(buf)))
    }
}

// 语义说明：`types` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod types {
    #[derive(Clone, Debug)]
    // 语义说明：`Datum` 枚举把 Go 里的有限状态翻成 Rust 可匹配分支，方便维持相同判定语义。
    pub enum Datum {
        Int(i64),
        Bytes(Vec<u8>),
        String(String),
    }

    // 语义说明：`NewIntDatum` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewIntDatum(v: i64) -> Datum {
        Datum::Int(v)
    }
    // 语义说明：`NewStringDatum` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewStringDatum(s: impl Into<String>) -> Datum {
        Datum::String(s.into())
    }
}

// 语义说明：这个 impl 块补齐 `types::Datum` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
impl Default for types::Datum {
    // 语义说明：`default` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    fn default() -> Self {
        types::Datum::Int(0)
    }
}

// 语义说明：`tidbtbl` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod tidbtbl {
    use super::types::Datum;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`TableMeta` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct TableMeta {
        pub clustered: bool,
    }

    // 语义说明：这个 impl 块补齐 `TableMeta` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl TableMeta {
        // 语义说明：`HasClusteredIndex` 保留布尔判定入口，供迁移代码沿用与 Go 相同的条件分支。
        pub fn HasClusteredIndex(&self) -> bool {
            self.clustered
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Column` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Column {
        pub id: i64,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Table` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Table {
        pub meta: TableMeta,
        pub cols: Vec<Column>,
    }

    // 语义说明：这个 impl 块补齐 `Table` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Table {
        // 语义说明：`Meta` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Meta(&self) -> &TableMeta {
            &self.meta
        }
        // 语义说明：`Cols` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Cols(&self) -> &[Column] {
            &self.cols
        }
    }

    /// Trait object stand-in used by ReplaceConflictKeys signature.
    pub type TableRef = Table;
}

// 语义说明：`tables` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod tables {
    use super::Result;
    use super::tablecodec::Handle;
    use super::tidbtbl::{Column, Table};
    use super::types::Datum;

    // 语义说明：`DecodeRawRowData` 维持编码解析辅助函数的最小行为，避免上层在协议边界上失配。
    pub fn DecodeRawRowData(
        _expr_ctx: (),
        _tbl: &Table,
        _handle: Handle,
        _cols: &[Column],
        raw: Vec<u8>,
    ) -> Result<(Vec<Datum>, Vec<()>)> {
        // Treat raw bytes as opaque; produce a single bytes datum for re-encode.
        Ok((vec![Datum::Bytes(raw)], vec![]))
    }
}

// 语义说明：`mysql` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod mysql {
    pub type SQLMode = u64;
    // 语义说明：`ModeNone` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const ModeNone: SQLMode = 0;
    // 语义说明：`ModeStrictAllTables` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const ModeStrictAllTables: u64 = 1;
}

// 语义说明：`encode` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod encode {
    use super::context::Context;
    use super::log::Logger;
    use super::mysql;
    use super::tidbtbl::Table;
    use super::types::Datum;
    use super::{Error, Result};
    use std::collections::HashMap;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`SessionOptions` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct SessionOptions {
        pub SQLMode: u64,
        pub Timestamp: i64,
        pub SysVars: HashMap<String, String>,
        pub AutoRandomSeed: i64,
    }

    // 语义说明：这个 impl 块补齐 `SessionOptions` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl SessionOptions {
        // 语义说明：`strict` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn strict() -> Self {
            Self {
                SQLMode: mysql::ModeStrictAllTables,
                ..Default::default()
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`EncTable` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct EncTable;

    #[derive(Clone, Debug)]
    // 语义说明：`EncodingConfig` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct EncodingConfig {
        pub Table: Table,
        pub SessionOptions: SessionOptions,
        pub Logger: Logger,
        pub Path: String,
        pub EncTable: EncTable,
    }

    // Alternate constructor fields used by dup_detect
    // 语义说明：这个 impl 块补齐 `EncodingConfig` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl EncodingConfig {
        // 语义说明：`from_parts` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn from_parts(
            session: SessionOptions,
            path: String,
            table: EncTable,
            logger: Logger,
        ) -> Self {
            Self {
                Table: Table::default(),
                SessionOptions: session,
                Logger: logger,
                Path: path,
                EncTable: table,
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`KvPair` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct KvPair {
        pub Key: Vec<u8>,
        pub Val: Vec<u8>,
        pub RowID: Vec<u8>,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Row` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Row {
        pub pairs: Vec<KvPair>,
    }

    pub trait Encoder: Send {
        // 语义说明：`Encode` 维持编码解析辅助函数的最小行为，避免上层在协议边界上失配。
        fn Encode(
            &mut self,
            row: &[Datum],
            row_id: i64,
            col_perm: &[i32],
            offset: i64,
        ) -> Result<Row>;
    }

    pub trait EncodingBuilder: Send + Sync {
        // 语义说明：`NewEncoder` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
        fn NewEncoder(&self, ctx: Context, cfg: &EncodingConfig) -> Result<Box<dyn Encoder>>;
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`NoopEncBuilder` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct NoopEncBuilder;

    // 语义说明：这个 impl 块补齐 `NoopEncBuilder` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl EncodingBuilder for NoopEncBuilder {
        // 语义说明：`NewEncoder` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
        fn NewEncoder(&self, _ctx: Context, _cfg: &EncodingConfig) -> Result<Box<dyn Encoder>> {
            Ok(Box::new(NoopEncoder))
        }
    }

    // 语义说明：`NoopEncoder` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    struct NoopEncoder;
    // 语义说明：这个 impl 块补齐 `NoopEncoder` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Encoder for NoopEncoder {
        // 语义说明：`Encode` 维持编码解析辅助函数的最小行为，避免上层在协议边界上失配。
        fn Encode(
            &mut self,
            _row: &[Datum],
            _row_id: i64,
            _col_perm: &[i32],
            _offset: i64,
        ) -> Result<Row> {
            Ok(Row::default())
        }
    }
}

// 语义说明：`kv` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod kv {
    use super::encode::EncodingConfig;
    use super::types::Datum;
    use super::{Result, errors};
    use std::sync::Mutex;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`KvPair` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct KvPair {
        pub Key: Vec<u8>,
        pub Val: Vec<u8>,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Pairs` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Pairs {
        pub Pairs: Vec<KvPair>,
    }

    #[derive(Debug)]
    // 语义说明：`SessionCtx` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct SessionCtx {
        pending: Mutex<Vec<KvPair>>,
        /// Optional map: raw_row -> produced pairs (test-driven).
        encode_map: std::sync::Arc<Mutex<Vec<(Vec<u8>, Vec<KvPair>)>>>,
    }

    // 语义说明：这个 impl 块补齐 `SessionCtx` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl SessionCtx {
        // 语义说明：`GetExprCtx` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn GetExprCtx(&self) {}
        // 语义说明：`TakeKvPairs` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn TakeKvPairs(&self) -> Pairs {
            let mut g = self.pending.lock().unwrap();
            Pairs {
                Pairs: std::mem::take(&mut *g),
            }
        }
    }

    #[derive(Debug)]
    // 语义说明：`BaseKVEncoder` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct BaseKVEncoder {
        pub SessionCtx: SessionCtx,
        encode_map: std::sync::Arc<Mutex<Vec<(Vec<u8>, Vec<KvPair>)>>>,
    }

    // 语义说明：这个 impl 块补齐 `BaseKVEncoder` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl BaseKVEncoder {
        // 语义说明：`AddRecord` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
        pub fn AddRecord(&self, decoded: Vec<Datum>) -> Result<()> {
            // Prefer encode_map lookup by bytes datum; else synthesize identity pairs.
            let raw = decoded
                .iter()
                .find_map(|d| match d {
                    Datum::Bytes(b) => Some(b.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            let pairs = {
                let map = self.encode_map.lock().unwrap();
                map.iter()
                    .find(|(k, _)| k == &raw)
                    .map(|(_, p)| p.clone())
                    .unwrap_or_else(|| {
                        vec![KvPair {
                            Key: raw.clone(),
                            Val: raw.clone(),
                        }]
                    })
            };
            let mut g = self.SessionCtx.pending.lock().unwrap();
            *g = pairs;
            Ok(())
        }
    }

    // 语义说明：`NewBaseKVEncoder` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewBaseKVEncoder(cfg: &EncodingConfig) -> Result<BaseKVEncoder> {
        let _ = cfg;
        let encode_map = std::sync::Arc::new(Mutex::new(Vec::new()));
        Ok(BaseKVEncoder {
            SessionCtx: SessionCtx {
                pending: Mutex::new(Vec::new()),
                encode_map: encode_map.clone(),
            },
            encode_map,
        })
    }

    /// Test helper: register how a raw row encodes into KV pairs.
    // 语义说明：`register_encode` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn register_encode(enc: &BaseKVEncoder, raw_row: Vec<u8>, pairs: Vec<KvPair>) {
        enc.encode_map.lock().unwrap().push((raw_row, pairs));
    }

    // 语义说明：`NewBaseKVEncoderWithMap` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewBaseKVEncoderWithMap(
        cfg: &EncodingConfig,
        encode_map: std::sync::Arc<Mutex<Vec<(Vec<u8>, Vec<KvPair>)>>>,
    ) -> Result<BaseKVEncoder> {
        let _ = cfg;
        Ok(BaseKVEncoder {
            SessionCtx: SessionCtx {
                pending: Mutex::new(Vec::new()),
                encode_map: encode_map.clone(),
            },
            encode_map,
        })
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Storage` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Storage;
    // 语义说明：这个 impl 块补齐 `Storage` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Storage {
        // 语义说明：`GetClient` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn GetClient(&self) {}
    }

    // 语义说明：`Row2KvPairs` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Row2KvPairs(row: &super::encode::Row) -> Vec<super::encode::KvPair> {
        row.pairs.clone()
    }

    // 语义说明：`ClearRow` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn ClearRow(_row: &mut super::encode::Row) {}
}
// 语义说明：`tikverr` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod tikverr {
    use super::Error;

    // 语义说明：`IsErrNotFound` 保留布尔判定入口，供迁移代码沿用与 Go 相同的条件分支。
    pub fn IsErrNotFound(err: &Error) -> bool {
        err.not_found
    }

    // 语义说明：`ErrNotFound` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn ErrNotFound(msg: impl Into<String>) -> Error {
        Error {
            msg: msg.into(),
            not_found: true,
            cause: None,
            class: None,
        }
    }
}

// 语义说明：`util` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod util {
    use super::Result;
    use super::errgroup::Group;

    #[derive(Clone, Debug)]
    // 语义说明：`WorkerPool` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct WorkerPool {
        _size: usize,
        _name: String,
    }

    // 语义说明：这个 impl 块补齐 `WorkerPool` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl WorkerPool {
        // 语义说明：`New` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
        pub fn New(size: usize, name: &str) -> Self {
            Self {
                _size: size.max(1),
                _name: name.to_string(),
            }
        }

        /// Spawn `f` onto the error group (async, matching Go ApplyOnErrorGroup).
        // 语义说明：`ApplyOnErrorGroup` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn ApplyOnErrorGroup<F>(&self, g: &Group, f: F)
        where
            F: FnOnce() -> Result<()> + Send + 'static,
        {
            g.Go(f);
        }
    }
}

// 语义说明：`errgroup` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod errgroup {
    use super::context::{CancelFunc, Context};
    use super::{Result, errors};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};

    // 语义说明：`Group` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Group {
        handles: Mutex<Vec<JoinHandle<Result<()>>>>,
        first_err: Arc<Mutex<Option<super::Error>>>,
        cancel: CancelFunc,
        _ctx: Context,
    }

    // 语义说明：`WithContext` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn WithContext(ctx: Context) -> (Group, Context) {
        let (child, cancel) = super::context::WithCancel(ctx.clone());
        (
            Group {
                handles: Mutex::new(Vec::new()),
                first_err: Arc::new(Mutex::new(None)),
                cancel,
                _ctx: ctx,
            },
            child,
        )
    }

    // 语义说明：这个 impl 块补齐 `Group` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Group {
        // 语义说明：`Go` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Go<F>(&self, f: F)
        where
            F: FnOnce() -> Result<()> + Send + 'static,
        {
            let first_err = self.first_err.clone();
            let cancel = self.cancel.clone();
            let handle = thread::spawn(move || match f() {
                Ok(()) => Ok(()),
                Err(e) => {
                    cancel.cancel();
                    let mut g = first_err.lock().unwrap();
                    if g.is_none() {
                        *g = Some(e.clone());
                    }
                    Err(e)
                }
            });
            self.handles.lock().unwrap().push(handle);
        }

        // 语义说明：`Wait` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Wait(&self) -> Result<()> {
            let handles = std::mem::take(&mut *self.handles.lock().unwrap());
            for h in handles {
                match h.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        let mut g = self.first_err.lock().unwrap();
                        if g.is_none() {
                            *g = Some(e);
                        }
                    }
                    Err(_) => {
                        let mut g = self.first_err.lock().unwrap();
                        if g.is_none() {
                            *g = Some(errors::New("worker panicked"));
                        }
                    }
                }
            }
            // x/sync/errgroup cancels the derived context when Wait returns,
            // regardless of whether a worker reported an error.
            self.cancel.clone().cancel();
            match self.first_err.lock().unwrap().take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    }
}

// 语义说明：`pretty_table` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod pretty_table {
    //! Minimal go-pretty StyleDefault + FgRed row painter renderer.

    const FG_RED: &str = "\x1b[31m";
    const RESET: &str = "\x1b[0m";

    // 语义说明：`render` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn render(headers: &[&str], rows: &[Vec<String>]) -> String {
        let ncols = headers.len();
        let mut widths = vec![0usize; ncols];
        for (i, h) in headers.iter().enumerate() {
            widths[i] = h.to_ascii_uppercase().len();
        }
        for row in rows {
            for (i, cell) in row.iter().enumerate() {
                widths[i] = widths[i].max(cell.len());
            }
        }

        let mut out = String::new();
        out.push_str(&separator(&widths));
        out.push('\n');
        // header
        out.push('|');
        for (i, h) in headers.iter().enumerate() {
            let label = h.to_ascii_uppercase();
            out.push(' ');
            out.push_str(&pad_right(&label, widths[i]));
            out.push(' ');
            out.push('|');
        }
        out.push('\n');
        out.push_str(&separator(&widths));
        out.push('\n');
        for row in rows {
            out.push('|');
            for (i, cell) in row.iter().enumerate() {
                let padded = if i == 0 || i == 2 {
                    pad_left(cell, widths[i])
                } else {
                    pad_right(cell, widths[i])
                };
                out.push_str(FG_RED);
                out.push(' ');
                out.push_str(&padded);
                out.push(' ');
                out.push_str(RESET);
                out.push('|');
            }
            out.push('\n');
        }
        out.push_str(&separator(&widths));
        out.push('\n');
        out
    }

    // 语义说明：`separator` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    fn separator(widths: &[usize]) -> String {
        let mut s = String::from("+");
        for w in widths {
            s.push_str(&"-".repeat(w + 2));
            s.push('+');
        }
        s
    }

    // 语义说明：`pad_right` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    fn pad_right(s: &str, w: usize) -> String {
        format!("{s:<w$}")
    }
    // 语义说明：`pad_left` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    fn pad_left(s: &str, w: usize) -> String {
        format!("{s:>w$}")
    }
}

// ---------------------------------------------------------------------------
// importer-specific stubs (model/mydump/importdef/pd/backend/...)
// ---------------------------------------------------------------------------

// 语义说明：`model` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod model {
    #[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
    // 语义说明：`CIStr` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct CIStr {
        pub O: String,
        pub L: String,
    }

    // 语义说明：这个 impl 块补齐 `CIStr` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl CIStr {
        // 语义说明：`new` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn new(s: &str) -> Self {
            Self {
                O: s.to_string(),
                L: s.to_ascii_lowercase(),
            }
        }
        // 语义说明：`String` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn String(&self) -> String {
            self.O.clone()
        }
    }

    // 语义说明：这个 impl 块补齐 `CIStr` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl std::fmt::Display for CIStr {
        // 语义说明：`fmt` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.O)
        }
    }

    // 语义说明：`ExtraHandleName` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn ExtraHandleName() -> CIStr {
        CIStr::new("_tidb_rowid")
    }

    // 语义说明：`StatePublic` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const StatePublic: i32 = 4;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`FieldType` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct FieldType {
        pub Tp: u8,
        pub Flen: i32,
    }

    // 语义说明：这个 impl 块补齐 `FieldType` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl std::fmt::Display for FieldType {
        // 语义说明：`fmt` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "FieldType({})", self.Tp)
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`ColumnInfo` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct ColumnInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub Offset: i32,
        pub Hidden: bool,
        pub GeneratedExprString: String,
        pub FieldType: FieldType,
        pub DefaultValue: Option<String>,
        pub NotNull: bool,
        pub AutoIncrement: bool,
        pub State: i32,
    }

    // 语义说明：这个 impl 块补齐 `ColumnInfo` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl ColumnInfo {
        // 语义说明：`IsGenerated` 保留布尔判定入口，供迁移代码沿用与 Go 相同的条件分支。
        pub fn IsGenerated(&self) -> bool {
            !self.GeneratedExprString.is_empty()
        }
        // 语义说明：`Clone` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Clone(&self) -> Self {
            self.clone()
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`IndexColumn` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct IndexColumn {
        pub Name: CIStr,
        pub Offset: i32,
        pub Length: i32,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`IndexInfo` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct IndexInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub Columns: Vec<IndexColumn>,
        pub Primary: bool,
        pub Unique: bool,
        pub State: i32,
    }

    // 语义说明：这个 impl 块补齐 `IndexInfo` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl IndexInfo {
        // 语义说明：`Clone` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Clone(&self) -> Self {
            self.clone()
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`TableInfo` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct TableInfo {
        pub ID: i64,
        pub Name: CIStr,
        pub Columns: Vec<ColumnInfo>,
        pub Indices: Vec<IndexInfo>,
        pub PKIsHandle: bool,
        pub IsCommonHandle: bool,
        pub State: i32,
        pub AutoIncID: i64,
        pub AutoRandID: i64,
        pub AutoRandomBits: u64,
        pub AutoRandomRangeBits: u64,
    }

    // 语义说明：这个 impl 块补齐 `TableInfo` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl TableInfo {
        // 语义说明：`Clone` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Clone(&self) -> Self {
            self.clone()
        }
        // 语义说明：`GetPkColInfo` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn GetPkColInfo(&self) -> Option<&ColumnInfo> {
            if !self.PKIsHandle {
                return None;
            }
            self.Columns.first()
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`DBInfo` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct DBInfo {
        pub Name: CIStr,
        pub Tables: Vec<TableInfo>,
    }
}

// 语义说明：`importdef` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod importdef {
    use super::model;
    use std::collections::HashMap;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`TableInfo` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct TableInfo {
        pub ID: i64,
        pub DB: String,
        pub Name: String,
        pub Core: model::TableInfo,
        pub Desired: Option<model::TableInfo>,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`DBInfo` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct DBInfo {
        pub Name: String,
        pub Tables: HashMap<String, TableInfo>,
    }
}

// 语义说明：`mydump` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod mydump {
    use super::config::Config;
    use super::context::Context;
    use super::storeapi::Storage;
    use super::{Error, Result};
    use std::sync::Arc;

    pub type SourceType = i32;
    // 语义说明：`SourceTypeCSV` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const SourceTypeCSV: SourceType = 1;
    // 语义说明：`SourceTypeSQL` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const SourceTypeSQL: SourceType = 2;
    // 语义说明：`SourceTypeParquet` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const SourceTypeParquet: SourceType = 3;

    pub type Compression = i32;
    // 语义说明：`CompressionNone` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const CompressionNone: Compression = 0;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`ExtendColumnData` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct ExtendColumnData {
        pub Columns: Vec<String>,
        pub Values: Vec<String>,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`SourceFileMeta` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct SourceFileMeta {
        pub Path: String,
        pub Type: SourceType,
        pub Compression: Compression,
        pub SortKey: String,
        pub FileSize: i64,
        pub RealSize: i64,
        pub ExtendData: ExtendColumnData,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Chunk` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Chunk {
        pub Offset: i64,
        pub RealOffset: i64,
        pub EndOffset: i64,
        pub PrevRowIDMax: i64,
        pub RowIDMax: i64,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`FileInfo` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct FileInfo {
        pub TableName: String,
        pub FileMeta: SourceFileMeta,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`MDTableMeta` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct MDTableMeta {
        pub DB: String,
        pub Name: String,
        pub TotalSize: i64,
        pub DataFiles: Vec<FileInfo>,
        pub SchemaFile: Option<SourceFileMeta>,
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`MDDatabaseMeta` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct MDDatabaseMeta {
        pub Name: String,
        pub Tables: Vec<MDTableMeta>,
    }

    pub type MDLoaderSetupOption = std::sync::Arc<dyn Fn(&mut MDLoaderSetupConfig) + Send + Sync>;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`MDLoaderSetupConfig` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct MDLoaderSetupConfig {
        pub scan_file_concurrency: usize,
    }

    // 语义说明：`WithScanFileConcurrency` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn WithScanFileConcurrency(concurrency: usize) -> MDLoaderSetupOption {
        std::sync::Arc::new(move |c: &mut MDLoaderSetupConfig| {
            c.scan_file_concurrency = concurrency.max(1);
        })
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Loader` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Loader {
        pub dbs: Vec<MDDatabaseMeta>,
        pub store: Storage,
        pub partial_err: Option<Error>,
    }

    // 语义说明：这个 impl 块补齐 `Loader` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Loader {
        // 语义说明：`GetDatabases` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn GetDatabases(&self) -> Vec<MDDatabaseMeta> {
            self.dbs.clone()
        }
        // 语义说明：`GetStore` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn GetStore(&self) -> Storage {
            self.store.clone()
        }
    }

    // 语义说明：`NewLoaderCfg` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewLoaderCfg(_cfg: &Config) -> LoaderCfg {
        LoaderCfg::default()
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`LoaderCfg` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct LoaderCfg;

    // 语义说明：`NewLoader` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewLoader(
        _ctx: Context,
        _cfg: LoaderCfg,
        _opts: Vec<MDLoaderSetupOption>,
    ) -> std::result::Result<Loader, (Option<Loader>, Error)> {
        Ok(Loader::default())
    }
}

// 语义说明：`storeapi` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod storeapi {
    use super::{Error, Result};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    // 语义说明：`LocalURIPrefix` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const LocalURIPrefix: &str = "file://";

    #[derive(Clone, Default)]
    // 语义说明：`Storage` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Storage {
        pub uri: String,
        pub closed: Arc<Mutex<bool>>,
        pub objects: Arc<Mutex<Vec<String>>>,
        pub data: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    }

    // 语义说明：这个 impl 块补齐 `Storage` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl std::fmt::Debug for Storage {
        // 语义说明：`fmt` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "Storage({})", self.uri)
        }
    }

    // 语义说明：这个 impl 块补齐 `Storage` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Storage {
        // 语义说明：`new` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn new(uri: impl Into<String>) -> Self {
            Self {
                uri: uri.into(),
                ..Default::default()
            }
        }
        // 语义说明：`URI` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn URI(&self) -> String {
            self.uri.clone()
        }
        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Close(&self) -> Result<()> {
            *self.closed.lock().unwrap() = true;
            Ok(())
        }
        // 语义说明：`is_closed` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn is_closed(&self) -> bool {
            *self.closed.lock().unwrap()
        }

        pub fn Put(&self, path: impl Into<String>, content: impl Into<Vec<u8>>) {
            let path = path.into();
            self.data
                .lock()
                .unwrap()
                .insert(path.clone(), content.into());
            let mut objects = self.objects.lock().unwrap();
            if !objects.contains(&path) {
                objects.push(path);
            }
        }

        pub fn Read(&self, path: &str) -> Result<Vec<u8>> {
            if let Some(content) = self.data.lock().unwrap().get(path).cloned() {
                return Ok(content);
            }

            if self.uri.starts_with(LocalURIPrefix) {
                let root = self.uri.trim_start_matches(LocalURIPrefix);
                let candidate = if std::path::Path::new(path).is_absolute()
                    && std::path::Path::new(path).exists()
                {
                    std::path::PathBuf::from(path)
                } else {
                    std::path::Path::new(root).join(path.trim_start_matches('/'))
                };
                return std::fs::read(&candidate).map_err(|e| {
                    Error::new(format!("read source file '{}': {e}", candidate.display()))
                });
            }

            Err(Error::new(format!("source object not found: {path}")))
        }
    }
}

// 语义说明：`objstore` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod objstore {
    pub use super::storeapi::LocalURIPrefix;
}

// 语义说明：`verify` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod verify {
    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    // 语义说明：`KVChecksum` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct KVChecksum {
        bytes: u64,
        kvs: u64,
        checksum: u64,
    }

    // 语义说明：`MakeKVChecksum` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn MakeKVChecksum(bytes: u64, kvs: u64, checksum: u64) -> KVChecksum {
        KVChecksum {
            bytes,
            kvs,
            checksum,
        }
    }

    // 语义说明：这个 impl 块补齐 `KVChecksum` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl KVChecksum {
        // 语义说明：`SumSize` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn SumSize(&self) -> u64 {
            self.bytes
        }
        // 语义说明：`SumKVS` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn SumKVS(&self) -> u64 {
            self.kvs
        }
        // 语义说明：`Sum` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Sum(&self) -> u64 {
            self.checksum
        }
        // 语义说明：`Add` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
        pub fn Add(&mut self, other: &KVChecksum) {
            self.bytes += other.bytes;
            self.kvs += other.kvs;
            self.checksum ^= other.checksum;
        }
    }
}

// 语义说明：`ingestctrl` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod ingestctrl {
    use super::context::Context;
    use super::importdef;
    use super::sql::DB;
    use super::{Error, Result};
    use std::sync::Arc;

    // 语义说明：`DefaultBackoffWeight` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const DefaultBackoffWeight: i32 = 2;

    #[derive(Clone, Debug, Default, PartialEq, Eq)]
    // 语义说明：`RemoteChecksum` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct RemoteChecksum {
        pub Schema: String,
        pub Table: String,
        pub Checksum: u64,
        pub TotalKVs: u64,
        pub TotalBytes: u64,
    }

    pub trait ChecksumManager: Send + Sync {
        // 语义说明：`Checksum` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Checksum(&self, ctx: Context, table: &importdef::TableInfo) -> Result<RemoteChecksum>;
        // 语义说明：`kind` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn kind(&self) -> &'static str;
    }

    #[derive(Clone, Debug)]
    // 语义说明：`TiKVChecksumManager` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct TiKVChecksumManager {
        pub concurrency: u32,
        pub backoff_weight: i32,
        pub resource_group: String,
        pub task_type: String,
    }

    // 语义说明：这个 impl 块补齐 `TiKVChecksumManager` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl ChecksumManager for TiKVChecksumManager {
        // 语义说明：`Checksum` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Checksum(&self, _ctx: Context, table: &importdef::TableInfo) -> Result<RemoteChecksum> {
            Ok(RemoteChecksum {
                Schema: table.DB.clone(),
                Table: table.Name.clone(),
                ..Default::default()
            })
        }
        // 语义说明：`kind` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn kind(&self) -> &'static str {
            "tikv"
        }
    }

    #[derive(Clone, Debug)]
    // 语义说明：`TiDBChecksumExecutor` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct TiDBChecksumExecutor {
        pub db: DB,
    }

    // 语义说明：这个 impl 块补齐 `TiDBChecksumExecutor` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl ChecksumManager for TiDBChecksumExecutor {
        // 语义说明：`Checksum` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Checksum(&self, _ctx: Context, table: &importdef::TableInfo) -> Result<RemoteChecksum> {
            Ok(RemoteChecksum {
                Schema: table.DB.clone(),
                Table: table.Name.clone(),
                ..Default::default()
            })
        }
        // 语义说明：`kind` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn kind(&self) -> &'static str {
            "tidb"
        }
    }

    // 语义说明：`NewTiKVChecksumManager` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewTiKVChecksumManager(
        _client: (),
        _pd: (),
        concurrency: u32,
        backoff_weight: i32,
        resource_group: String,
        task_type: String,
    ) -> TiKVChecksumManager {
        TiKVChecksumManager {
            concurrency,
            backoff_weight,
            resource_group,
            task_type,
        }
    }

    // 语义说明：`NewTiDBChecksumExecutor` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewTiDBChecksumExecutor(db: DB) -> TiDBChecksumExecutor {
        TiDBChecksumExecutor { db }
    }

    // 语义说明：`EstimateCompactionThreshold2` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn EstimateCompactionThreshold2(total_raw_file_size: i64) -> i64 {
        // Mirror Go ingestctrl.EstimateCompactionThreshold2: bound SST size.
        const MAX: i64 = 32 * 1024 * 1024 * 1024;
        if total_raw_file_size <= 0 {
            return 0;
        }
        let thr = total_raw_file_size / 500;
        if thr > MAX { MAX } else { thr }
    }

    pub trait TiKVModeSwitcher: Send + Sync {
        // 语义说明：`ToImportMode` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn ToImportMode(&self, ctx: Context) -> Result<()>;
        // 语义说明：`ToNormalMode` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn ToNormalMode(&self, ctx: Context) -> Result<()>;
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`NoopModeSwitcher` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct NoopModeSwitcher;

    // 语义说明：这个 impl 块补齐 `NoopModeSwitcher` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl TiKVModeSwitcher for NoopModeSwitcher {
        // 语义说明：`ToImportMode` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn ToImportMode(&self, _ctx: Context) -> Result<()> {
            Ok(())
        }
        // 语义说明：`ToNormalMode` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn ToNormalMode(&self, _ctx: Context) -> Result<()> {
            Ok(())
        }
    }
}

// 语义说明：`pdutil` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod pdutil {
    use super::context::Context;
    use super::{Error, Result};
    use std::sync::Arc;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Version` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Version {
        pub Major: i64,
        pub Minor: i64,
        pub Patch: i64,
    }

    #[derive(Clone, Default)]
    // 语义说明：`PdController` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct PdController {
        pub paused: Arc<std::sync::Mutex<bool>>,
    }

    // 语义说明：这个 impl 块补齐 `PdController` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl std::fmt::Debug for PdController {
        // 语义说明：`fmt` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("PdController")
        }
    }

    pub type UndoFunc = Arc<dyn Fn(Context) -> Result<()> + Send + Sync>;

    // 语义说明：`NopUndo` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn NopUndo() -> UndoFunc {
        Arc::new(|_ctx| Ok(()))
    }

    // 语义说明：`FetchPDVersion` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
    pub fn FetchPDVersion(_ctx: Context, _cli: PdHTTPClient) -> Result<Version> {
        Ok(Version {
            Major: 6,
            Minor: 0,
            Patch: 0,
        })
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`PdHTTPClient` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct PdHTTPClient {
        pub leader_urls: Vec<String>,
        pub fail_leader: bool,
        pub max_replicas: u64,
        pub stores: Vec<(u64, String, u64, u64, i64, i64)>,
        pub empty_regions: Vec<(u64, u64)>,
        pub request_error: Option<String>,
    }

    // 语义说明：这个 impl 块补齐 `PdHTTPClient` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl PdHTTPClient {
        // 语义说明：`GetLeader` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn GetLeader(&self, _ctx: Context) -> Result<LeaderInfo> {
            if self.fail_leader {
                return Err(Error::new("leader not found"));
            }
            Ok(LeaderInfo {
                client_urls: self.leader_urls.clone(),
            })
        }
        // 语义说明：`is_nil` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn is_nil(&self) -> bool {
            false
        }
    }

    // treat default as nil-like for builder
    // 语义说明：`nil_pd_http` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn nil_pd_http() -> Option<PdHTTPClient> {
        None
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`LeaderInfo` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct LeaderInfo {
        pub client_urls: Vec<String>,
    }

    // 语义说明：这个 impl 块补齐 `LeaderInfo` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl LeaderInfo {
        // 语义说明：`GetClientUrls` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn GetClientUrls(&self) -> Vec<String> {
            self.client_urls.clone()
        }
    }

    pub type PdHTTPClientOpt = Option<PdHTTPClient>;
}

// 语义说明：`pdhttp` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod pdhttp {
    pub use super::pdutil::{LeaderInfo, PdHTTPClient as Client};
}

// 语义说明：`pd` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod pd {
    #[derive(Clone, Debug, Default)]
    // 语义说明：`Client` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Client;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`APIContext` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct APIContext;
}

// 语义说明：`backend` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod backend {
    use super::context::Context;
    use super::{Error, Result};

    pub trait Backend: Send + Sync {
        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Close(&self);
        // 语义说明：`Name` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Name(&self) -> &str;
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`EngineManager` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct EngineManager;

    // 语义说明：这个 impl 块补齐 `EngineManager` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl EngineManager {
        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Close(&self) {}
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`LocalBackend` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct LocalBackend {
        pub name: String,
    }

    // 语义说明：这个 impl 块补齐 `LocalBackend` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Backend for LocalBackend {
        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Close(&self) {}
        // 语义说明：`Name` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Name(&self) -> &str {
            "local"
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`TidbBackend` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct TidbBackend;

    // 语义说明：这个 impl 块补齐 `TidbBackend` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Backend for TidbBackend {
        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Close(&self) {}
        // 语义说明：`Name` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Name(&self) -> &str {
            "tidb"
        }
    }
}

// 语义说明：`worker` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod worker {
    use std::sync::Arc;

    #[derive(Clone, Debug)]
    // 语义说明：`Pool` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Pool {
        pub size: usize,
        pub name: String,
    }

    // 语义说明：这个 impl 块补齐 `Pool` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Pool {
        // 语义说明：`New` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
        pub fn New(size: usize, name: &str) -> Arc<Self> {
            Arc::new(Self {
                size: size.max(1),
                name: name.into(),
            })
        }
        // 语义说明：`Apply` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Apply<F: FnOnce()>(&self, f: F) {
            f();
        }
    }
}

// 语义说明：`vardef` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod vardef {
    // 语义说明：`TiDBBuildStatsConcurrency` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const TiDBBuildStatsConcurrency: &str = "tidb_build_stats_concurrency";
    // 语义说明：`TiDBDistSQLScanConcurrency` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const TiDBDistSQLScanConcurrency: &str = "tidb_distsql_scan_concurrency";
    // 语义说明：`TiDBIndexSerialScanConcurrency` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const TiDBIndexSerialScanConcurrency: &str = "tidb_index_serial_scan_concurrency";
    // 语义说明：`TiDBChecksumTableConcurrency` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const TiDBChecksumTableConcurrency: &str = "tidb_checksum_table_concurrency";
    // 语义说明：`TiDBAllowAutoRandExplicitInsert` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const TiDBAllowAutoRandExplicitInsert: &str = "tidb_allow_auto_rand_explicit_insert";
    // 语义说明：`TiDBOptWriteRowID` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const TiDBOptWriteRowID: &str = "tidb_opt_write_rowid";
    // 语义说明：`AutoCommit` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const AutoCommit: &str = "autocommit";
    // 语义说明：`TiDBTxnMode` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const TiDBTxnMode: &str = "tidb_txn_mode";
    // 语义说明：`ForeignKeyChecks` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const ForeignKeyChecks: &str = "foreign_key_checks";
    // 语义说明：`TiDBExplicitRequestSourceType` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const TiDBExplicitRequestSourceType: &str = "tidb_explicit_request_source_type";
}

// 语义说明：`tikv_util` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod tikv_util {
    // 语义说明：`ExplicitTypeImport` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const ExplicitTypeImport: &str = "lightning";
}

// 语义说明：`parser` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod parser {
    use super::model::TableInfo;
    use super::{Error, Result};

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Parser` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Parser {
        pub sql_mode: u64,
    }

    // 语义说明：这个 impl 块补齐 `Parser` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Parser {
        // 语义说明：`New` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
        pub fn New() -> Self {
            Self::default()
        }
        // 语义说明：`SetSQLMode` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
        pub fn SetSQLMode(&mut self, mode: u64) {
            self.sql_mode = mode;
        }
        // 语义说明：`Parse` 维持编码解析辅助函数的最小行为，避免上层在协议边界上失配。
        pub fn Parse(sql: &str) -> Result<TableInfo> {
            let lower = sql.to_ascii_lowercase();
            let name = if let Some(idx) = lower.find("create table") {
                let rest = sql[idx + "create table".len()..].trim();
                let rest = rest.strip_prefix("if not exists").unwrap_or(rest).trim();
                let tok = rest.split_whitespace().next().unwrap_or("t");
                tok.trim_matches('`')
                    .split('.')
                    .next_back()
                    .unwrap_or(tok)
                    .trim_matches('`')
                    .to_string()
            } else {
                return Err(Error::new(
                    "cannot transfer the parsed SQL as an CREATE TABLE statement",
                ));
            };
            let mut columns = Vec::new();
            if let (Some(start), Some(end)) = (sql.find('('), sql.rfind(')')) {
                let body = &sql[start + 1..end];
                let mut definitions = Vec::new();
                let mut depth = 0i32;
                let mut quoted = None;
                let mut begin = 0usize;
                for (offset, ch) in body.char_indices() {
                    if matches!(ch, '\'' | '"' | '`') {
                        if quoted == Some(ch) {
                            quoted = None;
                        } else if quoted.is_none() {
                            quoted = Some(ch);
                        }
                    } else if quoted.is_none() {
                        match ch {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            ',' if depth == 0 => {
                                definitions.push(body[begin..offset].trim());
                                begin = offset + 1;
                            }
                            _ => {}
                        }
                    }
                }
                definitions.push(body[begin..].trim());
                for definition in definitions {
                    let trimmed = definition.trim();
                    let keyword = trimmed
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .trim_matches('`');
                    if matches!(
                        keyword.to_ascii_lowercase().as_str(),
                        "primary" | "key" | "unique" | "constraint" | "index"
                    ) {
                        continue;
                    }
                    let lower_definition = trimmed.to_ascii_lowercase();
                    let default_value = lower_definition.find(" default ").and_then(|offset| {
                        trimmed[offset + 9..]
                            .split_whitespace()
                            .next()
                            .map(|value| value.trim_matches(['\'', '"']).to_string())
                    });
                    columns.push(super::model::ColumnInfo {
                        ID: columns.len() as i64 + 1,
                        Name: super::model::CIStr::new(keyword),
                        Offset: columns.len() as i32,
                        DefaultValue: default_value,
                        NotNull: lower_definition.contains(" not null"),
                        AutoIncrement: lower_definition.contains("auto_increment"),
                        State: super::model::StatePublic,
                        ..Default::default()
                    });
                }
            }
            let auto_random = lower.find("auto_random").map(|offset| {
                let suffix = sql[offset + "auto_random".len()..].trim_start();
                if let Some(args) = suffix.strip_prefix('(').and_then(|s| s.split(')').next()) {
                    let mut values = args.split(',').map(|value| value.trim().parse::<u64>());
                    (
                        values.next().and_then(|value| value.ok()).unwrap_or(5),
                        values.next().and_then(|value| value.ok()).unwrap_or(64),
                    )
                } else {
                    (5, 64)
                }
            });
            Ok(TableInfo {
                Name: super::model::CIStr::new(&name),
                Columns: columns,
                AutoRandomBits: auto_random.map(|value| value.0).unwrap_or(0),
                AutoRandomRangeBits: auto_random.map(|value| value.1).unwrap_or(0),
                State: super::model::StatePublic,
                ..Default::default()
            })
        }
    }
}

// 语义说明：`version` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod version {
    // 语义说明：`NextMajorVersion` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn NextMajorVersion() -> Semver {
        Semver {
            Major: 9,
            Minor: 0,
            Patch: 0,
        }
    }

    #[derive(Clone, Debug)]
    // 语义说明：`Semver` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Semver {
        pub Major: u64,
        pub Minor: u64,
        pub Patch: u64,
    }

    // 语义说明：这个 impl 块补齐 `Semver` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Semver {
        // 语义说明：`New` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
        pub fn New(s: &str) -> Self {
            let mut parts = s.split('.');
            Self {
                Major: parts.next().unwrap_or("0").parse().unwrap_or(0),
                Minor: parts.next().unwrap_or("0").parse().unwrap_or(0),
                Patch: parts.next().unwrap_or("0").parse().unwrap_or(0),
            }
        }
    }
}

// 语义说明：`semver` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod semver {
    pub use super::version::Semver as Version;
    // 语义说明：`New` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn New(s: &str) -> Version {
        Version::New(s)
    }
}

// 语义说明：`set` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod set {
    use std::collections::HashSet;

    #[derive(Clone, Debug, Default)]
    // 语义说明：`StringSet` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct StringSet(HashSet<String>);

    // 语义说明：这个 impl 块补齐 `StringSet` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl StringSet {
        // 语义说明：`New` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
        pub fn New(items: &[String]) -> Self {
            Self(items.iter().cloned().collect())
        }
        // 语义说明：`Exist` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Exist(&self, s: &str) -> bool {
            self.0.contains(s)
        }
    }
}

// 语义说明：`codec` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod codec {
    use super::{Error, Result};

    // 语义说明：`EncodeVarint` 维持编码解析辅助函数的最小行为，避免上层在协议边界上失配。
    pub fn EncodeVarint(buf: Option<Vec<u8>>, v: i64) -> Vec<u8> {
        let mut out = buf.unwrap_or_default();
        let mut ux = ((v as u64) << 1) ^ ((v >> 63) as u64);
        while ux >= 0x80 {
            out.push((ux as u8) | 0x80);
            ux >>= 7;
        }
        out.push(ux as u8);
        out
    }

    /// Decode a value previously encoded by [`EncodeVarint`].
    /// Returns `(leftover, value)` matching Go `codec.DecodeVarint`.
    // 语义说明：`DecodeVarint` 维持编码解析辅助函数的最小行为，避免上层在协议边界上失配。
    pub fn DecodeVarint(b: &[u8]) -> Result<(Vec<u8>, i64)> {
        let mut ux: u64 = 0;
        let mut s = 0u32;
        let mut i = 0usize;
        loop {
            if i >= b.len() {
                return Err(Error::new("insufficient bytes to decode value"));
            }
            let x = b[i];
            i += 1;
            if x < 0x80 {
                if i > 9 || (i == 9 && x > 1) {
                    return Err(Error::new("value larger than 64 bits"));
                }
                ux |= (x as u64) << s;
                break;
            }
            ux |= ((x & 0x7f) as u64) << s;
            s += 7;
        }
        let mut v = (ux >> 1) as i64;
        if ux & 1 != 0 {
            v = !v;
        }
        Ok((b[i..].to_vec(), v))
    }
}

// 语义说明：`extsort` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod extsort {
    use super::context::Context;
    use super::{Error, Result};
    use std::sync::{Arc, Mutex};

    pub trait ExternalSorter: Send + Sync {
        // 语义说明：`NewWriter` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
        fn NewWriter(&self, ctx: Context) -> Result<Box<dyn Writer>>;
        // 语义说明：`CloseAndCleanup` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn CloseAndCleanup(&self) -> Result<()>;
    }

    pub trait Writer: Send {
        // 语义说明：`Put` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Put(&mut self, key: &[u8], val: &[u8]) -> Result<()>;
        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Close(&mut self) -> Result<()>;
    }

    #[derive(Clone, Default)]
    // 语义说明：`DiskSorter` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct DiskSorter {
        pub closed: Arc<Mutex<bool>>,
        pub records: Arc<Mutex<Vec<(Vec<u8>, Vec<u8>)>>>,
    }

    // 语义说明：这个 impl 块补齐 `DiskSorter` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl ExternalSorter for DiskSorter {
        // 语义说明：`NewWriter` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
        fn NewWriter(&self, _ctx: Context) -> Result<Box<dyn Writer>> {
            Ok(Box::new(MemWriter {
                records: self.records.clone(),
            }))
        }
        // 语义说明：`CloseAndCleanup` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn CloseAndCleanup(&self) -> Result<()> {
            *self.closed.lock().unwrap() = true;
            Ok(())
        }
    }

    // 语义说明：`MemWriter` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    struct MemWriter {
        records: Arc<Mutex<Vec<(Vec<u8>, Vec<u8>)>>>,
    }

    // 语义说明：这个 impl 块补齐 `MemWriter` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Writer for MemWriter {
        // 语义说明：`Put` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Put(&mut self, key: &[u8], val: &[u8]) -> Result<()> {
            self.records
                .lock()
                .unwrap()
                .push((key.to_vec(), val.to_vec()));
            Ok(())
        }
        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Close(&mut self) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`DiskSorterOptions` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct DiskSorterOptions {
        pub Concurrency: i32,
    }

    // 语义说明：`OpenDiskSorter` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn OpenDiskSorter(_dir: &str, _opts: &DiskSorterOptions) -> Result<DiskSorter> {
        Ok(DiskSorter::default())
    }
}

// 语义说明：`duplicate` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod duplicate {
    use super::context::Context;
    use super::extsort::ExternalSorter;
    use super::log::Logger;
    use super::{Error, Result};
    use std::sync::Arc;

    pub type HandlerConstructor = Arc<dyn Fn(Context) -> Result<Box<dyn Handler>> + Send + Sync>;

    pub trait Handler: Send {
        // 语义说明：`Begin` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Begin(&mut self, key: &[u8]) -> Result<()>;
        // 语义说明：`Append` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
        fn Append(&mut self, key_id: &[u8]) -> Result<()>;
        // 语义说明：`End` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn End(&mut self) -> Result<()>;
        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn Close(&mut self) -> Result<()>;
    }

    #[derive(Clone, Default)]
    // 语义说明：`DetectOptions` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct DetectOptions {
        pub Concurrency: i32,
        pub HandlerConstructor: Option<HandlerConstructor>,
    }

    // 语义说明：这个 impl 块补齐 `DetectOptions` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl std::fmt::Debug for DetectOptions {
        // 语义说明：`fmt` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("DetectOptions")
                .field("Concurrency", &self.Concurrency)
                .field("HandlerConstructor", &self.HandlerConstructor.is_some())
                .finish()
        }
    }

    // 语义说明：`Detector` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Detector {
        pub logger: Logger,
    }

    // 语义说明：这个 impl 块补齐 `Detector` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Detector {
        // 语义说明：`Detect` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Detect(&self, _ctx: Context, _opts: &DetectOptions) -> Result<i64> {
            Ok(0)
        }
        // 语义说明：`KeyAdder` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn KeyAdder(&self, _ctx: Context) -> Result<KeyAdder> {
            Ok(KeyAdder::default())
        }
    }

    // 语义说明：`NewDetector` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewDetector(_sorter: Arc<dyn ExternalSorter>, logger: Logger) -> Detector {
        Detector { logger }
    }

    #[derive(Default)]
    // 语义说明：`KeyAdder` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct KeyAdder {
        closed: bool,
    }

    // 语义说明：这个 impl 块补齐 `KeyAdder` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl KeyAdder {
        // 语义说明：`Add` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
        pub fn Add(&mut self, _key: &[u8], _row_id: &[u8]) -> Result<()> {
            Ok(())
        }
        // 语义说明：`Flush` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Flush(&mut self) -> Result<()> {
            Ok(())
        }
        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Close(&mut self) -> Result<()> {
            self.closed = true;
            Ok(())
        }
    }
}

// 语义说明：`autoid` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod autoid {
    #[derive(Clone, Debug, Default)]
    // 语义说明：`ClientDiscover` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct ClientDiscover;

    pub trait Requirement {
        // 语义说明：`Store` 负责写入或累积桩中的状态，重点保持调用顺序和副作用外观。
        fn Store(&self) -> super::kv::Storage;
        // 语义说明：`AutoIDClient` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        fn AutoIDClient(&self) -> &ClientDiscover;
    }
}

// 语义说明：`caller` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod caller {
    // 语义说明：`Component` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Component(_name: &str) -> &'static str {
        "lightning-importer"
    }
}

// 语义说明：`failpoint` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod failpoint {
    // 语义说明：`Inject` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn Inject(_name: &str, _f: impl FnOnce()) {}
}

// 语义说明：`etcd` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod etcd {
    use super::context::Context;
    use super::{Error, Result};

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Client` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Client;

    // 语义说明：这个 impl 块补齐 `Client` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Client {
        // 语义说明：`Get` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
        pub fn Get(&self, _ctx: Context, _key: &str) -> Result<Vec<u8>> {
            Ok(vec![])
        }
        // 语义说明：`Close` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Close(&self) {}
    }

    // 语义说明：`NewClient` 按 Go 构造入口返回最小可用对象，减少调用侧对桩实现细节的感知。
    pub fn NewClient(_addrs: &[String]) -> Result<Client> {
        Ok(Client)
    }
}

// 语义说明：`streamhelper` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod streamhelper {
    // 语义说明：`GetCDCPiTRStatus` 提供读取型辅助逻辑，让 importer 上层仍能按 Go 习惯取得所需信息。
    pub fn GetCDCPiTRStatus(_cli: &super::etcd::Client) -> super::Result<bool> {
        Ok(false)
    }
}

// Enhance common with TableHasAutoRowID and important variable maps / errors

// 语义说明：`metric` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod metric {
    use super::Error;
    use super::context::Context;
    use std::sync::{Arc, Mutex};

    // 语义说明：`TableStatePending` 常量继续承担跨模块共享语义锚点，避免 slim port 在字面值上发生漂移。
    pub const TableStatePending: &str = "pending";

    #[derive(Clone, Debug, Default)]
    // 语义说明：`Metrics` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct Metrics {
        pub checksum_seconds: Arc<Mutex<Vec<f64>>>,
    }

    // 语义说明：这个 impl 块补齐 `Metrics` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl Metrics {
        // 语义说明：`RecordTableCount` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn RecordTableCount(&self, _state: &str, _err: Option<&Error>) {}
        // 语义说明：`checksum_hist` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn checksum_hist(&self) -> ChecksumSecondsHistogram {
            ChecksumSecondsHistogram {
                inner: self.checksum_seconds.clone(),
            }
        }
    }

    #[derive(Clone, Debug, Default)]
    // 语义说明：`ChecksumSecondsHistogram` 保留 Go 侧同名数据形状，让上层测试和接线代码继续复用字段语义。
    pub struct ChecksumSecondsHistogram {
        inner: Arc<Mutex<Vec<f64>>>,
    }

    // 语义说明：这个 impl 块补齐 `ChecksumSecondsHistogram` 的 Go 风格方法集合，使调用点能继续按原协议取值和分支。
    impl ChecksumSecondsHistogram {
        // 语义说明：`Observe` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
        pub fn Observe(&self, v: f64) {
            self.inner.lock().unwrap().push(v);
        }
    }

    // 语义说明：`FromContext` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn FromContext(ctx: Context) -> Option<Metrics> {
        ctx.Value("metrics")
            .and_then(|v| v.downcast_ref::<Metrics>().cloned())
    }
}

// 语义说明：`common_ext` 模块提供 importer 迁移路径所需的最小外观，重点保留调用契约而不是完整底层能力。
pub mod common_ext {
    use super::Error;
    use super::model::TableInfo;
    use std::collections::HashMap;
    use std::sync::OnceLock;

    // 语义说明：`TableHasAutoRowID` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn TableHasAutoRowID(tbl: &TableInfo) -> bool {
        !tbl.PKIsHandle && !tbl.IsCommonHandle
    }

    // 语义说明：`DefaultImportantVariables` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn DefaultImportantVariables() -> &'static HashMap<String, String> {
        static M: OnceLock<HashMap<String, String>> = OnceLock::new();
        M.get_or_init(|| {
            [
                ("tidb_row_format_version", "2"),
                ("max_allowed_packet", "67108864"),
                ("div_precision_increment", "4"),
                ("time_zone", "SYSTEM"),
                ("sql_mode", ""),
            ]
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect()
        })
    }

    // 语义说明：`DefaultImportVariablesTiDB` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn DefaultImportVariablesTiDB() -> &'static HashMap<String, String> {
        static M: OnceLock<HashMap<String, String>> = OnceLock::new();
        M.get_or_init(|| {
            [("tidb_placement_mode", "ignore")]
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect()
        })
    }

    // 语义说明：`ErrSchemaNotExists` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn ErrSchemaNotExists(db: &str, table: &str) -> Error {
        Error {
            msg: format!("table `{db}`.`{table}` schema not exists"),
            not_found: false,
            cause: None,
            class: Some("ErrSchemaNotExists"),
        }
    }

    // 语义说明：`ErrUnknownColumns` 保留 Go 对应入口的最小返回约定，让 importer 迁移代码继续走同一路径。
    pub fn ErrUnknownColumns(cols: &str, table: &str) -> Error {
        Error {
            msg: format!("unknown columns {cols} in table {table}"),
            not_found: false,
            cause: None,
            class: Some("ErrUnknownColumns"),
        }
    }
}

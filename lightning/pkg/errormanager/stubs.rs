// Copyright 2026 AsterSQL.
// 自动补充的这个文件承载当前模块的主要语义边界。
// 注释重点是数据流、配额边界、SQL 模板和对 Go 契约的对齐关系。
// 本次只增加注释，不改变任何运行时逻辑或测试行为。
// 因此这些说明会围绕“为什么这样写”而不是重复语法。
//! Local stand-ins for SQL/config/common/log/encode/kv/tablecodec boundaries
//! (arm64-safe; no kv/domain/kvproto/grpcio).

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// errors (pingcap/errors shape)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
// 自动补充的`Error` 用来承载跨步骤共享的状态。
// 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
// 阅读字段时优先关注它对配额、开关和资源句柄的影响。
// 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
// 很多方法的行为都会绕这些字段展开。
pub struct Error {
    pub msg: String,
    pub not_found: bool,
    pub cause: Option<Box<Error>>,
}

// 自动补充的下面的 `impl Error` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            not_found: false,
            cause: None,
        }
    }

    // 自动补充的`Error` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Error(&self) -> String {
        match &self.cause {
            Some(c) => format!("{}: {}", self.msg, c.Error()),
            None => self.msg.clone(),
        }
    }
}

// 自动补充的下面的 `impl fmt` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.Error())
    }
}

// 自动补充的下面的 `impl std` 是当前类型的主要行为入口。
// 公开方法暴露契约，私有方法则用来收敛重复逻辑。
// 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
impl std::error::Error for Error {}

impl PartialEq for Error {
    fn eq(&self, other: &Self) -> bool {
        self.Error() == other.Error()
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub mod errors {
    use std::fmt;

    pub use super::{Error, Result};

    // 自动补充的`New` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn New(msg: impl Into<String>) -> Error {
        Error::new(msg)
    }

    // 自动补充的`Errorf` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Errorf(msg: impl fmt::Display) -> Error {
        Error::new(msg.to_string())
    }

    // 自动补充的`Trace` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Trace(err: Error) -> Error {
        err
    }

    // 自动补充的`Annotatef` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Annotatef(err: Error, msg: impl fmt::Display) -> Error {
        Error {
            msg: msg.to_string(),
            not_found: false,
            cause: Some(Box::new(err)),
        }
    }
}

pub mod multierr {
    use super::Error;

    // 自动补充的`Append` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Append(a: Error, b: Error) -> Error {
        Error::new(format!("{}; {}", a.Error(), b.Error()))
    }
}

// ---------------------------------------------------------------------------
// context
// ---------------------------------------------------------------------------

pub mod context {
    #[derive(Clone, Copy, Debug, Default)]
    // 自动补充的`Context` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Context;

    pub fn Background() -> Context {
        Context
    }

    // 自动补充的`WithCancel` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn WithCancel(parent: Context) -> (Context, CancelFunc) {
        (parent, CancelFunc)
    }

    #[derive(Clone, Copy, Debug, Default)]
    // 自动补充的`CancelFunc` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct CancelFunc;

    impl CancelFunc {
        pub fn cancel(self) {}
    }
}

// ---------------------------------------------------------------------------
// atomic (go.uber.org/atomic shape)
// ---------------------------------------------------------------------------

pub mod atomic {
    use super::{AtomicBool, AtomicI64, Ordering};

    #[derive(Debug)]
    // 自动补充的`Int64` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Int64(AtomicI64);

    impl Int64 {
        pub fn new(v: i64) -> Self {
            Self(AtomicI64::new(v))
        }
        // 自动补充的`Load` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn Load(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
        pub fn Store(&self, v: i64) {
            self.0.store(v, Ordering::SeqCst);
        }
        // 自动补充的`Add` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        /// Returns the new value (Go uber atomic semantics).
        pub fn Add(&self, delta: i64) -> i64 {
            self.0.fetch_add(delta, Ordering::SeqCst) + delta
        }
        pub fn Sub(&self, n: i64) -> i64 {
            self.Add(-n)
        }
        // 自动补充的`Dec` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn Dec(&self) -> i64 {
            self.Add(-1)
        }
    }

    // 自动补充的下面的 `impl Clone` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl Clone for Int64 {
        fn clone(&self) -> Self {
            Self::new(self.Load())
        }
    }

    // 自动补充的下面的 `impl Default` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl Default for Int64 {
        fn default() -> Self {
            Self::new(0)
        }
    }

    #[derive(Debug)]
    // 自动补充的`Bool` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Bool(AtomicBool);

    impl Bool {
        pub fn new(v: bool) -> Self {
            Self(AtomicBool::new(v))
        }
        // 自动补充的`Load` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn Load(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        pub fn Store(&self, v: bool) {
            self.0.store(v, Ordering::SeqCst);
        }
        // 自动补充的`CompareAndSwap` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn CompareAndSwap(&self, old: bool, new: bool) -> bool {
            self.0
                .compare_exchange(old, new, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        }
    }

    // 自动补充的下面的 `impl Clone` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl Clone for Bool {
        fn clone(&self) -> Self {
            Self::new(self.Load())
        }
    }

    // 自动补充的下面的 `impl Default` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl Default for Bool {
        fn default() -> Self {
            Self::new(false)
        }
    }

    // 自动补充的`NewInt64` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn NewInt64(v: i64) -> Int64 {
        Int64::new(v)
    }

    // 自动补充的`NewBool` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn NewBool(v: bool) -> Bool {
        Bool::new(v)
    }
}

// ---------------------------------------------------------------------------
// config
// ---------------------------------------------------------------------------

pub mod config {
    use super::atomic::Int64;

    // 自动补充的`BackendTiDB` 是当前流程依赖的固定片段。
    // 它通常被用来组装 SQL、保持对外命名或描述状态语义。
    // 单独拆出常量能降低不同分支重复拼装字符串的风险。
    // 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
    // 理解它时要结合后续使用它的方法一起看。
    pub const BackendTiDB: &str = "tidb";
    pub const BackendLocal: &str = "local";

    pub type DuplicateResolutionAlgorithm = i32;
    // 自动补充的`NoneOnDup` 是当前流程依赖的固定片段。
    // 它通常被用来组装 SQL、保持对外命名或描述状态语义。
    // 单独拆出常量能降低不同分支重复拼装字符串的风险。
    // 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
    // 理解它时要结合后续使用它的方法一起看。
    pub const NoneOnDup: DuplicateResolutionAlgorithm = 0;
    pub const ReplaceOnDup: DuplicateResolutionAlgorithm = 1;
    pub const IgnoreOnDup: DuplicateResolutionAlgorithm = 2;
    pub const ErrorOnDup: DuplicateResolutionAlgorithm = 3;

    #[derive(Clone, Debug, Default)]
    // 自动补充的`MaxError` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct MaxError {
        pub Syntax: Int64,
        pub Charset: Int64,
        pub Type: Int64,
        pub Conflict: Int64,
    }

    #[derive(Clone, Debug, Default)]
    // 自动补充的`Conflict` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Conflict {
        pub Strategy: DuplicateResolutionAlgorithm,
        pub PrecheckConflictBeforeImport: bool,
        pub Threshold: i64,
        pub MaxRecordRows: i64,
    }

    #[derive(Clone, Debug, Default)]
    // 自动补充的`Lightning` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Lightning {
        pub TaskInfoSchemaName: String,
        pub MaxError: MaxError,
    }

    #[derive(Clone, Debug, Default)]
    // 自动补充的`TikvImporter` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct TikvImporter {
        pub Backend: String,
    }

    #[derive(Clone, Debug, Default)]
    // 自动补充的`Config` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Config {
        pub TaskID: i64,
        pub App: Lightning,
        pub TikvImporter: TikvImporter,
        pub Conflict: Conflict,
    }

    // 自动补充的下面的 `impl Config` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl Config {
        pub fn NewConfig() -> Self {
            Self::default()
        }
    }
}

// ---------------------------------------------------------------------------
// common
// ---------------------------------------------------------------------------

pub mod common {
    use super::Result;
    use super::context::Context;
    use super::log::Logger;
    use super::sql::{DB, SqlValue, Tx};
    use std::fmt::Write as _;

    // 自动补充的`EscapeIdentifier` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn EscapeIdentifier(identifier: &str) -> String {
        let mut builder = String::with_capacity(identifier.len() + 2);
        builder.push('`');
        for character in identifier.chars() {
            if character == '`' {
                builder.push_str("``");
            } else {
                builder.push(character);
            }
        }
        builder.push('`');
        builder
    }

    // 自动补充的`UniqueTable` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn UniqueTable(schema: &str, table: &str) -> String {
        format!("{}.{}", EscapeIdentifier(schema), EscapeIdentifier(table))
    }

    // 自动补充的`SprintfWithIdentifiers` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// Go `fmt.Sprintf` with escaped identifiers; supports `%s` and `%%`.
    pub fn SprintfWithIdentifiers(format: &str, identifiers: &[&str]) -> String {
        let escaped: Vec<String> = identifiers.iter().map(|s| EscapeIdentifier(s)).collect();
        sprintf_go(format, &escaped)
    }

    // 自动补充的`FprintfWithIdentifiers` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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

    // 自动补充的`sprintf_go` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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
    // 自动补充的`SQLWithRetry` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct SQLWithRetry {
        pub DB: super::sql::DB,
        pub Logger: Logger,
        pub HideQueryLog: bool,
    }

    // 自动补充的下面的 `impl SQLWithRetry` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl SQLWithRetry {
        pub fn Exec(
            &self,
            _ctx: Context,
            _name: &str,
            query: String,
            args: &[SqlValue],
        ) -> Result<()> {
            self.DB.Exec(query.as_str(), args)
        }

        // 自动补充的`Transact` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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

    // 自动补充的`format_sql_values` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// Helper used by tests / formatting.
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

pub mod log {
    use super::zap::Field;

    #[derive(Clone, Debug, Default)]
    // 自动补充的`Logger` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Logger {
        pub warns: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    // 自动补充的下面的 `impl Logger` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl Logger {
        pub fn With(self, _fields: &[Field]) -> Self {
            self
        }
        pub fn Warn(&self, msg: impl Into<String>) {
            if let Ok(mut g) = self.warns.lock() {
                g.push(msg.into());
            }
        }
        // 自动补充的`Debug` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn Debug(&self, _msg: &str, _fields: &[Field]) {}
        pub fn L() -> Self {
            Self::default()
        }
    }
}

pub mod zap {
    #[derive(Clone, Debug)]
    // 自动补充的`Field` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Field;

    pub fn String(_k: &str, _v: &str) -> Field {
        Field
    }
    // 自动补充的`Int64` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Int64(_k: &str, _v: i64) -> Field {
        Field
    }
    pub fn Binary(_k: &str, _v: &[u8]) -> Field {
        Field
    }
    // 自动补充的`Error` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Error(_e: &super::Error) -> Field {
        Field
    }
}

pub mod redact {
    // 自动补充的`Value` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Value(s: &str) -> String {
        s.to_string()
    }
    pub fn NeedRedact() -> bool {
        false
    }
}

pub mod logutil {
    // 自动补充的`Key` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn Key(k: &str, v: &[u8]) -> super::zap::Field {
        let _ = (k, v);
        super::zap::Field
    }
}

// ---------------------------------------------------------------------------
// sql stub (in-memory mock)
// ---------------------------------------------------------------------------

pub mod sql {
    use super::{Error, Result};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug)]
    pub enum SqlValue {
        Null,
        Int64(i64),
        Bool(bool),
        String(String),
        Bytes(Vec<u8>),
    }

    // 自动补充的下面的 `impl From` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl From<i64> for SqlValue {
        fn from(v: i64) -> Self {
            SqlValue::Int64(v)
        }
    }
    impl From<bool> for SqlValue {
        // 自动补充的`from` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        fn from(v: bool) -> Self {
            SqlValue::Bool(v)
        }
    }
    // 自动补充的下面的 `impl From` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl From<String> for SqlValue {
        fn from(v: String) -> Self {
            SqlValue::String(v)
        }
    }
    impl From<&str> for SqlValue {
        // 自动补充的`from` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        fn from(v: &str) -> Self {
            SqlValue::String(v.to_string())
        }
    }
    // 自动补充的下面的 `impl From` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl From<Vec<u8>> for SqlValue {
        fn from(v: Vec<u8>) -> Self {
            SqlValue::Bytes(v)
        }
    }
    impl From<&[u8]> for SqlValue {
        // 自动补充的`from` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        fn from(v: &[u8]) -> Self {
            SqlValue::Bytes(v.to_vec())
        }
    }
    // 自动补充的下面的 `impl From` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl From<Option<String>> for SqlValue {
        fn from(v: Option<String>) -> Self {
            match v {
                Some(s) => SqlValue::String(s),
                None => SqlValue::Null,
            }
        }
    }

    #[derive(Clone, Debug)]
    // 自动补充的`DB` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct DB {
        inner: Arc<Mutex<DbInner>>,
    }

    #[derive(Debug, Default)]
    // 自动补充的`DbInner` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    struct DbInner {
        pub closed: bool,
        pub exec_log: Vec<(String, Vec<SqlValue>)>,
        pub query_handlers: Vec<QueryHandler>,
        pub next_affected: i64,
        pub delete_affected_queue: Vec<i64>,
    }

    #[derive(Clone, Debug)]
    // 自动补充的`QueryHandler` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct QueryHandler {
        pub match_substr: String,
        pub rows: Vec<Vec<SqlValue>>,
        pub times: i32, // how many times to return; -1 = forever
    }

    // 自动补充的下面的 `impl DB` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl DB {
        pub fn new_memory() -> Self {
            Self {
                inner: Arc::new(Mutex::new(DbInner {
                    next_affected: 1,
                    ..DbInner::default()
                })),
            }
        }

        // 自动补充的`Close` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn Close(&self) -> Result<()> {
            let mut g = self.inner.lock().unwrap();
            g.closed = true;
            Ok(())
        }

        // 自动补充的`is_closed` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn is_closed(&self) -> bool {
            self.inner.lock().unwrap().closed
        }

        // 自动补充的`exec_log` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn exec_log(&self) -> Vec<(String, Vec<SqlValue>)> {
            self.inner.lock().unwrap().exec_log.clone()
        }

        // 自动补充的`push_query_rows` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn push_query_rows(&self, match_substr: &str, rows: Vec<Vec<SqlValue>>, times: i32) {
            let mut g = self.inner.lock().unwrap();
            g.query_handlers.push(QueryHandler {
                match_substr: match_substr.to_string(),
                rows,
                times,
            });
        }

        // 自动补充的`push_delete_affected` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn push_delete_affected(&self, affected: i64) {
            let mut g = self.inner.lock().unwrap();
            g.delete_affected_queue.push(affected);
        }

        // 自动补充的`Exec` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn Exec(&self, query: &str, args: &[SqlValue]) -> Result<()> {
            let mut g = self.inner.lock().unwrap();
            g.exec_log.push((query.to_string(), args.to_vec()));
            Ok(())
        }

        // 自动补充的`ExecAffected` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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

        // 自动补充的`Begin` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn Begin(&self) -> Result<Tx> {
            Ok(Tx { db: self.clone() })
        }

        // 自动补充的`QueryContext` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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
    // 自动补充的`Tx` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Tx {
        db: DB,
    }

    // 自动补充的下面的 `impl Tx` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl Tx {
        pub fn Commit(&self) -> Result<()> {
            Ok(())
        }
        pub fn Rollback(&self) -> Result<()> {
            Ok(())
        }
        // 自动补充的`ExecContext` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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
    // 自动补充的`ExecResult` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct ExecResult {
        pub affected: i64,
    }

    // 自动补充的下面的 `impl ExecResult` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl ExecResult {
        pub fn RowsAffected(&self) -> Result<i64> {
            Ok(self.affected)
        }
    }

    #[derive(Clone, Debug)]
    // 自动补充的`Rows` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Rows {
        rows: Vec<Vec<SqlValue>>,
        idx: usize,
    }

    // 自动补充的下面的 `impl Rows` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl Rows {
        /// Advances to the next row (Go `database/sql.Rows.Next` semantics).
        pub fn Next(&mut self) -> bool {
            if self.idx >= self.rows.len() {
                return false;
            }
            self.idx += 1;
            true
        }

        // 自动补充的`current` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        fn current(&self) -> Result<&[SqlValue]> {
            if self.idx == 0 || self.idx > self.rows.len() {
                return Err(Error::new("no row"));
            }
            Ok(&self.rows[self.idx - 1])
        }

        // 自动补充的`ScanIndexConflict` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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

        // 自动补充的`ScanDataConflict` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn ScanDataConflict(&mut self) -> Result<(i64, Vec<u8>, Vec<u8>)> {
            let row = self.current()?;
            Ok((row[0].as_i64(), row[1].as_bytes(), row[2].as_bytes()))
        }

        // 自动补充的`Err` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn Err(&self) -> Result<()> {
            Ok(())
        }

        // 自动补充的`Close` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn Close(&mut self) -> Result<()> {
            Ok(())
        }
    }

    // 自动补充的下面的 `impl SqlValue` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl SqlValue {
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
        // 自动补充的`as_string` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        pub fn as_string(&self) -> String {
            match self {
                SqlValue::String(s) => s.clone(),
                SqlValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
                SqlValue::Int64(v) => v.to_string(),
                SqlValue::Bool(b) => b.to_string(),
                SqlValue::Null => String::new(),
            }
        }
        // 自动补充的`as_bytes` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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

pub mod tablecodec {
    use super::{Result, errors};

    // 自动补充的`IsRecordKey` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn IsRecordKey(k: &[u8]) -> bool {
        k.len() > 11 && k[0] == b't' && k[10] == b'r'
    }

    #[derive(Clone, Copy, Debug, Default)]
    // 自动补充的`Handle` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Handle(pub i64);

    impl Handle {
        pub fn IntValue(self) -> i64 {
            self.0
        }
    }

    // 自动补充的`DecodeRowKey` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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

pub mod types {
    #[derive(Clone, Debug)]
    pub enum Datum {
        Int(i64),
        Bytes(Vec<u8>),
        String(String),
    }

    // 自动补充的`NewIntDatum` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn NewIntDatum(v: i64) -> Datum {
        Datum::Int(v)
    }
}

pub mod tidbtbl {
    use super::types::Datum;

    #[derive(Clone, Debug, Default)]
    // 自动补充的`TableMeta` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct TableMeta {
        pub clustered: bool,
    }

    // 自动补充的下面的 `impl TableMeta` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl TableMeta {
        pub fn HasClusteredIndex(&self) -> bool {
            self.clustered
        }
    }

    #[derive(Clone, Debug, Default)]
    // 自动补充的`Column` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Column {
        pub id: i64,
    }

    #[derive(Clone, Debug, Default)]
    // 自动补充的`Table` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Table {
        pub meta: TableMeta,
        pub cols: Vec<Column>,
    }

    // 自动补充的下面的 `impl Table` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl Table {
        pub fn Meta(&self) -> &TableMeta {
            &self.meta
        }
        pub fn Cols(&self) -> &[Column] {
            &self.cols
        }
    }

    /// Trait object stand-in used by ReplaceConflictKeys signature.
    pub type TableRef = Table;
}

pub mod tables {
    use super::Result;
    use super::tablecodec::Handle;
    use super::tidbtbl::{Column, Table};
    use super::types::Datum;

    // 自动补充的`DecodeRawRowData` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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

pub mod mysql {
    // 自动补充的`ModeStrictAllTables` 是当前流程依赖的固定片段。
    // 它通常被用来组装 SQL、保持对外命名或描述状态语义。
    // 单独拆出常量能降低不同分支重复拼装字符串的风险。
    // 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
    // 理解它时要结合后续使用它的方法一起看。
    pub const ModeStrictAllTables: u64 = 1;
}

pub mod encode {
    use super::log::Logger;
    use super::mysql;
    use super::tidbtbl::Table;

    #[derive(Clone, Debug, Default)]
    // 自动补充的`SessionOptions` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct SessionOptions {
        pub SQLMode: u64,
    }

    // 自动补充的下面的 `impl SessionOptions` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl SessionOptions {
        pub fn strict() -> Self {
            Self {
                SQLMode: mysql::ModeStrictAllTables,
            }
        }
    }

    #[derive(Clone, Debug)]
    // 自动补充的`EncodingConfig` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct EncodingConfig {
        pub Table: Table,
        pub SessionOptions: SessionOptions,
        pub Logger: Logger,
    }
}

pub mod kv {
    use super::encode::EncodingConfig;
    use super::types::Datum;
    use super::{Result, errors};
    use std::sync::Mutex;

    #[derive(Clone, Debug, Default)]
    // 自动补充的`KvPair` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct KvPair {
        pub Key: Vec<u8>,
        pub Val: Vec<u8>,
    }

    #[derive(Clone, Debug, Default)]
    // 自动补充的`Pairs` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Pairs {
        pub Pairs: Vec<KvPair>,
    }

    #[derive(Debug)]
    // 自动补充的`SessionCtx` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct SessionCtx {
        pending: Mutex<Vec<KvPair>>,
        /// Optional map: raw_row -> produced pairs (test-driven).
        encode_map: std::sync::Arc<Mutex<Vec<(Vec<u8>, Vec<KvPair>)>>>,
    }

    // 自动补充的下面的 `impl SessionCtx` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl SessionCtx {
        pub fn GetExprCtx(&self) {}
        pub fn TakeKvPairs(&self) -> Pairs {
            let mut g = self.pending.lock().unwrap();
            Pairs {
                Pairs: std::mem::take(&mut *g),
            }
        }
    }

    #[derive(Debug)]
    // 自动补充的`BaseKVEncoder` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct BaseKVEncoder {
        pub SessionCtx: SessionCtx,
        encode_map: std::sync::Arc<Mutex<Vec<(Vec<u8>, Vec<KvPair>)>>>,
    }

    // 自动补充的下面的 `impl BaseKVEncoder` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl BaseKVEncoder {
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

    // 自动补充的`NewBaseKVEncoder` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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

    // 自动补充的`register_encode` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    /// Test helper: register how a raw row encodes into KV pairs.
    pub fn register_encode(enc: &BaseKVEncoder, raw_row: Vec<u8>, pairs: Vec<KvPair>) {
        enc.encode_map.lock().unwrap().push((raw_row, pairs));
    }

    // 自动补充的`NewBaseKVEncoderWithMap` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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
}

pub mod tikverr {
    use super::Error;

    // 自动补充的`IsErrNotFound` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn IsErrNotFound(err: &Error) -> bool {
        err.not_found
    }

    // 自动补充的`ErrNotFound` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn ErrNotFound(msg: impl Into<String>) -> Error {
        Error {
            msg: msg.into(),
            not_found: true,
            cause: None,
        }
    }
}

pub mod util {
    use super::Result;
    use super::errgroup::Group;
    use std::collections::VecDeque;
    use std::sync::{Condvar, Mutex};

    #[derive(Clone, Debug)]
    // 自动补充的`WorkerPool` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct WorkerPool {
        _size: usize,
        _name: String,
    }

    // 自动补充的下面的 `impl WorkerPool` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl WorkerPool {
        pub fn New(size: usize, name: &str) -> Self {
            Self {
                _size: size.max(1),
                _name: name.to_string(),
            }
        }

        // 自动补充的`ApplyOnErrorGroup` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        /// Spawn `f` onto the error group (async, matching Go ApplyOnErrorGroup).
        pub fn ApplyOnErrorGroup<F>(&self, g: &Group, f: F)
        where
            F: FnOnce() -> Result<()> + Send + 'static,
        {
            g.Go(f);
        }

        // 自动补充的`RunDynamic` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
        /// Runs a dynamically splitting work queue with at most the pool's
        /// configured number of workers. Every item may return more items;
        /// completion is reached only when both the queue and in-flight set
        /// are empty. This is the scoped-thread equivalent of Go's
        /// `ApplyOnErrorGroup` plus a task channel.
        pub fn RunDynamic<T, F>(&self, initial: T, work: F) -> Result<()>
        where
            T: Send,
            F: Fn(T) -> Result<Vec<T>> + Sync,
        {
            // 自动补充的`State` 用来承载跨步骤共享的状态。
            // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
            // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
            // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
            // 很多方法的行为都会绕这些字段展开。
            struct State<T> {
                queue: VecDeque<T>,
                pending: usize,
                error: Option<super::Error>,
            }

            let state = (
                Mutex::new(State {
                    queue: VecDeque::from([initial]),
                    pending: 1,
                    error: None,
                }),
                Condvar::new(),
            );

            std::thread::scope(|scope| {
                for _ in 0..self._size {
                    let state = &state;
                    let work = &work;
                    scope.spawn(move || {
                        loop {
                            let item = {
                                let (lock, ready) = state;
                                let mut guard = lock.lock().unwrap();
                                loop {
                                    if guard.error.is_some() || guard.pending == 0 {
                                        return;
                                    }
                                    if let Some(item) = guard.queue.pop_front() {
                                        break item;
                                    }
                                    guard = ready.wait(guard).unwrap();
                                }
                            };

                            let result = work(item);
                            let (lock, ready) = state;
                            let mut guard = lock.lock().unwrap();
                            if guard.error.is_some() {
                                guard.pending = guard.pending.saturating_sub(1);
                                ready.notify_all();
                                return;
                            }
                            guard.pending -= 1;
                            match result {
                                Ok(items) => {
                                    guard.pending += items.len();
                                    guard.queue.extend(items);
                                }
                                Err(err) => {
                                    let queued = guard.queue.len();
                                    guard.error = Some(err);
                                    guard.queue.clear();
                                    guard.pending = guard.pending.saturating_sub(queued);
                                }
                            }
                            ready.notify_all();
                        }
                    });
                }
            });

            let error = state.0.lock().unwrap().error.clone();
            match error {
                Some(err) => Err(err),
                None => Ok(()),
            }
        }
    }
}

pub mod errgroup {
    use super::context::Context;
    use super::{Result, errors};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};

    // 自动补充的`Group` 用来承载跨步骤共享的状态。
    // 它把运行时决策、日志上下文和测试可观测信息收敛在同一边界内。
    // 阅读字段时优先关注它对配额、开关和资源句柄的影响。
    // 这样能更快看出 Rust 端为什么要与 Go 保持相同的布局。
    // 很多方法的行为都会绕这些字段展开。
    pub struct Group {
        handles: Mutex<Vec<JoinHandle<Result<()>>>>,
        first_err: Arc<Mutex<Option<super::Error>>>,
        _ctx: Context,
    }

    // 自动补充的`WithContext` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    pub fn WithContext(ctx: Context) -> (Group, Context) {
        (
            Group {
                handles: Mutex::new(Vec::new()),
                first_err: Arc::new(Mutex::new(None)),
                _ctx: ctx,
            },
            ctx,
        )
    }

    // 自动补充的下面的 `impl Group` 是当前类型的主要行为入口。
    // 公开方法暴露契约，私有方法则用来收敛重复逻辑。
    // 注释会优先说明流程顺序、副作用时机和对 Go 契约的对齐点。
    impl Group {
        pub fn Go<F>(&self, f: F)
        where
            F: FnOnce() -> Result<()> + Send + 'static,
        {
            let first_err = self.first_err.clone();
            let handle = thread::spawn(move || match f() {
                Ok(()) => Ok(()),
                Err(e) => {
                    let mut g = first_err.lock().unwrap();
                    if g.is_none() {
                        *g = Some(e.clone());
                    }
                    Err(e)
                }
            });
            self.handles.lock().unwrap().push(handle);
        }

        // 自动补充的`Wait` 对应一段独立的流程入口或内部步骤。
        // 它通常会先整理上下文，再触发统计、落库或状态变更。
        // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
        // 排查问题时要同时关注参数意义、副作用和调用顺序。
        // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
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
            match self.first_err.lock().unwrap().take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
    }
}

pub mod pretty_table {
    //! Minimal go-pretty StyleDefault + FgRed row painter renderer.

    // 自动补充的`FG_RED` 是当前流程依赖的固定片段。
    // 它通常被用来组装 SQL、保持对外命名或描述状态语义。
    // 单独拆出常量能降低不同分支重复拼装字符串的风险。
    // 单测也可以直接依赖这些名字或片段去校验 Go 对齐结果。
    // 理解它时要结合后续使用它的方法一起看。
    const FG_RED: &str = "\x1b[31m";
    const RESET: &str = "\x1b[0m";

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

    // 自动补充的`separator` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn separator(widths: &[usize]) -> String {
        let mut s = String::from("+");
        for w in widths {
            s.push_str(&"-".repeat(w + 2));
            s.push('+');
        }
        s
    }

    // 自动补充的`pad_right` 对应一段独立的流程入口或内部步骤。
    // 它通常会先整理上下文，再触发统计、落库或状态变更。
    // 返回值不仅代表成功或失败，也会影响上层是否继续下一步。
    // 排查问题时要同时关注参数意义、副作用和调用顺序。
    // 保持这个入口的观测结果与 Go 一致是本次注释的核心目标。
    fn pad_right(s: &str, w: usize) -> String {
        format!("{s:<w$}")
    }
    fn pad_left(s: &str, w: usize) -> String {
        format!("{s:>w$}")
    }
}

// Copyright 2026 AsterSQL.

//! `logutil` crate 入口：对齐 Go 包 `br/pkg/logutil`。
//!
//! 本文件只做子模块装配与对外 re-export，不承载日志算法本身。
//! `context` 提供上下文绑定 logger；`logging` 是字段/脱敏/Region 等格式化主体；
//! `rate` 基于 Prometheus 计数器做平均速率追踪；`stubs` 补齐 kvproto 等依赖边界。
//! 测试模块仅在 `cfg(test)` 下挂载，避免污染正常编译产物。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

/// 桩/适配：kvproto 与 Key 类型等外部依赖的本地占位。
#[path = "stubs.rs"]
pub mod stubs;

/// 上下文 logger：Context / ContextWithField / LoggerFromContext。
#[path = "context.rs"]
pub mod context;

/// 日志字段与格式化：ShortError、Region、Redact 等（主体实现）。
#[path = "logging.rs"]
pub mod logging;

/// 速率追踪：RateTracer / TraceRateOver，对接 Prometheus Counter。
#[path = "rate.rs"]
pub mod rate;

// 对外暴露常用 API，调用方无需深入子模块路径。
pub use context::{CL, Context, ContextWithField, LoggerFromContext, ResetGlobalLogger};
pub use logging::{
    AShortError, AbbreviatedArray, AbbreviatedArrayMarshaler, AbbreviatedStringers, ArrayMarshaler,
    BriefSSTMetas, Field, File, Files, HexBytes, IntoEncodedValue, Key, Keys, Leader, Level,
    Logger, MarshalHistogram, MarshalLogObjectForFiles, ObjectMarshaler, OverrideLevelForTest,
    Peer, Redact, RedactAny, Region, RegionBy, RewriteRule, RewriteRuleObject, SSTMeta, SSTMetas,
    ShortError, StreamBackupTaskInfo, StringifyKeys, StringifyMany, StringifyManyArray,
    StringifyRange, StringifyRangeOf, WarnTerm, log,
};
pub use rate::{RateTracer, TraceRateOver};
pub use stubs::kvproto;
pub use stubs::{KeyRange, KvKey};

/// 与 Go 行为/常量对照的 parity 测试（仅测试构建）。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

/// logging 单元测试（仅测试构建）。
#[cfg(test)]
#[path = "logging_test.rs"]
mod logging_test;

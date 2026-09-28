// Copyright 2026 AsterSQL.

//! `br/pkg/rtree` crate 入口：对齐 Go `br/pkg/rtree` 包导出面。
//!
//! 职责分层：`stubs` 提供 backuppb/tablecodec 等本地替身，
//! `logging` 负责 KeyRange 脱敏展示与 ZapRanges 缩写，
//! `rtree` 承载区间树核心算法（Put/Find/NeedsMerge/Progress）。
//! 对外再导出 logging/rtree 全部公开符号，以及 stubs 中备份元数据编码辅助，
//! 使调用方无需直接依赖桩模块路径。
//! 测试模块按 Go 测试文件一一挂载；仅在 `cfg(test)` 下编译，不影响库消费者。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 本地替身：File/MetaWriter/编码前缀等，避免拉真实 kvproto。
#[path = "stubs.rs"]
pub mod stubs;

// 日志格式辅助：Display + ZapRanges，对齐 Go logging.go。
#[path = "logging.rs"]
pub mod logging;

// 区间树实现主体，对应 Go rtree.go。
#[path = "rtree.rs"]
pub mod rtree;

// 扁平再导出：调用方 `use astersql_br_pkg_rtree::*` 即可拿到核心类型。
pub use logging::*;
pub use rtree::*;
// stubs 只选择性导出备份/编码相关符号，避免把内部桩细节全部暴露。
pub use stubs::{
    AppendDataFile, ChecksumStats, DecodeKeyHead, EncodeIndexKeyPrefix, EncodeIndexSeekKey,
    EncodeKeyspaceKey, EncodeRecordKey, EncodeRowKeyPrefix, File, FreeListG, GenTableRecordPrefix,
    MetaWriter, RpcKeyRange, SummaryFiles, redact_key,
};

// 以下测试文件与 Go 同名用例一一对应，仅测试构建可见。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "logging_test.rs"]
mod logging_test;

#[cfg(test)]
#[path = "merge_fuzz_test.rs"]
mod merge_fuzz_test;

#[cfg(test)]
#[path = "rtree_test.rs"]
mod rtree_test;

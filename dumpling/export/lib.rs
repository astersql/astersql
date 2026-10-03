// Copyright 2026 AsterSQL.

//! Crate entry for `dumpling/export` (Go package `github.com/pingcap/tidb/dumpling/export`).
//!
//! 这个 crate 不是传统按 `mod xxx;` 分散编译的结构，而是通过 `include!` 把
//! `export/` 目录下的大量实现文件拼接成一个与 Go `export` 包相近的“单包视图”。
//! 这样做的主要目的有两点：
//! 1. 降低从 Go 机械迁移到 Rust 过程中的命名与调用面改造成本；
//! 2. 让 `crate::*` 风格的跨文件共享在迁移早期更接近 Go 包级符号可见性。

#![allow(
    // 迁移阶段允许保留大量 Go 风格命名与暂未完全接线的占位实现。
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    unused_attributes,
    clippy::all
)]

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use astersql_dumpling_cli as cli;
use astersql_dumpling_context as tcontext;
use astersql_dumpling_log::{self as log, Field, Logger};

// `stubs.rs` 提供大量与 Go 依赖对齐的轻量替身类型，是整个 export crate 的底座。
#[path = "stubs.rs"]
mod stubs;
pub use stubs::*;

// 下面这组 `include!` 基本按“准备 -> 任务/IR -> 配置 -> 连接/SQL -> writer/dump”顺序组织。
// 它们共同组成导出主流程，阅读时可以把这里当成模块导航索引。
include!("prepare.rs");
include!("task.rs");
include!("ir.rs");
include!("metrics.rs");
include!("sql_type.rs");
// writer_util / config / retry / util 提供公共辅助逻辑，位于主流程模块之前方便共享。
include!("writer_util.rs");
include!("config.rs");
include!("column_filter.rs");
include!("block_allow_list.rs");
include!("retry.rs");
include!("util.rs");
// http_handler / conn / sql / consistency / metadata 属于运行期基础设施层。
include!("http_handler.rs");
include!("conn.rs");
include!("sql.rs");
include!("consistency.rs");
include!("metadata.rs");
// ir_impl / status / writer / dump 则更靠近真正的导出执行面。
include!("ir_impl.rs");
include!("status.rs");
include!("writer.rs");
include!("dump.rs");

// 下面的测试模块按源码主题逐个挂载，保持与 Go 同名测试文件的可追溯关系。
// `parity_test` 额外承担 Rust/Go 行为对齐的回归锚点角色。
#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

// util_for_test 提供共享测试夹具，其他 `*_test.rs` 会间接复用它。
#[cfg(test)]
#[path = "util_for_test.rs"]
mod util_for_test;

// block_allow_list / config / consistency / dump / ir_impl / metadata / metrics / prepare / sql
// 这些测试模块基本一一对应实现文件，方便按主题回归。
#[cfg(test)]
#[path = "block_allow_list_test.rs"]
mod block_allow_list_test;

#[cfg(test)]
#[path = "config_test.rs"]
mod config_test;

#[cfg(test)]
#[path = "consistency_test.rs"]
mod consistency_test;

#[cfg(test)]
#[path = "conn_test.rs"]
mod conn_test;

#[cfg(test)]
#[path = "dump_test.rs"]
mod dump_test;

#[cfg(test)]
#[path = "ir_impl_test.rs"]
mod ir_impl_test;

#[cfg(test)]
#[path = "ir_test.rs"]
mod ir_test;

#[cfg(test)]
#[path = "metadata_test.rs"]
mod metadata_test;

#[cfg(test)]
#[path = "metrics_test.rs"]
mod metrics_test;

#[cfg(test)]
#[path = "http_handler_test.rs"]
mod http_handler_test;

#[cfg(test)]
#[path = "prepare_test.rs"]
mod prepare_test;

#[cfg(test)]
#[path = "sql_test.rs"]
mod sql_test;

#[cfg(test)]
#[path = "sql_type_test.rs"]
mod sql_type_test;

#[cfg(test)]
#[path = "status_test.rs"]
mod status_test;

#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;

#[cfg(test)]
#[path = "writer_serial_test.rs"]
mod writer_serial_test;

#[cfg(test)]
#[path = "writer_util_test.rs"]
mod writer_util_test;

// `writer_serial_test` 与 `writer_test` 一起覆盖 writer 的串行路径和普通路径。
// `status_test` / `util_test` 则补齐外围状态与通用辅助函数的回归检查。
#[cfg(test)]
#[path = "writer_test.rs"]
mod writer_test;

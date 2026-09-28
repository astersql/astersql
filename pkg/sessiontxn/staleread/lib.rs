// Copyright 2026 AsterSQL.

// 过期读（stale read）子包入口。
//
// 聚合错误类型、failpoint 断言、语句级 Processor、事务上下文 Provider，
// 以及 TSO/AS OF 时间戳求值工具；对应 Go 的 `sessiontxn/staleread`。

#![allow(dead_code)]

/// 过期读相关错误种类与构造。
pub mod errors;
/// 测试用 failpoint 断言辅助。
pub mod failpoint;
/// 语句级过期读判定与时间戳解析（Processor）。
pub mod processor;
/// 过期读事务上下文 Provider（只读快照事务）。
pub mod provider;
/// 会话后端抽象、Datum/TSO 换算与公共工具函数。
pub mod util;

pub use errors::*;
pub use failpoint::*;
pub use processor::*;
pub use provider::*;
pub use util::*;

#[cfg(test)]
/// 共享 MockBackend / mock_session 测试夹具。
mod main_test;

#[cfg(test)]
/// `tidb_external_ts` / 外部时间戳读相关用例。
mod externalts_test;

#[cfg(test)]
/// `StaleReadProcessor` 与 `parse_and_validate_as_of` 用例。
mod processor_test;

#[cfg(test)]
/// `StalenessTxnContextProvider` 用例。
mod provider_test;

#[cfg(test)]
/// `util.rs` 的 Go/Rust 边界与缓存语义回归用例。
mod util_test;

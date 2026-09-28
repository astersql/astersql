// Copyright 2026 AsterSQL.

// `util/fastrand` crate 入口：无锁快速伪随机数生成。
//
// 对应 Go `util/fastrand`。数据库内核里采样、哈希扰动、测试数据生成等场景
// 需要高频随机值，本包提供比标准库更轻量的 Uint32/Buf 等接口。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// wyrand 算法与 Buf/Uint32N/Uint64N 等高层随机 API。
pub mod random;
/// 底层 Uint32 快速随机源（对应 Go runtime.cheaprand）。
pub mod runtime;
pub use random::*;
pub use runtime::*;

#[cfg(test)]
#[path = "main_test.rs"]
/// 对应 Go TestMain 的公共测试初始化。
mod main_test;

#[cfg(test)]
#[path = "random_test.rs"]
/// 随机 API 正确性与基准对照测试。
mod random_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// 与 Go 向量对照的迁移单元测试。
mod migration_aster_unit_test;

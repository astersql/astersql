// Copyright 2026 AsterSQL.

// 资源管理器（Resource Manager）工具子 crate 入口。
//
// 汇总协程池（GoroutinePool）抽象、分片池映射（ShardPoolMap）以及测试用 Mock 池，
// 供资源调度器注册、调谐（Tune）与遍历各组件持有的执行池。

#![allow(non_snake_case, dead_code)]

/// 测试用 Mock 协程池实现。
pub mod mock_gpool;
/// 按 key 分片的池容器映射，降低锁竞争。
pub mod shard_pool_map;
/// 池抽象、组件枚举与调度相关常量。
pub mod util;
pub use mock_gpool::*;
pub use shard_pool_map::*;
pub use util::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "shard_pool_map_test.rs"]
mod shard_pool_map_test;

#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;

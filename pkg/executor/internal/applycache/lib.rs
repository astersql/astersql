// Copyright 2026 AsterSQL.

// Apply 算子结果缓存子模块入口。
//
// Apply 是相关子查询（correlated subquery）执行时的嵌套循环算子：对每一行外层行
// 执行内层计划。本模块提供按外层行编码键缓存内层行列表（`chunk::List`）的
// LRU 缓存，并转发内存追踪、Chunk、互斥锁与 kvcache 依赖。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as applycache;
extern crate self as astersql_executor_internal_applycache;

/// 内存占用追踪：ApplyCache 使用独立 Label 计入全局 Tracker。
pub mod memory {
    pub use tidb_memory::meminfo::InstanceMemUsed;
    pub use tidb_memory::tracker::{
        LabelForApplyCache, LabelForGlobalSimpleLRUCache, NewTracker, Tracker,
    };
}

/// Chunk 行列表类型，缓存值即内层查询结果的列式批。
pub mod chunk {
    pub use tidb_chunk::List;
    pub use tidb_chunk::list::NewList;

    pub mod types {
        pub use tidb_chunk::types::FieldType;
    }
}

/// 互斥锁封装；SimpleLRUCache 本身非线程安全，需外层串行化。
pub mod syncutil {
    pub use parking_lot::Mutex;
}

/// 通用 KV LRU 缓存实现依赖。
pub mod kvcache {
    pub use kvcache_dependency::*;
}

// 核心 ApplyCache 实现与对外 re-export。
mod apply_cache;
pub use apply_cache::*;

// 单元测试与迁移回归测试仅在 test 配置下编译。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "apply_cache_test.rs"]
mod apply_cache_test;

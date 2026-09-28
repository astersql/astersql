// Copyright 2026 AsterSQL.

// KV 缓存工具包入口：提供简单 LRU（Least Recently Used，最近最少使用）缓存。
//
// 对应 Go `pkg/util/kvcache`。对外再导出 `SimpleLRUCache` 等实现；`memory` 子模块
// 桥接进程内存用量与全局内存追踪器，供配额（quota）与 OOM 保护使用。

#![allow(dead_code, non_snake_case)]

/// 内存信息与 Tracker 再导出，供 LRU 配额守卫查询实例内存用量。
pub mod memory {
    pub use tidb_memory::meminfo::InstanceMemUsed;
    pub use tidb_memory::tracker::{LabelForGlobalSimpleLRUCache, NewTracker, Tracker};
}

#[path = "simple_lru.rs"]
mod simple_lru;

pub use simple_lru::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
#[cfg(test)]
#[path = "simple_lru_test.rs"]
mod simple_lru_test;

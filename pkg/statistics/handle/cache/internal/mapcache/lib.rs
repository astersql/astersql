// Copyright 2026 AsterSQL.

// 基于 HashMap、无淘汰策略的统计缓存 crate。
//
// 导出 `MapCache` / `NewMapCache`，实现 `StatsCacheInner`，适用于测试或无需容量控制的场景。

#![allow(non_snake_case)]

/// MapCache 实现模块。
mod map_cache;
pub use map_cache::*;

#[cfg(test)]
#[path = "map_cache_test.rs"]
mod map_cache_test;

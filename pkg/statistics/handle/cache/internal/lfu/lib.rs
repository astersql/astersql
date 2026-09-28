// Copyright 2026 AsterSQL.

// LFU（Least Frequently Used / TinyLFU）统计缓存 crate 入口。
//
// 导出 `lfu_cache` 中的加权 TinyLFU 实现及 `NewLFU` 等公共 API；
// 测试通过 `#[path]` 引入同目录的 `lfu_cache_test.rs`。

#![allow(
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    static_mut_refs
)]

/// 单分片键集合模块。
mod key_set;
/// 分片键集合模块。
mod key_set_shard;
/// LFU 主缓存实现模块。
mod lfu_cache;

pub use lfu_cache::*;

#[cfg(test)]
#[path = "lfu_cache_test.rs"]
mod lfu_cache_test;

#[cfg(test)]
#[path = "key_set_shard_test.rs"]
mod key_set_shard_test;

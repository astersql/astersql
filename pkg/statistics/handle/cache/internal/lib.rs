// Copyright 2026 AsterSQL.

// 统计缓存内部实现的公共边界 crate。
//
// 导出 `StatsCacheInner` 等内层存储接口，供 LFU、MapCache 等具体实现依赖；
// 测试通过 `#[path]` 引入 `inner_test.rs`。

#![allow(non_snake_case)]

/// 内层 trait 与类型定义模块。
mod inner;
pub use inner::*;

#[cfg(test)]
#[path = "inner_test.rs"]
mod inner_test;

// Copyright 2026 AsterSQL.

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 模块入口与共享导出层。
// 中文总览：重点在于模块职责、导出关系和串行化约束。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。

//! Crate entry for `tests/realtikvtest/addindextest2`
//! (Go package `github.com/pingcap/tidb/tests/realtikvtest/addindextest2`).
//!
//! Platform (darwin arm64): no kv/domain/kvproto/grpcio. TiKV / SQL / DXF /
//! failpoint / fake-GCS boundaries live in [`harness`] and the slim parent
//! crates `astersql-tests-realtikvtest` / `astersql-tests-realtikvtest-testutils`.

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables,
    unused_mut,
    clippy::all
)]

pub mod harness;

#[cfg(test)]
mod harness_test;

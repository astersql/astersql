// Copyright 2026 AsterSQL.

//! BR restore utils 包入口：聚合 common/merge/misc/rewrite_rule 与 stubs。
//! 对齐 Go `br/pkg/restore/utils`——提供恢复侧键重写、文件区间合并、
//! 表/分区/索引 ID 映射等共享工具。模块用 `#[path]` 固定文件名，
//! 便于与 Go 包布局对照；测试模块仅在 `cfg(test)` 下挂载，
//! 避免与实现文件混编。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

// 桩类型/外部依赖门面，隔离 backuppb、tablecodec 等，便于单测替换。
#[path = "stubs.rs"]
pub mod stubs;

// 恢复建表中间结构（CreatedTable）。
#[path = "common.rs"]
pub mod common;

// 按 split 阈值合并并重写备份文件区间。
#[path = "merge.rs"]
pub mod merge;

// 分区/表/索引 ID 映射与键前缀辅助函数。
#[path = "misc.rs"]
pub mod misc;

// 键重写规则生成、校验与 Range 改写。
#[path = "rewrite_rule.rs"]
pub mod rewrite_rule;

// 对外扁平导出，调用方不必写 utils::merge:: 等路径。
pub use common::*;
pub use merge::*;
pub use misc::*;
pub use rewrite_rule::*;
pub use stubs::AppliedFile;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "merge_test.rs"]
mod merge_test;

#[cfg(test)]
#[path = "misc_test.rs"]
mod misc_test;

#[cfg(test)]
#[path = "rewrite_rule_test.rs"]
mod rewrite_rule_test;

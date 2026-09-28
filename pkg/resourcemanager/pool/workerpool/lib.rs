// Copyright 2026 AsterSQL.

// `workerpool` crate 入口：导出通用 Worker 池及测试子模块。
//
// Worker 池（worker pool）按可配置并发度消费任务、产出结果，并支持动态调容（Tune）。

#[path = "workerpool.rs"]
pub mod workerpool;

/// 再导出 workerpool 子模块的全部公开 API。
pub use workerpool::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "workpool_test.rs"]
mod workpool_test;

#[cfg(test)]
#[path = "workerpool_test.rs"]
mod workerpool_test;

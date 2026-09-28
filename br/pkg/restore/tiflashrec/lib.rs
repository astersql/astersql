// Copyright 2026 AsterSQL.

//! TiFlash 恢复录制器包入口：导出 `tiflash_recorder` 实现。
//! 对应 Go `br/pkg/restore/tiflashrec`，在 restore 路径记录/回放 TiFlash
//! 相关元数据变更，供校验与重入使用。测试经 `#[path]` 挂载，不与实现混编。

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

#[path = "tiflash_recorder.rs"]
pub mod tiflash_recorder;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "tiflash_recorder_test.rs"]
mod tiflash_recorder_test;

// 扁平再导出，调用方直接使用录制器类型与函数。
pub use tiflash_recorder::*;

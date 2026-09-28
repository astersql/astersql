// Copyright 2026 AsterSQL.

#![allow(
    dead_code,
    non_snake_case,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    unused_variables
)]

//! 这个 crate 只负责承接 `file_checkpoints.proto` 生成的 Rust 代码。
//! 入口层保持极薄包装：通过 `#[path]` 绑定生成文件，再统一对外重导出，
//! 让上层检查点逻辑沿用 Go 侧 `checkpointspb` 包的使用方式。

#[path = "file_checkpoints.pb.rs"]
mod file_checkpoints_pb;

// 将生成类型全部重导出，调用方无需直接依赖生成文件名。
pub use file_checkpoints_pb::*;

#[cfg(test)]
// parity 测试放在独立文件，便于和源码入口职责保持分离。
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "file_checkpoints.pb_test.rs"]
mod file_checkpoints_pb_test;

// Copyright 2026 AsterSQL.
// 压缩 I/O 子模块入口：聚合 buffer / 定义 / 读 / 写，并统一再导出。
//
// 备份与对象存储写入大对象时常用 Gzip、Snappy、Zstd 压缩以节省带宽与空间；
// 本 crate 将压缩类型解析、内存缓冲、流式编解码拆到子模块，库根仅做组织与测试挂载。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 可压缩内存缓冲实现。
pub mod buffer;
/// 压缩类型、解压配置等公共定义。
pub mod def;
/// 按压缩类型构造解压 Reader。
pub mod reader;
/// 按压缩类型构造压缩 Writer。
pub mod writer;
pub use buffer::*;
pub use def::*;
pub use reader::*;
pub use writer::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "buffer_test.rs"]
mod buffer_test;

#[cfg(test)]
#[path = "reader_test.rs"]
mod reader_test;

// Copyright 2026 AsterSQL.

// `util/hack` crate 入口：零拷贝视图与 Go Swiss map ABI 镜像。
//
// 对应 Go `pkg/util/hack`。对外再导出 `hack`、`map_abi`（Go 1.25）与
// `map_abi_go126`（Go 1.26）中的类型与函数；测试模块按 cfg(test) 挂载。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code,
    ambiguous_glob_reexports
)]

/// 零拷贝字符串/字节切片互转（对齐 Go unsafe.String/Slice）。
pub mod hack;
/// Go 1.25 Swiss map runtime ABI 镜像与 MemAwareMap。
pub mod map_abi;
/// Go 1.26 Swiss map runtime ABI 镜像与 MemAwareMap。
pub mod map_abi_go126;
/// Safe Go Swiss-table storage shared by the two ABI versions.
pub mod swiss_map;
/// 再导出 hack 模块公开 API。
pub use hack::*;
/// 再导出当前 Go 1.26 map ABI 公开 API；1.25 兼容实现仍可经 `map_abi` 模块访问。
pub use map_abi_go126::*;

#[cfg(test)]
/// hack 零拷贝转换单元测试。
mod hack_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
/// Go 1.26 runtime map 镜像的字段偏移回归测试。
mod map_abi_go126_test;
#[cfg(test)]
/// Swiss map / MemAwareMap 行为与布局公式测试。
mod map_abi_test;
#[cfg(test)]
/// Go 1.25 测试用 table/slots 类型别名。
mod map_abi_test_type_go125_test;
#[cfg(test)]
/// Go 1.26 测试用 table/slots 类型别名。
mod map_abi_test_type_go126_test;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充的 hack / MemAwareMap 单元测试。
mod migration_aster_unit_test;

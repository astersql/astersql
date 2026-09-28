// Copyright 2026 AsterSQL.

// IMPORT INTO / LOAD DATA 导入器 crate 的模块根。
//
// 汇总 import、KV 编码、chunk/engine 处理、job、前置检查、采样与表导入等子模块，
// 并向外 re-export 公共 API；测试模块仅在 `cfg(test)` 下挂载。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 导入计划、控制器与数据源类型等核心定义。
mod import;
pub use import::*;
/// 行数据到 TiKV 键值对的编码逻辑。
mod kv_encode;
pub use kv_encode::*;
/// 按文件 chunk 解析、编码并投递到引擎。
mod chunk_process;
pub use chunk_process::*;
/// Lightning 后端引擎的打开、刷盘与导入流程。
mod engine_process;
pub use engine_process::*;
/// 导入任务（job）的创建、查询与状态管理。
mod job;
pub use job::*;
/// 导入前检查：空表、并发 job、CDC/PiTR、云存储权限等。
mod precheck;
pub use precheck::*;
mod production_storage;
pub use production_storage::*;
mod production_resource;
pub use production_resource::*;
mod production_regions;
pub use production_regions::*;
mod production_size;
pub use production_size::*;
/// 采样估算源文件与编码后 Data/Index KV 体量。
mod sampler;
pub use sampler::*;
/// 单表导入编排：排序目录、引擎、配额与校验和。
mod table_import;
pub use table_import::*;

#[cfg(test)]
#[path = "kv_encode_aster_unit_test.rs"]
mod kv_encode_aster_unit_test;

#[cfg(test)]
mod chunk_process_test;
#[cfg(test)]
mod chunk_process_testkit_test;
#[cfg(test)]
mod engine_process_test;
#[cfg(test)]
mod import_test;
#[cfg(test)]
mod importer_testkit_test;
#[cfg(test)]
mod job_test;
#[cfg(test)]
mod kv_encode_test;
#[cfg(test)]
mod main_test;
#[cfg(test)]
mod precheck_test;
#[cfg(test)]
mod production_regions_test;
#[cfg(test)]
mod production_resource_test;
#[cfg(test)]
mod production_size_test;
#[cfg(test)]
mod production_storage_test;
#[cfg(test)]
mod sampler_test;
#[cfg(test)]
mod table_import_test;
#[cfg(test)]
mod table_import_testkit_test;

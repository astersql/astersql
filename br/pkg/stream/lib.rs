// Copyright 2026 AsterSQL.

//! BR 流备份包入口：装配 decode/search/meta/rewrite/stream 管理等子模块并扁平再导出。
//! 对应 Go `br/pkg/stream`；日志恢复、表映射与元数据扫描的公共 API 由此 crate 对外暴露。
//! 实现文件用显式 `#[path]` 固定，便于与 Go 同名源文件一一对照。
//! 单元测试与 fuzz 仅在 `cfg(test)` 下挂载，避免进生产依赖图。

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

// 桩类型与错误：供尚未完全落地的依赖边编译通过。
#[path = "stubs.rs"]
pub mod stubs;

// KV 事件缓冲编解码与迭代器。
#[path = "decode_kv.rs"]
pub mod decode_kv;

// DB/表替换映射的日志辅助（测试可收集输出）。
#[path = "logging_helper.rs"]
pub mod logging_helper;

// 表历史与 DDL 相关追踪。
#[path = "table_history.rs"]
pub mod table_history;

// 元数据 KV 解析与改写辅助。
#[path = "meta_kv.rs"]
pub mod meta_kv;

// 流备份存储上的 KV 搜索与 CF 合并。
#[path = "search.rs"]
pub mod search;

// 流任务状态枚举与展示。
#[path = "stream_status.rs"]
pub mod stream_status;

// 上下游库表 ID/名称映射。
#[path = "table_mapping.rs"]
pub mod table_mapping;

// RawKV 元数据键改写。
#[path = "rewrite_meta_rawkv.rs"]
pub mod rewrite_meta_rawkv;

// 流元数据集合加载与 shift TS 计算。
#[path = "stream_metas.rs"]
pub mod stream_metas;

// 流任务管理器与生命周期。
#[path = "stream_mgr.rs"]
pub mod stream_mgr;

#[cfg(test)]
#[path = "parity_test.rs"]
mod parity_test;

#[cfg(test)]
#[path = "decode_kv_test.rs"]
mod decode_kv_test;

#[cfg(test)]
#[path = "export_test.rs"]
mod export_test;

#[cfg(test)]
#[path = "meta_kv_test.rs"]
mod meta_kv_test;

#[cfg(test)]
#[path = "rewrite_meta_rawkv_test.rs"]
mod rewrite_meta_rawkv_test;

#[cfg(test)]
#[path = "search_test.rs"]
mod search_test;

#[cfg(test)]
#[path = "stream_metas_test.rs"]
mod stream_metas_test;

#[cfg(test)]
#[path = "stream_mgr_fuzz_test.rs"]
mod stream_mgr_fuzz_test;

#[cfg(test)]
#[path = "stream_mgr_test.rs"]
mod stream_mgr_test;

#[cfg(test)]
#[path = "stream_misc_test.rs"]
mod stream_misc_test;

#[cfg(test)]
#[path = "stream_status_test.rs"]
mod stream_status_test;

#[cfg(test)]
#[path = "table_mapping_test.rs"]
mod table_mapping_test;

// 扁平再导出：调用方写 `stream::X` 即可，不必深入子模块路径。
pub use decode_kv::*;
pub use logging_helper::*;
pub use meta_kv::*;
pub use rewrite_meta_rawkv::*;
pub use search::*;
pub use stream_metas::*;
pub use stream_mgr::*;
pub use stream_status::*;
pub use stubs::*;
pub use table_history::*;
pub use table_mapping::*;

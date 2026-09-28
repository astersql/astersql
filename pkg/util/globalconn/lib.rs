// Copyright 2026 AsterSQL.

// 全局连接 ID（Global Connection ID）crate 入口。
//
// 导出 ID 池（`pool`）与 GCID 编解码/分配器（`globalconn`），支撑集群级唯一连接号
// 与 GlobalKill（跨实例杀掉指定连接）相关能力。测试模块在 `cfg(test)` 下挂载。

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

/// ID 分配池：自增池与无锁环形池。
mod pool;
pub use pool::*;

/// GCID 位布局、解析与连接 ID 分配器。
mod globalconn;
pub use globalconn::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
mod globalconn_test;
#[cfg(test)]
mod pool_test;

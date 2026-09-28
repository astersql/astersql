// Copyright 2026 AsterSQL.

// TTL 缓存 crate 入口：导出基础刷新、InfoSchema 表映射、物理表/任务与状态子模块。
//
// TTL（Time To Live）按列上的过期时间定期删除过期行；本 crate 缓存与扫描任务相关的
// 元数据，避免每次调度都全量遍历 schema。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// 基础缓存刷新间隔控制。
pub mod base;
/// 按物理表 ID 缓存已开启 TTL 的表信息（来自 InfoSchema）。
pub mod infoschema;
/// 物理表、主键句柄与 Region 切分扫描范围。
pub mod table;
/// TTL 扫描/删除任务描述与 Datum。
pub mod task;
/// TTL 作业运行状态。
pub mod ttlstatus;

#[cfg(test)]
mod base_test;
#[cfg(test)]
mod infoschema_test;
#[cfg(test)]
mod split_test;
#[cfg(test)]
mod table_test;
#[cfg(test)]
mod task_test;
#[cfg(test)]
mod ttlstatus_test;

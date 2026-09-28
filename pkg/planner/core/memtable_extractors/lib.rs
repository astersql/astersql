// Copyright 2026 AsterSQL.

// 内存表（memtable）谓词抽取器的 crate 入口。
//
// 通过 `#[path]` 聚合 infoschema 与通用谓词抽取实现，再统一 `pub use`，
// 供查询 INFORMATION_SCHEMA / 集群诊断类伪表时裁剪请求范围。
// memtable 指由 TiDB 进程内存构造、不落盘的系统表视图。

#![allow(non_snake_case)]

/// InfoSchema 系列抽取器（按表名/索引名等列过滤）。
#[path = "../memtable_infoschema_extractor.rs"]
mod memtable_infoschema_extractor;
/// 集群日志、指标、热点 Region 等通用谓词抽取器。
#[path = "../memtable_predicate_extractor.rs"]
mod memtable_predicate_extractor;

pub use memtable_infoschema_extractor::*;
pub use memtable_predicate_extractor::*;

#[cfg(test)]
mod memtable_extractors_test;

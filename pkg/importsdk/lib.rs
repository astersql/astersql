// Copyright 2026 AsterSQL.

// 导入 SDK（importsdk）crate 入口。
//
// 对应 Go `pkg/importsdk`：对外从外部数据源（本地文件、对象存储等）扫描表元数据、
// 生成 `IMPORT INTO` SQL、提交/查询/取消导入作业（Import Job）的对外 API。
// 导入作业是 TiDB 异步数据导入任务，状态可通过 `SHOW IMPORT JOB` 等语句查询。
//
// 子模块按职责划分：配置、错误、文件扫描、作业管理、数据模型、通配路径、
// SDK 门面与 SQL 生成；测试模块仅在 `cfg(test)` 下编译。

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

/// SDK 配置与选项构造。
mod config;
/// 导入相关错误常量。
mod error;
/// 扫描 dump/对象存储中的 schema 与数据文件。
mod file_scanner;
/// 导入作业的提交、查询与取消。
mod job_manager;
/// 表元数据、作业状态等数据结构。
mod model;
/// 为表数据文件生成唯一通配路径模式。
mod pattern;
/// SDK 门面，聚合扫描/作业/SQL 能力。
mod sdk;
/// 根据表元数据生成 IMPORT INTO SQL。
mod sql_generator;

pub use config::*;
pub use error::*;
pub use file_scanner::*;
pub use job_manager::*;
pub use model::*;
pub use sdk::*;
pub use sql_generator::*;

#[cfg(test)]
mod config_test;
#[cfg(test)]
mod file_scanner_test;
#[cfg(test)]
mod job_manager_test;
#[cfg(test)]
mod model_test;
#[cfg(test)]
mod pattern_test;
#[cfg(test)]
mod sdk_test;
#[cfg(test)]
mod sql_generator_test;

// Copyright 2026 AsterSQL.

// `astersql-util-importer` crate 入口：压测/导入工具的模块聚合与重导出。
//
// 子模块覆盖配置、唯一值生成、数据库抽象、主流程、并发调度、DDL 解析与随机数。

#![allow(dead_code)]

pub mod config;
pub mod data;
pub mod db;
pub mod importer;
pub mod job;
pub mod parser;
pub mod rand;

pub use config::*;
pub use data::*;
pub use db::*;
pub use importer::*;
pub use job::*;
pub use parser::*;
pub use rand::*;

#[cfg(test)]
mod config_test;
#[cfg(test)]
mod data_test;
#[cfg(test)]
mod db_test;
#[cfg(test)]
mod job_test;
#[cfg(test)]
mod parser_test;
#[cfg(test)]
mod rand_test;
#[cfg(test)]
mod tests;

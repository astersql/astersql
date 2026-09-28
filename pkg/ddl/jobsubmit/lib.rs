// Copyright 2026 AsterSQL.

// DDL Job Submit（任务提交）crate 入口。
//
// 负责将解析后的 DDL 规范（JobSpec）校验、分配全局 ID、写入系统表，
// 并处理表模式（TableMode）变更等提交侧逻辑。子模块：
// - `submit`：批量提交与全局 ID 分配/插入
// - `table_mode`：表模式（Normal/Import/Restore）切换
// - `types`：Job、JobArgs、Session 等公共类型与 trait

#![allow(dead_code)]

pub mod submit;
pub mod table_mode;
pub mod types;

pub use submit::*;
pub use table_mode::*;
pub use types::*;

#[cfg(test)]
mod submit_test;
#[cfg(test)]
mod table_mode_test;
#[cfg(test)]
mod types_test;

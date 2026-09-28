// Copyright 2026 AsterSQL.

// 静态 RecordSet 子模块入口。
//
// RecordSet 是查询结果集抽象（字段元数据、按 chunk 迭代、关闭）。
// 本 crate 导出普通 recordset 以及带游标（cursor）句柄的包装实现。

#![allow(dead_code, non_snake_case, non_camel_case_types)]

/// 游标包装的 RecordSet 实现。
pub mod cursorrecordset;
/// 基于执行器的静态 RecordSet 实现。
pub mod recordset;

pub use cursorrecordset::*;
pub use recordset::*;
#[cfg(test)]
mod cursorrecordset_test;
/// 集成测试：校验 Next/Close 转发语义。
#[cfg(test)]
mod integration_test;

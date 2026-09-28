// Copyright 2026 AsterSQL.

// testkit external 辅助：InfoSchema / Domain / 表元数据查询入口。
//
// 从 `util` 模块 re-export 列、索引、表、Domain 与错误类型，
// 供外部测试通过名称查找表/列/索引 ID 等。

#![allow(dead_code)]

/// 外部查询与元数据工具实现。
mod util;

pub use util::{
    Column, Domain, DomainOf, ExternalError, ExternalResult, GetIndexID, GetModifyColumn,
    GetTableByName, Index, InfoSchema, InfoSchemaOf, TableMetadata, TableOf, TestKitDomain,
};

#[cfg(test)]
mod util_aster_unit_test;

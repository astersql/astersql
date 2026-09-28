// Copyright 2026 AsterSQL.

// 规划器访问对象（access object）子包入口。
//
// 再导出 `access_obj` 中的表/索引/分区访问描述类型，供 EXPLAIN 展示与 tipb
// 序列化使用。访问对象（Access Object）记录算子实际触及的库表、分区与索引，
// 是执行计划（物理算子树）对外解释数据来源的核心载体。

#![allow(
    non_snake_case,
    non_upper_case_globals,
    non_camel_case_types,
    dead_code
)]

/// 表扫描、索引与动态分区等访问对象实现。
pub mod access_obj;
/// 将访问对象类型提升到 crate 根，便于 `use access::*` 直接引用。
pub use access_obj::*;

/// 迁移期单元测试：校验字符串格式化与 SetIntoPB 行为对齐 Go。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

// Copyright 2026 AsterSQL.

// serialization crate 根：聚合 spill 用的二进制序列化/反序列化工具。
//
// 导出类型码与长度常量（`common_util`）、写入侧（`serialization_util`）
// 与读取侧（`deserialization_util`）；测试下挂载迁移回归用例。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_imports,
    unused_mut,
    unused_variables
)]

/// chunk 依赖再导出：列存储相关类型，供 Reset 等从列取值。
pub mod chunk {
    pub use chunk_dependency::*;
}

/// types 依赖再导出：MyDecimal、Time、BinaryJSON 等结构化类型。
pub mod types {
    pub use types_dependency::*;
    pub use types_json_dependency::{JSONTypeCode, Opaque};
}

/// 类型码与定长宽度常量。
mod common_util;
pub use common_util::*;

/// 反序列化实现。
mod deserialization_util;
pub use deserialization_util::*;

/// 序列化实现。
mod serialization_util;
pub use serialization_util::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
/// AsterSQL 迁移补充回归测试。
mod migration_aster_unit_test;

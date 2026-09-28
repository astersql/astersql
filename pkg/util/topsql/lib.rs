// Copyright 2026 AsterSQL.

// TopSQL 门面 crate：聚合 collector、stmtstats、topsqlstate 与核心 topsql 逻辑。
//
// 对外 re-export 子 crate 与核心 `topsql` 模块；测试配置下挂载迁移基线、
// TestMain 配置与 topsql 单元测试模块。

#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]

pub use collector;
pub use stmtstats;
pub use topsql_state as topsqlstate;

/// 解析器 Digest 类型的薄 re-export，供调用方无需依赖 parser crate。
pub mod parser {
    pub use parser::digester_impl::Digest;
}

mod topsql;
pub use topsql::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;

#[cfg(test)]
#[path = "topsql_test.rs"]
mod topsql_test;

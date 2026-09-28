// Copyright 2026 AsterSQL.

// 会话侧表变更上下文（`tblsession`）crate：把 session 状态接到 `tblctx::MutateContext`。
//
// 对应 Go `pkg/table/tblsession`，实现基于会话变量/事务上下文的表突变支持，
// 供 DML 执行路径在持有 session 时复用表编码缓冲、统计增量与临时表处理。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_table_tblsession;

/// 自增/行 ID 分配器再导出。
pub mod autoid {
    pub use tblctx_dependency::autoid::*;
}
/// 表达式上下文再导出。
pub mod exprctx {
    pub use tblctx_dependency::exprctx::*;
}
/// 信息模式再导出。
pub mod infoschema {
    pub use tblctx_dependency::infoschema::*;
}
/// 内部断言辅助再导出。
pub mod intest {
    pub use tblctx_dependency::intest::*;
}
/// 元数据模型再导出。
pub mod model {
    pub use tblctx_dependency::model::*;
}
/// 行编解码再导出。
pub mod rowcodec {
    pub use tblctx_dependency::rowcodec::*;
}
/// 语句上下文再导出。
pub mod stmtctx {
    pub use tblctx_dependency::stmtctx::*;
}
/// 表变更上下文（tblctx）再导出。
pub mod tblctx {
    pub use tblctx_dependency::*;
}
/// 会话变量再导出。
pub mod variable {
    pub use tblctx_dependency::variable::*;
}

mod table;
pub use table::*;

#[cfg(test)]
mod table_test;

/// 迁移期单元测试：以 include 方式挂接 `migration_aster_unit_test.rs`。
#[cfg(test)]
mod migration_aster_unit_test {
    use super::*;
    use crate::exprctx;
    use contextutil;
    include!("migration_aster_unit_test.rs");
}

// Copyright 2026 AsterSQL.

// metabuild crate 根模块：组装元数据构建（meta build）所需的公共依赖与上下文。
//
// 元数据构建指根据 DDL / 会话配置生成表结构、默认值、字符集等 schema 对象的过程。
// 本文件通过 `pub mod` 再导出表达式上下文、信息模式（infoschema）上下文、
// MySQL 常量与系统变量默认值；真正的构建上下文实现位于 `context` 子模块。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_meta_metabuild;

/// 上下文工具：告警级别、错误包装等与会话侧共享的辅助类型。
pub mod contextutil {
    pub use contextutil_crate::*;
}
/// 表达式求值上下文（ExprContext）接口再导出。
pub mod exprctx {
    pub use exprctx_crate::*;
}
/// 静态表达式上下文工厂：可在无真实会话时构造带默认值的 Eval/Expr 上下文。
pub mod exprstatic {
    pub use exprstatic_crate::*;
}
/// 信息模式（infoschema）只读元数据接口：库表、放置策略、资源组等。
pub mod infoschemactx {
    pub use infoschema_context_crate::*;
}
/// MySQL 协议侧常量与字符集名，供构建上下文读取默认校对规则等。
pub mod mysql {
    pub use mysql_crate::charset::*;
    pub use mysql_crate::r#const::*;
}
/// 系统变量默认值定义（如聚簇索引模式、预拆分 Region 数等）。
pub mod vardef {
    pub use vardef_crate::*;
}

/// 元数据构建上下文实现（`Context`、选项构造器等）。
mod context;
pub use context::*;

#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

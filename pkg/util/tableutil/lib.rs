// Copyright 2026 AsterSQL.

// 临时表会话态工具 crate 入口。
//
// 对应 Go `pkg/util/tableutil`：为全局/局部临时表提供会话级 TempTable
// （独立 autoID、统计与修改标记），并通过可替换工厂对接正式表实现。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 自引用别名，供迁移测试以 crate 名引用本包。
extern crate self as astersql_util_tableutil;

/// 再导出 autoID 分配器相关类型。
pub use autoid;
/// 再导出表元数据模型。
pub use model;

/// TempTable 接口与工厂注册实现。
mod tableutil;
/// 对外导出 TempTable / TempTableFromMeta 等。
pub use tableutil::*;

/// AsterSQL 迁移补充：工厂与会话态行为回归。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

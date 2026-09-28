// Copyright 2026 AsterSQL.

// DDL Schema 变更通知器（notifier）crate 入口。
//
// 在线 DDL 完成后，需要把 schema 变更事件持久化并通知订阅方（如统计信息、
// CDC 相关组件）。本模块聚合事件定义（events）、持久化存储（store）、
// 发布（publish）与订阅（subscribe）子模块，并再导出 meta_model / parser_ast。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

/// 再导出元信息 model（表/列/索引等 DDL 元数据结构）。
pub mod model {
    pub use meta_model::*;
}
/// 再导出 AST 辅助类型（如大小写不敏感标识符 CIStr）。
pub mod ast {
    pub use parser_ast::*;
}

mod events;
pub use events::*;
mod store;
pub use store::*;
mod publish;
pub use publish::*;
mod subscribe;
pub use subscribe::*;

#[cfg(test)]
#[path = "events_test.rs"]
mod events_test;
#[cfg(test)]
#[path = "store_test.rs"]
mod store_test;
#[cfg(test)]
#[path = "testkit_test.rs"]
mod testkit_test;

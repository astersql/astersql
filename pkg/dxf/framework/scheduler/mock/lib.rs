// Copyright 2026 AsterSQL.

// Scheduler Extension 的 mock 包入口。
//
// 为单元测试提供 mockall 生成的 Extension mock，以及对齐 Go storage
// TaskHandle/SessionExecutor 的适配类型。

#![allow(non_snake_case, non_upper_case_globals)]

extern crate self as proto_crate;
extern crate self as storage_crate;

/// 测试用空 Context（对齐 Go context.Context 占位）。
pub type Context = ();

pub use astersql_dxf_framework_proto::{modify, step, task, r#type};

/// 再导出测试常用的 proto 子类型。
pub mod proto {
    pub use astersql_dxf_framework_proto::modify::{Modification, ModifyParam};
    pub use astersql_dxf_framework_proto::step::Step;
    pub use astersql_dxf_framework_proto::task::{Task, TaskBase};
}

#[path = "storage_adapter.rs"]
mod storage_adapter;
pub use storage_adapter::{Error, SessionExecutor, TaskHandle, execute, sessionctx};

/// 再导出 storage 侧 SessionExecutor / TaskHandle。
pub mod storage {
    pub use crate::{SessionExecutor, TaskHandle};
}

/// mockall 生成的 Extension mock。
pub mod scheduler_mock;
pub use scheduler_mock::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "storage_adapter_test.rs"]
mod storage_adapter_test;

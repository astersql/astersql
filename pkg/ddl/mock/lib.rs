// Copyright 2026 AsterSQL.

// DDL Mock 子模块入口。
//
// 聚合 Schema 加载器（`SchemaLoader`）与系统表管理器（systable manager）
// 的 GoMock 风格可控替身，供 DDL 单元/集成测试注入依赖。

#![allow(dead_code)]

pub mod schema_loader_mock;
pub mod systable_manager_mock;

pub use schema_loader_mock::*;
pub use systable_manager_mock::{
    Context as ManagerContext, JobWrapper, Manager, MockManager, MockManagerRecorder,
    Session as ManagerSession, new_mock_manager,
};

#[cfg(test)]
mod mock_aster_unit_test;

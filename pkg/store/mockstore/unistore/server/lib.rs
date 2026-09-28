// Copyright 2026 AsterSQL.

// unistore 嵌入式 Server 子 crate 入口。
//
// 再导出 `server` 模块中的模拟存储引擎启动、Region 管理适配与
// Mock/StandAlone 建服逻辑，供上层 unistore 集成使用。

#![allow(dead_code)]

/// Server 实现：引擎打开、Region 适配与 `new`/`new_mock` 工厂。
pub mod server;

pub use server::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

#[cfg(test)]
#[path = "server_test.rs"]
mod server_test;

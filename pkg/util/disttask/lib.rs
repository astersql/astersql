// Copyright 2026 AsterSQL.

// 分布式任务（disttask）工具包入口。
//
// 对应 Go `pkg/util/disttask`：当前导出执行器 ID 服务（`idservice`），
// 供分布式 DDL/IMPORT 等调度框架标识与定位工作节点。

#![allow(non_snake_case)]

mod idservice;
pub use idservice::*;

#[cfg(test)]
#[path = "idservice_test.rs"]
mod idservice_test;

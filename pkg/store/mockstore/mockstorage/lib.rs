// Copyright 2026 AsterSQL.

// mockstorage：内存版 mock KV Storage 的 crate 入口。
//
// 提供可注入的事务、快照与 PD（Placement Driver，集群元数据服务）客户端替身，
// 供上层在无真实 TiKV 时验证 Storage 接口与事务语义。

#![allow(dead_code, non_camel_case_types, non_snake_case, unused_variables)]

mod canonical_storage;
mod storage;

pub use canonical_storage::*;
pub use storage::*;

#[cfg(test)]
#[path = "canonical_storage_test.rs"]
mod canonical_storage_test;

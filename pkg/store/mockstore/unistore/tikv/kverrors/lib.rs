// Copyright 2026 AsterSQL.

// unistore KV 错误类型 crate 入口。
//
// 再导出 deadlock/kvrpc protobuf 与 MVCC 依赖，并暴露 `errors` 中的
// 各类事务冲突、锁与断言失败错误，供 mock TiKV 读写路径使用。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

/// 死锁相关 protobuf 类型再导出。
pub mod deadlockpb {
    pub use kvproto::deadlock::*;
}

/// KV RPC protobuf 类型再导出。
pub mod kvrpcpb {
    pub use kvproto::kvrpcpb::*;
}

/// MVCC 锁与写入类型再导出。
pub mod mvcc {
    pub use astersql_store_mockstore_unistore_tikv_mvcc::mvcc::*;
}

/// KV 错误定义模块（锁冲突、写冲突、死锁等）。
#[path = "errors.rs"]
pub mod kverrors;
pub use kverrors::*;

/// 迁移对齐单测：校验错误消息与 Go 格式一致。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

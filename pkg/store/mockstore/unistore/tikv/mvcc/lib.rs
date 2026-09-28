// Copyright 2026 AsterSQL.

// unistore MVCC crate 入口：锁/写编码、DB 写接口与快照。
//
// MVCC（多版本并发控制）在 mock TiKV 中管理键的多版本写入与锁。
// 本 crate 聚合 codec、lockstore 适配、db_writer、mvcc 核心结构与 tikv 兼容编码。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

/// 错误类型再导出，并提供与 Go `errors.Errorf` 风格接近的构造器。
pub mod errors {
    pub use astersql_errors::*;

    /// 用消息字符串构造共享错误。
    pub fn Errorf(message: impl Into<String>) -> SharedError {
        New(message.into())
    }
}

// 以 path 属性挂载编解码与锁存储适配模块。
#[path = "codec_adapter.rs"]
pub mod codec;
#[path = "lockstore_adapter.rs"]
pub mod lockstore;

pub use kvproto;

/// DB 写批、快照与 MVCC/TiKV 兼容编码子模块。
pub mod db_writer;
pub mod mvcc;
pub mod tikv;

pub use db_writer::*;
pub use mvcc::*;
pub use tikv::*;

// 迁移对齐单测（仅测试构建）。
#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

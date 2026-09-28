// Copyright 2026 AsterSQL.

// `sessiontxn/internal` 包入口：为内部事务辅助逻辑提供依赖重导出与 KV 桩类型。
//
// 该 crate 在机械迁移中作为独立编译单元，通过 `sessionctx`/`variable` 依赖别名
// 以及精简的 `kv`/`kvrpcpb` 模块，支撑 `txn` 子模块在不拉取完整 TiKV 客户端时编译。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

extern crate self as astersql_sessiontxn_internal;

pub use sessionctx_dependency as sessionctx;
pub use variable_dependency as variable;

/// 精简的 kvrpcpb 桩：仅保留事务断言级别枚举。
///
/// Assertion（断言）用于在提交路径校验写入是否符合预期的键值版本约束。
pub mod kvrpcpb {
    /// 事务断言严格程度：关闭 / 快速 / 严格。
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum AssertionLevel {
        Off,
        Fast,
        Strict,
    }
}

/// 精简的 KV 接口桩：事务选项常量、版本以及 Transaction/Snapshot trait。
pub mod kv {
    use std::any::Any;

    /// Snapshot 拦截器选项键。
    pub const SnapInterceptor: i32 = 27;
    /// 断言级别选项键。
    pub const AssertionLevel: i32 = 31;
    /// 标记请求是否来自内部系统路径。
    pub const RequestSourceInternal: i32 = 32;
    /// 请求来源类型选项键。
    pub const RequestSourceType: i32 = 33;
    /// 显式请求来源类型选项键。
    pub const ExplicitRequestSourceType: i32 = 34;
    /// 基于负载的副本读阈值选项键。
    pub const LoadBasedReplicaReadThreshold: i32 = 39;

    /// MVCC 版本包装：`Ver` 通常对应 StartTS/CommitTS 一类时间戳。
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Version {
        pub Ver: u64,
    }

    /// 事务句柄：可设置选项、查询有效性与起始时间戳（StartTS）。
    pub trait Transaction {
        fn SetOption(&mut self, option: i32, value: Option<Box<dyn Any>>);
        fn Valid(&self) -> bool;
        fn StartTS(&self) -> u64;
    }

    /// 只读快照：可设置与读路径相关的选项。
    pub trait Snapshot {
        fn SetOption(&mut self, option: i32, value: Option<Box<dyn Any>>);
    }

    /// Snapshot 拦截器标记 trait（具体行为由上层注入）。
    pub trait SnapshotInterceptor {}
}

mod txn;
pub use txn::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;

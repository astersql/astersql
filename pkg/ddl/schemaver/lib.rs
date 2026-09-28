// Copyright 2026 AsterSQL.

// Schema Version Syncer（模式版本同步器）crate 入口。
//
// 分布式数据库中，DDL 变更会递增全局 schema 版本号。各 TiDB 实例
// 需要把自己当前已加载的 schema 版本同步到 etcd（分布式协调服务），
// 并由 DDL owner（作业所有者）等待所有实例追上最新版本后，才推进
// 下一步作业。本 crate 提供：
// - [`syncer`]：基于 etcd 的真实同步器实现与相关协议类型；
// - [`mem_syncer`]：纯内存模拟实现，便于单测与本地推演。
//
// 术语：MDL（Metadata Lock，元数据锁）开启时，版本按 DDL job ID
// 分路径上报，避免不同作业之间互相阻塞。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals,
    unused_variables
)]

mod mem_syncer;
mod syncer;

pub use mem_syncer::*;
pub use syncer::*;

#[cfg(test)]
mod mem_syncer_test;
#[cfg(test)]
mod syncer_nokit_test;
#[cfg(test)]
mod syncer_test;

// Copyright 2026 AsterSQL.

// Server State（服务器状态）同步相关 crate 入口。
//
// 本 crate 提供 DDL 子系统感知集群服务器在线状态的能力，
// 主要包括内存实现 [`mem_syncer`] 与基于协调服务的 [`syncer`]。
// 服务器状态用于判断哪些实例需要参与 schema 版本同步等流程。

#![allow(dead_code)]

pub mod mem_syncer;
pub mod syncer;

pub use mem_syncer::*;
pub use syncer::*;

#[cfg(test)]
mod syncer_test;

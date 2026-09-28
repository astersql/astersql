// Copyright 2026 AsterSQL.

// 规划器 Domain 杂项（domainmisc）crate 入口。
//
// 导出与 Domain（TiDB 进程级共享状态容器）相关的辅助能力，
// 当前主要包含 `info` 子模块中的元信息查询等工具。

#![allow(dead_code)]

/// Domain 相关信息与辅助实现。
pub mod info;

#[cfg(test)]
mod info_test;

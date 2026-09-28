// Copyright 2026 AsterSQL.

// TTL 客户端 crate 入口。
//
// 导出基于 etcd 的命令通道（`command`）与通知通道（`notification`），
// 供 TTL worker 在节点间触发任务与广播事件。

#![allow(dead_code)]

pub mod command;
pub mod notification;

pub use command::*;
pub use notification::*;

#[cfg(test)]
mod command_test;

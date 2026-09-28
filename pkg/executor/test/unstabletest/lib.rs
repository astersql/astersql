// Copyright 2026 AsterSQL.

// 不稳定/内存控制相关执行器测试 crate 入口。
//
// 对应 Go `pkg/executor/test/unstabletest` 包。覆盖全局内存控制器
// （ServerMemoryLimit）按 Top1 会话 tracker 发送 kill 信号的链路，以及
// 包级 TestMain 配置与最小 Tracker 冒烟。
// Tracker 用于按会话累计内存字节；超限时 SQLKiller 会中止后续消费。
// 本 crate 仅在 `#[cfg(test)]` 下编译测试源，不导出生产 API。

#![allow(dead_code)]

/// 包级 TestMain 配置与 Tracker 最小冒烟用例。
#[cfg(test)]
mod main_test;
/// 全局内存控制 kill Top1 会话 tracker 的真实链路用例。
#[cfg(test)]
mod memory_test;

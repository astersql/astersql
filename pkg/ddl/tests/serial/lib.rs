// Copyright 2026 AsterSQL.

// DDL 串行（serial）测试包入口。
//
// 聚合需串行执行的 DDL 集成测试（如恢复表、表锁、AUTO_RANDOM、
// flashback 等），对应 Go `pkg/ddl/tests/serial`。串行是为了避免多用例
// 并发改写全局配置、failpoint 或共享 mock store 时相互干扰。

#![allow(dead_code)]

/// TestMain：进程级初始化与 goleak 配置的 Rust 对照测试。
#[cfg(test)]
mod main_test;
/// 串行 DDL 用例主体（步骤记录与部分可执行断言）。
#[cfg(test)]
mod serial_test;

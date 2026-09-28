// Copyright 2026 AsterSQL.
//! `pluginpkg` 的二进制入口。
//!
//! 本层只把进程入口转发给库 crate，确保命令行二进制与测试复用同一套打包流程。

/// 进入库 crate 中共享的命令执行流程。
fn main() {
    astersql_cmd_pluginpkg::main();
}

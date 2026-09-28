// Copyright 2026 AsterSQL.

//! `benchdb` 的二进制入口薄包装。
//!
//! 这里只把进程控制权转交给 `astersql_cmd_benchdb` 的库入口；参数解析、
//! TiKV 会话初始化和基准作业调度均保留在可复用模块中，使二进制与对照测试
//! 共享同一套执行流程，避免入口逻辑出现两份实现。

fn main() {
    astersql_cmd_benchdb::main();
}

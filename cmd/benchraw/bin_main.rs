// Copyright 2026 AsterSQL.

//! `benchraw` 的最小二进制入口。
//!
//! 参数解析、RawKV 并发写入与耗时统计均由共享库实现；这里仅负责转发进程入口，
//! 避免二进制包装层与可复用、可测试的命令逻辑产生两套实现。

fn main() {
    astersql_cmd_benchraw::main();
}

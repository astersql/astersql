// Copyright 2026 AsterSQL.

//! benchkv 命令行可执行文件的薄封装入口。
//!
//! 这里只负责把进程启动委托给库 crate，使二进制运行与测试复用同一套参数解析、压测和指标输出流程。

/// 启动 benchkv 的共享主流程，不在二进制壳层重复实现压测逻辑。
fn main() {
    astersql_cmd_benchkv::main();
}

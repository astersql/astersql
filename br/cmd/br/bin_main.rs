// Copyright 2026 AsterSQL.

//! BR 命令行可执行文件的薄封装入口。
//!
//! 这里只负责把进程启动委托给库 crate，使二进制运行与库模式复用同一套命令树装配和退出处理。

/// 启动 BR 的共享主流程，不在二进制壳层重复实现业务逻辑。
fn main() {
    astersql_br_cmd_br::main();
}

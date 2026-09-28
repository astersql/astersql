// Copyright 2026 AsterSQL.

//! `xprog` 的独立二进制入口，仅负责把进程启动转交给库中的共享主逻辑，
//! 以便命令行可执行文件与测试/复用场景保持同一份 Go 对齐实现。

/// 保持与 Go `main` 相同的启动边界，避免在二进制壳层重复搬运业务逻辑。
fn main() {
    astersql_tools_check_xprog::main();
}

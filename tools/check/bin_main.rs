// Copyright 2026 AsterSQL.

//! `tools/check` 的二进制入口只保留最薄的一层包装，
//! 实际检查流程统一复用库 crate 中的 `main`，便于与可测试逻辑分离。

fn main() {
    // 让可执行文件与库入口共用同一套启动路径，避免命令行壳层复制流程。
    astersql_tools_check::main();
}

// Copyright 2026 AsterSQL.

//! BR 事务 KV 备份集成测试的二进制入口。
//! 参数解析、场景执行与错误处理集中在同名库中，便于测试复用同一套启动流程。

/// 将进程控制权交给可复用的事务备份测试库入口。
fn main() {
    astersql_br_tests_br_txn::main();
}

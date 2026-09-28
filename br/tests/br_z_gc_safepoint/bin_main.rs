// Copyright 2026 AsterSQL.

//! GC safepoint 集成测试的二进制包装层。
//!
//! 实际的参数解析、PD 客户端创建和 safepoint 更新均由同名库 crate 负责，
//! 此处仅提供进程入口，避免二进制路径与可测试的库路径重复实现流程。

/// 将进程控制权交给共享的集成测试入口。
fn main() {
    astersql_br_tests_br_z_gc_safepoint::main();
}

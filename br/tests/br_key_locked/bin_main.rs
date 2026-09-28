// Copyright 2026 AsterSQL.

//! `br_key_locked` 集成测试的二进制薄入口。
//!
//! 本层仅把进程控制权转交给共享库；参数解析、表 ID 查询和制造未提交的
//! Prewrite 锁均由库内的 locker 流程负责。

fn main() {
    // 复用库入口，确保独立二进制与可测试的共享实现走同一条执行路径。
    astersql_br_tests_br_key_locked::main();
}

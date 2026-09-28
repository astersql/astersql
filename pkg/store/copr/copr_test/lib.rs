// Copyright 2026 AsterSQL.

// Coprocessor（协处理器）相关集成/单元测试 crate 入口。
//
// 挂载 `coprocessor_test`（任务切分、小任务并发、速率限制等）与 `main_test`
//（测试运行时 Async Commit 窗口配置）。Coprocessor 指把计算下推到 TiKV/TiFlash 存储节点执行。

#![allow(dead_code)]

#[cfg(test)]
#[path = "coprocessor_test.rs"]
/// Coprocessor 行为与辅助逻辑的测试用例。
mod coprocessor_test;
#[cfg(test)]
#[path = "main_test.rs"]
/// 测试入口与全局配置（对应 Go TestMain）相关断言。
mod main_test;

// Rust 的默认测试 harness 没有 Go `TestMain` 等价入口；使用仓库既有的平台
// 初始化段模式，在 harness 调度任何测试前完成本 crate 的进程级测试配置。
#[cfg(test)]
#[used]
#[cfg_attr(
    any(target_os = "linux", target_os = "android"),
    unsafe(link_section = ".init_array")
)]
#[cfg_attr(
    any(target_os = "macos", target_os = "ios"),
    unsafe(link_section = "__DATA,__mod_init_func")
)]
#[cfg_attr(target_os = "windows", unsafe(link_section = ".CRT$XCU"))]
static INITIALIZE_TEST_RUNTIME: extern "C" fn() = {
    extern "C" fn initialize() {
        main_test::initialize_test_runtime();
    }
    initialize
};

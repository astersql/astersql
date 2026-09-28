// Copyright 2026 AsterSQL.

//! Parity tests for `tools/patch-go` vs Go `check.go`.
//! 这组测试不关心业务输出，而是固定 `patch-go` 探针对外暴露的最小行为契约：
//! 入口会触发一次运行时探测，探测结果保持 `int64` 形状，
//! 缺失补丁时要以失败暴露问题，测试替身状态也必须能被彻底清理。
//! 由于 Go 原文件只有 `main()` 调一次 `grunningnanos()`，
//! Rust 侧把同一契约拆成多个小场景，便于分别覆盖正常路径、边界值、
//! 未打补丁时的失败模式，以及测试桩在多次调用之间的状态隔离。

use crate::check::{grunningnanos, run_main};
use crate::stubs;

#[test]
/// 汇总四类 contract 检查，保持“一个公开测试入口，内部按语义分场景”。
fn go_rust_public_contract_matches() {
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

/// Normal: main calls grunningnanos once; return value is discarded like Go.
/// 这里验证的是 Go `main()` 的最窄语义镜像：只要入口被执行，就应触发一次探测。
/// 之后直接调用底层符号，确认桩里记录的返回值没有在转发层被改写。
fn contract_normal_paths() {
    stubs::reset_for_test();
    stubs::set_nanos(42);
    run_main();
    assert_eq!(stubs::call_count(), 1, "main must call grunningnanos once");
    // Direct call returns the stubbed runtime nanos (Go linkname target).
    let n = unsafe { grunningnanos() };
    assert_eq!(n, 42);
    assert_eq!(stubs::call_count(), 2);
}

/// Boundary: i64 zero / extremes match Go `int64` return shape.
/// 这些断言固定 Rust FFI 边界和 Go `int64` 一致，避免补丁探针在极值上截断或改号。
fn contract_boundary() {
    stubs::reset_for_test();
    for v in [0i64, i64::MIN, i64::MAX, -1] {
        stubs::set_nanos(v);
        let n = unsafe { grunningnanos() };
        assert_eq!(n, v, "grunningnanos must surface runtime nanos {v}");
    }
    assert_eq!(stubs::call_count(), 4);
}

/// Error: unavailable runtime symbol panics (Go fails to link when unpatched).
/// Go 版若未打补丁，`linkname` 目标不可用会在链接或运行期暴露失败；
/// Rust 测试用 panic 表示“不能静默成功”，并顺便确认入口副作用仍被记录。
fn contract_error_paths() {
    stubs::reset_for_test();
    stubs::set_available(false);
    let r = std::panic::catch_unwind(|| unsafe { grunningnanos() });
    assert!(r.is_err(), "unpatched / unlinked symbol must not succeed");
    // Call still recorded before availability check (side-effect of entry).
    assert_eq!(stubs::call_count(), 1);
    stubs::set_available(true);
}

/// Resource cleanup: reset clears call counter and nanos; no leftover state.
/// 清理语义对这类探针测试很关键，否则前一个场景残留的计数和返回值会污染后续断言。
fn contract_resource_cleanup() {
    stubs::reset_for_test();
    stubs::set_nanos(99);
    let _ = unsafe { grunningnanos() };
    assert_eq!(stubs::call_count(), 1);
    stubs::reset_for_test();
    assert_eq!(stubs::call_count(), 0);
    assert_eq!(unsafe { grunningnanos() }, 0);
    assert_eq!(stubs::call_count(), 1);
}

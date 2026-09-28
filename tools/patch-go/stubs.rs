// Copyright 2026 AsterSQL.
//! Local stub for Go `runtime.grunningnanos` (via `//go:linkname`).
//! arm64-safe; no kv/domain/kvproto/grpcio.
//! 该模块不是 Go 运行时的移植实现，而是为 `check.rs` 提供一个可控的本地桩，
//! 用来复现“已打补丁工具链可调用、未打补丁工具链应当失败”的可观察边界。
//! 测试通过原子变量注入返回值、可用性与调用次数，从而验证探针程序只负责触发
//! `runtime.grunningnanos` 的链接/调用语义，而不把缺失符号包装成静默降级。

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

static NANOS: AtomicI64 = AtomicI64::new(0);
static CALLS: AtomicU64 = AtomicU64::new(0);
static AVAILABLE: AtomicBool = AtomicBool::new(true);

/// Reset stub state between tests (call counter, nanos, availability).
/// 这里统一回到“符号存在、返回 0、尚未调用”的初始态，避免前一个用例泄漏
/// 原子状态，确保每次断言都对应一次独立的 Go 运行时探测场景。
pub fn reset_for_test() {
    NANOS.store(0, Ordering::SeqCst);
    CALLS.store(0, Ordering::SeqCst);
    AVAILABLE.store(true, Ordering::SeqCst);
}

/// Configure the nanos value returned by the linked runtime symbol.
/// 该值模拟 Go 补丁里 `gp.runningnanos + nanotime() - gp.lastsched` 的最终观测结果，
/// 桩本身不重建运行时计时公式，只暴露探针真正消费的返回通道。
pub fn set_nanos(v: i64) {
    NANOS.store(v, Ordering::SeqCst);
}

/// Simulate patched (true) vs unpatched/unlinked (false) Go runtime.
/// `false` 表示目标符号没有成功接入；调用方应像未打补丁的 Go 工具链那样立刻失败，
/// 而不是伪造一个默认值继续执行。
pub fn set_available(ok: bool) {
    AVAILABLE.store(ok, Ordering::SeqCst);
}

/// Number of times `runtime_grunningnanos` was entered.
pub fn call_count() -> u64 {
    CALLS.load(Ordering::SeqCst)
}

/// Go `runtime.grunningnanos` after the patch is applied.
///
/// Panics when the symbol is unavailable — mirrors Go failing to link
/// `check.go` against an unpatched toolchain.
/// 先累计进入次数，再检查可用性，保持测试既能验证“确实尝试调用过”，
/// 也能验证失败发生在符号边界而不是调用前短路。
pub fn runtime_grunningnanos() -> i64 {
    CALLS.fetch_add(1, Ordering::SeqCst);
    assert!(
        AVAILABLE.load(Ordering::SeqCst),
        "runtime.grunningnanos: symbol not linked (go unpatched)"
    );
    NANOS.load(Ordering::SeqCst)
}

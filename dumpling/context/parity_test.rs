// Copyright 2026 AsterSQL.

//! Parity tests for `dumpling/context` public contracts vs Go `context.go`.
//!
//! 这些测试锁定 dumpling context 包装层的三个核心承诺：
//! logger 包装不破坏 Go 风格 context 取消语义、
//! `WithContext/WithLogger/WithCancel` 的字段保留规则与 Go 对齐、
//! 以及 `CancelFunc` 的显式调用时机不会被 Rust 生命周期偷偷改变。

use astersql_dumpling_log::{Field, Logger, Zap, ZapLogger};

use crate::{Background, GoBackground, GoWithCancel, NewContext};

#[test]
fn go_rust_public_contract_matches() {
    // 四个子场景分别覆盖常规构造、链式替换、取消错误和资源清理。
    contract_normal_paths();
    contract_boundary_chaining();
    contract_error_cancel();
    contract_resource_cleanup();
}

/// Normal: Background uses Zap nop logger; NewContext stores inputs; L returns logger.
fn contract_normal_paths() {
    // Background 代表最基础的 dumpling 包装上下文，应该既未取消也无错误。
    let bg = Background();
    assert!(!bg.Done());
    assert!(bg.Err().is_none());
    // Go Background uses log.Zap() (package nop).
    // 这里比较 logger 配置而不是具体输出，避免和底层实现细节绑定过紧。
    assert_eq!(bg.L().stacktrace_at(), Zap().stacktrace_at());
    assert!(bg.L().entries().is_empty());

    let capture = ZapLogger::capture(astersql_dumpling_log::Level::Debug);
    let logger = Logger {
        Logger: capture.clone(),
    };
    let go_ctx = GoBackground();
    let ctx = NewContext(go_ctx, logger.clone());
    // NewContext 只做封装，不应立刻改变 cancel 状态。
    assert!(!ctx.Done());
    ctx.L().Info("from-context", [Field::string("k", "v")]);
    let entries = capture.entries();
    assert_eq!(entries.len(), 1);
    assert!(entries[0].contains("[INFO] from-context"));
    assert!(entries[0].contains("k=v"));
}

/// Boundary: WithLogger / WithContext preserve the other field; chaining order.
fn contract_boundary_chaining() {
    let capture_a = ZapLogger::capture(astersql_dumpling_log::Level::Info);
    let logger_a = Logger {
        Logger: capture_a.clone(),
    };
    let capture_b = ZapLogger::capture(astersql_dumpling_log::Level::Info);
    let logger_b = Logger {
        Logger: capture_b.clone(),
    };

    let (parent, parent_cancel) = GoWithCancel(GoBackground());
    let base = NewContext(parent, logger_a);
    // WithLogger keeps go context, replaces logger.
    // 如果 capture_a 收到日志，说明 logger 替换没有彻底生效。
    let with_log = base.WithLogger(logger_b.clone());
    assert!(!with_log.Done());
    with_log.L().Info("b-only", []);
    assert!(capture_a.entries().is_empty());
    assert_eq!(capture_b.entries().len(), 1);

    // WithContext keeps logger, replaces go context.
    // 这里故意换到另一棵 context 树，用来验证取消传播是否随底层 context 走。
    let (child, child_cancel) = GoWithCancel(GoBackground());
    let swapped = with_log.WithContext(child);
    assert!(!swapped.Done());
    child_cancel.call();
    assert!(swapped.Done());
    // Parent cancel must not affect a replaced (unrelated) context tree.
    parent_cancel.call();
    // swapped's tree is independent of parent; already Done via child_cancel.
    assert!(swapped.Done());
    // Original base still follows its original go context (parent).
    // base 没有被 WithContext 原地修改，所以它仍然绑定原 parent。
    assert!(base.Done());

    // Method WithCancel derives from current go context and keeps logger.
    // 新派生出的 cancel 只应影响 derived，不应反向关闭 fresh。
    let fresh = Background().WithLogger(logger_b);
    let (derived, cancel) = fresh.WithCancel();
    assert!(!derived.Done());
    derived.L().Info("derived-logger", []);
    assert!(
        capture_b
            .entries()
            .iter()
            .any(|e| e.contains("derived-logger")),
        "WithCancel must keep the logger"
    );
    cancel.call();
    assert!(derived.Done());
    assert!(
        !fresh.Done(),
        "parent dumpling ctx must stay open until its go ctx cancels"
    );
}

/// Error: cancel sets Done/Err; parent cancel propagates to WithCancel child.
fn contract_error_cancel() {
    let (ctx, cancel) = Background().WithCancel();
    // 调用 cancel 前，Done/Err 都必须保持“未取消”状态。
    assert!(!ctx.Done());
    assert!(ctx.Err().is_none());
    cancel.call();
    assert!(ctx.Done());
    let err = ctx.Err().expect("canceled");
    assert_eq!(err.to_string(), "context canceled");
    // Idempotent cancel.
    // 重复取消不应改变最终状态，也不应 panic。
    cancel.call();
    assert!(ctx.Done());

    // Parent cancel propagates (Go WithCancel links to parent).
    // 这里重点验证父子链路传播，而不是只测本地 cancel 标记。
    let parent = Background();
    let (child, _child_cancel) = parent.WithCancel();
    assert!(!child.Done());
    let (parent2, parent_cancel) = Background().WithCancel();
    let (child2, _) = parent2.WithCancel();
    parent_cancel.call();
    assert!(parent2.Done());
    // child2 没有拿到自己的 cancel handle，也必须跟随父级关闭。
    // 这是 Go `WithCancel(parent)` 最关键的契约之一。
    assert!(child2.Done());
    assert_eq!(child2.Err().expect("err").to_string(), "context canceled");
}

/// Resource: CancelFunc drop without call leaves ctx open; cancel then release is clean.
fn contract_resource_cleanup() {
    let (ctx, cancel) = Background().WithCancel();
    // Rust drop 不应偷偷代替 Go 的显式 cancel 调用。
    drop(cancel);
    assert!(
        !ctx.Done(),
        "dropping CancelFunc without calling must not cancel (Go cancel is explicit)"
    );

    let (ctx2, cancel2) = Background().WithCancel();
    cancel2.call();
    drop(cancel2);
    // 已取消后的 logger 仍应可读，因为取消只影响 context 语义不影响日志对象。
    // 这也说明 dumpling wrapper 没有把 logger 生命周期错误地绑到 cancel handle 上。
    assert!(ctx2.Done());
    // Logger from Background remains usable after cancel.
    ctx2.L().Info("after-cancel", []);
}

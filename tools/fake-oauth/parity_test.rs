// Copyright 2026 AsterSQL.

//! Parity tests for `tools/fake-oauth` vs Go `main.go`.
//! 这些测试不验证业务分支，而是把 Rust 端暴露出来的最小 HTTP 契约
//! 和 Go `main.go` 的字面量行为逐项对齐，防止迁移时把“简单桩服务”
//! 误改成带额外校验、不同默认值或不同资源生命周期的实现。
//! 由于 Go 版本本身只有注册路由、写固定 JSON 和忽略监听错误三件事，
//! 这里按正常路径、边界值、错误路径、资源清理四类场景拆开断言，
//! 让回归时能直接定位是哪一类契约与上游脚本预期发生了偏差。

use crate::main::stubs::{self, Request, ResponseWriter};
use crate::main::{LISTEN_ADDR, TOKEN_JSON, TOKEN_PATH, register_routes, token_response_body};
use std::sync::Mutex;

static TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn go_rust_public_contract_matches() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 总入口只负责串起四组契约检查，保持失败信息按场景分层。
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

#[test]
#[should_panic(expected = "multiple registrations for /oauth/token")]
fn duplicate_route_registration_panics_like_go_serve_mux() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    stubs::reset_for_test();
    register_routes();
    register_routes();
}

/// Normal: `/oauth/token` returns the fixed Go access_token JSON.
/// 验证默认 mux 注册成功后，请求命中处理器就会返回与 Go 完全一致的固定令牌载荷。
fn contract_normal_paths() {
    // 每组测试都先清空 stub 全局状态，避免前一组注册残留污染当前断言。
    stubs::reset_for_test();
    register_routes();

    let mut w = ResponseWriter::new();
    let r = Request {
        method: "POST".into(),
        path: TOKEN_PATH.into(),
    };
    assert!(
        stubs::serve_default(&mut w, &r),
        "registered /oauth/token must be served"
    );
    // 先比较原始字节，再比较 UTF-8 文本，确保内容和编码层面都没有漂移。
    assert_eq!(w.body, TOKEN_JSON);
    assert_eq!(
        std::str::from_utf8(&w.body).unwrap(),
        r#"{"access_token": "ok", "token_type":"service_account", "expires_in":3600}"#
    );
    // Go `http.ResponseWriter` 未显式改状态码时默认就是 200，这里保持同样语义。
    assert_eq!(w.status, 200);
}

/// Boundary: exact path/addr/JSON bytes match Go literals.
/// 边界检查锁定常量字面量，避免“看起来等价”的重构改坏外部脚本依赖的精确协议。
fn contract_boundary() {
    // 路径与监听地址都必须逐字节保持一致，不能改成带主机名或其他别名。
    assert_eq!(TOKEN_PATH, "/oauth/token");
    assert_eq!(LISTEN_ADDR, ":5000");
    assert_eq!(
        TOKEN_JSON,
        br#"{"access_token": "ok", "token_type":"service_account", "expires_in":3600}"#
    );
    // 导出的辅助函数也必须返回同一份固定响应，避免库入口和主逻辑发生分叉。
    assert_eq!(token_response_body(), TOKEN_JSON);

    // Trailing-slash / wrong path must not hit the token handler.
    // 这里刻意使用带尾斜杠路径，证明 stub mux 仍按精确匹配而不是宽松前缀匹配。
    stubs::reset_for_test();
    register_routes();
    let mut w = ResponseWriter::new();
    let r = Request {
        method: "GET".into(),
        path: "/oauth/token/".into(),
    };
    assert!(!stubs::serve_default(&mut w, &r));
    assert!(w.body.is_empty());
}

/// Error: ListenAndServe error is discarded (`_ =` in Go); unknown paths miss.
/// 这组覆盖 Go 原实现最容易被“善意修复”的部分，即监听失败被忽略、未知路径静默 miss。
fn contract_error_paths() {
    stubs::reset_for_test();

    // Force listen failure; Go ignores the error.
    // 先直接观察 stub 返回错误，再确认 `run_main()` 沿用 Go 的忽略错误策略且不会 panic。
    stubs::set_listen_error(true);
    let err = stubs::ListenAndServe(LISTEN_ADDR, None);
    assert!(err.is_err());
    // main discards the error — calling run_main must not panic.
    crate::main::run_main();
    let last = stubs::take_last_listen().expect("listen recorded");
    assert_eq!(last.addr, LISTEN_ADDR);
    assert!(last.used_default_mux);

    // 恢复正常后再验证未知路径，说明错误分支不会把 mux 状态或后续路由分派污染掉。
    stubs::set_listen_error(false);
    stubs::reset_for_test();
    register_routes();
    let mut w = ResponseWriter::new();
    let r = Request {
        method: "GET".into(),
        path: "/nope".into(),
    };
    assert!(!stubs::serve_default(&mut w, &r));
    assert!(w.body.is_empty());
}

/// Resource cleanup: ResponseWriter buffer is owned and cleared after use.
/// 资源清理测试关注 Rust 自有缓冲区和全局路由注册是否能像 Go 进程重置那样被干净回收。
fn contract_resource_cleanup() {
    stubs::reset_for_test();
    register_routes();

    let mut w = ResponseWriter::new();
    let r = Request {
        method: "POST".into(),
        path: TOKEN_PATH.into(),
    };
    assert!(stubs::serve_default(&mut w, &r));
    assert!(!w.body.is_empty());
    let n = w.body.len();
    // 清空缓冲区后仍保留先前写入长度记录，证明响应体所有权完全在测试可控对象上。
    w.body.clear();
    assert!(w.body.is_empty());
    assert_eq!(n, TOKEN_JSON.len());

    // After reset, previous handlers are gone (no leaked registrations).
    // 再次 reset 后旧 handler 不应泄漏到下一轮请求，避免测试之间互相“借用”状态。
    stubs::reset_for_test();
    let mut w2 = ResponseWriter::new();
    assert!(!stubs::serve_default(&mut w2, &r));
}

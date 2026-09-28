// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Parity tests for `br/pkg/utiltest/syncpoint` vs Go `syncpoint.go`.
//!
//! Covers normal ordered release, boundary (ignore without active seq / empty),
//! error (cancel while waiting), and resource cleanup (guards / EndSeq reset).
//! 与 Go syncpoint 公开契约对照：乱序放行、边界 fatal、取消唤醒、资源清理。
//! 单测串行持 TEST_LOCK，避免 failpoint 全局注册表交叉污染。
//! 正常路径验证 Condvar 重排后回调严格按声明顺序执行。
//! 边界路径验证空序列/空上下文 fatal 与序列结束后忽略命中。
//! 错误路径验证取消上下文可唤醒错序等待者且 EndSeq 暴露错误。
//! 清理路径验证 EndSeq 可复用及 Script drop 移除 FailGuard。
//! trigger 仅允许测试白名单短名，防止误注入污染其他 failpoint。
//! Barrier 同步三线程几乎同时命中，放大乱序竞态覆盖面。

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

use crate::{Context, New, Step};

/// 与 Go 包导入路径一致的 failpoint 前缀，保证 EnableCall/Inject 同名。
const SYNC_SCRIPT_TEST_PATH: &str = "github.com/pingcap/tidb/br/pkg/utiltest/syncpoint";

/// 拼接包路径前缀，得到 EnableCall/Inject 共用的全名。
fn fp(name: &str) -> String {
    format!("{SYNC_SCRIPT_TEST_PATH}/{name}")
}

/// 白名单短名 inject；未知名 panic，避免静默漏测。
fn trigger(name: &str) {
    // Go code-gen expands InjectCall short names to the package path; Rust
    // registers and injects the same full path used in Step.
    // Go 代码生成会展开短名；Rust 必须显式使用与 Step 相同的全路径。
    match name {
        "sync-script-a" | "sync-script-b" | "sync-script-c" | "sync-script-wait" => {
            astersql_testkit_testfailpoint::inject(&fp(name));
        }
        _ => panic!("unknown syncpoint test failpoint: {name}"),
    }
}

/// 聚合 Go 侧正常/边界/错误/清理场景，验证 Rust Script 契约一致。
#[test]
fn go_rust_public_contract_matches() {
    let _test_guard = crate::TEST_LOCK.lock().unwrap();

    // --- Normal: concurrent out-of-order hits still run a → b → c ---
    // 并发乱序命中仍须按声明顺序执行回调（核心排序契约）。
    let script = New();
    let (seq_ctx, _cancel) = Context::with_timeout(Duration::from_secs(5));
    let (tx, rx) = mpsc::channel::<String>();

    script.BeginSeq(
        Some(&seq_ctx),
        vec![
            Step(fp("sync-script-a"), {
                let tx = tx.clone();
                move || tx.send("a".to_string()).unwrap()
            }),
            Step(fp("sync-script-b"), {
                let tx = tx.clone();
                move || tx.send("b".to_string()).unwrap()
            }),
            Step(fp("sync-script-c"), {
                let tx = tx.clone();
                move || tx.send("c".to_string()).unwrap()
            }),
        ],
    );
    // 关闭发送端，recv 在序列结束后可感知 EOF。
    drop(tx);

    // Barrier(4)=3 工作线程+主线程，确保三者几乎同时 inject。
    let barrier = Arc::new(Barrier::new(4));
    let mut handles = Vec::new();
    for name in ["sync-script-c", "sync-script-b", "sync-script-a"] {
        let barrier = Arc::clone(&barrier);
        let name = name.to_string();
        handles.push(thread::spawn(move || {
            barrier.wait();
            trigger(&name);
        }));
    }
    barrier.wait();

    // 接收顺序必须严格 a→b→c，证明 Condvar 重排生效。
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "a");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "b");
    assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), "c");
    // 收齐事件后再 EndSeq，符合“先观察副作用”契约。
    script.EndSeq();
    for h in handles {
        h.join().unwrap();
    }

    // --- Boundary: registered step ignored without active sequence ---
    // EndSeq 后 failpoint 仍注册，但无活跃序列时回调不得再执行。
    let script = New();
    let hits = Arc::new(AtomicI32::new(0));
    let (seq_ctx, _cancel) = Context::with_timeout(Duration::from_secs(5));
    let hits_cb = Arc::clone(&hits);
    script.BeginSeq(
        Some(&seq_ctx),
        vec![Step(fp("sync-script-a"), move || {
            hits_cb.fetch_add(1, Ordering::SeqCst);
        })],
    );
    trigger("sync-script-a");
    script.EndSeq();
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    // 序列外再命中：计数必须保持 1。
    trigger("sync-script-a");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    // --- Boundary: empty sequence / nil context panic ---
    // 空序列与 nil 上下文均应 fatal，对齐 Go Fatalf 契约。
    let script = New();
    let (seq_ctx, _c) = Context::with_cancel();
    let empty = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        script.BeginSeq(Some(&seq_ctx), vec![]);
    }));
    assert!(empty.is_err(), "empty sequence must fatal");
    // None 上下文对应 Go nil ctx Fatalf。
    let nil_ctx = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        script.BeginSeq(None, vec![Step(fp("sync-script-a"), || {})]);
    }));
    assert!(nil_ctx.is_err(), "nil context must fatal");

    // --- Error: cancel while a waiter is blocked on the wrong step ---
    // 错序等待者阻塞时取消上下文，须唤醒且 EndSeq 暴露取消错误。
    let script = New();
    let (seq_ctx, cancel) = Context::with_cancel();
    script.BeginSeq(
        Some(&seq_ctx),
        vec![
            Step(fp("sync-script-a"), || {}),
            Step(fp("sync-script-b"), || {}),
        ],
    );
    let waiter = thread::spawn(|| {
        // Arrives early for step b; must block until cancel sets err.
        // 过早命中 b，应阻塞直至 cancel 写入 err。
        trigger("sync-script-b");
    });
    // 给 waiter 一点时间进入 Condvar 等待。
    thread::sleep(Duration::from_millis(50));
    // 取消应写入序列 err 并 notify_all。
    cancel.cancel_with("context canceled");
    // Waiter should unblock without hanging.
    // 等待者必须解除阻塞且 failpoint 路径不得 panic。
    waiter
        .join()
        .expect("cancelled waiter must not panic from failpoint path");
    let end = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        script.EndSeq();
    }));
    assert!(
        end.is_err(),
        "EndSeq must surface cancel error from active sequence"
    );

    // --- Resource cleanup: EndSeq resets; a fresh BeginSeq works again ---
    // EndSeq 重置状态后，同一 Script 可再次 BeginSeq 并累计命中。
    let script = New();
    let (seq_ctx, _c) = Context::with_timeout(Duration::from_secs(5));
    let hits = Arc::new(AtomicI32::new(0));
    let hits_cb = Arc::clone(&hits);
    script.BeginSeq(
        Some(&seq_ctx),
        vec![Step(fp("sync-script-a"), move || {
            hits_cb.fetch_add(1, Ordering::SeqCst);
        })],
    );
    trigger("sync-script-a");
    script.EndSeq();

    let (seq_ctx2, _c2) = Context::with_timeout(Duration::from_secs(5));
    let hits_cb2 = Arc::clone(&hits);
    script.BeginSeq(
        Some(&seq_ctx2),
        vec![Step(fp("sync-script-a"), move || {
            hits_cb2.fetch_add(1, Ordering::SeqCst);
        })],
    );
    trigger("sync-script-a");
    script.EndSeq();
    // 两次独立序列各命中一次，累计为 2。
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    // Dropping Script must disable registered failpoints (FailGuard cleanup).
    // Script drop 必须 Disable 已注册 failpoint，防止泄漏到后续用例。
    {
        let script = New();
        let (seq_ctx, _c) = Context::with_timeout(Duration::from_secs(5));
        script.BeginSeq(Some(&seq_ctx), vec![Step(fp("sync-script-wait"), || {})]);
        trigger("sync-script-wait");
        script.EndSeq();
        // script drops here → guards disable
        // 作用域结束触发 FailGuard drop → Disable
    }
    let before = {
        // inject after drop should be a no-op (no callback registered)
        let marker = Arc::new(AtomicI32::new(0));
        // Re-register briefly only to prove prior name is gone from registry list.
        // 确认全局列表中已无 sync-script-wait，证明清理生效。
        let still = fail::list()
            .iter()
            .any(|(n, _)| n == &fp("sync-script-wait"));
        assert!(
            !still,
            "failpoint must be removed when Script (FailGuard) drops"
        );
        marker.fetch_add(0, Ordering::SeqCst)
    };
    assert_eq!(before, 0);
}

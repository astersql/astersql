// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 对照 Go `testfailpoint` 的迁移单元测试。
//
// 验证 `enable` / `enable_call` / `disable` 的注册、求值与 Guard 清理语义。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use super::{disable, enable, enable_call, enable_pause, enable_value_call, inject_value};

/// 验证表达式型 failpoint：启用后可求值，丢弃 Guard 后自动注销。
#[test]
fn enable_and_guard_cleanup_match_go_test_cleanup() {
    let name = "testfailpoint-enable";
    let guard = enable(name, "return(enabled)");

    // 注册表中应出现对应名称与表达式。
    assert!(
        fail::list().iter().any(|(registered, expression)| {
            registered == name && expression == "return(enabled)"
        })
    );
    assert_eq!(
        fail::eval(name, |argument| argument),
        Some(Some("enabled".to_owned()))
    );

    // Guard 丢弃等价于 Go `t.Cleanup` 关闭 failpoint。
    drop(guard);
    assert!(
        !fail::list()
            .iter()
            .any(|(registered, _)| registered == name)
    );
}

/// 验证回调型 failpoint：求值时触发回调，Guard 丢弃后不再触发。
#[test]
fn enable_call_registers_callback_and_cleans_it_up() {
    static CALLS: AtomicUsize = AtomicUsize::new(0);

    let name = "testfailpoint-enable-call";
    let guard = enable_call(name, || {
        CALLS.fetch_add(1, Ordering::SeqCst);
    });

    assert_eq!(CALLS.load(Ordering::SeqCst), 0);
    assert_eq!(fail::eval(name, |_| ()), None);
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);

    drop(guard);
    assert_eq!(fail::eval(name, |_| ()), None);
    assert_eq!(CALLS.load(Ordering::SeqCst), 1);
}

/// 验证显式 `disable` 后 failpoint 立即移除，且随后丢弃 Guard 仍安全。
#[test]
fn disable_removes_an_enabled_failpoint() {
    let name = "testfailpoint-disable";
    let guard = enable(name, "return(true)");

    disable(name);
    assert!(
        !fail::list()
            .iter()
            .any(|(registered, _)| registered == name)
    );

    // Go permits cleanup after an explicit Disable; dropping the guard must too.
    // Go 允许在显式 Disable 之后再 Cleanup；丢弃 Guard 也必须安全。
    drop(guard);
}

/// Go `InjectCall` forwards production arguments to the test callback.
#[test]
fn value_call_forwards_runtime_value_and_guard_cleans_up() {
    let observed = Arc::new(Mutex::new(Vec::new()));
    let callback_observed = Arc::clone(&observed);
    let guard = enable_value_call("testfailpoint-value-call", move |value| {
        callback_observed
            .lock()
            .expect("observed value lock poisoned")
            .push(value.to_owned());
    });

    inject_value("testfailpoint-value-call", "global");
    assert_eq!(
        *observed.lock().expect("observed value lock poisoned"),
        vec!["global"]
    );

    drop(guard);
    inject_value("testfailpoint-value-call", "table");
    assert_eq!(
        *observed.lock().expect("observed value lock poisoned"),
        vec!["global"]
    );
}

/// 暂停点未命中时按时返回；命中时仍保持“观察后恢复”的同步语义。
#[test]
fn pause_wait_timeout_never_strands_the_test() {
    let timeout_guard = enable_pause("testfailpoint-pause-timeout");
    assert!(!timeout_guard.wait_until_reached_timeout(Duration::from_millis(10)));

    let reached_guard = Arc::new(enable_pause("testfailpoint-pause-reached"));
    let worker_guard = Arc::clone(&reached_guard);
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let _ = fail::eval("testfailpoint-pause-reached", |_| ());
        done_tx.send(()).expect("signal resumed pause");
        drop(worker_guard);
    });

    assert!(reached_guard.wait_until_reached_timeout(Duration::from_secs(1)));
    assert!(
        done_rx.recv_timeout(Duration::from_millis(10)).is_err(),
        "pause callback resumed before the test released it"
    );
    reached_guard.resume();
    done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("pause callback resumes");
    worker.join().expect("pause worker");
}

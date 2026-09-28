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

// 测试用 failpoint（故障注入点）辅助。
//
// 封装 `fail` crate：用 RAII Guard 在测试期间启用/关闭注入点，
// 并提供暂停型 failpoint，便于测试与被测代码在指定位点同步。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

/// 重新导出 `fail::FailGuard`：持有期间 failpoint 生效，丢弃时自动关闭。
pub use fail::FailGuard;

/// Enables a failpoint for the lifetime of the returned guard.
///
/// Keeping the guard until the end of a test is the Rust equivalent of the
/// Go helper's `t.Cleanup` registration. Configuration errors fail the test
/// immediately, matching Go's `require.NoError` behavior.
///
/// 在返回的 Guard 生命周期内启用命名 failpoint；配置失败立即 panic，对齐 Go 的 `require.NoError`。
#[must_use = "dropping the guard disables the failpoint immediately"]
pub fn enable(name: &str, expression: &str) -> FailGuard {
    FailGuard::new(name, expression)
        .unwrap_or_else(|error| panic!("failed to enable failpoint {name:?}: {error}"))
}

/// Enables a callback failpoint for the lifetime of the returned guard.
///
/// 启用回调型 failpoint：命中时执行 `callback`，Guard 丢弃后注销。
#[must_use = "dropping the guard disables the failpoint immediately"]
pub fn enable_call<F>(name: &str, callback: F) -> FailGuard
where
    F: Fn() + Send + Sync + 'static,
{
    FailGuard::with_callback(name, callback)
        .unwrap_or_else(|error| panic!("failed to enable callback failpoint {name:?}: {error}"))
}

type ValueCallback = Arc<dyn Fn(&str) + Send + Sync>;
type ConcurrentCallback = Arc<dyn Fn() + Send + Sync>;

fn concurrent_callbacks() -> &'static Mutex<HashMap<String, (u64, ConcurrentCallback)>> {
    static CALLBACKS: OnceLock<Mutex<HashMap<String, (u64, ConcurrentCallback)>>> = OnceLock::new();
    CALLBACKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Guard for a callback copied out of the registry before invocation, allowing
/// independent threads to enter the same failpoint concurrently.
pub struct ConcurrentCallGuard {
    name: String,
    id: u64,
}

impl Drop for ConcurrentCallGuard {
    fn drop(&mut self) {
        let mut callbacks = concurrent_callbacks()
            .lock()
            .expect("concurrent failpoint callback lock poisoned");
        if callbacks
            .get(&self.name)
            .is_some_and(|(id, _)| *id == self.id)
        {
            callbacks.remove(&self.name);
        }
    }
}

/// Register a callback whose body may overlap across injection threads.
#[must_use = "dropping the guard disables the failpoint immediately"]
pub fn enable_concurrent_call<F>(name: &str, callback: F) -> ConcurrentCallGuard
where
    F: Fn() + Send + Sync + 'static,
{
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    concurrent_callbacks()
        .lock()
        .expect("concurrent failpoint callback lock poisoned")
        .insert(name.to_owned(), (id, Arc::new(callback)));
    ConcurrentCallGuard {
        name: name.to_owned(),
        id,
    }
}

fn value_callbacks() -> &'static Mutex<HashMap<String, (u64, ValueCallback)>> {
    static CALLBACKS: OnceLock<Mutex<HashMap<String, (u64, ValueCallback)>>> = OnceLock::new();
    CALLBACKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Guard for a callback that receives the value supplied by the production
/// injection site. Go's `failpoint.InjectCall` forwards its arguments; the
/// upstream `fail` crate only supports zero-argument callbacks, so this small
/// registry preserves that observable boundary for Rust integration tests.
pub struct ValueCallGuard {
    name: String,
    id: u64,
}

impl Drop for ValueCallGuard {
    fn drop(&mut self) {
        let mut callbacks = value_callbacks()
            .lock()
            .expect("value failpoint callback lock poisoned");
        if callbacks
            .get(&self.name)
            .is_some_and(|(id, _)| *id == self.id)
        {
            callbacks.remove(&self.name);
        }
    }
}

/// Register a callback receiving the runtime value passed to
/// [`inject_value`].
#[must_use = "dropping the guard disables the value callback immediately"]
pub fn enable_value_call<F>(name: &str, callback: F) -> ValueCallGuard
where
    F: Fn(&str) + Send + Sync + 'static,
{
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    value_callbacks()
        .lock()
        .expect("value failpoint callback lock poisoned")
        .insert(name.to_owned(), (id, Arc::new(callback)));
    ValueCallGuard {
        name: name.to_owned(),
        id,
    }
}

/// Invoke the value callback at a production failpoint boundary.
pub fn inject_value(name: &str, value: &str) {
    let callback = value_callbacks()
        .lock()
        .expect("value failpoint callback lock poisoned")
        .get(name)
        .map(|(_, callback)| Arc::clone(callback));
    if let Some(callback) = callback {
        callback(value);
    }
}

/// Disables a failpoint. Disabling an unknown name is a no-op, as in Go.
///
/// 关闭命名 failpoint；名称不存在时为空操作，与 Go 行为一致。
pub fn disable(name: &str) {
    fail::remove(name);
}

/// 触发命名 failpoint 求值（忽略返回值），用于注入副作用。
pub fn inject(name: &str) {
    let concurrent = concurrent_callbacks()
        .lock()
        .expect("concurrent failpoint callback lock poisoned")
        .get(name)
        .map(|(_, callback)| Arc::clone(callback));
    if let Some(callback) = concurrent {
        callback();
    }
    let _ = fail::eval(name, |_| ());
}

/// 将 failpoint 返回值解析为布尔：`"1"` / `"true"` / `"on"` 为真，未启用则为假。
pub fn eval_bool(name: &str) -> bool {
    fail::eval(name, |value| {
        value.is_some_and(|value| matches!(value.as_str(), "1" | "true" | "on"))
    })
    .unwrap_or(false)
}

/// Evaluate a value-bearing failpoint and return its string payload.
///
/// This is used by production retry paths whose behavior depends on an
/// injected timeout/error value rather than merely on failpoint presence.
pub fn eval_string(name: &str) -> Option<String> {
    fail::eval(name, |value| value).flatten()
}

/// Returns whether a failpoint is configured, including value-less
/// `return()` expressions used by TiDB fault-injection tests.
pub fn is_active(name: &str) -> bool {
    // `fail::eval` is not a read-only probe: callback failpoints execute their
    // callback while being evaluated. In particular, probing a pause
    // failpoint with `eval` blocks at the probe and then executes the real
    // injection site a second time, so observers see the state before the
    // intended boundary. `list` inspects the registry without consuming an
    // action or invoking a callback.
    concurrent_callbacks()
        .lock()
        .expect("concurrent failpoint callback lock poisoned")
        .contains_key(name)
        || fail::list()
            .into_iter()
            .any(|(configured_name, _)| configured_name == name)
}

/// 暂停型 failpoint 的共享状态：是否已到达注入点、是否已由测试侧恢复。
#[derive(Default)]
struct PauseState {
    /// 被测代码是否已进入 failpoint 回调。
    reached: bool,
    /// 测试侧是否已调用 [`PauseGuard::resume`]。
    resumed: bool,
}

/// A real callback failpoint whose callback blocks until the test resumes it.
///
/// 暂停型 failpoint 的守卫：回调会阻塞直到测试调用 [`PauseGuard::resume`]。
pub struct PauseGuard {
    /// 与回调共享的到达/恢复状态及条件变量。
    state: Arc<(Mutex<PauseState>, Condvar)>,
    /// 持有底层 FailGuard，确保作用域结束时注销 failpoint。
    _guard: FailGuard,
}

impl PauseGuard {
    /// 阻塞直到被测代码命中该 failpoint（`reached == true`）。
    pub fn wait_until_reached(&self) {
        let (lock, changed) = &*self.state;
        let state = lock.lock().expect("pause failpoint lock poisoned");
        drop(
            changed
                .wait_while(state, |state| !state.reached)
                .expect("pause failpoint lock poisoned"),
        );
    }

    /// 等待被测代码命中 failpoint，超时返回 `false`。
    pub fn wait_until_reached_timeout(&self, timeout: Duration) -> bool {
        let (lock, changed) = &*self.state;
        let state = lock.lock().expect("pause failpoint lock poisoned");
        let (state, _) = changed
            .wait_timeout_while(state, timeout, |state| !state.reached)
            .expect("pause failpoint lock poisoned");
        state.reached
    }

    /// 标记已恢复并唤醒阻塞在 failpoint 内的回调。
    pub fn resume(&self) {
        let (lock, changed) = &*self.state;
        lock.lock().expect("pause failpoint lock poisoned").resumed = true;
        changed.notify_all();
    }
}

/// 启用暂停型 failpoint：命中时先通知测试侧，再等待 `resume`。
pub fn enable_pause(name: &str) -> PauseGuard {
    let state = Arc::new((Mutex::new(PauseState::default()), Condvar::new()));
    let callback_state = Arc::clone(&state);
    // 回调：置 reached → 通知 wait_until_reached → 等待 resumed。
    let guard = enable_call(name, move || {
        let (lock, changed) = &*callback_state;
        let mut state = lock.lock().expect("pause failpoint lock poisoned");
        state.reached = true;
        changed.notify_all();
        drop(
            changed
                .wait_while(state, |state| !state.resumed)
                .expect("pause failpoint lock poisoned"),
        );
    });
    PauseGuard {
        state,
        _guard: guard,
    }
}

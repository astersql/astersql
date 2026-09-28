// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// TTL worker 集成测试的会话池借用计数 helper。
//
// 用内存 `SessionPool` + `PoolTestWrapper` 复现 Go `poolTestWrapper` 的
// “借出/归还成对、结束时计数归零”不变式，供适配器与 timer 同步测试检测 session 泄漏。

// Go 的 `poolTestWrapper` 包裹 `syssession.Pool`，用一个原子计数器统计当前借出的系统 session
// 数量，供集成测试在结束时断言 `TtlJobAdapter`/`TtlTimersSyncer` 没有泄漏 session（每次
// `WithSession` 调用前后都必须成对出现）。
//
// Rust 侧 `astersql-ttl-ttlworker` 的 `JobManager`/`TtlTimersSyncer` 都是纯内存实现，不持有任何
// 系统 session 池，因此没有直接对应的"真实资源"可包裹。这里改为对 `session::WorkerSession` 提供
// 一个真实、自包含的内存池实现（`SessionPool`）外加同样语义的借用计数包装器
// （`PoolTestWrapper`），保留 Go helper 的核心不变式："`with_session` 调用期间计数 > 0，
// 返回后计数归零；未配对的借用会被 `assert_no_session_in_use` 捕获"。其它测试文件如果需要一个
// 会话池占用检测工具，可以直接复用这里的 `PoolTestWrapper`。

use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

use astersql_ttl_ttlworker::session::{SessionError, SessionState, WorkerSession};

/// 最小的内存 `WorkerSession` 实现：只记录状态并回放调用者提供的 SQL 结果，足够用来验证
/// `PoolTestWrapper` 的借用计数行为，不需要真实的存储引擎。
pub struct InMemorySession {
    state: SessionState,
}

impl InMemorySession {
    /// 构造默认状态的空会话。
    pub fn new() -> Self {
        Self {
            state: SessionState::default(),
        }
    }
}

impl WorkerSession for InMemorySession {
    fn state(&self) -> &SessionState {
        &self.state
    }
    fn state_mut(&mut self) -> &mut SessionState {
        &mut self.state
    }
    fn execute(
        &mut self,
        _sql: &str,
        _args: &[astersql_ttl_ttlworker::session::Datum],
    ) -> Result<Vec<astersql_ttl_ttlworker::session::Row>, SessionError> {
        Ok(Vec::new())
    }
}

/// 单会话内存池：对应 Go `syssession.Pool` 的最小子集，只保证 `with_session` 期间独占访问。
pub struct SessionPool {
    session: Mutex<InMemorySession>,
}

impl SessionPool {
    /// 创建持有单个 `InMemorySession` 的池。
    pub fn new() -> Self {
        Self {
            session: Mutex::new(InMemorySession::new()),
        }
    }

    /// 互斥借用会话并执行闭包。
    pub fn with_session<R>(&self, f: impl FnOnce(&mut dyn WorkerSession) -> R) -> R {
        let mut guard = self.session.lock().expect("session pool mutex poisoned");
        f(&mut *guard)
    }
}

/// 对应 Go 的 `poolTestWrapper`：在每次借用前后递增/递减 `in_use` 计数，测试结束时可以断言
/// 计数归零，从而捕获"借了但没还"的 session 泄漏。
pub struct PoolTestWrapper<'a> {
    pool: &'a SessionPool,
    in_use: AtomicI64,
}

/// 在作用域退出（包括 panic 展开）时归还一次借用计数，对齐 Go `defer` 的语义。
struct InUseGuard<'a>(&'a AtomicI64);

impl Drop for InUseGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// 用零计数包装给定会话池，供测试断言无泄漏。
pub fn wrap_pool_for_test(pool: &SessionPool) -> PoolTestWrapper<'_> {
    PoolTestWrapper {
        pool,
        in_use: AtomicI64::new(0),
    }
}

impl<'a> PoolTestWrapper<'a> {
    /// 借用期间 `in_use` 加一；无论正常返回还是 panic 展开，都会在作用域退出时减一。
    pub fn with_session<R>(&self, f: impl FnOnce(&mut dyn WorkerSession) -> R) -> R {
        self.in_use.fetch_add(1, Ordering::SeqCst);
        let _in_use_guard = InUseGuard(&self.in_use);
        self.pool.with_session(f)
    }

    /// 当前借出中的会话数。
    pub fn in_use(&self) -> i64 {
        self.in_use.load(Ordering::SeqCst)
    }

    /// 对应 Go `AssertNoSessionInUse`：断言没有仍处于借出状态的 session。
    pub fn assert_no_session_in_use(&self) {
        assert_eq!(
            self.in_use(),
            0,
            "expected no session to be checked out from the pool"
        );
    }
}

// 对应 Go helper 的核心不变式：借用期间计数为 1，`with_session` 返回后立刻归零。
#[test]
fn test_pool_test_wrapper_tracks_in_use_count_around_with_session() {
    let pool = SessionPool::new();
    let wrapper = wrap_pool_for_test(&pool);
    wrapper.assert_no_session_in_use();

    let observed_during_call = wrapper.with_session(|_session| wrapper.in_use());
    assert_eq!(observed_during_call, 1);
    wrapper.assert_no_session_in_use();
}

// 对应 Go helper 在多次调用之间保持计数独立、互不残留的语义。
#[test]
fn test_pool_test_wrapper_resets_between_calls() {
    let pool = SessionPool::new();
    let wrapper = wrap_pool_for_test(&pool);

    for _ in 0..5 {
        wrapper.with_session(|_session| {});
        wrapper.assert_no_session_in_use();
    }
}

// 即便被包裹的闭包 panic，也不应该让计数永久卡在非零（借用计数应在 panic 展开时正确回退）。
#[test]
fn test_pool_test_wrapper_survives_panicking_closure() {
    let pool = SessionPool::new();
    let wrapper = wrap_pool_for_test(&pool);

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        wrapper.with_session(|_session| panic!("boom"));
    }));
    assert!(result.is_err());
    // Go 的 `defer w.inuse.Add(-1)` 保证 panic 展开时也会归还借用计数。
    assert_eq!(wrapper.in_use(), 0);
}

// SessionPool 本身也具备真实语义：状态在多次借用之间保持，写入的变量能在下一次借用时读到。
#[test]
fn test_session_pool_persists_state_across_borrows() {
    let pool = SessionPool::new();
    pool.with_session(|session| {
        session
            .state_mut()
            .variables
            .insert("tidb_retry_limit".into(), "0".into());
    });
    pool.with_session(|session| {
        assert_eq!(
            session
                .state()
                .variables
                .get("tidb_retry_limit")
                .map(String::as_str),
            Some("0")
        );
    });
}

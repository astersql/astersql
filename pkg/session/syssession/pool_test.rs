// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 系统会话池行为测试：创建、Get/Put/WithSession/Close 及复用语义。
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// 记录 TestContext 生命周期回调次数，供断言复用/关闭路径。
#[derive(Default)]
pub(crate) struct Lifecycle {
    /// on_became_owner 调用次数。
    pub(crate) became_owner: AtomicUsize,
    /// on_resign_owner 调用次数。
    pub(crate) resigned_owner: AtomicUsize,
    /// close 调用次数。
    pub(crate) closed: AtomicUsize,
    /// rollback_transaction 调用次数。
    pub(crate) rollback: AtomicUsize,
    /// reset_state 调用次数。
    pub(crate) reset: AtomicUsize,
    /// register_internal_session 调用次数。
    pub(crate) registered: AtomicUsize,
    /// unregister_internal_session 调用次数。
    pub(crate) unregistered: AtomicUsize,
    /// execute 调用次数。
    pub(crate) executed: AtomicUsize,
    /// 是否模拟存在未决事务。
    pub(crate) pending: AtomicBool,
    /// 模拟成为 owner 失败。
    pub(crate) fail_became_owner: AtomicBool,
    /// 模拟交出 owner 失败。
    pub(crate) fail_resign_owner: AtomicBool,
    /// 模拟回滚失败。
    pub(crate) fail_rollback: AtomicBool,
    /// 模拟状态重置失败。
    pub(crate) fail_reset: AtomicBool,
}

/// 测试用 SessionContext：把关键生命周期事件写入 Lifecycle。
pub(crate) struct TestContext {
    pub(crate) lifecycle: Arc<Lifecycle>,
}

impl crate::SessionContext for TestContext {
    fn close(&mut self) {
        self.lifecycle.closed.fetch_add(1, Ordering::SeqCst);
    }
    fn on_became_owner(&mut self) -> crate::Result<()> {
        self.lifecycle.became_owner.fetch_add(1, Ordering::SeqCst);
        if self.lifecycle.fail_became_owner.load(Ordering::SeqCst) {
            Err(crate::SessionError::new("on became owner failed"))
        } else {
            Ok(())
        }
    }
    fn on_resign_owner(&mut self) -> crate::Result<()> {
        self.lifecycle.resigned_owner.fetch_add(1, Ordering::SeqCst);
        if self.lifecycle.fail_resign_owner.load(Ordering::SeqCst) {
            Err(crate::SessionError::new("on resign owner failed"))
        } else {
            Ok(())
        }
    }
    fn has_pending_transaction(&self) -> bool {
        self.lifecycle.pending.load(Ordering::SeqCst)
    }
    fn rollback_transaction(&mut self) -> crate::Result<()> {
        self.lifecycle.rollback.fetch_add(1, Ordering::SeqCst);
        if self.lifecycle.fail_rollback.load(Ordering::SeqCst) {
            Err(crate::SessionError::new("rollback transaction failed"))
        } else {
            Ok(())
        }
    }
    fn reset_state(&mut self) -> crate::Result<()> {
        self.lifecycle.reset.fetch_add(1, Ordering::SeqCst);
        if self.lifecycle.fail_reset.load(Ordering::SeqCst) {
            Err(crate::SessionError::new("reset state failed"))
        } else {
            Ok(())
        }
    }
    fn register_internal_session(&mut self) -> bool {
        self.lifecycle.registered.fetch_add(1, Ordering::SeqCst);
        true
    }
    fn unregister_internal_session(&mut self) {
        self.lifecycle.unregistered.fetch_add(1, Ordering::SeqCst);
    }
    fn execute(&mut self, sql: &str) -> crate::Result<Vec<crate::RecordSet>> {
        self.lifecycle.executed.fetch_add(1, Ordering::SeqCst);
        Ok(vec![Box::new(sql.to_owned())])
    }
    fn execute_internal(
        &mut self,
        sql: &str,
        _args: &[crate::SqlValue],
    ) -> crate::Result<crate::RecordSet> {
        Ok(Box::new(sql.to_owned()))
    }
    fn execute_statement(&mut self, _statement: &dyn Any) -> crate::Result<crate::RecordSet> {
        Ok(Box::new(()))
    }
    fn parse_with_params(
        &mut self,
        sql: &str,
        _args: &[crate::SqlValue],
    ) -> crate::Result<crate::Statement> {
        Ok(Box::new(sql.to_owned()))
    }
    fn exec_restricted_statement(
        &mut self,
        _statement: &dyn Any,
    ) -> crate::Result<Vec<crate::Row>> {
        Ok(vec![Box::new(1_i64)])
    }
    fn exec_restricted_sql(
        &mut self,
        sql: &str,
        _args: &[crate::SqlValue],
    ) -> crate::Result<Vec<crate::Row>> {
        Ok(vec![Box::new(sql.to_owned())])
    }
}

/// Close 幂等，且关闭后 Get 返回 "session pool closed"。
#[test]
fn advanced_pool_close_is_idempotent_and_rejects_get() {
    let pool = crate::NewAdvancedSessionPool(1, || {
        Err(crate::SessionError::new("factory must not be reached"))
    });
    assert!(!pool.IsClosed());
    pool.Close();
    pool.Close();
    assert!(pool.IsClosed());
    let error = match pool.Get() {
        Ok(_) => panic!("closed pool unexpectedly returned a session"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), "session pool closed");
}

/// 干净会话 Put 后可复用；AvoidReuse 的会话 Put 时关闭而非入池。
#[test]
fn advanced_pool_reuses_clean_sessions_and_closes_avoid_reuse_sessions() {
    let lifecycle = Arc::new(Lifecycle::default());
    let factory_state = Arc::clone(&lifecycle);
    let pool = crate::NewAdvancedSessionPool(1, move || {
        Ok(Box::new(TestContext {
            lifecycle: Arc::clone(&factory_state),
        }))
    });
    let session = pool.Get().expect("create system session");
    assert_eq!(session.Execute("select 1").unwrap().len(), 1);
    // Put 触发 rollback + reset，会话进入空闲队列。
    pool.Put(&session);
    assert_eq!(pool.Size(), 1);
    assert_eq!(lifecycle.rollback.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.reset.load(Ordering::SeqCst), 1);

    let reused = pool.Get().expect("reuse system session");
    reused.AvoidReuse();
    // 标记不可复用后 Put 应关闭并保持队列为空。
    pool.Put(&reused);
    assert_eq!(pool.Size(), 0);
    assert_eq!(lifecycle.closed.load(Ordering::SeqCst), 1);

    let failing_lifecycle = Arc::new(Lifecycle::default());
    failing_lifecycle
        .fail_became_owner
        .store(true, Ordering::SeqCst);
    let failing_state = Arc::clone(&failing_lifecycle);
    let failing_pool = crate::NewAdvancedSessionPool(1, move || {
        Ok(Box::new(TestContext {
            lifecycle: Arc::clone(&failing_state),
        }))
    });
    let error = match failing_pool.Get() {
        Ok(_) => panic!("failing owner hook unexpectedly created a session"),
        Err(error) => error,
    };
    assert_eq!(error.to_string(), "on became owner failed");
    assert_eq!(failing_lifecycle.closed.load(Ordering::SeqCst), 1);
}

/// 对照 Go TestNewSessionPool：正常容量和非法容量回退规则保持一致。
#[test]
fn test_new_session_pool() {
    let expected = [
        (128, 128_usize),
        (crate::PoolMaxSize as isize, crate::PoolMaxSize),
        (0, crate::PoolMaxSize),
        (-1, crate::PoolMaxSize),
        (crate::PoolMaxSize as isize + 1, crate::PoolMaxSize),
    ];
    for (capacity, expected_capacity) in expected {
        let pool = crate::NewAdvancedSessionPool(capacity, || {
            Err(crate::SessionError::new("factory must not run"))
        });
        assert_eq!(pool.Capacity(), expected_capacity);
        assert_eq!(pool.Size(), 0);
        assert!(!pool.IsClosed());
    }
}

/// 对照 Go TestSessionPoolWithSession：成功归还，错误和 panic 都关闭会话，
/// Get 失败时不执行回调。
#[test]
fn test_session_pool_with_session() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let lifecycle = Arc::new(Lifecycle::default());
    let factory_state = Arc::clone(&lifecycle);
    let pool = crate::NewAdvancedSessionPool(1, move || {
        Ok(Box::new(TestContext {
            lifecycle: Arc::clone(&factory_state),
        }))
    });

    pool.WithSession(|session| {
        assert!(session.IsOwner());
        Ok(())
    })
    .expect("successful callback");
    assert_eq!(pool.Size(), 1);

    let error = pool
        .WithSession(|_| Err(crate::SessionError::new("callback failed")))
        .unwrap_err();
    assert_eq!(error.to_string(), "callback failed");
    assert_eq!(pool.Size(), 0);
    assert_eq!(lifecycle.closed.load(Ordering::SeqCst), 1);

    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = pool.WithSession(|_| -> crate::Result<()> { panic!("callback panic") });
    }));
    assert!(panic.is_err());
    assert_eq!(pool.Size(), 0);
    assert_eq!(lifecycle.closed.load(Ordering::SeqCst), 2);

    let failing_pool =
        crate::NewAdvancedSessionPool(1, || Err(crate::SessionError::new("factory failed")));
    let called = AtomicBool::new(false);
    let error = failing_pool
        .WithSession(|_| {
            called.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "factory failed");
    assert!(!called.load(Ordering::SeqCst));
}

/// 对照 Go TestSessionPoolPut：未决事务、回滚/重置失败、满池和已关闭池
/// 都必须关闭归还的内部会话，不能重新进入空闲队列。
#[test]
fn test_session_pool_put_rejects_unclean_or_unstorable_sessions() {
    fn pool_with_lifecycle(lifecycle: &Arc<Lifecycle>) -> crate::AdvancedSessionPool {
        let factory_state = Arc::clone(lifecycle);
        crate::NewAdvancedSessionPool(1, move || {
            Ok(Box::new(TestContext {
                lifecycle: Arc::clone(&factory_state),
            }))
        })
    }

    let pending = Arc::new(Lifecycle::default());
    pending.pending.store(true, Ordering::SeqCst);
    let pool = pool_with_lifecycle(&pending);
    let session = pool.Get().expect("create pending session");
    pool.Put(&session);
    assert_eq!(pool.Size(), 0);
    assert_eq!(pending.rollback.load(Ordering::SeqCst), 0);
    assert_eq!(pending.reset.load(Ordering::SeqCst), 0);
    assert_eq!(pending.closed.load(Ordering::SeqCst), 1);

    let rollback_failure = Arc::new(Lifecycle::default());
    rollback_failure.fail_rollback.store(true, Ordering::SeqCst);
    let pool = pool_with_lifecycle(&rollback_failure);
    let session = pool.Get().expect("create rollback-failure session");
    pool.Put(&session);
    assert_eq!(pool.Size(), 0);
    assert_eq!(rollback_failure.rollback.load(Ordering::SeqCst), 1);
    assert_eq!(rollback_failure.reset.load(Ordering::SeqCst), 0);
    assert_eq!(rollback_failure.closed.load(Ordering::SeqCst), 1);

    let reset_failure = Arc::new(Lifecycle::default());
    reset_failure.fail_reset.store(true, Ordering::SeqCst);
    let pool = pool_with_lifecycle(&reset_failure);
    let session = pool.Get().expect("create reset-failure session");
    pool.Put(&session);
    assert_eq!(pool.Size(), 0);
    assert_eq!(reset_failure.rollback.load(Ordering::SeqCst), 1);
    assert_eq!(reset_failure.reset.load(Ordering::SeqCst), 1);
    assert_eq!(reset_failure.closed.load(Ordering::SeqCst), 1);

    let full = Arc::new(Lifecycle::default());
    let pool = pool_with_lifecycle(&full);
    let first = pool.Get().expect("create first session");
    let second = pool.Get().expect("create second session");
    pool.Put(&first);
    pool.Put(&second);
    assert_eq!(pool.Size(), 1);
    assert_eq!(full.closed.load(Ordering::SeqCst), 1);
    pool.Close();
    assert_eq!(full.closed.load(Ordering::SeqCst), 2);

    let closed = Arc::new(Lifecycle::default());
    let pool = pool_with_lifecycle(&closed);
    let session = pool.Get().expect("create session before closing pool");
    pool.Close();
    pool.Put(&session);
    assert_eq!(pool.Size(), 0);
    assert_eq!(closed.closed.load(Ordering::SeqCst), 1);
}

/// 对照 Go WithForceBlockGCSession：注册成功才运行回调；取消时关闭会话。
#[test]
fn test_session_pool_with_force_block_gc_session() {
    let lifecycle = Arc::new(Lifecycle::default());
    let factory_state = Arc::clone(&lifecycle);
    let pool = crate::NewAdvancedSessionPool(1, move || {
        Ok(Box::new(TestContext {
            lifecycle: Arc::clone(&factory_state),
        }))
    });
    let cancellation = crate::CancellationToken::default();
    let called = AtomicBool::new(false);
    pool.WithForceBlockGCSession(&cancellation, |session| {
        assert!(session.IsOwner());
        called.store(true, Ordering::SeqCst);
        Ok(())
    })
    .expect("registered callback succeeds");
    assert!(called.load(Ordering::SeqCst));
    assert_eq!(lifecycle.registered.load(Ordering::SeqCst), 1);
    assert_eq!(pool.Size(), 1);

    let cancelled_lifecycle = Arc::new(Lifecycle::default());
    let cancelled_state = Arc::clone(&cancelled_lifecycle);
    let cancelled_pool = crate::NewAdvancedSessionPool(1, move || {
        Ok(Box::new(TestContext {
            lifecycle: Arc::clone(&cancelled_state),
        }))
    });
    let cancellation = crate::CancellationToken::default();
    cancellation.cancel();
    let called = AtomicBool::new(false);
    let error = cancelled_pool
        .WithForceBlockGCSession(&cancellation, |_| {
            called.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "operation cancelled");
    assert!(!called.load(Ordering::SeqCst));
    assert_eq!(cancelled_pool.Size(), 0);
    assert_eq!(cancelled_lifecycle.closed.load(Ordering::SeqCst), 1);
}

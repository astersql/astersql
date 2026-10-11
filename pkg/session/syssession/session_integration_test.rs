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

// Domain 高级系统会话池与内部会话登记/脏会话回收集成测试。
/// 无内部会话的空壳 Session：非 Owner、视为已关闭，Execute 报错。
#[test]
fn unowned_session_is_closed_and_cannot_execute() {
    let session = crate::Session::default();
    assert!(!session.IsOwner());
    assert!(session.IsInternalClosed());
    assert_eq!(
        session.Execute("select 1").unwrap_err().to_string(),
        "session is closed"
    );
}

/// WithForceBlockGCSession 会注册内部会话，成功后干净归还池中。
#[test]
fn force_block_gc_registers_then_returns_clean_session_to_pool() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let factory_state = Arc::clone(&lifecycle);
    let pool = crate::NewAdvancedSessionPool(1, move || {
        Ok(Box::new(crate::pool_test::TestContext {
            lifecycle: Arc::clone(&factory_state),
        }))
    });
    let cancellation = crate::CancellationToken::default();
    // 注册成功后执行受限 SQL，再 Put 回池。
    pool.WithForceBlockGCSession(&cancellation, |session| {
        assert_eq!(session.ExecRestrictedSQL("select 1", &[])?.len(), 1);
        Ok(())
    })
    .expect("registered GC-blocking system session");

    assert_eq!(lifecycle.registered.load(Ordering::SeqCst), 1);
    assert_eq!(pool.Size(), 1);
}

/// Go only invokes the registry hooks while the public `Session` owns the
/// internal session. The pool is a noop owner, so creating an internal session
/// and returning it to the pool must not look like additional registrations.
#[test]
fn public_session_ownership_has_one_registry_lifecycle() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let factory_state = Arc::clone(&lifecycle);
    let pool = crate::NewAdvancedSessionPool(1, move || {
        Ok(Box::new(crate::pool_test::TestContext {
            lifecycle: Arc::clone(&factory_state),
        }))
    });

    let session = pool.Get().expect("create owned system session");
    assert!(session.IsOwner());
    assert_eq!(lifecycle.became_owner.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.resigned_owner.load(Ordering::SeqCst), 0);

    pool.Put(&session);
    assert!(!session.IsOwner());
    assert_eq!(lifecycle.became_owner.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.resigned_owner.load(Ordering::SeqCst), 1);
    assert_eq!(pool.Size(), 1);
}

/// 对照 Go TestDomainAdvancedSessionPoolPutBackDirtySession：错误、未决事务、
/// avoid-reuse 和重置失败的会话都不得回池，干净会话可复用。
#[test]
fn test_domain_advanced_session_pool_put_back_dirty_session() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let factory_state = Arc::clone(&lifecycle);
    let pool = crate::NewAdvancedSessionPool(1, move || {
        Ok(Box::new(crate::pool_test::TestContext {
            lifecycle: Arc::clone(&factory_state),
        }))
    });

    let callback_error = pool
        .WithSession(|_| Err(crate::SessionError::new("callback error")))
        .unwrap_err();
    assert_eq!(callback_error.to_string(), "callback error");
    assert_eq!(pool.Size(), 0);

    let pending = pool.Get().expect("pending session");
    lifecycle.pending.store(true, Ordering::SeqCst);
    pool.Put(&pending);
    assert!(pending.IsInternalClosed());
    assert_eq!(pool.Size(), 0);
    lifecycle.pending.store(false, Ordering::SeqCst);

    let avoid = pool.Get().expect("avoid-reuse session");
    avoid.AvoidReuse();
    pool.Put(&avoid);
    assert!(avoid.IsInternalClosed());
    assert_eq!(pool.Size(), 0);

    let reset_failure = pool.Get().expect("reset-failure session");
    lifecycle.fail_reset.store(true, Ordering::SeqCst);
    pool.Put(&reset_failure);
    assert!(reset_failure.IsInternalClosed());
    assert_eq!(pool.Size(), 0);
    lifecycle.fail_reset.store(false, Ordering::SeqCst);

    let clean = pool.Get().expect("clean session");
    pool.Put(&clean);
    pool.Put(&clean);
    assert!(!clean.IsOwner());
    assert!(!clean.IsInternalClosed());
    assert_eq!(pool.Size(), 1);
}

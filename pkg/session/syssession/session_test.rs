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

// 内部会话、Owner 转移、代理方法与线程安全边界的单元测试。
/// SessionError 保留构造时的规范消息，且同消息可判等。
#[test]
fn session_error_preserves_canonical_message() {
    let error = crate::SessionError::new("rollback transaction failed");
    assert_eq!(error.to_string(), "rollback transaction failed");
    assert_eq!(
        error,
        crate::SessionError::new("rollback transaction failed")
    );
}

/// 持有 Owner 的会话可委托 Execute；Close 幂等且触发注销与关闭。
#[test]
fn owned_session_delegates_operations_and_closes_once() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext {
        lifecycle: Arc::clone(&lifecycle),
    }))
    .expect("create owned session");
    assert!(session.IsOwner());
    assert_eq!(session.Execute("select 42").unwrap().len(), 1);
    // 二次 Close 不应重复注销/关闭。
    session.Close();
    session.Close();
    assert!(session.IsInternalClosed());
    assert_eq!(lifecycle.executed.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.unregistered.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.closed.load(Ordering::SeqCst), 1);
}

/// 对照 Go TestInternalSessionAvoidReuse：代理操作 panic 后仍完成退出清理，
/// 并把会话标记为不可复用。
#[test]
fn test_internal_session_avoid_reuse() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext { lifecycle }))
        .expect("create owned session");

    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = session.WithSessionContext::<()>(|_| panic!("proxy panic"));
    }));
    assert!(panic.is_err());
    assert!(session.IsAvoidReuse());

    session.Close();
    assert!(session.IsInternalClosed());
}

fn take_error<T>(result: crate::Result<T>) -> crate::SessionError {
    match result {
        Ok(_) => panic!("operation unexpectedly succeeded"),
        Err(error) => error,
    }
}

/// 对照 Go TestNewInternalSession：初始化 owner 状态，hook 失败时不返回会话。
#[test]
fn test_new_internal_session() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext {
        lifecycle: Arc::clone(&lifecycle),
    }))
    .expect("new internal session");
    assert!(session.IsOwner());
    assert_eq!(session.InuseForTest(), 0);
    assert!(!session.IsAvoidReuse());
    assert_eq!(lifecycle.became_owner.load(Ordering::SeqCst), 1);

    let failing = Arc::new(crate::pool_test::Lifecycle::default());
    failing.fail_became_owner.store(true, Ordering::SeqCst);
    let error = take_error(crate::NewSessionForTest(Box::new(
        crate::pool_test::TestContext {
            lifecycle: Arc::clone(&failing),
        },
    )));
    assert_eq!(error.to_string(), "on became owner failed");
    assert_eq!(failing.closed.load(Ordering::SeqCst), 0);
}

/// 对照 Go TestResignOwnerAndCloseSctx：resign 失败也必须关闭上下文。
#[test]
fn test_resign_owner_and_close_sctx() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    lifecycle.fail_resign_owner.store(true, Ordering::SeqCst);
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext {
        lifecycle: Arc::clone(&lifecycle),
    }))
    .expect("create session");
    session.Close();
    assert!(session.IsInternalClosed());
    assert_eq!(lifecycle.resigned_owner.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.closed.load(Ordering::SeqCst), 1);
}

/// 对照 Go TestInternalSessionTransferOwner：校验来源 owner、同 owner 幂等，
/// hook 失败时关闭会话。
#[test]
fn test_internal_session_transfer_owner() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext {
        lifecycle: Arc::clone(&lifecycle),
    }))
    .expect("create session");
    let internal = session.internal.as_ref().expect("internal session");
    assert!(crate::session::transfer_owner(internal, session.owner(), session.owner()).is_ok());
    assert!(session.IsOwner());

    let invalid = crate::session::Owner::Pool(u64::MAX);
    let error = crate::session::transfer_owner(internal, invalid, session.owner()).unwrap_err();
    assert!(error.to_string().contains("TransferOwner error"));
    assert!(session.IsOwner());

    lifecycle.fail_resign_owner.store(true, Ordering::SeqCst);
    let error = crate::session::transfer_owner(internal, session.owner(), invalid).unwrap_err();
    assert_eq!(error.to_string(), "on resign owner failed");
    assert!(session.IsInternalClosed());
    assert_eq!(lifecycle.closed.load(Ordering::SeqCst), 1);
}

/// Go 的 nil owner 表示已关闭：既不能转入 nil，也不能从已关闭状态继续转移。
#[test]
fn transfer_owner_rejects_closed_owner_like_go_nil() {
    use std::sync::Arc;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext { lifecycle }))
        .expect("create session");
    let internal = session.internal.as_ref().expect("internal session");

    let error =
        crate::session::transfer_owner(internal, session.owner(), crate::session::Owner::Closed)
            .expect_err("Go rejects transferring to a nil owner");
    assert!(
        error
            .to_string()
            .contains("cannot transfer to a closed owner")
    );
    assert!(session.IsOwner());

    session.Close();
    let error = crate::session::transfer_owner(
        internal,
        crate::session::Owner::Closed,
        crate::session::Owner::Closed,
    )
    .expect_err("Go rejects transfers from an already closed session");
    assert!(error.to_string().contains("session is closed"));
}

struct BlockingContext {
    inner: crate::pool_test::TestContext,
    started: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

impl crate::SessionContext for BlockingContext {
    fn close(&mut self) {
        crate::SessionContext::close(&mut self.inner);
    }
    fn on_became_owner(&mut self) -> crate::Result<()> {
        crate::SessionContext::on_became_owner(&mut self.inner)
    }
    fn on_resign_owner(&mut self) -> crate::Result<()> {
        crate::SessionContext::on_resign_owner(&mut self.inner)
    }
    fn has_pending_transaction(&self) -> bool {
        crate::SessionContext::has_pending_transaction(&self.inner)
    }
    fn rollback_transaction(&mut self) -> crate::Result<()> {
        crate::SessionContext::rollback_transaction(&mut self.inner)
    }
    fn reset_state(&mut self) -> crate::Result<()> {
        crate::SessionContext::reset_state(&mut self.inner)
    }
    fn register_internal_session(&mut self) -> bool {
        crate::SessionContext::register_internal_session(&mut self.inner)
    }
    fn unregister_internal_session(&mut self) {
        crate::SessionContext::unregister_internal_session(&mut self.inner);
    }
    fn execute(&mut self, sql: &str) -> crate::Result<Vec<crate::RecordSet>> {
        self.started.send(()).expect("signal operation start");
        self.release.recv().expect("wait for operation release");
        crate::SessionContext::execute(&mut self.inner, sql)
    }
    fn execute_internal(
        &mut self,
        sql: &str,
        args: &[crate::SqlValue],
    ) -> crate::Result<crate::RecordSet> {
        crate::SessionContext::execute_internal(&mut self.inner, sql, args)
    }
    fn execute_statement(
        &mut self,
        statement: &dyn std::any::Any,
    ) -> crate::Result<crate::RecordSet> {
        crate::SessionContext::execute_statement(&mut self.inner, statement)
    }
    fn parse_with_params(
        &mut self,
        sql: &str,
        args: &[crate::SqlValue],
    ) -> crate::Result<crate::Statement> {
        crate::SessionContext::parse_with_params(&mut self.inner, sql, args)
    }
    fn exec_restricted_statement(
        &mut self,
        statement: &dyn std::any::Any,
    ) -> crate::Result<Vec<crate::Row>> {
        crate::SessionContext::exec_restricted_statement(&mut self.inner, statement)
    }
    fn exec_restricted_sql(
        &mut self,
        sql: &str,
        args: &[crate::SqlValue],
    ) -> crate::Result<Vec<crate::Row>> {
        crate::SessionContext::exec_restricted_sql(&mut self.inner, sql, args)
    }
}

fn blocking_session() -> (
    std::sync::Arc<crate::Session>,
    std::sync::Arc<crate::pool_test::Lifecycle>,
    std::sync::mpsc::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    use std::sync::{Arc, mpsc};

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let session = crate::NewSessionForTest(Box::new(BlockingContext {
        inner: crate::pool_test::TestContext {
            lifecycle: Arc::clone(&lifecycle),
        },
        started: started_tx,
        release: release_rx,
    }))
    .expect("create blocking session");
    (Arc::new(session), lifecycle, started_rx, release_tx)
}

/// 对照 Go TestInternalSessionClose：有进行中操作时先标记关闭，
/// 最后一个操作退出后才关闭底层上下文。
#[test]
fn test_internal_session_close() {
    use std::sync::atomic::Ordering;

    let (session, lifecycle, started, release) = blocking_session();
    let worker_session = std::sync::Arc::clone(&session);
    let worker = std::thread::spawn(move || worker_session.Execute("select 1"));
    started.recv().expect("operation started");
    assert_eq!(session.InuseForTest(), 1);
    session.Close();
    assert!(session.IsInternalClosed());
    assert_eq!(lifecycle.closed.load(Ordering::SeqCst), 0);
    release.send(()).expect("release operation");
    assert_eq!(worker.join().expect("worker join").unwrap().len(), 1);
    assert_eq!(session.InuseForTest(), 0);
    assert_eq!(lifecycle.closed.load(Ordering::SeqCst), 1);
    session.Close();
    assert_eq!(lifecycle.closed.load(Ordering::SeqCst), 1);
}

/// 对照 Go TestInternalSessionEnterOperation：进入/退出配对更新 inUse，
/// 非 owner 和已关闭会话均拒绝操作。
#[test]
fn test_internal_session_enter_operation() {
    use std::sync::Arc;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext { lifecycle }))
        .expect("create session");
    session
        .WithSessionContext(|_| {
            assert_eq!(session.InuseForTest(), 1);
            Ok(())
        })
        .expect("owned operation");
    assert_eq!(session.InuseForTest(), 0);

    let impostor = crate::Session::from_internal(Arc::clone(
        session.internal.as_ref().expect("internal session"),
    ));
    let error = impostor.WithSessionContext(|_| Ok(())).unwrap_err();
    assert_eq!(error.to_string(), "session is not owned by the caller");
    session.Close();
    assert_eq!(
        session
            .WithSessionContext(|_| Ok(()))
            .unwrap_err()
            .to_string(),
        "session is closed"
    );
}

/// 对照 Go TestInternalSessionOwnerWithSctx：回调错误原样返回，
/// 回调 panic 后仍归零 inUse。
#[test]
fn test_internal_session_owner_with_sctx() {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Arc;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext { lifecycle }))
        .expect("create session");
    let error = session
        .WithSessionContext::<()>(|_| Err(crate::SessionError::new("callback error")))
        .unwrap_err();
    assert_eq!(error.to_string(), "callback error");
    assert_eq!(session.InuseForTest(), 0);

    let panic = catch_unwind(AssertUnwindSafe(|| {
        let _ = session.WithSessionContext::<()>(|_| panic!("callback panic"));
    }));
    assert!(panic.is_err());
    assert_eq!(session.InuseForTest(), 0);
    assert!(session.IsAvoidReuse());
}

/// 对照 Go TestInternalSessionCheckNoPendingTxn：未决事务拒绝回池，
/// 无未决事务时允许回池。
#[test]
fn test_internal_session_check_no_pending_txn() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let factory_state = Arc::clone(&lifecycle);
    let pool = crate::NewAdvancedSessionPool(1, move || {
        Ok(Box::new(crate::pool_test::TestContext {
            lifecycle: Arc::clone(&factory_state),
        }))
    });
    let pending = pool.Get().expect("pending session");
    lifecycle.pending.store(true, Ordering::SeqCst);
    pool.Put(&pending);
    assert!(pending.IsInternalClosed());
    assert_eq!(pool.Size(), 0);

    lifecycle.pending.store(false, Ordering::SeqCst);
    let clean = pool.Get().expect("clean session");
    pool.Put(&clean);
    assert_eq!(pool.Size(), 1);
}

/// 对照 Go TestInternalSessionResetState：回滚和重置成功后回池，
/// 任一步失败都关闭会话。
#[test]
fn test_internal_session_reset_state() {
    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let factory_state = Arc::clone(&lifecycle);
    let pool = crate::NewAdvancedSessionPool(1, move || {
        Ok(Box::new(crate::pool_test::TestContext {
            lifecycle: Arc::clone(&factory_state),
        }))
    });
    let clean = pool.Get().expect("clean session");
    pool.Put(&clean);
    assert_eq!(lifecycle.rollback.load(Ordering::SeqCst), 1);
    assert_eq!(lifecycle.reset.load(Ordering::SeqCst), 1);
    assert_eq!(pool.Size(), 1);

    let rollback_failure = pool.Get().expect("rollback failure session");
    lifecycle.fail_rollback.store(true, Ordering::SeqCst);
    pool.Put(&rollback_failure);
    assert!(rollback_failure.IsInternalClosed());
    assert_eq!(pool.Size(), 0);
}

/// 对照 Go TestSessionAvoidReuse：标记幂等、标记后仍可使用，
/// 非 owner 不能修改底层标记。
#[test]
fn test_session_avoid_reuse() {
    use std::sync::Arc;

    let lifecycle = Arc::new(crate::pool_test::Lifecycle::default());
    let session = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext { lifecycle }))
        .expect("create session");
    session.AvoidReuse();
    session.AvoidReuse();
    assert!(session.IsAvoidReuse());
    assert!(session.ExecuteInternal("select 1", &[]).is_ok());

    let other = crate::Session::from_internal(Arc::clone(
        session.internal.as_ref().expect("internal session"),
    ));
    let lifecycle2 = Arc::new(crate::pool_test::Lifecycle::default());
    let owned = crate::NewSessionForTest(Box::new(crate::pool_test::TestContext {
        lifecycle: lifecycle2,
    }))
    .expect("second session");
    let other2 = crate::Session::from_internal(Arc::clone(
        owned.internal.as_ref().expect("second internal session"),
    ));
    assert!(!owned.IsAvoidReuse());
    other2.AvoidReuse();
    assert!(!owned.IsAvoidReuse());
    drop(other);
}

/// 对照 Go TestInternalSessionUnThreadSafeOperations：第二个线程不安全操作
/// 被拒绝并累计冲突计数，首个操作退出后计数归零。
#[test]
fn test_internal_session_un_thread_safe_operations() {
    let (session, _, started, release) = blocking_session();
    let worker_session = std::sync::Arc::clone(&session);
    let worker = std::thread::spawn(move || worker_session.Execute("select 1"));
    started.recv().expect("operation started");
    assert_eq!(session.InuseForTest(), 1);
    assert_eq!(session.UnsafeForTest(), 1);
    let error = take_error(session.ExecuteInternal("select 2", &[]));
    assert_eq!(
        error.to_string(),
        "EnterOperation error: race detected for concurrent thread-unsafe operations"
    );
    assert_eq!(session.InuseForTest(), 1);
    assert_eq!(session.UnsafeForTest(), 2);
    release.send(()).expect("release operation");
    assert!(worker.join().expect("worker join").is_ok());
    assert_eq!(session.InuseForTest(), 0);
    assert_eq!(session.UnsafeForTest(), 0);
}

/// 对照 Go TestSessionThreadUnsafeOperations：全部代理方法共享同一个
/// thread-unsafe 互斥检测，竞争调用不会进入底层上下文。
#[test]
fn test_session_thread_unsafe_operations() {
    let (session, _, started, release) = blocking_session();

    assert!(session.ExecuteInternal("select internal", &[]).is_ok());
    assert!(session.ExecuteStmt(&()).is_ok());
    assert!(session.ParseWithParams("select ?", &[]).is_ok());
    assert!(session.ExecRestrictedStmt(&()).is_ok());
    assert!(session.ExecRestrictedSQL("select restricted", &[]).is_ok());

    let worker_session = std::sync::Arc::clone(&session);
    let worker = std::thread::spawn(move || worker_session.Execute("select blocking"));
    started.recv().expect("operation started");
    let expected = "EnterOperation error: race detected for concurrent thread-unsafe operations";
    assert_eq!(
        take_error(session.ExecuteInternal("select 1", &[])).to_string(),
        expected
    );
    assert_eq!(take_error(session.ExecuteStmt(&())).to_string(), expected);
    assert_eq!(
        take_error(session.ParseWithParams("select ?", &[])).to_string(),
        expected
    );
    assert_eq!(
        take_error(session.ExecRestrictedStmt(&())).to_string(),
        expected
    );
    assert_eq!(
        take_error(session.ExecRestrictedSQL("select 1", &[])).to_string(),
        expected
    );
    release.send(()).expect("release operation");
    assert!(worker.join().expect("worker join").is_ok());
    assert_eq!(session.InuseForTest(), 0);
    assert_eq!(session.UnsafeForTest(), 0);
}

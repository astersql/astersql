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
//
// 前半段以原始字符串保留 Go 测试草稿；后半段为可运行的 Rust 单元测试与 mock 上下文。

// session pool 的创建、Get/Put/WithSession/Close 行为测试。
// testing、require、testkit、session、failpoint、goroutine/channel、KV/DDL/bootstrap 等依赖均按 Go 调用形状保留。

/// 保留的 Go 版 pool_test 源码草稿，便于对照迁移语义（非可执行 Rust）。
const GO_POOL_TEST_DRAFT: &str = r########################################"

// mockSessionFactory 对应 Go 的同名辅助类型，字段和嵌入接口按原测试 mock 语义保留。
// Go 类型声明: type mockSessionFactory struct {
pub struct mockSessionFactory {
	mock.Mock
}

// create 对应 Go 方法，接收者 `f *mockSessionFactory`；保留 mock/session 接口调用语义。
// Go 签名: func (f *mockSessionFactory) create() (SessionContext, error) {
pub fn create() {
	args := f.Called()
	if args.Get(0) == nil {
		return nil, args.Error(1)
	}
	return args.Get(0).(SessionContext), args.Error(1)
}

// TestNewSessionPool 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestNewSessionPool(t *testing.T) {
#[test]
pub fn TestNewSessionPool() {
	factory := func() (SessionContext, error) {
		return &mockSessionContext{}, nil
	}

	p := NewAdvancedSessionPool(128, factory)
	require.NotNil(t, p)
	require.Equal(t, 128, cap(p.pool))
	require.Equal(t, 0, len(p.pool))
	require.False(t, p.IsClosed())
	require.NotNil(t, p.ctx)
	require.NoError(t, p.ctx.Err())

	// pool with PoolMaxSize
	p = NewAdvancedSessionPool(PoolMaxSize, factory)
	require.Equal(t, PoolMaxSize, cap(p.pool))
	require.False(t, p.IsClosed())

	// pool with zero-size
	WithSuppressAssert(func() {
		p = NewAdvancedSessionPool(0, factory)
		require.Equal(t, PoolMaxSize, cap(p.pool))
		require.False(t, p.IsClosed())
	})

	// test pool size limit
	WithSuppressAssert(func() {
		p = NewAdvancedSessionPool(PoolMaxSize+1, factory)
		require.Equal(t, PoolMaxSize, cap(p.pool))
		require.False(t, p.IsClosed())
	})

	WithSuppressAssert(func() {
		p = NewAdvancedSessionPool(-1, factory)
		require.Equal(t, PoolMaxSize, cap(p.pool))
		require.False(t, p.IsClosed())
	})
}

// TestSessionPoolGet 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestSessionPoolGet(t *testing.T) {
#[test]
pub fn TestSessionPoolGet() {
	mockFactory := &mockSessionFactory{}
	p := NewAdvancedSessionPool(128, mockFactory.create)

	// get a new Session from pool
	sctx := &mockSessionContext{}
	mockFactory.On("create").Return(sctx, nil).Once()
	se, err := p.Get()
	require.NoError(t, err)
	require.Same(t, se, se.internal.Owner())
	require.False(t, se.internal.IsClosed())
	require.Zero(t, se.internal.Inuse())
	mockFactory.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// reuse the session
	// 这里跨越事务、KV 或元数据访问边界；只保留 Go 调用顺序与错误检查语义。
	sctx.MockNoPendingTxn()
	sctx.MockResetState(p.ctx, "")
	p.Put(se)
	require.Equal(t, 1, len(p.pool))
	sctx.AssertExpectations(t)
	se2, err := p.Get()
	require.NoError(t, err)
	require.NotSame(t, se, se2)
	require.Equal(t, 0, len(p.pool))
	require.Same(t, se.internal, se2.internal)
	require.Same(t, se2, se2.internal.Owner())
	require.False(t, se2.internal.IsClosed())
	require.Zero(t, se2.internal.Inuse())
	mockFactory.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// factory returns error
	mockFactory.On("create").Return(nil, errors.New("mockErr")).Once()
	se, err = p.Get()
	require.EqualError(t, err, "mockErr")
	require.Nil(t, se)
	mockFactory.AssertExpectations(t)

	// get session from a closed pool
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
	p.Close()
	se, err = p.Get()
	require.EqualError(t, err, "session pool closed")
	require.Nil(t, se)
}

// TestSessionPoolPut 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestSessionPoolPut(t *testing.T) {
#[test]
pub fn TestSessionPoolPut() {
	mockFactory := &mockSessionFactory{}
	poolCap := 4
	p := NewAdvancedSessionPool(poolCap, mockFactory.create)
	require.Equal(t, 4, cap(p.pool))
	// Put invalid Session
	WithSuppressAssert(func() {
		p.Put(nil)
		p.Put(&Session{})
		require.Equal(t, 0, len(p.pool))
	})

	getCachedSessionFromPool := func(sctx *mockSessionContext) *Session {
		se, err := p.Get()
		require.NoError(t, err)
		require.Same(t, se, se.internal.Owner())
		require.True(t, se.IsOwner())
		mockFactory.AssertExpectations(t)
		sctx.AssertExpectations(t)
		return se
	}

	getNewSessionFromPool := func(sctx *mockSessionContext) *Session {
		mockFactory.On("create").Return(sctx, nil).Once()
		se, err := p.Get()
		require.NoError(t, err)
		require.Same(t, se, se.internal.Owner())
		require.True(t, se.IsOwner())
		mockFactory.AssertExpectations(t)
		sctx.AssertExpectations(t)
		return se
	}

	// Put a normal session
	sctx := &mockSessionContext{}
	se := getNewSessionFromPool(sctx)
	sctx.MockResetState(p.ctx, "")
	// 这里跨越事务、KV 或元数据访问边界；只保留 Go 调用顺序与错误检查语义。
	sctx.MockNoPendingTxn()
	p.Put(se)
	require.Same(t, p, se.internal.Owner())
	require.False(t, se.IsOwner())
	require.False(t, se.IsInternalClosed())
	mockFactory.AssertExpectations(t)
	sctx.AssertExpectations(t)
	require.Equal(t, 1, len(p.pool))

	// Get a cached Session and put the old one that is not the owner
	se2 := getCachedSessionFromPool(sctx)
	require.Equal(t, 0, len(p.pool))
	p.Put(se)
	require.Equal(t, 0, len(p.pool))

	// Put a Session that is the owner
	sctx.MockNoPendingTxn()
	sctx.MockResetState(p.ctx, "")
	p.Put(se2)
	require.Same(t, p, se2.internal.Owner())
	mockFactory.AssertExpectations(t)
	sctx.AssertExpectations(t)
	require.Equal(t, 1, len(p.pool))

	// Put a Session again takes no effect
	p.Put(se2)
	require.Equal(t, 1, len(p.pool))
	require.Same(t, p, se2.internal.Owner())

	// Put a Session that is inUse
	se = getCachedSessionFromPool(sctx)
	require.Equal(t, 0, len(p.pool))
	_, exit, err := se.internal.EnterOperation(se, false)
	require.NoError(t, err)
	WithSuppressAssert(func() {
		p.Put(se)
	})
	require.True(t, se.internal.IsClosed())
	require.False(t, se.IsOwner())
	require.True(t, se.IsInternalClosed())
	require.Equal(t, 0, len(p.pool))
	sctx.On("Close").Once()
	WithSuppressAssert(exit)
	sctx.AssertExpectations(t)

	// Put a Session that avoids reusing
	se = getNewSessionFromPool(sctx)
	require.Equal(t, 0, len(p.pool))
	se.internal.avoidReuse = true
	sctx.On("Close").Once()
	p.Put(se)
	require.True(t, se.internal.IsClosed())
	require.Equal(t, 0, len(p.pool))
	sctx.AssertExpectations(t)

	// Put a Session that has pending txn
	se = getNewSessionFromPool(sctx)
	sctx.On("GetPreparedTxnFuture").Return(&mockPreparedFuture{}).Once()
	sctx.On("Close").Once()
	WithSuppressAssert(func() {
		p.Put(se)
	})
	require.True(t, se.internal.IsClosed())
	require.Equal(t, 0, len(p.pool))
	sctx.AssertExpectations(t)

	// Put a Session but `CheckPendingTxn` panics
	se = getNewSessionFromPool(sctx)
	sctx.On("GetPreparedTxnFuture").Panic("txnFuturePanic").Once()
	sctx.On("Close").Once()
	WithSuppressAssert(func() {
		require.PanicsWithValue(t, "txnFuturePanic", func() {
			p.Put(se)
		})
	})
	require.True(t, se.internal.IsClosed())
	require.Equal(t, 0, len(p.pool))
	sctx.AssertExpectations(t)

	// Put a Session but `OwnerResetState` panics
	se = getNewSessionFromPool(sctx)
	sctx.MockNoPendingTxn()
	sctx.MockResetState(p.ctx, "resetStatePanic")
	sctx.On("Close").Once()
	WithSuppressAssert(func() {
		require.PanicsWithValue(t, "resetStatePanic", func() {
			p.Put(se)
		})
	})
	require.True(t, se.internal.IsClosed())
	require.Equal(t, 0, len(p.pool))
	sctx.AssertExpectations(t)

	// Put a closed session
	se = getNewSessionFromPool(sctx)
	require.Equal(t, 0, len(p.pool))
	require.False(t, se.internal.IsClosed())
	sctx.On("Close").Once()
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
	se.Close()
	require.True(t, se.internal.IsClosed())
	p.Put(se)
	require.Equal(t, 0, len(p.pool))
	sctx.AssertExpectations(t)

	// put a full pool
	sessions := make([]*Session, poolCap+2)
	for i := 0; i <= poolCap+1; i++ {
		sctx = &mockSessionContext{}
		se = getNewSessionFromPool(sctx)
		sessions[i] = se
	}

	for i := range poolCap {
		require.Equal(t, i, len(p.pool))
		sctx = sessions[i].internal.sctx.(*mockSessionContext)
		sctx.MockNoPendingTxn()
		sctx.MockResetState(p.ctx, "")
		p.Put(sessions[i])
		require.Equal(t, i+1, len(p.pool))
		require.Same(t, p, sessions[i].internal.Owner())
		sctx.AssertExpectations(t)
	}

	se = sessions[poolCap]
	sctx = se.internal.sctx.(*mockSessionContext)
	sctx.MockNoPendingTxn()
	sctx.MockResetState(p.ctx, "")
	sctx.On("Close").Once()
	p.Put(se)
	require.Equal(t, poolCap, len(p.pool))
	require.Nil(t, se.internal.Owner())
	require.True(t, se.internal.IsClosed())
	sctx.AssertExpectations(t)

	// put a closed pool
	for i := range poolCap {
		sctx = sessions[i].internal.sctx.(*mockSessionContext)
		sctx.On("Close").Once()
	}
	p.Close()
	require.True(t, p.IsClosed())
	require.Equal(t, 0, len(p.pool))
	for i := range poolCap {
		sctx = sessions[i].internal.sctx.(*mockSessionContext)
		sctx.AssertExpectations(t)
	}

	se = sessions[poolCap+1]
	sctx = se.internal.sctx.(*mockSessionContext)
	sctx.MockNoPendingTxn()
	sctx.MockResetState(p.ctx, "")
	sctx.On("Close").Once()
	p.Put(se)
	require.Nil(t, se.internal.Owner())
	require.True(t, se.internal.IsClosed())
	sctx.AssertExpectations(t)
}

// TestSessionPoolWithSession 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestSessionPoolWithSession(t *testing.T) {
#[test]
pub fn TestSessionPoolWithSession() {
	factory := &mockSessionFactory{}
	capacity := 8
	sctx := &mockSessionContext{}
	p := NewAdvancedSessionPool(capacity, factory.create)

	// Go 这里依赖 goroutine/channel/时间等待或原子状态；仅保留并发同步意图。
	var called atomic.Bool
	fn := func(err error, panicS string) func(*Session) error {
		return func(se *Session) error {
			factory.AssertExpectations(t)
			sctx.AssertExpectations(t)
			require.Zero(t, len(p.pool))
			require.True(t, called.CompareAndSwap(false, true))
			if panicS != "" {
				sctx.On("Close").Once()
				panic(panicS)
			}

			if err != nil {
				sctx.On("Close").Once()
				return err
			}

	// 这里跨越事务、KV 或元数据访问边界；只保留 Go 调用顺序与错误检查语义。
			sctx.MockNoPendingTxn()
			sctx.MockResetState(p.ctx, "")
			return nil
		}
	}

	// success case
	require.Zero(t, len(p.pool))
	factory.On("create").Return(sctx, nil).Once()
	err := p.WithSession(fn(nil, ""))
	require.Nil(t, err)
	require.True(t, called.CompareAndSwap(true, false))
	sctx.AssertExpectations(t)
	require.Equal(t, 1, len(p.pool))

	// error case
	err = p.WithSession(fn(errors.New("mockErr1"), ""))
	require.EqualError(t, err, "mockErr1")
	require.True(t, called.CompareAndSwap(true, false))
	sctx.AssertExpectations(t)
	require.Zero(t, len(p.pool))

	// panic case
	factory.On("create").Return(sctx, nil).Once()
	require.PanicsWithValue(t, "mockPanic1", func() {
		_ = p.WithSession(fn(nil, "mockPanic1"))
	})
	require.True(t, called.CompareAndSwap(true, false))
	sctx.AssertExpectations(t)
	require.Zero(t, len(p.pool))

	// p.Get returns error, the function should not be called
	factory.On("create").Return(nil, errors.New("mockErr2")).Once()
	err = p.WithSession(func(*Session) error {
		require.FailNow(t, "should not be called")
		return nil
	})
	require.EqualError(t, err, "mockErr2")
	factory.AssertExpectations(t)
	require.Zero(t, len(p.pool))
}

// TestSessionPoolClose 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestSessionPoolClose(t *testing.T) {
#[test]
pub fn TestSessionPoolClose() {
	factory := &mockSessionFactory{}
	capacity := 8
	p := NewAdvancedSessionPool(capacity, factory.create)

	// make a pool with some sessions
	sctxs := make([]*mockSessionContext, capacity)
	ses := make([]*Session, capacity)
	for i := range capacity {
		sctx := &mockSessionContext{}
		sctxs[i] = sctx
		factory.On("create").Return(sctx, nil).Once()
		se, err := p.Get()
		require.NoError(t, err)
		ses[i] = se
		factory.AssertExpectations(t)
		sctx.AssertExpectations(t)
	}
	for i := range capacity {
		sctx := sctxs[i]
		se := ses[i]
	// 这里跨越事务、KV 或元数据访问边界；只保留 Go 调用顺序与错误检查语义。
		sctx.MockNoPendingTxn()
		sctx.MockResetState(p.ctx, "")
		p.Put(se)
		sctx.AssertExpectations(t)
	}

	// close pool should close all sessions in it
	for i := range capacity {
		sctx := sctxs[i]
		sctx.On("Close").Once()
	}
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
	p.Close()
	require.True(t, p.IsClosed())
	require.Error(t, p.ctx.Err())
	require.Equal(t, 0, len(p.pool))
	select {
	case _, ok := <-p.pool:
		require.False(t, ok)
	default:
		require.FailNow(t, "pool is still active")
	}
	for i := range capacity {
		sctx := sctxs[i]
		sctx.AssertExpectations(t)
	}

	// close a closed pool should take no effect
	p.Close()
	require.True(t, p.IsClosed())
}
"########################################;

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

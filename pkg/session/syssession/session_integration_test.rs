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
//
// 前半段保留 Go 集成测试草稿；后半段覆盖空会话错误与 ForceBlockGC 注册路径。

// Domain 高级系统 session pool 与内部 session 注册/脏 session 回收集成测试。
// testing、require、testkit、session、failpoint、goroutine/channel、KV/DDL/bootstrap 等依赖均按 Go 调用形状保留。

/// 保留的 Go 版 session_integration_test 草稿，对照 Domain 池与脏会话语义。
const GO_SESSION_INTEGRATION_TEST_DRAFT: &str = r########################################"

// TestDomainAdvancedSessionPoolInternalSessionRegistry 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestDomainAdvancedSessionPoolInternalSessionRegistry(t *testing.T) {
#[test]
pub fn TestDomainAdvancedSessionPoolInternalSessionRegistry() {
	// Session/bootstrap harness call site.
	_, do := testkit.CreateMockStoreAndDomain(t)
	p := do.AdvancedSysSessionPool()
	require.NotNil(t, p)

	sessManager := do.InfoSyncer().GetSessionManager()

	// test session manager registry when put back
	// We test for more than one times to cover the case that the session is in the pool.
	var sctx sessionctx.Context
	var se *syssession.Session
	for range 2 {
		sctx = nil
		se = nil
		require.NoError(t, p.WithSession(func(session *syssession.Session) error {
			require.Nil(t, se)
			se = session
			require.True(t, session.IsOwner())
			return session.WithSessionContext(func(ctx sessionctx.Context) error {
				require.Nil(t, sctx)
				sctx = ctx
				require.True(t, sessManager.ContainsInternalSession(ctx))
				return nil
			})
		}))
		require.NotNil(t, se)
		require.False(t, se.IsInternalClosed())
		require.False(t, se.IsOwner())
		require.NotNil(t, sctx)
		require.False(t, sessManager.ContainsInternalSession(sctx))
	}

	// test session manager registry when close session
	sctx = nil
	se, err := p.Get()
	require.NoError(t, err)
	require.NoError(t, se.WithSessionContext(func(ctx sessionctx.Context) error {
		sctx = ctx
		return nil
	}))
	require.NotNil(t, sctx)
	require.True(t, sessManager.ContainsInternalSession(sctx))
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
	se.Close()
	require.False(t, sessManager.ContainsInternalSession(sctx))
}

// TestDomainAdvancedSessionPoolPutBackDirtySession 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestDomainAdvancedSessionPoolPutBackDirtySession(t *testing.T) {
#[test]
pub fn TestDomainAdvancedSessionPoolPutBackDirtySession() {
	// Go failpoint 用于改写运行时分支；只记录注入点和期望错误路径。
	testfailpoint.Enable(t, "github.com/pingcap/tidb/pkg/statistics/handle/SkipSystemTableCheck", `return(true)`)
	// Session/bootstrap harness call site.
	store, do := testkit.CreateMockStoreAndDomain(t)
	p := do.AdvancedSysSessionPool()
	require.NotNil(t, p)

	tk := testkit.NewTestKit(t, store)
	tk.MustExec("use test")
	tk.MustExec("create table t1(a int)")
	tk.MustExec("insert into t1 values(1), (2), (3), (4), (5)")

	// 这里跨越事务、KV 或元数据访问边界；只保留 Go 调用顺序与错误检查语义。
	ctx := kv.WithInternalSourceType(context.Background(), kv.InternalTxnOthers)
	cases := []struct {
		name               string
		withSession        func(*syssession.Session) error
		withSessionContext func(sessionctx.Context) error
	}{
		{
			name: "put back closed one",
			withSession: func(session *syssession.Session) error {
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
				session.Close()
				return nil
			},
		},
		{
			name: "return error for withSession",
			withSession: func(session *syssession.Session) error {
				return errors.New("err1")
			},
		},
		{
			name: "return error for withSessionContext",
			withSessionContext: func(sctx sessionctx.Context) error {
				return errors.New("err2")
			},
		},
		{
			name: "resultSetNotClose",
			withSession: func(session *syssession.Session) error {
				_, err := session.ExecuteInternal(ctx, "select * from test.t1")
				require.NoError(t, err)
				return nil
			},
		},
		{
			name: "optimisticTxnNotClose",
			withSession: func(session *syssession.Session) error {
				_, err := session.ExecuteInternal(ctx, "begin optimistic")
				require.NoError(t, err)
				return nil
			},
		},
		{
			name: "pessimisticTxnNotClose",
			withSession: func(session *syssession.Session) error {
				_, err := session.ExecuteInternal(ctx, "begin pessimistic")
				require.NoError(t, err)
				return nil
			},
		},
		{
			name: "tsFuturePrepared",
			withSessionContext: func(sctx sessionctx.Context) error {
				require.NoError(t, sctx.PrepareTSFuture(ctx, sessiontxn.ConstantFuture(1), kv.GlobalTxnScope))
				return nil
			},
		},
		{
			name: "avoid reuse in withSession",
			withSession: func(session *syssession.Session) error {
				session.AvoidReuse()
				return nil
			},
		},
		{
			name: "avoid reuse in withSessionContext",
			withSession: func(session *syssession.Session) error {
				return session.WithSessionContext(func(sessionctx.Context) error {
					session.AvoidReuse()
					return nil
				})
			},
		},
	}

	for _, c := range cases {
		t.Run(c.name, func(t *testing.T) {
			var se *syssession.Session
			var expectedErr error
			syssession.WithSuppressAssert(func() {
				err := p.WithSession(func(session *syssession.Session) error {
					require.Nil(t, se)
					se = session
					require.True(t, session.IsOwner())
					require.False(t, se.IsInternalClosed())
					if c.withSession != nil {
						expectedErr = c.withSession(session)
						return expectedErr
					}

					if c.withSessionContext != nil {
						err := session.WithSessionContext(func(sessionctx sessionctx.Context) error {
							expectedErr = c.withSessionContext(sessionctx)
							return expectedErr
						})
						if expectedErr != nil {
							require.EqualError(t, err, expectedErr.Error())
						} else {
							require.NoError(t, err)
						}
						return err
					}

					return nil
				})

				if expectedErr != nil {
					require.EqualError(t, err, expectedErr.Error())
				} else {
					require.NoError(t, err)
				}
			})
			require.NotNil(t, se)
			require.True(t, se.IsInternalClosed())
			require.False(t, se.IsOwner())
			require.Zero(t, p.(*syssession.AdvancedSessionPool).Size())
		})
	}

	t.Run("success case", func(t *testing.T) {
		var se *syssession.Session
		require.NoError(t, p.WithSession(func(s *syssession.Session) error {
			se = s
			return s.WithSessionContext(func(sessionctx.Context) error { return nil })
		}))
		require.NotNil(t, se)
		require.False(t, se.IsInternalClosed())
		require.False(t, se.IsOwner())
		require.Equal(t, 1, p.(*syssession.AdvancedSessionPool).Size())
	})

	t.Run("put back a put back case", func(t *testing.T) {
		var se *syssession.Session
		require.NoError(t, p.WithSession(func(s *syssession.Session) error {
			se = s
			p.Put(s)
			return nil
		}))
		require.NotNil(t, se)
		require.False(t, se.IsInternalClosed())
		require.False(t, se.IsOwner())
		require.Equal(t, 1, p.(*syssession.AdvancedSessionPool).Size())
	})
}
"########################################;

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

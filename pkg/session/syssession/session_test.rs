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
//
// 前半段保留 Go session_test 草稿；后半段验证 SessionError 与委托执行/关闭路径。

// 内部 session、owner 转移、代理方法和线程安全边界测试。
// testing、require、testkit、session、failpoint、goroutine/channel、KV/DDL/bootstrap 等依赖均按 Go 调用形状保留。

/// 保留的 Go 版 session_test 草稿，对照 Owner/代理/竞态检测语义。
const GO_SESSION_TEST_DRAFT: &str = r########################################"

// WithSuppressAssert suppress asserts in test
// WithSuppressAssert 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func WithSuppressAssert(fn func()) {
pub fn WithSuppressAssert() {
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
	defer func() {
		suppressAssertInTest = false
	}()
	suppressAssertInTest = true
	fn()
}

// mockOwner 对应 Go 的同名辅助类型，字段和嵌入接口按原测试 mock 语义保留。
// Go 类型声明: type mockOwner struct {
pub struct mockOwner {
	sessionOwner
	mock.Mock
}

// onBecameOwner 对应 Go 方法，接收者 `m *mockOwner`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockOwner) onBecameOwner(sctx sessionctx.Context) error {
pub fn onBecameOwner() {
	return m.Called(sctx).Error(0)
}

// onResignOwner 对应 Go 方法，接收者 `m *mockOwner`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockOwner) onResignOwner(sctx sessionctx.Context) error {
pub fn onResignOwner() {
	return m.Called(sctx).Error(0)
}

// mockTxn 对应 Go 的同名辅助类型，字段和嵌入接口按原测试 mock 语义保留。
// Go 类型声明: type mockTxn struct {
pub struct mockTxn {
	mock.Mock
	// 这里跨越事务、KV 或元数据访问边界；只保留 Go 调用顺序与错误检查语义。
	kv.Transaction
	valid bool
}

// Valid 对应 Go 方法，接收者 `txn *mockTxn`；保留 mock/session 接口调用语义。
// Go 签名: func (txn *mockTxn) Valid() bool {
pub fn Valid() {
	return txn.valid
}

// String 对应 Go 方法，接收者 `txn *mockTxn`；保留 mock/session 接口调用语义。
// Go 签名: func (txn *mockTxn) String() string {
pub fn String() {
	return txn.Transaction.String()
}

// mockPreparedFuture 对应 Go 的同名辅助类型，字段和嵌入接口按原测试 mock 语义保留。
// Go 类型声明: type mockPreparedFuture struct {
pub struct mockPreparedFuture {
	sessionctx.TxnFuture
}

// mockSessionContext 对应 Go 的同名辅助类型，字段和嵌入接口按原测试 mock 语义保留。
// Go 类型声明: type mockSessionContext struct {
pub struct mockSessionContext {
	sessionctx.Context
	sessmgr.Manager
	mock.Mock
}

// Close 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) Close() {
pub fn Close() {
	m.Called()
}

// RollbackTxn 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) RollbackTxn(ctx context.Context) {
pub fn RollbackTxn() {
	m.Called(ctx)
}

// GetSQLExecutor 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) GetSQLExecutor() sqlexec.SQLExecutor {
pub fn GetSQLExecutor() {
	return m
}

// Execute 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) Execute(ctx context.Context, sql string) (rs []sqlexec.RecordSet, err error) {
pub fn Execute() {
	args := m.Called(ctx, sql)
	if arg := args.Get(0); arg != nil {
		rs = arg.([]sqlexec.RecordSet)
	}
	err = args.Error(1)
	return
}

// ExecuteInternal 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) ExecuteInternal(ctx context.Context, sql string, sqlArgs ...any) (rs sqlexec.RecordSet, err error) {
pub fn ExecuteInternal() {
	args := m.Called(ctx, sql, sqlArgs)
	if arg := args.Get(0); arg != nil {
		rs = arg.(sqlexec.RecordSet)
	}
	err = args.Error(1)
	return
}

// ExecuteStmt 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) ExecuteStmt(ctx context.Context, stmtNode ast.StmtNode) (rs sqlexec.RecordSet, err error) {
pub fn ExecuteStmt() {
	args := m.Called(ctx, stmtNode)
	if arg := args.Get(0); arg != nil {
		rs = arg.(sqlexec.RecordSet)
	}
	err = args.Error(1)
	return
}

// ParseWithParams 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) ParseWithParams(ctx context.Context, sql string, sqlArgs ...any) (n ast.StmtNode, err error) {
pub fn ParseWithParams() {
	args := m.Called(ctx, sql, sqlArgs)
	if arg := args.Get(0); arg != nil {
		n = arg.(ast.StmtNode)
	}
	err = args.Error(1)
	return
}

// ExecRestrictedStmt 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) ExecRestrictedStmt(ctx context.Context, stmt ast.StmtNode, opts ...sqlexec.OptionFuncAlias) (rows []chunk.Row, fields []*resolve.ResultField, err error) {
pub fn ExecRestrictedStmt() {
	args := m.Called(ctx, stmt, opts)
	if arg := args.Get(0); arg != nil {
		rows = arg.([]chunk.Row)
	}
	if arg := args.Get(1); arg != nil {
		fields = arg.([]*resolve.ResultField)
	}
	err = args.Error(2)
	return
}

// ExecRestrictedSQL 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) ExecRestrictedSQL(ctx context.Context, opts []sqlexec.OptionFuncAlias, sql string, sqlArgs ...any) (rows []chunk.Row, fields []*resolve.ResultField, err error) {
pub fn ExecRestrictedSQL() {
	args := m.Called(ctx, opts, sql, sqlArgs)
	if arg := args.Get(0); arg != nil {
		rows = arg.([]chunk.Row)
	}
	if arg := args.Get(1); arg != nil {
		fields = arg.([]*resolve.ResultField)
	}
	err = args.Error(2)
	return
}

// GetRestrictedSQLExecutor 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) GetRestrictedSQLExecutor() sqlexec.RestrictedSQLExecutor {
pub fn GetRestrictedSQLExecutor() {
	return m
}

// GetPreparedTxnFuture 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) GetPreparedTxnFuture() sessionctx.TxnFuture {
pub fn GetPreparedTxnFuture() {
	if arg := m.Called().Get(0); arg != nil {
		return arg.(sessionctx.TxnFuture)
	}
	return nil
}

// Txn 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) Txn(active bool) (txn kv.Transaction, err error) {
pub fn Txn() {
	args := m.Called(active)
	err = args.Error(1)
	if args.Get(0) != nil {
	// 这里跨越事务、KV 或元数据访问边界；只保留 Go 调用顺序与错误检查语义。
		txn = args.Get(0).(kv.Transaction)
	}
	return
}

// MockNoPendingTxn 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) MockNoPendingTxn() {
pub fn MockNoPendingTxn() {
	m.On("Txn", false).Return(&mockTxn{valid: false}, nil).Once()
	m.On("GetPreparedTxnFuture").Return(nil).Once()
}

// MockResetState 对应 Go 方法，接收者 `m *mockSessionContext`；保留 mock/session 接口调用语义。
// Go 签名: func (m *mockSessionContext) MockResetState(ctx context.Context, panicStr string) {
pub fn MockResetState() {
	// 这里跨越事务、KV 或元数据访问边界；只保留 Go 调用顺序与错误检查语义。
	m.On("RollbackTxn", ctx).Run(func(args mock.Arguments) {
		if panicStr != "" {
			panic(panicStr)
		}
	}).Once()
}

// TestNewInternalSession 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestNewInternalSession(t *testing.T) {
#[test]
pub fn TestNewInternalSession() {
	sctx := &mockSessionContext{}
	owner := &mockOwner{}

	// newInternalSession success case
	owner.On("onBecameOwner", sctx).Return(nil).Once()
	se, err := newInternalSession(sctx, owner)
	require.NoError(t, err)
	require.NotNil(t, se)
	require.Same(t, owner, se.owner)
	require.Same(t, sctx, se.sctx)
	require.Zero(t, se.inUse)
	require.False(t, se.avoidReuse)
	owner.AssertExpectations(t)

	// onBecameOwner returns an error
	mockErr := errors.New("mockOnBecameOwnerErr")
	owner.On("onBecameOwner", sctx).Return(mockErr).Once()
	se, err = newInternalSession(sctx, owner)
	require.EqualError(t, err, mockErr.Error())
	require.Nil(t, se)
	owner.AssertExpectations(t)
}

// mockInternalSession 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func mockInternalSession(t *testing.T, sctx *mockSessionContext, owner *mockOwner) *session {
pub fn mockInternalSession() {
	owner.On("onBecameOwner", sctx).Return(nil).Once()
	se, err := newInternalSession(sctx, owner)
	require.NoError(t, err)
	require.Same(t, owner, se.Owner())
	require.False(t, se.IsClosed())
	owner.AssertExpectations(t)
	return se
}

// mockSession 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func mockSession(t *testing.T, sctx *mockSessionContext) *Session {
pub fn mockSession() {
	se, err := NewSessionForTest(sctx)
	require.NoError(t, err)
	require.Same(t, se, se.internal.Owner())
	require.False(t, se.internal.IsClosed())
	sctx.AssertExpectations(t)
	return se
}

// TestResignOwnerAndCloseSctx 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestResignOwnerAndCloseSctx(t *testing.T) {
#[test]
pub fn TestResignOwnerAndCloseSctx() {
	sctx := &mockSessionContext{}
	owner := &mockOwner{}

	// success case
	owner.On("onResignOwner", sctx).Return(nil).Once()
	sctx.On("Close").Once()
	require.NoError(t, resignOwnerAndCloseSctx(owner, sctx))
	owner.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// onResignOwner returns an error, should also close the session
	owner.On("onResignOwner", sctx).Return(errors.New("mockErr")).Once()
	sctx.On("Close").Once()
	require.EqualError(t, resignOwnerAndCloseSctx(owner, sctx), "mockErr")
	owner.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// onResignOwner panics, should also close the session
	owner.On("onResignOwner", sctx).Panic("mockPanic").Once()
	sctx.On("Close").Once()
	require.PanicsWithValue(t, "mockPanic", func() {
		_ = resignOwnerAndCloseSctx(owner, sctx)
	})
	owner.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// Close panics
	owner.On("onResignOwner", sctx).Return(nil).Once()
	sctx.On("Close").Panic("mockClosePanic")
	require.PanicsWithValue(t, "mockClosePanic", func() {
		_ = resignOwnerAndCloseSctx(owner, sctx)
	})
	owner.AssertExpectations(t)
	sctx.AssertExpectations(t)
}

// TestInternalSessionTransferOwner 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestInternalSessionTransferOwner(t *testing.T) {
#[test]
pub fn TestInternalSessionTransferOwner() {
	sctx := &mockSessionContext{}
	// TransferOwner success
	owner1 := &mockOwner{}
	se := mockInternalSession(t, sctx, owner1)
	owner2 := &mockOwner{}
	owner1.On("onResignOwner", sctx).Return(nil).Once()
	owner2.On("onBecameOwner", sctx).Return(nil).Once()
	require.NoError(t, se.TransferOwner(owner1, owner2))
	require.Same(t, owner2, se.Owner())
	owner1.AssertExpectations(t)
	owner2.AssertExpectations(t)

	owner3 := &mockOwner{}
	// transfer from an invalid owner should fail
	require.Error(t, se.TransferOwner(owner1, owner3))
	require.Same(t, owner2, se.Owner())
	require.Error(t, se.TransferOwner(owner1, owner2))
	require.Same(t, owner2, se.Owner())

	// transfer to the same owner should take no effect
	require.NoError(t, se.TransferOwner(owner2, owner2))
	require.Same(t, owner2, se.Owner())

	// TransferOwner from=nil should fail
	require.Error(t, se.TransferOwner(nil, owner3))
	require.Same(t, owner2, se.Owner())

	// TransferOwner to=nil should fail
	require.Error(t, se.TransferOwner(owner2, nil))
	require.Same(t, owner2, se.Owner())

	// TransferOwner from=to=nil should fail
	require.Error(t, se.TransferOwner(nil, nil))
	require.Same(t, owner2, se.Owner())
	require.False(t, se.IsClosed())

	// onResignOwner returns an error and should close the session
	mockErr := errors.New("mockOnResignOwnerErr")
	owner2.On("onResignOwner", sctx).Return(mockErr).Once()
	sctx.On("Close").Once()
	require.EqualError(t, se.TransferOwner(owner2, owner1), mockErr.Error())
	require.Nil(t, se.Owner())
	require.True(t, se.IsClosed())
	owner2.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// onResignOwner panics should close the session
	se = mockInternalSession(t, sctx, owner2)
	owner2.On("onResignOwner", sctx).Panic("mock panic1").Once()
	sctx.On("Close").Once()
	require.PanicsWithValue(t, "mock panic1", func() {
		_ = se.TransferOwner(owner2, owner3)
	})
	require.Nil(t, se.Owner())
	require.True(t, se.IsClosed())
	owner2.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// onBecameOwner returns an error and should close the session
	se = mockInternalSession(t, sctx, owner2)
	mockErr = errors.New("mockOnBecameOwner")
	owner2.On("onResignOwner", sctx).Return(nil).Once()
	owner3.On("onBecameOwner", sctx).Return(mockErr).Once()
	sctx.On("Close").Once()
	require.EqualError(t, se.TransferOwner(owner2, owner3), mockErr.Error())
	require.Nil(t, se.Owner())
	require.True(t, se.IsClosed())
	owner2.AssertExpectations(t)
	owner3.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// onBecameOwner panics should close the session
	se = mockInternalSession(t, sctx, owner2)
	mockErr = errors.New("mockOnBecameOwner")
	owner2.On("onResignOwner", sctx).Return(nil).Once()
	owner3.On("onBecameOwner", sctx).Panic("mock panic2").Once()
	sctx.On("Close").Once()
	require.PanicsWithValue(t, "mock panic2", func() {
		_ = se.TransferOwner(owner2, owner3)
	})
	require.Nil(t, se.Owner())
	require.True(t, se.IsClosed())
	owner2.AssertExpectations(t)
	owner3.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// close panics
	se = mockInternalSession(t, sctx, owner2)
	owner2.On("onResignOwner", sctx).Return(nil).Once()
	owner3.On("onBecameOwner", sctx).Return(mockErr).Once()
	sctx.On("Close").Panic("close panic").Once()
	require.PanicsWithValue(t, "close panic", func() {
		_ = se.TransferOwner(owner2, owner3)
	})
	require.Nil(t, se.Owner())
	require.True(t, se.IsClosed())
	owner2.AssertExpectations(t)
	owner3.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// A closed session should not transfer the owner again
	require.Error(t, se.TransferOwner(nil, owner1))
	require.Nil(t, se.Owner())
	require.True(t, se.IsClosed())
	require.Error(t, se.TransferOwner(owner2, owner1))
	require.Nil(t, se.Owner())
	require.True(t, se.IsClosed())
	require.Error(t, se.TransferOwner(nil, nil))
	require.Nil(t, se.Owner())
	require.True(t, se.IsClosed())

	// inUse session should not transfer the owner
	se = mockInternalSession(t, sctx, owner1)
	_, exit, err := se.EnterOperation(owner1, false)
	require.NoError(t, err)
	require.Equal(t, uint64(1), se.Inuse())
	err = se.TransferOwner(owner1, owner2)
	require.Error(t, err)
	require.Contains(t, err.Error(), "session is still inUse: 1")
	require.Same(t, owner1, se.Owner())
	require.False(t, se.IsClosed())

	// after exit, the session can be transferred again
	exit()
	owner1.On("onResignOwner", sctx).Return(nil).Once()
	owner2.On("onBecameOwner", sctx).Return(nil).Once()
	require.NoError(t, se.TransferOwner(owner1, owner2))
	require.Same(t, owner2, se.Owner())
	require.False(t, se.IsClosed())
	owner1.AssertExpectations(t)
	owner2.AssertExpectations(t)
}

// TestInternalSessionClose 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestInternalSessionClose(t *testing.T) {
#[test]
pub fn TestInternalSessionClose() {
	sctx := &mockSessionContext{}
	owner := &mockOwner{}
	se := mockInternalSession(t, sctx, owner)
	require.False(t, se.IsClosed())
	require.Same(t, owner, se.Owner())

	// Close with a nil owner
	se.OwnerClose(nil)
	require.False(t, se.IsClosed())
	require.Same(t, owner, se.Owner())

	// Close with an invalid owner
	owner2 := &mockOwner{}
	se.OwnerClose(owner2)
	require.False(t, se.IsClosed())
	require.Same(t, owner, se.Owner())

	// Close with the current owner
	owner.On("onResignOwner", sctx).Return(nil).Once()
	sctx.On("Close").Once()
	se.OwnerClose(owner)
	require.True(t, se.IsClosed())
	require.Nil(t, se.Owner())
	owner.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// Close a closed session
	se.OwnerClose(owner)
	se.OwnerClose(nil)
	se.OwnerClose(owner2)
	require.True(t, se.IsClosed())
	require.Nil(t, se.Owner())

	// Close without an owner
	se = mockInternalSession(t, sctx, owner)
	owner.On("onResignOwner", sctx).Return(nil).Once()
	sctx.On("Close").Once()
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
	se.Close()
	require.Nil(t, se.Owner())
	require.True(t, se.IsClosed())

	// Close after close
	se.Close()
	require.Nil(t, se.Owner())
	require.True(t, se.IsClosed())

	// test with error when close
	testWithErrorMock := func(closeFn func(se *session, owner *mockOwner)) {
		// onResignOwner failed should also close the context
		se = mockInternalSession(t, sctx, owner)
		owner.On("onResignOwner", sctx).Return(errors.New("mockErr1")).Once()
		sctx.On("Close").Once()
		WithSuppressAssert(func() {
			closeFn(se, owner)
		})
		require.True(t, se.IsClosed())
		require.Nil(t, se.Owner())
		owner.AssertExpectations(t)
		sctx.AssertExpectations(t)

		// onResignOwner panics should also close the context
		se = mockInternalSession(t, sctx, owner)
		owner.On("onResignOwner", sctx).Panic("panic1").Once()
		sctx.On("Close").Once()
		require.PanicsWithValue(t, "panic1", func() {
			closeFn(se, owner)
		})
		require.True(t, se.IsClosed())
		require.Nil(t, se.Owner())
		owner.AssertExpectations(t)
		sctx.AssertExpectations(t)

		// context.Close() panics
		se = mockInternalSession(t, sctx, owner)
		owner.On("onResignOwner", sctx).Return(nil).Once()
		sctx.On("Close").Panic("panic2").Once()
		require.PanicsWithValue(t, "panic2", func() {
			closeFn(se, owner)
		})
		require.True(t, se.IsClosed())
		require.Nil(t, se.Owner())
		owner.AssertExpectations(t)
		sctx.AssertExpectations(t)

		// inUse > 0 when close
		se = mockInternalSession(t, sctx, owner)
		_, exit1, err := se.EnterOperation(owner, false)
		require.NoError(t, err)
		require.Equal(t, uint64(1), se.Inuse())
		_, exit2, err := se.EnterOperation(owner, true)
		require.NoError(t, err)
		require.Equal(t, uint64(2), se.Inuse())
		_, exit3, err := se.EnterOperation(owner, true)
		require.NoError(t, err)
		require.Equal(t, uint64(3), se.Inuse())
		// should not call `sctx.Close()` when inuse > 0 to avoid some data race
		WithSuppressAssert(func() {
			closeFn(se, owner)
		})
		require.True(t, se.IsClosed())
		require.Nil(t, se.Owner())
		// sctx.Close should not be called when inuse > 0 after exit
		require.Equal(t, uint64(3), se.Inuse())
		WithSuppressAssert(exit1)
		require.Equal(t, uint64(2), se.Inuse())
		WithSuppressAssert(exit3)
		require.Equal(t, uint64(1), se.Inuse())
		// sctx.Close should be called after inuse decreased to 0
		owner.On("onResignOwner", sctx).Return(nil).Once()
		sctx.On("Close").Once()
		WithSuppressAssert(exit2)
		require.Zero(t, se.Inuse())
		sctx.AssertExpectations(t)
		owner.AssertExpectations(t)
	}

	testWithErrorMock(func(se *session, owner *mockOwner) {
		se.OwnerClose(owner)
	})

	testWithErrorMock(func(_ *session, _ *mockOwner) {
		se.Close()
	})
}

// TestInternalSessionEnterOperation 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestInternalSessionEnterOperation(t *testing.T) {
#[test]
pub fn TestInternalSessionEnterOperation() {
	sctx := &mockSessionContext{}
	owner := &mockOwner{}
	se := mockInternalSession(t, sctx, owner)

	// test EnterOperation will add inUse
	gotSctx, exit1, err := se.EnterOperation(owner, true)
	require.NoError(t, err)
	require.Same(t, sctx, gotSctx)
	require.Equal(t, uint64(1), se.Inuse())

	gotSctx, exit2, err := se.EnterOperation(owner, false)
	require.NoError(t, err)
	require.Same(t, sctx, gotSctx)
	require.Equal(t, uint64(2), se.Inuse())

	gotSctx, exit3, err := se.EnterOperation(owner, true)
	require.NoError(t, err)
	require.Same(t, sctx, gotSctx)
	require.Equal(t, uint64(3), se.Inuse())

	// test exit will decrease inUse
	exit1()
	require.Equal(t, uint64(2), se.Inuse())

	exit3()
	require.Equal(t, uint64(1), se.Inuse())

	WithSuppressAssert(func() {
		// multiple exit should take no effect
		exit1()
		require.Equal(t, uint64(1), se.Inuse())
	})

	require.Equal(t, uint64(1), se.Inuse())
	exit2()
	require.Equal(t, uint64(0), se.Inuse())

	// call with an invalid owner should report an error
	WithSuppressAssert(func() {
		gotSctx, exit, err := se.EnterOperation(&mockOwner{}, false)
		require.Error(t, err)
		require.Contains(t, err.Error(), "caller is not the owner")
		require.Nil(t, gotSctx)
		require.Nil(t, exit)
		require.Equal(t, uint64(0), se.Inuse())
	})

	// call with a nil owner should report an error
	WithSuppressAssert(func() {
		gotSctx, exit, err := se.EnterOperation(nil, false)
		require.Error(t, err)
		require.Contains(t, err.Error(), "caller is not the owner")
		require.Nil(t, gotSctx)
		require.Nil(t, exit)
		require.Equal(t, uint64(0), se.Inuse())
	})

	// call in a closed session should report an error
	owner.On("onResignOwner", sctx).Return(nil).Once()
	sctx.On("Close").Once()
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
	se.Close()
	WithSuppressAssert(func() {
		gotSctx, exit, err := se.EnterOperation(owner, false)
		require.Error(t, err)
		require.Contains(t, err.Error(), "session is closed")
		require.Nil(t, gotSctx)
		require.Nil(t, exit)
		require.Equal(t, uint64(0), se.Inuse())
	})
	owner.AssertExpectations(t)
	sctx.AssertExpectations(t)

	// close the session after enter operation
	se = mockInternalSession(t, sctx, owner)
	gotSctx, exit4, err := se.EnterOperation(owner, true)
	require.NoError(t, err)
	require.Same(t, sctx, gotSctx)
	require.Equal(t, uint64(1), se.Inuse())

	gotSctx, exit5, err := se.EnterOperation(owner, true)
	require.NoError(t, err)
	require.Same(t, sctx, gotSctx)
	require.Equal(t, uint64(2), se.Inuse())

	// Close the session before exit
	WithSuppressAssert(se.Close)
	require.True(t, se.IsClosed())
	require.Equal(t, uint64(2), se.Inuse())
	require.True(t, se.IsClosed())

	// new EnterOperation should be rejected after close but inuse > 0
	WithSuppressAssert(func() {
		_, _, err = se.EnterOperation(owner, false)
		require.Error(t, err)
		require.Contains(t, err.Error(), "session is closed")
	})

	// The exit should still work to decrease inUse
	WithSuppressAssert(exit4)
	require.Equal(t, uint64(1), se.Inuse())
	// when inuse > 0, the session should not call sctx.Close() even if onResignOwner fails
	owner.On("onResignOwner", sctx).Return(errors.New("mockErr")).Once()
	sctx.On("Close").Once()
	WithSuppressAssert(exit5)
	require.Equal(t, uint64(0), se.Inuse())
	owner.AssertExpectations(t)
	sctx.AssertExpectations(t)
}

// TestInternalSessionOwnerWithSctx 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestInternalSessionOwnerWithSctx(t *testing.T) {
#[test]
pub fn TestInternalSessionOwnerWithSctx() {
	sctx := &mockSessionContext{}
	owner := &mockOwner{}
	se := mockInternalSession(t, sctx, owner)
	mockCb := &mock.Mock{}
	cb := func(sctx SessionContext) error {
		require.Equal(t, uint64(1), se.Inuse())
		return mockCb.MethodCalled("cb", sctx).Error(0)
	}

	// normal case
	mockCb.On("cb", sctx).Return(nil).Once()
	require.NoError(t, se.OwnerWithSctx(owner, cb))
	require.Zero(t, se.Inuse())
	mockCb.AssertExpectations(t)

	// invalid owner
	WithSuppressAssert(func() {
		err := se.OwnerWithSctx(&mockOwner{}, cb)
		require.Error(t, err)
		require.Contains(t, err.Error(), "caller is not the owner")
		require.Zero(t, se.Inuse())
	})

	// error in cb
	mockCb.On("cb", sctx).Return(errors.New("mockErr")).Once()
	err := se.OwnerWithSctx(owner, cb)
	require.Error(t, err)
	require.Contains(t, err.Error(), "mockErr")
	require.Zero(t, se.Inuse())
	mockCb.AssertExpectations(t)

	// panic in cb
	mockCb.On("cb", sctx).Panic("mockPanic").Once()
	require.PanicsWithValue(t, "mockPanic", func() {
		_ = se.OwnerWithSctx(owner, cb)
	})
	require.Zero(t, se.Inuse())
	mockCb.AssertExpectations(t)

	// session closed
	sctx.On("Close").Once()
	owner.On("onResignOwner", sctx).Return(nil).Once()
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
	se.Close()
	require.True(t, se.IsClosed())
	sctx.AssertExpectations(t)
	owner.AssertExpectations(t)
	WithSuppressAssert(func() {
		err := se.OwnerWithSctx(owner, cb)
		require.Error(t, err)
		require.Contains(t, err.Error(), "session is closed")
		require.Zero(t, se.Inuse())
	})
}

// TestInternalSessionAvoidReuse 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestInternalSessionAvoidReuse(t *testing.T) {
#[test]
pub fn TestInternalSessionAvoidReuse() {
	sctx := &mockSessionContext{}
	owner := &mockOwner{}
	se := mockInternalSession(t, sctx, owner)
	require.False(t, se.IsAvoidReuse())

	execute := func(do func()) {
		gotSctx, exit, err := se.EnterOperation(owner, false)
		require.NoError(t, err)
		require.Same(t, sctx, gotSctx)
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
		defer exit()
		do()
	}

	// normal use
	execute(func() {})
	require.False(t, se.IsAvoidReuse())
	require.False(t, se.IsClosed())
	require.Same(t, owner, se.Owner())

	// panic use
	require.PanicsWithValue(t, "panic1", func() {
		execute(func() {
			panic("panic1")
		})
	})
	require.True(t, se.IsAvoidReuse())
	require.False(t, se.IsClosed())
	require.Same(t, owner, se.Owner())

	// allow to close
	owner.On("onResignOwner", sctx).Return(nil).Once()
	sctx.On("Close").Once()
	se.OwnerClose(owner)
	require.True(t, se.IsAvoidReuse())
	require.True(t, se.IsClosed())
	require.Nil(t, se.Owner())
	owner.AssertExpectations(t)
	sctx.AssertExpectations(t)
}

// TestInternalSessionCheckNoPendingTxn 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestInternalSessionCheckNoPendingTxn(t *testing.T) {
#[test]
pub fn TestInternalSessionCheckNoPendingTxn() {
	sctx := &mockSessionContext{}
	se := mockInternalSession(t, sctx, &mockOwner{})

	// 这里跨越事务、KV 或元数据访问边界；只保留 Go 调用顺序与错误检查语义。
	sctx.MockNoPendingTxn()
	require.NoError(t, se.CheckNoPendingTxn())
	sctx.AssertExpectations(t)

	sctx.On("GetPreparedTxnFuture").Return(&mockPreparedFuture{}).Once()
	require.EqualError(t, se.CheckNoPendingTxn(), "txn is pending for TSO")
	sctx.AssertExpectations(t)

	sctx.On("GetPreparedTxnFuture").Return(nil).Once()
	sctx.On("Txn", false).Return(nil, errors.New("mockErr")).Once()
	require.EqualError(t, se.CheckNoPendingTxn(), "mockErr")
	sctx.AssertExpectations(t)

	sctx.On("GetPreparedTxnFuture").Return(nil).Once()
	sctx.On("Txn", false).Return(&mockTxn{valid: true}, nil).Once()
	require.EqualError(t, se.CheckNoPendingTxn(), "txn is still valid")
	sctx.AssertExpectations(t)
}

// TestInternalSessionResetState 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestInternalSessionResetState(t *testing.T) {
#[test]
pub fn TestInternalSessionResetState() {
	sctx := &mockSessionContext{}
	owner := &mockOwner{}
	se := mockInternalSession(t, sctx, owner)
	ctx := context.WithValue(context.Background(), "a", "b")
	checkInuse := func(mock.Arguments) { require.Equal(t, uint64(1), se.Inuse()) }

	// normal case
	// 这里跨越事务、KV 或元数据访问边界；只保留 Go 调用顺序与错误检查语义。
	sctx.On("RollbackTxn", ctx).Run(checkInuse).Once()
	require.NoError(t, se.OwnerResetState(ctx, owner))
	require.Zero(t, se.Inuse())
	sctx.AssertExpectations(t)

	// RollbackTxn panic
	sctx.On("RollbackTxn", ctx).Run(checkInuse).Panic("mockPanic1").Once()
	require.PanicsWithValue(t, "mockPanic1", func() {
		_ = se.OwnerResetState(ctx, owner)
	})
	require.Zero(t, se.Inuse())
	sctx.AssertExpectations(t)

	// not owner
	WithSuppressAssert(func() {
		err := se.OwnerResetState(ctx, &mockOwner{})
		require.Error(t, err)
		require.Contains(t, err.Error(), "caller is not the owner")
		require.Zero(t, se.Inuse())
	})

	// closed session
	owner.On("onResignOwner", sctx).Return(nil).Once()
	sctx.On("Close").Once()
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
	se.Close()
	sctx.AssertExpectations(t)
	owner.AssertExpectations(t)
	WithSuppressAssert(func() {
		err := se.OwnerResetState(ctx, owner)
		require.Error(t, err)
		require.Contains(t, err.Error(), "session is closed")
		require.Zero(t, se.Inuse())
	})
}

// testCallProxyMethod 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func testCallProxyMethod(
// Go 签名: t *testing.T,
// Go 签名: sctx *mockSessionContext,
// Go 签名: se *Session,
// Go 签名: name string,
// Go 签名: args []any,
// Go 签名: returns []any,
// Go 签名: callMethod func([]any) []any,
// Go 签名: ) {
pub fn testCallProxyMethod() {
	// normal call
	inuse := se.internal.Inuse()
	require.GreaterOrEqual(t, inuse, uint64(0))
	inCallInuse := uint64(0)
	sctx.On(name, args...).Return(returns...).Run(func(_ mock.Arguments) {
		inCallInuse = se.internal.Inuse()
	}).Once()
	actualRets := callMethod(args)
	require.Equal(t, returns, actualRets)
	require.Equal(t, inuse+1, inCallInuse)
	require.Equal(t, inuse, se.internal.Inuse())
	sctx.AssertExpectations(t)

	// transfer the owner to make call fail
	owner2 := noopOwnerHook{}
	require.NoError(t, se.internal.TransferOwner(se, owner2))
	sctx.AssertExpectations(t)

	// error call
	WithSuppressAssert(func() {
		actualRets = callMethod(args)
	})
	require.Equal(t, inuse, se.internal.Inuse())
	for i := range actualRets {
		if i < len(actualRets)-1 {
			require.Nil(t, actualRets[i], fmt.Sprintf("%d: %v", i, actualRets[i]))
		} else {
			err := actualRets[i].(error)
			require.Contains(t, err.Error(), "caller is not the owner")
		}
	}

	// transfer the owner back
	require.NoError(t, se.internal.TransferOwner(owner2, se))
	sctx.AssertExpectations(t)

	// test panics
	require.False(t, se.internal.IsAvoidReuse())
	sctx.On(name, args...).Run(func(_ mock.Arguments) {
		inCallInuse = se.internal.Inuse()
		panic("panicTest")
	}).Once()
	require.PanicsWithValue(t, "panicTest", func() {
		callMethod(args)
	})
	require.Equal(t, inuse+1, inCallInuse)
	require.Equal(t, inuse, se.internal.Inuse())
	require.True(t, se.internal.IsAvoidReuse())
	se.internal.avoidReuse = false
	sctx.AssertExpectations(t)
}

// testCallProxyMethod22 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func testCallProxyMethod22[A1 any, A2 any, R1 any, R2 any](
// Go 签名: t *testing.T,
// Go 签名: sctx *mockSessionContext,
// Go 签名: se *Session,
// Go 签名: name string,
// Go 签名: arg1 A1, arg2 A2,
// Go 签名: r1 R1, r2 R2,
// Go 签名: callMethod func(arg1 A1, arg2 A2) (R1, R2),
// Go 签名: ) {
pub fn testCallProxyMethod22() {
	testCallProxyMethod(t, sctx, se, name, []any{arg1, arg2}, []any{r1, r2}, func(args []any) []any {
		ret1, ret2 := callMethod(args[0].(A1), args[1].(A2))
		return []any{ret1, ret2}
	})
}

// testCallProxyMethod32 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func testCallProxyMethod32[A1 any, A2 any, A3 any, R1 any, R2 any](
// Go 签名: t *testing.T,
// Go 签名: sctx *mockSessionContext,
// Go 签名: se *Session,
// Go 签名: name string,
// Go 签名: arg1 A1, arg2 A2, arg3 A3,
// Go 签名: r1 R1, r2 R2,
// Go 签名: callMethod func(arg1 A1, arg2 A2, arg3 A3) (R1, R2),
// Go 签名: ) {
pub fn testCallProxyMethod32() {
	testCallProxyMethod(t, sctx, se, name, []any{arg1, arg2, arg3}, []any{r1, r2}, func(args []any) []any {
		ret1, ret2 := callMethod(args[0].(A1), args[1].(A2), args[2].(A3))
		return []any{ret1, ret2}
	})
}

// testCallProxyMethod33 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func testCallProxyMethod33[A1 any, A2 any, A3 any, R1 any, R2 any, R3 any](
// Go 签名: t *testing.T,
// Go 签名: sctx *mockSessionContext,
// Go 签名: se *Session,
// Go 签名: name string,
// Go 签名: arg1 A1, arg2 A2, arg3 A3,
// Go 签名: r1 R1, r2 R2, r3 R3,
// Go 签名: callMethod func(arg1 A1, arg2 A2, arg3 A3) (R1, R2, R3),
// Go 签名: ) {
pub fn testCallProxyMethod33() {
	testCallProxyMethod(t, sctx, se, name, []any{arg1, arg2, arg3}, []any{r1, r2, r3}, func(args []any) []any {
		ret1, ret2, ret3 := callMethod(args[0].(A1), args[1].(A2), args[2].(A3))
		return []any{ret1, ret2, ret3}
	})
}

// testCallProxyMethod43 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func testCallProxyMethod43[A1 any, A2 any, A3 any, A4 any, R1 any, R2 any, R3 any](
// Go 签名: t *testing.T,
// Go 签名: sctx *mockSessionContext,
// Go 签名: se *Session,
// Go 签名: name string,
// Go 签名: arg1 A1, arg2 A2, arg3 A3, arg4 A4,
// Go 签名: r1 R1, r2 R2, r3 R3,
// Go 签名: callMethod func(arg1 A1, arg2 A2, arg3 A3, arg4 A4) (R1, R2, R3),
// Go 签名: ) {
pub fn testCallProxyMethod43() {
	testCallProxyMethod(t, sctx, se, name, []any{arg1, arg2, arg3, arg4}, []any{r1, r2, r3}, func(args []any) []any {
		ret1, ret2, ret3 := callMethod(args[0].(A1), args[1].(A2), args[2].(A3), args[3].(A4))
		return []any{ret1, ret2, ret3}
	})
}

// mockRecordSet 对应 Go 的同名辅助类型，字段和嵌入接口按原测试 mock 语义保留。
// Go 类型声明: type mockRecordSet struct {
pub struct mockRecordSet {
	v int
	sqlexec.RecordSet
}

// TestSessionProxyMethods 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestSessionProxyMethods(t *testing.T) {
#[test]
pub fn TestSessionProxyMethods() {
	sctx := &mockSessionContext{}
	se := mockSession(t, sctx)
	require.Same(t, se, se.GetSQLExecutor())
	require.Same(t, se, se.GetRestrictedSQLExecutor())

	ctx := context.WithValue(context.Background(), "a", "b")

	testCallProxyMethod22(
		t, sctx, se, "Execute",
		ctx, "select 1, 2, 3",
		[]sqlexec.RecordSet{&mockRecordSet{v: 1}, &mockRecordSet{v: 2}}, errors.New("err"),
		se.Execute,
	)

	testCallProxyMethod32(
		t, sctx, se, "ExecuteInternal",
		ctx, "select 1, 2, 3", []any{"a", "c", 2},
		sqlexec.RecordSet(&mockRecordSet{v: 3}), errors.New("err"),
		func(arg1 context.Context, arg2 string, arg3 []any) (sqlexec.RecordSet, error) {
			return se.ExecuteInternal(arg1, arg2, arg3...)
		},
	)

	testCallProxyMethod22(
		t, sctx, se, "ExecuteStmt",
		ctx, ast.StmtNode(&ast.SelectStmt{Distinct: true}),
		sqlexec.RecordSet(&mockRecordSet{v: 3}), errors.New("err"),
		se.ExecuteStmt,
	)

	testCallProxyMethod32(
		t, sctx, se, "ParseWithParams",
		ctx, "select a+?, b+?, c+? from t", []any{"a", 10, "5"},
		ast.StmtNode(&ast.SelectStmt{}), errors.New("mockErr"),
		func(arg1 context.Context, arg2 string, arg3 []any) (ast.StmtNode, error) {
			return se.ParseWithParams(arg1, arg2, arg3...)
		},
	)

	testCallProxyMethod33(
		t, sctx, se, "ExecRestrictedStmt",
		ctx,
		ast.StmtNode(&ast.SelectStmt{}),
		[]sqlexec.OptionFuncAlias{sqlexec.ExecOptionIgnoreWarning, sqlexec.ExecOptionAnalyzeVer2},
		[]chunk.Row{{}, {}},
		[]*resolve.ResultField{{DBName: ast.NewCIStr("v1")}, {DBName: ast.NewCIStr("v2")}},
		errors.New("mockErr"),
		func(arg1 context.Context, arg2 ast.StmtNode, arg3 []sqlexec.OptionFuncAlias) (
			[]chunk.Row, []*resolve.ResultField, error,
		) {
			return se.ExecRestrictedStmt(arg1, arg2, arg3...)
		},
	)

	testCallProxyMethod43(
		t, sctx, se, "ExecRestrictedSQL",
		ctx,
		[]sqlexec.OptionFuncAlias{sqlexec.ExecOptionIgnoreWarning, sqlexec.ExecOptionAnalyzeVer2},
		"select ?, ?, ?",
		[]any{1, 2, "3"},
		[]chunk.Row{{}, {}},
		[]*resolve.ResultField{{DBName: ast.NewCIStr("v1")}, {DBName: ast.NewCIStr("v2")}},
		errors.New("mockErr"),
		func(arg1 context.Context, arg2 []sqlexec.OptionFuncAlias, arg3 string, arg4 []any) (
			[]chunk.Row, []*resolve.ResultField, error,
		) {
			return se.ExecRestrictedSQL(arg1, arg2, arg3, arg4...)
		},
	)

	testCallProxyMethod22(
		t, sctx, se, "Execute",
		ctx, "select 1, 2, 3",
		[]sqlexec.RecordSet{&mockRecordSet{v: 1}, &mockRecordSet{v: 2}}, errors.New("err"),
		func(arg1 context.Context, arg2 string) (rs []sqlexec.RecordSet, err error) {
			var execErr error
			err = se.WithSessionContext(func(sctx sessionctx.Context) error {
				rs, execErr = sctx.GetSQLExecutor().Execute(arg1, arg2)
				return execErr
			})

			if execErr != nil {
				require.Same(t, execErr, err)
			}

			return
		},
	)
}

// TestSessionClose 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestSessionClose(t *testing.T) {
#[test]
pub fn TestSessionClose() {
	sctx := &mockSessionContext{}
	se := mockSession(t, sctx)

	// normal close
	sctx.On("Close").Once()
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
	se.Close()
	require.True(t, se.internal.IsClosed())
	require.True(t, se.IsInternalClosed())
	sctx.AssertExpectations(t)

	// cannot use it again after closed
	WithSuppressAssert(func() {
		_, err := se.Execute(context.TODO(), "select 1")
		require.Error(t, err)
		require.Contains(t, err.Error(), "session is closed")
	})

	// close a closed session should take no effect
	se.Close()

	// close a not own session should take no effect
	se2 := mockSession(t, sctx)
	se.internal = se2.internal
	require.False(t, se.IsInternalClosed())
	se.Close()
	require.False(t, se.IsInternalClosed())
	require.False(t, se2.IsInternalClosed())

	// owner should close
	sctx.On("Close").Once()
	se2.Close()
	require.True(t, se.IsInternalClosed())
	require.True(t, se2.IsInternalClosed())
	sctx.AssertExpectations(t)
}

// TestSessionAvoidReuse 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestSessionAvoidReuse(t *testing.T) {
#[test]
pub fn TestSessionAvoidReuse() {
	// owner
	sctx := &mockSessionContext{}
	s, err := NewSessionForTest(sctx)
	require.NoError(t, err)
	require.False(t, s.internal.avoidReuse)
	s.AvoidReuse()
	require.True(t, s.internal.avoidReuse)

	// multiple calls of AvoidReuse
	s.AvoidReuse()
	require.True(t, s.internal.avoidReuse)

	// still can use the session after AvoidReuse
	ctx := context.WithValue(context.Background(), "a", "b")
	sctx.On("ExecuteInternal", ctx, "select 1", []any(nil)).Return(nil, nil).Once()
	rs, err := s.ExecuteInternal(ctx, "select 1")
	require.Nil(t, rs)
	require.NoError(t, err)
	sctx.AssertExpectations(t)

	// not owner
	s, err = NewSessionForTest(sctx)
	require.NoError(t, err)
	s2 := &Session{internal: s.internal}
	require.False(t, s.internal.avoidReuse)
	s2.AvoidReuse()
	require.False(t, s.internal.avoidReuse)
}

// TestInternalSessionUnThreadSafeOperations 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestInternalSessionUnThreadSafeOperations(t *testing.T) {
#[test]
pub fn TestInternalSessionUnThreadSafeOperations() {
	sctx := &mockSessionContext{}
	owner := &mockOwner{}
	se := mockInternalSession(t, sctx, owner)

	enterThreadSafeOperation := func() func() {
		inUse, unsafe := se.inUse, se.unsafe
		gotSctx, exit, err := se.EnterOperation(owner, true)
		require.NoError(t, err)
		require.Same(t, sctx, gotSctx)
		require.NotNil(t, exit)
		require.Equal(t, inUse+1, se.inUse)
		require.Equal(t, unsafe, se.unsafe)
		return exit
	}

	enterFirstThreadUnsafeOperation := func() func() {
		inUse, unsafe := se.inUse, se.unsafe
		gotSctx, exit, err := se.EnterOperation(owner, false)
		require.NoError(t, err)
		require.Same(t, sctx, gotSctx)
		require.NotNil(t, exit)
		require.Equal(t, inUse+1, se.inUse)
		require.Equal(t, unsafe+1, se.unsafe)
		return exit
	}

	enterThreadUnsafeOperationExpectError := func() {
		inUse, unsafe := se.inUse, se.unsafe
		WithSuppressAssert(func() {
			gotSctx, exit, err := se.EnterOperation(owner, false)
			require.EqualError(t, err, "EnterOperation error: race detected for concurrent thread-unsafe operations")
			require.Nil(t, gotSctx)
			require.Nil(t, exit)
			require.Equal(t, inUse, se.inUse)
			require.Equal(t, unsafe+1, se.unsafe)
		})
	}

	// multiple thread-safe operations should be allowed
	exit1 := enterThreadSafeOperation()
	exit2 := enterThreadSafeOperation()

	// the first thread-unsafe operation should be allowed
	exitUnsafe := enterFirstThreadUnsafeOperation()

	// next thread-unsafe operation should return error
	enterThreadUnsafeOperationExpectError()

	// net thread safe operation should still success
	exit3 := enterThreadSafeOperation()

	// next thread-unsafe operation should return error
	enterThreadUnsafeOperationExpectError()

	// exit
	require.Equal(t, uint64(4), se.inUse)
	require.Equal(t, uint64(3), se.unsafe)
	exit1()
	require.Equal(t, uint64(3), se.inUse)
	require.Equal(t, uint64(3), se.unsafe)
	exit3()
	require.Equal(t, uint64(2), se.inUse)
	require.Equal(t, uint64(3), se.unsafe)
	WithSuppressAssert(exitUnsafe)
	require.Equal(t, uint64(1), se.inUse)
	require.Equal(t, uint64(0), se.unsafe)
	exit2()
	require.Equal(t, uint64(0), se.inUse)
	require.Equal(t, uint64(0), se.unsafe)

	// new thread-unsafe operation should be allowed when after previous exits
	exitUnsafe = enterThreadSafeOperation()
	exitUnsafe()
	require.Equal(t, uint64(0), se.inUse)
	require.Equal(t, uint64(0), se.unsafe)

	// when panic happens, the session should also decrease `inUse` and `unsafe`
	func() {
	// Go defer/Close 表示资源收尾；后续接线 Rust 时应改成 RAII 或显式 drop。
		defer func() {
			require.Equal(t, uint64(0), se.inUse)
			require.Equal(t, uint64(0), se.unsafe)
			r := recover()
			require.Equal(t, "mockPanic", r)
		}()

		exitUnsafe = enterFirstThreadUnsafeOperation()
		defer exitUnsafe()
		require.Equal(t, uint64(1), se.inUse)
		require.Equal(t, uint64(1), se.unsafe)
		enterThreadUnsafeOperationExpectError()
		require.Equal(t, uint64(1), se.inUse)
		require.Equal(t, uint64(2), se.unsafe)
		panic("mockPanic")
	}()
}

// TestSessionThreadUnsafeOperations 对应 Go 的同名测试/辅助函数，保留原控制流、断言和错误传播形状。
// Go 签名: func TestSessionThreadUnsafeOperations(t *testing.T) {
#[test]
pub fn TestSessionThreadUnsafeOperations() {
	sctx := &mockSessionContext{}
	se := mockSession(t, sctx)
	ctx := context.WithValue(context.Background(), "a", "b")
	// Go 这里依赖 goroutine/channel/时间等待或原子状态；仅保留并发同步意图。
	ch := make(chan bool)
	waitCh := func(inOp bool) {
		select {
		case fromOp, ok := <-ch:
			require.True(t, ok)
			require.True(t, inOp != fromOp)
			return
		case <-time.After(10 * time.Second):
			require.FailNow(t, "timed out")
		}
	}

	sendCh := func(fromOp bool) {
		select {
		case ch <- fromOp:
		case <-time.After(10 * time.Second):
			require.FailNow(t, "timed out")
		}
	}

	called := false
	onCalled := func(_ mock.Arguments) {
		require.False(t, called)
		called = true
		require.Equal(t, uint64(1), se.internal.Inuse())
		sendCh(true)
		waitCh(true)
	}

	operations := []struct {
		name string
		call func(first bool) error
	}{
		{
			name: "WithSessionContext",
			call: func(first bool) error {
				err := se.WithSessionContext(func(got sessionctx.Context) error {
					require.Same(t, sctx, got)
					onCalled(nil)
					return nil
				})
				if first {
					require.True(t, called)
				}
				return err
			},
		},
		{
			name: "Execute",
			call: func(first bool) error {
				if first {
					sctx.On("Execute", ctx, "select 1").
						Run(onCalled).
						Return(nil, nil).
						Once()
				}
				_, err := se.Execute(ctx, "select 1")
				sctx.AssertExpectations(t)
				return err
			},
		},
		{
			name: "ExecuteInternal",
			call: func(first bool) error {
				if first {
					sctx.On("ExecuteInternal", ctx, "select 1", []any(nil)).
						Run(onCalled).
						Return(nil, nil).
						Once()
				}
				_, err := se.ExecuteInternal(ctx, "select 1")
				sctx.AssertExpectations(t)
				return err
			},
		},
		{
			name: "ExecuteStmt",
			call: func(first bool) error {
				n := &ast.SelectStmt{}
				if first {
					sctx.On("ExecuteStmt", ctx, n).
						Run(onCalled).
						Return(nil, nil).
						Once()
				}
				_, err := se.ExecuteStmt(ctx, n)
				sctx.AssertExpectations(t)
				return err
			},
		},
		{
			name: "ParseWithParams",
			call: func(first bool) error {
				if first {
					sctx.On("ParseWithParams", ctx, "select 1", []any(nil)).
						Run(onCalled).
						Return(nil, nil).
						Once()
				}
				_, err := se.ParseWithParams(ctx, "select 1")
				sctx.AssertExpectations(t)
				return err
			},
		},
		{
			name: "ExecRestrictedStmt",
			call: func(first bool) error {
				n := &ast.SelectStmt{}
				if first {
					sctx.On("ExecRestrictedStmt", ctx, n, []sqlexec.OptionFuncAlias(nil)).
						Run(onCalled).
						Return(nil, nil, nil).
						Once()
				}
				_, _, err := se.ExecRestrictedStmt(ctx, n)
				sctx.AssertExpectations(t)
				return err
			},
		},
		{
			name: "ExecRestrictedSQL",
			call: func(first bool) error {
				if first {
					sctx.On("ExecRestrictedSQL", ctx, []sqlexec.OptionFuncAlias(nil), "select 1", []any(nil)).
						Run(onCalled).
						Return(nil, nil, nil).
						Once()
				}
				_, _, err := se.ExecRestrictedSQL(ctx, nil, "select 1")
				sctx.AssertExpectations(t)
				return err
			},
		},
	}

	for i, op := range operations {
		t.Run(op.name, func(t *testing.T) {
			WithSuppressAssert(func() {
				called = false
				require.Zero(t, se.internal.inUse)
				require.Zero(t, se.internal.unsafe)
				// the first operation should be allowed
				go func() {
					require.NoError(t, op.call(true))
					require.True(t, called)
					sendCh(true)
				}()
				waitCh(false)
				require.Equal(t, uint64(1), se.internal.inUse)
				require.Equal(t, uint64(1), se.internal.unsafe)
				// other operations should return errors
				for j := range 3 {
					next := i + j
					if next >= len(operations) {
						next = len(operations) - 1
					}
					require.EqualError(
						t, operations[next].call(false),
						"EnterOperation error: race detected for concurrent thread-unsafe operations",
					)
					require.Equal(t, uint64(1), se.internal.inUse)
					require.Equal(t, uint64(j+2), se.internal.unsafe)
				}

				// notify first op to continue
				sendCh(false)
				// wait first op exit
				waitCh(false)
				require.Zero(t, se.internal.inUse)
				require.Zero(t, se.internal.unsafe)
			})
		})
	}
}
"########################################;

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

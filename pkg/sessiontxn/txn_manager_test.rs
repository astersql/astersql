// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

#![allow(non_snake_case, non_upper_case_globals, dead_code)]

// Shared test harness plus `TxnManager`/`EnterNewTxn` coverage for
// `pkg/sessiontxn`.
//
// The Go suite (`txn_manager_test.go`) drives every scenario through a real
// `testkit`/`mockstore`/`session` stack: a live TiKV/PD oracle, a concrete
// `sessionctx.Context`, and the production `sessiontxn.TxnManager` wired to
// the executor, planner and `infoschema` packages. This crate's Rust port
// (`interface.rs`, `failpoint.rs`, `future.rs`) intentionally keeps the
// `TxnManager`/`TxnContextProvider` traits and the package-level helpers
// (`GetTxnManager`, `NewTxn`, `NewTxnInStmt`,
// `AdviseOptimizeWithPlanAndThenWarmUp`) decoupled from any concrete
// session/store implementation: there is no Rust `sessionctx`/`mockstore`
// stack to plug into these tests yet (the same limitation already recorded
// for `pkg/sessiontxn/isolation`, see `isolation/main_test.rs`).
//
// Per the porting instructions for this task we substitute a minimal but
// real `TxnManager`/`TxnContextProvider` pair (`MockManager`/`MockProvider`)
// that behaves like an in-memory session: it records every call the public
// helpers make (in call order), exposes a `SessionValueStore` so the
// `failpoint.rs` assertions can be exercised for real, and never fabricates
// transaction/snapshot state that would require the full `astersql-kv`
// transaction machinery this package does not own. This lets
// `txn_context_test.rs` and `txn_rc_tso_optimize_test.rs` reuse the same
// harness to exercise the real dispatch/bookkeeping logic in
// `interface.rs` and `failpoint.rs` end to end, without inventing new
// production semantics or deleting any Go branch.
//
// 本文件提供会话事务管理器（TxnManager）测试替身与
// `EnterNewTxn`/`NewTxn`/`NewTxnInStmt`/优化建议热身等覆盖。Go 套件依赖完整
// testkit/mockstore；Rust 侧用 MockManager/MockProvider 记录调用顺序，验证
// `interface.rs`/`failpoint.rs` 的分发与记账，不伪造完整 KV 事务状态。

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::*;

/// Builds a real `InfoSchemaRef` backed by the production
/// `astersql-infoschema` mock helper (empty table list, given schema
/// version), mirroring the Go tests' use of `domain.GetDomain(...).InfoSchema()`.
/// 构造真实 `InfoSchemaRef`（空表列表 + 指定 schema 版本），对齐 Go domain.InfoSchema。

pub(crate) fn mock_info_schema(version: i64) -> InfoSchemaRef {
    astersql_infoschema::MockInfoSchemaWithSchemaVer(Vec::new(), version)
}

/// 构造带消息的测试用 Error。
pub(crate) fn mock_error(message: impl Into<String>) -> Error {
    Error::from(message.into())
}

/// A minimal `ast::ast::StmtNode` double: only identity (for `Debug`
/// assertions) matters to these tests, since the Go `OnStmtStart` plumbing
/// under test never inspects statement shape.
/// 最小语句 AST 替身：仅标签身份用于断言，OnStmtStart 不检查语句形态。

#[derive(Clone, Debug, Default)]
pub(crate) struct DummyStmtNode {
    /// 语句文本/身份标签。
    pub(crate) label: String,
    /// Parser node text required by the current `Node` contract.
    node_text: astersql_parser_ast::base::AstNode,
}

impl astersql_parser_ast::ast::Node for DummyStmtNode {
    fn node_text(&self) -> &astersql_parser_ast::base::AstNode {
        &self.node_text
    }

    fn node_text_mut(&mut self) -> &mut astersql_parser_ast::base::AstNode {
        &mut self.node_text
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }

    fn accept(&self, visitor: &mut dyn astersql_parser_ast::ast::Visitor) -> bool {
        visitor.enter(self);
        visitor.leave(self)
    }

    fn accept_in_place(
        &mut self,
        visitor: &mut dyn astersql_parser_ast::ast::InPlaceVisitor,
    ) -> bool {
        visitor.enter(self);
        visitor.leave(self)
    }
}

impl astersql_parser_ast::ast::StmtNode for DummyStmtNode {
    fn statement(&self) {}

    fn SEMCommand(&self) -> String {
        self.label.clone()
    }
}

/// 包装 DummyStmtNode 为 Statement 智能指针。
pub(crate) fn dummy_statement(label: impl Into<String>) -> Statement {
    Rc::new(DummyStmtNode {
        label: label.into(),
        node_text: Default::default(),
    })
}

/// 创建空的请求上下文。
pub(crate) fn request_context() -> RequestContext {
    RequestContext::new()
}

/// Every call a `MockManager`/`MockProvider` observed, in call order. Tests
/// assert against this instead of a real TiKV/session side effect.
/// MockManager/MockProvider 按调用顺序记录的事件列表，供测试断言。

#[derive(Default)]
pub(crate) struct RecordedCalls {
    /// 形如 `manager:...` / `provider:...` 的事件序列。
    pub(crate) events: Vec<String>,
}

/// Fake `TxnContextProvider`: an in-memory stand-in for the Go
/// `sessiontxn.TxnContextProvider` implementations (`isolation`/`staleread`
/// providers), which this package only depends on through the trait.
/// 假 TxnContextProvider：内存替身，对应 Go isolation/staleread 等 Provider 实现。

pub(crate) struct MockProvider {
    /// 事务可见的 InfoSchema（元数据快照）。
    pub(crate) info_schema: InfoSchemaRef,
    /// 事务作用域（如 global）。
    pub(crate) txn_scope: String,
    /// 读副本作用域。
    pub(crate) read_replica_scope: String,
    /// 语句读时间戳（stmt read ts）。
    pub(crate) stmt_read_ts: u64,
    /// 语句 FOR UPDATE 时间戳。
    pub(crate) stmt_for_update_ts: u64,
    /// 为真时 AdviseOptimizeWithPlan 返回错误。
    pub(crate) fail_advise_optimize: bool,
    /// 为真时语句错误处理建议 RetryReady。
    pub(crate) retry_ready: bool,
    /// 为真时 OnStmtRollback 返回错误。
    pub(crate) fail_stmt_rollback: bool,
    /// Identity of the local-temporary-tables snapshot attached to the
    /// transaction's `InfoSchema`, mirroring the Go
    /// `TxnCtx.TemporaryTables` sharing invariant that
    /// `AssertTxnManagerInfoSchema` checks.
    /// 事务 InfoSchema 上本地临时表快照身份，对齐 Go TemporaryTables 共享不变量。
    pub(crate) txn_local_temp_tables: Option<usize>,
    /// 与 Manager 共享的调用记录。
    calls: Rc<RefCell<RecordedCalls>>,
}

impl MockProvider {
    /// 使用共享调用记录构造默认 Provider。
    pub(crate) fn new(calls: Rc<RefCell<RecordedCalls>>) -> Self {
        Self {
            info_schema: mock_info_schema(1),
            txn_scope: "global".to_owned(),
            read_replica_scope: "global".to_owned(),
            stmt_read_ts: 100,
            stmt_for_update_ts: 200,
            fail_advise_optimize: false,
            retry_ready: false,
            fail_stmt_rollback: false,
            txn_local_temp_tables: None,
            calls,
        }
    }

    /// 追加一条 Provider 侧事件。
    fn record(&self, event: impl Into<String>) {
        self.calls.borrow_mut().events.push(event.into());
    }
}

/// Provider 侧优化建议与预热回调。
impl TxnAdvisable for MockProvider {
    fn AdviseWarmup(&mut self) -> Result<(), Error> {
        self.record("provider:warmup");
        Ok(())
    }

    fn AdviseOptimizeWithPlan(&mut self, _plan: &dyn Any) -> Result<(), Error> {
        self.record("provider:advise-optimize");
        // 测试注入：模拟基于执行计划的优化建议失败。
        if self.fail_advise_optimize {
            return Err(mock_error("advise optimize failed"));
        }
        Ok(())
    }
}

/// 实现事务上下文 Provider：时间戳、语句生命周期与错误下一步动作。
impl TxnContextProvider for MockProvider {
    fn GetTxnInfoSchema(&self) -> InfoSchemaRef {
        self.info_schema.clone()
    }

    fn GetTxnScope(&self) -> String {
        self.txn_scope.clone()
    }

    fn GetReadReplicaScope(&self) -> String {
        self.read_replica_scope.clone()
    }

    fn GetStmtReadTS(&mut self) -> Result<u64, Error> {
        self.record("provider:get-stmt-read-ts");
        Ok(self.stmt_read_ts)
    }

    fn GetStmtForUpdateTS(&mut self) -> Result<u64, Error> {
        self.record("provider:get-stmt-for-update-ts");
        Ok(self.stmt_for_update_ts)
    }

    fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, Error> {
        self.record("provider:get-snapshot-with-stmt-read-ts");
        Err(mock_error(
            "GetSnapshotWithStmtReadTS is not exercised by this mock",
        ))
    }

    fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, Error> {
        self.record("provider:get-snapshot-with-stmt-for-update-ts");
        Err(mock_error(
            "GetSnapshotWithStmtForUpdateTS is not exercised by this mock",
        ))
    }

    fn OnInitialize(&mut self, _ctx: &RequestContext, kind: EnterNewTxnType) -> Result<(), Error> {
        self.record(format!("provider:on-initialize:{kind:?}"));
        Ok(())
    }

    fn OnStmtStart(
        &mut self,
        _ctx: &RequestContext,
        _node: Option<Statement>,
    ) -> Result<(), Error> {
        self.record("provider:on-stmt-start");
        Ok(())
    }

    fn OnPessimisticStmtStart(&mut self, _ctx: &RequestContext) -> Result<(), Error> {
        self.record("provider:on-pessimistic-stmt-start");
        Ok(())
    }

    fn OnPessimisticStmtEnd(
        &mut self,
        _ctx: &RequestContext,
        successful: bool,
    ) -> Result<(), Error> {
        self.record(format!("provider:on-pessimistic-stmt-end:{successful}"));
        Ok(())
    }

    fn OnStmtErrorForNextAction(
        &mut self,
        _ctx: &RequestContext,
        point: StmtErrorHandlePoint,
        error: Error,
    ) -> StmtErrorAdvice {
        self.record(format!("provider:on-stmt-error:{point:?}:{error}"));
        // 可配置返回 RetryReady，否则 NoIdea（由上层决定）。
        if self.retry_ready {
            RetryReady()
        } else {
            NoIdea()
        }
    }

    fn OnStmtRetry(&mut self, _ctx: &RequestContext) -> Result<(), Error> {
        self.record("provider:on-stmt-retry");
        Ok(())
    }

    fn OnStmtCommit(&mut self, _ctx: &RequestContext) -> Result<(), Error> {
        self.record("provider:on-stmt-commit");
        Ok(())
    }

    fn OnStmtRollback(
        &mut self,
        _ctx: &RequestContext,
        pessimistic_retry: bool,
    ) -> Result<(), Error> {
        self.record(format!("provider:on-stmt-rollback:{pessimistic_retry}"));
        if self.fail_stmt_rollback {
            return Err(mock_error("stmt rollback failed"));
        }
        Ok(())
    }

    fn OnLocalTemporaryTableCreated(&mut self) {
        self.record("provider:on-local-temporary-table-created");
    }

    fn ActivateTxn(&mut self) -> Result<Transaction, Error> {
        self.record("provider:activate-txn");
        Err(mock_error("ActivateTxn is not exercised by this mock"))
    }

    fn SetOptionsBeforeCommit(
        &mut self,
        _txn: &mut dyn astersql_kv::Transaction,
        _commit_ts_checker: &dyn Fn(u64) -> bool,
    ) -> Result<(), Error> {
        self.record("provider:set-options-before-commit");
        Ok(())
    }
}

/// Fake `TxnManager`: forwards to `MockProvider` the same way the Go
/// `txnManager` forwards to whichever `TxnContextProvider` is bound for the
/// current transaction, while tracking `GetCurrentStmt`/`EnterNewTxn`
/// bookkeeping that `NewTxn`/`NewTxnInStmt` rely on.
/// 假 TxnManager：转发到 MockProvider，并跟踪当前语句与 EnterNewTxn 记账。

pub(crate) struct MockManager {
    /// 当前绑定的上下文 Provider。
    pub(crate) provider: MockProvider,
    /// 当前语句；NewTxnInStmt 据此决定是否再调 OnStmtStart。
    pub(crate) current_stmt: Option<Statement>,
    /// 已进入的新事务请求（类型 + 事务模式）历史。
    pub(crate) entered: Vec<(EnterNewTxnType, String)>,
    /// 与 Provider 共享的调用记录。
    calls: Rc<RefCell<RecordedCalls>>,
}

impl MockManager {
    /// 构造 Manager 并返回共享调用记录句柄。
    pub(crate) fn new() -> (Self, Rc<RefCell<RecordedCalls>>) {
        let calls = Rc::new(RefCell::new(RecordedCalls::default()));
        (
            Self {
                provider: MockProvider::new(calls.clone()),
                current_stmt: None,
                entered: Vec::new(),
                calls: calls.clone(),
            },
            calls,
        )
    }

    /// 追加一条 Manager 侧事件。
    fn record(&self, event: impl Into<String>) {
        self.calls.borrow_mut().events.push(event.into());
    }
}

/// Manager 先记账再转发 Advise* 到 Provider。
impl TxnAdvisable for MockManager {
    fn AdviseWarmup(&mut self) -> Result<(), Error> {
        self.record("manager:warmup");
        self.provider.AdviseWarmup()
    }

    fn AdviseOptimizeWithPlan(&mut self, plan: &dyn Any) -> Result<(), Error> {
        self.record("manager:advise-optimize");
        self.provider.AdviseOptimizeWithPlan(plan)
    }
}

/// 完整 TxnManager：EnterNewTxn、语句生命周期与错误动作转发。
impl TxnManager for MockManager {
    fn GetTxnInfoSchema(&self) -> InfoSchemaRef {
        self.provider.GetTxnInfoSchema()
    }

    fn GetTxnScope(&self) -> String {
        self.provider.GetTxnScope()
    }

    fn GetReadReplicaScope(&self) -> String {
        self.provider.GetReadReplicaScope()
    }

    fn GetStmtReadTS(&mut self) -> Result<u64, Error> {
        self.provider.GetStmtReadTS()
    }

    fn GetStmtForUpdateTS(&mut self) -> Result<u64, Error> {
        self.provider.GetStmtForUpdateTS()
    }

    fn GetContextProvider(&mut self) -> &mut dyn TxnContextProvider {
        &mut self.provider
    }

    fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, Error> {
        self.provider.GetSnapshotWithStmtReadTS()
    }

    fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, Error> {
        self.provider.GetSnapshotWithStmtForUpdateTS()
    }

    fn EnterNewTxn(
        &mut self,
        ctx: &RequestContext,
        request: &mut EnterNewTxnRequest,
    ) -> Result<(), Error> {
        // 记录进入新事务请求，再初始化 Provider。
        self.entered.push((request.Type, request.TxnMode.clone()));
        self.record(format!("manager:enter-new-txn:{:?}", request.Type));
        self.provider.OnInitialize(ctx, request.Type)
    }

    fn OnTxnEnd(&mut self) {
        self.record("manager:on-txn-end");
        // 事务结束清空当前语句。
        self.current_stmt = None;
    }

    fn OnStmtStart(&mut self, ctx: &RequestContext, node: Option<Statement>) -> Result<(), Error> {
        self.record("manager:on-stmt-start");
        // 语句开始时缓存当前语句节点。
        self.current_stmt = node.clone();
        self.provider.OnStmtStart(ctx, node)
    }

    fn OnPessimisticStmtStart(&mut self, ctx: &RequestContext) -> Result<(), Error> {
        self.record("manager:on-pessimistic-stmt-start");
        self.provider.OnPessimisticStmtStart(ctx)
    }

    fn OnPessimisticStmtEnd(
        &mut self,
        ctx: &RequestContext,
        successful: bool,
    ) -> Result<(), Error> {
        self.record("manager:on-pessimistic-stmt-end");
        self.provider.OnPessimisticStmtEnd(ctx, successful)
    }

    fn OnStmtErrorForNextAction(
        &mut self,
        ctx: &RequestContext,
        point: StmtErrorHandlePoint,
        error: Error,
    ) -> StmtErrorAdvice {
        self.record(format!("manager:on-stmt-error:{point:?}"));
        self.provider.OnStmtErrorForNextAction(ctx, point, error)
    }

    fn OnStmtRetry(&mut self, ctx: &RequestContext) -> Result<(), Error> {
        self.record("manager:on-stmt-retry");
        self.provider.OnStmtRetry(ctx)
    }

    fn OnStmtCommit(&mut self, ctx: &RequestContext) -> Result<(), Error> {
        self.record("manager:on-stmt-commit");
        self.provider.OnStmtCommit(ctx)
    }

    fn OnStmtRollback(
        &mut self,
        ctx: &RequestContext,
        pessimistic_retry: bool,
    ) -> Result<(), Error> {
        self.record("manager:on-stmt-rollback");
        self.provider.OnStmtRollback(ctx, pessimistic_retry)
    }

    fn OnStmtEnd(&mut self) {
        self.record("manager:on-stmt-end");
        self.current_stmt = None;
    }

    fn OnLocalTemporaryTableCreated(&mut self) {
        self.record("manager:on-local-temporary-table-created");
        self.provider.OnLocalTemporaryTableCreated();
    }

    fn ActivateTxn(&mut self) -> Result<Transaction, Error> {
        self.record("manager:activate-txn");
        self.provider.ActivateTxn()
    }

    fn GetCurrentStmt(&self) -> Option<Statement> {
        self.current_stmt.clone()
    }

    fn SetOptionsBeforeCommit(
        &mut self,
        txn: &mut dyn astersql_kv::Transaction,
        commit_ts_checker: &dyn Fn(u64) -> bool,
    ) -> Result<(), Error> {
        self.record("manager:set-options-before-commit");
        self.provider.SetOptionsBeforeCommit(txn, commit_ts_checker)
    }
}

/// Fake session: a minimal `TxnManagerContext` + `SessionValueStore` pair,
/// standing in for the Go `sessionctx.Context` that owns both the value
/// store `failpoint.go` reads/writes and the `TxnManager` `interface.go`
/// dispatches through.
/// 假会话：最小 TxnManagerContext + SessionValueStore，对齐 Go sessionctx.Context。

pub(crate) struct MockSession {
    /// 会话绑定的事务管理器。
    pub(crate) manager: MockManager,
    /// 会话侧本地临时表身份（断言用）。
    pub(crate) local_temp_tables: Option<usize>,
    /// failpoint/测试钩子使用的会话键值存储。
    values: HashMap<&'static str, SessionValue>,
}

impl MockSession {
    /// 构造空会话并返回调用记录。
    pub(crate) fn new() -> (Self, Rc<RefCell<RecordedCalls>>) {
        let (manager, calls) = MockManager::new();
        (
            Self {
                manager,
                local_temp_tables: None,
                values: HashMap::new(),
            },
            calls,
        )
    }
}

/// 暴露可变 TxnManager 引用。
impl TxnManagerContext for MockSession {
    fn txn_manager(&mut self) -> &mut dyn TxnManager {
        &mut self.manager
    }
}

/// 会话级任意值存取（TSO 计数、锁错误 map、测试钩子等）。
impl SessionValueStore for MockSession {
    fn Value(&self, key: &str) -> Option<&dyn Any> {
        self.values.get(key).map(|value| value.as_ref())
    }

    fn ValueMut(&mut self, key: &str) -> Option<&mut dyn Any> {
        self.values.get_mut(key).map(|value| value.as_mut())
    }

    fn SetValue(&mut self, key: &'static str, value: SessionValue) {
        self.values.insert(key, value);
    }
}

/// 本地临时表身份断言上下文。
impl TxnAssertionContext for MockSession {
    fn LocalTemporaryTablesIdentity(&self) -> Option<usize> {
        self.local_temp_tables
    }

    fn TxnInfoSchemaLocalTemporaryTablesIdentity(&mut self) -> Option<usize> {
        self.manager.provider.txn_local_temp_tables
    }
}

#[test]
/// GetTxnManager 应返回会话绑定的 Manager 及其默认作用域/schema。
fn get_txn_manager_returns_the_session_bound_manager() {
    let (mut session, _calls) = MockSession::new();
    let manager = GetTxnManager(&mut session);
    assert_eq!(manager.GetTxnScope(), "global");
    assert_eq!(manager.GetTxnInfoSchema().SchemaMetaVersion(), 1);
}

#[test]
/// NewTxn 默认进入乐观事务（Optimistic）并初始化 Provider。
fn new_txn_enters_a_default_optimistic_transaction() {
    let (mut session, calls) = MockSession::new();
    let ctx = request_context();
    NewTxn(&ctx, &mut session).expect("NewTxn must succeed against the mock manager");

    assert_eq!(
        session.manager.entered,
        vec![(
            EnterNewTxnDefault,
            astersql_parser_ast::Optimistic.to_owned()
        )]
    );
    assert_eq!(
        calls.borrow().events,
        vec![
            "manager:enter-new-txn:EnterNewTxnDefault".to_owned(),
            format!("provider:on-initialize:{:?}", EnterNewTxnDefault),
        ]
    );
}

#[test]
/// 无当前语句时 NewTxnInStmt 仍以 nil 语句调用 OnStmtStart，与 Go 一致。
fn new_txn_in_stmt_calls_on_stmt_start_when_manager_has_no_current_statement() {
    let (mut session, calls) = MockSession::new();
    let ctx = request_context();

    NewTxnInStmt(&ctx, &mut session).expect("NewTxnInStmt must succeed against the mock manager");

    assert_eq!(
        calls.borrow().events,
        vec![
            "manager:enter-new-txn:EnterNewTxnDefault".to_owned(),
            "provider:on-initialize:EnterNewTxnDefault".to_owned(),
            "manager:on-stmt-start".to_owned(),
            "provider:on-stmt-start".to_owned(),
        ]
    );
}

#[test]
/// 已有当前语句时 NewTxnInStmt 在进入事务后再调 OnStmtStart。
fn new_txn_in_stmt_calls_on_stmt_start_when_manager_has_a_current_statement() {
    let (mut session, calls) = MockSession::new();
    let ctx = request_context();
    // Mirrors the Go executor already having recorded the statement being
    // compiled/run before `NewTxnInStmt` enters (or re-enters) a
    // transaction for it.
    // 模拟执行器已登记正在编译/运行的语句后再 Enter/再进事务。

    session.manager.current_stmt = Some(dummy_statement("select 1"));

    NewTxnInStmt(&ctx, &mut session).expect("NewTxnInStmt must succeed against the mock manager");

    assert_eq!(
        calls.borrow().events,
        vec![
            "manager:enter-new-txn:EnterNewTxnDefault".to_owned(),
            "provider:on-initialize:EnterNewTxnDefault".to_owned(),
            "manager:on-stmt-start".to_owned(),
            "provider:on-stmt-start".to_owned(),
        ]
    );
    assert_eq!(
        session.manager.GetCurrentStmt().unwrap().SEMCommand(),
        "select 1"
    );
}

#[test]
/// AdviseOptimizeWithPlanAndThenWarmUp 先优化建议再预热。
fn advise_optimize_with_plan_and_then_warm_up_runs_optimize_before_warmup() {
    let (mut session, calls) = MockSession::new();
    let plan: i32 = 42;

    AdviseOptimizeWithPlanAndThenWarmUp(&mut session, &plan)
        .expect("advise+warmup must succeed when optimize does not fail");

    assert_eq!(
        calls.borrow().events,
        vec![
            "manager:advise-optimize".to_owned(),
            "provider:advise-optimize".to_owned(),
            "manager:warmup".to_owned(),
            "provider:warmup".to_owned(),
        ]
    );
}

#[test]
/// 优化建议失败时短路，不执行 warmup。
fn advise_optimize_with_plan_and_then_warm_up_short_circuits_when_optimize_fails() {
    let (mut session, calls) = MockSession::new();
    session.manager.provider.fail_advise_optimize = true;
    let plan: i32 = 42;

    let result = AdviseOptimizeWithPlanAndThenWarmUp(&mut session, &plan);

    assert!(result.is_err(), "optimize failure must propagate");
    assert_eq!(
        calls.borrow().events,
        vec![
            "manager:advise-optimize".to_owned(),
            "provider:advise-optimize".to_owned(),
        ],
        "warmup must never run once optimize fails"
    );
}

#[test]
/// BEGIN 语句 + 悲观模式（Pessimistic）的 EnterNewTxn 到达 Provider。
fn enter_new_txn_with_begin_stmt_and_pessimistic_mode_reaches_the_provider() {
    let (mut session, calls) = MockSession::new();
    let ctx = request_context();
    let mut request = EnterNewTxnRequest {
        Type: EnterNewTxnWithBeginStmt,
        TxnMode: astersql_parser_ast::Pessimistic.to_owned(),
        ..EnterNewTxnRequest::default()
    };

    GetTxnManager(&mut session)
        .EnterNewTxn(&ctx, &mut request)
        .expect("EnterNewTxn must succeed against the mock manager");

    assert_eq!(
        session.manager.entered,
        vec![(
            EnterNewTxnWithBeginStmt,
            astersql_parser_ast::Pessimistic.to_owned()
        )]
    );
    assert_eq!(
        calls.borrow().events,
        vec![
            "manager:enter-new-txn:EnterNewTxnWithBeginStmt".to_owned(),
            format!("provider:on-initialize:{:?}", EnterNewTxnWithBeginStmt),
        ]
    );
}

#[test]
/// GetContextProvider 暴露的作用域与 Manager 一致。
fn get_context_provider_exposes_the_same_scope_the_manager_reports() {
    let (mut session, _calls) = MockSession::new();
    let manager = GetTxnManager(&mut session);
    let provider = manager.GetContextProvider();
    assert_eq!(provider.GetTxnScope(), "global");
    assert_eq!(provider.GetReadReplicaScope(), "global");
}

#[test]
/// OnStmtErrorForNextAction 转发 Provider 的 NoIdea/RetryReady 建议。
fn on_stmt_error_for_next_action_forwards_to_the_provider_advice() {
    let (mut session, calls) = MockSession::new();
    let ctx = request_context();

    let (action, error) = GetTxnManager(&mut session).OnStmtErrorForNextAction(
        &ctx,
        StmtErrAfterQuery,
        mock_error("boom"),
    );
    assert_eq!(action, StmtActionNoIdea);
    assert!(error.is_none());

    session.manager.provider.retry_ready = true;
    let (action, error) = GetTxnManager(&mut session).OnStmtErrorForNextAction(
        &ctx,
        StmtErrAfterPessimisticLock,
        mock_error("write conflict"),
    );
    assert_eq!(action, StmtActionRetryReady);
    assert!(error.is_none());

    assert_eq!(
        calls.borrow().events,
        vec![
            "manager:on-stmt-error:StmtErrAfterQuery".to_owned(),
            "provider:on-stmt-error:StmtErrAfterQuery:boom".to_owned(),
            "manager:on-stmt-error:StmtErrAfterPessimisticLock".to_owned(),
            "provider:on-stmt-error:StmtErrAfterPessimisticLock:write conflict".to_owned(),
        ]
    );
}

#[test]
/// OnStmtEnd/OnTxnEnd 均清空当前语句。
fn on_txn_end_and_on_stmt_end_clear_the_current_statement() {
    let (mut session, _calls) = MockSession::new();
    session.manager.current_stmt = Some(dummy_statement("select 1"));
    GetTxnManager(&mut session).OnStmtEnd();
    assert!(session.manager.GetCurrentStmt().is_none());

    session.manager.current_stmt = Some(dummy_statement("select 1"));
    GetTxnManager(&mut session).OnTxnEnd();
    assert!(session.manager.GetCurrentStmt().is_none());
}

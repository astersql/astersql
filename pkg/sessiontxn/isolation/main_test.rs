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

// Shared test harness for `pkg/sessiontxn/isolation`.
//
// The Go suite (`main_test.go`) builds every scenario on top of a real
// `testkit`/`mockstore`/`session` stack: a live TiKV oracle, a real
// `sessionctx.Context` and a `sessiontxn.TxnManager` wired to the store.
// The Rust port of this package (`base.rs`, `optimistic.rs`,
// `readcommitted.rs`, `repeatable_read.rs`, `serializable.rs`) intentionally
// decouples the isolation-level state machines from any concrete
// session/store implementation behind the `IsolationRuntime` trait, so
// there is no Rust `sessionctx`/`mockstore` to plug into these tests yet.
//
// Per the porting instructions for this task, we substitute a minimal but
// real `IsolationRuntime` implementation (`MockRuntime`) that behaves like
// an in-memory oracle/session pair: it hands out caller-supplied
// timestamps in order (mirroring the monotonically increasing PD oracle),
// records every runtime callback the providers make, and exposes the same
// `SessionState` the production code reads and mutates. This lets the
// isolation-level tests in the sibling `*_test.rs` files exercise the real
// `BaseTxnContextProvider`/`OptimisticTxnContextProvider`/... state
// machines end to end, asserting the same transaction-state invariants the
// Go `txnAssert[T]` helper checks (isolation level, pessimistic flag,
// active/explicit txn, start ts monotonicity, causal-consistency and
// retryability), without inventing new production semantics.
//
// 中文概述：本文件为 `pkg/sessiontxn/isolation` 的共享测试夹具。
// Go 侧依赖真实 testkit/mockstore/会话与 PD Oracle；Rust 移植将隔离状态机与
// 具体会话解耦到 `IsolationRuntime`，故用内存 `MockRuntime`（按序发放时间戳、
// 记录回调、暴露 `SessionState`）驱动兄弟 `*_test.rs`，对齐 Go `txnAssert` 不变量。

use crate::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Minimal `TxnInfoSchema` double; mirrors the Go `infoschema.InfoSchema`
/// pair of `SchemaMetaVersion` and "is this already session-extended".
/// 最小 InfoSchema 测试双，对应 Go 的 SchemaMetaVersion 与是否已会话扩展。
#[derive(Debug)]
pub(crate) struct MockInfoSchema {
    pub(crate) version: u64,
    pub(crate) extended: bool,
}

impl TxnInfoSchema for MockInfoSchema {
    fn schema_meta_version(&self) -> u64 {
        self.version
    }

    fn is_session_extended(&self) -> bool {
        self.extended
    }
}

/// Every side effect a provider asked the runtime to perform, in call order,
/// plus the last activation/commit options and snapshots it produced. Tests
/// assert against this instead of poking at a real TiKV client.
/// 按调用顺序记录 Provider 请求运行时执行的副作用，以及最近一次激活/提交选项与快照。
#[derive(Default)]
pub(crate) struct Captured {
    pub(crate) events: Vec<String>,
    pub(crate) activation: Option<TxnActivationOptions>,
    pub(crate) commit: Option<CommitOptions>,
    pub(crate) snapshots: Vec<Snapshot>,
}

/// A shared handle onto the same monotonically increasing PD-like clock a
/// `MockRuntime` draws its timestamps from. The real Go tests read this
/// clock through `sctx.GetStore().GetOracle()`, independently of whatever
/// the provider under test is doing; `MockRuntime` moves into a `Box<dyn
/// IsolationRuntime>` almost immediately in every test, so tests keep a
/// cloned `OracleClock` handle (an `Rc`) around beforehand instead of trying
/// to reach back into the boxed trait object.
/// 与 MockRuntime 共用的单调递增类 PD 时钟句柄；因 runtime 很快被 Box 化，测试需提前克隆 Rc。
#[derive(Clone)]
pub(crate) struct OracleClock(Rc<RefCell<VecDeque<u64>>>);

impl OracleClock {
    /// 向时钟队列追加一个时间戳。
    pub(crate) fn push(&self, timestamp: u64) {
        self.0.borrow_mut().push_back(timestamp);
    }

    /// Mirrors the Go `getOracleTS(t, sctx)` helper.
    /// 弹出并返回下一个时间戳。
    pub(crate) fn next(&self) -> u64 {
        self.0
            .borrow_mut()
            .pop_front()
            .expect("test must enqueue enough oracle timestamps")
    }
}

/// A fake `IsolationRuntime`: an in-memory stand-in for the TiKV oracle and
/// the session the real `github.com/pingcap/tidb/pkg/sessiontxn` package
/// binds a provider to. Timestamps are dispensed strictly in the order the
/// test enqueues them, which is how the Go tests rely on `getOracleTS`
/// always observing a strictly increasing PD clock.
/// 伪造的 IsolationRuntime：内存版 Oracle+会话；时间戳严格按入队顺序发放。
pub(crate) struct MockRuntime {
    pub(crate) session: SessionState,
    clock: OracleClock,
    pub(crate) captured: Rc<RefCell<Captured>>,
    fair_locking: bool,
    pub(crate) txn_scope: String,
    pub(crate) info_schema_version: u64,
}

impl MockRuntime {
    /// 用给定会话与预置时间戳构造 MockRuntime。
    pub(crate) fn new(session: SessionState, timestamps: &[u64]) -> (Self, Rc<RefCell<Captured>>) {
        let captured = Rc::new(RefCell::new(Captured::default()));
        let clock = OracleClock(Rc::new(RefCell::new(timestamps.iter().copied().collect())));
        (
            Self {
                session,
                clock,
                captured: captured.clone(),
                fair_locking: false,
                txn_scope: "global".into(),
                info_schema_version: 1,
            },
            captured,
        )
    }

    /// 覆盖配置的事务作用域后返回 self。
    pub(crate) fn with_scope(mut self, scope: impl Into<String>) -> Self {
        self.txn_scope = scope.into();
        self
    }

    /// A clonable handle to this runtime's timestamp source, to be taken
    /// before the runtime is boxed into a `Box<dyn IsolationRuntime>`.
    /// 在 Box 化前取出可克隆的时钟句柄。
    pub(crate) fn oracle_clock(&self) -> OracleClock {
        self.clock.clone()
    }

    fn next_timestamp(&mut self) -> Result<u64, TxnError> {
        self.clock
            .0
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| TxnError::new(TxnErrorKind::Runtime, "mock timestamp queue is empty"))
    }
}

/// Stand-in for the Go `getOracleTS` helper.
/// 对应 Go `getOracleTS` 辅助函数。
pub(crate) fn get_oracle_ts(clock: &OracleClock) -> u64 {
    clock.next()
}

impl IsolationRuntime for MockRuntime {
    fn session(&self) -> &SessionState {
        &self.session
    }

    fn session_mut(&mut self) -> &mut SessionState {
        &mut self.session
    }

    fn latest_info_schema(&self) -> TxnInfoSchemaRef {
        Rc::new(MockInfoSchema {
            version: self.info_schema_version,
            extended: false,
        })
    }

    fn ensure_session_extended_info_schema(
        &mut self,
        info_schema: &TxnInfoSchemaRef,
    ) -> TxnInfoSchemaRef {
        self.captured
            .borrow_mut()
            .events
            .push("extend-schema".into());
        Rc::new(MockInfoSchema {
            version: info_schema.schema_meta_version(),
            extended: true,
        })
    }

    fn configured_txn_scope(&self) -> String {
        self.txn_scope.clone()
    }

    fn existing_transaction_start_ts(&self) -> Option<u64> {
        None
    }

    fn take_prepared_timestamp_future(&mut self) -> Option<Box<dyn TimestampFuture>> {
        None
    }

    fn commit_before_enter_new_txn(&mut self, _context: &RuntimeContext) -> Result<(), TxnError> {
        self.captured.borrow_mut().events.push("commit-old".into());
        Ok(())
    }

    fn oracle_future(
        &mut self,
        _context: &RuntimeContext,
        _scope: &str,
        _low_resolution: bool,
    ) -> Result<Box<dyn TimestampFuture>, TxnError> {
        Ok(Box::new(ConstantFuture(self.next_timestamp()?)))
    }

    fn latest_timestamp(
        &mut self,
        _context: &RuntimeContext,
        _scope: &str,
    ) -> Result<u64, TxnError> {
        self.next_timestamp()
    }

    fn activate_transaction(
        &mut self,
        _context: &RuntimeContext,
        start_ts: u64,
        options: &TxnActivationOptions,
    ) -> Result<(), TxnError> {
        self.captured
            .borrow_mut()
            .events
            .push(format!("activate:{start_ts}"));
        self.captured.borrow_mut().activation = Some(options.clone());
        Ok(())
    }

    fn set_transaction_snapshot_ts(&mut self, timestamp: u64) -> Result<(), TxnError> {
        self.captured
            .borrow_mut()
            .events
            .push(format!("snapshot-ts:{timestamp}"));
        Ok(())
    }

    fn snapshot(&mut self, timestamp: u64, from_active: bool) -> Result<Snapshot, TxnError> {
        let snapshot = Snapshot {
            timestamp,
            isolation: IsolationLevel::Optimistic,
            from_active_transaction: from_active,
            rc_check_ts: false,
        };
        self.captured.borrow_mut().snapshots.push(snapshot.clone());
        Ok(snapshot)
    }

    fn set_commit_options(&mut self, options: &CommitOptions) -> Result<(), TxnError> {
        self.captured.borrow_mut().commit = Some(options.clone());
        Ok(())
    }

    fn attach_local_temporary_tables(
        &mut self,
        info_schema: &TxnInfoSchemaRef,
    ) -> TxnInfoSchemaRef {
        Rc::new(MockInfoSchema {
            version: info_schema.schema_meta_version() + 1,
            extended: true,
        })
    }

    fn start_fair_locking(&mut self) -> Result<(), TxnError> {
        self.fair_locking = true;
        self.captured.borrow_mut().events.push("fair-start".into());
        Ok(())
    }

    fn done_fair_locking(&mut self, _context: &RuntimeContext) -> Result<(), TxnError> {
        self.fair_locking = false;
        self.captured.borrow_mut().events.push("fair-done".into());
        Ok(())
    }

    fn cancel_fair_locking(&mut self, _context: &RuntimeContext) -> Result<(), TxnError> {
        self.fair_locking = false;
        self.captured.borrow_mut().events.push("fair-cancel".into());
        Ok(())
    }

    fn retry_fair_locking(&mut self, _context: &RuntimeContext) -> Result<(), TxnError> {
        self.fair_locking = true;
        self.captured.borrow_mut().events.push("fair-retry".into());
        Ok(())
    }

    fn is_in_fair_locking_mode(&self) -> bool {
        self.fair_locking
    }
}

/// Adapter double for `ast.StmtNode`: only `IsReadOnly()`/read-vs-write
/// classification is observed by the providers under test.
/// AST 语句适配测试双：仅暴露只读/读写分类。
pub(crate) struct TestStatement(pub(crate) bool);

impl StatementInspection for TestStatement {
    fn is_read_only(&self) -> bool {
        self.0
    }
}

/// Adapter double for a `base.PhysicalPlan` tree: enough shape (point-get,
/// batch point-get, lock wrapper, DML root, ...) to exercise the plan
/// predicates in `optimistic.rs`/`readcommitted.rs`/`repeatable_read.rs`
/// without depending on the real planner/executor crates.
/// 物理计划树适配测试双：足以覆盖乐观/RC/RR 计划谓词，无需真实优化器。
pub(crate) struct TestPlan {
    pub(crate) kind: PlanKind,
    pub(crate) children: Vec<TestPlan>,
}

impl TestPlan {
    /// 构造无子节点的叶子计划。
    pub(crate) fn leaf(kind: PlanKind) -> Self {
        Self {
            kind,
            children: Vec::new(),
        }
    }

    /// 构造带单个子节点的计划。
    pub(crate) fn with_child(kind: PlanKind, child: TestPlan) -> Self {
        Self {
            kind,
            children: vec![child],
        }
    }
}

impl PlanInspection for TestPlan {
    fn kind(&self) -> PlanKind {
        self.kind
    }

    fn children(&self) -> Vec<&dyn PlanInspection> {
        self.children
            .iter()
            .map(|child| child as &dyn PlanInspection)
            .collect()
    }
}

/// Rust analogue of the Go generic `txnAssert[T]`. Since providers here are
/// concrete Rust structs rather than an interface value pulled out of a
/// `sessiontxn.TxnManager`, callers pass in the provider's
/// `BaseTxnContextProvider` directly instead of relying on a runtime type
/// assertion.
/// 对应 Go 泛型 `txnAssert[T]`：直接传入 `BaseTxnContextProvider` 做状态断言。
pub(crate) struct TxnAssert {
    /// `None` mirrors Go's `isolation == ""` (optimistic / non-pessimistic).
    pub(crate) isolation: Option<IsolationLevel>,
    pub(crate) active: bool,
    pub(crate) in_txn: bool,
    pub(crate) min_start_ts: u64,
    pub(crate) start_ts: u64,
    pub(crate) causal_consistency_only: bool,
    pub(crate) could_retry: bool,
}

impl TxnAssert {
    /// 断言非活跃事务状态。
    pub(crate) fn inactive(isolation: Option<IsolationLevel>) -> Self {
        Self {
            isolation,
            active: false,
            in_txn: false,
            min_start_ts: 0,
            start_ts: 0,
            causal_consistency_only: false,
            could_retry: false,
        }
    }

    /// 断言活跃事务状态（可选显式 in_txn 与最小 start_ts）。
    pub(crate) fn active(
        isolation: Option<IsolationLevel>,
        in_txn: bool,
        min_start_ts: u64,
    ) -> Self {
        Self {
            isolation,
            active: true,
            in_txn,
            min_start_ts,
            start_ts: 0,
            causal_consistency_only: false,
            could_retry: false,
        }
    }

    /// Mirrors `(*txnAssert[T]).Check`: validates isolation/pessimistic
    /// flag, explicit-txn flag, staleness, causal-consistency, retryability
    /// and start-ts monotonicity against the provider's base state.
    /// 校验隔离级别/悲观标志、显式事务、过期读、因果一致、可重试与 start_ts 单调性。
    pub(crate) fn check(&self, base: &BaseTxnContextProvider) {
        let session = base.runtime.session();
        assert_eq!(session.txn.isolation, self.isolation, "isolation mismatch");
        assert_eq!(
            self.isolation.is_some(),
            session.txn.is_pessimistic,
            "pessimistic flag should follow isolation presence"
        );
        assert!(!session.txn.is_staleness, "txn must not be a stale read");
        assert_eq!(self.in_txn, session.in_txn, "in_txn mismatch");
        assert_eq!(
            self.could_retry, session.txn.could_retry,
            "could_retry mismatch"
        );
        assert_eq!(
            self.causal_consistency_only, base.causal_consistency_only,
            "causal consistency flag mismatch"
        );
        if !self.active {
            assert!(!self.in_txn, "inactive txn cannot be explicit");
            assert_eq!(self.start_ts, 0);
            assert_eq!(
                session.txn.start_ts, 0,
                "inactive txn must have no start ts"
            );
        } else {
            assert!(
                self.min_start_ts != 0 || self.start_ts != 0,
                "active assertion needs a baseline"
            );
            assert!(
                session.txn.start_ts > self.min_start_ts,
                "start ts must be newer than the baseline"
            );
            if self.start_ts != 0 {
                assert_eq!(self.start_ts, session.txn.start_ts);
            }
            assert_eq!(session.txn.is_pessimistic, base.pessimistic);
        }
    }
}

/// 测试：get_oracle_ts 与 Provider 共用同一单调时钟。
#[test]
fn get_oracle_ts_observes_the_same_monotonic_clock_as_the_provider() {
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[10, 20, 30]);
    let clock = runtime.oracle_clock();
    // The baseline draws from the same queue a provider would use for its
    // own oracle calls, so it always precedes what the provider observes
    // next -- exactly like a real PD timestamp being strictly increasing.
    let baseline = get_oracle_ts(&clock);
    assert_eq!(baseline, 10);
    let mut provider = NewOptimisticTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::Default)
        .unwrap();
    assert!(provider.base.runtime.session().txn.start_ts > baseline);
    // The clock handle keeps working after the runtime has been boxed away
    // behind `Box<dyn IsolationRuntime>`: `OnInitialize` above already
    // consumed 20 as the txn's start ts, so the next pop is 30.
    assert_eq!(get_oracle_ts(&clock), 30);
}

/// 测试：TxnAssert 对非活跃→激活后的事务状态断言。
#[test]
fn txn_assert_reports_expected_transaction_state_for_active_and_inactive_providers() {
    // Mirrors the Go "non-active txn and then active it" scenario, which
    // requires `autocommit=0` for `BeforeStmt` activation to mark the txn
    // as explicit (`in_txn`) once it becomes active.
    let session = SessionState {
        autocommit: false,
        ..SessionState::default()
    };
    let (runtime, _) = MockRuntime::new(session, &[50, 100]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewRegisteredTxnContextProvider(
        ProviderKind::PessimisticRepeatableRead,
        Box::new(runtime),
        false,
    );
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::BeforeStmt)
        .unwrap();
    TxnAssert::inactive(Some(IsolationLevel::RepeatableRead)).check(provider.BaseRef());

    provider.ActivateTxn().unwrap();
    TxnAssert::active(Some(IsolationLevel::RepeatableRead), true, baseline)
        .check(provider.BaseRef());
}

/// 测试：TxnAssert 跟踪仅因果一致标志。
#[test]
fn txn_assert_tracks_causal_consistency_only_flag() {
    // `WithBeginStmt` (an explicit `START TRANSACTION ... WITH CAUSAL
    // CONSISTENCY ONLY`) marks the txn explicit, unlike `Default`.
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[5, 15]);
    let clock = runtime.oracle_clock();
    let baseline = get_oracle_ts(&clock);
    let mut provider = NewPessimisticSerializableTxnContextProvider(Box::new(runtime), true);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::WithBeginStmt)
        .unwrap();
    let mut assert_active = TxnAssert::active(Some(IsolationLevel::Serializable), true, baseline);
    assert_active.causal_consistency_only = true;
    assert_active.check(&provider.base);
}

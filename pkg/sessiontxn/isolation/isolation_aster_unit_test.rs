// Copyright 2026 AsterSQL.

// 隔离级别综合原生单元测试。
//
// 用 `MockRuntime` 捕获激活/提交/公平锁等回调，验证各 Provider 对会话配置、
// InfoSchema 扩展、RC check_ts、RR 重试刷新 TS、计划谓词与注册表构造的行为。

use crate::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

#[derive(Debug)]
/// 测试用 InfoSchema：固定版本号与是否已会话扩展。
struct MockInfoSchema {
    version: u64,
    extended: bool,
}

impl TxnInfoSchema for MockInfoSchema {
    fn schema_meta_version(&self) -> u64 {
        self.version
    }

    fn is_session_extended(&self) -> bool {
        self.extended
    }
}

#[derive(Default)]
/// 记录 Provider 对运行时的副作用：事件序列、激活/提交选项与快照。
struct Captured {
    events: Vec<String>,
    activation: Option<TxnActivationOptions>,
    commit: Option<CommitOptions>,
    snapshots: Vec<Snapshot>,
}

/// 内存版 `IsolationRuntime`：按队列发放时间戳并写入 `Captured`。
struct MockRuntime {
    session: SessionState,
    timestamps: VecDeque<u64>,
    captured: Rc<RefCell<Captured>>,
    fair_locking: bool,
}

impl MockRuntime {
    /// 构造 Mock 并返回共享的 `Captured` 句柄。
    fn new(session: SessionState, timestamps: &[u64]) -> (Self, Rc<RefCell<Captured>>) {
        let captured = Rc::new(RefCell::new(Captured::default()));
        (
            Self {
                session,
                timestamps: timestamps.iter().copied().collect(),
                captured: captured.clone(),
                fair_locking: false,
            },
            captured,
        )
    }

    /// 弹出下一个预置时间戳；队列空则报错。
    fn next_timestamp(&mut self) -> Result<u64, TxnError> {
        self.timestamps
            .pop_front()
            .ok_or_else(|| TxnError::new(TxnErrorKind::Runtime, "mock timestamp queue is empty"))
    }
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
            version: 42,
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
        "zone-a".into()
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

/// 计划树测试双，仅携带 `PlanKind` 与子节点。
struct TestPlan {
    kind: PlanKind,
    children: Vec<TestPlan>,
}

/// 语句测试双：元组字段表示是否只读。
struct TestStatement(bool);

impl StatementInspection for TestStatement {
    fn is_read_only(&self) -> bool {
        self.0
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

/// 辅助：对计划调用乐观 Provider 的 `AdviseOptimizeWithPlan`，返回是否启用 max-ts。
fn optimistic_accepts(plan: TestPlan) -> bool {
    let (runtime, _) = MockRuntime::new(SessionState::default(), &[]);
    let mut provider = NewOptimisticTxnContextProvider(Box::new(runtime), false);
    provider.AdviseOptimizeWithPlan(&plan).unwrap();
    provider.optimize_with_max_ts
}

/// 激活与提交选项应保留会话上的副本读、拦截器、异步提交等配置。
#[test]
fn activation_and_commit_options_preserve_session_configuration() {
    let session = SessionState {
        autocommit: false,
        in_txn: true,
        replica_read: ReplicaReadMode::Follower,
        has_snapshot_interceptor: true,
        weak_consistency: true,
        assertion_level: AssertionLevel::Strict,
        restricted_sql: true,
        request_source_type: "ddl".into(),
        explicit_request_source_type: "br".into(),
        load_based_replica_read_threshold: 7,
        enable_async_commit: true,
        enable_one_pc: true,
        disk_full_option: DiskFullOption::AllowedOnAlmostFull,
        connection_id: 99,
        has_rpc_interceptor: true,
        has_resource_group_tagger: true,
        table_delta_ids: vec![1, 2, 3],
        temporary_table_ids: vec![2],
        temporary_table_count: 1,
        cdc_write_source: 8,
        ..SessionState::default()
    };
    let (runtime, captured) = MockRuntime::new(session, &[100]);
    let mut provider = NewRegisteredTxnContextProvider(
        ProviderKind::PessimisticRepeatableRead,
        Box::new(runtime),
        false,
    );
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::Default)
        .unwrap();
    let activation = captured.borrow().activation.clone().unwrap();
    assert!(activation.pessimistic && activation.weak_consistency);
    assert_eq!(activation.replica_read, ReplicaReadMode::Follower);
    assert_eq!(activation.session_id, 99);
    assert!(activation.enable_async_commit && activation.enable_one_pc);
    assert!(activation.install_snapshot_interceptor);
    assert!(activation.install_rpc_interceptor);
    assert!(activation.install_resource_group_tagger);
    assert!(activation.guarantee_linearizability);
    assert_eq!(activation.txn_scope, "zone-a");

    provider.SetOptionsBeforeCommit(true).unwrap();
    let commit = captured.borrow().commit.clone().unwrap();
    assert_eq!(commit.related_physical_table_ids, vec![1, 3]);
    assert_eq!(commit.temporary_table_ids, vec![2]);
    assert_eq!(commit.cdc_write_source, 8);
    assert!(commit.has_commit_ts_checker);
}

/// InfoSchema：普通 schema 只扩展一次；若会话已有 snapshot schema 则直接使用。
#[test]
fn info_schema_prefers_snapshot_and_extends_normal_schema_once() {
    let (runtime, captured) = MockRuntime::new(SessionState::default(), &[1]);
    let mut provider =
        NewRegisteredTxnContextProvider(ProviderKind::Optimistic, Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::Default)
        .unwrap();
    assert!(provider.GetTxnInfoSchema().is_session_extended());
    assert!(provider.GetTxnInfoSchema().is_session_extended());
    assert_eq!(
        captured
            .borrow()
            .events
            .iter()
            .filter(|event| event.as_str() == "extend-schema")
            .count(),
        1
    );

    let snapshot: TxnInfoSchemaRef = Rc::new(MockInfoSchema {
        version: 77,
        extended: false,
    });
    let session = SessionState {
        snapshot_info_schema: Some(snapshot),
        ..SessionState::default()
    };
    let (runtime, captured) = MockRuntime::new(session, &[2]);
    let mut provider =
        NewRegisteredTxnContextProvider(ProviderKind::Optimistic, Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::Default)
        .unwrap();
    assert_eq!(provider.GetTxnInfoSchema().schema_meta_version(), 77);
    assert!(
        !captured
            .borrow()
            .events
            .iter()
            .any(|event| event == "extend-schema")
    );
}

/// RC：复用最新时间戳，且读快照带 `rc_check_ts` 标记。
#[test]
fn rc_reuses_latest_timestamp_and_marks_check_ts_snapshot() {
    let session = SessionState {
        connection_id: 1,
        in_txn: true,
        rc_read_check_ts_enabled: true,
        rc_write_check_ts: true,
        ..SessionState::default()
    };
    let (runtime, _) = MockRuntime::new(session, &[10, 20]);
    let mut provider = NewPessimisticRCTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::BeforeStmt)
        .unwrap();
    provider
        .OnStmtStart(RuntimeContext::default(), &TestStatement(true))
        .unwrap();
    assert_eq!(provider.GetStmtReadTS().unwrap(), 10);
    let snapshot = provider.GetSnapshotWithStmtReadTS().unwrap();
    assert!(snapshot.rc_check_ts);
    assert_eq!(snapshot.isolation, IsolationLevel::ReadCommitted);
}

/// RR：写冲突后重试刷新 for_update_ts，并走公平锁 start/retry。
#[test]
fn repeatable_read_refreshes_ts_and_preserves_it_for_retry() {
    let session = SessionState {
        connection_id: 7,
        pessimistic_fair_locking: true,
        lock_wait_timeout_ms: 100,
        lock_wait_elapsed_ms: 1,
        ..SessionState::default()
    };
    let (runtime, captured) = MockRuntime::new(session, &[10, 20]);
    let mut provider = NewPessimisticRRTxnContextProvider(Box::new(runtime), false);
    provider
        .OnInitialize(RuntimeContext::default(), EnterNewTxnType::Default)
        .unwrap();
    provider
        .base
        .OnPessimisticStmtStart(RuntimeContext::default())
        .unwrap();
    let action = provider.OnStmtErrorForNextAction(
        RuntimeContext::default(),
        StmtErrorHandlePoint::AfterPessimisticLock,
        TxnError::new(TxnErrorKind::WriteConflict, "conflict"),
    );
    assert_eq!(action, StmtErrorAction::RetryReady);
    provider.OnStmtRetry(RuntimeContext::default()).unwrap();
    assert_eq!(provider.for_update_ts, 20);
    assert_eq!(
        captured.borrow().events,
        vec![
            "commit-old",
            "activate:10",
            "fair-start",
            "snapshot-ts:20",
            "fair-retry"
        ]
    );
}

/// RR/RC 计划谓词对 Update 下未加锁 PointGet、Insert、BatchPointGet 的判定不同。
#[test]
fn plan_inspection_keeps_rr_and_rc_rules_distinct() {
    let unlocked_point_under_update = TestPlan {
        kind: PlanKind::Update,
        children: vec![TestPlan {
            kind: PlanKind::PointGet {
                lock: false,
                no_second_read: true,
                cache_table: false,
            },
            children: Vec::new(),
        }],
    };
    assert!(!NotNeedGetLatestTSFromPD(
        &unlocked_point_under_update,
        false
    ));
    assert!(PlanSkipGetTSOFromPD(
        true,
        &unlocked_point_under_update,
        false
    ));
    assert!(!PlanSkipGetTSOFromPD(
        false,
        &unlocked_point_under_update,
        false
    ));
    let insert = TestPlan {
        kind: PlanKind::Insert {
            has_select: false,
            on_duplicate: true,
            replace: false,
        },
        children: Vec::new(),
    };
    assert!(NotNeedGetLatestTSFromPD(&insert, false));
    assert!(!PlanSkipGetTSOFromPD(true, &insert, false));
    let batch = TestPlan {
        kind: PlanKind::BatchPointGet { lock: true },
        children: Vec::new(),
    };
    assert!(NotNeedGetLatestTSFromPD(&batch, false));
    assert!(!PlanSkipGetTSOFromPD(true, &batch, false));
}

/// 乐观 max-ts 优化仅接受与 Go 一致的点查计划谓词。
#[test]
fn optimistic_max_ts_uses_exact_common_plan_predicates() {
    let point = |no_second_read, cache_table| TestPlan {
        kind: PlanKind::PointGet {
            lock: false,
            no_second_read,
            cache_table,
        },
        children: Vec::new(),
    };
    assert!(optimistic_accepts(point(true, false)));
    assert!(!optimistic_accepts(point(false, false)));
    assert!(!optimistic_accepts(point(true, true)));
    assert!(!optimistic_accepts(TestPlan {
        kind: PlanKind::BatchPointGet { lock: false },
        children: Vec::new(),
    }));
    assert!(optimistic_accepts(TestPlan {
        kind: PlanKind::Projection,
        children: vec![TestPlan {
            kind: PlanKind::PhysicalIndexReader {
                unique_point_get: true
            },
            children: Vec::new(),
        }],
    }));
    assert!(!optimistic_accepts(TestPlan {
        kind: PlanKind::PhysicalIndexReader {
            unique_point_get: false
        },
        children: Vec::new(),
    }));
    assert!(optimistic_accepts(TestPlan {
        kind: PlanKind::PhysicalTableReader {
            primary_key_point_get: true
        },
        children: Vec::new(),
    }));
    assert!(!optimistic_accepts(TestPlan {
        kind: PlanKind::PhysicalTableReader {
            primary_key_point_get: false
        },
        children: Vec::new(),
    }));
}

/// 注册表能为四种 ProviderKind 均成功初始化并取得读 TS。
#[test]
fn registry_constructs_every_provider() {
    for kind in [
        ProviderKind::Optimistic,
        ProviderKind::PessimisticReadCommitted,
        ProviderKind::PessimisticRepeatableRead,
        ProviderKind::PessimisticSerializable,
    ] {
        let (runtime, _) = MockRuntime::new(SessionState::default(), &[1, 2]);
        let mut provider = NewRegisteredTxnContextProvider(kind, Box::new(runtime), false);
        provider
            .OnInitialize(RuntimeContext::default(), EnterNewTxnType::Default)
            .unwrap();
        provider
            .OnStmtStart(RuntimeContext::default(), &TestStatement(false))
            .unwrap();
        assert!(provider.GetStmtReadTS().unwrap() > 0);
    }
}

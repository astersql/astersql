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

// 隔离级别事务上下文的基础类型与 `BaseTxnContextProvider`。
//
// 定义运行时上下文、隔离级别、会话/事务状态、计划/语句探查 trait，
// 以及与 Go `sessiontxn/isolation` 基类对齐的事务准备、激活、快照与提交选项逻辑。
// 时间戳（TS）由 Oracle（如 PD）分配，用于 MVCC（多版本并发控制）读可见性。
// 基类负责“准备时间戳 -> 激活事务 -> 语句生命周期回调”的主骨架，子类只覆盖差异。

use std::fmt;
use std::rc::Rc;

/// 最大时间戳常量，常用于点查优化时作为读 TS（可见所有已提交版本）。
pub const MAX_TIMESTAMP: u64 = u64::MAX;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 语句/事务操作携带的运行时上下文（请求 ID、是否已取消）。
pub struct RuntimeContext {
    pub request_id: String,
    pub cancelled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 进入新事务的方式：默认、显式 BEGIN、语句前惰性进入。
pub enum EnterNewTxnType {
    Default,
    WithBeginStmt,
    BeforeStmt,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// SQL 事务隔离级别（乐观 / 读已提交 / 可重复读 / 可串行化）。
pub enum IsolationLevel {
    Optimistic,
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 副本读模式：主、从、就近。
pub enum ReplicaReadMode {
    Leader,
    Follower,
    Closest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 写路径键值断言级别（关闭 / 快速 / 严格）。
pub enum AssertionLevel {
    Off,
    Fast,
    Strict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 磁盘将满时是否仍允许写入。
pub enum DiskFullOption {
    NotAllowed,
    AllowedOnAlmostFull,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 两阶段提交（2PC）预写阶段遇到锁时的策略：尝试解析或不解析。
pub enum PrewriteEncounterLockPolicy {
    TryResolve,
    NoResolve,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 语句错误处理切入点（查询后 / 悲观锁后 / 其他）。
pub enum StmtErrorHandlePoint {
    AfterQuery,
    AfterPessimisticLock,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 事务错误分类：写冲突、死锁、锁等待超时等。
pub enum TxnErrorKind {
    Runtime,
    InvalidTransaction,
    WriteConflict,
    Deadlock { retryable: bool },
    LockWaitTimeout,
    Lock,
    UnsupportedCommitOption,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 带种类与消息的事务错误。
pub struct TxnError {
    pub kind: TxnErrorKind,
    pub message: String,
}

impl TxnError {
    /// 构造 `TxnError`。
    pub fn new(kind: TxnErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for TxnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TxnError {}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 语句出错后的下一步动作：报错、准备重试、或交给上层决定。
pub enum StmtErrorAction {
    Error(TxnError),
    RetryReady,
    NoIdea,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 当前事务上下文状态（start_ts、for_update_ts、隔离级别、锁缓存等）。
pub struct TxnContextState {
    pub start_ts: u64,
    pub for_update_ts: u64,
    pub txn_scope: String,
    pub info_schema_version: u64,
    pub is_pessimistic: bool,
    pub isolation: Option<IsolationLevel>,
    pub could_retry: bool,
    pub has_history: bool,
    pub is_staleness: bool,
    pub current_stmt_lock_cache: bool,
    pub flushed_stmt_lock_cache: bool,
}

#[derive(Clone, Debug)]
/// 会话侧事务相关状态：快照 TS、自动提交、重试、RC 检查 TS、流水线等。
pub struct SessionState {
    pub txn: TxnContextState,
    pub snapshot_ts: u64,
    pub snapshot_info_schema: Option<TxnInfoSchemaRef>,
    pub last_commit_ts: u64,
    pub in_txn: bool,
    pub autocommit: bool,
    pub restricted_sql: bool,
    pub disable_txn_auto_retry: bool,
    pub retry_limit: i64,
    pub retrying: bool,
    pub stale_read: bool,
    pub use_low_resolution_tso: bool,
    pub connection_id: u64,
    pub rc_read_check_ts_enabled: bool,
    pub rc_write_check_ts: bool,
    pub stmt_rc_check_ts: bool,
    pub lock_wait_timeout_ms: u64,
    pub lock_wait_elapsed_ms: u64,
    pub pessimistic_fair_locking: bool,
    pub pipelined: bool,
    pub enable_mdl: bool,
    pub temporary_table_count: usize,
    pub cdc_write_source: u64,
    pub replica_read: ReplicaReadMode,
    pub has_snapshot_interceptor: bool,
    pub weak_consistency: bool,
    pub assertion_level: AssertionLevel,
    pub request_source_type: String,
    pub explicit_request_source_type: String,
    pub load_based_replica_read_threshold: u64,
    pub enable_async_commit: bool,
    pub enable_one_pc: bool,
    pub disk_full_option: DiskFullOption,
    pub guarantee_linearizability: bool,
    pub has_rpc_interceptor: bool,
    pub has_resource_group_tagger: bool,
    pub table_delta_ids: Vec<i64>,
    pub temporary_table_ids: Vec<i64>,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            txn: TxnContextState {
                txn_scope: "global".to_owned(),
                ..TxnContextState::default()
            },
            snapshot_ts: 0,
            snapshot_info_schema: None,
            last_commit_ts: 0,
            in_txn: false,
            autocommit: true,
            restricted_sql: false,
            disable_txn_auto_retry: false,
            retry_limit: 10,
            retrying: false,
            stale_read: false,
            use_low_resolution_tso: false,
            connection_id: 0,
            rc_read_check_ts_enabled: false,
            rc_write_check_ts: false,
            stmt_rc_check_ts: false,
            lock_wait_timeout_ms: 50_000,
            lock_wait_elapsed_ms: 0,
            pessimistic_fair_locking: false,
            pipelined: false,
            enable_mdl: true,
            temporary_table_count: 0,
            cdc_write_source: 0,
            replica_read: ReplicaReadMode::Leader,
            has_snapshot_interceptor: false,
            weak_consistency: false,
            assertion_level: AssertionLevel::Off,
            request_source_type: String::new(),
            explicit_request_source_type: String::new(),
            load_based_replica_read_threshold: 0,
            enable_async_commit: false,
            enable_one_pc: false,
            disk_full_option: DiskFullOption::NotAllowed,
            guarantee_linearizability: true,
            has_rpc_interceptor: false,
            has_resource_group_tagger: false,
            table_delta_ids: Vec::new(),
            temporary_table_ids: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 执行计划（Plan）形状摘要，供隔离路径做 TS/优化决策而不复制整棵计划树。
pub enum PlanKind {
    PointGet {
        lock: bool,
        no_second_read: bool,
        cache_table: bool,
    },
    BatchPointGet {
        lock: bool,
    },
    Projection,
    PhysicalIndexReader {
        unique_point_get: bool,
    },
    PhysicalTableReader {
        primary_key_point_get: bool,
    },
    Physical {
        lock: bool,
    },
    Update,
    Delete,
    Insert {
        has_select: bool,
        on_duplicate: bool,
        replace: bool,
    },
    Execute,
    Other,
}

/// Adapter implemented by real planner plans without copying their topology.
/// 由真实优化器计划实现的适配器，避免复制计划拓扑。
pub trait PlanInspection {
    fn kind(&self) -> PlanKind;
    fn children(&self) -> Vec<&dyn PlanInspection>;
}

/// Adapter implemented by parser AST statement nodes.
/// 由解析器 AST 语句节点实现的适配器。
pub trait StatementInspection {
    fn is_read_only(&self) -> bool;
}

/// 异步时间戳 Future：调用 `wait` 取得 Oracle 时间戳。
pub trait TimestampFuture {
    fn wait(&mut self) -> Result<u64, TxnError>;
}

/// 事务可见的 InfoSchema（信息模式）抽象。
pub trait TxnInfoSchema: fmt::Debug {
    fn schema_meta_version(&self) -> u64;
    fn is_session_extended(&self) -> bool;
}

/// InfoSchema 的引用计数句柄。
pub type TxnInfoSchemaRef = Rc<dyn TxnInfoSchema>;

/// 立即返回常量时间戳的 Future。
pub struct ConstantFuture(pub u64);

impl TimestampFuture for ConstantFuture {
    fn wait(&mut self) -> Result<u64, TxnError> {
        Ok(self.0)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 激活底层 KV 事务时传入的选项集合。
pub struct TxnActivationOptions {
    pub pessimistic: bool,
    pub isolation: IsolationLevel,
    pub causal_consistency_only: bool,
    pub pipelined: bool,
    pub txn_scope: String,
    pub replica_read: ReplicaReadMode,
    pub install_snapshot_interceptor: bool,
    pub weak_consistency: bool,
    pub assertion_level: AssertionLevel,
    pub request_source_internal: bool,
    pub request_source_type: String,
    pub explicit_request_source_type: String,
    pub load_based_replica_read_threshold: u64,
    pub enable_async_commit: bool,
    pub enable_one_pc: bool,
    pub disk_full_option: DiskFullOption,
    pub info_schema_version: u64,
    pub session_id: u64,
    pub install_rpc_interceptor: bool,
    pub install_resource_group_tagger: bool,
    pub guarantee_linearizability: bool,
    pub install_commit_hook: bool,
    pub install_background_lifecycle_hooks: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 提交前设置的选项（schema 校验、临时表过滤、预写锁策略等）。
pub struct CommitOptions {
    pub pipelined_noop: bool,
    pub schema_version: u64,
    pub check_schema_by_delta: bool,
    pub related_physical_table_ids: Vec<i64>,
    pub temporary_table_ids: Vec<i64>,
    pub install_temporary_table_filter: bool,
    pub install_rpc_interceptor: bool,
    pub install_resource_group_tagger: bool,
    pub cdc_write_source: u64,
    pub has_commit_ts_checker: bool,
    pub prewrite_lock_policy: PrewriteEncounterLockPolicy,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 读快照描述：时间戳、隔离级别、是否来自活跃事务、RC check_ts 标记。
pub struct Snapshot {
    pub timestamp: u64,
    pub isolation: IsolationLevel,
    pub from_active_transaction: bool,
    pub rc_check_ts: bool,
}

/// Strong integration boundary implemented by the future sessiontxn runtime.
/// 未来 sessiontxn 运行时实现的强集成边界。
/// 隔离级别状态机与真实会话/存储之间的强集成边界。
pub trait IsolationRuntime {
    fn session(&self) -> &SessionState;
    fn session_mut(&mut self) -> &mut SessionState;
    fn latest_info_schema(&self) -> TxnInfoSchemaRef;
    fn ensure_session_extended_info_schema(
        &mut self,
        info_schema: &TxnInfoSchemaRef,
    ) -> TxnInfoSchemaRef;
    fn configured_txn_scope(&self) -> String;
    fn existing_transaction_start_ts(&self) -> Option<u64>;
    fn take_prepared_timestamp_future(&mut self) -> Option<Box<dyn TimestampFuture>>;
    fn commit_before_enter_new_txn(&mut self, context: &RuntimeContext) -> Result<(), TxnError>;
    fn oracle_future(
        &mut self,
        context: &RuntimeContext,
        scope: &str,
        low_resolution: bool,
    ) -> Result<Box<dyn TimestampFuture>, TxnError>;
    fn latest_timestamp(&mut self, context: &RuntimeContext, scope: &str) -> Result<u64, TxnError>;
    fn activate_transaction(
        &mut self,
        context: &RuntimeContext,
        start_ts: u64,
        options: &TxnActivationOptions,
    ) -> Result<(), TxnError>;
    fn set_transaction_snapshot_ts(&mut self, timestamp: u64) -> Result<(), TxnError>;
    fn snapshot(&mut self, timestamp: u64, from_active: bool) -> Result<Snapshot, TxnError>;
    fn set_commit_options(&mut self, options: &CommitOptions) -> Result<(), TxnError>;
    fn attach_local_temporary_tables(&mut self, info_schema: &TxnInfoSchemaRef)
    -> TxnInfoSchemaRef;
    fn start_fair_locking(&mut self) -> Result<(), TxnError>;
    fn done_fair_locking(&mut self, context: &RuntimeContext) -> Result<(), TxnError>;
    fn cancel_fair_locking(&mut self, context: &RuntimeContext) -> Result<(), TxnError>;
    fn retry_fair_locking(&mut self, context: &RuntimeContext) -> Result<(), TxnError>;
    fn is_in_fair_locking_mode(&self) -> bool;
}

/// 各隔离级别提供者共享的基类：准备 TS、激活事务、快照与提交选项。
pub struct BaseTxnContextProvider {
    pub(crate) runtime: Box<dyn IsolationRuntime>,
    pub causal_consistency_only: bool,
    pub isolation: IsolationLevel,
    pub pessimistic: bool,
    pub info_schema: Option<TxnInfoSchemaRef>,
    pub is_txn_prepared: bool,
    pub txn_active: bool,
    pub enter_new_txn_type: EnterNewTxnType,
    pub const_start_ts: u64,
    pub context: RuntimeContext,
    prepared_future: Option<Box<dyn TimestampFuture>>,
}

impl BaseTxnContextProvider {
    /// 构造基类提供者：指定隔离级别、是否悲观、是否仅因果一致。
    pub fn new(
        runtime: Box<dyn IsolationRuntime>,
        isolation: IsolationLevel,
        pessimistic: bool,
        causal_consistency_only: bool,
    ) -> Self {
        Self {
            runtime,
            causal_consistency_only,
            isolation,
            pessimistic,
            info_schema: None,
            is_txn_prepared: false,
            txn_active: false,
            enter_new_txn_type: EnterNewTxnType::Default,
            const_start_ts: 0,
            context: RuntimeContext::default(),
            prepared_future: None,
        }
    }

    /// 按进入方式初始化：必要时提交旧事务、准备 Oracle TS、填充 TxnContextState 并可选激活。
    pub fn OnInitialize(
        &mut self,
        context: RuntimeContext,
        enter: EnterNewTxnType,
    ) -> Result<(), TxnError> {
        self.context = context;
        // 按进入类型决定是否立即激活；BeforeStmt 仅准备状态。
        let activate_now = match enter {
            EnterNewTxnType::Default => {
                self.runtime.commit_before_enter_new_txn(&self.context)?;
                self.PrepareTxnWithOracleTS()?;
                true
            }
            EnterNewTxnType::WithBeginStmt => {
                if !self.CanReuseTxnWhenExplicitBegin() {
                    self.runtime.commit_before_enter_new_txn(&self.context)?;
                    self.PrepareTxnWithOracleTS()?;
                }
                self.runtime.session_mut().in_txn = true;
                true
            }
            EnterNewTxnType::BeforeStmt => false,
        };
        self.enter_new_txn_type = enter;
        self.info_schema = Some(self.runtime.latest_info_schema());
        let scope = self.runtime.configured_txn_scope();
        self.runtime.session_mut().txn = TxnContextState {
            txn_scope: scope,
            info_schema_version: self
                .info_schema
                .as_ref()
                .map_or(0, |schema| schema.schema_meta_version()),
            is_pessimistic: self.pessimistic,
            isolation: Some(self.isolation),
            ..TxnContextState::default()
        };
        if !self.is_txn_prepared {
            if let Some(start_ts) = self.runtime.existing_transaction_start_ts() {
                self.ReplaceTxnTsFuture(Box::new(ConstantFuture(start_ts)))?;
            } else if let Some(future) = self.runtime.take_prepared_timestamp_future() {
                self.ReplaceTxnTsFuture(future)?;
            }
        }
        if activate_now {
            self.ActivateTxn()?;
        }
        Ok(())
    }

    /// 显式 BEGIN 时是否可复用当前事务（无历史、非过期读、无 snapshot_ts）。
    pub fn CanReuseTxnWhenExplicitBegin(&self) -> bool {
        let session = self.runtime.session();
        !session.txn.has_history && !session.txn.is_staleness && session.snapshot_ts == 0
    }

    /// 取事务 InfoSchema：优先会话快照 schema，否则确保会话扩展后缓存。
    pub fn GetTxnInfoSchema(&mut self) -> TxnInfoSchemaRef {
        if let Some(info_schema) = &self.runtime.session().snapshot_info_schema {
            return info_schema.clone();
        }
        let current = self
            .info_schema
            .as_ref()
            .expect("provider must be initialized before reading info schema")
            .clone();
        if current.is_session_extended() {
            return current;
        }
        let extended = self.runtime.ensure_session_extended_info_schema(&current);
        self.runtime.session_mut().txn.info_schema_version = extended.schema_meta_version();
        self.info_schema = Some(extended.clone());
        extended
    }

    /// 返回当前事务作用域字符串。
    pub fn GetTxnScope(&self) -> String {
        self.runtime.session().txn.txn_scope.clone()
    }

    /// 读副本作用域：非 global 事务 scope，或 Closest 副本读时用配置 scope，否则 global。
    pub fn GetReadReplicaScope(&self) -> String {
        let scope = self.GetTxnScope();
        if !scope.is_empty() && scope != "global" {
            scope
        } else if self.runtime.session().replica_read == ReplicaReadMode::Closest {
            self.runtime.configured_txn_scope()
        } else {
            "global".to_owned()
        }
    }

    /// 准备事务时间戳 Future：有 snapshot_ts 则用常量，否则向 Oracle 取。
    pub fn PrepareTxn(&mut self) -> Result<(), TxnError> {
        if self.is_txn_prepared {
            return Ok(());
        }
        let snapshot_ts = self.runtime.session().snapshot_ts;
        if snapshot_ts != 0 {
            return self.ReplaceTxnTsFuture(Box::new(ConstantFuture(snapshot_ts)));
        }
        self.PrepareTxnWithOracleTS()
    }

    /// 向 Oracle 申请时间戳 Future 并标记已准备。
    pub fn PrepareTxnWithOracleTS(&mut self) -> Result<(), TxnError> {
        if self.is_txn_prepared {
            return Ok(());
        }
        let scope = self.runtime.session().txn.txn_scope.clone();
        let low_resolution = self.runtime.session().use_low_resolution_tso;
        let future = self
            .runtime
            .oracle_future(&self.context, &scope, low_resolution)?;
        self.ReplaceTxnTsFuture(future)
    }

    /// 替换尚未激活事务的 TS Future；已激活则忽略。
    pub fn ReplaceTxnTsFuture(&mut self, future: Box<dyn TimestampFuture>) -> Result<(), TxnError> {
        if self.txn_active {
            return Ok(());
        }
        self.prepared_future = Some(future);
        self.is_txn_prepared = true;
        Ok(())
    }

    /// 强制使用常量 start TS（如 MAX_TIMESTAMP 优化）；激活后不可再强制。
    pub fn ForcePrepareConstStartTS(&mut self, timestamp: u64) -> Result<(), TxnError> {
        if self.txn_active {
            return Err(TxnError::new(
                TxnErrorKind::InvalidTransaction,
                "cannot force a constant start timestamp after activation",
            ));
        }
        self.const_start_ts = timestamp;
        self.ReplaceTxnTsFuture(Box::new(ConstantFuture(timestamp)))
    }

    /// 等待 TS、校验与 last_commit_ts 关系、组装激活选项并真正激活 KV 事务。
    pub fn ActivateTxn(&mut self) -> Result<u64, TxnError> {
        if self.txn_active {
            return Ok(self.runtime.session().txn.start_ts);
        }
        self.PrepareTxn()?;
        if self.const_start_ts != 0 {
            self.prepared_future = Some(Box::new(ConstantFuture(self.const_start_ts)));
        }
        let start_ts = self
            .prepared_future
            .as_mut()
            .expect("prepared transaction must have a timestamp future")
            .wait()?;
        // 非预设 TS 时，start_ts 不得早于会话上次提交 TS（保证因果序）。
        let preset = self.const_start_ts != 0 || self.runtime.session().snapshot_ts != 0;
        if !preset && self.runtime.session().last_commit_ts > start_ts {
            return Err(TxnError::new(
                TxnErrorKind::Runtime,
                format!(
                    "txn start_ts:{start_ts} is before session last_commit_ts:{}",
                    self.runtime.session().last_commit_ts
                ),
            ));
        }
        if self.enter_new_txn_type == EnterNewTxnType::BeforeStmt
            && !self.runtime.session().autocommit
            && self.runtime.session().snapshot_ts == 0
        {
            self.runtime.session_mut().in_txn = true;
        }
        let session = self.runtime.session();
        // 线性一致性：非仅因果一致，且会话要求，并满足 autocommit/快照/进入方式条件。
        let guarantee_linearizability = !self.causal_consistency_only
            && session.guarantee_linearizability
            && (!session.autocommit
                || session.snapshot_ts > 0
                || matches!(
                    self.enter_new_txn_type,
                    EnterNewTxnType::Default | EnterNewTxnType::WithBeginStmt
                ));
        let options = TxnActivationOptions {
            pessimistic: self.pessimistic,
            isolation: self.isolation,
            causal_consistency_only: self.causal_consistency_only,
            pipelined: session.pipelined,
            txn_scope: session.txn.txn_scope.clone(),
            replica_read: session.replica_read,
            install_snapshot_interceptor: session.has_snapshot_interceptor,
            weak_consistency: session.weak_consistency,
            assertion_level: session.assertion_level,
            request_source_internal: session.restricted_sql,
            request_source_type: if session.pipelined {
                "p-dml".to_owned()
            } else {
                session.request_source_type.clone()
            },
            explicit_request_source_type: session.explicit_request_source_type.clone(),
            load_based_replica_read_threshold: session.load_based_replica_read_threshold,
            enable_async_commit: session.enable_async_commit,
            enable_one_pc: session.enable_one_pc,
            disk_full_option: session.disk_full_option,
            info_schema_version: self
                .info_schema
                .as_ref()
                .map_or(0, |schema| schema.schema_meta_version()),
            session_id: session.connection_id,
            install_rpc_interceptor: session.has_rpc_interceptor,
            install_resource_group_tagger: session.has_resource_group_tagger,
            guarantee_linearizability,
            install_commit_hook: true,
            install_background_lifecycle_hooks: true,
        };
        self.runtime
            .activate_transaction(&self.context, start_ts, &options)?;
        let txn = &mut self.runtime.session_mut().txn;
        txn.start_ts = start_ts;
        txn.for_update_ts = start_ts;
        txn.is_pessimistic = self.pessimistic;
        txn.isolation = Some(self.isolation);
        self.txn_active = true;
        Ok(start_ts)
    }

    /// 确保事务已激活并返回 start_ts。
    pub fn GetTxnStartTS(&mut self) -> Result<u64, TxnError> {
        self.ActivateTxn()
    }

    /// 语句读时间戳：有 snapshot_ts 用其，否则用事务 start_ts。
    pub fn GetStmtReadTS(&mut self) -> Result<u64, TxnError> {
        self.ActivateTxn()?;
        Ok(if self.runtime.session().snapshot_ts != 0 {
            self.runtime.session().snapshot_ts
        } else {
            self.runtime.session().txn.start_ts
        })
    }

    /// 基类中 for-update TS 与读 TS 相同（子类可覆盖）。
    pub fn GetStmtForUpdateTS(&mut self) -> Result<u64, TxnError> {
        self.GetStmtReadTS()
    }

    /// 语句开始：更新上下文。
    pub fn OnStmtStart(
        &mut self,
        context: RuntimeContext,
        _statement: &dyn StatementInspection,
    ) -> Result<(), TxnError> {
        self.context = context;
        Ok(())
    }

    /// 语句重试：清除当前语句锁缓存标记。
    pub fn OnStmtRetry(&mut self, context: RuntimeContext) -> Result<(), TxnError> {
        self.context = context;
        self.runtime.session_mut().txn.current_stmt_lock_cache = false;
        Ok(())
    }

    /// 语句提交钩子（基类无额外操作）。
    pub fn OnStmtCommit(&mut self, _context: RuntimeContext) -> Result<(), TxnError> {
        Ok(())
    }

    /// 语句回滚钩子（基类无额外操作）。
    pub fn OnStmtRollback(
        &mut self,
        _context: RuntimeContext,
        _pessimistic_retry: bool,
    ) -> Result<(), TxnError> {
        Ok(())
    }

    /// 基类错误策略：悲观锁后直接报错，其余交上层（NoIdea）。
    pub fn OnStmtErrorForNextAction(
        &self,
        point: StmtErrorHandlePoint,
        error: TxnError,
    ) -> StmtErrorAction {
        match point {
            StmtErrorHandlePoint::AfterPessimisticLock => StmtErrorAction::Error(error),
            _ => StmtErrorAction::NoIdea,
        }
    }

    /// 预热建议：未准备且非过期读时提前 PrepareTxn。
    pub fn AdviseWarmup(&mut self) -> Result<(), TxnError> {
        if self.is_txn_prepared || self.runtime.session().stale_read {
            Ok(())
        } else {
            self.PrepareTxn()
        }
    }

    /// 会话是否已设置 snapshot_ts。
    pub fn IsSnapshotEnabled(&self) -> bool {
        self.runtime.session().snapshot_ts != 0
    }

    /// 是否以过期读（stale read）方式 BEGIN。
    pub fn IsBeginStmtWithStaleRead(&self) -> bool {
        self.runtime.session().stale_read
    }

    /// 按给定时间戳与隔离级别构造快照。
    pub fn GetSnapshotByTS(
        &mut self,
        timestamp: u64,
        isolation: IsolationLevel,
    ) -> Result<Snapshot, TxnError> {
        let from_active = self.txn_active
            && self.runtime.session().txn.start_ts == timestamp
            && self.runtime.session().txn.for_update_ts == timestamp;
        let mut snapshot = self.runtime.snapshot(timestamp, from_active)?;
        snapshot.isolation = isolation;
        Ok(snapshot)
    }

    /// 使用语句读 TS 构造当前隔离级别快照。
    pub fn GetSnapshotWithStmtReadTS(&mut self) -> Result<Snapshot, TxnError> {
        let timestamp = self.GetStmtReadTS()?;
        self.GetSnapshotByTS(timestamp, self.isolation)
    }

    /// 使用 for-update TS 构造当前隔离级别快照。
    pub fn GetSnapshotWithStmtForUpdateTS(&mut self) -> Result<Snapshot, TxnError> {
        let timestamp = self.GetStmtForUpdateTS()?;
        self.GetSnapshotByTS(timestamp, self.isolation)
    }

    /// 本地临时表创建后，将临时表挂到 InfoSchema 并更新版本。
    pub fn OnLocalTemporaryTableCreated(&mut self) {
        let info_schema = self.GetTxnInfoSchema();
        self.info_schema = Some(self.runtime.attach_local_temporary_tables(&info_schema));
        self.runtime.session_mut().txn.info_schema_version = self
            .info_schema
            .as_ref()
            .map_or(0, |schema| schema.schema_meta_version());
    }

    /// 提交前组装并下发 CommitOptions；流水线 DML 在不兼容配置下报错。
    pub fn SetOptionsBeforeCommit(
        &mut self,
        has_commit_ts_checker: bool,
    ) -> Result<CommitOptions, TxnError> {
        let schema_version = self.GetTxnInfoSchema().schema_meta_version();
        let session = self.runtime.session();
        // 流水线 DML 提交前检查 MDL/临时表/CDC/commit_ts_checker 等不兼容项。
        if session.pipelined {
            let unsupported = if !session.enable_mdl {
                Some("metadata locking is disabled")
            } else if session.temporary_table_count != 0 {
                Some("temporary tables are present")
            } else if session.cdc_write_source != 0 {
                Some("CDC write source is set")
            } else if has_commit_ts_checker {
                Some("commit timestamp checker is set")
            } else {
                None
            };
            if let Some(reason) = unsupported {
                return Err(TxnError::new(
                    TxnErrorKind::UnsupportedCommitOption,
                    format!("pipelined DML cannot commit because {reason}"),
                ));
            }
        }
        let temporary_table_ids = session.temporary_table_ids.clone();
        let related_physical_table_ids = session
            .table_delta_ids
            .iter()
            .copied()
            .filter(|id| !temporary_table_ids.contains(id))
            .collect();
        let options = CommitOptions {
            pipelined_noop: session.pipelined,
            schema_version,
            check_schema_by_delta: !session.enable_mdl,
            related_physical_table_ids,
            temporary_table_ids,
            install_temporary_table_filter: session.temporary_table_count != 0,
            install_rpc_interceptor: session.has_rpc_interceptor,
            install_resource_group_tagger: session.has_resource_group_tagger,
            cdc_write_source: session.cdc_write_source,
            has_commit_ts_checker,
            // 可重试的自动提交乐观事务对预写遇锁选择不解析，其余尝试解析。
            prewrite_lock_policy: if session.txn.could_retry
                && session.autocommit
                && !session.in_txn
                && !session.txn.is_pessimistic
            {
                PrewriteEncounterLockPolicy::NoResolve
            } else {
                PrewriteEncounterLockPolicy::TryResolve
            },
        };
        self.runtime.set_commit_options(&options)?;
        Ok(options)
    }
}

/// 悲观事务语句起止与公平锁（fair locking）相关的基类包装。
pub struct BasePessimisticTxnContextProvider {
    pub base: BaseTxnContextProvider,
}

impl BasePessimisticTxnContextProvider {
    /// 悲观语句开始：满足条件时启动公平锁。
    pub fn OnPessimisticStmtStart(&mut self, context: RuntimeContext) -> Result<(), TxnError> {
        self.base.context = context;
        let session = self.base.runtime.session();
        if self.base.txn_active
            && session.pessimistic_fair_locking
            && session.connection_id != 0
            && !session.restricted_sql
        {
            self.base.runtime.start_fair_locking()?;
        }
        Ok(())
    }

    /// 悲观语句结束：成功则完成公平锁并标记锁缓存已刷，失败则取消。
    pub fn OnPessimisticStmtEnd(
        &mut self,
        context: RuntimeContext,
        successful: bool,
    ) -> Result<(), TxnError> {
        self.base.context = context;
        if self.base.txn_active && self.base.runtime.is_in_fair_locking_mode() {
            if successful {
                self.base.runtime.done_fair_locking(&self.base.context)?;
            } else {
                self.base.runtime.cancel_fair_locking(&self.base.context)?;
            }
        }
        let txn = &mut self.base.runtime.session_mut().txn;
        if successful {
            txn.flushed_stmt_lock_cache = true;
        } else {
            txn.current_stmt_lock_cache = false;
        }
        Ok(())
    }

    /// 若仍在公平锁模式则发起重试。
    pub fn RetryFairLockingIfNeeded(&mut self) -> Result<(), TxnError> {
        if self.base.txn_active && self.base.runtime.is_in_fair_locking_mode() {
            self.base.runtime.retry_fair_locking(&self.base.context)?;
        }
        Ok(())
    }

    /// 若仍在公平锁模式则取消。
    pub fn CancelFairLockingIfNeeded(&mut self) -> Result<(), TxnError> {
        if self.base.txn_active && self.base.runtime.is_in_fair_locking_mode() {
            self.base.runtime.cancel_fair_locking(&self.base.context)?;
        }
        Ok(())
    }
}

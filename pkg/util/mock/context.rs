// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Mock 会话上下文（Session Context）实现。
//
// 对应 Go `context.go`：为 util 包单测提供可配置的会话变量、假/真事务、
// InfoSchema 工厂钩子以及大量未实现接口的固定默认行为。

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use chrono::FixedOffset;
use thiserror::Error;

use crate::{kv, sessionctx, sli, vardef, variable};

/// 可跨线程共享的 `Any` 句柄别名。
pub type SharedAny = Arc<dyn Any + Send + Sync>;

#[derive(Debug, Error)]
/// Mock 层错误类型：未支持、无效事务、缺少 Store、消息包装与 KV 错误。
pub enum MockError {
    #[error("Not Supported")]
    NotSupported,
    #[error("invalid transaction")]
    InvalidTransaction,
    #[error("mock.Context has no store for a pending transaction")]
    MissingStore,
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Kv(#[from] kv::errors::SharedError),
}

/// Mock 操作结果别名。
pub type MockResult<T> = Result<T, MockError>;

/// Mock 会话变量：内嵌正式 `SessionVars`，并携带 chunk/TiFlash 等测试常用字段。
/// SessionVars：会话级系统变量与执行状态；Chunk：列式批处理行组。
/// MockSessionVars carries the session state used by this mock while retaining
/// the completed variable package as the authoritative system-variable store.
pub struct MockSessionVars {
    pub Inner: variable::session::SessionVars,
    pub InitChunkSize: usize,
    pub MaxChunkSize: usize,
    pub TimeZone: FixedOffset,
    pub EnablePaging: bool,
    pub MinPagingSize: i64,
    pub EnableChunkRPC: bool,
    pub DivPrecisionIncrement: i64,
    pub EnabledRateLimitAction: bool,
    pub OriginalSQL: String,
    pub RegardNULLAsPoint: bool,
    pub OptPrefixIndexSingleScan: bool,
    pub TiFlashFastScan: bool,
    pub TiFlashFineGrainedShuffleBatchSize: u64,
    pub GroupConcatMaxLen: u64,
    pub InExplainStmt: bool,
    pub TiFlashMaxThreads: i64,
    pub TiFlashMaxBytesBeforeExternalJoin: i64,
    pub TiFlashMaxBytesBeforeExternalGroupBy: i64,
    pub TiFlashMaxBytesBeforeExternalSort: i64,
    pub TiFlashMaxQueryMemoryPerNode: i64,
    pub TiFlashQuerySpillRatio: f64,
    pub TiFlashHashJoinVersion: String,
    pub ResourceGroupName: String,
}

impl Default for MockSessionVars {
    fn default() -> Self {
        let mut inner = variable::session::SessionVars::new();
        inner
            .SetSystemVar(vardef::MaxAllowedPacket, "67108864")
            .expect("valid max_allowed_packet default");
        inner
            .SetSystemVar(vardef::CharacterSetConnection, "utf8mb4")
            .expect("valid character_set_connection default");
        Self {
            Inner: inner,
            InitChunkSize: 2,
            MaxChunkSize: 32,
            TimeZone: FixedOffset::east_opt(0).expect("UTC offset is valid"),
            EnablePaging: vardef::DefTiDBEnablePaging,
            MinPagingSize: vardef::DefMinPagingSize,
            EnableChunkRPC: true,
            DivPrecisionIncrement: vardef::DefDivPrecisionIncrement,
            EnabledRateLimitAction: false,
            OriginalSQL: String::new(),
            RegardNULLAsPoint: false,
            OptPrefixIndexSingleScan: false,
            TiFlashFastScan: false,
            TiFlashFineGrainedShuffleBatchSize: 0,
            GroupConcatMaxLen: 0,
            InExplainStmt: false,
            TiFlashMaxThreads: 0,
            TiFlashMaxBytesBeforeExternalJoin: -1,
            TiFlashMaxBytesBeforeExternalGroupBy: -1,
            TiFlashMaxBytesBeforeExternalSort: -1,
            TiFlashMaxQueryMemoryPerNode: -1,
            TiFlashQuerySpillRatio: 0.0,
            TiFlashHashJoinVersion: String::new(),
            ResourceGroupName: String::new(),
        }
    }
}

impl MockSessionVars {
    /// 设置是否处于显式事务中（InTxn）。
    pub fn SetInTxn(&mut self, in_txn: bool) {
        self.Inner.SetInTxn(in_txn);
    }

    /// 当前是否处于显式事务中。
    pub fn InTxn(&self) -> bool {
        self.Inner.InTxn()
    }

    /// 设置会话系统变量。
    pub fn SetSystemVar(&mut self, name: &str, value: &str) -> Result<(), String> {
        self.Inner.SetSystemVar(name, value)
    }

    /// 读取会话系统变量。
    pub fn GetSystemVar(&self, name: &str) -> Option<String> {
        self.Inner.GetSystemVar(name)
    }
}

/// 事务内部状态：空、真实 KV 事务或无存储假事务。
enum TransactionState {
    Empty,
    Real(Box<dyn kv::Transaction>),
    Fake(fakeTxn),
}

/// 事务包装：保留 Go 在空 / 挂起时间戳 / 真事务 / 假事务之间的转换。
/// 挂起时间戳指已 `PrepareTSFuture` 但尚未 `Wait` 落到真实事务。
/// wrapTxn retains the Go transition between empty, timestamp-pending, real,
/// and fake transaction states.
pub struct wrapTxn {
    state: TransactionState,
    ts_future: Option<Box<dyn sessionctx::OracleFuture>>,
}

impl Default for wrapTxn {
    fn default() -> Self {
        Self {
            state: TransactionState::Empty,
            ts_future: None,
        }
    }
}

impl wrapTxn {
    /// 是否已有有效事务或挂起的 TS Future。
    fn validOrPending(&self) -> bool {
        self.ts_future.is_some() || self.Valid()
    }

    /// 是否处于“空状态但已挂起 Future”的 pending。
    fn pending(&self) -> bool {
        matches!(self.state, TransactionState::Empty) && self.ts_future.is_some()
    }

    /// 等待挂起 Future 完成并在有 Store 时 `Begin` 真实事务。
    pub fn Wait(
        &mut self,
        _ctx: &sessionctx::ExecutionContext,
        store: Option<&dyn kv::Storage>,
    ) -> MockResult<&mut Self> {
        if !self.validOrPending() {
            return Err(MockError::InvalidTransaction);
        }
        if self.pending() {
            let future = self.ts_future.take().expect("pending future checked");
            let start_ts = future
                .Wait()
                .map_err(|error| MockError::Message(error.to_string()))?;
            let store = store.ok_or(MockError::MissingStore)?;
            let txn = store.Begin(&[kv::tikv::TxnOption::StartTS(start_ts)])?;
            self.state = TransactionState::Real(txn);
        }
        Ok(self)
    }

    /// 当前事务是否有效。
    /// 假事务始终视为有效。
    pub fn Valid(&self) -> bool {
        match &self.state {
            TransactionState::Empty => false,
            TransactionState::Real(txn) => txn.Valid(),
            TransactionState::Fake(txn) => txn.Valid(),
        }
    }

    /// 当前事务起始时间戳（StartTS）；空状态为 0。
    /// 返回假事务起始时间戳。
    pub fn StartTS(&self) -> u64 {
        match &self.state {
            TransactionState::Empty => 0,
            TransactionState::Real(txn) => txn.StartTS(),
            TransactionState::Fake(txn) => txn.StartTS(),
        }
    }

    /// 缓存表元信息到当前事务（空状态忽略）。
    pub fn CacheTableInfo(&mut self, id: i64, info: kv::model::TableInfo) {
        match &mut self.state {
            TransactionState::Real(txn) => txn.CacheTableInfo(id, info),
            TransactionState::Fake(txn) => txn.CacheTableInfo(id, info),
            TransactionState::Empty => {}
        }
    }

    /// 按表 ID 取缓存的表元信息。
    pub fn GetTableInfo(&self, id: i64) -> Option<&kv::model::TableInfo> {
        match &self.state {
            TransactionState::Real(txn) => txn.GetTableInfo(id),
            TransactionState::Fake(txn) => txn.GetTableInfo(id),
            TransactionState::Empty => None,
        }
    }

    /// 提交当前事务（空状态成功）。
    /// 提交空操作。
    fn Commit(&mut self) -> MockResult<()> {
        match &mut self.state {
            TransactionState::Real(txn) => {
                txn.Commit(&kv::context::Context::default())?;
                Ok(())
            }
            TransactionState::Fake(txn) => txn.Commit(),
            TransactionState::Empty => Ok(()),
        }
    }

    /// 回滚当前事务（空状态成功）。
    /// 回滚空操作。
    fn Rollback(&mut self) -> MockResult<()> {
        match &mut self.state {
            TransactionState::Real(txn) => {
                txn.Rollback()?;
                Ok(())
            }
            TransactionState::Fake(txn) => txn.Rollback(),
            TransactionState::Empty => Ok(()),
        }
    }

    /// 设置磁盘满时的处理选项（DiskFullOpt）。
    fn SetDiskFullOpt(&mut self, level: kv::kvrpcpb::DiskFullOpt) {
        match &mut self.state {
            TransactionState::Real(txn) => txn.SetDiskFullOpt(level),
            TransactionState::Fake(txn) => txn.SetDiskFullOpt(level),
            TransactionState::Empty => {}
        }
    }
}

/// 无底层存储的假事务，供遗留测试使用。
/// fakeTxn is the no-storage transaction used by legacy tests.
pub struct fakeTxn {
    start_ts: u64,
    disk_full_opt: kv::kvrpcpb::DiskFullOpt,
    table_info: HashMap<i64, kv::model::TableInfo>,
}

impl fakeTxn {
    /// 构造 StartTS=1 的假事务。
    fn new() -> Self {
        Self {
            start_ts: 1,
            disk_full_opt: kv::kvrpcpb::DiskFullOpt::default(),
            table_info: HashMap::new(),
        }
    }

    pub fn StartTS(&self) -> u64 {
        self.start_ts
    }

    /// 记录磁盘满选项（测试桩）。
    pub fn SetDiskFullOpt(&mut self, level: kv::kvrpcpb::DiskFullOpt) {
        self.disk_full_opt = level;
    }

    /// 设置事务选项（空实现）。
    pub fn SetOption(&mut self, _option: i32, _value: Box<dyn Any>) {}

    /// 读键返回空值（假事务）。
    pub fn Get(
        &self,
        _ctx: &sessionctx::ExecutionContext,
        _key: kv::Key,
        _options: &[kv::GetOption],
    ) -> MockResult<kv::ValueEntry> {
        Ok(kv::ValueEntry::default())
    }

    pub fn Valid(&self) -> bool {
        true
    }

    fn Commit(&mut self) -> MockResult<()> {
        Ok(())
    }

    fn Rollback(&mut self) -> MockResult<()> {
        Ok(())
    }

    /// 缓存表信息。
    fn CacheTableInfo(&mut self, id: i64, info: kv::model::TableInfo) {
        self.table_info.insert(id, info);
    }

    /// 读取缓存表信息。
    fn GetTableInfo(&self, id: i64) -> Option<&kv::model::TableInfo> {
        self.table_info.get(&id)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 进程信息桩类型（空结构）。
pub struct ProcessInfo;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 表锁类型桩；`None` 表示未加锁。
pub enum TableLockType {
    None,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表锁信息桩。
pub struct TableLockInfo {
    pub TableID: i64,
}

#[derive(Clone, Debug, PartialEq)]
/// DistSQL（分布式 SQL）执行上下文快照，从会话变量拷贝关键字段。
pub struct DistSQLContext {
    pub EnabledRateLimitAction: bool,
    pub EnableChunkRPC: bool,
    pub OriginalSQL: String,
    pub TiFlashMaxThreads: i64,
    pub TiFlashMaxBytesBeforeExternalJoin: i64,
    pub TiFlashMaxBytesBeforeExternalGroupBy: i64,
    pub TiFlashMaxBytesBeforeExternalSort: i64,
    pub TiFlashMaxQueryMemoryPerNode: i64,
    pub TiFlashQuerySpillRatio: f64,
    pub TiFlashHashJoinVersion: String,
    pub ResourceGroupName: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Ranger（范围推导）上下文：NULL 点与前缀索引单扫选项。
pub struct RangerContext {
    pub RegardNULLAsPoint: bool,
    pub OptPrefixIndexSingleScan: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 构建 tipb 计划时的上下文标志。
pub struct BuildPBContext {
    pub HasClient: bool,
    pub TiFlashFastScan: bool,
    pub TiFlashFineGrainedShuffleBatchSize: u64,
    pub GroupConcatMaxLen: u64,
    pub InExplainStmt: bool,
}

/// InfoSchema 工厂函数类型：由测试注入。
/// InfoSchema：库表元数据的只读快照视图。
type InfoSchemaFactory = Arc<dyn Fn(&[kv::model::TableInfo]) -> SharedAny + Send + Sync + 'static>;

/// 全局 InfoSchema 工厂存储。
fn info_schema_factory() -> &'static RwLock<Option<InfoSchemaFactory>> {
    static FACTORY: OnceLock<RwLock<Option<InfoSchemaFactory>>> = OnceLock::new();
    FACTORY.get_or_init(|| RwLock::new(None))
}

/// 设置或清除测试用 InfoSchema 工厂。
pub fn SetMockInfoschemaFactory(factory: Option<InfoSchemaFactory>) {
    *info_schema_factory()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = factory;
}

/// 若已设置工厂则用给定表列表构造 InfoSchema。
pub fn MockInfoschema(tables: &[kv::model::TableInfo]) -> Option<SharedAny> {
    info_schema_factory()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .as_ref()
        .map(|factory| factory(tables))
}

/// 测试专用会话上下文，对应 Go `context.go` 的 Context。
/// Context represents the test-only session context from context.go.
pub struct Context {
    txn: wrapTxn,
    dom: Option<SharedAny>,
    schema_validator: Option<SharedAny>,
    pub Store: Option<Arc<dyn kv::Storage + Send + Sync>>,
    execution_ctx: sessionctx::ExecutionContext,
    session_manager: Option<SharedAny>,
    info_schema: Option<SharedAny>,
    values: HashMap<String, Box<dyn Any>>,
    session_vars: MockSessionVars,
    plan_cache: Option<SharedAny>,
    level: kv::kvrpcpb::DiskFullOpt,
    in_sandbox_mode: bool,
    is_ddl_owner: bool,
}

impl Context {
    /// 返回执行/追踪用上下文。
    pub fn GetTraceCtx(&self) -> &sessionctx::ExecutionContext {
        &self.execution_ctx
    }

    /// 执行 SQL（未实现）。
    pub fn Execute(&self, _sql: &str) -> MockResult<Vec<()>> {
        Err(MockError::NotSupported)
    }

    /// 执行语句节点（未实现）。
    pub fn ExecuteStmt<T>(&self, _stmt: T) -> MockResult<()> {
        Err(MockError::NotSupported)
    }

    /// 带参数解析 SQL（未实现）。
    pub fn ParseWithParams(&self, _sql: &str, _args: &[Box<dyn Any>]) -> MockResult<()> {
        Err(MockError::NotSupported)
    }

    /// 受限语句执行（未实现）。
    pub fn ExecRestrictedStmt<T>(&self, _stmt: T) -> MockResult<(Vec<()>, Vec<()>)> {
        Err(MockError::NotSupported)
    }

    /// 受限 SQL 执行（未实现）。
    pub fn ExecRestrictedSQL(
        &self,
        _options: &[()],
        _sql: &str,
        _args: &[Box<dyn Any>],
    ) -> MockResult<(Vec<()>, Vec<()>)> {
        Err(MockError::NotSupported)
    }

    /// 返回自身作为 SQL 执行器。
    pub fn GetSQLExecutor(&self) -> &Self {
        self
    }

    /// 返回自身作为受限 SQL 执行器。
    pub fn GetRestrictedSQLExecutor(&self) -> &Self {
        self
    }

    /// 内部 SQL 执行（未实现）。
    pub fn ExecuteInternal(&self, _sql: &str, _args: &[Box<dyn Any>]) -> MockResult<()> {
        Err(MockError::NotSupported)
    }

    /// 返回空进程信息。
    pub fn ShowProcess(&self) -> ProcessInfo {
        ProcessInfo
    }

    /// 设置是否为 DDL Owner。
    pub fn SetIsDDLOwner(&mut self, is_owner: bool) {
        self.is_ddl_owner = is_owner;
    }

    /// 是否为 DDL Owner。
    pub fn IsDDLOwner(&self) -> bool {
        self.is_ddl_owner
    }

    /// 在上下文中存任意键值。
    pub fn SetValue(&mut self, key: impl ToString, value: impl Any) {
        self.values.insert(key.to_string(), Box::new(value));
    }

    /// 按键读取并向下转型。
    pub fn Value<T: Any>(&self, key: impl ToString) -> Option<&T> {
        self.values
            .get(&key.to_string())
            .and_then(|value| value.downcast_ref())
    }

    /// 清除键对应的值。
    pub fn ClearValue(&mut self, key: impl ToString) {
        self.values.remove(&key.to_string());
    }

    /// 是否有脏表内容（固定 false）。
    pub fn HasDirtyContent(&self, _table_id: i64) -> bool {
        false
    }

    /// 只读会话变量。
    pub fn GetSessionVars(&self) -> &MockSessionVars {
        &self.session_vars
    }

    /// 可变会话变量。
    pub fn GetSessionVarsMut(&mut self) -> &mut MockSessionVars {
        &mut self.session_vars
    }

    /// 计划阶段上下文（返回自身）。
    pub fn GetPlanCtx(&self) -> &Self {
        self
    }

    /// 空值拒绝检查表达式上下文（返回自身）。
    pub fn GetNullRejectCheckExprCtx(&self) -> &Self {
        self
    }

    /// 表达式求值上下文（返回自身）。
    pub fn GetExprCtx(&self) -> &Self {
        self
    }

    /// 表操作上下文（返回自身）。
    pub fn GetTableCtx(&self) -> &Self {
        self
    }

    /// 从会话变量组装 DistSQL 上下文。
    pub fn GetDistSQLCtx(&self) -> DistSQLContext {
        let vars = self.GetSessionVars();
        DistSQLContext {
            EnabledRateLimitAction: vars.EnabledRateLimitAction,
            EnableChunkRPC: vars.EnableChunkRPC,
            OriginalSQL: vars.OriginalSQL.clone(),
            TiFlashMaxThreads: vars.TiFlashMaxThreads,
            TiFlashMaxBytesBeforeExternalJoin: vars.TiFlashMaxBytesBeforeExternalJoin,
            TiFlashMaxBytesBeforeExternalGroupBy: vars.TiFlashMaxBytesBeforeExternalGroupBy,
            TiFlashMaxBytesBeforeExternalSort: vars.TiFlashMaxBytesBeforeExternalSort,
            TiFlashMaxQueryMemoryPerNode: vars.TiFlashMaxQueryMemoryPerNode,
            TiFlashQuerySpillRatio: vars.TiFlashQuerySpillRatio,
            TiFlashHashJoinVersion: vars.TiFlashHashJoinVersion.clone(),
            ResourceGroupName: vars.ResourceGroupName.clone(),
        }
    }

    /// 从会话变量组装 Ranger 上下文。
    pub fn GetRangerCtx(&self) -> RangerContext {
        RangerContext {
            RegardNULLAsPoint: self.session_vars.RegardNULLAsPoint,
            OptPrefixIndexSingleScan: self.session_vars.OptPrefixIndexSingleScan,
        }
    }

    /// 组装构建 PB 计划所需标志。
    pub fn GetBuildPBCtx(&self) -> BuildPBContext {
        BuildPBContext {
            HasClient: self.GetClient().is_some(),
            TiFlashFastScan: self.session_vars.TiFlashFastScan,
            TiFlashFineGrainedShuffleBatchSize: self
                .session_vars
                .TiFlashFineGrainedShuffleBatchSize,
            GroupConcatMaxLen: self.session_vars.GroupConcatMaxLen,
            InExplainStmt: self.session_vars.InExplainStmt,
        }
    }

    /// 取事务包装；`active` 且当前无效时会新建事务。
    pub fn Txn(&mut self, active: bool) -> MockResult<&mut wrapTxn> {
        if active && !self.txn.validOrPending() {
            self.newTxn()?;
        }
        Ok(&mut self.txn)
    }

    /// 从 Store 取 KV Client。
    pub fn GetClient(&self) -> Option<&dyn kv::Client> {
        self.Store.as_ref().map(|store| store.GetClient())
    }

    /// 从 Store 取 MPP Client（大规模并行处理客户端）。
    pub fn GetMPPClient(&self) -> Option<&dyn kv::MPPClient> {
        self.Store.as_ref().map(|store| store.GetMPPClient())
    }

    /// 懒加载 InfoSchema。
    pub fn GetInfoSchema(&mut self) -> Option<&SharedAny> {
        if self.info_schema.is_none() {
            self.info_schema = MockInfoschema(&[]);
        }
        self.info_schema.as_ref()
    }

    /// 最新 InfoSchema（同 `GetInfoSchema`）。
    pub fn GetLatestInfoSchema(&mut self) -> Option<&SharedAny> {
        self.GetInfoSchema()
    }

    /// 不含会话扩展的最新 InfoSchema。
    pub fn GetLatestISWithoutSessExt(&mut self) -> Option<&SharedAny> {
        self.GetLatestInfoSchema()
    }

    /// 从 Domain 向下转型 SQL Server。
    pub fn GetSQLServer<T: Any>(&self) -> &T {
        self.dom
            .as_ref()
            .expect("mock.Context domain is not bound")
            .downcast_ref()
            .expect("mock.Context domain has an unexpected type")
    }

    /// 是否跨 Keyspace（固定 false）。Keyspace：多租户键空间隔离单位。
    pub fn IsCrossKS(&self) -> bool {
        false
    }

    /// 返回 schema 校验器句柄。
    pub fn GetSchemaValidator(&self) -> Option<&SharedAny> {
        self.schema_validator.as_ref()
    }

    /// 内建函数使用统计（空 map）。
    pub fn GetBuiltinFunctionUsage(&self) -> HashMap<String, u32> {
        HashMap::new()
    }

    /// 增加内建函数使用计数（空实现）。
    pub fn BuiltinFunctionUsageInc(&self, _name: &str) {}

    /// 读取全局系统变量。
    pub fn GetGlobalSysVar(&self, name: &str) -> MockResult<String> {
        variable::GetSysVar(name)
            .map(|variable| variable.Value.clone())
            .ok_or_else(|| MockError::Message(format!("unknown system variable: {name}")))
    }

    /// 设置全局系统变量。
    pub fn SetGlobalSysVar(&self, name: &str, value: &str) -> MockResult<()> {
        variable::SetSysVar(name, value).map_err(|error| MockError::Message(error.to_string()))
    }

    /// 会话计划缓存句柄。
    pub fn GetSessionPlanCache(&self) -> Option<&SharedAny> {
        self.plan_cache.as_ref()
    }

    /// 新建事务：无 Store 时用假事务，否则 Begin 真实事务。
    fn newTxn(&mut self) -> MockResult<()> {
        let Some(store) = self.Store.clone() else {
            self.fakeTxn();
            return Ok(());
        };
        if self.txn.Valid() {
            self.txn.Commit()?;
        }
        self.txn.state = TransactionState::Real(store.Begin(&[])?);
        self.txn.ts_future = None;
        Ok(())
    }

    /// 切换为假事务并清除挂起 Future。
    fn fakeTxn(&mut self) {
        self.txn.state = TransactionState::Fake(fakeTxn::new());
        self.txn.ts_future = None;
    }

    /// 刷新事务上下文（重新 `newTxn`）。
    pub fn RefreshTxnCtx(&mut self, _ctx: &sessionctx::ExecutionContext) -> MockResult<()> {
        self.newTxn()
    }

    /// 回滚事务并清除 InTxn。
    pub fn RollbackTxn(&mut self, _ctx: &sessionctx::ExecutionContext) {
        let _ = self.txn.Rollback();
        self.session_vars.SetInTxn(false);
    }

    /// 提交有效事务并清除 InTxn。
    pub fn CommitTxn(&mut self, _ctx: &sessionctx::ExecutionContext) -> MockResult<()> {
        self.txn.SetDiskFullOpt(self.level);
        let result = if self.txn.Valid() {
            self.txn.Commit()
        } else {
            Ok(())
        };
        self.session_vars.SetInTxn(false);
        result
    }

    /// 返回底层 KV Store。
    pub fn GetStore(&self) -> Option<&(dyn kv::Storage + Send + Sync)> {
        self.Store.as_deref()
    }

    /// 会话管理器句柄。
    pub fn GetSessionManager(&self) -> Option<&SharedAny> {
        self.session_manager.as_ref()
    }

    /// 设置会话管理器。
    pub fn SetSessionManager(&mut self, manager: SharedAny) {
        self.session_manager = Some(manager);
    }

    /// 取消执行上下文。
    pub fn Cancel(&self) {
        self.execution_ctx.cancel();
    }

    /// 克隆当前执行上下文（对应 Go context）。
    pub fn GoCtx(&self) -> sessionctx::ExecutionContext {
        self.execution_ctx.clone()
    }

    /// 更新列统计使用信息（空实现）。
    pub fn UpdateColStatsUsage<I>(&self, _items: I)
    where
        I: IntoIterator,
    {
    }

    /// 记录索引使用（空实现）。
    pub fn StoreIndexUsage(&self, _table_id: i64, _index_id: i64, _usage: i64) {}

    /// 事务写吞吐 SLI 桩。
    pub fn GetTxnWriteThroughputSLI(&self) -> sli::TxnWriteThroughputSLI {
        sli::TxnWriteThroughputSLI::default()
    }

    /// 语句级提交（空实现）。
    pub fn StmtCommit(&self, _ctx: &sessionctx::ExecutionContext) {}

    /// 语句级回滚（空实现）。
    pub fn StmtRollback(&self, _ctx: &sessionctx::ExecutionContext, _is_pessimistic: bool) {}

    /// 添加表锁（空实现）。
    pub fn AddTableLock(&self, _locks: &[TableLockInfo]) {}

    /// 释放表锁（空实现）。
    pub fn ReleaseTableLocks(&self, _locks: &[TableLockInfo]) {}

    /// 按表 ID 释放表锁（空实现）。
    pub fn ReleaseTableLockByTableIDs(&self, _ids: &[i64]) {}

    /// 检查表是否加锁（固定未锁）。
    pub fn CheckTableLocked(&self, _table_id: i64) -> (bool, TableLockType) {
        (false, TableLockType::None)
    }

    /// 全部表锁列表（空）。
    pub fn GetAllTableLocks(&self) -> Vec<TableLockInfo> {
        Vec::new()
    }

    /// 释放全部表锁（空实现）。
    pub fn ReleaseAllTableLocks(&self) {}

    /// 是否持有表锁（固定 false）。
    pub fn HasLockedTables(&self) -> bool {
        false
    }

    /// 准备 TS Future：清空事务状态并挂起 Future。
    pub fn PrepareTSFuture(
        &mut self,
        _ctx: &sessionctx::ExecutionContext,
        future: Box<dyn sessionctx::OracleFuture>,
        _scope: &str,
    ) -> MockResult<()> {
        self.txn.state = TransactionState::Empty;
        self.txn.ts_future = Some(future);
        Ok(())
    }

    /// 若事务有效或挂起则返回包装器。
    pub fn GetPreparedTxnFuture(&mut self) -> Option<&mut wrapTxn> {
        if !self.txn.validOrPending() {
            return None;
        }
        Some(&mut self.txn)
    }

    /// 语句统计句柄（无）。
    pub fn GetStmtStats(&self) -> Option<&dyn Any> {
        None
    }

    /// 获取咨询锁（空成功）。
    pub fn GetAdvisoryLock(&self, _name: &str, _timeout: i64) -> MockResult<()> {
        Ok(())
    }

    /// 咨询锁是否被使用（固定 0）。
    pub fn IsUsedAdvisoryLock(&self, _name: &str) -> u64 {
        0
    }

    /// 释放咨询锁（固定成功）。
    pub fn ReleaseAdvisoryLock(&self, _name: &str) -> bool {
        true
    }

    /// 释放全部咨询锁（固定 0）。
    pub fn ReleaseAllAdvisoryLocks(&self) -> usize {
        0
    }

    /// 编码会话状态（未实现）。
    pub fn EncodeStates<T>(&self, _states: &mut T) -> MockResult<()> {
        Err(MockError::NotSupported)
    }

    /// 解码会话状态（未实现）。
    pub fn DecodeStates<T>(&self, _states: &mut T) -> MockResult<()> {
        Err(MockError::NotSupported)
    }

    /// 扩展点句柄（无）。
    pub fn GetExtensions(&self) -> Option<&dyn Any> {
        None
    }

    /// 启用沙箱模式。
    pub fn EnableSandBoxMode(&mut self) {
        self.in_sandbox_mode = true;
    }

    /// 禁用沙箱模式。
    pub fn DisableSandBoxMode(&mut self) {
        self.in_sandbox_mode = false;
    }

    /// 是否处于沙箱模式。
    pub fn InSandBoxMode(&self) -> bool {
        self.in_sandbox_mode
    }

    /// 直接设置 InfoSchema。
    pub fn SetInfoSchema(&mut self, info_schema: SharedAny) {
        self.info_schema = Some(info_schema);
    }

    /// 重置会话与语句时区。
    pub fn ResetSessionAndStmtTimeZone(&mut self, time_zone: FixedOffset) {
        self.session_vars.TimeZone = time_zone;
    }

    /// 上报使用统计（空实现）。
    pub fn ReportUsageStats(&self) {}

    /// 关闭上下文（空实现）。
    pub fn Close(&self) {}

    /// 语句索引使用收集器（无）。
    pub fn NewStmtIndexUsageCollector(&self) -> Option<&dyn Any> {
        None
    }

    /// 游标跟踪器（无）。
    pub fn GetCursorTracker(&self) -> Option<&dyn Any> {
        None
    }

    /// 提交等待组（无）。
    pub fn GetCommitWaitGroup(&self) -> Option<&dyn Any> {
        None
    }

    /// 绑定 Domain 与 schema 校验器。
    pub fn BindDomainAndSchValidator(&mut self, domain: SharedAny, validator: SharedAny) {
        self.dom = Some(domain);
        self.schema_validator = Some(validator);
    }

    /// 返回 Domain 句柄。
    pub fn GetDomain(&self) -> Option<&SharedAny> {
        self.dom.as_ref()
    }
}

/// 遗留调用方使用的 NewContext 别名。
/// NewContextDeprecated is retained for legacy callers.
pub fn NewContextDeprecated() -> Box<Context> {
    newContext()
}

/// 构造默认空 Store 的 mock Context。
pub(crate) fn newContext() -> Box<Context> {
    Box::new(Context {
        txn: wrapTxn::default(),
        dom: None,
        schema_validator: None,
        Store: None,
        execution_ctx: sessionctx::ExecutionContext::new(),
        session_manager: None,
        info_schema: None,
        values: HashMap::new(),
        session_vars: MockSessionVars::default(),
        plan_cache: None,
        level: kv::kvrpcpb::DiskFullOpt::default(),
        in_sandbox_mode: false,
        is_ddl_owner: false,
    })
}

/// 测试钩子键类型别名（字符串）。
/// HookKeyForTest is the string key alias used with context values.
pub type HookKeyForTest = String;

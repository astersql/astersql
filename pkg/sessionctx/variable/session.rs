// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Session-local state and the state-only methods from session.go. External
// executor, transaction and storage objects remain owned by their modules.
// 会话局部状态与 `session.go` 中仅状态相关的方法。
//
// 汇总会话作用域系统变量、事务上下文、计划缓存、Hint/SET_VAR 还原栈、
// 标量子查询注册等；外部执行器/存储对象由各自模块持有，此处用占位类型表示。
#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use chrono::FixedOffset;
use std::any::Any;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use std::time::Instant;
use stmtctx_dependency::StatementContext;

// RetryInfo 保存一次事务重试所需的自增/自随机 ID 游标。
/// 事务重试时复用自增（AUTO_INCREMENT）与自随机（AUTO_RANDOM）ID 的游标状态。
#[derive(Default)]
pub struct RetryInfo {
    pub Retrying: bool,
    pub DroppedPreparedStmtIDs: Vec<u32>,
    autoIncrementIDs: Mutex<RetryInfoAutoIDs>,
    autoRandomIDs: Mutex<RetryInfoAutoIDs>,
    pub LastRcReadTS: u64,
}
impl Clone for RetryInfo {
    fn clone(&self) -> Self {
        Self {
            Retrying: self.Retrying,
            DroppedPreparedStmtIDs: self.DroppedPreparedStmtIDs.clone(),
            autoIncrementIDs: Mutex::new(
                self.autoIncrementIDs
                    .lock()
                    .expect("retry auto increment lock poisoned")
                    .clone(),
            ),
            autoRandomIDs: Mutex::new(
                self.autoRandomIDs
                    .lock()
                    .expect("retry auto random lock poisoned")
                    .clone(),
            ),
            LastRcReadTS: self.LastRcReadTS,
        }
    }
}
/// 一组自增/自随机 ID 及其当前读取偏移。
#[derive(Default, Clone)]
struct RetryInfoAutoIDs {
    currentOffset: usize,
    autoIDs: Vec<i64>,
}
impl RetryInfoAutoIDs {
    /// 将读取偏移重置到开头。
    fn resetOffset(&mut self) {
        self.currentOffset = 0;
    }
    /// 清空偏移与全部已记录 ID。
    fn clean(&mut self) {
        self.currentOffset = 0;
        self.autoIDs.clear();
    }
    /// 取出当前偏移处的 ID 并前进；无可用时返回 `(0, false)`。
    fn getCurrent(&mut self) -> (i64, bool) {
        match self.autoIDs.get(self.currentOffset).copied() {
            Some(id) => {
                self.currentOffset += 1;
                (id, true)
            }
            None => (0, false),
        }
    }
}
impl RetryInfo {
    // Clean 清理 retry 期间产生的 ID 和 prepared statement 列表。
    pub fn Clean(&mut self) {
        self.autoIncrementIDs
            .get_mut()
            .expect("retry auto increment lock poisoned")
            .clean();
        self.autoRandomIDs
            .get_mut()
            .expect("retry auto random lock poisoned")
            .clean();
        self.DroppedPreparedStmtIDs.clear();
    }
    /// 重置自增与自随机 ID 的读取偏移。
    pub fn ResetOffset(&self) {
        self.autoIncrementIDs
            .lock()
            .expect("retry auto increment lock poisoned")
            .resetOffset();
        self.autoRandomIDs
            .lock()
            .expect("retry auto random lock poisoned")
            .resetOffset();
    }
    /// 记录一次分配的自增 ID，供重试时按序复用。
    pub fn AddAutoIncrementID(&mut self, id: i64) {
        self.autoIncrementIDs
            .get_mut()
            .expect("retry auto increment lock poisoned")
            .autoIDs
            .push(id);
    }
    /// 取得下一个待复用的自增 ID。
    pub fn GetCurrAutoIncrementID(&mut self) -> (i64, bool) {
        self.autoIncrementIDs
            .get_mut()
            .expect("retry auto increment lock poisoned")
            .getCurrent()
    }
    /// 记录一次分配的自随机 ID。
    pub fn AddAutoRandomID(&mut self, id: i64) {
        self.autoRandomIDs
            .get_mut()
            .expect("retry auto random lock poisoned")
            .autoIDs
            .push(id);
    }
    /// 取得下一个待复用的自随机 ID。
    pub fn GetCurrAutoRandomID(&mut self) -> (i64, bool) {
        self.autoRandomIDs
            .get_mut()
            .expect("retry auto random lock poisoned")
            .getCurrent()
    }
}

// TransactionContext 将事务结束时需要清理的状态与 savepoint 恢复状态分开保存。
/// 当前事务上下文：将需在 savepoint 恢复的状态与无需恢复的状态分开保存。
#[derive(Default)]
pub struct TransactionContext {
    pub needToRestore: TxnCtxNeedToRestore,
    pub noNeedToRestore: TxnCtxNoNeedToRestore,
    pub FairLockingUsed: AtomicBool,
    pub FairLockingEffective: AtomicBool,
}

/// SQL 改写阶段耗时统计（含预处理子查询次数）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RewritePhaseInfo {
    pub DurationRewrite: Duration,
    pub DurationPreprocessSubQuery: Duration,
    pub PreprocessSubQueries: usize,
}

impl RewritePhaseInfo {
    /// 清零改写阶段统计。
    pub fn Reset(&mut self) {
        *self = Self::default();
    }
}

/// 优化器各子阶段耗时：绑定匹配、统计同步、逻辑/物理优化等。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DurationOptimizer {
    pub Total: Duration,
    pub BindingMatch: Duration,
    pub StatsSyncWait: Duration,
    pub LogicalOpt: Duration,
    pub PhysicalOpt: Duration,
    pub StatsDerive: Duration,
    pub TiFlashInfoFetch: Duration,
}
/// 事务结束或回滚到 savepoint 时需要恢复的状态。
#[derive(Default, Clone)]
pub struct TxnCtxNeedToRestore {
    pub TableDeltaMap: HashMap<i64, TableDelta>,
    pub CachedTables: HashMap<i64, String>,
    pub InsertTTLRowsCount: i32,
    pessimisticLockCache: HashMap<String, Vec<u8>>,
}
/// 事务内运行时状态：时间戳、隔离级别、savepoint 列表等。
#[derive(Default)]
pub struct TxnCtxNoNeedToRestore {
    forUpdateTS: u64,
    pub StartTS: AtomicU64,
    pub StaleReadTs: u64,
    pub IsStaleness: bool,
    pub IsExplicit: bool,
    pub Isolation: String,
    pub LockExpire: u32,
    pub ForUpdate: u32,
    pub TxnScope: String,
    pub Savepoints: Vec<SavepointRecord>,
    unchangedKeys: Mutex<HashMap<Vec<u8>, bool>>,
    pub PessimisticCacheHit: i32,
    pub StatementCount: i32,
    pub CouldRetry: bool,
    pub IsPessimistic: bool,
    tdmLock: Mutex<()>,
    pub EnableMDL: bool,
    relatedTableForMDL: Option<HashMap<i64, i64>>,
    pub CurrentStmtPessimisticLockCache: HashMap<String, Vec<u8>>,
}
/// 单表在事务中的行数/字节变化增量。
#[derive(Clone, Default)]
pub struct TableDelta {
    pub Delta: i64,
    pub Count: i64,
    pub InitTime: Option<Instant>,
}
/// 具名保存点：MemDB 检查点与可恢复的事务子状态。
#[derive(Clone, Default)]
pub struct SavepointRecord {
    pub Name: String,
    pub MemDBCheckpoint: Option<String>,
    pub TxnCtxSavepoint: TxnCtxNeedToRestore,
}

impl TransactionContext {
    pub fn SetStartTS(&self, ts: u64) {
        self.noNeedToRestore.StartTS.store(ts, Ordering::Release);
    }

    pub fn StartTS(&self) -> u64 {
        self.noNeedToRestore.StartTS.load(Ordering::Acquire)
    }
    /// 记录未改动但仍需加锁的 key；`shared` 表示共享锁意图。
    pub fn AddUnchangedKeyForLock(&self, key: &[u8], shared: bool) {
        let k = key.to_vec();
        let mut unchanged = self
            .noNeedToRestore
            .unchangedKeys
            .lock()
            .expect("unchanged key lock poisoned");
        let old = unchanged.get(&k).copied();
        unchanged.insert(k, shared && old.unwrap_or(true));
    }
    /// 收集需要排他锁（X Lock）的未改动 key。
    pub fn CollectUnchangedKeysForXLock(&self, mut buf: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        let unchanged = self
            .noNeedToRestore
            .unchangedKeys
            .lock()
            .expect("unchanged key lock poisoned");
        for (k, shared) in unchanged.iter() {
            if !shared {
                buf.push(k.clone());
            }
        }
        buf
    }
    /// 收集需要共享锁（S Lock）的未改动 key。
    pub fn CollectUnchangedKeysForSLock(&self, mut buf: Vec<Vec<u8>>) -> Vec<Vec<u8>> {
        let unchanged = self
            .noNeedToRestore
            .unchangedKeys
            .lock()
            .expect("unchanged key lock poisoned");
        for (k, shared) in unchanged.iter() {
            if *shared {
                buf.push(k.clone());
            }
        }
        buf
    }
    /// 清空未改动 key 集合。
    pub fn ResetUnchangedKeysForLock(&self) {
        self.noNeedToRestore
            .unchangedKeys
            .lock()
            .expect("unchanged key lock poisoned")
            .clear();
    }
    // UpdateDeltaForTable 在锁内合并行数/字节变化；这对应 Go 的 defer Unlock 边界。
    pub fn UpdateDeltaForTable(&mut self, table_id: i64, delta: i64, count: i64) {
        let _g = self.noNeedToRestore.tdmLock.lock().unwrap();
        let item = self
            .needToRestore
            .TableDeltaMap
            .entry(table_id)
            .or_default();
        item.Delta += delta;
        item.Count += count;
    }
    /// 返回 ForUpdate 时间戳与 StartTS 的较大值。
    pub fn GetForUpdateTS(&self) -> u64 {
        self.noNeedToRestore.forUpdateTS.max(self.StartTS())
    }
    /// 单调提升 ForUpdate 时间戳。
    pub fn SetForUpdateTS(&mut self, ts: u64) {
        self.noNeedToRestore.forUpdateTS = self.noNeedToRestore.forUpdateTS.max(ts);
    }
    /// 在锁保护下清空表增量映射。
    pub fn ClearDelta(&mut self) {
        let _g = self.noNeedToRestore.tdmLock.lock().unwrap();
        self.needToRestore.TableDeltaMap.clear();
    }
    // Cleanup 只清理不应跨事务的缓存；InfoSchema 等字段在 Go 中刻意保留，因此这里也不碰。
    pub fn Cleanup(&mut self) {
        self.needToRestore.TableDeltaMap.clear();
        self.needToRestore.pessimisticLockCache.clear();
        self.noNeedToRestore.CurrentStmtPessimisticLockCache.clear();
        self.noNeedToRestore.relatedTableForMDL = None;
        self.noNeedToRestore.Savepoints.clear();
        self.noNeedToRestore.IsStaleness = false;
        self.noNeedToRestore.EnableMDL = false;
    }
    /// 将当前语句的悲观锁缓存合并进事务级缓存。
    pub fn FlushStmtPessimisticLockCache(&mut self) {
        let current = std::mem::take(&mut self.noNeedToRestore.CurrentStmtPessimisticLockCache);
        self.needToRestore.pessimisticLockCache.extend(current);
    }
}

// WriteStmtBufs 和 TableSnapshot 是执行器复用的会话级缓冲，不在此处做 IO。
/// 写语句复用的会话级字节/字符串缓冲。
#[derive(Default)]
pub struct WriteStmtBufs {
    pub RowValBuf: Vec<u8>,
    pub AddRowValues: Vec<String>,
    pub IndexValsBuf: Vec<String>,
    pub IndexKeyBuf: Vec<u8>,
}
/// 临时表或快照读的行集合与可选错误信息。
#[derive(Default)]
pub struct TableSnapshot {
    pub Rows: Vec<Vec<String>>,
    pub Err: Option<String>,
}
/// 临时表数据访问：按表 ID 查询大小、删除或设置 key。
pub trait TemporaryTableData {
    fn GetTableSize(&self, table_id: i64) -> i64;
    fn DeleteTableKey(&mut self, table_id: i64, key: &[u8]) -> Result<(), String>;
    fn SetTableKey(&mut self, table_id: i64, key: &[u8], value: &[u8]) -> Result<(), String>;
}

/// 读一致性级别：Strict 强一致 / Weak 弱一致。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReadConsistencyLevel {
    Strict,
    Weak,
}
impl ReadConsistencyLevel {
    /// 是否为弱读一致性。
    pub fn IsWeak(self) -> bool {
        self == Self::Weak
    }
}
/// 校验读一致性字符串，仅接受 `strict` / `weak`。
pub fn validateReadConsistencyLevel(value: &str) -> Result<(), String> {
    match value.to_ascii_lowercase().as_str() {
        "strict" | "weak" => Ok(()),
        _ => Err(format!("invalid read consistency: {value}")),
    }
}

/// 用户变量的值与类型双映射内部状态。
#[derive(Clone, Default)]
struct UserVarsState {
    values: HashMap<String, String>,
    // A user variable's type cannot always be inferred before its value is
    // assigned, so preserve the real FieldType independently, as Go does.
    types: HashMap<String, parser_ast::ast::FieldType>,
}

// UserVars 的两个 map 共用读写锁；Clone 时复制值而不是共享可变容器。
/// 会话用户变量（`@var`）容器，读写锁保护内部状态。
#[derive(Default)]
pub struct UserVars {
    state: RwLock<UserVarsState>,
}
impl Clone for UserVars {
    fn clone(&self) -> Self {
        self.CloneVars()
    }
}
impl UserVars {
    /// 构造空的用户变量容器。
    pub fn new() -> Self {
        Self::default()
    }
    /// 深拷贝当前用户变量状态。
    pub fn CloneVars(&self) -> Self {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        Self {
            state: RwLock::new(state),
        }
    }
    /// 设置用户变量的字符串值。
    pub fn SetUserVarVal(&self, name: &str, value: String) {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values
            .insert(name.to_owned(), value);
    }
    /// 按小写名删除用户变量的值与类型。
    pub fn UnsetUserVar(&self, name: &str) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let name = name.to_ascii_lowercase();
        state.values.remove(&name);
        state.types.remove(&name);
    }
    /// 读取用户变量字符串值。
    pub fn GetUserVarVal(&self, name: &str) -> Option<String> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values
            .get(name)
            .cloned()
    }
    /// 单独保存用户变量的 `FieldType`。
    pub fn SetUserVarType(&self, name: &str, ty: parser_ast::ast::FieldType) {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .types
            .insert(name.to_owned(), ty);
    }
    /// 读取已保存的用户变量字段类型。
    pub fn GetUserVarType(&self, name: &str) -> Option<parser_ast::ast::FieldType> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .types
            .get(name)
            .cloned()
    }
}

/// 计划缓存参数列表，可标记是否用于非 prepare 缓存。
#[derive(Clone, Default)]
pub struct PlanCacheParamList {
    paramValues: Vec<String>,
    forNonPrepCache: bool,
}
impl PlanCacheParamList {
    /// 构造空参数列表。
    pub fn new() -> Self {
        Self::default()
    }
    /// 清空参数并复位非 prepare 标记。
    pub fn Reset(&mut self) {
        self.paramValues.clear();
        self.forNonPrepCache = false;
    }
    /// Render prepared-plan arguments for statement logging. Non-prepared
    /// cache parameters are intentionally hidden, matching Go.
    pub fn String(&self) -> String {
        if self.paramValues.is_empty() || self.forNonPrepCache {
            String::new()
        } else {
            format!(" [arguments: {}]", self.paramValues.join(", "))
        }
    }
    /// 追加一批参数值。
    pub fn Append(&mut self, values: &[String]) {
        self.paramValues.extend_from_slice(values);
    }
    /// 标记是否用于非 prepare 计划缓存。
    pub fn SetForNonPrepCache(&mut self, flag: bool) {
        self.forNonPrepCache = flag;
    }
    /// 按索引取参数值。
    pub fn GetParamValue(&self, idx: usize) -> Option<&String> {
        self.paramValues.get(idx)
    }
    /// 返回全部参数切片。
    pub fn AllParamValues(&self) -> &[String] {
        &self.paramValues
    }
}
/// 惰性拼接的语句文本：可缓存完整串，或由 redact/SQL/参数重组。
#[derive(Default)]
pub struct LazyStmtText {
    text: Option<String>,
    pub SQL: String,
    pub Redact: String,
    pub Params: PlanCacheParamList,
}
impl LazyStmtText {
    /// 直接设置已拼好的语句文本缓存。
    pub fn SetText(&mut self, text: String) {
        self.text = Some(text);
    }
    /// 更新脱敏前缀、SQL 与参数，并失效文本缓存。
    pub fn Update(&mut self, redact: &str, sql: &str, params: Option<&PlanCacheParamList>) {
        self.text = None;
        self.Redact = redact.into();
        self.SQL = sql.into();
        self.Params = params.cloned().unwrap_or_default();
    }
    /// 返回完整语句文本，必要时惰性拼接。
    pub fn String(&mut self) -> String {
        self.text
            .get_or_insert_with(|| {
                let input = format!("{}{}", self.SQL, self.Params.String());
                match self.Redact.as_str() {
                    "OFF" | "" => input,
                    "ON" => String::new(),
                    "MARKER" => {
                        let escaped = input.replace('‹', "‹‹").replace('›', "››");
                        format!("‹{escaped}›")
                    }
                    _ => {
                        debug_assert!(false, "invalid redact mode");
                        String::new()
                    }
                }
            })
            .clone()
    }
}

/// 分区裁剪模式：静态/动态及其强制与过渡态。
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PartitionPruneMode {
    Static,
    Dynamic,
    StaticOnly,
    DynamicOnly,
    StaticButPrepareDynamic,
}

/// 返回合法的 TiFlash 预聚合模式枚举字符串列表。
pub fn ValidTiFlashPreAggMode() -> String {
    format!(
        "{}, {}, {}",
        vardef::ForcePreAggStr,
        vardef::AutoStr,
        vardef::ForceStreamingStr
    )
}
impl PartitionPruneMode {
    /// `StaticButPrepareDynamic` 为过渡态，视为非法最终模式。
    pub fn Valid(self) -> bool {
        !matches!(self, Self::StaticButPrepareDynamic)
    }
    /// 将 Only/过渡态规范化为 Static 或 Dynamic。
    pub fn Update(self) -> Self {
        match self {
            Self::StaticOnly | Self::StaticButPrepareDynamic => Self::Static,
            Self::DynamicOnly => Self::Dynamic,
            x => x,
        }
    }
}
/// 运行时过滤器类型：IN 列表或 MinMax 范围。
#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum RuntimeFilterType {
    In,
    MinMax,
}
impl RuntimeFilterType {
    /// 转为协议/配置使用的大写名称。
    pub fn String(self) -> &'static str {
        match self {
            Self::In => "IN",
            Self::MinMax => "MIN_MAX",
        }
    }
}
/// 将名称解析为运行时过滤器类型。
pub fn RuntimeFilterTypeStringToType(name: &str) -> Option<RuntimeFilterType> {
    match name {
        "IN" => Some(RuntimeFilterType::In),
        "MIN_MAX" => Some(RuntimeFilterType::MinMax),
        _ => None,
    }
}
/// 解析逗号分隔的过滤器类型列表；任一非法则返回空列表与 `false`。
pub fn ToRuntimeFilterType(value: &str) -> (Vec<RuntimeFilterType>, bool) {
    let mut out = Vec::new();
    for name in value.split(',') {
        let Some(v) = RuntimeFilterTypeStringToType(&name.to_ascii_uppercase()) else {
            return (Vec::new(), false);
        };
        if !out.contains(&v) {
            out.push(v);
        }
    }
    (out, true)
}

/// 规划器 Select 块别名列表的线程安全容器（对应 Go atomic.Pointer）。
/// Thread-safe equivalent of Go's `atomic.Pointer[[]ast.HintTable]`.
#[derive(Default)]
pub struct PlannerSelectBlockNames {
    value: RwLock<Option<Arc<Vec<parser_ast::HintTable>>>>,
}

impl PlannerSelectBlockNames {
    /// 读取当前块别名列表快照。
    pub fn Load(&self) -> Option<Arc<Vec<parser_ast::HintTable>>> {
        self.value
            .read()
            .expect("planner block-name lock poisoned")
            .clone()
    }

    /// 替换块别名列表。
    pub fn Store(&self, value: Option<Vec<parser_ast::HintTable>>) {
        *self
            .value
            .write()
            .expect("planner block-name lock poisoned") = value.map(Arc::new);
    }

    /// 按索引取单个 Hint 表元数据。
    pub fn get(&self, index: usize) -> Option<parser_ast::HintTable> {
        self.Load()?.get(index).cloned()
    }
}

/// 会话变量与运行时状态的核心聚合结构。
// SessionVars 汇总 session 作用域的系统变量、缓存和事务上下文；外部 TiDB 对象用字符串/映射占位。
pub struct SessionVars {
    /// Internal restricted-SQL option permitting WriteReorganization index analysis.
    pub EnableDDLAnalyzeExecOpt: bool,
    pub DMLBatchSize: i32,
    pub RetryLimit: i64,
    pub DisableTxnAutoRetry: bool,
    pub EnableNonPreparedPlanCache: bool,
    pub EnableNonPreparedPlanCacheForDML: bool,
    pub InMultiStmts: bool,
    pub PlanCacheStrategy: String,
    pub UsePlanBaselines: bool,
    pub EvolvePlanBaselines: bool,
    pub SelectLimit: u64,
    pub UserVars: UserVars,
    systems: HashMap<String, String>,
    pub SysWarningCount: i32,
    pub SysErrorCount: u16,
    pub PreparedStmts: HashMap<u32, String>,
    pub PreparedStmtNameToID: HashMap<String, u32>,
    preparedStmtID: u32,
    pub PlanCacheParams: PlanCacheParamList,
    pub PlanCacheValue: Option<String>,
    pub User: Option<parser_auth::auth::UserIdentity>,
    pub ActiveRoles: Vec<parser_auth::auth::RoleIdentity>,
    pub ConnectionInfo: Option<crate::ConnectionInfo>,
    pub RetryInfo: RetryInfo,
    pub TxnCtx: TransactionContext,
    pub StmtCtx: StatementContext,
    /// Cumulative processed keys for the session status variable `tidb_keys_examined`.
    pub KeysExamined: AtomicU64,
    /// Number of rows found by the previous completed SELECT, matching Go LastFoundRows.
    pub LastFoundRows: AtomicU64,
    /// Trace ID of the previously started statement in this session.
    /// Go `SessionVars.PrevTraceID` is updated at every statement boundary.
    pub PrevTraceID: Mutex<Vec<u8>>,
    pub GlobalVarsAccessor: Box<dyn crate::GlobalVarAccessor>,
    pub ConnectionID: u64,
    pub CommandValue: u8,
    pub ClientCapability: u32,
    pub InRestrictedSQL: bool,
    pub RequestSourceType: String,
    pub TTLJobID: String,
    pub InTxn: bool,
    pub Status: u16,
    pub Autocommit: bool,
    pub TxnMode: String,
    current_db: RwLock<String>,
    pub SessionAlias: String,
    pub SlowLogRules: crate::slow_log::SessionSlowLogRules,
    pub DurationParse: Mutex<Duration>,
    pub DurationCompile: Duration,
    pub DurationOptimizer: DurationOptimizer,
    pub DurationWaitTS: Mutex<Duration>,
    pub StartTime: Mutex<Instant>,
    /// 会话时区，供表达式求值可选属性使用；未显式设置时 Go 侧默认为 UTC。
    /// Session time zone used by expression evaluation optional properties.
    /// Go initializes SessionVars.TimeZone to UTC when no explicit zone is set.
    location: FixedOffset,
    pub EnablePlanReplayerCapture: bool,
    pub EnablePlanReplayedContinuesCapture: bool,
    pub OptPartialOrderedIndexForTopN: String,
    pub PartitionPruneMode: PartitionPruneMode,
    pub PlanID: AtomicI32,
    pub PlanColumnID: AtomicI64,
    /// 同步加载统计信息的最长等待毫秒数，对应 Go `SessionVars.StatsLoadSyncWait`。
    pub StatsLoadSyncWait: AtomicI64,
    pub PlannerSelectBlockAsName: PlannerSelectBlockNames,
    pub IsolationReadEngines: HashSet<kv::StoreType>,
    pub AllowMPPExecution: bool,
    pub EnforceMPPExecution: bool,
    pub CorrelationThreshold: f64,
    pub EnableCorrelationAdjustment: bool,
    pub CorrelationExpFactor: i64,
    pub RiskEqSkewRatio: f64,
    pub RiskRangeSkewRatio: f64,
    pub RiskScaleNDVSkewRatio: f64,
    pub RiskGroupNDVSkewRatio: f64,
    pub SelectivityFactor: f64,
    pub EnableVectorizedExpression: bool,
    pub EnableChunkRPC: bool,
    pub TiDBOptJoinReorderThreshold: i64,
    pub TiDBOptEnableAdvancedJoinReorder: bool,
    pub EnableLateMaterialization: bool,
    pub RangeMaxSize: i64,
    pub TiFlashPreAggMode: String,
    pub TiFlashFineGrainedShuffleStreamCount: i64,
    pub TiFlashMaxThreads: i64,
    pub OptOrderingIdxSelRatio: f64,
    pub DefaultStrMatchSelectivity: f64,
    pub ExplainNonEvaledSubQuery: bool,
    pub EnableAlternativeLogicalPlans: bool,
    pub EnableCorrelateSubquery: bool,
    pub EnableSemiJoinRewrite: bool,
    pub EnableFullOuterJoin: bool,
    pub EnableNoDecorrelateInSelect: bool,
    /// 控制 NO_BACKSLASH_ESCAPES 是否改变 LIKE 的默认转义行为。
    /// Controls whether NO_BACKSLASH_ESCAPES changes LIKE's default escape.
    pub EnableNoBackslashEscapesInLike: bool,
    allowInSubqToJoinAndAgg: bool,
    record_relevant_opt_vars_and_fixes: AtomicBool,
    relevant_opt_vars: Mutex<HashSet<String>>,
    relevant_opt_fixes: Mutex<HashSet<u64>>,
    hint_system_vars: Mutex<crate::SessionVars>,
    hint_system_var_restore: Mutex<HashMap<String, String>>,
    // Go keeps the real `*ScalarSubqueryEvalCtx` values in a session-local
    // `[]any` so EXPLAIN can downcast and recursively flatten their plans.
    // These planner contexts are intentionally thread-affine; requiring Send
    // or Sync here would incorrectly strengthen PlanContext/PhysicalPlan.
    scalar_subqueries: RefCell<Vec<Rc<dyn Any>>>,
    extended_col_unique_ids: RefCell<HashMap<String, i64>>,
    rewrite_phase_info: RefCell<RewritePhaseInfo>,
    alternative_round_overrides: Mutex<AlternativeRoundOverrides>,
}

// SAFETY: SessionVars is session-affine. The RefCell/Rc planner fields above must
// not be accessed concurrently. SessionContext and other Arc wrappers still need
// Send+Sync so the session handle can move across task boundaries; callers must
// keep those interior fields single-threaded.
unsafe impl Send for SessionVars {}
unsafe impl Sync for SessionVars {}

/// 默认全局变量访问器：内存 map + 回退到内置 sysvar 定义。
#[derive(Default)]
struct DefaultGlobalVarAccessor {
    values: HashMap<String, String>,
    tidb_table_values: HashMap<String, String>,
}

/// 备选逻辑计划轮次的临时开关覆盖。
#[derive(Default)]
struct AlternativeRoundOverrides {
    enable_correlate_subquery: Option<bool>,
    enable_semi_join_rewrite: Option<bool>,
    fts_like_fallback: Option<bool>,
}

impl crate::GlobalVarAccessor for DefaultGlobalVarAccessor {
    fn get_global_sys_var(&self, name: &str) -> Result<String, crate::VariableError> {
        if let Some(value) = self.values.get(name) {
            return Ok(value.clone());
        }
        crate::GetSysVar(name)
            .map(|sys_var| sys_var.Value.clone())
            .ok_or_else(|| crate::VariableError::unknown(name))
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &crate::Context,
        name: &str,
        value: &str,
        _update_local: bool,
    ) -> Result<(), crate::VariableError> {
        if crate::GetSysVar(name).is_none() {
            return Err(crate::VariableError::unknown(name));
        }
        self.values.insert(name.to_owned(), value.to_owned());
        Ok(())
    }

    fn get_tidb_table_value(&self, name: &str) -> Result<String, crate::VariableError> {
        self.tidb_table_values.get(name).cloned().ok_or_else(|| {
            crate::VariableError::new(crate::VariableErrorKind::InvalidValue, "Get SysVar Failed")
        })
    }

    fn set_tidb_table_value(
        &mut self,
        name: &str,
        value: &str,
        _comment: &str,
    ) -> Result<(), crate::VariableError> {
        self.tidb_table_values
            .insert(name.to_owned(), value.to_owned());
        Ok(())
    }
}

impl Default for SessionVars {
    fn default() -> Self {
        Self::new()
    }
}
impl SessionVars {
    /// Reads legacy RU weights from the TiKV client configuration.
    pub fn RUV2Weights(&self) -> execdetails::ruv2_metrics::RUV2Weights {
        let cfg = config::get_global_config();
        let weights = &cfg.tikv_client.ruv2;
        execdetails::ruv2_metrics::RUV2Weights {
            RUScale: weights.ru_scale,
            ResultChunkCells: weights.result_chunk_cells,
            ExecutorL1: weights.executor_l1,
            ExecutorL2: weights.executor_l2,
            ExecutorL3: weights.executor_l3,
            ExecutorL5InsertRows: weights.executor_l5_insert_rows,
            PlanCnt: weights.plan_cnt,
            PlanDeriveStatsPaths: weights.plan_derive_stats_paths,
            ResourceManagerReadCnt: weights.resource_manager_read_cnt,
            ResourceManagerWriteCnt: weights.resource_manager_write_cnt,
            WriteKeys: weights.write_keys,
            SessionParserTotal: weights.session_parser_total,
            TxnCnt: weights.txn_cnt,
        }
    }
    /// 返回会话当前数据库的快照。
    pub fn CurrentDB(&self) -> String {
        self.current_db
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub fn SetLastFoundRows(&self, rows: u64) {
        self.LastFoundRows.store(rows, Ordering::Release);
    }

    pub fn GetLastFoundRows(&self) -> u64 {
        self.LastFoundRows.load(Ordering::Acquire)
    }

    /// 更新会话当前数据库，供 USE 与 MySQL COM_INIT_DB 共享同一真源。
    pub fn SetCurrentDB(&self, database: impl Into<String>) {
        *self
            .current_db
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = database.into();
    }

    // NewSessionVars 初始化 Go 中所有必须非 nil 的 map、缓存和事务辅助对象。
    /// 构造带 Go 侧等价默认值的会话变量实例。
    pub fn new() -> Self {
        Self {
            EnableDDLAnalyzeExecOpt: false,
            DMLBatchSize: 0,
            RetryLimit: 10,
            DisableTxnAutoRetry: false,
            EnableNonPreparedPlanCache: vardef::DefTiDBEnableNonPreparedPlanCache,
            EnableNonPreparedPlanCacheForDML: vardef::DefTiDBEnableNonPreparedPlanCacheForDML,
            InMultiStmts: false,
            PlanCacheStrategy: vardef::DefTiDBPlanCacheStrategy.to_owned(),
            UsePlanBaselines: vardef::DefTiDBUsePlanBaselines,
            EvolvePlanBaselines: vardef::DefTiDBEvolvePlanBaselines,
            SelectLimit: u64::MAX,
            UserVars: UserVars::new(),
            systems: HashMap::new(),
            SysWarningCount: 0,
            SysErrorCount: 0,
            PreparedStmts: HashMap::new(),
            PreparedStmtNameToID: HashMap::new(),
            preparedStmtID: 0,
            PlanCacheParams: PlanCacheParamList::new(),
            PlanCacheValue: None,
            User: None,
            ActiveRoles: Vec::new(),
            ConnectionInfo: None,
            RetryInfo: RetryInfo::default(),
            TxnCtx: TransactionContext::default(),
            StmtCtx: *stmtctx_dependency::NewStmtCtx(),
            KeysExamined: AtomicU64::new(0),
            LastFoundRows: AtomicU64::new(0),
            PrevTraceID: Mutex::new(Vec::new()),
            GlobalVarsAccessor: Box::new(DefaultGlobalVarAccessor::default()),
            ConnectionID: 0,
            CommandValue: 0,
            ClientCapability: 0,
            InRestrictedSQL: false,
            RequestSourceType: String::new(),
            TTLJobID: String::new(),
            InTxn: false,
            Status: 0,
            Autocommit: true,
            TxnMode: "OPTIMISTIC".into(),
            current_db: RwLock::new(String::new()),
            SessionAlias: String::new(),
            SlowLogRules: crate::slow_log::NewSessionSlowLogRules(Some(
                crate::slow_log::SlowLogRules::default(),
            )),
            DurationParse: Mutex::new(Duration::default()),
            DurationCompile: Duration::default(),
            DurationOptimizer: DurationOptimizer::default(),
            DurationWaitTS: Mutex::new(Duration::default()),
            StartTime: Mutex::new(Instant::now()),
            location: FixedOffset::east_opt(0).expect("UTC offset is valid"),
            EnablePlanReplayerCapture: false,
            EnablePlanReplayedContinuesCapture: false,
            OptPartialOrderedIndexForTopN: String::new(),
            PartitionPruneMode: PartitionPruneMode::Static,
            PlanID: AtomicI32::new(0),
            PlanColumnID: AtomicI64::new(0),
            StatsLoadSyncWait: AtomicI64::new(vardef::DefTiDBStatsLoadSyncWait),
            PlannerSelectBlockAsName: PlannerSelectBlockNames::default(),
            IsolationReadEngines: HashSet::from([
                kv::StoreType::TiKV,
                kv::StoreType::TiFlash,
                kv::StoreType::TiDB,
            ]),
            AllowMPPExecution: false,
            EnforceMPPExecution: false,
            CorrelationThreshold: vardef::DefOptCorrelationThreshold,
            EnableCorrelationAdjustment: vardef::DefOptEnableCorrelationAdjustment,
            CorrelationExpFactor: vardef::DefOptCorrelationExpFactor,
            RiskEqSkewRatio: vardef::DefOptRiskEqSkewRatio,
            RiskRangeSkewRatio: vardef::DefOptRiskRangeSkewRatio,
            RiskScaleNDVSkewRatio: vardef::DefOptRiskScaleNDVSkewRatio,
            RiskGroupNDVSkewRatio: vardef::DefOptRiskGroupNDVSkewRatio,
            SelectivityFactor: vardef::DefOptSelectivityFactor,
            EnableVectorizedExpression: vardef::DefEnableVectorizedExpression,
            EnableChunkRPC: false,
            TiDBOptJoinReorderThreshold: vardef::DefTiDBOptJoinReorderThreshold,
            TiDBOptEnableAdvancedJoinReorder: vardef::DefTiDBOptEnableAdvancedJoinReorder,
            EnableLateMaterialization: vardef::DefTiDBOptEnableLateMaterialization,
            RangeMaxSize: vardef::DefTiDBOptRangeMaxSize,
            TiFlashPreAggMode: vardef::DefTiFlashPreAggMode.to_owned(),
            TiFlashFineGrainedShuffleStreamCount: vardef::DefTiFlashFineGrainedShuffleStreamCount,
            TiFlashMaxThreads: vardef::DefTiFlashMaxThreads,
            OptOrderingIdxSelRatio: vardef::DefTiDBOptOrderingIdxSelRatio,
            DefaultStrMatchSelectivity: vardef::DefTiDBDefaultStrMatchSelectivity as f64,
            ExplainNonEvaledSubQuery: false,
            EnableAlternativeLogicalPlans: vardef::DefOptEnableAlternativeLogicalPlans,
            EnableCorrelateSubquery: false,
            EnableSemiJoinRewrite: vardef::DefOptEnableSemiJoinRewrite,
            EnableFullOuterJoin: vardef::DefTiDBEnableFullOuterJoin,
            EnableNoDecorrelateInSelect: vardef::DefOptEnableNoDecorrelateInSelect,
            EnableNoBackslashEscapesInLike: vardef::DefTiDBEnableNoBackslashEscapesInLike,
            allowInSubqToJoinAndAgg: vardef::DefOptInSubqToJoinAndAgg,
            record_relevant_opt_vars_and_fixes: AtomicBool::new(false),
            relevant_opt_vars: Mutex::new(HashSet::new()),
            relevant_opt_fixes: Mutex::new(HashSet::new()),
            hint_system_vars: Mutex::new(crate::SessionVars::new(Box::new(
                DefaultGlobalVarAccessor::default(),
            ))),
            hint_system_var_restore: Mutex::new(HashMap::new()),
            scalar_subqueries: RefCell::new(Vec::new()),
            extended_col_unique_ids: RefCell::new(HashMap::new()),
            rewrite_phase_info: RefCell::new(RewritePhaseInfo::default()),
            alternative_round_overrides: Mutex::new(AlternativeRoundOverrides::default()),
        }
    }

    /// 开始语句级表缓存记账（对齐 Go 语句上下文生命周期）。
    /// Starts the Go statement-context lifecycle for table-cache accounting.
    pub fn BeginTableCacheStatement(&self) {
        self.StmtCtx.ResetReadFromTableCache();
    }

    /// 标记本语句曾从 table cache 读取。
    pub fn MarkReadFromTableCache(&self) {
        self.StmtCtx.SetReadFromTableCache();
    }

    /// 是否从 table cache 读取过。
    pub fn ReadFromTableCache(&self) -> bool {
        self.StmtCtx.IsReadFromTableCache()
    }
    /// 分配新的计划节点 ID（会话内单调递增）。
    pub fn AllocNewPlanID(&self) -> i32 {
        self.PlanID.fetch_add(1, Ordering::SeqCst) + 1
    }
    /// 返回会话时区偏移。
    pub fn location(&self) -> FixedOffset {
        self.location
    }
    /// 设置会话时区偏移。
    pub fn set_location(&mut self, location: FixedOffset) {
        self.location = location;
    }
    /// 分配新的计划列 ID。
    pub fn AllocPlanColumnID(&self) -> i64 {
        self.PlanColumnID.fetch_add(1, Ordering::SeqCst) + 1
    }
    /// 返回隔离读允许的存储引擎集合。
    pub fn GetIsolationReadEngines(&self) -> HashSet<kv::StoreType> {
        if self.StmtCtx.TiFlashEngineRemovedDueToStrictSQLMode {
            return self.IsolationReadEngines.clone();
        }
        self.GetSystemVar(vardef::TiDBIsolationReadEngines)
            .map(|value| {
                value
                    .split(',')
                    .filter_map(|engine| match engine.trim() {
                        "tikv" => Some(kv::StoreType::TiKV),
                        "tiflash" => Some(kv::StoreType::TiFlash),
                        "tidb" => Some(kv::StoreType::TiDB),
                        _ => None,
                    })
                    .collect()
            })
            .filter(|engines: &HashSet<_>| !engines.is_empty())
            .unwrap_or_else(|| self.IsolationReadEngines.clone())
    }
    /// 是否同时允许并强制 MPP 执行。
    pub fn IsMPPEnforced(&self) -> bool {
        let allow = self.AllowMPPExecution
            || self
                .GetSystemVar(vardef::TiDBAllowMPPExecution)
                .is_some_and(|value| {
                    matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true")
                });
        let enforce = self.EnforceMPPExecution
            || self
                .GetSystemVar(vardef::TiDBEnforceMPPExecution)
                .is_some_and(|value| {
                    matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true")
                });
        allow && enforce
    }
    /// 在强制 MPP 时追加警告。
    pub fn RaiseWarningWhenMPPEnforced(&self, warning: impl Into<String>) {
        if !self.IsMPPEnforced() {
            return;
        }
        let warning = warning.into();
        if self.StmtCtx.IsInExplainStmt() {
            self.StmtCtx
                .AppendWarning(stmtctx_dependency::errors::NewNoStackError(warning));
        } else {
            self.StmtCtx
                .AppendExtraWarning(stmtctx_dependency::errors::NewNoStackError(warning));
        }
    }
    /// 返回自身引用，满足 `SessionVarsProvider`。
    pub fn GetSessionVars(&self) -> &Self {
        self
    }

    /// Return the previous statement trace ID, matching Go's session field.
    pub fn PrevTraceIDValue(&self) -> Vec<u8> {
        self.PrevTraceID
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Store the trace ID generated for the current statement.
    pub fn SetPrevTraceID(&self, trace_id: Vec<u8>) {
        *self
            .PrevTraceID
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = trace_id;
    }

    /// Clear the previous statement trace ID for integration-test setup.
    pub fn ResetPrevTraceID(&self) {
        self.SetPrevTraceID(Vec::new());
    }
    /// 按 Hint 变量 → 会话 map → Hint 钩子顺序读取系统变量。
    pub fn GetSystemVar(&self, name: &str) -> Option<String> {
        if name.eq_ignore_ascii_case(vardef::WarningCount) {
            return Some(self.SysWarningCount.to_string());
        }
        if name.eq_ignore_ascii_case(vardef::ErrorCount) {
            return Some(self.SysErrorCount.to_string());
        }
        let hinted = self
            .hint_system_vars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .system(name)
            .map(str::to_owned);
        hinted
            .or_else(|| self.systems.get(name).cloned())
            .or_else(|| self.GetHintSystemVar(name).ok())
    }
    // SetSystemVar 保留 Go 的 session map 更新顺序；具体校验由 variable::SysVar 模块注入。
    pub fn SetSystemVar(&mut self, name: &str, value: &str) -> Result<(), String> {
        self.SetHintSystemVarWithOldState(name, value)
            .map_err(|error| error.to_string())?;
        let normalized = self
            .GetHintSystemVar(name)
            .map_err(|error| error.to_string())?;
        if name.eq_ignore_ascii_case(vardef::TiDBIsolationReadEngines) {
            self.IsolationReadEngines = normalized
                .split(',')
                .filter_map(|engine| match engine {
                    "tikv" => Some(kv::StoreType::TiKV),
                    "tiflash" => Some(kv::StoreType::TiFlash),
                    "tidb" => Some(kv::StoreType::TiDB),
                    _ => None,
                })
                .collect();
        } else if name.eq_ignore_ascii_case(vardef::TiDBOptRangeMaxSize) {
            self.RangeMaxSize = normalized.parse().map_err(|_| {
                format!("invalid {} value {normalized}", vardef::TiDBOptRangeMaxSize)
            })?;
        } else if name.eq_ignore_ascii_case(vardef::SQLSelectLimit) {
            self.SelectLimit = normalized
                .parse()
                .map_err(|_| format!("invalid {} value {normalized}", vardef::SQLSelectLimit))?;
        } else if name.eq_ignore_ascii_case(vardef::TiDBOptEnableAdvancedJoinReorder) {
            self.TiDBOptEnableAdvancedJoinReorder = matches!(
                normalized.to_ascii_lowercase().as_str(),
                "1" | "on" | "true"
            );
        } else if name.eq_ignore_ascii_case(vardef::TiDBOptEnableLateMaterialization) {
            self.EnableLateMaterialization = matches!(
                normalized.to_ascii_lowercase().as_str(),
                "1" | "on" | "true"
            );
        } else if name.eq_ignore_ascii_case(vardef::TiDBOptJoinReorderThreshold) {
            self.TiDBOptJoinReorderThreshold =
                value.trim_matches(['\'', '"']).parse().map_err(|_| {
                    format!(
                        "invalid {} value {value}",
                        vardef::TiDBOptJoinReorderThreshold
                    )
                })?;
        } else if name.eq_ignore_ascii_case(vardef::TiDBOptEnableSemiJoinRewrite) {
            self.EnableSemiJoinRewrite = matches!(
                normalized.to_ascii_lowercase().as_str(),
                "1" | "on" | "true"
            );
        } else if name.eq_ignore_ascii_case(vardef::TiDBEnableFullOuterJoin) {
            self.EnableFullOuterJoin = matches!(
                normalized.to_ascii_lowercase().as_str(),
                "1" | "on" | "true"
            );
        } else if name.eq_ignore_ascii_case(vardef::TiDBDefaultStrMatchSelectivity) {
            self.DefaultStrMatchSelectivity = normalized.parse().map_err(|_| {
                format!(
                    "invalid {} value {normalized}",
                    vardef::TiDBDefaultStrMatchSelectivity
                )
            })?;
        } else if name.eq_ignore_ascii_case(vardef::TiFlashHashAggPreAggMode) {
            self.TiFlashPreAggMode = normalized.clone();
        } else if name.eq_ignore_ascii_case(vardef::TiFlashFineGrainedShuffleStreamCount) {
            self.TiFlashFineGrainedShuffleStreamCount = normalized.parse().map_err(|_| {
                format!(
                    "invalid {} value {normalized}",
                    vardef::TiFlashFineGrainedShuffleStreamCount
                )
            })?;
        } else if name.eq_ignore_ascii_case(vardef::TiDBMaxTiFlashThreads) {
            self.TiFlashMaxThreads = normalized.parse().map_err(|_| {
                format!(
                    "invalid {} value {normalized}",
                    vardef::TiDBMaxTiFlashThreads
                )
            })?;
        } else if name.eq_ignore_ascii_case(vardef::TiDBOptCorrelationExpFactor) {
            self.CorrelationExpFactor = normalized.parse().map_err(|_| {
                format!(
                    "invalid {} value {normalized}",
                    vardef::TiDBOptCorrelationExpFactor
                )
            })?;
        }
        self.systems.insert(name.to_owned(), normalized);
        Ok(())
    }

    /// 通过规范 sysvar 注册表读取 Hint 相关系统变量。
    /// Reads through the canonical sysvar registry used by statement hints.
    pub fn GetHintSystemVar(&self, name: &str) -> Result<String, crate::VariableError> {
        crate::register_builtin_sysvars();
        self.hint_system_vars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .GetSessionOrGlobalSystemVar(&crate::Context, name)
    }

    /// Apply a GLOBAL hook through the session's existing canonical registry
    /// context. SQL callers persist the returned normalized value themselves.
    pub fn ValidateAndSetGlobalSystemVar(
        &self,
        name: &str,
        value: &str,
        scope: vardef::ScopeFlag,
    ) -> Result<(String, Vec<crate::VariableError>), crate::VariableError> {
        crate::register_builtin_sysvars();
        let variable = crate::GetSysVar(name).ok_or_else(|| crate::VariableError::unknown(name))?;
        let mut vars = self
            .hint_system_vars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let before = vars.StmtCtx.warnings().len();
        let normalized = variable.Validate(&mut vars, value, scope)?;
        variable.SetGlobalFromHook(&crate::Context, &mut vars, &normalized, false)?;
        Ok((normalized, vars.StmtCtx.warnings()[before..].to_vec()))
    }

    /// 校验并应用一次 SET_VAR，返回语句前原值以便结束时还原。
    /// Validates and applies one SET_VAR value, returning the raw pre-statement
    /// value so `timestamp=default` remains dynamic after restoration.
    pub fn SetHintSystemVarWithOldState(
        &self,
        name: &str,
        value: &str,
    ) -> Result<String, crate::VariableError> {
        crate::register_builtin_sysvars();
        let system_variable =
            crate::GetSysVar(name).ok_or_else(|| crate::VariableError::unknown(name))?;
        let mut variables = self
            .hint_system_vars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let old_value = variables
            .system(name)
            .map(str::to_owned)
            .unwrap_or_else(|| system_variable.Value.clone());
        let normalized = system_variable.Validate(&mut variables, value, vardef::ScopeSession)?;
        system_variable.SetSessionFromHook(&mut variables, &normalized)?;
        Ok(old_value)
    }

    /// 登记 Hint 变量还原项（同名仅保留首次原值）。
    pub fn AddHintSystemVarRestore(&self, name: &str, old_value: &str) {
        self.hint_system_var_restore
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry(name.to_owned())
            .or_insert_with(|| old_value.to_owned());
    }

    /// 开始新语句的 Hint 处理，清除 FoundInBinding。
    pub fn BeginHintStatement(&self) {
        self.hint_system_vars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .FoundInBinding = false;
    }

    /// 标记本语句命中了执行计划绑定（binding）。
    pub fn MarkHintStatementFromBinding(&self) {
        self.hint_system_vars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .FoundInBinding = true;
    }

    /// 还原全部 SET_VAR（遇错仍继续），并推进 binding 标记。
    /// Restores every SET_VAR value even if one hook fails, then advances the
    /// current binding flag to the read-only `last_plan_from_binding` value.
    pub fn FinishHintStatement(&self) -> Result<(), crate::VariableError> {
        let mut variables = self
            .hint_system_vars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let restore = std::mem::take(
            &mut *self
                .hint_system_var_restore
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let mut first_error = None;
        for (name, value) in restore {
            let result = crate::GetSysVar(&name)
                .ok_or_else(|| crate::VariableError::unknown(&name))
                .and_then(|system_variable| {
                    system_variable.SetSessionFromHook(&mut variables, &value)
                });
            if let Err(error) = result
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        variables.PrevFoundInBinding = variables.FoundInBinding;
        variables.FoundInBinding = false;
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    /// 返回可读的事务模式字符串，空则默认为 OPTIMISTIC。
    pub fn GetReadableTxnMode(&self) -> String {
        if self.TxnMode.is_empty() {
            "OPTIMISTIC".into()
        } else {
            self.TxnMode.clone()
        }
    }
    /// 设置语句上下文的 LastInsertID。
    pub fn SetLastInsertID(&mut self, id: u64) {
        self.StmtCtx.LastInsertID = id;
    }
    /// 设置是否处于显式事务中。
    pub fn SetInTxn(&mut self, value: bool) {
        self.InTxn = value;
    }
    /// 是否处于事务中。
    pub fn InTxn(&self) -> bool {
        self.InTxn
    }
    /// 是否开启自动提交。
    pub fn IsAutocommit(&self) -> bool {
        self.Autocommit
    }
    /// 是否允许 MPP 执行。
    pub fn IsMPPAllowed(&self) -> bool {
        self.AllowMPPExecution
            || self
                .GetSystemVar(vardef::TiDBAllowMPPExecution)
                .is_some_and(|value| {
                    matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true")
                })
    }
    /// TiFlash Cop/BatchCop 是否被禁用；默认只允许 MPP，与 Go SessionVars 一致。
    pub fn IsTiFlashCopBanned(&self) -> bool {
        !self
            .GetSystemVar(vardef::TiDBAllowTiFlashCop)
            .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true"))
    }
    /// 代价模型 CPU 因子。
    pub fn GetCPUFactor(&self) -> f64 {
        self.GetSystemVar(vardef::TiDBOptCPUFactor)
            .and_then(|value| value.parse().ok())
            .unwrap_or(vardef::DefOptCPUFactor)
    }
    /// 代价模型 Coprocessor CPU 因子。
    pub fn GetCopCPUFactor(&self) -> f64 {
        self.GetSystemVar(vardef::TiDBOptCopCPUFactor)
            .and_then(|value| value.parse().ok())
            .unwrap_or(vardef::DefOptCopCPUFactor)
    }
    /// 代价模型内存因子。
    pub fn GetMemoryFactor(&self) -> f64 {
        self.GetSystemVar(vardef::TiDBOptMemoryFactor)
            .and_then(|value| value.parse().ok())
            .unwrap_or(vardef::DefOptMemoryFactor)
    }
    /// 代价模型磁盘因子。
    pub fn GetDiskFactor(&self) -> f64 {
        self.GetSystemVar(vardef::TiDBOptDiskFactor)
            .and_then(|value| value.parse().ok())
            .unwrap_or(vardef::DefOptDiskFactor)
    }
    /// TopN 是否启用部分有序索引优化（值为 COST 时开启）。
    pub fn IsPartialOrderedIndexForTopNEnabled(&self) -> bool {
        self.GetHintSystemVar(vardef::TiDBOptPartialOrderedIndexForTopN)
            .is_ok_and(|value| value == "COST")
    }
    /// 是否启用 Plan Replayer 捕获。
    pub fn IsPlanReplayerCaptureEnabled(&self) -> bool {
        self.EnablePlanReplayerCapture || self.EnablePlanReplayedContinuesCapture
    }
    /// 分配下一个 prepared statement ID。
    pub fn GetNextPreparedStmtID(&mut self) -> u32 {
        self.preparedStmtID += 1;
        self.preparedStmtID
    }
    /// 设置下一个 prepared statement ID 起点。
    pub fn SetNextPreparedStmtID(&mut self, id: u32) {
        self.preparedStmtID = id;
    }
    /// 通过全局变量访问器读取全局系统变量。
    pub fn GetGlobalSystemVar<C>(&self, _ctx: C, name: &str) -> Result<String, String> {
        self.GlobalVarsAccessor
            .get_global_sys_var(name)
            .map_err(|error| error.to_string())
    }
    /// 优先会话、否则全局地读取系统变量。
    pub fn GetSessionOrGlobalSystemVar<C: Clone>(
        &self,
        ctx: C,
        name: &str,
    ) -> Result<String, String> {
        self.GetSystemVar(name)
            .map(Ok)
            .unwrap_or_else(|| self.GetGlobalSystemVar(ctx, name))
    }
    /// 读取会话状态序列化用的系统变量；第二项表示是否存在。
    pub fn GetSessionStatesSystemVar(&self, name: &str) -> (String, bool) {
        match self.systems.get(name).cloned() {
            Some(value) => (value, true),
            None => (String::new(), false),
        }
    }
    /// 不额外校验地设置系统变量（当前仍走 SetSystemVar）。
    pub fn SetSystemVarWithoutValidation(&mut self, name: &str, value: &str) -> Result<(), String> {
        self.SetSystemVar(name, value)
    }
    /// 以放宽校验方式设置系统变量（当前仍走 SetSystemVar）。
    pub fn SetSystemVarWithRelaxedValidation(
        &mut self,
        name: &str,
        value: &str,
    ) -> Result<(), String> {
        self.SetSystemVar(name, value)
    }
    /// 记录本语句相关的优化器变量名。
    pub fn RecordRelevantOptVar(&self, name: &str) {
        if !self
            .record_relevant_opt_vars_and_fixes
            .load(Ordering::Relaxed)
        {
            return;
        }
        self.relevant_opt_vars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_owned());
    }
    /// 记录本语句相关的优化器 fix id。
    pub fn RecordRelevantOptFix(&self, id: u64) {
        if !self
            .record_relevant_opt_vars_and_fixes
            .load(Ordering::Relaxed)
        {
            return;
        }
        self.relevant_opt_fixes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(id);
    }
    /// 返回排序后的相关优化器变量与 fix 列表。
    pub fn RelevantOptVarsAndFixes(&self) -> (Vec<String>, Vec<u64>) {
        let mut vars: Vec<_> = self
            .relevant_opt_vars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect();
        let mut fixes: Vec<_> = self
            .relevant_opt_fixes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .copied()
            .collect();
        vars.sort();
        fixes.sort_unstable();
        (vars, fixes)
    }
    /// 是否允许 IN 子查询转 Join/Agg。
    pub fn GetAllowInSubqToJoinAndAgg(&self) -> bool {
        self.allowInSubqToJoinAndAgg
    }
    /// 设置 IN 子查询转 Join/Agg 开关。
    pub fn SetAllowInSubqToJoinAndAgg(&mut self, value: bool) {
        self.allowInSubqToJoinAndAgg = value;
    }
    /// 设置用户变量字段类型（转发到 `UserVars`）。
    pub fn SetUserVarType(&self, name: impl AsRef<str>, field_type: parser_ast::ast::FieldType) {
        self.UserVars.SetUserVarType(name.as_ref(), field_type);
    }
    /// 获取用户变量字段类型的堆分配副本。
    pub fn GetUserVarType(&self, name: impl AsRef<str>) -> Option<Box<parser_ast::ast::FieldType>> {
        self.UserVars.GetUserVarType(name.as_ref()).map(Box::new)
    }
    /// 注册标量子查询评估上下文，供 EXPLAIN 递归展开。
    pub fn RegisterScalarSubQ<T: Any + 'static>(&self, subquery: T) {
        self.scalar_subqueries.borrow_mut().push(Rc::new(subquery));
    }
    /// 快照当前标量子查询列表。
    pub fn SnapshotScalarSubQueries(&self) -> Vec<Rc<dyn Any>> {
        self.scalar_subqueries.borrow().clone()
    }
    /// 用快照还原标量子查询列表。
    pub fn RestoreScalarSubQueries(&self, subqueries: Vec<Rc<dyn Any>>) {
        *self.scalar_subqueries.borrow_mut() = subqueries;
    }
    /// 快照扩展列唯一 ID 映射。
    pub fn SnapshotExtendedColumnUniqueIDs(&self) -> HashMap<String, i64> {
        self.extended_col_unique_ids.borrow().clone()
    }
    /// 还原扩展列唯一 ID 映射。
    pub fn RestoreExtendedColumnUniqueIDs(&self, values: HashMap<String, i64>) {
        *self.extended_col_unique_ids.borrow_mut() = values;
    }
    /// 插入或覆盖一个扩展列唯一 ID。
    pub fn InsertExtendedColumnUniqueID(&self, hash: impl Into<String>, id: i64) {
        self.extended_col_unique_ids
            .borrow_mut()
            .insert(hash.into(), id);
    }
    /// 按哈希查询扩展列唯一 ID。
    pub fn ExtendedColumnUniqueID(&self, hash: &str) -> Option<i64> {
        self.extended_col_unique_ids.borrow().get(hash).copied()
    }
    /// 快照改写阶段耗时信息。
    pub fn SnapshotRewritePhaseInfo(&self) -> RewritePhaseInfo {
        self.rewrite_phase_info.borrow().clone()
    }
    /// 还原改写阶段耗时信息。
    pub fn RestoreRewritePhaseInfo(&self, value: RewritePhaseInfo) {
        *self.rewrite_phase_info.borrow_mut() = value;
    }
    /// 会话启动至今耗时加上解析耗时，用作慢日志总代价近似。
    pub fn GetTotalCostDuration(&self) -> Duration {
        self.StatementStartTimeValue().elapsed() + self.DurationParseValue()
    }
    pub fn SetStatementStartTime(&self, started: Instant) {
        *self
            .StartTime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = started;
    }
    pub fn StatementStartTimeValue(&self) -> Instant {
        *self
            .StartTime
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn GetExecuteDuration(&self) -> Duration {
        self.StatementStartTimeValue()
            .elapsed()
            .saturating_sub(self.DurationCompile)
    }
    pub fn DurationParseValue(&self) -> Duration {
        *self
            .DurationParse
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn SetDurationParse(&self, duration: Duration) {
        *self
            .DurationParse
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = duration;
    }
    pub fn ResetDurationParse(&self) {
        self.SetDurationParse(Duration::ZERO);
    }
    /// 设置/取出备选轮相关子查询覆盖开关。
    pub fn SetAlternativeCorrelateOverride(&self, value: Option<bool>) -> Option<bool> {
        let mut overrides = self
            .alternative_round_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::replace(&mut overrides.enable_correlate_subquery, value)
    }
    /// 设置/取出备选轮半连接改写覆盖开关。
    pub fn SetAlternativeSemiJoinOverride(&self, value: Option<bool>) -> Option<bool> {
        let mut overrides = self
            .alternative_round_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::replace(&mut overrides.enable_semi_join_rewrite, value)
    }
    /// 设置/取出备选轮全文检索 LIKE 回退覆盖开关。
    pub fn SetAlternativeFTSLikeFallbackOverride(&self, value: Option<bool>) -> Option<bool> {
        let mut overrides = self
            .alternative_round_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::replace(&mut overrides.fts_like_fallback, value)
    }
    /// 备选轮是否启用相关子查询（覆盖优先于会话默认）。
    pub fn AlternativeCorrelateEnabled(&self) -> bool {
        self.alternative_round_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .enable_correlate_subquery
            .unwrap_or(self.EnableCorrelateSubquery)
    }
    /// 备选轮是否启用半连接改写。
    pub fn AlternativeSemiJoinRewriteEnabled(&self) -> bool {
        self.alternative_round_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .enable_semi_join_rewrite
            .unwrap_or(self.EnableSemiJoinRewrite)
    }
    /// 备选轮是否启用 FTS LIKE 回退。
    pub fn AlternativeFTSLikeFallbackEnabled(&self) -> bool {
        self.alternative_round_overrides
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .fts_like_fallback
            .unwrap_or_else(|| self.StmtCtx.AlternativeFTSLikeFallback())
    }
    /// 在回调作用域内访问已注册标量子查询，避免克隆线程亲和上下文。
    /// Visits the actual registered values while keeping the RefCell borrow
    /// scoped to the callback. EXPLAIN can downcast entries to its planner type
    /// without cloning or replacing the captured plan contexts.
    pub fn WithScalarSubQueries<R>(&self, visitor: impl FnOnce(&[Rc<dyn Any>]) -> R) -> R {
        let subqueries = self.scalar_subqueries.borrow();
        visitor(&subqueries)
    }
    /// 已注册标量子查询数量。
    pub fn ScalarSubqueryCount(&self) -> usize {
        self.scalar_subqueries.borrow().len()
    }
    /// 清空相关优化器变量与 fix 集合。
    pub fn ResetRelevantOptVarsAndFixes(&self, record: bool) {
        self.record_relevant_opt_vars_and_fixes
            .store(record, Ordering::Relaxed);
        self.relevant_opt_vars
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        self.relevant_opt_fixes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }
    /// 字符串匹配默认选择率为 0 时启用 TopN 估算。
    pub fn EnableEvalTopNEstimationForStrMatch(&self) -> bool {
        self.GetSystemVar(vardef::TiDBDefaultStrMatchSelectivity)
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(self.DefaultStrMatchSelectivity)
            == 0.0
    }
    /// 字符串匹配默认选择率；为 0 时回退到 0.1。
    pub fn GetStrMatchDefaultSelectivity(&self) -> f64 {
        let configured = self
            .GetSystemVar(vardef::TiDBDefaultStrMatchSelectivity)
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(self.DefaultStrMatchSelectivity);
        if configured == 0.0 { 0.1 } else { configured }
    }
    /// 取反字符串匹配的默认选择率。
    pub fn GetNegateStrMatchDefaultSelectivity(&self) -> f64 {
        let selectivity = self.GetStrMatchDefaultSelectivity();
        if selectivity == vardef::DefOptSelectivityFactor {
            vardef::DefOptSelectivityFactor
        } else {
            1.0 - selectivity
        }
    }
}

/// 提供对 `SessionVars` 的只读访问。
pub trait SessionVarsProvider {
    fn GetSessionVars(&self) -> &SessionVars;
}
impl SessionVarsProvider for SessionVars {
    fn GetSessionVars(&self) -> &SessionVars {
        self
    }
}
/// 连接状态：已关闭。
pub const ConnStatusShutdown: i32 = 2;
static ENABLE_ADAPTIVE_REPLICA_READ: AtomicBool = AtomicBool::new(false);
/// 设置自适应副本读全局开关；返回是否相对旧值发生了变化。
pub fn SetEnableAdaptiveReplicaRead(enabled: bool) -> bool {
    ENABLE_ADAPTIVE_REPLICA_READ.swap(enabled, Ordering::SeqCst) != enabled
}
/// 自适应副本读是否开启。
pub fn IsAdaptiveReplicaReadEnabled() -> bool {
    ENABLE_ADAPTIVE_REPLICA_READ.load(Ordering::SeqCst)
}

#[cfg(test)]
#[path = "scalar_subquery_registry_aster_unit_test.rs"]
mod scalar_subquery_registry_aster_unit_test;

#[cfg(test)]
#[path = "session_test.rs"]
mod session_test;

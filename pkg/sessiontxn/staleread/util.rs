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

// 过期读公共类型与时间戳工具。
//
// 提供会话/后端桩、`AS OF TIMESTAMP` 与 `tidb_read_staleness` 的 TSO
// （Timestamp Oracle，全局时间戳）换算与校验，以及快照 InfoSchema 获取。
// 本文件隔离真实 sessionctx / PD / TiKV 依赖，便于单元测试注入。

use std::sync::{Arc, Mutex};

use crate::{Error, ErrorKind};

/// 合法 TSO 物理时间下界（2013-01-01 00:00:00 UTC 的毫秒），更早则视为无效。
pub const MIN_TSO_PHYSICAL_MS: i64 = 1_356_998_400_000;
/// TSO 逻辑位宽：物理毫秒左移该位数后与逻辑部分拼接。
pub const TSO_LOGICAL_BITS: u32 = 18;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 轻量请求上下文占位（对应 Go 的 context.Context 透传位）。
pub struct Context;

#[derive(Clone, Debug, PartialEq, Eq)]
/// 表达式求值结果：对应 Go `types.Datum` 的过期读相关子集。
pub enum Datum {
    Null,
    String(String),
    Bytes(Vec<u8>),
    Int(i64),
    Uint(u64),
    /// 已解析为毫秒的日期时间字面量。
    DateTimeMillis(i64),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 已编译表达式的桩：本包不解析 SQL，仅携带可求值的规格字符串。
pub struct Expression(pub String);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 快照级信息模式（InfoSchema）：表结构元数据在指定 snapshot_ts 下的视图。
pub struct InfoSchema {
    /// 该 InfoSchema 对应的快照时间戳。
    pub snapshot_ts: u64,
    /// 是否已挂接会话本地临时表。
    pub local_temporary_tables_attached: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 副本读偏好：Leader / Follower / Mixed。
pub enum ReplicaRead {
    #[default]
    Leader,
    Follower,
    Mixed,
}

impl ReplicaRead {
    /// 是否允许走 Follower 读（含 Mixed）。
    pub fn is_follower_read(self) -> bool {
        matches!(self, Self::Follower | Self::Mixed)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 会话事务上下文中与过期读相关的字段快照。
pub struct TransactionContext {
    pub info_schema: InfoSchema,
    /// 事务 start_ts（对过期读即固定读时间戳）。
    pub start_ts: u64,
    pub is_staleness: bool,
    /// 事务作用域（如 global）。
    pub txn_scope: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// KV 快照句柄的桩：携带读 ts、副本偏好与只读标记。
pub struct Snapshot {
    pub ts: u64,
    pub replica_read: ReplicaRead,
    pub staleness_read_only: bool,
    pub temporary_table_interceptor: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 活跃事务句柄的桩。
pub struct Transaction {
    pub start_ts: u64,
    pub valid: bool,
    pub staleness_read_only: bool,
    pub txn_scope: String,
    pub assertion_level: u8,
    pub shard_allocate_step: u64,
    pub temporary_table_interceptor: bool,
    pub snapshot: Snapshot,
}

/// 会话/存储后端抽象：隔离真实 sessionctx、PD、TiKV 依赖，便于单测注入。
pub trait SessionBackend: Send + Sync {
    /// 求值 AS OF 表达式，返回 Datum。
    fn evaluate_expression(&self, expression: &Expression) -> Result<Datum, Error>;
    /// 将 Datum 解析为日期时间毫秒。
    fn parse_datetime_millis(&self, datum: &Datum) -> Result<i64, Error>;
    /// 校验快照读 ts（例如拒绝未来时间）。
    fn validate_snapshot_read_ts(&self, ts: u64) -> Result<(), Error>;
    /// 获取 PD/存储侧 stale timestamp（用于表达式求值缓存预热）。
    fn stale_timestamp(&self) -> Result<u64, Error>;
    /// 当前语句时间戳（毫秒）。
    fn statement_timestamp_millis(&self) -> Result<i64, Error>;
    /// GC safe point 对应的最小可读毫秒（防读已被回收版本）。
    fn statement_min_safe_millis(&self) -> Result<i64, Error>;
    /// 读取 `tidb_external_ts` 全局外部时间戳。
    fn external_timestamp(&self) -> Result<u64, Error>;
    /// 按 ts 取快照 InfoSchema。
    fn snapshot_info_schema(&self, ts: u64) -> Result<InfoSchema, Error>;
    /// 进入新事务前必要时先提交当前事务。
    fn commit_before_enter_new_txn(&self) -> Result<(), Error>;
    /// 以指定 start_ts 创建事务。
    fn create_transaction(&self, ts: u64) -> Result<Transaction, Error>;
    /// 按 ts 创建 KV 快照。
    fn snapshot_with_ts(&self, ts: u64) -> Result<Snapshot, Error>;
}

#[derive(Clone)]
/// 过期读相关的会话状态（生产环境中对应 sessionvars 子集）。
pub struct Session {
    pub backend: Arc<dyn SessionBackend>,
    pub in_txn: bool,
    pub autocommit: bool,
    pub txn_context: Option<TransactionContext>,
    /// `SET TRANSACTION ... AS OF` / `tx_read_ts` 设定的读时间戳。
    pub txn_read_ts: u64,
    pub txn_read_ts_used: bool,
    /// `tidb_read_staleness`：相对当前时间的负偏移毫秒。
    pub read_staleness_millis: i64,
    /// `tidb_enable_external_ts_read`：启用后普通语句钉在外部 ts。
    pub enable_external_ts_read: bool,
    /// 内部/受限 SQL（InRestrictedSQL）不受外部 ts 读影响。
    pub restricted_sql: bool,
    pub statement_is_staleness: bool,
    pub provider_is_staleness: bool,
    pub snapshot_system_variable: String,
    /// Configured transaction scope used for replica reads.
    pub txn_scope_config: String,
    pub replica_read: ReplicaRead,
    pub assertion_level: u8,
    pub shard_allocate_step: u64,
    pub active_transaction: Option<Transaction>,
    /// 会话内 stale_timestamp 结果缓存。
    stale_tso_cache: Option<Result<u64, Error>>,
    /// 会话内 external_timestamp 结果缓存。
    external_ts_cache: Option<Result<u64, Error>>,
}

impl Session {
    /// 用给定后端构造默认会话状态。
    pub fn new(backend: Arc<dyn SessionBackend>) -> Self {
        Self {
            backend,
            in_txn: false,
            autocommit: true,
            txn_context: None,
            txn_read_ts: 0,
            txn_read_ts_used: false,
            read_staleness_millis: 0,
            enable_external_ts_read: false,
            restricted_sql: false,
            statement_is_staleness: false,
            provider_is_staleness: false,
            snapshot_system_variable: String::new(),
            txn_scope_config: "global".to_owned(),
            replica_read: ReplicaRead::Leader,
            assertion_level: 0,
            shard_allocate_step: 0,
            active_transaction: None,
            stale_tso_cache: None,
            external_ts_cache: None,
        }
    }

    /// 标记并返回事务级读 ts（对应 Go 的 UseTxnReadTS）。
    pub fn use_txn_read_ts(&mut self) -> u64 {
        self.txn_read_ts_used = true;
        self.txn_read_ts
    }

    /// Reset statement-scoped caches and flags, matching Go's fresh
    /// StatementContext at the beginning of each statement.
    pub fn begin_statement(&mut self) {
        self.statement_is_staleness = false;
        self.stale_tso_cache = None;
        self.external_ts_cache = None;
    }
}

/// 线程安全的会话引用。
pub type SessionRef = Arc<Mutex<Session>>;

/// 将 `AS OF TIMESTAMP` 表达式求值为 TSO，并做合法性校验。
pub fn calculate_as_of_ts_expr(
    session: &SessionRef,
    expression: &Expression,
) -> Result<u64, Error> {
    let (backend, datum) = {
        let mut session = session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?;
        // 预热 stale_timestamp 缓存（与 Go 侧在求值路径上的缓存行为一致）。
        if session.stale_tso_cache.is_none() {
            session.stale_tso_cache = Some(session.backend.stale_timestamp());
        }
        let backend = Arc::clone(&session.backend);
        let datum = backend.evaluate_expression(expression)?;
        (backend, datum)
    };
    if datum == Datum::Null {
        return Err(Error::as_of("as of timestamp cannot be NULL"));
    }
    // 优先按日期时间解析；失败再尝试把 Datum 当作原始 TSO。
    if let Ok(milliseconds) = backend.parse_datetime_millis(&datum) {
        return millis_to_tso(milliseconds);
    }
    let tso = tso_from_datum(&datum).ok_or_else(|| {
        Error::as_of("cannot parse AS OF TIMESTAMP expression as datetime or TSO")
    })?;
    if extract_physical(tso) <= MIN_TSO_PHYSICAL_MS {
        return Err(Error::as_of(
            "invalid TSO timestamp: TSO is before 2013-01-01",
        ));
    }
    backend.validate_snapshot_read_ts(tso)?;
    Ok(tso)
}

/// 从字符串/字节/整数 Datum 解析正数 TSO。
pub fn tso_from_datum(datum: &Datum) -> Option<u64> {
    match datum {
        // Go's strconv.ParseUint accepts the string "0"; the caller then
        // classifies it as an invalid pre-2013 TSO. Preserve that error
        // branch instead of turning zero into a generic parse failure.
        Datum::String(value) => value.parse().ok(),
        Datum::Bytes(value) => std::str::from_utf8(value).ok()?.parse().ok(),
        Datum::Int(value) if *value > 0 => Some(*value as u64),
        Datum::Uint(value) if *value > 0 => Some(*value),
        _ => None,
    }
}

/// 按 `tidb_read_staleness`（相对当前时间的偏移毫秒）计算读 ts，并钳制到 GC safe point。
pub fn calculate_ts_with_read_staleness(
    session: &SessionRef,
    staleness_millis: i64,
) -> Result<u64, Error> {
    let backend = Arc::clone(
        &session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?
            .backend,
    );
    let now = backend.statement_timestamp_millis()?;
    let requested = now.saturating_add(staleness_millis);
    let safe = backend.statement_min_safe_millis()?;
    // 目标读时间不能早于 safe point；若 safe 异常大于 now 则退回 now。
    let calculated = if safe < requested {
        requested
    } else if safe > now {
        now
    } else {
        safe
    };
    let read_ts = millis_to_tso(calculated)?;
    if calculated > safe {
        backend.validate_snapshot_read_ts(read_ts)?;
    }
    Ok(read_ts)
}

/// 当前语句是否被标记为过期读。
pub fn is_stmt_staleness(session: &SessionRef) -> bool {
    session
        .lock()
        .map(|session| session.statement_is_staleness)
        .unwrap_or(false)
}

/// 解析并缓存外部时间戳（`tidb_external_ts`）；后端错误包装为 AsOf。
pub fn get_external_timestamp(session: &SessionRef) -> Result<u64, Error> {
    let mut session = session
        .lock()
        .map_err(|_| Error::backend("session lock poisoned"))?;
    if session.external_ts_cache.is_none() {
        let timestamp = session
            .backend
            .external_timestamp()
            .map_err(|error| Error::new(ErrorKind::AsOf, error.message))?;
        session.external_ts_cache = Some(Ok(timestamp));
    }
    session
        .external_ts_cache
        .clone()
        .expect("cache was initialized")
        .map_err(|error| Error::new(ErrorKind::AsOf, error.message))
}

/// 按 snapshot_ts 取 InfoSchema，并强制挂接本地临时表标记。
pub fn get_session_snapshot_info_schema(
    session: &SessionRef,
    snapshot_ts: u64,
) -> Result<InfoSchema, Error> {
    let backend = Arc::clone(
        &session
            .lock()
            .map_err(|_| Error::backend("session lock poisoned"))?
            .backend,
    );
    let mut info = backend.snapshot_info_schema(snapshot_ts)?;
    info.local_temporary_tables_attached = true;
    Ok(info)
}

/// 毫秒物理时间转为 TSO（左移逻辑位）。
pub fn millis_to_tso(milliseconds: i64) -> Result<u64, Error> {
    if milliseconds < 0 {
        return Err(Error::new(
            ErrorKind::InvalidTimestamp,
            "negative timestamp",
        ));
    }
    Ok((milliseconds as u64) << TSO_LOGICAL_BITS)
}

/// 从 TSO 提取物理毫秒部分。
pub fn extract_physical(tso: u64) -> i64 {
    (tso >> TSO_LOGICAL_BITS) as i64
}

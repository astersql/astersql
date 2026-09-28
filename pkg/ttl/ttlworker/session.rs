// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL worker 会话抽象：Datum/行、物理表元数据、会话变量准备与工作校验。
//
// `WorkerSession` 封装执行 SQL 的能力；`prepare_session` / `restore_session`
// 在任务前后切换会话变量（关闭重试、开启 1PC/异步提交、固定时区等）。
// `validate_ttl_work` 在表定义变更、TTL 关闭或过期间隔缩短时中止工作，
// 避免用过期元数据继续删除。

use std::collections::BTreeMap;

/// SQL 参数与结果单元格的简化取值类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Datum {
    /// SQL NULL。
    Null,
    /// 有符号整数。
    Integer(i64),
    /// 无符号整数（常用于时间戳水位）。
    Unsigned(u64),
    /// 文本。
    Text(String),
    /// 字节序列。
    Bytes(Vec<u8>),
}

/// 一行数据：按列顺序的 `Datum` 向量。
pub type Row = Vec<Datum>;

/// TTL 可见的物理表元数据（逻辑表 + 分区/非分区物理 ID）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhysicalTable {
    /// 逻辑表 ID。
    pub table_id: i64,
    /// 物理表/分区 ID。
    pub physical_id: i64,
    /// schema（库）名。
    pub schema: String,
    /// 表名。
    pub table: String,
    /// 主键（或扫描键）列名列表。
    pub key_columns: Vec<String>,
    /// TTL 时间列名。
    pub ttl_column: String,
    /// 是否启用 TTL。
    pub ttl_enabled: bool,
    /// 表定义版本；供缓存与调用方识别元数据快照。
    pub definition_version: u64,
    /// 过期间隔（秒）：行存活时长。
    pub expire_after_seconds: u64,
}

impl PhysicalTable {
    /// 计算过期水位：`now - expire_after_seconds`（下溢时饱和为 0）。
    pub fn expire_time(&self, now: u64) -> u64 {
        now.saturating_sub(self.expire_after_seconds)
    }
}

/// worker 会话可变状态：系统变量、事务标志与时区偏移。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionState {
    /// 会话级系统变量名 → 值。
    pub variables: BTreeMap<String, String>,
    /// 是否处于显式事务中。
    pub in_transaction: bool,
    /// 时区相对 UTC 的偏移秒数。
    pub timezone_offset_seconds: i32,
    /// 是否允许内部 SQL 扫描用户表。
    pub internal_sql_scan_user_table: bool,
    /// 分布式 SQL 扫描并发度。
    pub distsql_scan_concurrency: usize,
    /// 是否启用分页下推。
    pub enable_paging: bool,
}

/// TTL worker 使用的会话接口：读写状态并执行参数化 SQL。
pub trait WorkerSession {
    /// 只读访问会话状态。
    fn state(&self) -> &SessionState;
    /// 可变访问会话状态。
    fn state_mut(&mut self) -> &mut SessionState;
    /// 执行 SQL，返回结果行或会话错误。
    fn execute(&mut self, sql: &str, args: &[Datum]) -> Result<Vec<Row>, SessionError>;
    /// 全局 TTL job 开关。默认开启，便于轻量测试会话只实现 SQL 接口。
    fn ttl_jobs_enabled(&self) -> bool {
        true
    }
    /// 在执行前同步全局时区；真实会话可覆盖并传播同步错误。
    fn reset_with_global_timezone(&mut self) -> Result<(), SessionError> {
        Ok(())
    }
    /// 在乐观事务内执行 SQL；真实会话可覆盖以暴露 begin/commit 错误。
    fn execute_in_transaction(
        &mut self,
        sql: &str,
        args: &[Datum],
    ) -> Result<Vec<Row>, SessionError> {
        self.execute(sql, args)
    }
    /// Mark the underlying pooled session as unsafe to reuse after lifecycle failure.
    fn avoid_reuse(&mut self) {}
}

/// 会话/TTL 工作过程中的错误类别。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionError {
    /// 通用执行失败，携带消息。
    Execute(String),
    /// 执行失败但表元数据仍有效；Go 的 `ExecuteSQLWithCheck` 将此类错误
    /// 标记为不可重试，并直接计入错误行。
    NonRetryable(String),
    /// 表 ID/物理 ID/定义版本变化。
    TableChanged,
    /// TTL 已被关闭。
    TtlDisabled,
    /// 过期间隔变化导致新的过期水位早于作业水位。
    ExpireIntervalChanged,
}

/// 为 TTL 工作准备会话：退出事务并设置推荐系统变量；返回先前状态以便恢复。
///
/// 关键变量含：关闭 tidb_retry_limit、开启 1PC（单阶段提交）与异步提交、
/// 指定读引擎、固定 `time_zone` 为 UTC。
pub fn prepare_session(session: &mut dyn WorkerSession) -> SessionState {
    let previous = session.state().clone();
    let state = session.state_mut();
    state.in_transaction = false;
    state
        .variables
        .insert("tidb_retry_limit".into(), "0".into());
    state
        .variables
        .insert("tidb_enable_1pc".into(), "ON".into());
    state
        .variables
        .insert("tidb_enable_async_commit".into(), "ON".into());
    let has_all_read_engines = state
        .variables
        .get("tidb_isolation_read_engines")
        .map(|value| {
            let engines = value
                .split(',')
                .map(|engine| engine.trim().to_ascii_lowercase())
                .collect::<std::collections::BTreeSet<_>>();
            ["tidb", "tikv", "tiflash"]
                .iter()
                .all(|engine| engines.contains(*engine))
        })
        .unwrap_or(false);
    if !has_all_read_engines {
        state.variables.insert(
            "tidb_isolation_read_engines".into(),
            "tikv,tiflash,tidb".into(),
        );
    }
    state.variables.insert("time_zone".into(), "UTC".into());
    state.timezone_offset_seconds = 0;
    previous
}

/// 将会话状态恢复为 `prepare_session` 之前的快照。
pub fn restore_session(session: &mut dyn WorkerSession, previous: SessionState) {
    *session.state_mut() = previous;
}

/// SQL-backed variant of [`prepare_session`] used at pooled-session boundaries.
///
/// It preserves the Go ordering so every individual setup statement can fail.
pub fn prepare_session_checked(
    session: &mut dyn WorkerSession,
) -> Result<SessionState, SessionError> {
    let previous = session.state().clone();
    for sql in [
        "set tidb_retry_limit=0",
        "set tidb_enable_1pc=ON",
        "set tidb_enable_async_commit=ON",
        "ROLLBACK",
        "set @@time_zone='UTC'",
    ] {
        if let Err(error) = execute_lifecycle_sql(session, sql) {
            session.avoid_reuse();
            return Err(error);
        }
    }
    let has_all_read_engines = previous
        .variables
        .get("tidb_isolation_read_engines")
        .map(|value| {
            let engines = value
                .split(',')
                .map(|engine| engine.trim().to_ascii_lowercase())
                .collect::<std::collections::BTreeSet<_>>();
            ["tidb", "tikv", "tiflash"]
                .iter()
                .all(|engine| engines.contains(*engine))
        })
        .unwrap_or(false);
    if !has_all_read_engines
        && let Err(error) = execute_lifecycle_sql(
            session,
            "set tidb_isolation_read_engines='tikv,tiflash,tidb'",
        )
    {
        session.avoid_reuse();
        return Err(error);
    }
    let _ = prepare_session(session);
    Ok(previous)
}

/// Restore a pooled session through the same observable SQL steps as Go.
pub fn restore_session_checked(
    session: &mut dyn WorkerSession,
    previous: SessionState,
) -> Result<(), SessionError> {
    let retry_limit = previous
        .variables
        .get("tidb_retry_limit")
        .map(String::as_str)
        .unwrap_or("0");
    let mut statements = vec![format!("set tidb_retry_limit={retry_limit}")];
    if previous
        .variables
        .get("tidb_enable_1pc")
        .map(String::as_str)
        != Some("ON")
    {
        statements.push("set tidb_enable_1pc=OFF".into());
    }
    if previous
        .variables
        .get("tidb_enable_async_commit")
        .map(String::as_str)
        != Some("ON")
    {
        statements.push("set tidb_enable_async_commit=OFF".into());
    }
    statements.push(format!(
        "set @@time_zone={}",
        previous
            .variables
            .get("time_zone")
            .map(String::as_str)
            .unwrap_or("SYSTEM")
    ));
    if let Some(engines) = previous.variables.get("tidb_isolation_read_engines") {
        statements.push(format!("set tidb_isolation_read_engines={engines}"));
    }
    for sql in statements {
        if let Err(error) = execute_lifecycle_sql(session, &sql) {
            session.avoid_reuse();
            return Err(error);
        }
    }
    restore_session(session, previous);
    Ok(())
}

/// `NewScanSession` 会临时修改的三项会话状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanSessionState {
    internal_sql_scan_user_table: bool,
    distsql_scan_concurrency: usize,
    enable_paging: bool,
}

/// 为 TTL 扫描准备会话：标记内部用户表访问、限制扫描并发并关闭分页。
pub fn prepare_scan_session(session: &mut dyn WorkerSession) -> ScanSessionState {
    let state = session.state_mut();
    let previous = ScanSessionState {
        internal_sql_scan_user_table: state.internal_sql_scan_user_table,
        distsql_scan_concurrency: state.distsql_scan_concurrency,
        enable_paging: state.enable_paging,
    };
    state.internal_sql_scan_user_table = true;
    state.distsql_scan_concurrency = 1;
    state.enable_paging = false;
    previous
}

/// 恢复 `prepare_scan_session` 修改的扫描会话状态。
pub fn restore_scan_session(session: &mut dyn WorkerSession, previous: ScanSessionState) {
    let state = session.state_mut();
    state.internal_sql_scan_user_table = previous.internal_sql_scan_user_table;
    state.distsql_scan_concurrency = previous.distsql_scan_concurrency;
    state.enable_paging = previous.enable_paging;
}

/// Fallible scan setup matching Go's statement ordering and cleanup behavior.
pub fn prepare_scan_session_checked(
    session: &mut dyn WorkerSession,
) -> Result<ScanSessionState, SessionError> {
    let previous = prepare_scan_session(session);
    for sql in [
        "set @@tidb_distsql_scan_concurrency=1",
        "set @@tidb_enable_paging=OFF",
    ] {
        if let Err(error) = execute_lifecycle_sql(session, sql) {
            let _ = restore_scan_session_checked(session, previous);
            return Err(error);
        }
    }
    Ok(previous)
}

/// Restore both scan variables even when the first restoration fails.
pub fn restore_scan_session_checked(
    session: &mut dyn WorkerSession,
    previous: ScanSessionState,
) -> Result<(), SessionError> {
    session.state_mut().internal_sql_scan_user_table = false;
    let first = execute_lifecycle_sql(
        session,
        &format!(
            "set @@tidb_distsql_scan_concurrency={}",
            previous.distsql_scan_concurrency
        ),
    );
    let second = execute_lifecycle_sql(
        session,
        &format!("set @@tidb_enable_paging={}", previous.enable_paging),
    );
    if first.is_err() || second.is_err() {
        session.avoid_reuse();
        return first.and(second).map(|_| ());
    }
    restore_scan_session(session, previous);
    Ok(())
}

fn execute_lifecycle_sql(
    session: &mut dyn WorkerSession,
    sql: &str,
) -> Result<Vec<Row>, SessionError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| session.execute(sql, &[]))) {
        Ok(result) => result,
        Err(payload) => {
            session.avoid_reuse();
            std::panic::resume_unwind(payload)
        }
    }
}

/// 校验当前表元数据相对作业开始时是否仍可继续 TTL 工作。
///
/// 表消失、ID 或 TTL 时间列变化 → `TableChanged`；TTL 关闭 → `TtlDisabled`；
/// 当前计算的过期水位早于作业水位 → `ExpireIntervalChanged`。其它列和索引等安全
/// 元数据变化不会中止任务，与 Go 的 `validateTTLWork` 一致。
pub fn validate_ttl_work(
    original: &PhysicalTable,
    current: Option<&PhysicalTable>,
    expire_time: u64,
    now: u64,
) -> Result<(), SessionError> {
    let current = current.ok_or(SessionError::TableChanged)?;
    if current.table_id != original.table_id || current.physical_id != original.physical_id {
        return Err(SessionError::TableChanged);
    }
    if !current.ttl_enabled {
        return Err(SessionError::TtlDisabled);
    }
    if current.ttl_column != original.ttl_column {
        return Err(SessionError::TableChanged);
    }
    if current.expire_time(now) < expire_time {
        return Err(SessionError::ExpireIntervalChanged);
    }
    Ok(())
}

/// 绑定到某物理表与过期水位的会话包装，执行失败时先做 TTL 工作校验。
pub struct TableSession<'a> {
    /// 底层 worker 会话。
    pub session: &'a mut dyn WorkerSession,
    /// 作业开始时的表快照。
    pub table: PhysicalTable,
    /// 作业锁定的过期水位。
    pub expire_time: u64,
}

impl TableSession<'_> {
    /// 执行 SQL；若失败则调用 `validate_ttl_work`，优先返回元数据变更类错误。
    pub fn execute_sql_with_check(
        &mut self,
        sql: &str,
        args: &[Datum],
        current: Option<&PhysicalTable>,
        now: u64,
    ) -> Result<(Vec<Row>, bool), SessionError> {
        if !self.session.ttl_jobs_enabled() {
            return Err(SessionError::TtlDisabled);
        }
        self.session.reset_with_global_timezone()?;
        let execution = self.session.execute_in_transaction(sql, args);

        // Go deliberately validates after the statement on both success and
        // failure: only then is the transaction's metadata snapshot known.
        validate_ttl_work(&self.table, current, self.expire_time, now)?;
        execution.map(|rows| (rows, false))
    }
}

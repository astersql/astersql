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

// TTL 专用会话实现：事务包装、SQL 执行、时区复位与语句打断。
//
// 对应 Go `ttl/session`：在通用 SessionContext 之上提供 TTL 路径所需的
// 乐观/悲观事务（Optimistic/Pessimistic）、request source 标记与相位钩子。

use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// 事务模式：乐观（Optimistic）先写后检测冲突；悲观（Pessimistic）加锁。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxnMode {
    Optimistic,
    Pessimistic,
    Unknown(i32),
}

/// TTL 会话路径可返回的错误类别。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionError {
    /// SQL 执行失败。
    Sql(String),
    /// 读取全局变量失败。
    GlobalVariable(String),
    /// 时区字符串非法。
    InvalidTimeZone(String),
    /// 未知事务模式。
    UnknownTransactionMode,
}

impl Display for SessionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sql(error) => write!(f, "SQL error: {error}"),
            Self::GlobalVariable(error) => write!(f, "global variable error: {error}"),
            Self::InvalidTimeZone(value) => write!(f, "invalid time zone: {value}"),
            Self::UnknownTransactionMode => write!(f, "unknown transaction mode"),
        }
    }
}

impl std::error::Error for SessionError {}

/// 请求来源标记；TTL 路径会强制设为 `Ttl`。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RequestSource {
    #[default]
    Unspecified,
    Ttl,
}

/// 单次 SQL/事务执行上下文：取消标志、截止时间与可选相位追踪器。
#[derive(Clone)]
pub struct ExecutionContext {
    /// 请求来源。
    pub request_source: RequestSource,
    cancelled: Arc<AtomicBool>,
    deadline: Option<Instant>,
    phase_tracer: Option<Arc<PhaseTracer>>,
}

impl Default for ExecutionContext {
    fn default() -> Self {
        Self {
            request_source: RequestSource::Unspecified,
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: None,
            phase_tracer: None,
        }
    }
}

impl ExecutionContext {
    /// 带超时截止时间的上下文。
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            deadline: Some(Instant::now() + timeout),
            ..Self::default()
        }
    }

    /// 挂载相位追踪器。
    pub fn with_phase_tracer(mut self, tracer: Arc<PhaseTracer>) -> Self {
        self.phase_tracer = Some(tracer);
        self
    }

    /// 标记取消。
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// 已取消或已超时则视为完成。
    pub fn is_done(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// 取出相位追踪器。
    pub fn phase_tracer(&self) -> Option<Arc<PhaseTracer>> {
        self.phase_tracer.clone()
    }
}

/// 事务包装过程中对外暴露的粗粒度相位。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Phase {
    BeginTransaction,
    CommitTransaction,
    #[default]
    Other,
}

/// 会话级相位追踪器（与 metrics 包中的细粒度 PhaseTracer 不同）。
#[derive(Default)]
pub struct PhaseTracer {
    phase: Mutex<Phase>,
}

impl PhaseTracer {
    /// 当前相位。
    pub fn phase(&self) -> Phase {
        *self.phase.lock().unwrap()
    }

    /// 进入指定相位。
    pub fn enter_phase(&self, phase: Phase) {
        *self.phase.lock().unwrap() = phase;
    }
}

/// SQL 参数/结果单元格值。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Unsigned(u64),
    String(String),
    Bytes(Vec<u8>),
}

/// 一行结果。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Row {
    /// 单元格值列表。
    pub values: Vec<SqlValue>,
}

/// 结果集：按批 drain 并关闭。
pub trait RecordSet: Send {
    fn drain(&mut self, batch_size: usize) -> Result<Vec<Row>, SessionError>;
    fn close(&mut self) -> Result<(), SessionError>;
}

/// 内部 SQL 执行器抽象。
pub trait SqlExecutor: Send + Sync {
    fn execute_internal(
        &self,
        context: &ExecutionContext,
        sql: &str,
        arguments: &[SqlValue],
    ) -> Result<Option<Box<dyn RecordSet>>, SessionError>;
}

/// 存储引擎占位 trait（TTL 会话透传）。
pub trait Storage: Send + Sync {}
/// 仅元数据 InfoSchema 占位 trait。
pub trait MetaOnlyInfoSchema: Send + Sync {}

/// 时区：名称与相对 UTC 的偏移秒数。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TimeZone {
    /// 时区名称（UTC / ±HH:MM / IANA）。
    pub name: String,
    /// 相对 UTC 的偏移秒数。
    pub offset_seconds: i32,
}

impl TimeZone {
    /// 解析 UTC、`±HH:MM` 或 IANA 名称；命名区偏移留给宿主时区库解析。
    pub fn parse(value: &str) -> Result<Self, SessionError> {
        if value.eq_ignore_ascii_case("UTC") || value == "+00:00" || value == "-00:00" {
            return Ok(Self {
                name: value.to_owned(),
                offset_seconds: 0,
            });
        }
        if let Some(sign) = value
            .as_bytes()
            .first()
            .filter(|sign| **sign == b'+' || **sign == b'-')
        {
            let (hours, minutes) = value[1..]
                .split_once(':')
                .ok_or_else(|| SessionError::InvalidTimeZone(value.to_owned()))?;
            let hours: i32 = hours
                .parse()
                .map_err(|_| SessionError::InvalidTimeZone(value.to_owned()))?;
            let minutes: i32 = minutes
                .parse()
                .map_err(|_| SessionError::InvalidTimeZone(value.to_owned()))?;
            if hours > 14 || minutes > 59 {
                return Err(SessionError::InvalidTimeZone(value.to_owned()));
            }
            let offset = (hours * 3600 + minutes * 60) * if *sign == b'-' { -1 } else { 1 };
            return Ok(Self {
                name: value.to_owned(),
                offset_seconds: offset,
            });
        }
        if value.is_empty() {
            Err(SessionError::InvalidTimeZone(value.to_owned()))
        } else {
            // Named IANA zones retain their identity. Offset resolution belongs
            // to the embedding server's timezone database.
            // 命名 IANA 时区只保留名称，偏移由宿主时区库解析。
            Ok(Self {
                name: value.to_owned(),
                offset_seconds: 0,
            })
        }
    }
}

/// 会话变量：会话/全局时区、location 与 killed 标志。
pub struct SessionVariables {
    time_zone: Mutex<Option<String>>,
    global_time_zone: Mutex<String>,
    location: Mutex<TimeZone>,
    killed: AtomicI32,
}

impl Default for SessionVariables {
    fn default() -> Self {
        Self {
            time_zone: Mutex::new(None),
            global_time_zone: Mutex::new("UTC".to_owned()),
            location: Mutex::new(TimeZone {
                name: "UTC".to_owned(),
                offset_seconds: 0,
            }),
            killed: AtomicI32::new(0),
        }
    }
}

impl SessionVariables {
    /// 设置会话级 time_zone；None 表示回退到全局。
    pub fn set_time_zone(&self, value: Option<String>) {
        *self.time_zone.lock().unwrap() = value;
    }

    /// 设置全局 time_zone 缓存。
    pub fn set_global_time_zone(&self, value: impl Into<String>) {
        *self.global_time_zone.lock().unwrap() = value.into();
    }

    /// 读取指定全局系统变量（当前仅支持 time_zone）。
    pub fn global_system_variable(&self, name: &str) -> Result<String, SessionError> {
        if name == "time_zone" {
            Ok(self.global_time_zone.lock().unwrap().clone())
        } else {
            Err(SessionError::GlobalVariable(name.to_owned()))
        }
    }

    /// 会话 time_zone 优先，否则用全局。
    pub fn session_or_global_time_zone(&self) -> String {
        self.time_zone
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| self.global_time_zone.lock().unwrap().clone())
    }

    /// 更新 location（NOW() 等使用）。
    pub fn set_location(&self, location: TimeZone) {
        *self.location.lock().unwrap() = location;
    }

    /// 当前 location。
    pub fn location(&self) -> TimeZone {
        self.location.lock().unwrap().clone()
    }

    /// 标记语句被打断（对应 Go Killed）。
    pub fn send_query_interrupted(&self) {
        self.killed.store(1, Ordering::Release);
    }

    /// 查询是否已被打断。
    pub fn is_query_interrupted(&self) -> bool {
        self.killed.load(Ordering::Acquire) == 1
    }
}

/// 宿主注入的会话上下文：存储、变量、InfoSchema 与 SQL 执行器。
pub trait SessionContext: Send + Sync {
    fn store(&self) -> Arc<dyn Storage>;
    fn session_variables(&self) -> Arc<SessionVariables>;
    fn latest_info_schema(&self) -> Arc<dyn MetaOnlyInfoSchema>;
    fn transaction_info_schema(&self) -> Arc<dyn MetaOnlyInfoSchema>;
    fn sql_executor(&self) -> Arc<dyn SqlExecutor>;
}

/// TTL 会话对外接口。
pub trait Session: Send + Sync {
    fn store(&self) -> Arc<dyn Storage>;
    fn session_variables(&self) -> Arc<SessionVariables>;
    fn latest_info_schema(&self) -> Arc<dyn MetaOnlyInfoSchema>;
    fn session_info_schema(&self) -> Arc<dyn MetaOnlyInfoSchema>;
    fn sql_executor(&self) -> Arc<dyn SqlExecutor>;
    fn execute_sql(
        &self,
        context: &ExecutionContext,
        sql: &str,
        arguments: &[SqlValue],
    ) -> Result<Vec<Row>, SessionError>;
    fn run_in_transaction(
        &self,
        context: &ExecutionContext,
        callback: &mut dyn FnMut() -> Result<(), SessionError>,
        mode: TxnMode,
    ) -> Result<(), SessionError>;
    fn reset_with_global_time_zone(&self, context: &ExecutionContext) -> Result<(), SessionError>;
    fn global_time_zone(&self, context: &ExecutionContext) -> Result<TimeZone, SessionError>;
    fn kill_statement(&self);
    fn now(&self) -> (SystemTime, TimeZone);
    fn avoid_reuse(&self);
}

/// TTL 会话具体实现。
pub struct TtlSession {
    context: Arc<dyn SessionContext>,
    sql_executor: Arc<dyn SqlExecutor>,
    avoid_reuse: Mutex<Box<dyn FnMut() + Send>>,
}

/// Mirrors Go's deferred transaction cleanup, including during panic unwinding.
struct TransactionCleanup<'a> {
    session: &'a TtlSession,
    tracer: Option<Arc<PhaseTracer>>,
    old_phase: Option<Phase>,
    success: bool,
}

impl Drop for TransactionCleanup<'_> {
    fn drop(&mut self) {
        if !self.success {
            // Use an independent context so cancellation of the caller cannot
            // suppress rollback, matching the one-second Go cleanup context.
            let rollback_context = ExecutionContext::with_timeout(Duration::from_secs(1));
            let _ = self.session.execute_sql(&rollback_context, "ROLLBACK", &[]);
        }
        if let (Some(tracer), Some(old_phase)) = (&self.tracer, self.old_phase) {
            tracer.enter_phase(old_phase);
        }
    }
}

impl TtlSession {
    /// 从 SessionContext 构造；`avoid_reuse` 在连接不可复用时回调。
    pub fn new(
        context: Arc<dyn SessionContext>,
        avoid_reuse: impl FnMut() + Send + 'static,
    ) -> Self {
        let sql_executor = context.sql_executor();
        Self {
            context,
            sql_executor,
            avoid_reuse: Mutex::new(Box::new(avoid_reuse)),
        }
    }
}

impl Session for TtlSession {
    fn store(&self) -> Arc<dyn Storage> {
        self.context.store()
    }

    fn session_variables(&self) -> Arc<SessionVariables> {
        self.context.session_variables()
    }

    fn latest_info_schema(&self) -> Arc<dyn MetaOnlyInfoSchema> {
        self.context.latest_info_schema()
    }

    fn session_info_schema(&self) -> Arc<dyn MetaOnlyInfoSchema> {
        self.context.transaction_info_schema()
    }

    fn sql_executor(&self) -> Arc<dyn SqlExecutor> {
        Arc::clone(&self.sql_executor)
    }

    fn execute_sql(
        &self,
        context: &ExecutionContext,
        sql: &str,
        arguments: &[SqlValue],
    ) -> Result<Vec<Row>, SessionError> {
        let mut context = context.clone();
        // TTL 路径统一标记 request source，便于审计与限流分类。
        context.request_source = RequestSource::Ttl;
        let Some(mut record_set) = self
            .sql_executor
            .execute_internal(&context, sql, arguments)?
        else {
            return Ok(Vec::new());
        };
        let result = record_set.drain(8);
        let _ = record_set.close();
        result
    }

    fn run_in_transaction(
        &self,
        context: &ExecutionContext,
        callback: &mut dyn FnMut() -> Result<(), SessionError>,
        mode: TxnMode,
    ) -> Result<(), SessionError> {
        let tracer = context.phase_tracer();
        let old_phase = tracer.as_ref().map(|tracer| tracer.phase());
        let mut cleanup = TransactionCleanup {
            session: self,
            tracer: tracer.clone(),
            old_phase,
            success: false,
        };
        if let Some(tracer) = &tracer {
            tracer.enter_phase(Phase::BeginTransaction);
        }
        let result = (|| {
            let begin = match mode {
                TxnMode::Optimistic => "BEGIN OPTIMISTIC",
                TxnMode::Pessimistic => "BEGIN PESSIMISTIC",
                TxnMode::Unknown(_) => return Err(SessionError::UnknownTransactionMode),
            };
            self.execute_sql(context, begin, &[])?;
            if let Some(tracer) = &tracer {
                tracer.enter_phase(Phase::Other);
            }
            callback()?;
            if let Some(tracer) = &tracer {
                tracer.enter_phase(Phase::CommitTransaction);
            }
            self.execute_sql(context, "COMMIT", &[])?;
            if let Some(tracer) = &tracer {
                tracer.enter_phase(Phase::Other);
            }
            Ok(())
        })();
        cleanup.success = result.is_ok();
        result
    }

    fn reset_with_global_time_zone(&self, context: &ExecutionContext) -> Result<(), SessionError> {
        let variables = self.context.session_variables();
        // 会话时区已与全局一致时可跳过 SET。
        if variables.time_zone.lock().unwrap().is_some() {
            let global = variables.global_system_variable("time_zone")?;
            if global == variables.session_or_global_time_zone() {
                return Ok(());
            }
        }
        self.execute_sql(context, "SET @@time_zone=@@global.time_zone", &[])?;
        let global = variables.global_system_variable("time_zone")?;
        variables.set_time_zone(Some(global.clone()));
        variables.set_location(TimeZone::parse(&global)?);
        Ok(())
    }

    fn global_time_zone(&self, _context: &ExecutionContext) -> Result<TimeZone, SessionError> {
        TimeZone::parse(
            &self
                .context
                .session_variables()
                .global_system_variable("time_zone")?,
        )
    }

    fn kill_statement(&self) {
        self.context.session_variables().send_query_interrupted();
    }

    fn now(&self) -> (SystemTime, TimeZone) {
        (
            SystemTime::now(),
            self.context.session_variables().location(),
        )
    }

    fn avoid_reuse(&self) {
        (self.avoid_reuse.lock().unwrap())();
    }
}

/// 工厂：构造 `Arc<dyn Session>`。
pub fn new_session(
    context: Arc<dyn SessionContext>,
    avoid_reuse: impl FnMut() + Send + 'static,
) -> Arc<dyn Session> {
    Arc::new(TtlSession::new(context, avoid_reuse))
}

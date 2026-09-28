// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// DDL 内部会话抽象与包装。
//
// 定义会话错误、事务模式、SQL 值/行、执行上下文，以及 `SessionContext` trait
// 与高层 `Session` 包装器。DDL 后台作业通过这些接口在内部会话上执行 SQL，
// 并支持乐观/悲观事务（transaction）与失败注入（failpoint）联调。

use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Eq, PartialEq)]
/// DDL 内部会话相关错误。
pub enum SessionError {
    /// 事务层面错误。
    Transaction(String),
    /// SQL 执行错误。
    Sql(String),
    /// 会话池已关闭。
    PoolClosed,
    /// 资源类型不符合预期。
    InvalidResource(String),
    /// 不支持的资源池实现。
    UnsupportedPool(String),
}

impl Display for SessionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transaction(error) => write!(f, "transaction error: {error}"),
            Self::Sql(error) => write!(f, "SQL error: {error}"),
            Self::PoolClosed => write!(f, "session pool is closed"),
            Self::InvalidResource(error) => write!(f, "invalid session resource: {error}"),
            Self::UnsupportedPool(error) => write!(f, "unsupported session pool: {error}"),
        }
    }
}

impl std::error::Error for SessionError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 事务模式：乐观（冲突时重试）或悲观（先加锁）。
pub enum TransactionMode {
    /// 乐观事务。
    Optimistic,
    /// 悲观事务。
    Pessimistic,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 事务快照信息。
pub struct Transaction {
    /// 事务开始时间戳（start_ts，MVCC 版本）。
    pub start_ts: u64,
    /// 事务是否仍有效。
    pub valid: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 内部 SQL 参数/结果的简化值类型。
pub enum SqlValue {
    /// SQL NULL。
    Null,
    /// 有符号整数。
    Integer(i64),
    /// 无符号整数。
    Unsigned(u64),
    /// 字符串。
    String(String),
    /// 字节串。
    Bytes(Vec<u8>),
    /// 布尔值。
    Bool(bool),
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一行查询结果。
pub struct Row {
    /// 按列顺序的单元格值。
    pub values: Vec<SqlValue>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 请求来源标记，用于可观测性与权限路径区分。
pub enum RequestSource {
    #[default]
    /// 未指定。
    Unspecified,
    /// 来自 DDL 子系统。
    Ddl,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单次执行的上下文。
pub struct ExecutionContext {
    /// 请求来源。
    pub request_source: RequestSource,
}

/// 查询结果集：分批拉取行并关闭。
pub trait RecordSet: Send {
    /// 最多拉取 `batch_size` 行。
    fn drain(&mut self, batch_size: usize) -> Result<Vec<Row>, SessionError>;
    /// 关闭结果集，释放底层资源。
    fn close(&mut self) -> Result<(), SessionError>;
}

/// 观察 SQL 执行耗时的钩子（用于指标/测试）。
pub trait DurationObserver: Send + Sync {
    /// 记录一次执行的标签、耗时与成败。
    fn observe(&self, label: &str, elapsed: Duration, success: bool);
}

#[derive(Default)]
/// 空实现的耗时观察者。
pub struct NoopDurationObserver;

impl DurationObserver for NoopDurationObserver {
    fn observe(&self, _label: &str, _elapsed: Duration, _success: bool) {}
}

/// 会话变量的线程安全快照（DDL 内部会话常用子集）。
pub struct SessionVariables {
    /// 是否处于事务中。
    in_transaction: AtomicBool,
    /// 是否自动提交。
    autocommit: AtomicBool,
    /// 是否仅允许受限 SQL。
    restricted_sql: AtomicBool,
    /// 磁盘接近满时是否仍允许写入。
    disk_full_allowed_on_almost_full: AtomicBool,
    /// 会话时区/位置名。
    location: Mutex<String>,
    /// 语句级时区。
    statement_time_zone: Mutex<String>,
}

impl Default for SessionVariables {
    fn default() -> Self {
        Self {
            in_transaction: AtomicBool::new(false),
            autocommit: AtomicBool::new(false),
            restricted_sql: AtomicBool::new(false),
            disk_full_allowed_on_almost_full: AtomicBool::new(false),
            location: Mutex::new("UTC".to_owned()),
            statement_time_zone: Mutex::new("UTC".to_owned()),
        }
    }
}

impl SessionVariables {
    /// 设置是否处于事务中。
    pub fn set_in_transaction(&self, value: bool) {
        self.in_transaction.store(value, Ordering::Release);
    }

    /// 查询是否处于事务中。
    pub fn in_transaction(&self) -> bool {
        self.in_transaction.load(Ordering::Acquire)
    }

    /// 设置自动提交。
    pub fn set_autocommit(&self, value: bool) {
        self.autocommit.store(value, Ordering::Release);
    }

    /// 查询自动提交。
    pub fn autocommit(&self) -> bool {
        self.autocommit.load(Ordering::Acquire)
    }

    /// 设置受限 SQL 模式。
    pub fn set_restricted_sql(&self, value: bool) {
        self.restricted_sql.store(value, Ordering::Release);
    }

    /// 查询是否受限 SQL。
    pub fn restricted_sql(&self) -> bool {
        self.restricted_sql.load(Ordering::Acquire)
    }

    /// 允许在磁盘近满时继续写。
    pub fn set_disk_full_allowed_on_almost_full(&self) {
        self.disk_full_allowed_on_almost_full
            .store(true, Ordering::Release);
    }

    /// 清除磁盘近满写入选项。
    pub fn clear_disk_full_option(&self) {
        self.disk_full_allowed_on_almost_full
            .store(false, Ordering::Release);
    }

    /// 查询磁盘近满写入选项。
    pub fn disk_full_allowed_on_almost_full(&self) -> bool {
        self.disk_full_allowed_on_almost_full
            .load(Ordering::Acquire)
    }

    /// 设置会话位置/时区名。
    pub fn set_location(&self, location: impl Into<String>) {
        *self.location.lock().unwrap() = location.into();
    }

    /// 读取会话位置。
    pub fn location(&self) -> String {
        self.location.lock().unwrap().clone()
    }

    /// 用会话位置同步语句时区。
    pub fn set_statement_time_zone_from_location(&self) {
        *self.statement_time_zone.lock().unwrap() = self.location();
    }

    /// 读取语句时区。
    pub fn statement_time_zone(&self) -> String {
        self.statement_time_zone.lock().unwrap().clone()
    }
}

/// 会话上下文：DDL 内部执行 SQL/事务所需的最小能力集。
pub trait SessionContext: Send + Sync {
    /// 会话唯一 ID。
    fn session_id(&self) -> u64;
    /// 会话变量。
    fn session_variables(&self) -> Arc<SessionVariables>;
    /// 开启新事务。
    fn enter_new_transaction(&self, mode: TransactionMode) -> Result<(), SessionError>;
    /// 语句级提交（statement commit）。
    fn statement_commit(&self, context: &ExecutionContext);
    /// 提交事务。
    fn commit_transaction(&self, context: &ExecutionContext) -> Result<(), SessionError>;
    /// 获取当前事务；`activate` 为 true 时必要时激活。
    fn transaction(&self, activate: bool) -> Result<Option<Transaction>, SessionError>;
    /// 语句级回滚；悲观重试路径可区分处理。
    fn statement_rollback(&self, context: &ExecutionContext, pessimistic_retry: bool);
    /// 回滚整个事务。
    fn rollback_transaction(&self, context: &ExecutionContext);
    /// 在内部会话上执行 SQL，返回可选结果集。
    fn execute_internal(
        &self,
        context: &ExecutionContext,
        query: &str,
        arguments: &[SqlValue],
    ) -> Result<Option<Box<dyn RecordSet>>, SessionError>;
    /// 关闭会话。
    fn close(&self);
}

/// 高层会话包装：在 `SessionContext` 上提供 begin/commit/execute 等便利方法。
pub struct Session {
    /// 底层会话上下文。
    context: Arc<dyn SessionContext>,
    /// 执行耗时观察者。
    observer: Arc<dyn DurationObserver>,
}

impl Session {
    /// 用默认空观察者包装上下文。
    pub fn new(context: Arc<dyn SessionContext>) -> Self {
        Self {
            context,
            observer: Arc::new(NoopDurationObserver),
        }
    }

    /// 替换耗时观察者（建造者模式）。
    pub fn with_observer(mut self, observer: Arc<dyn DurationObserver>) -> Self {
        self.observer = observer;
        self
    }

    /// 开启乐观事务。
    pub fn begin(&self, _context: &ExecutionContext) -> Result<(), SessionError> {
        self.context
            .enter_new_transaction(TransactionMode::Optimistic)?;
        self.context.session_variables().set_in_transaction(true);
        Ok(())
    }

    /// 开启悲观事务。
    pub fn begin_pessimistic(&self, _context: &ExecutionContext) -> Result<(), SessionError> {
        self.context
            .enter_new_transaction(TransactionMode::Pessimistic)?;
        self.context.session_variables().set_in_transaction(true);
        Ok(())
    }

    /// 提交当前事务（先 statement commit 再事务提交）。
    pub fn commit(&self, context: &ExecutionContext) -> Result<(), SessionError> {
        self.context.statement_commit(context);
        self.context.commit_transaction(context)
    }

    /// 获取并必要时激活当前事务。
    pub fn transaction(&self) -> Result<Option<Transaction>, SessionError> {
        self.context.transaction(true)
    }

    /// 回滚当前语句与事务。
    pub fn rollback(&self) {
        let context = ExecutionContext::default();
        self.context.statement_rollback(&context, false);
        self.context.rollback_transaction(&context);
    }

    /// 仅做语句级回滚，用于重置会话中间态。
    pub fn reset(&self) {
        self.context
            .statement_rollback(&ExecutionContext::default(), false);
    }

    /// 执行 SQL：默认标记请求来源为 DDL，拉取结果并上报耗时。
    pub fn execute(
        &self,
        context: &ExecutionContext,
        query: &str,
        label: &str,
        arguments: &[SqlValue],
    ) -> Result<Option<Vec<Row>>, SessionError> {
        let started = Instant::now();
        let mut execution_context = context.clone();
        // 未指定来源时记为 DDL，便于链路追踪。
        if execution_context.request_source == RequestSource::Unspecified {
            execution_context.request_source = RequestSource::Ddl;
        }
        let result = (|| {
            let Some(mut record_set) =
                self.context
                    .execute_internal(&execution_context, query, arguments)?
            else {
                return Ok(None);
            };
            // 小批量排空结果集后关闭，任一错误优先返回 drain 错误。
            let rows = record_set.drain(8);
            let close = record_set.close();
            match (rows, close) {
                (Ok(rows), _) => Ok(Some(rows)),
                (Err(error), _) => Err(error),
            }
        })();
        self.observer
            .observe(label, started.elapsed(), result.is_ok());
        result
    }

    /// 返回底层上下文的克隆。
    pub fn context(&self) -> Arc<dyn SessionContext> {
        Arc::clone(&self.context)
    }

    /// 在单个事务中运行回调：失败回滚，成功提交。
    pub fn run_in_transaction<F>(&self, callback: F) -> Result<(), SessionError>
    where
        F: FnOnce(&Session) -> Result<(), SessionError>,
    {
        self.begin(&ExecutionContext::default())?;
        run_begin_transaction_failpoint();
        if let Err(error) = callback(self) {
            self.rollback();
            return Err(error);
        }
        self.commit(&ExecutionContext::default())
    }
}

/// 测试用：与 begin 事务 failpoint 握手的一次性标志。
pub static MOCK_DDL_ONCE: AtomicI64 = AtomicI64::new(0);
/// 测试用：begin 事务失败注入模式（0 关闭，1/2 为两端会合）。
static NOTIFY_BEGIN_TRANSACTION_MODE: AtomicI32 = AtomicI32::new(0);

#[derive(Default)]
/// begin 事务 failpoint 的会合状态。
struct RendezvousState {
    /// 是否有一端在等待。
    pending: bool,
}

/// begin 事务两端线程的条件变量会合点。
static BEGIN_TRANSACTION_RENDEZVOUS: (Mutex<RendezvousState>, Condvar) = (
    Mutex::new(RendezvousState { pending: false }),
    Condvar::new(),
);

/// 设置 begin 事务 failpoint 模式。
pub fn set_notify_begin_transaction_mode(mode: i32) {
    NOTIFY_BEGIN_TRANSACTION_MODE.store(mode, Ordering::Release);
}

/// 按模式执行 begin 事务失败注入/线程会合。
fn run_begin_transaction_failpoint() {
    match NOTIFY_BEGIN_TRANSACTION_MODE.load(Ordering::Acquire) {
        // 模式 1：置标志并等待对端清除 pending。
        1 => {
            MOCK_DDL_ONCE.store(1, Ordering::Release);
            let (lock, ready) = &BEGIN_TRANSACTION_RENDEZVOUS;
            let mut state = lock.lock().unwrap();
            state.pending = true;
            ready.notify_all();
            while state.pending {
                state = ready.wait(state).unwrap();
            }
        }
        // 模式 2：等待模式 1 就绪后唤醒对方并复位标志。
        2 if MOCK_DDL_ONCE.load(Ordering::Acquire) == 1 => {
            let (lock, ready) = &BEGIN_TRANSACTION_RENDEZVOUS;
            let mut state = lock.lock().unwrap();
            while !state.pending {
                state = ready.wait(state).unwrap();
            }
            state.pending = false;
            MOCK_DDL_ONCE.store(0, Ordering::Release);
            ready.notify_all();
        }
        _ => {}
    }
}

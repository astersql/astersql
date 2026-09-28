// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 系统会话包装层：所有权（Owner）、内部会话与线程不安全操作守卫。
//
// `Session` 代理到底层 `SessionContext` 执行 SQL；池与会话之间通过
// `transfer_owner` 转移独占所有权，保证同一时刻只有一方可操作。

use std::any::Any;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// 查询结果集的类型擦除句柄。
pub type RecordSet = Box<dyn Any + Send>;
/// 已解析语句节点的类型擦除句柄。
pub type Statement = Box<dyn Any + Send>;
/// SQL 绑定参数的类型擦除句柄。
pub type SqlValue = Box<dyn Any + Send + Sync>;
/// 结果行的类型擦除句柄。
pub type Row = Box<dyn Any + Send>;

/// 系统会话错误，仅携带消息字符串（对齐 Go error.Error）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionError(String);

impl SessionError {
    /// 由消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SessionError {}

/// 系统会话操作的 Result 别名。
pub type Result<T> = std::result::Result<T, SessionError>;

/// The concrete database session contract used by the system-session pool.
/// Implementations own parsing and SQL execution; the wrapper only enforces
/// exclusive ownership and balanced enter/leave operations.
/// 池使用的具体会话契约：解析与执行由实现方完成，包装层只管所有权与进出配对。
pub trait SessionContext: Send {
    /// 关闭底层会话资源。
    fn close(&mut self);
    /// 成为 Owner 时的回调（如注册内部会话）。
    fn on_became_owner(&mut self) -> Result<()>;
    /// 交出 Owner 时的回调。
    fn on_resign_owner(&mut self) -> Result<()>;
    /// 是否仍有未决事务（含 prepared TS future）。
    fn has_pending_transaction(&self) -> bool;
    /// 回滚当前事务。
    fn rollback_transaction(&mut self) -> Result<()>;
    /// 重置会话状态以便复用。
    fn reset_state(&mut self) -> Result<()>;
    /// 向会话管理器注册内部会话；返回是否注册成功。
    fn register_internal_session(&mut self) -> bool;
    /// 注销内部会话登记。
    fn unregister_internal_session(&mut self);
    /// 执行文本 SQL，返回多个结果集。
    fn execute(&mut self, sql: &str) -> Result<Vec<RecordSet>>;
    /// 执行内部 SQL（可带绑定参数）。
    fn execute_internal(&mut self, sql: &str, args: &[SqlValue]) -> Result<RecordSet>;
    /// 执行已解析语句。
    fn execute_statement(&mut self, statement: &dyn Any) -> Result<RecordSet>;
    /// 带参数解析 SQL。
    fn parse_with_params(&mut self, sql: &str, args: &[SqlValue]) -> Result<Statement>;
    /// 受限语句执行，返回行集。
    fn exec_restricted_statement(&mut self, statement: &dyn Any) -> Result<Vec<Row>>;
    /// 受限 SQL 执行，返回行集。
    fn exec_restricted_sql(&mut self, sql: &str, args: &[SqlValue]) -> Result<Vec<Row>>;
}

/// 共享的可变 SessionContext。
pub type SharedSessionContext = Arc<Mutex<Box<dyn SessionContext>>>;

/// 内部会话当前所有者：池、外层 Session，或已关闭。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Owner {
    /// 高级会话池持有。
    Pool(u64),
    /// 外层 Session 包装持有。
    Session(u64),
    /// 已关闭，不可再转移。
    Closed,
}

/// 池与 Session 共享的内部会话状态。
pub(crate) struct InternalSession {
    /// 底层上下文。
    pub(crate) context: SharedSessionContext,
    /// 当前所有者。
    pub(crate) owner: Owner,
    /// 所有权转移序号（调试/诊断）。
    sequence: u64,
    /// 正在进行的 with_context 嵌套计数。
    pub(crate) in_use: u64,
    /// 线程不安全操作并发检测计数。
    pub(crate) unsafe_count: u64,
    /// 标记为不可再入池。
    pub(crate) avoid_reuse: bool,
}

/// 共享内部会话句柄。
pub(crate) type SharedInternalSession = Arc<Mutex<InternalSession>>;

fn close_context(context: &SharedSessionContext, resign_owner: bool) {
    let mut context = context.lock().expect("session context lock poisoned");
    let resign_panic = resign_owner
        .then(|| catch_unwind(AssertUnwindSafe(|| context.on_resign_owner())))
        .and_then(|result| result.err());
    let unregister_panic =
        catch_unwind(AssertUnwindSafe(|| context.unregister_internal_session())).err();
    let close_panic = catch_unwind(AssertUnwindSafe(|| context.close())).err();

    if let Some(panic) = close_panic.or(unregister_panic).or(resign_panic) {
        resume_unwind(panic);
    }
}

/// 创建内部会话并触发 on_became_owner。
pub(crate) fn new_internal_session(
    context: Box<dyn SessionContext>,
    owner: Owner,
) -> Result<SharedInternalSession> {
    new_internal_session_impl(context, owner, false)
}

/// 池创建内部会话；初始化失败时由池负责关闭已经创建的上下文。
pub(crate) fn new_internal_session_for_pool(
    context: Box<dyn SessionContext>,
    owner: Owner,
) -> Result<SharedInternalSession> {
    new_internal_session_impl(context, owner, true)
}

fn new_internal_session_impl(
    context: Box<dyn SessionContext>,
    owner: Owner,
    close_on_failure: bool,
) -> Result<SharedInternalSession> {
    let context = Arc::new(Mutex::new(context));
    let became_owner = if matches!(owner, Owner::Session(_)) {
        let mut guard = context.lock().expect("session context lock poisoned");
        Some(catch_unwind(AssertUnwindSafe(|| guard.on_became_owner())))
    } else {
        None
    };
    match became_owner {
        None | Some(Ok(Ok(()))) => {}
        Some(Ok(Err(error))) => {
            if close_on_failure {
                context
                    .lock()
                    .expect("session context lock poisoned")
                    .close();
            }
            return Err(error);
        }
        Some(Err(panic)) => {
            if close_on_failure {
                context
                    .lock()
                    .expect("session context lock poisoned")
                    .close();
            }
            resume_unwind(panic);
        }
    }
    Ok(Arc::new(Mutex::new(InternalSession {
        context,
        owner,
        sequence: 0,
        in_use: 0,
        unsafe_count: 0,
        avoid_reuse: false,
    })))
}

/// 在 from→to 之间转移 Owner；要求当前 owner 匹配且 in_use 为 0。
pub(crate) fn transfer_owner(
    internal: &SharedInternalSession,
    from: Owner,
    to: Owner,
) -> Result<()> {
    let mut state = internal.lock().expect("internal session lock poisoned");
    state.sequence += 1;
    if state.owner == Owner::Closed {
        return Err(SessionError::new("TransferOwner error: session is closed"));
    }
    if state.owner != from {
        return Err(SessionError::new(format!(
            "TransferOwner error: expected {from:?}, found {:?}",
            state.owner
        )));
    }
    if to == Owner::Closed {
        return Err(SessionError::new(
            "TransferOwner error: cannot transfer to a closed owner",
        ));
    }
    if from == to {
        return Ok(());
    }
    if state.in_use != 0 {
        return Err(SessionError::new(format!(
            "TransferOwner error: session is still inUse: {}",
            state.in_use
        )));
    }
    let shared_context = Arc::clone(&state.context);
    let mut context = shared_context
        .lock()
        .expect("session context lock poisoned");
    // 先 resign 再 became；任一步失败则关闭并标记 Closed。
    let transition = catch_unwind(AssertUnwindSafe(|| {
        if matches!(from, Owner::Session(_)) {
            context.on_resign_owner()?;
        }
        if matches!(to, Owner::Session(_)) {
            context.on_became_owner()?;
        }
        Ok(())
    }));
    match transition {
        Ok(Ok(())) => {
            state.owner = to;
            Ok(())
        }
        Ok(Err(error)) => {
            state.owner = Owner::Closed;
            drop(context);
            drop(state);
            close_context_only(internal);
            Err(error)
        }
        Err(panic) => {
            state.owner = Owner::Closed;
            drop(context);
            drop(state);
            close_context_only(internal);
            resume_unwind(panic);
        }
    }
}

fn close_context_only(internal: &SharedInternalSession) {
    let context = {
        let state = internal.lock().expect("internal session lock poisoned");
        Arc::clone(&state.context)
    };
    close_context(&context, false);
}

/// 由 caller（若指定）关闭内部会话；in_use 为 0 时注销并 close 上下文。
pub(crate) fn close_internal(internal: &SharedInternalSession, caller: Option<Owner>) {
    let mut state = internal.lock().expect("internal session lock poisoned");
    // 已关闭，或调用方不是当前 owner，则忽略。
    if state.owner == Owner::Closed || caller.is_some_and(|owner| state.owner != owner) {
        return;
    }
    let resign_owner = matches!(state.owner, Owner::Session(_));
    let context = (state.in_use == 0).then(|| Arc::clone(&state.context));
    state.owner = Owner::Closed;
    drop(state);
    if let Some(context) = context {
        close_context(&context, resign_owner);
    }
}

/// 全局递增的 Session ID。
static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

/// 对外暴露的系统会话句柄，持有可选的内部会话。
pub struct Session {
    /// 本包装的唯一 ID（用于 Owner::Session）。
    id: u64,
    /// 底层内部会话；None 表示空壳/已剥离。
    pub(crate) internal: Option<SharedInternalSession>,
}

impl Session {
    /// 构造无内部会话的空 Session。
    pub(crate) fn empty() -> Self {
        Self {
            id: NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed),
            internal: None,
        }
    }

    /// 本 Session 对应的 Owner 值。
    pub(crate) fn owner(&self) -> Owner {
        Owner::Session(self.id)
    }

    /// 包装已有内部会话。
    pub(crate) fn from_internal(internal: SharedInternalSession) -> Self {
        let mut session = Self::empty();
        session.internal = Some(internal);
        session
    }

    /// 在持有所有权的前提下进入上下文执行操作；
    /// `thread_safe` 为假时用 unsafe_count 检测并发重入。
    fn with_context<T>(
        &self,
        thread_safe: bool,
        operation: impl FnOnce(&mut dyn SessionContext) -> Result<T>,
    ) -> Result<T> {
        let internal = self
            .internal
            .as_ref()
            .ok_or_else(|| SessionError::new("session is closed"))?;
        let mut state = internal.lock().expect("internal session lock poisoned");
        if state.owner == Owner::Closed {
            return Err(SessionError::new("session is closed"));
        }
        if state.owner != self.owner() {
            return Err(SessionError::new("session is not owned by the caller"));
        }
        if !thread_safe {
            state.unsafe_count += 1;
            if state.unsafe_count > 1 {
                return Err(SessionError::new(
                    "EnterOperation error: race detected for concurrent thread-unsafe operations",
                ));
            }
        }
        state.in_use += 1;
        let context = Arc::clone(&state.context);
        // 释放内部锁后再调上下文，避免死锁。
        drop(state);

        let operation_result = {
            let mut context = context.lock().expect("session context lock poisoned");
            catch_unwind(AssertUnwindSafe(|| operation(&mut **context)))
        };

        let mut state = internal.lock().expect("internal session lock poisoned");
        state.in_use -= 1;
        if !thread_safe {
            state.unsafe_count = 0;
        }
        if operation_result.is_err() {
            state.avoid_reuse = true;
        }
        let close_context_after_exit =
            (state.owner == Owner::Closed && state.in_use == 0).then(|| Arc::clone(&state.context));
        drop(state);

        let close_panic = close_context_after_exit.and_then(|context| {
            catch_unwind(AssertUnwindSafe(|| close_context(&context, true))).err()
        });
        match operation_result {
            Ok(result) => {
                if let Some(panic) = close_panic {
                    resume_unwind(panic);
                }
                result
            }
            Err(panic) => resume_unwind(panic),
        }
    }

    /// 关闭本会话持有的内部会话。
    pub fn Close(&self) {
        if let Some(internal) = &self.internal {
            close_internal(internal, Some(self.owner()));
        }
    }

    /// 当前是否仍为本 Session 的 Owner。
    pub fn IsOwner(&self) -> bool {
        self.internal.as_ref().is_some_and(|internal| {
            internal
                .lock()
                .expect("internal session lock poisoned")
                .owner
                == self.owner()
        })
    }

    /// 标记内部会话不可再入池。
    pub fn AvoidReuse(&self) {
        if let Some(internal) = &self.internal {
            let mut state = internal.lock().expect("internal session lock poisoned");
            if state.owner == self.owner() {
                state.avoid_reuse = true;
            }
        }
    }

    /// 以线程不安全模式访问底层 SessionContext。
    pub fn WithSessionContext<T>(
        &self,
        operation: impl FnOnce(&mut dyn SessionContext) -> Result<T>,
    ) -> Result<T> {
        self.with_context(false, operation)
    }

    /// 代理执行文本 SQL。
    pub fn Execute(&self, sql: &str) -> Result<Vec<RecordSet>> {
        self.with_context(false, |context| context.execute(sql))
    }

    /// 代理执行内部 SQL。
    pub fn ExecuteInternal(&self, sql: &str, args: &[SqlValue]) -> Result<RecordSet> {
        self.with_context(false, |context| context.execute_internal(sql, args))
    }

    /// 代理执行已解析语句。
    pub fn ExecuteStmt(&self, statement: &dyn Any) -> Result<RecordSet> {
        self.with_context(false, |context| context.execute_statement(statement))
    }

    /// 代理带参解析。
    pub fn ParseWithParams(&self, sql: &str, args: &[SqlValue]) -> Result<Statement> {
        self.with_context(false, |context| context.parse_with_params(sql, args))
    }

    /// 代理受限语句执行。
    pub fn ExecRestrictedStmt(&self, statement: &dyn Any) -> Result<Vec<Row>> {
        self.with_context(false, |context| {
            context.exec_restricted_statement(statement)
        })
    }

    /// 代理受限 SQL 执行。
    pub fn ExecRestrictedSQL(&self, sql: &str, args: &[SqlValue]) -> Result<Vec<Row>> {
        self.with_context(false, |context| context.exec_restricted_sql(sql, args))
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::empty()
    }
}

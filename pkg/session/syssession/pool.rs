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

// 系统会话高级池（AdvancedSessionPool）。
//
// 通过工厂创建内部会话，Get/Put 在池与调用方之间转移所有权（Owner）；
// Put 前回滚事务并重置状态，脏会话或不复用标记则关闭而非入池。

use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::session::{
    Owner, Result, Session, SessionContext, SessionError, SharedInternalSession, close_internal,
    new_internal_session_for_pool, transfer_owner,
};

/// 池容量上限（与 Go PoolMaxSize 对齐的哨兵值）。
pub const PoolMaxSize: usize = 1024 * 1024 * 1024;

/// 创建底层 `SessionContext` 的工厂闭包类型。
pub type Factory = Arc<dyn Fn() -> Result<Box<dyn SessionContext>> + Send + Sync>;

/// 可取消操作的令牌；一旦 cancel 则保持为已取消。
#[derive(Default)]
pub struct CancellationToken(AtomicBool);

impl CancellationToken {
    /// 标记操作为已取消。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    /// 查询是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// 会话池抽象：借出、归还、以及带回调的借用辅助。
pub trait Pool {
    /// 从池中取出或新建一个会话。
    fn Get(&self) -> Result<Session>;
    /// 尝试将会话归还池中（失败或脏则关闭）。
    fn Put(&self, session: &Session);
    /// 借出会话执行回调；成功则 Put，失败则 Close。
    fn WithSession(&self, callback: &mut dyn FnMut(&Session) -> Result<()>) -> Result<()>;
    /// 强制注册以阻塞 GC 的会话借用；可被 CancellationToken 中止。
    fn WithForceBlockGCSession(
        &self,
        cancellation: &CancellationToken,
        callback: &mut dyn FnMut(&Session) -> Result<()>,
    ) -> Result<()>;
}

/// 全局递增的池 ID 生成器。
static NEXT_POOL_ID: AtomicU64 = AtomicU64::new(1);

/// 带容量限制的先进先出系统会话池。
pub struct AdvancedSessionPool {
    /// 池唯一 ID，用作 Owner::Pool。
    id: u64,
    /// 空闲队列最大长度。
    pub(crate) capacity: usize,
    /// 空闲内部会话队列。
    pub(crate) sessions: Mutex<VecDeque<SharedInternalSession>>,
    /// 会话工厂。
    factory: Factory,
    /// 池是否已关闭。
    closed: AtomicBool,
}

/// 创建高级会话池；非法容量（≤0 或超过上限）回退为 PoolMaxSize。
pub fn NewAdvancedSessionPool<F>(capacity: isize, factory: F) -> AdvancedSessionPool
where
    F: Fn() -> Result<Box<dyn SessionContext>> + Send + Sync + 'static,
{
    let capacity = if capacity <= 0 || capacity as usize > PoolMaxSize {
        PoolMaxSize
    } else {
        capacity as usize
    };
    AdvancedSessionPool {
        id: NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed),
        capacity,
        sessions: Mutex::new(VecDeque::new()),
        factory: Arc::new(factory),
        closed: AtomicBool::new(false),
    }
}

impl AdvancedSessionPool {
    /// 本池对应的 Owner 标识。
    fn owner(&self) -> Owner {
        Owner::Pool(self.id)
    }

    /// 从空闲队列弹出或通过工厂新建内部会话。
    fn get_internal(&self) -> Result<SharedInternalSession> {
        if self.closed.load(Ordering::Acquire) {
            return Err(SessionError::new("session pool closed"));
        }
        if let Some(internal) = self
            .sessions
            .lock()
            .expect("session pool lock poisoned")
            .pop_front()
        {
            return Ok(internal);
        }
        new_internal_session_for_pool((self.factory)()?, self.owner())
    }

    /// 借出会话并将所有权从池转移到 Session 包装。
    pub fn Get(&self) -> Result<Session> {
        let internal = self.get_internal()?;
        let session = Session::from_internal(Arc::clone(&internal));
        // 所有权转移失败则关闭内部会话，避免泄漏。
        if let Err(error) = transfer_owner(&internal, self.owner(), session.owner()) {
            close_internal(&internal, None);
            return Err(error);
        }
        Ok(session)
    }

    /// 归还会话：校验可复用后入队，否则关闭。
    pub fn Put(&self, session: &Session) {
        let Some(internal) = &session.internal else {
            return;
        };
        // 先把 Owner 从 Session 转回 Pool。
        if transfer_owner(internal, session.owner(), self.owner()).is_err() {
            return;
        }

        // 有 avoid_reuse、未决事务或重置失败则不可复用。
        let reusable = catch_unwind(AssertUnwindSafe(|| {
            let state = internal.lock().expect("internal session lock poisoned");
            if state.avoid_reuse {
                false
            } else {
                let mut context = state.context.lock().expect("session context lock poisoned");
                !context.has_pending_transaction()
                    && context.rollback_transaction().is_ok()
                    && context.reset_state().is_ok()
            }
        }));
        let reusable = match reusable {
            Ok(reusable) => reusable,
            Err(panic) => {
                close_internal(internal, Some(self.owner()));
                resume_unwind(panic);
            }
        };
        if !reusable {
            close_internal(internal, Some(self.owner()));
            return;
        }

        let mut sessions = self.sessions.lock().expect("session pool lock poisoned");
        // The closed check and enqueue must share the same critical section with Close's drain.
        // Otherwise Put can observe `closed == false`, lose the lock race to Close, and enqueue
        // after Close has drained the queue.
        if self.closed.load(Ordering::Acquire) {
            drop(sessions);
            close_internal(internal, Some(self.owner()));
            return;
        }
        // 已满则关闭当前会话，保持容量上限。
        if sessions.len() == self.capacity {
            drop(sessions);
            close_internal(internal, Some(self.owner()));
        } else {
            sessions.push_back(Arc::clone(internal));
        }
    }

    /// 借出会话执行一次性回调；成功 Put，失败 Close。
    pub fn WithSession<F>(&self, callback: F) -> Result<()>
    where
        F: FnOnce(&Session) -> Result<()>,
    {
        let session = self.Get()?;
        match catch_unwind(AssertUnwindSafe(|| callback(&session))) {
            Ok(Ok(())) => {
                self.Put(&session);
                Ok(())
            }
            Ok(Err(error)) => {
                session.Close();
                Err(error)
            }
            Err(panic) => {
                session.Close();
                resume_unwind(panic);
            }
        }
    }

    /// 轮询直到内部会话成功注册（阻塞 GC），再执行回调。
    pub fn WithForceBlockGCSession<F>(
        &self,
        cancellation: &CancellationToken,
        callback: F,
    ) -> Result<()>
    where
        F: FnOnce(&Session) -> Result<()>,
    {
        let session = self.Get()?;
        let operation = catch_unwind(AssertUnwindSafe(|| -> Result<()> {
            loop {
                if cancellation.is_cancelled() {
                    return Err(SessionError::new("operation cancelled"));
                }
                let registered = session
                    .WithSessionContext(|context| Ok(context.register_internal_session()))?;
                if registered {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            callback(&session)
        }));
        match operation {
            Ok(Ok(())) => {
                self.Put(&session);
                Ok(())
            }
            Ok(Err(error)) => {
                session.Close();
                Err(error)
            }
            Err(panic) => {
                session.Close();
                resume_unwind(panic);
            }
        }
    }

    /// 关闭池并关闭所有空闲会话；幂等。
    pub fn Close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let sessions = self
            .sessions
            .lock()
            .expect("session pool lock poisoned")
            .drain(..)
            .collect::<Vec<_>>();
        for session in sessions {
            close_internal(&session, Some(self.owner()));
        }
    }

    /// 池是否已关闭。
    pub fn IsClosed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

impl Pool for AdvancedSessionPool {
    fn Get(&self) -> Result<Session> {
        AdvancedSessionPool::Get(self)
    }

    fn Put(&self, session: &Session) {
        AdvancedSessionPool::Put(self, session)
    }

    fn WithSession(&self, callback: &mut dyn FnMut(&Session) -> Result<()>) -> Result<()> {
        AdvancedSessionPool::WithSession(self, callback)
    }

    fn WithForceBlockGCSession(
        &self,
        cancellation: &CancellationToken,
        callback: &mut dyn FnMut(&Session) -> Result<()>,
    ) -> Result<()> {
        AdvancedSessionPool::WithForceBlockGCSession(self, cancellation, callback)
    }
}

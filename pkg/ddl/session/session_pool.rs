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

// DDL 内部会话池。
//
// 在底层 `ResourcePool` 之上封装借出（get）/归还（put）/销毁（destroy）逻辑，
// 并维护当前在用的内部会话 ID 集合，便于排查与优雅关闭。
// 借出时会把会话配置为自动提交、受限 SQL 等适合后台 DDL 的安全默认值。

use crate::session::{ExecutionContext, SessionContext, SessionError};
use std::collections::HashSet;
use std::sync::{Arc, LazyLock, Mutex, RwLock};

/// 资源池返回的资源形态：会话或其他类型（报错用）。
pub enum Resource {
    /// 可用的会话上下文。
    Session(Arc<dyn SessionContext>),
    /// 非会话资源，携带类型名以便诊断。
    Other(String),
}

/// 底层资源池的种类，决定 `destroy` 时的回收策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourcePoolKind {
    /// 支持显式销毁会话。
    Destroyable,
    /// 基于槽位计数：销毁时关闭会话并归还空槽。
    SlotCounting,
    /// 其他未分类实现。
    Other,
}

/// 底层资源池抽象：负责真正的获取、归还、关闭。
pub trait ResourcePool: Send + Sync {
    /// 从池中取出一个资源。
    fn get(&self) -> Result<Resource, SessionError>;
    /// 归还会话；`None` 表示仅归还槽位（用于 SlotCounting 销毁路径）。
    fn put(&self, resource: Option<Arc<dyn SessionContext>>);
    /// 销毁会话（仅 Destroyable 池需要实现）。
    fn destroy(&self, _resource: Arc<dyn SessionContext>) {}
    /// 关闭池，拒绝后续借出。
    fn close(&self);
    /// 返回池种类，默认 `Other`。
    fn kind(&self) -> ResourcePoolKind {
        ResourcePoolKind::Other
    }
    /// 返回实现类型名，用于错误信息。
    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// 当前已借出、尚未归还的内部会话 ID 集合。
static INTERNAL_SESSIONS: LazyLock<RwLock<HashSet<u64>>> =
    LazyLock::new(|| RwLock::new(HashSet::new()));

/// 列出当前在用的内部会话 ID。
pub fn internal_session_ids() -> Vec<u64> {
    INTERNAL_SESSIONS.read().unwrap().iter().copied().collect()
}

/// 将会话登记为在用内部会话。
fn store_internal_session(context: &dyn SessionContext) {
    INTERNAL_SESSIONS
        .write()
        .unwrap()
        .insert(context.session_id());
}

/// 从在用集合中移除会话。
fn delete_internal_session(context: &dyn SessionContext) {
    INTERNAL_SESSIONS
        .write()
        .unwrap()
        .remove(&context.session_id());
}

/// 会话池自身的关闭标记。
#[derive(Default)]
struct PoolState {
    /// 是否已关闭。
    closed: bool,
}

/// DDL 会话池：包装底层 `ResourcePool` 并施加安全归还约束。
pub struct Pool {
    /// 关闭状态。
    state: Mutex<PoolState>,
    /// 底层资源池。
    resource_pool: Arc<dyn ResourcePool>,
}

impl Pool {
    /// 用给定底层池构造会话池。
    pub fn new(resource_pool: Arc<dyn ResourcePool>) -> Self {
        Self {
            state: Mutex::new(PoolState::default()),
            resource_pool,
        }
    }

    /// 借出一个已配置好的内部会话；池已关闭则报错。
    pub fn get(&self) -> Result<Arc<dyn SessionContext>, SessionError> {
        if self.state.lock().unwrap().closed {
            return Err(SessionError::PoolClosed);
        }
        let context = match self.resource_pool.get()? {
            Resource::Session(context) => context,
            Resource::Other(type_name) => {
                return Err(SessionError::InvalidResource(format!(
                    "need SessionContext, but got {type_name}"
                )));
            }
        };
        // 后台 DDL 会话：自动提交、受限 SQL，并同步时区与磁盘满策略。
        let variables = context.session_variables();
        variables.set_autocommit(true);
        variables.set_restricted_sql(true);
        variables.set_statement_time_zone_from_location();
        variables.set_disk_full_allowed_on_almost_full();
        store_internal_session(context.as_ref());
        Ok(context)
    }

    /// 归还前校验：会话上不得仍挂着有效事务（transaction）。
    fn validate_idle(context: &dyn SessionContext) -> Result<(), SessionError> {
        if context
            .transaction(false)?
            .is_some_and(|transaction| transaction.valid)
        {
            Err(SessionError::Transaction(
                "session returned to pool with a valid transaction".to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    /// 归还会话：回滚残留事务、清理磁盘选项后放回底层池。
    pub fn put(&self, context: Arc<dyn SessionContext>) -> Result<(), SessionError> {
        Self::validate_idle(context.as_ref())?;
        context.rollback_transaction(&ExecutionContext::default());
        context.session_variables().clear_disk_full_option();
        // Return first even when the pool is closing; slot-counting pools wait
        // for every outstanding Get before Close can finish.
        // 即使池正在关闭也先归还，以便槽位计数池能等齐所有 Get 再结束 Close。
        self.resource_pool.put(Some(Arc::clone(&context)));
        delete_internal_session(context.as_ref());
        Ok(())
    }

    /// 销毁会话：按池种类选择 destroy / 关会话还槽 / 退回并报不支持。
    pub fn destroy(&self, context: Arc<dyn SessionContext>) -> Result<(), SessionError> {
        Self::validate_idle(context.as_ref())?;
        context.rollback_transaction(&ExecutionContext::default());
        context.session_variables().clear_disk_full_option();
        delete_internal_session(context.as_ref());

        match self.resource_pool.kind() {
            ResourcePoolKind::Destroyable => {
                self.resource_pool.destroy(context);
                Ok(())
            }
            ResourcePoolKind::SlotCounting => {
                context.close();
                self.resource_pool.put(None);
                Ok(())
            }
            ResourcePoolKind::Other => {
                self.resource_pool.put(Some(context));
                Err(SessionError::UnsupportedPool(
                    self.resource_pool.type_name().to_owned(),
                ))
            }
        }
    }

    /// 关闭池（幂等）：先关底层池再标记 closed。
    pub fn close(&self) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        self.resource_pool.close();
        state.closed = true;
    }

    /// 查询池是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.state.lock().unwrap().closed
    }
}

/// 便捷构造函数，等价于 `Pool::new`。
pub fn new_session_pool(resource_pool: Arc<dyn ResourcePool>) -> Pool {
    Pool::new(resource_pool)
}

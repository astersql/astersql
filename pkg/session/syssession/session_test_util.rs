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

// 系统会话（syssession）测试辅助工具。
//
// 提供测试专用的会话构造、内部 `SessionContext` 窥探/替换，以及会话池大小查询，
// 便于单元测试绕过正常获取路径直接操作内部状态。

use std::sync::Arc;

use crate::pool::AdvancedSessionPool;
use crate::session::{
    Owner, Result, Session, SessionContext, SessionError, SharedSessionContext,
    new_internal_session,
};

/// 为测试构造带内部会话的 `Session`。
///
/// 使用空壳会话挂接给定的 `SessionContext`，跳过池化获取流程。
pub fn NewSessionForTest(context: Box<dyn SessionContext>) -> Result<Session> {
    let mut session = Session::empty();
    session.internal = Some(new_internal_session(context, session.owner())?);
    Ok(session)
}

impl Session {
    /// 在测试中取出内部共享的 `SessionContext`。
    ///
    /// 校验内部会话未关闭且当前调用方仍持有所有权（Owner）后返回克隆的 `Arc`。
    pub fn InternalSctxForTest(&self) -> Result<SharedSessionContext> {
        let internal = self
            .internal
            .as_ref()
            .ok_or_else(|| SessionError::new("internal session is closed"))?;
        let state = internal.lock().expect("internal session lock poisoned");
        Ok(Arc::clone(&state.context))
    }

    /// 在测试中原地替换内部 `SessionContext`。
    ///
    /// `replace` 闭包在已加锁的上下文上执行，用于注入 mock 或重置状态。
    pub fn ResetSctxForTest(
        &self,
        replace: impl FnOnce(&mut Box<dyn SessionContext>),
    ) -> Result<()> {
        let internal = self
            .internal
            .as_ref()
            .ok_or_else(|| SessionError::new("internal session is closed"))?;
        let state = internal.lock().expect("internal session lock poisoned");
        if state.owner != self.owner() {
            return Err(SessionError::new("session is not owned by the caller"));
        }
        let context = Arc::clone(&state.context);
        replace(&mut *context.lock().expect("session context lock poisoned"));
        Ok(())
    }

    /// 判断内部会话是否已关闭（`internal` 为空或 Owner 为 Closed）。
    pub fn IsInternalClosed(&self) -> bool {
        self.internal.as_ref().is_none_or(|internal| {
            internal
                .lock()
                .expect("internal session lock poisoned")
                .owner
                == Owner::Closed
        })
    }

    /// 判断会话是否标记为避免复用（`avoid_reuse`）。
    ///
    /// 标记后归还到会话池时不应再被后续请求取出。
    pub fn IsAvoidReuse(&self) -> bool {
        self.internal.as_ref().is_some_and(|internal| {
            internal
                .lock()
                .expect("internal session lock poisoned")
                .avoid_reuse
        })
    }

    /// 返回当前正在执行的操作数。
    pub fn InuseForTest(&self) -> u64 {
        self.internal.as_ref().map_or(0, |internal| {
            internal
                .lock()
                .expect("internal session lock poisoned")
                .in_use
        })
    }

    /// 返回线程不安全操作检测计数。
    pub fn UnsafeForTest(&self) -> u64 {
        self.internal.as_ref().map_or(0, |internal| {
            internal
                .lock()
                .expect("internal session lock poisoned")
                .unsafe_count
        })
    }
}

impl AdvancedSessionPool {
    /// 返回池中当前空闲会话数量，供测试断言池化行为。
    pub fn Size(&self) -> usize {
        self.sessions
            .lock()
            .expect("session pool lock poisoned")
            .len()
    }

    /// 返回池配置的容量。
    pub fn Capacity(&self) -> usize {
        self.capacity
    }
}

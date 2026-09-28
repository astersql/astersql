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

// 会话级咨询锁（Advisory Lock）实现。
//
// 通过悲观事务向 `mysql.advisory_locks` 插入锁名行来抢占命名锁；
// 参考计数管理同一会话多次 GET_LOCK，`IsUsedLock` 用短超时探测锁是否被占用。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use crate::{SessionError, SessionResult};

/// 内部 SQL 来源类型，标记语句由内核内部发起而非用户直接下发。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InternalSourceType {
    /// 未标记来源。
    None,
    /// 其他内部事务（含咨询锁相关 SQL）。
    InternalTxnOthers,
}

/// 执行咨询锁内部 SQL 时携带的上下文。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdvisoryContext {
    /// 内部 SQL 来源标记。
    pub source: InternalSourceType,
}

/// 咨询锁所需的会话能力：执行内部 SQL 与关闭时记录错误。
pub trait AdvisorySession: Send {
    /// 执行无绑定参数的内部 SQL。
    fn ExecuteInternal(&mut self, context: &AdvisoryContext, sql: &str) -> SessionResult;
    /// 执行带整型绑定参数的内部 SQL。
    fn ExecuteInternalWithInt(
        &mut self,
        context: &AdvisoryContext,
        sql: &str,
        value: i64,
    ) -> SessionResult;
    /// 执行带字符串绑定参数的内部 SQL。
    fn ExecuteInternalWithString(
        &mut self,
        context: &AdvisoryContext,
        sql: &str,
        value: &str,
    ) -> SessionResult;
    /// 关闭锁时若 ROLLBACK 失败则记录日志。
    fn LogCloseError(&self, error: &SessionError);
}

/// 单把命名咨询锁的状态：上下文、会话、清理回调、引用计数与所有者。
pub struct advisoryLock {
    /// 内部执行上下文。
    pub ctx: AdvisoryContext,
    /// 用于执行锁相关 SQL 的会话。
    pub session: Box<dyn AdvisorySession>,
    /// 锁释放时的可选清理回调。
    pub clean: Option<Box<dyn FnOnce() + Send>>,
    /// 同一会话对该锁的引用次数。
    pub reference_count: i32,
    /// 锁所有者会话标识。
    pub owner: u64,
}

impl advisoryLock {
    /// 增加引用计数（对应多次 GET_LOCK 同一名称）。
    pub fn IncrReferences(&mut self) {
        self.reference_count += 1;
    }

    /// 减少引用计数。
    pub fn DecrReferences(&mut self) {
        self.reference_count -= 1;
    }

    /// 返回当前引用计数。
    pub fn ReferenceCount(&self) -> i32 {
        self.reference_count
    }

    /// 回滚悲观事务并执行清理回调，释放锁资源。
    pub fn Close(&mut self) {
        if let Err(error) = self.session.ExecuteInternal(&self.ctx, "ROLLBACK") {
            self.session.LogCloseError(&error);
        }
        if let Some(clean) = self.clean.take() {
            clean();
        }
    }

    /// 以给定超时获取命名锁：设置锁等待、开启悲观事务并插入锁名行。
    ///
    /// 悲观事务（PESSIMISTIC）在执行阶段即加锁，插入冲突表示名称已被占用。
    pub fn GetLock(&mut self, lock_name: &str, timeout: i64) -> SessionResult {
        self.ctx.source = InternalSourceType::InternalTxnOthers;
        self.session.ExecuteInternalWithInt(
            &self.ctx,
            "SET innodb_lock_wait_timeout = %?",
            timeout,
        )?;
        self.session
            .ExecuteInternal(&self.ctx, "BEGIN PESSIMISTIC")?;
        // 插入失败说明锁已被占用，关闭事务后向上返回错误。
        if let Err(error) = self.session.ExecuteInternalWithString(
            &self.ctx,
            "INSERT INTO mysql.advisory_locks (lock_name) VALUES (%?)",
            lock_name,
        ) {
            self.Close();
            return Err(error);
        }
        self.reference_count += 1;
        Ok(())
    }

    /// 探测锁名是否已被使用：短超时尝试插入，无论成败都关闭探测事务。
    pub fn IsUsedLock(&mut self, lock_name: &str) -> SessionResult {
        self.ctx.source = InternalSourceType::InternalTxnOthers;
        let result = (|| {
            self.session
                .ExecuteInternal(&self.ctx, "SET innodb_lock_wait_timeout = 1")?;
            self.session
                .ExecuteInternal(&self.ctx, "BEGIN PESSIMISTIC")?;
            self.session.ExecuteInternalWithString(
                &self.ctx,
                "INSERT INTO mysql.advisory_locks (lock_name) VALUES (%?)",
                lock_name,
            )
        })();
        self.Close();
        result
    }
}

/// 包级封装：增加咨询锁引用计数。
pub fn IncrReferences(lock: &mut advisoryLock) {
    lock.IncrReferences();
}

/// 包级封装：减少咨询锁引用计数。
pub fn DecrReferences(lock: &mut advisoryLock) {
    lock.DecrReferences();
}

/// 包级封装：读取咨询锁引用计数。
pub fn ReferenceCount(lock: &advisoryLock) -> i32 {
    lock.ReferenceCount()
}

/// 包级封装：关闭并释放咨询锁。
pub fn Close(lock: &mut advisoryLock) {
    lock.Close();
}

/// 包级封装：获取命名咨询锁。
pub fn GetLock(lock: &mut advisoryLock, name: &str, timeout: i64) -> SessionResult {
    lock.GetLock(name, timeout)
}

/// 包级封装：探测命名锁是否已被占用。
pub fn IsUsedLock(lock: &mut advisoryLock, name: &str) -> SessionResult {
    lock.IsUsedLock(name)
}

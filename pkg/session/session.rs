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

// 会话核心类型与提交/表锁等生命周期辅助逻辑。
//
// 定义语句历史、表锁、会话变量摘要、事务接口以及 `session` 结构体上的
// 提交、回滚、表锁管理与缓存表租约续期等行为。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use crate::{SessionError, SessionResult};

/// 一条已执行 SQL 语句的文本记录。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Statement {
    pub sql: String,
}

/// 单条语句执行后的上下文摘要（插入 ID、影响行数、消息等）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatementContext {
    pub LastInsertID: u64,
    pub InsertID: u64,
    pub Message: String,
    pub AffectedRows: u64,
}

/// 语句及其执行上下文的成对记录。
#[derive(Clone, Debug, Eq, PartialEq)]
struct stmtRecord {
    st: Statement,
    stmtCtx: StatementContext,
}

/// 会话内语句历史，用于诊断或重试场景回溯。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StmtHistory {
    history: Vec<stmtRecord>,
}

impl StmtHistory {
    /// 追加一条语句及其上下文。
    pub fn Add(&mut self, statement: Statement, statement_context: StatementContext) {
        self.history.push(stmtRecord {
            st: statement,
            stmtCtx: statement_context,
        });
    }

    /// 返回历史条目数量。
    pub fn Count(&self) -> usize {
        self.history.len()
    }
}

/// 表锁类型：读锁、写锁或只读。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TableLockType {
    #[default]
    None,
    Read,
    Write,
    ReadOnly,
}

/// 单张表的锁信息（表 ID + 锁类型）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableLockTpInfo {
    pub TableID: i64,
    pub Tp: TableLockType,
}

/// 重试时需要清理的预编译语句 ID 列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RetryInfo {
    pub DroppedPreparedStmtIDs: Vec<u32>,
}

impl RetryInfo {
    /// 清空待删除的预编译语句 ID。
    fn Clean(&mut self) {
        self.DroppedPreparedStmtIDs.clear();
    }
}

/// 会话变量中与连接/语句状态相关的精简视图。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionVars {
    pub StmtCtx: StatementContext,
    pub RetryInfo: RetryInfo,
    pub EnablePreparedPlanCache: bool,
    pub InRestrictedSQL: bool,
    pub RestrictedReadOnly: bool,
    pub ClientCapability: u32,
    pub ConnectionID: u64,
    pub CommandValue: u8,
    pub CompressionAlgorithm: i32,
    pub CompressionLevel: i32,
    pub StatusValue: u16,
}

/// 事务摘要：开始时间戳（StartTS）、状态、写入条目数与 SQL digest。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TxnInfo {
    pub StartTS: u64,
    pub State: String,
    pub EntriesCount: u64,
    pub CurrentSQLDigest: String,
    pub AllSQLDigests: Vec<String>,
}

/// 会话持有的事务抽象：有效性、只读判断、提交与回滚。
pub trait SessionTransaction: Send {
    fn Valid(&self) -> bool;
    fn IsReadOnly(&self) -> bool;
    fn Info(&self) -> Option<TxnInfo>;
    fn Commit(&mut self) -> SessionResult;
    fn Rollback(&mut self) -> SessionResult;
}

/// DDL Owner 管理器：判断本节点是否为 DDL 所有者。
pub trait DDLOwnerManager: Send + Sync {
    fn IsOwner(&self) -> bool;
}

/// Production boundary for commit options, cached-table lease handling, plan-cache
/// cleanup and session-owned resource shutdown. Every method is mandatory.
///
/// 生产侧运行时边界：提交选项、缓存表租约、计划缓存清理与会话资源关闭。
pub trait SessionRuntime: Send + Sync {
    /// 提交前设置事务选项。
    fn SetOptionsBeforeCommit(&self, transaction: &mut dyn SessionTransaction) -> SessionResult;
    /// 提交事务并处理临时表数据。
    fn CommitTxnWithTemporaryData(&self, transaction: &mut dyn SessionTransaction)
    -> SessionResult;
    /// 删除指定预编译语句的缓存计划。
    fn DeletePreparedPlan(&self, statement_id: u32);
    /// 关闭游标跟踪器。
    fn CloseCursorTracker(&self);
    /// 关闭会话变量相关资源。
    fn CloseSessionVars(&self);
    /// 为缓存表续租，返回各表租约时间戳。
    fn RenewCachedTableLeases(&self, table_ids: &[i64]) -> SessionResult<Vec<u64>>;
    /// 停止缓存表租约续期。
    fn StopCachedTableLeaseRenewal(&self);
}

/// 会话本体：持有运行时、事务、会话变量、表锁与 DDL Owner。
pub struct session {
    pub runtime: Arc<dyn SessionRuntime>,
    pub txn: Box<dyn SessionTransaction>,
    pub values: HashMap<String, Box<dyn Any + Send + Sync>>,
    pub currentCtx: Option<String>,
    pub processInfo: Option<String>,
    pub crossKS: bool,
    pub sessionVars: SessionVars,
    pub lockedTables: HashMap<i64, TableLockTpInfo>,
    pub ddlOwnerManager: Option<Arc<dyn DDLOwnerManager>>,
}

impl session {
    /// 返回当前跟踪上下文标识（若有）。
    pub fn GetTraceCtx(&self) -> Option<&str> {
        self.currentCtx.as_deref()
    }

    /// 批量添加表锁；`ReadOnly` 类型不写入 lockedTables。
    pub fn AddTableLock(&mut self, locks: &[TableLockTpInfo]) {
        for lock in locks {
            if lock.Tp != TableLockType::ReadOnly {
                self.lockedTables.insert(lock.TableID, lock.clone());
            }
        }
    }

    /// 按锁信息列表释放表锁。
    pub fn ReleaseTableLocks(&mut self, locks: &[TableLockTpInfo]) {
        for lock in locks {
            self.lockedTables.remove(&lock.TableID);
        }
    }

    /// 按表 ID 列表释放表锁。
    pub fn ReleaseTableLockByTableIDs(&mut self, table_ids: &[i64]) {
        for table_id in table_ids {
            self.lockedTables.remove(table_id);
        }
    }

    /// 查询指定表是否已加锁及其锁类型。
    pub fn CheckTableLocked(&self, table_id: i64) -> (bool, TableLockType) {
        self.lockedTables
            .get(&table_id)
            .map(|lock| (true, lock.Tp))
            .unwrap_or((false, TableLockType::None))
    }

    /// 返回当前全部表锁快照。
    pub fn GetAllTableLocks(&self) -> Vec<TableLockTpInfo> {
        self.lockedTables.values().cloned().collect()
    }

    /// 是否持有任意表锁。
    pub fn HasLockedTables(&self) -> bool {
        !self.lockedTables.is_empty()
    }

    /// 释放全部表锁。
    pub fn ReleaseAllTableLocks(&mut self) {
        self.lockedTables.clear();
    }

    /// 当前节点是否为 DDL Owner。
    pub fn IsDDLOwner(&self) -> bool {
        self.ddlOwnerManager
            .as_ref()
            .is_some_and(|manager| manager.IsOwner())
    }

    /// 返回会话状态标志位。
    pub fn Status(&self) -> u16 {
        self.sessionVars.StatusValue
    }

    /// 返回 LastInsertID，若为 0 则回退到 InsertID。
    pub fn LastInsertID(&self) -> u64 {
        if self.sessionVars.StmtCtx.LastInsertID > 0 {
            self.sessionVars.StmtCtx.LastInsertID
        } else {
            self.sessionVars.StmtCtx.InsertID
        }
    }

    /// 返回最近一条语句消息。
    pub fn LastMessage(&self) -> String {
        self.sessionVars.StmtCtx.Message.clone()
    }

    /// 返回影响行数。
    pub fn AffectedRows(&self) -> u64 {
        self.sessionVars.StmtCtx.AffectedRows
    }

    /// 设置客户端能力标志。
    pub fn SetClientCapability(&mut self, capability: u32) {
        self.sessionVars.ClientCapability = capability;
    }

    /// 设置连接 ID。
    pub fn SetConnectionID(&mut self, connection_id: u64) {
        self.sessionVars.ConnectionID = connection_id;
    }

    /// 设置当前 MySQL 命令值。
    pub fn SetCommandValue(&mut self, command: u8) {
        self.sessionVars.CommandValue = command;
    }

    /// 设置压缩算法。
    pub fn SetCompressionAlgorithm(&mut self, algorithm: i32) {
        self.sessionVars.CompressionAlgorithm = algorithm;
    }

    /// 设置压缩级别。
    pub fn SetCompressionLevel(&mut self, level: i32) {
        self.sessionVars.CompressionLevel = level;
    }

    // 重试结束后清理：若启用预编译计划缓存，先删除已丢弃语句的缓存计划。
    fn cleanRetryInfo(&mut self) {
        if self.sessionVars.EnablePreparedPlanCache {
            for statement_id in &self.sessionVars.RetryInfo.DroppedPreparedStmtIDs {
                self.runtime.DeletePreparedPlan(*statement_id);
            }
        }
        self.sessionVars.RetryInfo.Clean();
    }

    /// 执行提交：跳过无效/只读事务；受限只读模式下拒绝写入提交。
    pub fn doCommit(&mut self) -> SessionResult {
        if !self.txn.Valid() || self.txn.IsReadOnly() {
            return Ok(());
        }
        // Go permits internal/restricted SQL to bypass cluster read-only mode, while
        // ordinary client SQL must not commit a write transaction.
        if !self.sessionVars.InRestrictedSQL && self.sessionVars.RestrictedReadOnly {
            return Err(SessionError::new(
                "SQL is not allowed in restricted read-only mode",
            ));
        }
        self.runtime.SetOptionsBeforeCommit(self.txn.as_mut())?;
        self.commitTxnWithTemporaryData()
    }

    /// 委托运行时提交并处理临时表数据。
    fn commitTxnWithTemporaryData(&mut self) -> SessionResult {
        self.runtime.CommitTxnWithTemporaryData(self.txn.as_mut())
    }

    /// 回滚当前事务。
    pub fn RollbackTxn(&mut self) -> SessionResult {
        let result = if self.txn.Valid() {
            self.txn.Rollback()
        } else {
            Ok(())
        };
        self.cleanRetryInfo();
        result
    }

    /// 返回事务信息；StartTS 为 0 视为无有效事务。
    pub fn TxnInfo(&self) -> Option<TxnInfo> {
        self.txn.Info().filter(|info| info.StartTS != 0)
    }

    /// 关闭会话：关闭游标跟踪器与会话变量资源。
    pub fn Close(&mut self) {
        // Go logs rollback errors during close and continues releasing all owned
        // resources, so cleanup must not be short-circuited here.
        let _ = self.RollbackTxn();
        self.runtime.CloseCursorTracker();
        self.runtime.CloseSessionVars();
    }
}

/// 缓存表租约续期助手：定期续租并在提交时校验 commit_ts 是否仍小于租约。
pub struct cachedTableRenewLease {
    pub runtime: Arc<dyn SessionRuntime>,
    pub tables: Vec<i64>,
    pub lease: Vec<u64>,
    pub stopped: bool,
}

impl cachedTableRenewLease {
    /// 向运行时续租；结果数量必须与表数量一致。
    pub fn start(&mut self) -> SessionResult {
        if self.stopped {
            return Err(SessionError::new("cached-table lease renewal was stopped"));
        }
        self.lease = self.runtime.RenewCachedTableLeases(&self.tables)?;
        if self.lease.len() != self.tables.len() {
            return Err(SessionError::new(
                "cached-table lease result does not match table count",
            ));
        }
        Ok(())
    }

    /// 停止续期（幂等）。
    pub fn stop(&mut self) {
        if !self.stopped {
            self.runtime.StopCachedTableLeaseRenewal();
            self.stopped = true;
        }
    }

    /// 校验提交时间戳是否仍严格小于所有表租约（防止读到过期缓存）。
    pub fn commitTSCheck(&self, commit_ts: u64) -> bool {
        self.lease.iter().all(|lease| commit_ts < *lease)
    }
}

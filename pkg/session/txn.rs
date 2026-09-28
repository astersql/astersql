// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 会话事务（LazyTxn）：惰性初始化、语句缓冲与公平加锁。
//
// [`LazyTxn`] 包装底层 [`TransactionBackend`]，支持 pending future → valid 状态迁移、
// staging 缓冲（StmtCommit/Rollback）、悲观公平加锁（fair locking）以及
// [`TxnInfo`] 可观测字段更新。另含自动自增/随机 ID 重试 mock 与 `KeyNeedToLock` 判定。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::SystemTime;

use crate::session::{SessionTransaction, TxnInfo};
use crate::{SessionError, SessionResult};

/// KV 键字节序列。
pub type Key = Vec<u8>;
/// 语句级 staging 缓冲区句柄。
pub type StagingHandle = u64;
/// 无效 staging 句柄哨兵值。
pub const InvalidStagingHandle: StagingHandle = u64::MAX;
/// 事务内记录的 SQL digest 历史条数上限。
const MAX_TRANSACTION_STMT_HISTORY: usize = 50;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 事务运行状态机：空闲、执行中、加锁、提交、回滚。
pub enum TxnRunningState {
    #[default]
    Idle,
    Running,
    LockAcquiring,
    Committing,
    RollingBack,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 键语义标志：表键、唯一性预检、索引/临时索引等，影响是否需要加锁。
pub struct KeyFlags {
    pub table_key: bool,
    pub need_constraint_check_in_prewrite: bool,
    pub presume_key_not_exists: bool,
    pub need_locked: bool,
    pub record_key: bool,
    pub untouched_index_value: bool,
    pub index_key: bool,
    pub temp_index_key: bool,
    pub temp_index_has_handle: bool,
    pub index_value_is_unique: bool,
    pub next_gen: bool,
}

/// 底层事务后端（对应 TiKV client transaction）。
pub trait TransactionBackend: Send {
    fn String(&self) -> String;
    fn Valid(&self) -> bool;
    fn IsReadOnly(&self) -> bool;
    fn IsPipelined(&self) -> bool;
    fn StartTS(&self) -> u64;
    fn CommitTS(&self) -> u64;
    fn Size(&self) -> usize;
    fn Mem(&self) -> u64;
    fn Len(&self) -> usize;
    fn SetMemoryFootprintChangeHook(&mut self, hook: Option<fn(u64)>);
    fn MemHookSet(&self) -> bool;
    fn Stage(&mut self) -> StagingHandle;
    fn ReleaseStage(&mut self, handle: StagingHandle);
    fn CleanupStage(&mut self, handle: StagingHandle);
    fn PersistPresumeKeyNotExists(&mut self, handle: StagingHandle);
    fn KeysInStage(&self, handle: StagingHandle) -> Vec<(Key, Vec<u8>, KeyFlags)>;
    fn Commit(&mut self) -> SessionResult;
    fn Rollback(&mut self) -> SessionResult;
    fn RollbackMemDBToCheckpoint(&mut self, checkpoint: &[u8]);
    fn LockKeys(&mut self, keys: &[Key], after_lock: Option<fn()>) -> SessionResult;
    fn StartFairLocking(&mut self) -> SessionResult;
    fn RetryFairLocking(&mut self) -> SessionResult;
    fn CancelFairLocking(&mut self) -> SessionResult;
    fn DoneFairLocking(&mut self) -> SessionResult;
    fn IsInFairLockingMode(&self) -> bool;
    fn HasTablePrefix(&self, table_id: i64) -> bool;
    fn GetTableInfo(&self, table_id: i64) -> Option<String>;
    fn CacheTableInfo(&mut self, table_id: i64, info: String);
    fn GetOption(&self, option: i32) -> Option<String>;
}

/// 异步获取事务（如等待 start TS）的 Future。
pub trait TransactionFuture: Send {
    fn Wait(&mut self) -> SessionResult<Box<dyn TransactionBackend>>;
}

/// 惰性事务：可能处于 invalid / pending / valid，并跟踪语句缓冲与 TxnInfo。
pub struct LazyTxn {
    pub Transaction: Option<Box<dyn TransactionBackend>>,
    pub txnFuture: Option<Box<dyn TransactionFuture>>,
    pub initCnt: usize,
    pub stagingHandle: StagingHandle,
    pub enterFairLockingOnValid: bool,
    pub lazyUniquenessCheckEnabled: bool,
    pub lastCommitTS: u64,
    pub txnInfo: TxnInfo,
    pub state: TxnRunningState,
    pub lastStateChangeTime: SystemTime,
}

impl Default for LazyTxn {
    fn default() -> Self {
        Self {
            Transaction: None,
            txnFuture: None,
            initCnt: 0,
            stagingHandle: InvalidStagingHandle,
            enterFairLockingOnValid: false,
            lazyUniquenessCheckEnabled: false,
            lastCommitTS: 0,
            txnInfo: TxnInfo::default(),
            state: TxnRunningState::Idle,
            lastStateChangeTime: SystemTime::now(),
        }
    }
}

impl LazyTxn {
    /// 从底层事务缓存读取表信息。
    pub fn GetTableInfo(&self, table_id: i64) -> Option<String> {
        self.Transaction.as_ref()?.GetTableInfo(table_id)
    }

    /// 将表信息写入底层事务缓存。
    pub fn CacheTableInfo(&mut self, table_id: i64, info: String) {
        if let Some(transaction) = self.Transaction.as_mut() {
            transaction.CacheTableInfo(table_id, info);
        }
    }

    /// 重置 TxnInfo 与运行状态为 Idle。
    pub fn init(&mut self) {
        self.txnInfo = TxnInfo::default();
        self.state = TxnRunningState::Idle;
        self.lastStateChangeTime = SystemTime::now();
    }

    /// 若状态变化则更新并记录变更时间。
    pub fn updateState(&mut self, state: TxnRunningState) {
        if self.state != state {
            self.state = state;
            self.lastStateChangeTime = SystemTime::now();
        }
    }

    /// 语句开始时建立 staging：记录初始条目数，非 pipelined 时 Stage()。
    pub fn initStmtBuf(&mut self) {
        let Some(transaction) = self.Transaction.as_mut() else {
            return;
        };
        self.initCnt = transaction.Len();
        if !transaction.IsPipelined() {
            self.stagingHandle = transaction.Stage();
        }
    }

    /// 当前语句 staging 内新增条目数提示。
    pub fn countHint(&self) -> usize {
        if self.stagingHandle == InvalidStagingHandle {
            return 0;
        }
        self.Transaction
            .as_ref()
            .map(|transaction| transaction.Len().saturating_sub(self.initCnt))
            .unwrap_or(0)
    }

    /// 语句提交：可选持久化 presume-not-exists，ReleaseStage 并推进 initCnt。
    pub fn flushStmtBuf(&mut self) {
        if self.stagingHandle == InvalidStagingHandle {
            return;
        }
        if let Some(transaction) = self.Transaction.as_mut() {
            if self.lazyUniquenessCheckEnabled {
                transaction.PersistPresumeKeyNotExists(self.stagingHandle);
            }
            if !transaction.IsPipelined() {
                transaction.ReleaseStage(self.stagingHandle);
            }
            self.initCnt = transaction.Len();
        }
        self.stagingHandle = InvalidStagingHandle;
    }

    /// 语句回滚：CleanupStage，丢弃本语句缓冲。
    pub fn cleanupStmtBuf(&mut self) {
        if self.stagingHandle == InvalidStagingHandle {
            return;
        }
        if let Some(transaction) = self.Transaction.as_mut() {
            if !transaction.IsPipelined() {
                transaction.CleanupStage(self.stagingHandle);
            }
            self.initCnt = transaction.Len();
            self.txnInfo.EntriesCount = transaction.Len() as u64;
        }
        self.stagingHandle = InvalidStagingHandle;
    }

    /// 用 start TS、状态、条目数与 SQL digest 重建 TxnInfo。
    pub fn resetTxnInfo(
        &mut self,
        start_ts: u64,
        state: TxnRunningState,
        entries: u64,
        current_digest: String,
        all_digests: Vec<String>,
    ) {
        self.txnInfo = TxnInfo {
            StartTS: start_ts,
            State: format!("{state:?}"),
            EntriesCount: entries,
            CurrentSQLDigest: current_digest,
            AllSQLDigests: all_digests,
        };
        self.updateState(state);
    }

    /// 底层事务字节大小。
    pub fn Size(&self) -> usize {
        self.Transaction
            .as_ref()
            .map_or(0, |transaction| transaction.Size())
    }

    /// 底层事务内存占用。
    pub fn Mem(&self) -> u64 {
        self.Transaction
            .as_ref()
            .map_or(0, |transaction| transaction.Mem())
    }

    /// 设置内存占用变化回调。
    pub fn SetMemoryFootprintChangeHook(&mut self, hook: fn(u64)) {
        if let Some(transaction) = self.Transaction.as_mut() {
            transaction.SetMemoryFootprintChangeHook(Some(hook));
        }
    }

    /// 是否已设置内存钩子。
    pub fn MemHookSet(&self) -> bool {
        self.Transaction
            .as_ref()
            .is_some_and(|transaction| transaction.MemHookSet())
    }

    /// 事务已就绪且无 pending future。
    pub fn Valid(&self) -> bool {
        self.Transaction
            .as_ref()
            .is_some_and(|transaction| transaction.Valid())
            && self.txnFuture.is_none()
    }

    /// 是否在等待 future（例如 start TS）。
    pub fn pending(&self) -> bool {
        self.Transaction.is_none() && self.txnFuture.is_some()
    }

    /// valid 或 pending 均视为仍有事务语境。
    pub fn validOrPending(&self) -> bool {
        self.Valid() || self.pending()
    }

    /// 人类可读状态描述。
    pub fn String(&self) -> String {
        if let Some(transaction) = self.Transaction.as_ref() {
            transaction.String()
        } else if self.pending() {
            if self.enterFairLockingOnValid {
                "txnFuture (pending fair locking)".to_owned()
            } else {
                "txnFuture".to_owned()
            }
        } else {
            "invalid transaction".to_owned()
        }
    }

    /// GoString 风格调试输出。
    pub fn GoString(&self) -> String {
        if self.pending() {
            "Txn{state=pending}".to_owned()
        } else if let Some(transaction) = self.Transaction.as_ref() {
            format!("Txn{{state=valid, txnStartTS={}}}", transaction.StartTS())
        } else {
            "Txn{state=invalid}".to_owned()
        }
    }

    /// 读取底层事务选项。
    pub fn GetOption(&self, option: i32) -> Option<String> {
        self.Transaction
            .as_ref()
            .and_then(|transaction| transaction.GetOption(option))
    }

    /// 进入 pending：清空 Transaction，挂上 future。
    pub fn changeToPending(&mut self, future: Box<dyn TransactionFuture>) {
        self.Transaction = None;
        self.txnFuture = Some(future);
    }

    /// 等待 future 完成，安装事务，初始化语句缓冲；必要时启动公平加锁。
    pub fn changePendingToValid(&mut self) -> SessionResult {
        let mut future = self
            .txnFuture
            .take()
            .ok_or_else(|| SessionError::new("transaction future is not set"))?;
        let transaction = match future.Wait() {
            Ok(transaction) => transaction,
            Err(error) => {
                self.Transaction = None;
                return Err(error);
            }
        };
        let start_ts = transaction.StartTS();
        self.Transaction = Some(transaction);
        self.initStmtBuf();
        if self.enterFairLockingOnValid {
            self.enterFairLockingOnValid = false;
            self.Transaction
                .as_mut()
                .expect("transaction was installed")
                .StartFairLocking()?;
        }
        self.resetTxnInfo(
            start_ts,
            TxnRunningState::Idle,
            self.Transaction
                .as_ref()
                .expect("transaction was installed")
                .Len() as u64,
            self.txnInfo.CurrentSQLDigest.clone(),
            self.txnInfo.AllSQLDigests.clone(),
        );
        Ok(())
    }

    /// 清理缓冲并清空 Transaction/future，回到 invalid。
    pub fn changeToInvalid(&mut self) {
        self.cleanupStmtBuf();
        self.Transaction = None;
        self.txnFuture = None;
        self.stagingHandle = InvalidStagingHandle;
        self.enterFairLockingOnValid = false;
        self.txnInfo = TxnInfo::default();
        self.state = TxnRunningState::Idle;
    }

    /// 语句开始：更新状态为 Running，记录当前与历史 SQL digest。
    pub fn onStmtStart(&mut self, digest: String) {
        if digest.is_empty() {
            return;
        }
        self.updateState(TxnRunningState::Running);
        self.txnInfo.CurrentSQLDigest = digest.clone();
        if self.txnInfo.AllSQLDigests.len() < MAX_TRANSACTION_STMT_HISTORY {
            self.txnInfo.AllSQLDigests.push(digest);
        }
    }

    /// 语句结束：清空当前 digest，状态回到 Idle。
    pub fn onStmtEnd(&mut self) {
        self.txnInfo.CurrentSQLDigest.clear();
        self.updateState(TxnRunningState::Idle);
    }

    /// 提交事务：flush 缓冲后调用后端 Commit，成功则记录 lastCommitTS。
    pub fn Commit(&mut self) -> SessionResult {
        if !self.Valid() {
            return Err(SessionError::new("invalid transaction"));
        }
        self.updateState(TxnRunningState::Committing);
        self.flushStmtBuf();
        let result = self
            .Transaction
            .as_mut()
            .expect("valid transaction must have a backend")
            .Commit();
        if result.is_ok() {
            self.lastCommitTS = self
                .Transaction
                .as_ref()
                .expect("transaction backend exists until reset")
                .CommitTS();
        }
        self.reset();
        result
    }

    /// 回滚事务：清除内存钩子后调用后端 Rollback。
    pub fn Rollback(&mut self) -> SessionResult {
        if !self.Valid() {
            return Err(SessionError::new("invalid transaction"));
        }
        self.updateState(TxnRunningState::RollingBack);
        if let Some(transaction) = self.Transaction.as_mut() {
            transaction.SetMemoryFootprintChangeHook(Some(noopMemoryFootprintChangeHook));
        }
        let result = self
            .Transaction
            .as_mut()
            .expect("valid transaction must have a backend")
            .Rollback();
        self.reset();
        result
    }

    /// 将 MemDB 回滚到检查点，并重建语句缓冲。
    pub fn RollbackMemDBToCheckpoint(&mut self, checkpoint: &[u8]) {
        self.flushStmtBuf();
        if let Some(transaction) = self.Transaction.as_mut() {
            transaction.RollbackMemDBToCheckpoint(checkpoint);
        }
        self.cleanup();
    }

    /// 对键加锁（无 after_lock 回调）。
    pub fn LockKeys(&mut self, keys: &[Key]) -> SessionResult {
        self.LockKeysFunc(None, keys)
    }

    /// 加锁并在期间将状态置为 LockAcquiring，结束后恢复原状态。
    pub fn LockKeysFunc(&mut self, after_lock: Option<fn()>, keys: &[Key]) -> SessionResult {
        if !self.Valid() {
            return Err(SessionError::new("invalid transaction"));
        }
        let original_state = self.state;
        self.updateState(TxnRunningState::LockAcquiring);
        let result = self
            .Transaction
            .as_mut()
            .expect("valid transaction must have a backend")
            .LockKeys(keys, after_lock);
        self.updateState(original_state);
        if let Some(transaction) = self.Transaction.as_ref() {
            self.txnInfo.EntriesCount = transaction.Len() as u64;
        }
        result
    }

    /// 启动公平加锁；若仍 pending 则标记待 valid 后启动。
    pub fn StartFairLocking(&mut self) -> SessionResult {
        if self.Valid() {
            return self
                .Transaction
                .as_mut()
                .expect("valid transaction must have a backend")
                .StartFairLocking();
        }
        if !self.pending() {
            return Err(SessionError::new(
                "trying to start fair locking on a transaction in invalid state",
            ));
        }
        self.enterFairLockingOnValid = true;
        Ok(())
    }

    /// 重试公平加锁；pending 时视为空操作成功。
    pub fn RetryFairLocking(&mut self) -> SessionResult {
        if self.Valid() {
            return self
                .Transaction
                .as_mut()
                .expect("valid transaction must have a backend")
                .RetryFairLocking();
        }
        if self.pending() {
            return Ok(());
        }
        Err(SessionError::new(
            "trying to retry fair locking on a transaction in invalid state",
        ))
    }

    /// 取消公平加锁；pending 时清除待启动标记。
    pub fn CancelFairLocking(&mut self) -> SessionResult {
        if self.Valid() {
            return self
                .Transaction
                .as_mut()
                .expect("valid transaction must have a backend")
                .CancelFairLocking();
        }
        if !self.pending() {
            return Err(SessionError::new(
                "trying to cancel fair locking on a transaction in invalid state",
            ));
        }
        if !self.enterFairLockingOnValid {
            return Err(SessionError::new(
                "trying to cancel fair locking when it is not started",
            ));
        }
        self.enterFairLockingOnValid = false;
        Ok(())
    }

    /// 结束公平加锁阶段。
    pub fn DoneFairLocking(&mut self) -> SessionResult {
        if self.Valid() {
            return self
                .Transaction
                .as_mut()
                .expect("valid transaction must have a backend")
                .DoneFairLocking();
        }
        if !self.pending() {
            return Err(SessionError::new(
                "trying to finish fair locking on a transaction in invalid state",
            ));
        }
        if !self.enterFairLockingOnValid {
            return Err(SessionError::new(
                "trying to finish fair locking when it is not started",
            ));
        }
        self.enterFairLockingOnValid = false;
        Ok(())
    }

    /// 是否处于公平加锁模式（valid 问后端，pending 看标记）。
    pub fn IsInFairLockingMode(&self) -> bool {
        if self.Valid() {
            self.Transaction
                .as_ref()
                .expect("valid transaction must have a backend")
                .IsInFairLockingMode()
        } else if self.pending() {
            self.enterFairLockingOnValid
        } else {
            false
        }
    }

    /// cleanup 后变为 invalid。
    pub fn reset(&mut self) {
        self.cleanup();
        self.changeToInvalid();
    }

    /// 丢弃当前语句缓冲并重新 initStmtBuf。
    pub fn cleanup(&mut self) {
        self.cleanupStmtBuf();
        self.initStmtBuf();
    }

    /// 当前 staging 中需要加锁的键（经 KeyNeedToLock 过滤）。
    pub fn KeysNeedToLock(&self) -> Vec<Key> {
        if self.stagingHandle == InvalidStagingHandle {
            return Vec::new();
        }
        self.Transaction
            .as_ref()
            .map(|transaction| {
                transaction
                    .KeysInStage(self.stagingHandle)
                    .into_iter()
                    .filter(|(_, value, flags)| KeyNeedToLock(value, flags))
                    .map(|(key, _, _)| key)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 若 pending 则转为 valid，并设置惰性唯一性检查开关。
    pub fn Wait(&mut self, lazy_uniqueness_check_enabled: bool) -> SessionResult<&mut Self> {
        if !self.validOrPending() {
            return Err(SessionError::new("invalid transaction"));
        }
        if self.pending() {
            if let Err(error) = self.changePendingToValid() {
                self.cleanup();
                self.txnInfo.StartTS = 0;
                return Err(error);
            }
            self.lazyUniquenessCheckEnabled = lazy_uniqueness_check_enabled;
        }
        Ok(self)
    }
}

/// 将会话事务接口委托到 LazyTxn 实现。
impl SessionTransaction for LazyTxn {
    fn Valid(&self) -> bool {
        LazyTxn::Valid(self)
    }

    fn IsReadOnly(&self) -> bool {
        self.Transaction
            .as_ref()
            .is_none_or(|transaction| transaction.IsReadOnly())
    }

    fn Info(&self) -> Option<TxnInfo> {
        Some(self.txnInfo.clone())
    }

    fn Commit(&mut self) -> SessionResult {
        LazyTxn::Commit(self)
    }

    fn Rollback(&mut self) -> SessionResult {
        LazyTxn::Rollback(self)
    }
}

/// 测试：设置 mock 自动随机 ID 重试剩余失败次数。
pub fn ResetMockAutoRandIDRetryCount(fail_times: i64) {
    MOCK_AUTO_RAND_ID_RETRY_COUNT.store(fail_times, Ordering::Release);
}

/// 测试：是否注入自动自增 ID 重试一次。
static HAS_MOCK_AUTO_INC_ID_RETRY: AtomicI64 = AtomicI64::new(0);

/// 测试：启用下一次自动自增 ID 重试注入。
pub fn enableMockAutoIncIDRetry() {
    HAS_MOCK_AUTO_INC_ID_RETRY.store(1, Ordering::Release);
}

/// 测试：读取自动自增 ID 重试标记。
pub fn mockAutoIncIDRetry() -> bool {
    HAS_MOCK_AUTO_INC_ID_RETRY.load(Ordering::Acquire) == 1
}

fn noopMemoryFootprintChangeHook(_: u64) {}

/// 测试：是否仍需 mock 自动随机 ID 重试。
pub fn needMockAutoRandIDRetry() -> bool {
    MOCK_AUTO_RAND_ID_RETRY_COUNT.load(Ordering::Acquire) > 0
}

/// 测试：自动随机 ID 重试计数减一。
pub fn decreaseMockAutoRandIDRetryCount() {
    MOCK_AUTO_RAND_ID_RETRY_COUNT.fetch_sub(1, Ordering::AcqRel);
}

/// 根据键值与标志判断悲观锁是否需要锁定该键。
pub fn KeyNeedToLock(value: &[u8], flags: &KeyFlags) -> bool {
    if !flags.table_key {
        return true;
    }
    if flags.need_constraint_check_in_prewrite {
        return false;
    }
    if flags.presume_key_not_exists {
        return true;
    }
    if value.is_empty() {
        return flags.need_locked || flags.record_key;
    }
    if flags.untouched_index_value {
        return false;
    }
    if !flags.index_key {
        return true;
    }
    if flags.temp_index_key {
        return flags.next_gen || flags.temp_index_has_handle || flags.index_value_is_unique;
    }
    flags.index_value_is_unique || flags.need_locked
}

/// 总是失败的事务 Future（mock 取时间戳失败）。
pub struct txnFailFuture;

impl TransactionFuture for txnFailFuture {
    fn Wait(&mut self) -> SessionResult<Box<dyn TransactionBackend>> {
        Err(SessionError::new("mock get timestamp fail"))
    }
}

/// 带事务作用域与 pipelined 参数的事务 Future 包装。
pub struct txnFuture {
    pub future: Box<dyn TransactionFuture>,
    pub txnScope: String,
    pub pipelined: bool,
    pub pipelinedFlushConcurrency: i32,
    pub pipelinedResolveLockConcurrency: i32,
    pub pipelinedWriteThrottleRatio: f64,
}

impl TransactionFuture for txnFuture {
    fn Wait(&mut self) -> SessionResult<Box<dyn TransactionBackend>> {
        self.future.Wait()
    }
}

/// 非 pipelined 事务是否已对指定表前缀有脏写。
pub fn HasDirtyContent(transaction: &LazyTxn, table_id: i64) -> bool {
    transaction
        .Transaction
        .as_ref()
        .is_some_and(|backend| !backend.IsPipelined() && backend.HasTablePrefix(table_id))
}

/// 语句级事务钩子：提交/回滚回调与错误日志。
pub trait StatementTxnManager {
    fn OnStmtCommit(&mut self) -> SessionResult;
    fn OnStmtRollback(&mut self, pessimistic_retry: bool) -> SessionResult;
    fn LogHookError(&self, operation: &str, error: &SessionError);
}

/// 语句提交：调用 OnStmtCommit，再 flush 并 cleanup 缓冲。
pub fn StmtCommit(transaction: &mut LazyTxn, manager: &mut dyn StatementTxnManager) {
    if let Err(error) = manager.OnStmtCommit() {
        manager.LogHookError("OnStmtCommit", &error);
    }
    transaction.flushStmtBuf();
    transaction.cleanup();
}

/// 语句回滚：调用 OnStmtRollback（可标悲观重试），再 cleanup。
pub fn StmtRollback(
    transaction: &mut LazyTxn,
    manager: &mut dyn StatementTxnManager,
    pessimistic_retry: bool,
) {
    if let Err(error) = manager.OnStmtRollback(pessimistic_retry) {
        manager.LogHookError("OnStmtRollback", &error);
    }
    transaction.cleanup();
}

/// 返回空的事务选项表。
pub fn EmptyOptions() -> HashMap<i32, String> {
    HashMap::new()
}

/// 测试：自动随机 ID 重试剩余次数。
static MOCK_AUTO_RAND_ID_RETRY_COUNT: AtomicI64 = AtomicI64::new(0);

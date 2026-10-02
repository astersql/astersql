// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 将 mockStorage 适配为规范 `kv::Storage` trait 实现。
//
// 把内部 KVTxn/Transaction/Snapshot/MemManager 接到 Getter、Retriever、
// Mutator、MemBuffer、Transaction、Snapshot、Storage 等接口上，供上层以
// 标准 Storage API 驱动 mock。

use std::any::Any;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;
use std::time::Duration;

use astersql_kv as kv;

use crate::storage::{KVTxn, MemManager, OptionKey, Snapshot, Transaction, mockStorage};

/// 将任意错误信息包装为 SharedError。
fn storage_error(error: impl ToString) -> kv::errors::SharedError {
    kv::errors::New(error.to_string())
}

/// 将 mock MVCC 写冲突保留为规范的可重试事务错误。
fn transaction_error(error: crate::storage::MockStorageError) -> kv::errors::SharedError {
    match error {
        crate::storage::MockStorageError::WriteConflict => kv::ErrTxnRetryable.FastGenByArgs(&[]),
        error => storage_error(error),
    }
}

/// 键不存在错误。
fn not_found() -> kv::errors::SharedError {
    kv::ErrNotExist.FastGenByArgs(&[])
}

/// 构造带提交时间戳的 ValueEntry。
fn value_entry(value: Vec<u8>, commit_ts: u64) -> kv::ValueEntry {
    kv::NewValueEntry(value, commit_ts)
}

/// 键的有损 UTF-8 字符串表示（BatchGet 映射用）。
fn key_name(key: &kv::Key) -> String {
    kv::KeyMapName(key.as_ref())
}

/// 基于预物化条目列表的规范迭代器。
pub struct CanonicalIterator {
    entries: Vec<(kv::Key, Vec<u8>)>,
    position: usize,
}

impl CanonicalIterator {
    /// 将原始字节键值转为 `kv::Key` 条目。
    fn new(entries: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(key, value)| (kv::Key(key), value))
                .collect(),
            position: 0,
        }
    }
}

/// 正向遍历预加载条目。
impl kv::Iterator for CanonicalIterator {
    fn Valid(&self) -> bool {
        self.position < self.entries.len()
    }

    fn Key(&self) -> kv::Key {
        self.entries
            .get(self.position)
            .map(|entry| entry.0.clone())
            .unwrap_or_default()
    }

    fn Value(&self) -> Vec<u8> {
        self.entries
            .get(self.position)
            .map(|entry| entry.1.clone())
            .unwrap_or_default()
    }

    fn Next(&mut self) -> Result<(), kv::errors::SharedError> {
        if !self.Valid() {
            return Err(storage_error("iterator is invalid"));
        }
        self.position += 1;
        Ok(())
    }

    fn Close(&mut self) {
        self.position = self.entries.len();
    }
}

/// 基于 BTreeMap 的只读 Getter/Retriever，用于 SnapshotGetter。
struct BufferedRetriever {
    entries: BTreeMap<Vec<u8>, Vec<u8>>,
}

/// 从缓冲 map 点查。
impl kv::Getter for BufferedRetriever {
    fn Get(
        &self,
        _ctx: &kv::context::Context,
        key: kv::Key,
        _options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        self.entries
            .get(key.as_ref())
            .cloned()
            .map(|value| value_entry(value, 0))
            .ok_or_else(not_found)
    }
}

/// 正/反向范围迭代。
impl kv::Retriever for BufferedRetriever {
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        let entries = self
            .entries
            .range(key.0..)
            .take_while(|(candidate, _)| {
                upper_bound
                    .as_ref()
                    .is_none_or(|upper| candidate.as_slice() < upper.as_ref())
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Ok(Box::new(CanonicalIterator::new(entries)))
    }

    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        let entries = self
            .entries
            .iter()
            .rev()
            .filter(|(candidate, _)| {
                key.as_ref()
                    .is_none_or(|upper| candidate.as_slice() < upper.as_ref())
                    && lower_bound
                        .as_ref()
                        .is_none_or(|lower| candidate.as_slice() >= lower.as_ref())
            })
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Ok(Box::new(CanonicalIterator::new(entries)))
    }
}

/// 事务读：优先本地写缓冲，否则按 start_ts 读存储。
impl kv::Getter for KVTxn {
    fn Get(
        &self,
        _ctx: &kv::context::Context,
        key: kv::Key,
        _options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        // 本地缓冲命中：Some 为值，None 为已删除。
        if let Some(value) = self.writes.get(key.as_ref()) {
            return value
                .clone()
                .map(|value| value_entry(value, 0))
                .ok_or_else(not_found);
        }
        if let Some(rpc) = self.store.EmbeddedRpc() {
            return rpc
                .get(
                    key.as_ref(),
                    self.start_ts,
                    self.request_priority,
                    self.request_marker,
                )?
                .map(|value| value_entry(value, 0))
                .ok_or_else(not_found);
        }
        self.store
            .read_entry_at(key.as_ref(), self.start_ts)
            // Go mockstore's transaction Getter does not expose MVCC commit_ts.
            .map(|(value, _commit_ts)| value_entry(value, 0))
            .ok_or_else(not_found)
    }
}

impl KVTxn {
    fn canonical_scan(
        &self,
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        reverse: bool,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, kv::errors::SharedError> {
        let Some(rpc) = self.store.EmbeddedRpc() else {
            return Ok(self.scan(lower, upper, reverse));
        };
        let mut rows = rpc
            .scan(
                self.start_ts,
                lower,
                upper,
                false,
                self.request_priority,
                self.request_marker,
            )?
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        for (key, value) in &self.writes {
            if lower.is_some_and(|lower| key.as_slice() < lower)
                || upper.is_some_and(|upper| key.as_slice() >= upper)
            {
                continue;
            }
            match value {
                Some(value) => {
                    rows.insert(key.clone(), value.clone());
                }
                None => {
                    rows.remove(key);
                }
            }
        }
        let mut rows = rows.into_iter().collect::<Vec<_>>();
        if reverse {
            rows.reverse();
        }
        Ok(rows)
    }
}

/// 事务扫描：合并快照与本地写。
impl kv::Retriever for KVTxn {
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        Ok(Box::new(CanonicalIterator::new(self.canonical_scan(
            Some(key.as_ref()),
            upper_bound.as_ref().map(AsRef::as_ref),
            false,
        )?)))
    }

    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        Ok(Box::new(CanonicalIterator::new(self.canonical_scan(
            lower_bound.as_ref().map(AsRef::as_ref),
            key.as_ref().map(AsRef::as_ref),
            true,
        )?)))
    }
}

/// 事务写：拒绝空值 Set，Delete 写入删除标记。
impl kv::Mutator for KVTxn {
    fn Set(&mut self, key: kv::Key, value: Vec<u8>) -> Result<(), kv::errors::SharedError> {
        if value.is_empty() {
            return Err(kv::ErrCannotSetNilValue.FastGenByArgs(&[]));
        }
        let previous_size = self
            .writes
            .get(&key.0)
            .map_or(0, |value| key.0.len() + value.as_ref().map_or(0, Vec::len));
        let total_size = self.buffered_write_size - previous_size + key.0.len() + value.len();
        if total_size as u64 > kv::TxnTotalSizeLimit.load(Ordering::Relaxed) {
            return Err(kv::ErrTxnTooLarge.FastGenByArgs(&[(total_size as u64).into()]));
        }
        self.Set(key.0, value);
        Ok(())
    }

    fn Delete(&mut self, key: kv::Key) -> Result<(), kv::errors::SharedError> {
        self.Delete(key.0);
        Ok(())
    }
}

/// Getter+Mutator 组合标记。
impl kv::RetrieverMutator for KVTxn {}

/// 内存写缓冲：flags、staging、快照视图与批量读。
impl kv::MemBuffer for KVTxn {
    fn RLock(&self) {}

    fn RUnlock(&self) {}

    fn GetFlags(&self, key: &kv::Key) -> Result<kv::KeyFlags, kv::errors::SharedError> {
        Ok(kv::KeyFlags(
            self.flags.get(key.as_ref()).copied().unwrap_or_default(),
        ))
    }

    fn SetWithFlags(
        &mut self,
        key: kv::Key,
        value: Vec<u8>,
        operations: &[kv::FlagsOp],
    ) -> Result<(), kv::errors::SharedError> {
        kv::Mutator::Set(self, key.clone(), value)?;
        self.UpdateFlags(key, operations);
        Ok(())
    }

    fn UpdateFlags(&mut self, key: kv::Key, operations: &[kv::FlagsOp]) {
        let flags = kv::ApplyFlagsOps(
            kv::KeyFlags(self.flags.get(key.as_ref()).copied().unwrap_or_default()),
            operations,
        );
        self.flags.insert(key.0, flags.0);
    }

    fn UpdateAssertionFlags(&mut self, key: kv::Key, operation: kv::AssertionOp) {
        let flags = kv::ApplyAssertionOp(
            kv::KeyFlags(self.flags.get(key.as_ref()).copied().unwrap_or_default()),
            operation,
        );
        self.flags.insert(key.0, flags.0);
    }

    fn DeleteWithFlags(
        &mut self,
        key: kv::Key,
        operations: &[kv::FlagsOp],
    ) -> Result<(), kv::errors::SharedError> {
        kv::Mutator::Delete(self, key.clone())?;
        self.UpdateFlags(key, operations);
        Ok(())
    }

    /// 开启一层 staging，返回句柄。
    fn Staging(&mut self) -> kv::StagingHandle {
        // 保存当前写集/flags 快照以便 Release/Cleanup。
        let handle = self.next_stage;
        self.next_stage += 1;
        self.stages
            .push((handle, self.writes.clone(), self.flags.clone()));
        handle
    }

    /// 释放 staging 层（简化：仅移除记录）。
    fn Release(&mut self, handle: kv::StagingHandle) {
        if let Some(index) = self.stages.iter().position(|stage| stage.0 == handle) {
            self.stages.remove(index);
        }
    }

    /// 回滚到指定 staging 层。
    fn Cleanup(&mut self, handle: kv::StagingHandle) {
        if let Some(index) = self.stages.iter().position(|stage| stage.0 == handle) {
            // 回滚到 staging 点并丢弃之后层级。
            let (_, writes, flags) = self.stages[index].clone();
            self.writes = writes;
            self.buffered_write_size = self
                .writes
                .iter()
                .map(|(key, value)| key.len() + value.as_ref().map_or(0, Vec::len))
                .sum();
            self.flags = flags;
            self.stages.truncate(index);
        }
    }

    /// 回调遍历相对 staging 基线新增/变更的键。
    fn InspectStage(
        &self,
        handle: kv::StagingHandle,
        callback: &mut dyn FnMut(kv::Key, kv::KeyFlags, Vec<u8>),
    ) {
        let baseline = self
            .stages
            .iter()
            .find(|stage| stage.0 == handle)
            .map(|stage| &stage.1);
        for (key, value) in &self.writes {
            // 跳过相对 staging 基线未变化的键。
            if baseline.is_some_and(|writes| writes.get(key) == Some(value)) {
                continue;
            }
            callback(
                kv::Key(key.clone()),
                kv::KeyFlags(self.flags.get(key).copied().unwrap_or_default()),
                value.clone().unwrap_or_default(),
            );
        }
    }

    /// 物化当前可见条目为只读 Getter。
    fn SnapshotGetter(&self) -> Box<dyn kv::Getter> {
        Box::new(BufferedRetriever {
            entries: self.scan(None, None, false).into_iter().collect(),
        })
    }

    fn SnapshotIter(&self, key: kv::Key, upper_bound: Option<kv::Key>) -> Box<dyn kv::Iterator> {
        Box::new(CanonicalIterator::new(self.scan(
            Some(key.as_ref()),
            upper_bound.as_ref().map(AsRef::as_ref),
            false,
        )))
    }

    fn SnapshotIterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Box<dyn kv::Iterator> {
        Box::new(CanonicalIterator::new(self.scan(
            lower_bound.as_ref().map(AsRef::as_ref),
            key.as_ref().map(AsRef::as_ref),
            true,
        )))
    }

    fn Len(&self) -> usize {
        self.writes.len()
    }

    fn Size(&self) -> usize {
        self.writes
            .iter()
            .map(|(key, value)| key.len() + value.as_ref().map_or(0, Vec::len))
            .sum()
    }

    fn RemoveFromBuffer(&mut self, key: kv::Key) {
        self.writes.remove(key.as_ref());
        self.flags.remove(key.as_ref());
    }

    fn GetLocal(
        &self,
        ctx: &kv::context::Context,
        key: &[u8],
    ) -> Result<Vec<u8>, kv::errors::SharedError> {
        kv::Getter::Get(self, ctx, kv::Key(key.to_vec()), &[]).map(|entry| entry.Value)
    }

    fn BatchGet(
        &self,
        ctx: &kv::context::Context,
        keys: &[Vec<u8>],
        options: &[kv::BatchGetOption],
    ) -> Result<HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        // 将 BatchGet 选项转为单点 Get 选项。
        let get_options = kv::BatchGetToGetOptions(options.to_vec()).unwrap_or_default();
        let mut result = HashMap::new();
        for key in keys {
            if let Ok(value) = kv::Getter::Get(self, ctx, kv::Key(key.clone()), &get_options) {
                result.insert(kv::KeyMapName(key), value);
            }
        }
        Ok(result)
    }
}

/// 委托内层 KVTxn。
impl kv::Getter for Transaction {
    fn Get(
        &self,
        ctx: &kv::context::Context,
        key: kv::Key,
        options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        if let Some(value) = self.inner.writes.get(key.as_ref()) {
            return value
                .clone()
                .map(|value| value_entry(value, 0))
                .ok_or_else(not_found);
        }
        kv::Getter::Get(&self.snapshot, ctx, key, options).map(|mut entry| {
            // Preserve the existing transaction Getter contract: snapshot
            // commit timestamps are not exposed through the union read.
            entry.CommitTs = 0;
            entry
        })
    }
}

/// 委托内层扫描。
impl kv::Retriever for Transaction {
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        kv::Retriever::Iter(&self.inner, key, upper_bound)
    }

    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        kv::Retriever::IterReverse(&self.inner, key, lower_bound)
    }
}

/// 写后触发内存占用钩子。
impl kv::Mutator for Transaction {
    fn Set(&mut self, key: kv::Key, value: Vec<u8>) -> Result<(), kv::errors::SharedError> {
        kv::Mutator::Set(&mut self.inner, key, value)?;
        // 通知上层内存占用变化。
        if let Some(hook) = &self.memory_hook {
            hook(kv::MemBuffer::Size(&self.inner) as u64);
        }
        Ok(())
    }

    fn Delete(&mut self, key: kv::Key) -> Result<(), kv::errors::SharedError> {
        kv::Mutator::Delete(&mut self.inner, key)?;
        if let Some(hook) = &self.memory_hook {
            hook(kv::MemBuffer::Size(&self.inner) as u64);
        }
        Ok(())
    }
}

/// Getter+Mutator 组合标记。
impl kv::RetrieverMutator for Transaction {}

/// 公平锁模式开关（mock 仅维护标志）。
impl kv::FairLockingController for Transaction {
    fn StartFairLocking(&mut self) -> Result<(), kv::errors::SharedError> {
        self.fair_locking = true;
        Ok(())
    }

    fn RetryFairLocking(
        &mut self,
        _ctx: &kv::context::Context,
    ) -> Result<(), kv::errors::SharedError> {
        self.fair_locking = true;
        Ok(())
    }

    fn CancelFairLocking(
        &mut self,
        _ctx: &kv::context::Context,
    ) -> Result<(), kv::errors::SharedError> {
        self.fair_locking = false;
        Ok(())
    }

    fn DoneFairLocking(
        &mut self,
        _ctx: &kv::context::Context,
    ) -> Result<(), kv::errors::SharedError> {
        self.fair_locking = false;
        Ok(())
    }

    fn IsInFairLockingMode(&self) -> bool {
        self.fair_locking
    }
}

/// 规范事务接口：提交/回滚、选项、表信息、流水线探测等。
impl kv::Transaction for Transaction {
    fn StageStatement(&mut self) -> Result<kv::StagingHandle, kv::errors::SharedError> {
        Ok(kv::MemBuffer::Staging(&mut self.inner))
    }

    fn ReleaseStatement(
        &mut self,
        handle: kv::StagingHandle,
    ) -> Result<(), kv::errors::SharedError> {
        kv::MemBuffer::Release(&mut self.inner, handle);
        Ok(())
    }

    fn CleanupStatement(
        &mut self,
        handle: kv::StagingHandle,
    ) -> Result<(), kv::errors::SharedError> {
        kv::MemBuffer::Cleanup(&mut self.inner, handle);
        Ok(())
    }
    fn Size(&self) -> usize {
        kv::MemBuffer::Size(&self.inner)
    }

    fn Mem(&self) -> u64 {
        kv::MemBuffer::Size(&self.inner) as u64
    }

    /// 注册内存占用变化钩子。
    fn SetMemoryFootprintChangeHook(&mut self, hook: Box<dyn Fn(u64)>) {
        self.memory_hook = Some(hook);
    }

    fn MemHookSet(&self) -> bool {
        self.memory_hook.is_some()
    }

    fn Len(&self) -> usize {
        kv::MemBuffer::Len(&self.inner)
    }

    /// 提交事务写集。
    fn Commit(&mut self, _ctx: &kv::context::Context) -> Result<(), kv::errors::SharedError> {
        if let Some(rpc) = self.inner.store.EmbeddedRpc() {
            if !self.inner.valid {
                return Err(storage_error("transaction is closed"));
            }
            let commit_ts = self
                .inner
                .store
                .CurrentTimestamp("global")
                .map_err(storage_error)?;
            if !self.inner.writes.is_empty() {
                if let Some(checker) = self
                    .options
                    .get(&kv::SchemaChecker)
                    .and_then(|value| value.downcast_ref::<kv::TransactionSchemaChecker>())
                {
                    (checker.0)(commit_ts)?;
                }
            }
            rpc.commit(
                self.inner.start_ts,
                commit_ts,
                &self.inner.writes,
                self.inner.request_priority,
                self.inner.request_marker,
            )?;
            self.inner.writes.clear();
            self.inner.buffered_write_size = 0;
            self.inner.valid = false;
            self.commit_ts = commit_ts;
            return Ok(());
        }
        let async_commit = self
            .options
            .get(&kv::EnableAsyncCommit)
            .and_then(|value| value.downcast_ref::<bool>())
            .copied()
            .unwrap_or(false);
        if let Some(checker) = self
            .options
            .get(&kv::SchemaChecker)
            .and_then(|value| value.downcast_ref::<kv::TransactionSchemaChecker>())
            .cloned()
        {
            self.commit_ts = self.inner.CommitWithSchemaChecker(
                async_commit,
                |timestamp| (checker.0)(timestamp),
                transaction_error,
            )?;
            return Ok(());
        }
        let commit_ts = if async_commit {
            self.inner.CommitAsync()
        } else {
            self.inner.Commit()
        }
        .map_err(transaction_error)?;
        self.commit_ts = commit_ts;
        Ok(())
    }

    /// 回滚事务。
    fn Rollback(&mut self) -> Result<(), kv::errors::SharedError> {
        if self.inner.valid {
            if let Some(rpc) = self.inner.store.EmbeddedRpc() {
                rpc.rollback(
                    self.inner.start_ts,
                    self.inner.writes.keys().cloned().collect(),
                    self.inner.request_priority,
                    self.inner.request_marker,
                )?;
            }
        }
        self.inner.Rollback().map_err(storage_error)
    }

    fn String(&self) -> String {
        format!("mock-txn-{}", self.inner.start_ts)
    }

    fn LockKeys(
        &mut self,
        ctx: &kv::context::Context,
        _lock_ctx: &mut kv::LockCtx,
        keys: &[kv::Key],
    ) -> Result<(), kv::errors::SharedError> {
        // Model an optimistic lock-only mutation as a same-value write. It
        // participates in the engine's atomic MVCC conflict check without
        // changing the snapshot value. The client-rust adapter uses real locks.
        for key in keys {
            match kv::GetValue(ctx, self, key.clone()) {
                Ok(value) => kv::Mutator::Set(self, key.clone(), value)?,
                Err(error) if kv::ErrNotExist.Equal(Some(&error)) => {
                    kv::Mutator::Delete(self, key.clone())?
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn LockKeysFunc(
        &mut self,
        ctx: &kv::context::Context,
        lock_ctx: &mut kv::LockCtx,
        callback: &mut dyn FnMut(),
        keys: &[kv::Key],
    ) -> Result<(), kv::errors::SharedError> {
        callback();
        self.LockKeys(ctx, lock_ctx, keys)
    }

    fn SetOption(&mut self, option: i32, value: Option<Box<dyn Any>>) {
        if option == kv::Priority {
            // Capture the caller's scoped context when configuring this
            // statement transaction. Separate metadata transactions stay unmarked.
            self.inner.request_marker = self
                .inner
                .store
                .EmbeddedRpc()
                .and_then(|rpc| rpc.client().request_marker());
            self.snapshot.request_marker = self.inner.request_marker;
            self.inner.request_priority = value
                .as_ref()
                .and_then(|value| value.downcast_ref::<i32>())
                .copied()
                .unwrap_or(kv::PriorityNormal);
            kv::Snapshot::SetOption(
                &mut self.snapshot,
                option,
                Some(Box::new(self.inner.request_priority)),
            );
        }
        if option == kv::Pessimistic {
            self.inner.SetPessimistic(
                value
                    .as_ref()
                    .and_then(|value| value.downcast_ref::<bool>())
                    .copied()
                    .unwrap_or(false),
            );
        }
        if let Some(value) = value {
            self.options.insert(option, value);
        } else {
            self.options.remove(&option);
        }
    }

    fn GetOption(&self, option: i32) -> Option<&dyn Any> {
        self.options.get(&option).map(Box::as_ref)
    }

    fn IsReadOnly(&self) -> bool {
        self.inner.writes.is_empty()
    }

    fn StartTS(&self) -> u64 {
        self.inner.start_ts
    }

    fn CommitTS(&self) -> u64 {
        self.commit_ts
    }

    fn Valid(&self) -> bool {
        self.inner.valid
    }

    fn GetMemBuffer(&self) -> &dyn kv::MemBuffer {
        &self.inner
    }

    fn GetSnapshot(&self) -> &dyn kv::Snapshot {
        &self.snapshot
    }

    fn SetVars(&mut self, vars: Box<dyn Any>) {
        self.vars = vars;
    }

    fn GetVars(&self) -> &dyn Any {
        self.vars.as_ref()
    }

    fn BatchGet(
        &self,
        ctx: &kv::context::Context,
        keys: &[kv::Key],
        _options: &[kv::BatchGetOption],
    ) -> Result<HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        let mut result = HashMap::new();
        for key in keys {
            match kv::Getter::Get(self, ctx, key.clone(), &[]) {
                Ok(value) => {
                    result.insert(key_name(key), value);
                }
                Err(err) if kv::IsErrNotFound(&err) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(result)
    }

    fn IsPessimistic(&self) -> bool {
        self.options
            .get(&kv::Pessimistic)
            .and_then(|value| value.downcast_ref::<bool>())
            .copied()
            .unwrap_or(false)
    }

    fn CacheTableInfo(&mut self, id: i64, info: kv::model::TableInfo) {
        self.table_info.insert(id, info);
    }

    fn GetTableInfo(&self, id: i64) -> Option<&kv::model::TableInfo> {
        self.table_info.get(&id)
    }

    fn SetDiskFullOpt(&mut self, _level: kv::kvrpcpb::DiskFullOpt) {}

    fn ClearDiskFullOpt(&mut self) {}

    fn GetMemDBCheckpoint(&self) -> &kv::tikv::MemDBCheckpoint {
        &self.checkpoint
    }

    fn RollbackMemDBToCheckpoint(&mut self, _checkpoint: &kv::tikv::MemDBCheckpoint) {}

    fn IsPipelined(&self) -> bool {
        self.inner
            .opts
            .iter()
            .any(|option| matches!(option, crate::storage::TxnOption::Pipelined(true)))
    }

    fn MayFlush(&mut self) -> Result<(), kv::errors::SharedError> {
        Ok(())
    }
}

/// 快照点查。
impl kv::Getter for Snapshot {
    fn Get(
        &self,
        _ctx: &kv::context::Context,
        key: kv::Key,
        _options: &[kv::GetOption],
    ) -> Result<kv::ValueEntry, kv::errors::SharedError> {
        if let Some(cached) = self.cache.borrow().get(key.as_ref()).cloned() {
            return cached.ok_or_else(not_found);
        }
        let value = if let Some(rpc) = self.store.EmbeddedRpc() {
            rpc.get(
                key.as_ref(),
                self.version,
                self.rpc_priority(),
                self.request_marker,
            )?
            .map(|value| value_entry(value, 0))
        } else {
            self.store
                .read_entry_at(key.as_ref(), self.version)
                .map(|(value, commit_ts)| value_entry(value, commit_ts))
        };
        self.cache
            .borrow_mut()
            .insert(key.as_ref().to_vec(), value.clone());
        value.ok_or_else(not_found)
    }
}

impl Snapshot {
    fn rpc_priority(&self) -> i32 {
        self.options
            .get(&kv::Priority)
            .and_then(|value| value.downcast_ref::<i32>())
            .copied()
            .unwrap_or(kv::PriorityNormal)
    }
    fn canonical_scan(
        &self,
        lower: Option<&[u8]>,
        upper: Option<&[u8]>,
        reverse: bool,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>, kv::errors::SharedError> {
        if let Some(rpc) = self.store.EmbeddedRpc() {
            rpc.scan(
                self.version,
                lower,
                upper,
                reverse,
                self.rpc_priority(),
                self.request_marker,
            )
        } else {
            Ok(self.store.scan_at(self.version, lower, upper, reverse))
        }
    }
}

/// 快照范围扫描。
impl kv::Retriever for Snapshot {
    fn Iter(
        &self,
        key: kv::Key,
        upper_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        Ok(Box::new(CanonicalIterator::new(self.canonical_scan(
            Some(key.as_ref()),
            upper_bound.as_ref().map(AsRef::as_ref),
            false,
        )?)))
    }

    fn IterReverse(
        &self,
        key: Option<kv::Key>,
        lower_bound: Option<kv::Key>,
    ) -> Result<Box<dyn kv::Iterator>, kv::errors::SharedError> {
        Ok(Box::new(CanonicalIterator::new(self.canonical_scan(
            lower_bound.as_ref().map(AsRef::as_ref),
            key.as_ref().map(AsRef::as_ref),
            true,
        )?)))
    }
}

/// BatchGet 与选项设置。
impl kv::Snapshot for Snapshot {
    fn BatchGet(
        &self,
        ctx: &kv::context::Context,
        keys: &[kv::Key],
        _options: &[kv::BatchGetOption],
    ) -> Result<HashMap<String, kv::ValueEntry>, kv::errors::SharedError> {
        let mut result = HashMap::new();
        for key in keys {
            match kv::Getter::Get(self, ctx, key.clone(), &[]) {
                Ok(value) => {
                    result.insert(key_name(key), value);
                }
                Err(err) if kv::IsErrNotFound(&err) => {}
                Err(err) => return Err(err),
            }
        }
        Ok(result)
    }

    fn SetOption(&mut self, option: i32, value: Option<Box<dyn Any>>) {
        if let Some(value) = value {
            self.options.insert(option, value);
        } else {
            self.options.remove(&option);
        }
    }

    fn SnapCacheSize(&self) -> usize {
        self.cache.borrow().len()
    }
}

/// UnionGet：先查表缓存，未命中则读快照并回填。
impl kv::MemManager for MemManager {
    /// 先查表级缓存，未命中则读快照并缓存。
    fn UnionGet(
        &self,
        ctx: &kv::context::Context,
        table_id: i64,
        snapshot: &dyn kv::Snapshot,
        key: &kv::Key,
    ) -> Result<Vec<u8>, kv::Error> {
        if let Some(value) = self.get_for_table(table_id, key.as_ref()) {
            return Ok(value);
        }
        let value = kv::GetValue(ctx, snapshot, key.clone())?;
        self.set_for_table(table_id, key.as_ref(), &value);
        Ok(value)
    }

    fn Delete(&self, table_id: i64) {
        self.delete_table(table_id);
    }
}

/// 不支持协处理器请求的占位 Client。
struct CanonicalClient;

/// 首次 Next 即报「未实现协处理器」的 Response。
struct UnsupportedResponse {
    closed: bool,
}

/// 未实现的协处理器响应流。
impl kv::Response for UnsupportedResponse {
    fn Next(
        &mut self,
        _ctx: &kv::context::Context,
    ) -> Result<Option<Box<dyn kv::ResultSubset>>, kv::errors::SharedError> {
        if self.closed {
            return Ok(None);
        }
        Err(storage_error(
            "mock storage does not implement coprocessor requests",
        ))
    }

    /// 关闭存储。
    fn Close(&mut self) -> Result<(), kv::errors::SharedError> {
        self.closed = true;
        Ok(())
    }
}

/// 始终返回 UnsupportedResponse。
impl kv::Client for CanonicalClient {
    fn Send(
        &self,
        _ctx: &kv::context::Context,
        _request: &kv::Request,
        _vars: &dyn Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        Some(Box::new(UnsupportedResponse { closed: false }))
    }

    fn IsRequestTypeSupported(&self, _request_type: i64, _sub_type: i64) -> bool {
        false
    }
}

/// 无 MPP store 的占位 MPP 客户端。
struct CanonicalMppClient;

/// MPP 任务构造为空；派发/建连报错。
impl kv::MPPClient for CanonicalMppClient {
    fn ConstructMPPTasks(
        &self,
        _ctx: &kv::Context,
        _request: &kv::MPPBuildTasksRequest,
        _timeout: Duration,
        _policy: kv::tiflashcompute::DispatchPolicy,
        _replica_read: kv::tiflash::ReplicaRead,
        _on_error: &mut dyn FnMut(kv::Error),
    ) -> Result<Vec<Box<dyn kv::MPPTaskMeta>>, kv::Error> {
        Ok(Vec::new())
    }

    fn DispatchMPPTask(
        &self,
        _param: kv::DispatchMPPTaskParam<'_>,
    ) -> Result<(kv::DispatchTaskResponse, bool), kv::Error> {
        Err(storage_error("mock storage has no MPP stores"))
    }

    fn EstablishMPPConns(
        &self,
        _param: kv::EstablishMPPConnsParam<'_>,
    ) -> Result<(kv::MPPStreamResponse, bool), kv::Error> {
        Err(storage_error("mock storage has no MPP stores"))
    }

    fn CancelMPPTasks(&self, _param: kv::CancelMPPTasksParam) {}

    fn CheckVisibility(&self, _start_time: u64) -> Result<(), kv::Error> {
        Ok(())
    }

    fn GetMPPStoreCount(&self) -> Result<i32, kv::Error> {
        Ok(0)
    }
}

/// Timestamp requests use this store's existing monotonic TSO clock.
struct CanonicalOracle(crate::storage::KVStore);
impl kv::oracle::Oracle for CanonicalOracle {
    fn GetTimestampAsync(&self, scope: &str) -> Option<Box<dyn kv::oracle::Future>> {
        Some(Box::new(kv::oracle::ReadyFuture(
            self.0.CurrentTimestamp(scope).map_err(storage_error),
        )))
    }
}

/// Per-store delegation permits test wrappers without changing other stores.
#[derive(Clone)]
pub struct OracleHandle(std::sync::Arc<std::sync::RwLock<std::sync::Arc<dyn kv::oracle::Oracle>>>);
impl OracleHandle {
    pub(crate) fn new(store: crate::storage::KVStore) -> Self {
        Self(std::sync::Arc::new(std::sync::RwLock::new(
            std::sync::Arc::new(CanonicalOracle(store)),
        )))
    }
    pub fn GetOracle(&self) -> std::sync::Arc<dyn kv::oracle::Oracle> {
        self.0.read().expect("oracle lock poisoned").clone()
    }
    pub fn SetOracle(&self, oracle: std::sync::Arc<dyn kv::oracle::Oracle>) {
        *self.0.write().expect("oracle lock poisoned") = oracle;
    }
}
impl kv::oracle::Oracle for OracleHandle {
    fn GetTimestampAsync(&self, scope: &str) -> Option<Box<dyn kv::oracle::Future>> {
        self.GetOracle().GetTimestampAsync(scope)
    }
    fn GetLowResolutionTimestampAsync(&self, scope: &str) -> Option<Box<dyn kv::oracle::Future>> {
        self.GetOracle().GetLowResolutionTimestampAsync(scope)
    }
}

static CANONICAL_CLIENT: CanonicalClient = CanonicalClient;
static CANONICAL_MPP_CLIENT: CanonicalMppClient = CanonicalMppClient;

/// 将动态类型键下转为 OptionKey。
fn option_key(key: &dyn Any) -> Option<OptionKey> {
    if let Some(value) = key.downcast_ref::<String>() {
        return Some(OptionKey::String(value.clone()));
    }
    if let Some(value) = key.downcast_ref::<&'static str>() {
        return Some(OptionKey::String((*value).to_owned()));
    }
    if let Some(value) = key.downcast_ref::<i64>() {
        return Some(OptionKey::Signed(*value));
    }
    if let Some(value) = key.downcast_ref::<u64>() {
        return Some(OptionKey::Unsigned(*value));
    }
    key.downcast_ref::<bool>().copied().map(OptionKey::Bool)
}

/// 将常见选项值泄漏为 static 引用（对齐 Go 选项生命周期简化）。
fn leak_option_value(value: Box<dyn Any>) -> Option<&'static (dyn Any + Send + Sync)> {
    let value = match value.downcast::<String>() {
        Ok(value) => return Some(Box::leak(value)),
        Err(value) => value,
    };
    let value = match value.downcast::<Vec<u8>>() {
        Ok(value) => return Some(Box::leak(value)),
        Err(value) => value,
    };
    let value = match value.downcast::<bool>() {
        Ok(value) => return Some(Box::leak(value)),
        Err(value) => value,
    };
    let value = match value.downcast::<i64>() {
        Ok(value) => return Some(Box::leak(value)),
        Err(value) => value,
    };
    let value = match value.downcast::<u64>() {
        Ok(value) => return Some(Box::leak(value)),
        Err(value) => value,
    };
    let value = match value.downcast::<usize>() {
        Ok(value) => return Some(Box::leak(value)),
        Err(value) => value,
    };
    drop(value);
    None
}

/// mockStorage 的规范 Storage 适配。
impl kv::Storage for mockStorage {
    fn ImportSST(
        &self,
        commit_ts: u64,
        pairs: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<kv::SSTImportStats, kv::errors::SharedError> {
        // This explicitly selected mock KV store has no TiKV RPC endpoint.
        // Preserve physical import TS and historical ordering in local MVCC.

        let stats = kv::SSTImportStats {
            keys: pairs.len(),
            bytes: pairs
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum(),
            ..Default::default()
        };
        self.KVStore
            .ingest_sst(commit_ts, pairs)
            .map_err(storage_error)?;
        Ok(stats)
    }
    /// 开启规范事务。
    fn Begin(
        &self,
        options: &[kv::tikv::TxnOption],
    ) -> Result<Box<dyn kv::Transaction>, kv::errors::SharedError> {
        let options = options
            .iter()
            .filter_map(|option| match option {
                kv::tikv::TxnOption::Default => None,
                kv::tikv::TxnOption::StartTS(start_ts) => {
                    Some(crate::storage::TxnOption::StartTS(*start_ts))
                }
            })
            .collect::<Vec<_>>();
        mockStorage::Begin(self, &options)
            .map(|transaction| Box::new(transaction) as Box<dyn kv::Transaction>)
            .map_err(storage_error)
    }

    /// 取规范快照。
    fn GetSnapshot(&self, version: kv::Version) -> Box<dyn kv::Snapshot> {
        Box::new(mockStorage::GetSnapshot(
            self,
            crate::storage::Version { Ver: version.Ver },
        ))
    }

    fn GetClient(&self) -> &dyn kv::Client {
        &CANONICAL_CLIENT
    }

    fn GetMPPClient(&self) -> &dyn kv::MPPClient {
        &CANONICAL_MPP_CLIENT
    }

    fn Close(&mut self) -> Result<(), kv::errors::SharedError> {
        mockStorage::Close(self).map_err(storage_error)
    }

    fn UUID(&self) -> String {
        mockStorage::UUID(self)
    }

    fn CurrentVersion(
        &self,
        transaction_scope: &str,
    ) -> Result<kv::Version, kv::errors::SharedError> {
        mockStorage::CurrentVersion(self, transaction_scope)
            .map(|version| kv::NewVersion(version.Ver))
            .map_err(storage_error)
    }

    fn GetOracle(&self) -> &dyn kv::oracle::Oracle {
        &self.canonical_oracle
    }

    fn SupportDeleteRange(&self) -> bool {
        mockStorage::SupportDeleteRange(self)
    }

    fn Name(&self) -> String {
        mockStorage::Name(self)
    }

    fn Describe(&self) -> String {
        mockStorage::Describe(self)
    }

    fn ShowStatus(
        &self,
        _ctx: &kv::context::Context,
        _key: &str,
    ) -> Result<Box<dyn Any>, kv::errors::SharedError> {
        Err(kv::ErrNotImplemented.FastGenByArgs(&[]))
    }

    fn GetMemCache(&self) -> &dyn kv::MemManager {
        self.canonical_mem_cache()
    }

    fn GetMinSafeTS(&self, transaction_scope: &str) -> u64 {
        mockStorage::GetMinSafeTS(self, transaction_scope)
    }

    fn TSORequestCountForTest(&self) -> u64 {
        mockStorage::TSORequestCount(self)
    }

    fn GetLockWaits(&self) -> Result<Vec<kv::deadlockpb::WaitForEntry>, kv::errors::SharedError> {
        mockStorage::GetLockWaits(self)
            .map(|waits| {
                waits
                    .into_iter()
                    .map(|entry| kv::deadlockpb::WaitForEntry {
                        txn: entry.txn,
                        wait_for_txn: entry.wait_for_txn,
                        key_hash: entry.key_hash,
                        ..Default::default()
                    })
                    .collect()
            })
            .map_err(storage_error)
    }

    fn GetCodec(&self) -> kv::tikv::Codec {
        kv::tikv::Codec
    }

    /// 以泄漏 static 方式保存规范选项。
    fn SetOption(&self, key: Box<dyn Any>, value: Box<dyn Any>) {
        // 无法识别的选项键则忽略。
        let Some(key) = option_key(key.as_ref()) else {
            return;
        };
        // 无法泄漏为 static 的值类型则忽略。
        let Some(value) = leak_option_value(value) else {
            return;
        };
        self.canonicalOptions
            .write()
            .expect("canonical options lock poisoned")
            .insert(key, value);
    }

    /// 读取规范选项。
    fn GetOption(&self, key: &dyn Any) -> Option<&dyn Any> {
        let key = option_key(key)?;
        self.canonicalOptions
            .read()
            .expect("canonical options lock poisoned")
            .get(&key)
            .copied()
            .map(|value| value as &dyn Any)
    }

    fn GetClusterID(&self) -> u64 {
        mockStorage::GetClusterID(self)
    }

    fn GetKeyspace(&self) -> String {
        mockStorage::GetKeyspace(self)
    }
}

// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// KV 故障注入（Fault Injection）：在 Storage/Transaction/Snapshot 外包一层可配置错误。
//
// 测试或混沌场景下，通过 `InjectionConfig` 强制 Get/BatchGet/Commit 返回指定错误，
// 其余方法透明转发到底层实现。快照（Snapshot）是某一版本时间戳下的只读视图。

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// 本文件内共享错误别名。
pub type SharedError = Error;

/// 可注入的读错误与提交错误，受 RwLock 保护以便并发测试改配。
#[derive(Default)]
struct InjectionErrors {
    getError: Option<SharedError>,
    commitError: Option<SharedError>,
}

/// 故障注入配置：运行时切换 Get/Commit 注入错误。
#[derive(Default)]
pub struct InjectionConfig {
    errors: RwLock<InjectionErrors>,
}

impl InjectionConfig {
    /// 设置或清除 Get/BatchGet 路径注入的错误。
    pub fn SetGetError(&self, err: Option<SharedError>) {
        if let Ok(mut errors) = self.errors.write() {
            errors.getError = err;
        }
    }

    /// 设置或清除 Commit 路径注入的错误。
    pub fn SetCommitError(&self, err: Option<SharedError>) {
        if let Ok(mut errors) = self.errors.write() {
            errors.commitError = err;
        }
    }
}

/// 包装真实 Storage，使 Begin/GetSnapshot 返回注入版事务与快照。
pub struct InjectedStore {
    Storage: Box<dyn Storage>,
    cfg: Arc<InjectionConfig>,
}

/// 构造带故障注入的 Storage 包装。
pub fn NewInjectedStore(store: Box<dyn Storage>, cfg: Arc<InjectionConfig>) -> Box<dyn Storage> {
    Box::new(InjectedStore {
        Storage: store,
        cfg,
    })
}

impl Storage for InjectedStore {
    fn Begin(&self, opts: &[tikv::TxnOption]) -> Result<Box<dyn Transaction>, Error> {
        // 开启事务后包一层 InjectedTransaction，共享同一份 cfg。
        let transaction = self.Storage.Begin(opts)?;
        Ok(Box::new(InjectedTransaction {
            Transaction: transaction,
            cfg: Arc::clone(&self.cfg),
        }))
    }

    fn GetSnapshot(&self, ver: Version) -> Box<dyn Snapshot> {
        Box::new(InjectedSnapshot {
            Snapshot: self.Storage.GetSnapshot(ver),
            cfg: Arc::clone(&self.cfg),
        })
    }

    fn GetClient(&self) -> &dyn Client {
        self.Storage.GetClient()
    }
    fn GetMPPClient(&self) -> &dyn MPPClient {
        self.Storage.GetMPPClient()
    }
    fn Close(&mut self) -> Result<(), Error> {
        self.Storage.Close()
    }
    fn UUID(&self) -> String {
        self.Storage.UUID()
    }
    fn CurrentVersion(&self, scope: &str) -> Result<Version, Error> {
        self.Storage.CurrentVersion(scope)
    }
    fn GetOracle(&self) -> &dyn oracle::Oracle {
        self.Storage.GetOracle()
    }
    fn SupportDeleteRange(&self) -> bool {
        self.Storage.SupportDeleteRange()
    }
    fn Name(&self) -> String {
        self.Storage.Name()
    }
    fn Describe(&self) -> String {
        self.Storage.Describe()
    }
    fn ShowStatus(&self, ctx: &context::Context, key: &str) -> Result<Box<dyn Any>, Error> {
        self.Storage.ShowStatus(ctx, key)
    }
    fn GetMemCache(&self) -> &dyn MemManager {
        self.Storage.GetMemCache()
    }
    fn GetMinSafeTS(&self, scope: &str) -> u64 {
        self.Storage.GetMinSafeTS(scope)
    }
    fn GetLockWaits(&self) -> Result<Vec<deadlockpb::WaitForEntry>, Error> {
        self.Storage.GetLockWaits()
    }
    fn GetCodec(&self) -> tikv::Codec {
        self.Storage.GetCodec()
    }
    fn SetOption(&self, key: Box<dyn Any>, value: Box<dyn Any>) {
        self.Storage.SetOption(key, value)
    }
    fn GetOption(&self, key: &dyn Any) -> Option<&dyn Any> {
        self.Storage.GetOption(key)
    }
    fn GetClusterID(&self) -> u64 {
        self.Storage.GetClusterID()
    }
    fn GetKeyspace(&self) -> String {
        self.Storage.GetKeyspace()
    }
}

/// 包装真实事务：Get/BatchGet/Commit 可按配置短路返回注入错误。
pub struct InjectedTransaction {
    Transaction: Box<dyn Transaction>,
    cfg: Arc<InjectionConfig>,
}

impl Getter for InjectedTransaction {
    fn Get(
        &self,
        ctx: &context::Context,
        key: Key,
        options: &[GetOption],
    ) -> Result<ValueEntry, Error> {
        let errors = self
            .cfg
            .errors
            .read()
            .map_err(|err| errors::New(err.to_string()))?;
        // 若配置了 getError，优先返回注入错误，不触达底层。
        if let Some(err) = &errors.getError {
            return Err(err.clone());
        }
        self.Transaction.Get(ctx, key, options)
    }
}

impl Retriever for InjectedTransaction {
    fn Iter(&self, key: Key, upper: Option<Key>) -> Result<Box<dyn Iterator>, Error> {
        self.Transaction.Iter(key, upper)
    }
    fn IterReverse(
        &self,
        key: Option<Key>,
        lower: Option<Key>,
    ) -> Result<Box<dyn Iterator>, Error> {
        self.Transaction.IterReverse(key, lower)
    }
}

impl Mutator for InjectedTransaction {
    fn Set(&mut self, key: Key, value: Vec<u8>) -> Result<(), Error> {
        self.Transaction.Set(key, value)
    }
    fn Delete(&mut self, key: Key) -> Result<(), Error> {
        self.Transaction.Delete(key)
    }
}

impl RetrieverMutator for InjectedTransaction {}

impl FairLockingController for InjectedTransaction {
    fn StartFairLocking(&mut self) -> Result<(), Error> {
        self.Transaction.StartFairLocking()
    }
    fn RetryFairLocking(&mut self, ctx: &context::Context) -> Result<(), Error> {
        self.Transaction.RetryFairLocking(ctx)
    }
    fn CancelFairLocking(&mut self, ctx: &context::Context) -> Result<(), Error> {
        self.Transaction.CancelFairLocking(ctx)
    }
    fn DoneFairLocking(&mut self, ctx: &context::Context) -> Result<(), Error> {
        self.Transaction.DoneFairLocking(ctx)
    }
    fn IsInFairLockingMode(&self) -> bool {
        self.Transaction.IsInFairLockingMode()
    }
}

impl Transaction for InjectedTransaction {
    fn Size(&self) -> usize {
        self.Transaction.Size()
    }
    fn Mem(&self) -> u64 {
        self.Transaction.Mem()
    }
    fn SetMemoryFootprintChangeHook(&mut self, hook: Box<dyn Fn(u64)>) {
        self.Transaction.SetMemoryFootprintChangeHook(hook)
    }
    fn MemHookSet(&self) -> bool {
        self.Transaction.MemHookSet()
    }
    fn Len(&self) -> usize {
        self.Transaction.Len()
    }
    fn Commit(&mut self, ctx: &context::Context) -> Result<(), Error> {
        let errors = self
            .cfg
            .errors
            .read()
            .map_err(|err| errors::New(err.to_string()))?;
        // 提交路径独立于读路径注入，便于单独模拟两阶段提交失败。
        if let Some(err) = &errors.commitError {
            return Err(err.clone());
        }
        self.Transaction.Commit(ctx)
    }
    fn Rollback(&mut self) -> Result<(), Error> {
        self.Transaction.Rollback()
    }
    fn String(&self) -> String {
        self.Transaction.String()
    }
    fn LockKeys(
        &mut self,
        ctx: &context::Context,
        lock_ctx: &mut LockCtx,
        keys: &[Key],
    ) -> Result<(), Error> {
        self.Transaction.LockKeys(ctx, lock_ctx, keys)
    }
    fn LockKeysFunc(
        &mut self,
        ctx: &context::Context,
        lock_ctx: &mut LockCtx,
        f: &mut dyn FnMut(),
        keys: &[Key],
    ) -> Result<(), Error> {
        self.Transaction.LockKeysFunc(ctx, lock_ctx, f, keys)
    }
    fn SetOption(&mut self, opt: i32, val: Option<Box<dyn Any>>) {
        self.Transaction.SetOption(opt, val)
    }
    fn GetOption(&self, opt: i32) -> Option<&dyn Any> {
        self.Transaction.GetOption(opt)
    }
    fn IsReadOnly(&self) -> bool {
        self.Transaction.IsReadOnly()
    }
    fn StartTS(&self) -> u64 {
        self.Transaction.StartTS()
    }
    fn CommitTS(&self) -> u64 {
        self.Transaction.CommitTS()
    }
    fn Valid(&self) -> bool {
        self.Transaction.Valid()
    }
    fn GetMemBuffer(&self) -> &dyn MemBuffer {
        self.Transaction.GetMemBuffer()
    }
    fn GetSnapshot(&self) -> &dyn Snapshot {
        self.Transaction.GetSnapshot()
    }
    fn SetVars(&mut self, vars: Box<dyn Any>) {
        self.Transaction.SetVars(vars)
    }
    fn GetVars(&self) -> &dyn Any {
        self.Transaction.GetVars()
    }
    fn BatchGet(
        &self,
        ctx: &context::Context,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, Error> {
        let errors = self
            .cfg
            .errors
            .read()
            .map_err(|err| errors::New(err.to_string()))?;
        if let Some(err) = &errors.getError {
            return Err(err.clone());
        }
        self.Transaction.BatchGet(ctx, keys, options)
    }
    fn IsPessimistic(&self) -> bool {
        self.Transaction.IsPessimistic()
    }
    fn CacheTableInfo(&mut self, id: i64, info: model::TableInfo) {
        self.Transaction.CacheTableInfo(id, info)
    }
    fn GetTableInfo(&self, id: i64) -> Option<&model::TableInfo> {
        self.Transaction.GetTableInfo(id)
    }
    fn SetDiskFullOpt(&mut self, level: kvrpcpb::DiskFullOpt) {
        self.Transaction.SetDiskFullOpt(level)
    }
    fn ClearDiskFullOpt(&mut self) {
        self.Transaction.ClearDiskFullOpt()
    }
    fn GetMemDBCheckpoint(&self) -> &tikv::MemDBCheckpoint {
        self.Transaction.GetMemDBCheckpoint()
    }
    fn RollbackMemDBToCheckpoint(&mut self, checkpoint: &tikv::MemDBCheckpoint) {
        self.Transaction.RollbackMemDBToCheckpoint(checkpoint)
    }
    fn IsPipelined(&self) -> bool {
        self.Transaction.IsPipelined()
    }
    fn MayFlush(&mut self) -> Result<(), Error> {
        self.Transaction.MayFlush()
    }
}

/// 包装真实快照：Get/BatchGet 可按配置返回注入错误。
pub struct InjectedSnapshot {
    Snapshot: Box<dyn Snapshot>,
    cfg: Arc<InjectionConfig>,
}

impl Getter for InjectedSnapshot {
    fn Get(
        &self,
        ctx: &context::Context,
        key: Key,
        options: &[GetOption],
    ) -> Result<ValueEntry, Error> {
        let errors = self
            .cfg
            .errors
            .read()
            .map_err(|err| errors::New(err.to_string()))?;
        if let Some(err) = &errors.getError {
            return Err(err.clone());
        }
        self.Snapshot.Get(ctx, key, options)
    }
}

impl Retriever for InjectedSnapshot {
    fn Iter(&self, key: Key, upper: Option<Key>) -> Result<Box<dyn Iterator>, Error> {
        self.Snapshot.Iter(key, upper)
    }
    fn IterReverse(
        &self,
        key: Option<Key>,
        lower: Option<Key>,
    ) -> Result<Box<dyn Iterator>, Error> {
        self.Snapshot.IterReverse(key, lower)
    }
}

impl Snapshot for InjectedSnapshot {
    fn BatchGet(
        &self,
        ctx: &context::Context,
        keys: &[Key],
        options: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, Error> {
        let errors = self
            .cfg
            .errors
            .read()
            .map_err(|err| errors::New(err.to_string()))?;
        if let Some(err) = &errors.getError {
            return Err(err.clone());
        }
        self.Snapshot.BatchGet(ctx, keys, options)
    }

    fn SetOption(&mut self, opt: i32, val: Option<Box<dyn Any>>) {
        self.Snapshot.SetOption(opt, val)
    }
}

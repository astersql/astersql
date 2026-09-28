// Copyright 2019-present PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// DB 写路径抽象：写批、锁存与快照。
//
// 对应 Go unistore MVCC 的 DBWriter/WriteBatch。预写（Prewrite）、提交（Commit）、
// 回滚与悲观锁操作通过 WriteBatch 批量落到存储；DBBundle/DBSnapshot 聚合引擎与锁存储。

#![allow(non_snake_case, non_camel_case_types, dead_code)]

use std::sync::{Arc, Mutex};

use crate::lockstore::MemStore;
use anyhow::Result;
use kvproto::kvrpcpb;

use crate::mvcc::Lock;

/// 底层存储写接口：打开/关闭、写批提交、按范围删除与创建写批。
pub trait DBWriter {
    /// 打开写路径资源。
    fn Open(&mut self);
    /// 关闭写路径资源。
    fn Close(&mut self);
    /// 将写批持久化到存储引擎。
    fn Write(&mut self, batch: Box<dyn WriteBatch>) -> Result<()>;
    /// 在持有 latch 的前提下删除 `[start, end)` 范围数据。
    fn DeleteRange(
        &mut self,
        start: &[u8],
        end: &[u8],
        latchHandle: &dyn LatchHandle,
    ) -> Result<()>;
    /// 按 start/commit 时间戳创建新的写批（两阶段提交上下文）。
    fn NewWriteBatch(
        &self,
        startTS: u64,
        commitTS: u64,
        ctx: Option<&kvrpcpb::Context>,
    ) -> Box<dyn WriteBatch>;
}

/// Latch（闩锁）句柄：按 hash 值批量获取/释放，避免并发写冲突。
pub trait LatchHandle {
    /// 获取一组 hash 对应的 latch。
    fn AcquireLatches(&self, hashVals: &[u64]);
    /// 释放一组 hash 对应的 latch。
    fn ReleaseLatches(&self, hashVals: &[u64]);
}

/// 事务写批：封装预写、提交、回滚与悲观锁变更。
pub trait WriteBatch {
    /// 两阶段提交第一阶段：写入锁（Lock）。
    fn Prewrite(&mut self, key: &[u8], lock: &mut Lock);
    /// 两阶段提交第二阶段：将锁转为已提交写。
    fn Commit(&mut self, key: &[u8], lock: &mut Lock);
    /// 回滚事务；`deleleLock` 控制是否删除锁记录。
    fn Rollback(&mut self, key: &[u8], deleleLock: bool);
    /// 写入悲观锁（Pessimistic Lock）。
    fn PessimisticLock(&mut self, key: &[u8], lock: &mut Lock);
    /// 回滚悲观锁。
    fn PessimisticRollback(&mut self, key: &[u8]);
}

/// 为 [`NewDBSnapshot`] 提供只读快照来源的后端能力。
/// Backend operation needed by [`NewDBSnapshot`].
///
/// Badger's `NewTransaction(false)` is represented by an associated snapshot
/// type, so the MVCC interfaces remain usable with the storage engine selected
/// by the package integration task.
pub trait DBSnapshotSource {
    type Snapshot;

    fn NewReadSnapshot(&self) -> Self::Snapshot;
}

/// 数据库束：引擎实例、内存锁存储、互斥与状态时间戳。
// DBBundle represents the db bundle.
pub struct DBBundle<D> {
    pub DB: D,
    pub LockStore: Arc<MemStore>,
    pub MemStoreMu: Mutex<()>,
    pub StateTS: u64,
}

/// 读快照：引擎事务快照加上共享的锁存储视图。
// DBSnapshot represents the db snapshot.
pub struct DBSnapshot<S> {
    pub Txn: S,
    pub LockStore: Arc<MemStore>,
}

/// 从 DBBundle 创建读快照（共享 LockStore 引用）。
// NewDBSnapshot returns a new db snapshot.
pub fn NewDBSnapshot<D>(db: &DBBundle<D>) -> DBSnapshot<D::Snapshot>
where
    D: DBSnapshotSource,
{
    DBSnapshot {
        Txn: db.DB.NewReadSnapshot(),
        LockStore: Arc::clone(&db.LockStore),
    }
}

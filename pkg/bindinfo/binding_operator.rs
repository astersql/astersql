// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// SQL 绑定（SQL Binding）操作器模块。
//
// SQL 绑定是数据库中的一种执行计划管理机制：通过把某条 SQL（以其
// “SQL 摘要 / SQL Digest”，即对 SQL 归一化后计算出的哈希值来标识）
// 与一组优化器提示（Hint）绑定在一起，强制优化器在执行该 SQL 时采用
// 指定的执行计划，从而避免因统计信息变化等原因导致的计划抖动。
//
// 本模块定义了绑定的核心写路径操作接口 [`BindingOperator`] 及其默认
// 实现 [`bindingOperator`]，涵盖以下能力：
// - 创建绑定（`CreateBinding`）：校验并写入存储，同时刷新内存缓存；
// - 删除绑定（`DropBinding`）：将绑定标记为已删除并从缓存移除；
// - 修改绑定状态（`SetBindingStatus`）：如启用/禁用某条绑定；
// - 垃圾回收（`GCBinding`）：物理清理早已标记删除的过期绑定记录。
//
// 所有写操作都遵循“先持久化存储、后更新缓存”的顺序，以保证缓存
// 中的数据不会领先于持久化状态。

use crate::{
    BindError, Binding, BindingCacheUpdater, BindingSqlContext, BindingStore, BindingTime, Result,
    prepareHints,
};
use std::sync::Arc;
use std::time::Duration;

/// SQL 绑定写路径操作接口，定义绑定生命周期管理的四类操作。
///
/// 实现方需要保证线程安全（`Send + Sync`），因为绑定操作可能被
/// 多个会话并发调用。
pub trait BindingOperator: Send + Sync {
    /// 创建（或覆盖）一批绑定。
    ///
    /// `sctx` 提供绑定校验能力（如解析并校验 Hint 是否合法），
    /// `bindings` 为待创建的绑定列表；同一 SQL 摘要的旧绑定会被替换。
    fn CreateBinding(
        &self,
        sctx: &dyn crate::BindingValidator,
        bindings: Vec<Binding>,
    ) -> Result<()>;
    /// 按 SQL 摘要删除绑定（逻辑删除），返回受影响的绑定数量。
    fn DropBinding(&self, sqlDigests: &[String]) -> Result<u64>;
    /// 修改指定 SQL 摘要绑定的状态（如启用/禁用），
    /// 返回状态是否发生了实际变更。
    fn SetBindingStatus(&self, newStatus: &str, sqlDigest: &str) -> Result<bool>;
    /// 垃圾回收：物理删除超过保留期限的“已删除”绑定，返回清理数量。
    fn GCBinding(&self) -> Result<u64>;
}

/// [`BindingOperator`] 的默认实现，组合了持久化存储与内存缓存。
pub struct bindingOperator {
    /// 绑定的持久化存储（对应系统表中的绑定记录）。
    pub store: Arc<dyn BindingStore>,
    /// 绑定的内存缓存更新器，用于让优化器快速查询绑定。
    pub cache: Arc<dyn BindingCacheUpdater>,
    /// 绑定加载租约（lease）：缓存从存储同步的周期，
    /// 同时用于推算垃圾回收的安全保留时间窗口。
    pub lease: Duration,
}

/// 构造一个默认的绑定操作器，并以 trait 对象形式返回。
pub fn newBindingOperator(
    store: Arc<dyn BindingStore>,
    cache: Arc<dyn BindingCacheUpdater>,
    lease: Duration,
) -> Arc<dyn BindingOperator> {
    Arc::new(bindingOperator {
        store,
        cache,
        lease,
    })
}

/// 故障注入点名称：用于测试中模拟“加载绑定存在时间延迟”的场景。
pub static TestTimeLagInLoadingBinding: &str = "TestTimeLagInLoadingBinding";

impl BindingOperator for bindingOperator {
    fn CreateBinding(
        &self,
        sctx: &dyn crate::BindingValidator,
        mut bindings: Vec<Binding>,
    ) -> Result<()> {
        let mut prepared = Vec::with_capacity(bindings.len());
        // 逐条校验并补全绑定元数据：Hint 需要预解析，数据库名按存储契约
        // 转为小写，并统一填充创建/更新时间戳。空摘要按 Go 语义持久化为 NULL。
        for binding in &mut bindings {
            prepareHints(sctx, binding)?;
            let now = BindingTime::now();
            binding.Db.make_ascii_lowercase();
            binding.CreateTime = now;
            binding.UpdateTime = now;
            prepared.push(Arc::new(binding.clone()));
        }
        // 先写入持久化存储（覆盖同摘要的旧绑定），成功后再同步内存缓存，
        // 保证缓存不会包含未落盘的数据。
        self.store.replace_bindings(&prepared)?;
        self.cache.LoadFromStorageToCache(false, false)
    }

    fn DropBinding(&self, sqlDigests: &[String]) -> Result<u64> {
        if sqlDigests.is_empty() {
            return Err(BindError("sql digest is empty".to_owned()));
        }
        // 逻辑删除：存储层只把状态标记为已删除（保留记录供后续 GC），
        // 随后从存储增量重载，使缓存按墓碑和时间顺序统一合并。
        let deleted = self.store.mark_deleted(sqlDigests, BindingTime::now())?;
        self.cache.LoadFromStorageToCache(false, false)?;
        Ok(deleted)
    }

    fn SetBindingStatus(&self, newStatus: &str, sqlDigest: &str) -> Result<bool> {
        // Go 只为 enabled/disabled 构造旧状态过滤条件；其它目标状态
        // 不会命中记录，但成功路径仍会刷新缓存。
        let changed = if newStatus == crate::StatusEnabled || newStatus == crate::StatusDisabled {
            self.store
                .set_status(sqlDigest, newStatus, BindingTime::now())?
        } else {
            false
        };
        self.cache.LoadFromStorageToCache(false, false)?;
        Ok(changed)
    }

    fn GCBinding(&self) -> Result<u64> {
        // 只清理更新时间早于“当前时间 - 10 倍租约”的已删除绑定，
        // 留出足够的时间窗口，确保所有节点的缓存都已感知到删除，
        // 避免误删仍可能被读取的记录。
        let lease_micros = i64::try_from(self.lease.as_micros()).unwrap_or(i64::MAX);
        let cutoff = BindingTime(
            BindingTime::now()
                .0
                .saturating_sub(lease_micros.saturating_mul(10)),
        );
        self.store.gc_deleted_before(cutoff)
    }
}

/// 对绑定信息系统表加锁，用于在多节点间串行化绑定的写操作，
/// 防止并发修改导致绑定数据不一致。
pub fn lockBindInfoTable(sctx: &dyn BindingSqlContext) -> Result<()> {
    sctx.execute(crate::LockBindInfoSQL, &[]).map(|_| ())
}

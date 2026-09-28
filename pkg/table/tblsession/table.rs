// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 基于会话的表突变上下文实现。
//
// 将 `SessionContext` 适配为 `tblctx::MutateContext`，转发连接信息、行编码配置、
// 统计增量、缓存表 handle、临时表与交换分区约束检查等能力。

use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;

use crate::{autoid, exprctx, infoschema, model, rowcodec, stmtctx, tblctx, variable};

#[cfg(feature = "intest")]
use crate::intest;

/// Transaction state used by table mutation support.
///
/// The optional owner remains in the session implementation, matching Go's
/// nullable `SessionVars.TxnCtx`. This value only contains the two maps touched
/// by this package.
/// 表突变用到的事务状态子集（对应 Go `TxnCtx` 中本包触碰的两个 map）。
#[derive(Default)]
pub struct TransactionContext {
    /// 物理表 ID → 行数/影响行增量。
    pub TableDeltaMap: HashMap<i64, TableDelta>,
    /// 缓存表 ID → 事务内 handle。
    pub CachedTables: HashMap<i64, Box<dyn Any + Send + Sync>>,
}

/// 单表统计增量：Delta 为净变化，Count 为影响行计数。
#[derive(Clone, Default)]
pub struct TableDelta {
    pub Delta: i64,
    pub Count: i64,
}

impl TransactionContext {
    /// 累加指定表的 Delta/Count。
    pub fn UpdateDeltaForTable(&mut self, table_id: i64, delta: i64, count: i64) {
        let item = self.TableDeltaMap.entry(table_id).or_default();
        item.Delta += delta;
        item.Count += count;
    }
}

/// A session temporary table is cloned as a shared handle, as Go interface
/// values are. Implementations must make clones observe the same modified and
/// dirty-size state.
/// 会话临时表：Clone 后共享同一 modified/脏大小状态（对齐 Go interface 语义）。
pub trait SessionTemporaryTable: tblctx::TemporaryTable + Clone + 'static {
    fn GetAutoIDAllocator(&self) -> Option<Arc<dyn autoid::Allocator>>;
    fn SetModified(&mut self, modified: bool);
    fn GetModified(&self) -> bool;
}

/// Narrow production boundary from `sessionctx.Context` and `SessionVars` used
/// by table mutation. The later module-integration task can implement this for
/// the concrete session without weakening the already migrated package APIs.
/// 表突变所需的会话窄边界（从 sessionctx / SessionVars 抽取）。
pub trait SessionContext {
    type ExprContext: exprctx::ExprContext + ?Sized;
    type InfoSchema: infoschema::MetaOnlyInfoSchema + ?Sized;
    type RowIDShardGenerator;
    type TemporaryTable: SessionTemporaryTable;

    fn GetExprCtx(&self) -> &Self::ExprContext;
    fn GetLatestInfoSchema(&self) -> &Self::InfoSchema;
    fn TakeWriteStmtBufs(&mut self) -> variable::WriteStmtBufs;
    fn ConnectionID(&self) -> u64;
    fn InRestrictedSQL(&self) -> bool;
    fn TxnAssertionLevel(&self) -> variable::AssertionLevel;
    fn EnableMutationChecker(&self) -> bool;
    fn EnableRowLevelChecksum(&self) -> bool;
    fn RowEncoderEnabled(&self) -> bool;
    fn GetRowIDShardGenerator(&mut self) -> &mut Self::RowIDShardGenerator;
    fn GetReservedRowIDAlloc(&mut self) -> Option<&mut stmtctx::ReservedRowIDAlloc>;
    fn HasTxnContext(&self) -> bool;
    fn GetTxnContextMut(&mut self) -> Option<&mut TransactionContext>;
    fn GetTemporaryTable(&self, tbl: &model::TableInfo) -> Option<Self::TemporaryTable>;
    fn TemporaryTableDataForHandler(&self) -> Option<Box<dyn tblctx::TemporaryTableData>>;
    fn GetTemporaryTableSizeLimit(&self) -> i64;
}

/// Provides the session-owned state required by table operations.
/// 持有会话与突变缓冲，实现 `tblctx::MutateContext`。
pub struct MutateContext<C: SessionContext> {
    /// 底层会话上下文。
    pub Context: C,
    /// 行编码/检查缓冲（由 TakeWriteStmtBufs 初始化）。
    mutateBuffers: tblctx::MutateBuffers,
}

/// 从会话取出 WriteStmtBufs 并构造 MutateContext。
pub fn NewMutateContext<C: SessionContext>(mut sctx: C) -> MutateContext<C> {
    let write_stmt_bufs = sctx.TakeWriteStmtBufs();
    MutateContext {
        Context: sctx,
        mutateBuffers: tblctx::NewMutateBuffers(write_stmt_bufs),
    }
}

impl<C: SessionContext> tblctx::AllocatorContext for MutateContext<C> {
    fn AlternativeAllocators(&mut self, tbl: &model::TableInfo) -> (autoid::Allocators, bool) {
        // 全局临时表若绑定了自增分配器，则作为替代 Allocators 返回。
        if tbl.TempTableType == model::TempTableGlobal
            && let Some(temp_table) = self.Context.GetTemporaryTable(tbl)
            && let Some(allocator) = temp_table.GetAutoIDAllocator()
        {
            return (autoid::Allocators::new(false, vec![allocator]), true);
        }
        (autoid::Allocators::default(), false)
    }
}

impl<C: SessionContext> tblctx::MutateContext for MutateContext<C> {
    type ExprContext = C::ExprContext;
    type RowIDShardGenerator = C::RowIDShardGenerator;
    type Statistics = Self;
    type CachedTables = Self;
    type TemporaryTables = Self;
    type ExchangePartitions = Self;

    fn GetExprCtx(&self) -> &Self::ExprContext {
        self.Context.GetExprCtx()
    }

    fn ConnectionID(&self) -> u64 {
        self.Context.ConnectionID()
    }

    fn InRestrictedSQL(&self) -> bool {
        self.Context.InRestrictedSQL()
    }

    fn TxnAssertionLevel(&self) -> variable::AssertionLevel {
        self.Context.TxnAssertionLevel()
    }

    fn EnableMutationChecker(&self) -> bool {
        self.Context.EnableMutationChecker()
    }

    fn GetRowEncodingConfig(&self) -> tblctx::RowEncodingConfig {
        // 行级校验和：需同时开启 checksum、RowEncoder，且非 restricted SQL。
        tblctx::RowEncodingConfig {
            IsRowLevelChecksumEnabled: self.Context.EnableRowLevelChecksum()
                && self.Context.RowEncoderEnabled()
                && !self.Context.InRestrictedSQL(),
            RowEncoder: Some(rowcodec::Encoder::new(self.Context.RowEncoderEnabled())),
        }
    }

    fn GetMutateBuffers(&mut self) -> &mut tblctx::MutateBuffers {
        &mut self.mutateBuffers
    }

    fn GetRowIDShardGenerator(&mut self) -> &mut Self::RowIDShardGenerator {
        self.Context.GetRowIDShardGenerator()
    }

    fn GetReservedRowIDAlloc(&mut self) -> (Option<&mut stmtctx::ReservedRowIDAlloc>, bool) {
        match self.Context.GetReservedRowIDAlloc() {
            Some(allocator) => (Some(allocator), true),
            None => {
                // Go asserts only under the `intest` build and returns safely in
                // production. Task 609 deliberately wires the no-assert build.
                // Go 仅在 intest 构建下断言；生产路径安全返回 (None, false)。
                #[cfg(feature = "intest")]
                intest::Assert(false, &[]);
                (None, false)
            }
        }
    }

    fn GetStatisticsSupport(&mut self) -> (Option<&mut Self::Statistics>, bool) {
        if self.Context.HasTxnContext() {
            (Some(self), true)
        } else {
            (None, false)
        }
    }

    fn GetCachedTableSupport(&mut self) -> (Option<&mut Self::CachedTables>, bool) {
        if self.Context.HasTxnContext() {
            (Some(self), true)
        } else {
            (None, false)
        }
    }

    fn GetTemporaryTableSupport(&mut self) -> (Option<&mut Self::TemporaryTables>, bool) {
        if self.Context.HasTxnContext() {
            (Some(self), true)
        } else {
            (None, false)
        }
    }

    fn GetExchangePartitionDMLSupport(&mut self) -> (Option<&mut Self::ExchangePartitions>, bool) {
        (Some(self), true)
    }
}

impl<C: SessionContext> tblctx::StatisticsSupport for MutateContext<C> {
    fn UpdatePhysicalTableDelta(&mut self, physical_table_id: i64, delta: i64, count: i64) {
        if let Some(txn_context) = self.Context.GetTxnContextMut() {
            txn_context.UpdateDeltaForTable(physical_table_id, delta, count);
        }
    }
}

impl<C: SessionContext> tblctx::CachedTableSupport for MutateContext<C> {
    fn AddCachedTableHandleToTxn(&mut self, table_id: i64, handle: Box<dyn Any + Send + Sync>) {
        // 仅在尚无 handle 时插入，避免覆盖已有缓存表句柄。
        if let Some(txn_context) = self.Context.GetTxnContextMut() {
            txn_context.CachedTables.entry(table_id).or_insert(handle);
        }
    }
}

impl<C: SessionContext> tblctx::TemporaryTableSupport for MutateContext<C> {
    fn GetTemporaryTableSizeLimit(&self) -> i64 {
        self.Context.GetTemporaryTableSizeLimit()
    }

    fn AddTemporaryTableToTxn(
        &mut self,
        tbl_info: &model::TableInfo,
    ) -> (Option<tblctx::TemporaryTableHandler>, bool) {
        let Some(mut table) = self.Context.GetTemporaryTable(tbl_info) else {
            return (None, false);
        };
        // 加入事务时标记 modified，并附带已提交大小数据源。
        table.SetModified(true);
        let data = self.Context.TemporaryTableDataForHandler();
        (Some(tblctx::NewTemporaryTableHandler(table, data)), true)
    }
}

impl<C: SessionContext> tblctx::ExchangePartitionDMLSupport for MutateContext<C> {
    type InfoSchema = C::InfoSchema;

    fn GetInfoSchemaToCheckExchangeConstraint(&self) -> &Self::InfoSchema {
        self.Context.GetLatestInfoSchema()
    }
}

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

// 表突变上下文接口与临时表处理器。
//
// 定义 `MutateContext` / `AllocatorContext` 等 trait，以及行编码配置、
// 统计增量、缓存表、临时表与交换分区 DML 等可选支持边界。

use crate::{autoid, exprctx, infoschema, model, rowcodec, stmtctx, tableutil, variable};

use super::MutateBuffers;

/// Configuration used while encoding a table row.
/// 编码表行时使用的配置。
pub struct RowEncodingConfig {
    /// 是否启用行级校验和（row-level checksum）。
    pub IsRowLevelChecksumEnabled: bool,
    /// 行编码器；生产路径通常始终提供。
    pub RowEncoder: Option<rowcodec::Encoder>,
}

/// 物理表变更统计增量支持。
pub trait StatisticsSupport {
    fn UpdatePhysicalTableDelta(&mut self, physicalTableID: i64, delta: i64, count: i64);
}

/// 将缓存表 handle 挂入当前事务。
pub trait CachedTableSupport {
    fn AddCachedTableHandleToTxn(
        &mut self,
        tableID: i64,
        handle: Box<dyn std::any::Any + Send + Sync>,
    );
}

/// The part of `tableutil::TempTable` needed by the handler.
///
/// The blanket implementation keeps the production boundary connected to the
/// integrated tableutil crate while allowing focused tests to use a small mock.
/// 临时表在事务内的元数据与脏大小接口；blanket impl 对接生产 TempTable。
pub trait TemporaryTable: Send + Sync {
    fn GetMeta(&self) -> &model::TableInfo;
    fn GetSize(&self) -> i64;
    fn SetSize(&mut self, size: i64);
}

impl<T: tableutil::TempTable + ?Sized> TemporaryTable for T {
    fn GetMeta(&self) -> &model::TableInfo {
        tableutil::TempTable::GetMeta(self)
    }

    fn GetSize(&self) -> i64 {
        tableutil::TempTable::GetSize(self)
    }

    fn SetSize(&mut self, size: i64) {
        tableutil::TempTable::SetSize(self, size);
    }
}

/// Read-only committed-size boundary from session temporary-table data.
/// 会话临时表已提交大小的只读查询边界。
pub trait TemporaryTableData {
    fn GetTableSize(&self, tableID: i64) -> i64;
}

impl<T: variable::TemporaryTableData + ?Sized> TemporaryTableData for T {
    fn GetTableSize(&self, tableID: i64) -> i64 {
        variable::TemporaryTableData::GetTableSize(self, tableID)
    }
}

/// 临时表处理器：组合事务内临时表与可选的已提交大小数据源。
pub struct TemporaryTableHandler {
    /// 事务内临时表实例。
    tblInTxn: Box<dyn TemporaryTable>,
    /// 会话级已提交大小数据；缺失时已提交大小视为 0。
    data: Option<Box<dyn TemporaryTableData>>,
}

/// 构造临时表处理器。
pub fn NewTemporaryTableHandler<T>(
    tbl: T,
    data: Option<Box<dyn TemporaryTableData>>,
) -> TemporaryTableHandler
where
    T: TemporaryTable + 'static,
{
    TemporaryTableHandler {
        tblInTxn: Box::new(tbl),
        data,
    }
}

impl TemporaryTableHandler {
    /// 返回临时表元数据。
    pub fn Meta(&self) -> &model::TableInfo {
        self.tblInTxn.GetMeta()
    }

    /// 事务内未提交的脏大小（dirty size）。
    pub fn GetDirtySize(&self) -> i64 {
        self.tblInTxn.GetSize()
    }

    /// 已提交大小；无 data 时返回 0。
    pub fn GetCommittedSize(&self) -> i64 {
        self.data
            .as_ref()
            .map(|data| data.GetTableSize(self.tblInTxn.GetMeta().ID))
            .unwrap_or(0)
    }

    /// 按 delta 调整事务内脏大小。
    pub fn UpdateTxnDeltaSize(&mut self, delta: i64) {
        self.tblInTxn.SetSize(self.tblInTxn.GetSize() + delta);
    }
}

/// 临时表支持：大小限制与将临时表加入事务。
pub trait TemporaryTableSupport {
    fn GetTemporaryTableSizeLimit(&self) -> i64;
    fn AddTemporaryTableToTxn(
        &mut self,
        tblInfo: &model::TableInfo,
    ) -> (Option<TemporaryTableHandler>, bool);
}

/// 交换分区（exchange partition）DML 约束检查所需 infoschema。
pub trait ExchangePartitionDMLSupport {
    type InfoSchema: infoschema::MetaOnlyInfoSchema + ?Sized;

    fn GetInfoSchemaToCheckExchangeConstraint(&self) -> &Self::InfoSchema;
}

/// Context used by table mutation paths.
///
/// Associated types express Go interface return values without erasing their
/// concrete implementations or inventing placeholder dependency types.
/// 表突变路径使用的上下文；关联类型对应 Go 接口返回值的具体实现。
pub trait MutateContext: AllocatorContext {
    type ExprContext: exprctx::ExprContext + ?Sized;
    type RowIDShardGenerator;
    type Statistics: StatisticsSupport + ?Sized;
    type CachedTables: CachedTableSupport + ?Sized;
    type TemporaryTables: TemporaryTableSupport + ?Sized;
    type ExchangePartitions: ExchangePartitionDMLSupport + ?Sized;

    fn GetExprCtx(&self) -> &Self::ExprContext;
    fn ConnectionID(&self) -> u64;
    fn InRestrictedSQL(&self) -> bool;
    fn TxnAssertionLevel(&self) -> variable::AssertionLevel;
    fn EnableMutationChecker(&self) -> bool;
    fn GetRowEncodingConfig(&self) -> RowEncodingConfig;
    fn GetMutateBuffers(&mut self) -> &mut MutateBuffers;
    fn GetRowIDShardGenerator(&mut self) -> &mut Self::RowIDShardGenerator;
    fn GetReservedRowIDAlloc(&mut self) -> (Option<&mut stmtctx::ReservedRowIDAlloc>, bool);
    fn GetStatisticsSupport(&mut self) -> (Option<&mut Self::Statistics>, bool);
    fn GetCachedTableSupport(&mut self) -> (Option<&mut Self::CachedTables>, bool);
    fn GetTemporaryTableSupport(&mut self) -> (Option<&mut Self::TemporaryTables>, bool);
    fn GetExchangePartitionDMLSupport(&mut self) -> (Option<&mut Self::ExchangePartitions>, bool);
}

/// 自增/行 ID 分配器上下文：可为全局临时表提供替代分配器。
pub trait AllocatorContext {
    fn AlternativeAllocators(&mut self, tbl: &model::TableInfo) -> (autoid::Allocators, bool);
}

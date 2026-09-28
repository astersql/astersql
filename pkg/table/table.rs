// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Copyright 2013 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// Executable table contracts corresponding to `pkg/table/table.go`.
//
// Go interface values are represented by borrowed trait objects or `Arc`s.
// The object-safe mutation facade below delegates to the formal `tblctx`
// traits; it does not replace any table behavior or state.
//
// 对应 Go `pkg/table/table.go` 的可执行表契约。
//
// Go 接口以借用 trait 对象或 `Arc` 表示；下方 object-safe 变更门面委托给
// 正式 `tblctx` trait，不替代任何表行为或状态。表类型涵盖普通表、虚拟表、
// 集群表，以及分区表、缓存表等扩展；行变更通过 Add/Update/RemoveRecord
// 选项对象配置重复键检查、自增预留等语义。

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;

use chunk_dependency::Row;
use errors_dependency::SharedError;
use kv_dependency::{Handle, Key, MemBuffer, Storage, Transaction};
use model_dependency::group_4 as table_model;
use types_dependency::datum::Datum;

use crate::{
    Column, Constraint, CreateIdxOpt, CreateIdxOption, DupKeyCheckMode, Index, IndexMutateContext,
    PessimisticLazyDupKeyCheckMode, commonMutateOpt,
};

/// 表操作统一结果类型，错误为共享 `SharedError`。
pub type TableResult<T> = Result<T, SharedError>;

/// Distinguishes tables backed by persisted data from virtual table kinds.
///
/// 区分落盘普通表与虚拟表/集群表等表种类（iota 值与 Go 对齐）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(i16)]
pub enum Type {
    /// 普通持久化表。
    #[default]
    NormalTable = 0,
    /// 虚拟表（无独立持久化存储）。
    VirtualTable = 1,
    /// 集群信息类表。
    ClusterTable = 2,
}

impl Type {
    /// 是否为普通持久化表。
    pub fn IsNormalTable(self) -> bool {
        self == Self::NormalTable
    }

    /// 是否为虚拟表。
    pub fn IsVirtualTable(self) -> bool {
        self == Self::VirtualTable
    }

    /// 是否为集群表。
    pub fn IsClusterTable(self) -> bool {
        self == Self::ClusterTable
    }
}

/// terror 标准错误的堆分配别名，供 `table_error!` 生成原型。
type StandardError = Box<dbterror_dependency::terror::Error>;

/// 按错误类与 errno 惰性构造与 Go 对齐的表错误原型静态量。
macro_rules! table_error {
    ($name:ident, $class:ident, $code:ident) => {
        pub static $name: LazyLock<StandardError> =
            LazyLock::new(|| dbterror_dependency::$class.NewStd(dbterror_dependency::errno::$code));
    };
}

table_error!(ErrColumnCantNull, ClassTable, ErrBadNull);
table_error!(ErrUnknownColumn, ClassTable, ErrBadField);
table_error!(errDuplicateColumn, ClassTable, ErrFieldSpecifiedTwice);
table_error!(ErrWarnNullToNotnull, ClassExecutor, ErrWarnNullToNotnull);
table_error!(errGetDefaultFailed, ClassTable, ErrFieldGetDefaultFailed);
table_error!(ErrNoDefaultValue, ClassTable, ErrNoDefaultForField);
table_error!(ErrIndexOutBound, ClassTable, ErrIndexOutBound);
table_error!(ErrUnsupportedOp, ClassTable, ErrUnsupportedOp);
table_error!(ErrRowNotFound, ClassTable, ErrRowNotFound);
table_error!(ErrTableStateCantNone, ClassTable, ErrTableStateCantNone);
table_error!(ErrColumnStateCantNone, ClassTable, ErrColumnStateCantNone);
table_error!(ErrColumnStateNonPublic, ClassTable, ErrColumnStateNonPublic);
table_error!(ErrIndexStateCantNone, ClassTable, ErrIndexStateCantNone);
table_error!(ErrInvalidRecordKey, ClassTable, ErrInvalidRecordKey);
table_error!(
    ErrTruncatedWrongValueForField,
    ClassTable,
    ErrTruncatedWrongValueForField
);
table_error!(ErrUnknownPartition, ClassTable, ErrUnknownPartition);
table_error!(
    ErrNoPartitionForGivenValue,
    ClassTable,
    ErrNoPartitionForGivenValue
);
table_error!(
    ErrLockOrActiveTransaction,
    ClassTable,
    ErrLockOrActiveTransaction
);
table_error!(ErrSequenceHasRunOut, ClassTable, ErrSequenceRunOut);
table_error!(
    ErrRowDoesNotMatchGivenPartitionSet,
    ClassTable,
    ErrRowDoesNotMatchGivenPartitionSet
);
table_error!(ErrTempTableFull, ClassTable, ErrRecordFileFull);
table_error!(ErrOptOnCacheTable, ClassDDL, ErrOptOnCacheTable);
table_error!(
    ErrCheckConstraintViolated,
    ClassTable,
    ErrCheckConstraintViolated
);

/// Low-level record iterator. Returning `Ok(false)` stops iteration.
///
/// 底层记录迭代回调；返回 `Ok(false)` 停止迭代。
pub type RecordIterFunc =
    dyn FnMut(Box<dyn Handle>, Vec<Datum>, Vec<Arc<Column>>) -> TableResult<bool>;

/// 插入（AddRecord）时的可变选项集合。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AddRecordOpt {
    /// 公共变更选项（上下文、重复键检查等）。
    pub(crate) commonMutateOpt: commonMutateOpt,
    /// 是否处于更新路径派生的插入（如更新时回写）。
    isUpdate: bool,
    /// 是否生成新的记录 ID（handle）。
    genRecordID: bool,
    /// 预留自增 ID 数量提示。
    reserveAutoID: i32,
}

/// 按选项列表累积构造 `AddRecordOpt`。
pub fn NewAddRecordOpt(options: &[&dyn AddRecordOption]) -> AddRecordOpt {
    let mut option = AddRecordOpt::default();
    for apply in options {
        apply.applyAddRecordOpt(&mut option);
    }
    option
}

impl AddRecordOpt {
    /// 可选的 KV 操作上下文。
    pub fn Ctx(&self) -> Option<kv_dependency::Context> {
        self.commonMutateOpt.Ctx()
    }

    /// 重复键检查模式。
    pub fn DupKeyCheck(&self) -> DupKeyCheckMode {
        self.commonMutateOpt.DupKeyCheck()
    }

    /// 悲观事务惰性重复键检查模式。
    pub fn PessimisticLazyDupKeyCheck(&self) -> PessimisticLazyDupKeyCheckMode {
        self.commonMutateOpt.PessimisticLazyDupKeyCheck()
    }

    /// 是否标记为更新路径上的插入。
    pub fn IsUpdate(&self) -> bool {
        self.isUpdate
    }

    /// 是否需要生成记录 ID。
    pub fn GenerateRecordID(&self) -> bool {
        self.genRecordID
    }

    /// 预留自增 ID 数量。
    pub fn ReserveAutoID(&self) -> i32 {
        self.reserveAutoID
    }

    /// 从公共变更选项派生创建索引选项。
    pub fn GetCreateIdxOpt(&self) -> CreateIdxOpt {
        CreateIdxOpt::from_common(self.commonMutateOpt.clone())
    }
}

/// 可应用到 `AddRecordOpt` 的选项 trait。
pub trait AddRecordOption {
    /// 将本选项写入目标 `AddRecordOpt`。
    fn applyAddRecordOpt(&self, option: &mut AddRecordOpt);
}

/// 更新（UpdateRecord）时的可变选项集合。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UpdateRecordOpt {
    /// 公共变更选项。
    pub(crate) commonMutateOpt: commonMutateOpt,
    /// 是否跳过未触及索引的写回。
    skipWriteUntouchedIndices: bool,
}

/// 按选项列表累积构造 `UpdateRecordOpt`。
pub fn NewUpdateRecordOpt(options: &[&dyn UpdateRecordOption]) -> UpdateRecordOpt {
    let mut option = UpdateRecordOpt::default();
    for apply in options {
        apply.applyUpdateRecordOpt(&mut option);
    }
    option
}

impl UpdateRecordOpt {
    /// 可选的 KV 操作上下文。
    pub fn Ctx(&self) -> Option<kv_dependency::Context> {
        self.commonMutateOpt.Ctx()
    }

    /// 重复键检查模式。
    pub fn DupKeyCheck(&self) -> DupKeyCheckMode {
        self.commonMutateOpt.DupKeyCheck()
    }

    /// 悲观事务惰性重复键检查模式。
    pub fn PessimisticLazyDupKeyCheck(&self) -> PessimisticLazyDupKeyCheckMode {
        self.commonMutateOpt.PessimisticLazyDupKeyCheck()
    }

    /// 是否跳过未触及索引的写回。
    pub fn SkipWriteUntouchedIndices(&self) -> bool {
        self.skipWriteUntouchedIndices
    }

    /// 派生「生成记录 ID」的 AddRecord 选项视图。
    pub fn GetAddRecordOpt(&self) -> AddRecordOpt {
        AddRecordOpt {
            commonMutateOpt: self.commonMutateOpt.clone(),
            isUpdate: true,
            genRecordID: true,
            reserveAutoID: 0,
        }
    }

    /// 派生「保留原记录 ID」的 AddRecord 选项视图。
    pub fn GetAddRecordOptKeepRecordID(&self) -> AddRecordOpt {
        AddRecordOpt {
            commonMutateOpt: self.commonMutateOpt.clone(),
            isUpdate: true,
            genRecordID: false,
            reserveAutoID: 0,
        }
    }

    /// 从公共变更选项派生创建索引选项。
    pub fn GetCreateIdxOpt(&self) -> CreateIdxOpt {
        CreateIdxOpt::from_common(self.commonMutateOpt.clone())
    }
}

/// 可应用到 `UpdateRecordOpt` 的选项 trait。
pub trait UpdateRecordOption {
    /// 将本选项写入目标 `UpdateRecordOpt`。
    fn applyUpdateRecordOpt(&self, option: &mut UpdateRecordOpt);
}

/// 删除（RemoveRecord）时的可变选项集合。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RemoveRecordOpt {
    /// 可选的各索引行布局（列偏移顺序）。
    indexesLayoutOffset: Option<IndexesLayout>,
}

impl RemoveRecordOpt {
    /// 是否携带索引布局信息。
    pub fn HasIndexesLayout(&self) -> bool {
        self.indexesLayoutOffset.is_some()
    }

    /// 取得完整索引布局映射。
    pub fn GetIndexesLayout(&self) -> Option<&IndexesLayout> {
        self.indexesLayoutOffset.as_ref()
    }

    /// 按索引 ID 取得单索引列布局。
    pub fn GetIndexLayout(&self, indexID: i64) -> Option<&IndexRowLayoutOption> {
        self.indexesLayoutOffset
            .as_ref()
            .and_then(|layout| layout.GetIndexLayout(indexID))
    }
}

/// 按选项列表累积构造 `RemoveRecordOpt`。
pub fn NewRemoveRecordOpt(options: &[&dyn RemoveRecordOption]) -> RemoveRecordOpt {
    let mut option = RemoveRecordOpt::default();
    for apply in options {
        apply.applyRemoveRecordOpt(&mut option);
    }
    option
}

/// 可应用到 `RemoveRecordOpt` 的选项 trait。
pub trait RemoveRecordOption {
    /// 将本选项写入目标 `RemoveRecordOpt`。
    fn applyRemoveRecordOpt(&self, option: &mut RemoveRecordOpt);
}

/// 单个索引行内列偏移顺序。
pub type IndexRowLayoutOption = Vec<i32>;

/// 索引 ID 到行布局的映射，本身也可作为删除选项。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexesLayout(pub HashMap<i64, IndexRowLayoutOption>);

impl IndexesLayout {
    /// 按索引 ID 查询列布局。
    pub fn GetIndexLayout(&self, indexID: i64) -> Option<&IndexRowLayoutOption> {
        self.0.get(&indexID)
    }
}

impl RemoveRecordOption for IndexesLayout {
    fn applyRemoveRecordOpt(&self, option: &mut RemoveRecordOpt) {
        option.indexesLayoutOffset = Some(self.clone());
    }
}

/// 以闭包形式改写公共变更选项的通用选项适配器。
#[derive(Clone)]
pub struct CommonMutateOptFunc(Arc<dyn Fn(&mut commonMutateOpt) + Send + Sync>);

impl CommonMutateOptFunc {
    /// 由闭包构造适配器。
    pub fn new(apply: impl Fn(&mut commonMutateOpt) + Send + Sync + 'static) -> Self {
        Self(Arc::new(apply))
    }
}

impl AddRecordOption for CommonMutateOptFunc {
    fn applyAddRecordOpt(&self, option: &mut AddRecordOpt) {
        (self.0)(&mut option.commonMutateOpt);
    }
}

impl UpdateRecordOption for CommonMutateOptFunc {
    fn applyUpdateRecordOpt(&self, option: &mut UpdateRecordOpt) {
        (self.0)(&mut option.commonMutateOpt);
    }
}

impl CreateIdxOption for CommonMutateOptFunc {
    fn applyCreateIdxOpt(&self, option: &mut CreateIdxOpt) {
        (self.0)(option.common_mutate_opt_mut());
    }
}

/// 选项：注入 KV 操作上下文。
pub fn WithCtx(context: kv_dependency::Context) -> CommonMutateOptFunc {
    CommonMutateOptFunc::new(move |option| option.ctx = Some(context.clone()))
}

/// 选项：预留自增 ID 数量提示。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WithReserveAutoIDHint(pub i32);

impl AddRecordOption for WithReserveAutoIDHint {
    fn applyAddRecordOpt(&self, option: &mut AddRecordOpt) {
        option.reserveAutoID = self.0;
    }
}

/// 选项标记类型：将 AddRecord 标为更新路径并生成记录 ID。
#[derive(Clone, Copy, Debug, Default)]
pub struct isUpdate;

impl AddRecordOption for isUpdate {
    fn applyAddRecordOpt(&self, option: &mut AddRecordOpt) {
        option.isUpdate = true;
        option.genRecordID = true;
    }
}

/// 预置的 `isUpdate` 选项实例。
pub static IsUpdate: isUpdate = isUpdate;

/// 选项标记类型：更新时跳过未触及索引写回。
#[derive(Clone, Copy, Debug, Default)]
pub struct skipWriteUntouchedIndices;

impl UpdateRecordOption for skipWriteUntouchedIndices {
    fn applyUpdateRecordOpt(&self, option: &mut UpdateRecordOpt) {
        option.skipWriteUntouchedIndices = true;
    }
}

/// 预置的 `skipWriteUntouchedIndices` 选项实例。
pub static SkipWriteUntouchedIndices: skipWriteUntouchedIndices = skipWriteUntouchedIndices;

impl AddRecordOption for DupKeyCheckMode {
    fn applyAddRecordOpt(&self, option: &mut AddRecordOpt) {
        option.commonMutateOpt.dupKeyCheck = *self;
    }
}

impl UpdateRecordOption for DupKeyCheckMode {
    fn applyUpdateRecordOpt(&self, option: &mut UpdateRecordOpt) {
        option.commonMutateOpt.dupKeyCheck = *self;
    }
}

impl AddRecordOption for PessimisticLazyDupKeyCheckMode {
    fn applyAddRecordOpt(&self, option: &mut AddRecordOpt) {
        option.commonMutateOpt.pessimisticLazyDupKeyCheck = *self;
    }
}

impl UpdateRecordOption for PessimisticLazyDupKeyCheckMode {
    fn applyUpdateRecordOpt(&self, option: &mut UpdateRecordOpt) {
        option.commonMutateOpt.pessimisticLazyDupKeyCheck = *self;
    }
}

/// 表列集合访问 API（可见/隐藏/可写/可删等视图）。
pub trait columnAPI {
    /// 返回表的列列表（通常为公共列）。
    fn Cols(&self) -> Vec<Arc<Column>>;
    /// 对用户可见的列。
    fn VisibleCols(&self) -> Vec<Arc<Column>>;
    /// 隐藏列。
    fn HiddenCols(&self) -> Vec<Arc<Column>>;
    /// 写入路径可写的列。
    fn WritableCols(&self) -> Vec<Arc<Column>>;
    /// 删除路径需要的列。
    fn DeletableCols(&self) -> Vec<Arc<Column>>;
    /// 隐藏列与可见列的并集视图。
    fn FullHiddenColsAndVisibleCols(&self) -> Vec<Arc<Column>>;
}

/// Behavior required from the session-owned row-ID shard generator.
///
/// 会话持有的行 ID 分片生成器所需行为。
pub trait RowIDShardGenerator {
    /// 取得当前分片基值，供分配 `count` 个行 ID。
    fn GetCurrentShard(&mut self, count: usize) -> i64;
}

/// Object-safe exchange-partition view of the formal associated-type trait.
///
/// 交换分区（exchange partition）DML 所需的 object-safe 视图。
pub trait ExchangePartitionDMLSupport {
    /// Returns the concrete InfoSchema interface value for Go-style dynamic
    /// type assertion by the exchange-partition implementation.
    ///
    /// 返回用于交换分区约束检查的 InfoSchema 接口值。
    fn GetInfoSchemaToCheckExchangeConstraint(&self) -> &dyn Any;
}

impl<T> ExchangePartitionDMLSupport for T
where
    T: tblctx_dependency::ExchangePartitionDMLSupport,
    T::InfoSchema: Any + Sized,
{
    fn GetInfoSchemaToCheckExchangeConstraint(&self) -> &dyn Any {
        tblctx_dependency::ExchangePartitionDMLSupport::GetInfoSchemaToCheckExchangeConstraint(self)
    }
}

/// Object-safe form of the complete formal table mutation context.
///
/// 完整表变更上下文的 object-safe 形式，委托正式 `tblctx::MutateContext`。
pub trait MutateContext: IndexMutateContext + tblctx_dependency::AllocatorContext {
    /// 是否处于受限 SQL（内部/系统语句）路径。
    fn InRestrictedSQL(&self) -> bool;
    /// 事务断言级别（控制键断言严格程度）。
    fn TxnAssertionLevel(&self) -> tblctx_dependency::variable::AssertionLevel;
    /// 是否启用变更一致性检查器。
    fn EnableMutationChecker(&self) -> bool;
    /// 行编码配置（新旧行格式等）。
    fn GetRowEncodingConfig(&self) -> tblctx_dependency::RowEncodingConfig;
    /// 行 ID 分片生成器。
    fn GetRowIDShardGenerator(&mut self) -> &mut dyn RowIDShardGenerator;
    /// 预留行 ID 分配器；第二返回值表示是否可用。
    fn GetReservedRowIDAlloc(
        &mut self,
    ) -> (
        Option<&mut tblctx_dependency::stmtctx::ReservedRowIDAlloc>,
        bool,
    );
    /// 可选的统计信息支持；第二返回值表示是否可用。
    fn GetStatisticsSupport(
        &mut self,
    ) -> (Option<&mut dyn tblctx_dependency::StatisticsSupport>, bool);
    /// 可选的缓存表支持。
    fn GetCachedTableSupport(
        &mut self,
    ) -> (Option<&mut dyn tblctx_dependency::CachedTableSupport>, bool);
    /// 可选的临时表支持。
    fn GetTemporaryTableSupport(
        &mut self,
    ) -> (
        Option<&mut dyn tblctx_dependency::TemporaryTableSupport>,
        bool,
    );
    /// 可选的交换分区 DML 支持。
    fn GetExchangePartitionDMLSupport(
        &mut self,
    ) -> (Option<&mut dyn ExchangePartitionDMLSupport>, bool);
}

impl<T> MutateContext for T
where
    T: tblctx_dependency::MutateContext + IndexMutateContext,
    T::RowIDShardGenerator: RowIDShardGenerator + Sized,
    T::Statistics: Sized,
    T::CachedTables: Sized,
    T::TemporaryTables: Sized,
    T::ExchangePartitions: ExchangePartitionDMLSupport + Sized,
{
    fn InRestrictedSQL(&self) -> bool {
        tblctx_dependency::MutateContext::InRestrictedSQL(self)
    }

    fn TxnAssertionLevel(&self) -> tblctx_dependency::variable::AssertionLevel {
        tblctx_dependency::MutateContext::TxnAssertionLevel(self)
    }

    fn EnableMutationChecker(&self) -> bool {
        tblctx_dependency::MutateContext::EnableMutationChecker(self)
    }

    fn GetRowEncodingConfig(&self) -> tblctx_dependency::RowEncodingConfig {
        tblctx_dependency::MutateContext::GetRowEncodingConfig(self)
    }

    fn GetRowIDShardGenerator(&mut self) -> &mut dyn RowIDShardGenerator {
        tblctx_dependency::MutateContext::GetRowIDShardGenerator(self)
    }

    fn GetReservedRowIDAlloc(
        &mut self,
    ) -> (
        Option<&mut tblctx_dependency::stmtctx::ReservedRowIDAlloc>,
        bool,
    ) {
        tblctx_dependency::MutateContext::GetReservedRowIDAlloc(self)
    }

    fn GetStatisticsSupport(
        &mut self,
    ) -> (Option<&mut dyn tblctx_dependency::StatisticsSupport>, bool) {
        let (support, ok) = tblctx_dependency::MutateContext::GetStatisticsSupport(self);
        (
            support.map(|value| value as &mut dyn tblctx_dependency::StatisticsSupport),
            ok,
        )
    }

    fn GetCachedTableSupport(
        &mut self,
    ) -> (Option<&mut dyn tblctx_dependency::CachedTableSupport>, bool) {
        let (support, ok) = tblctx_dependency::MutateContext::GetCachedTableSupport(self);
        (
            support.map(|value| value as &mut dyn tblctx_dependency::CachedTableSupport),
            ok,
        )
    }

    fn GetTemporaryTableSupport(
        &mut self,
    ) -> (
        Option<&mut dyn tblctx_dependency::TemporaryTableSupport>,
        bool,
    ) {
        let (support, ok) = tblctx_dependency::MutateContext::GetTemporaryTableSupport(self);
        (
            support.map(|value| value as &mut dyn tblctx_dependency::TemporaryTableSupport),
            ok,
        )
    }

    fn GetExchangePartitionDMLSupport(
        &mut self,
    ) -> (Option<&mut dyn ExchangePartitionDMLSupport>, bool) {
        let (support, ok) = tblctx_dependency::MutateContext::GetExchangePartitionDMLSupport(self);
        (
            support.map(|value| value as &mut dyn ExchangePartitionDMLSupport),
            ok,
        )
    }
}

/// 再导出分配器上下文 trait。
pub use tblctx_dependency::AllocatorContext;

/// Retrieves and mutates rows in a table.
///
/// 表的核心契约：检索与变更行数据、索引、约束与元信息。
pub trait Table: columnAPI {
    /// 表上全部索引。
    fn Indices(&self) -> Vec<Arc<dyn Index>>;
    /// 删除路径需要维护的索引。
    fn DeletableIndices(&self) -> Vec<Arc<dyn Index>>;
    /// 可写状态下的 CHECK 约束列表。
    fn WritableConstraint(&self) -> Vec<Arc<Constraint>>;
    /// 记录键前缀。
    fn RecordPrefix(&self) -> Key;
    /// 索引键前缀。
    fn IndexPrefix(&self) -> Key;

    /// 插入一行并返回 handle（行标识）。
    fn AddRecord(
        &self,
        context: &mut dyn MutateContext,
        transaction: &mut dyn Transaction,
        row: &[Datum],
        options: &[&dyn AddRecordOption],
    ) -> TableResult<Box<dyn Handle>>;

    /// 按 handle 更新一行；`touched` 标记哪些列被修改。
    fn UpdateRecord(
        &self,
        context: &mut dyn MutateContext,
        transaction: &mut dyn Transaction,
        handle: &dyn Handle,
        current_data: &[Datum],
        new_data: &[Datum],
        touched: &[bool],
        options: &[&dyn UpdateRecordOption],
    ) -> TableResult<()>;

    /// 按 handle 删除一行。
    fn RemoveRecord(
        &self,
        context: &mut dyn MutateContext,
        transaction: &mut dyn Transaction,
        handle: &dyn Handle,
        row: &[Datum],
        options: &[&dyn RemoveRecordOption],
    ) -> TableResult<()>;

    /// 取得自增/序列等分配器集合。
    fn Allocators(&self, context: &mut dyn AllocatorContext) -> autoid_dependency::Allocators;
    /// 表元信息。
    fn Meta(&self) -> &table_model::TableInfo;
    /// 是否使用新校对规则。
    fn UseNewCollate(&self) -> bool;
    /// 表种类。
    fn Type(&self) -> Type;
    /// 若为分区表则返回分区表视图。
    fn GetPartitionedTable(&self) -> Option<&dyn PartitionedTable>;
}

/// Formal session boundary used by the two auto-increment helpers.
///
/// 自增分配辅助函数所需的会话边界。
pub trait AutoIncrementContext {
    /// `AUTO_INCREMENT` 步长。
    fn AutoIncrementIncrement(&self) -> i64;
    /// `AUTO_INCREMENT` 偏移。
    fn AutoIncrementOffset(&self) -> i64;
    /// 取得表分配器上下文。
    fn GetTableCtx(&mut self) -> &mut dyn AllocatorContext;
}

/// 读取步长与偏移；偏移大于步长时回退为 1（对齐 Go）。
fn getIncrementAndOffset(context: &dyn AutoIncrementContext) -> (i64, i64) {
    let increment = context.AutoIncrementIncrement();
    let mut offset = context.AutoIncrementOffset();
    if offset > increment {
        offset = 1;
    }
    (increment, offset)
}

/// 从表分配器集合中取出 AutoIncrement 分配器。
fn auto_increment_allocator(
    table: &dyn Table,
    context: &mut dyn AutoIncrementContext,
) -> TableResult<Arc<dyn autoid_dependency::Allocator>> {
    table
        .Allocators(context.GetTableCtx())
        .get(autoid_dependency::AllocatorType::AutoIncrement)
        .ok_or_else(|| errors_dependency::New("auto_increment allocator is missing"))
}

/// 分配单个自增值，返回区间上界（与 Go 一致）。
pub fn AllocAutoIncrementValue(
    context: &autoid_dependency::Context,
    table: &dyn Table,
    session: &mut dyn AutoIncrementContext,
) -> TableResult<i64> {
    let (increment, offset) = getIncrementAndOffset(session);
    let allocator = auto_increment_allocator(table, session)?;
    allocator
        .alloc(context, 1, increment, offset)
        .map(|(_, maximum)| maximum)
        .map_err(SharedError::new)
}

/// 批量分配自增值，返回（首个可用 ID，步长）。
pub fn AllocBatchAutoIncrementValue(
    context: &autoid_dependency::Context,
    table: &dyn Table,
    session: &mut dyn AutoIncrementContext,
    count: usize,
) -> TableResult<(i64, i64)> {
    let (increment, offset) = getIncrementAndOffset(session);
    let allocator = auto_increment_allocator(table, session)?;
    let (minimum, _) = allocator
        .alloc(context, count as u64, increment, offset)
        .map_err(SharedError::new)?;
    let first = autoid_dependency::seek_to_first_auto_id_unsigned(
        minimum as u64,
        increment as u64,
        offset as u64,
    ) as i64;
    Ok((first, increment))
}

/// 具有物理 ID 的表（如分区的具体分片）。
pub trait PhysicalTable: Table {
    /// 物理表 ID（TiKV 键空间中的表 ID）。
    fn GetPhysicalID(&self) -> i64;
}

/// 分区表：按物理 ID 或行值路由到具体分区。
pub trait PartitionedTable: Table {
    /// 按物理 ID 取得分区。
    fn GetPartition(&self, physicalID: i64) -> Option<&dyn PhysicalTable>;
    /// 按行数据计算所属分区。
    fn GetPartitionByRow(
        &self,
        context: &dyn expression_dependency::EvalContext,
        row: &[Datum],
    ) -> TableResult<&dyn PhysicalTable>;
    /// 按行数据计算分区下标。
    fn GetPartitionIdxByRow(
        &self,
        context: &dyn expression_dependency::EvalContext,
        row: &[Datum],
    ) -> TableResult<i32>;
    /// 全部物理分区 ID。
    fn GetAllPartitionIDs(&self) -> Vec<i64>;
    /// 分区键列 ID 列表。
    fn GetPartitionColumnIDs(&self) -> Vec<i64>;
    /// 分区键列名列表。
    fn GetPartitionColumnNames(&self) -> Vec<table_model::ast::CIStr>;
    /// 交换分区前校验行是否满足目标分区约束。
    fn CheckForExchangePartition(
        &self,
        context: &dyn expression_dependency::EvalContext,
        partition_info: &table_model::PartitionInfo,
        row: &[Datum],
        partition_id: i64,
        table_id: i64,
    ) -> TableResult<()>;
}

/// 生产环境：由分配器与表元信息构造表实例的工厂函数类型。
pub type TableFromMetaFn =
    fn(autoid_dependency::Allocators, &table_model::TableInfo) -> TableResult<Box<dyn Table>>;
/// 测试/规划器：仅由表元信息构造表实例的 mock 工厂类型。
pub type MockTableFromMetaFn = fn(&table_model::TableInfo) -> Box<dyn Table>;
/// 分配器集合别名。
pub type AllocatorCollection = autoid_dependency::Allocators;

/// 可安装的生产表工厂槽位。
pub static TableFromMeta: RwLock<Option<TableFromMetaFn>> = RwLock::new(None);
/// 可安装的 mock 表工厂槽位。
pub static MockTableFromMeta: RwLock<Option<MockTableFromMetaFn>> = RwLock::new(None);

/// Builds a planner/executor table through the installed production factory,
/// falling back to the installed mock factory. Keeping allocator construction
/// in the table crate avoids leaking the autoid implementation into planners.
///
/// 经已安装的生产工厂构造表，失败则回退 mock 工厂；分配器构造留在本 crate，
/// 避免规划器泄漏 autoid 实现细节。
pub fn BuildTableFromMeta(meta: &table_model::TableInfo) -> TableResult<Option<Box<dyn Table>>> {
    if let Some(factory) = *TableFromMeta
        .read()
        .map_err(|_| errors_dependency::New("TableFromMeta provider lock is poisoned"))?
    {
        return factory(autoid_dependency::Allocators::default(), meta).map(Some);
    }
    if let Some(factory) = *MockTableFromMeta
        .read()
        .map_err(|_| errors_dependency::New("MockTableFromMeta provider lock is poisoned"))?
    {
        return Ok(Some(factory(meta)));
    }
    Ok(None)
}

/// 缓存表扩展：初始化、租约内读缓存、读写锁保活。
pub trait CachedTable: Table {
    /// 初始化缓存表（通常触发首次加载）。
    fn Init(&self, executor: &mut dyn sqlexec_dependency::SQLExecutor) -> TableResult<()>;
    /// 尝试在租约窗口内读取缓存；第二返回值表示是否仍在加载。
    fn TryReadFromCache(
        &self,
        timestamp: u64,
        lease_duration: Duration,
    ) -> (Option<Box<dyn MemBuffer>>, bool);
    /// 为读路径更新远程读锁/租约。
    fn UpdateLockForRead(
        &self,
        context: &sqlexec_dependency::context::Context,
        store: &dyn Storage,
        timestamp: u64,
        lease_duration: Duration,
    );
    /// 获取写锁并在后台保活直至 `exit` 收到信号。
    fn WriteLockAndKeepAlive(
        &self,
        context: &sqlexec_dependency::context::Context,
        exit: std::sync::mpsc::Receiver<()>,
        lease: &mut u64,
        result: std::sync::mpsc::Sender<TableResult<()>>,
    );
}

/// CHECK evaluation remains intentionally blocked on the formal expression
/// AST-to-Expression/EvalInt API. Parsing is real; no callback or table-local
/// evaluator is used to claim the missing execution path is complete.
///
/// 对一行求值全部 CHECK 约束；任一约束为假（非 NULL）则返回违例错误。
pub fn CheckRowConstraint(
    expression_context: &dyn expression_dependency::BuildContext,
    constraints: &[Arc<Constraint>],
    row_to_check: Row,
    table_info: &table_model::TableInfo,
) -> TableResult<()> {
    if constraints.is_empty() {
        return Ok(());
    }

    let current_database = expression_context.GetEvalCtx().CurrentDB();
    // 逐条解析约束表达式并以 EvalInt 判定；0 且非 NULL 视为违例。
    for constraint in constraints {
        let expression = expression_dependency::ParseSimpleExpr(
            expression_context,
            &constraint.ConstraintInfo.ExprString,
            vec![expression_dependency::WithTableInfo(
                &current_database,
                table_info,
            )],
        )
        .map_err(SharedError::new)?;
        let (value, is_null) = expression
            .EvalInt(expression_context.GetEvalCtx(), row_to_check.clone())
            .map_err(SharedError::new)?;
        if value == 0 && !is_null {
            return Err(ErrCheckConstraintViolated.FastGenByArgs(&[constraint
                .ConstraintInfo
                .Name
                .O
                .clone()
                .into()]));
        }
    }

    Ok(())
}

/// 以 Datum 行构造临时 MutRow 后委托 `CheckRowConstraint`。
pub fn CheckRowConstraintWithDatum(
    expression_context: &dyn expression_dependency::BuildContext,
    constraints: &[Arc<Constraint>],
    row: Vec<Datum>,
    table_info: &table_model::TableInfo,
) -> TableResult<()> {
    if constraints.is_empty() {
        return Ok(());
    }
    let mutable_row = chunk_dependency::mutrow::MutRowFromDatums(row);
    CheckRowConstraint(
        expression_context,
        constraints,
        mutable_row.ToRow(),
        table_info,
    )
}

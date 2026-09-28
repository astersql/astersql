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

// 表契约（table）相关的 Aster 迁移单元测试。
//
// 覆盖表类型 iota、记录迭代器与约束入口形状、变更选项累积、
// 删除布局、表工厂安装、自增分配规则以及导出错误码映射。

use super::table_impl::*;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use autoid_dependency::{Allocator, AllocatorType, Allocators, AutoIdError, Context};
use kv_dependency::{Handle, Key, Transaction};
use model_dependency::group_4 as model;
use types_dependency::datum::Datum;

use crate::{DupKeyCheckMode, PessimisticLazyDupKeyCheckMode};

/// 表类型判别与 Go iota 数值对齐。
#[test]
fn table_type_predicates_match_go_iota_values() {
    assert_eq!(Type::NormalTable as i16, 0);
    assert_eq!(Type::VirtualTable as i16, 1);
    assert_eq!(Type::ClusterTable as i16, 2);
    assert!(Type::NormalTable.IsNormalTable());
    assert!(Type::VirtualTable.IsVirtualTable());
    assert!(Type::ClusterTable.IsClusterTable());
}

/// 记录迭代器与 CHECK 入口函数签名保持 Go 侧形状。
#[test]
fn record_iterator_and_constraint_entrypoints_keep_go_shapes() {
    let mut iterator: Box<RecordIterFunc> = Box::new(|handle, row, columns| {
        assert_eq!(handle.IntValue(), 7);
        assert!(row.is_empty());
        assert!(columns.is_empty());
        Ok(true)
    });
    assert!(
        iterator(
            Box::new(kv_dependency::IntHandle(7)),
            Vec::new(),
            Vec::new()
        )
        .unwrap()
    );

    let _row_entry: fn(
        &dyn expression_dependency::BuildContext,
        &[Arc<crate::Constraint>],
        chunk_dependency::Row,
        &model::TableInfo,
    ) -> TableResult<()> = CheckRowConstraint;
    let _datum_entry: fn(
        &dyn expression_dependency::BuildContext,
        &[Arc<crate::Constraint>],
        Vec<Datum>,
        &model::TableInfo,
    ) -> TableResult<()> = CheckRowConstraintWithDatum;

    fn object_safe_extensions(
        _physical: Option<&dyn PhysicalTable>,
        _partitioned: Option<&dyn PartitionedTable>,
        _cached: Option<&dyn CachedTable>,
    ) {
    }
    object_safe_extensions(None, None, None);
}

/// Add/Update 变更选项累积与互转语义对齐 Go。
#[test]
fn mutation_options_accumulate_and_convert_like_go() {
    let default_add = NewAddRecordOpt(&[]);
    assert_eq!(default_add.Ctx(), None);
    assert!(!default_add.IsUpdate());
    assert!(!default_add.GenerateRecordID());
    assert_eq!(default_add.ReserveAutoID(), 0);
    assert_eq!(default_add.GetCreateIdxOpt().Ctx(), None);

    let default_update = NewUpdateRecordOpt(&[]);
    assert_eq!(default_update.Ctx(), None);
    assert!(!default_update.SkipWriteUntouchedIndices());
    assert!(default_update.GetAddRecordOpt().GenerateRecordID());
    assert!(
        !default_update
            .GetAddRecordOptKeepRecordID()
            .GenerateRecordID()
    );
    assert_eq!(default_update.GetCreateIdxOpt().Ctx(), None);

    let context = kv_dependency::Context::todo();
    let with_context = WithCtx(context.clone());
    let reserve = WithReserveAutoIDHint(32);
    let add = NewAddRecordOpt(&[
        &with_context,
        &DupKeyCheckMode::DupKeyCheckLazy,
        &PessimisticLazyDupKeyCheckMode::DupKeyCheckInPrewrite,
        &reserve,
        &IsUpdate,
    ]);

    assert_eq!(add.Ctx(), Some(context.clone()));
    assert_eq!(add.DupKeyCheck(), DupKeyCheckMode::DupKeyCheckLazy);
    assert_eq!(
        add.PessimisticLazyDupKeyCheck(),
        PessimisticLazyDupKeyCheckMode::DupKeyCheckInPrewrite
    );
    assert!(add.IsUpdate());
    assert!(add.GenerateRecordID());
    assert_eq!(add.ReserveAutoID(), 32);
    let create = add.GetCreateIdxOpt();
    assert_eq!(create.Ctx(), Some(context.clone()));
    assert_eq!(create.DupKeyCheck(), DupKeyCheckMode::DupKeyCheckLazy);
    assert_eq!(
        create.PessimisticLazyDupKeyCheck(),
        PessimisticLazyDupKeyCheckMode::DupKeyCheckInPrewrite
    );

    let update = NewUpdateRecordOpt(&[
        &with_context,
        &DupKeyCheckMode::DupKeyCheckSkip,
        &SkipWriteUntouchedIndices,
    ]);
    assert!(update.SkipWriteUntouchedIndices());
    assert_eq!(update.Ctx(), Some(context));
    assert_eq!(update.DupKeyCheck(), DupKeyCheckMode::DupKeyCheckSkip);
    let generated = update.GetAddRecordOpt();
    assert!(generated.IsUpdate());
    assert!(generated.GenerateRecordID());
    let kept = update.GetAddRecordOptKeepRecordID();
    assert!(kept.IsUpdate());
    assert!(!kept.GenerateRecordID());
}

/// RemoveRecord 索引布局保留列偏移顺序。
#[test]
fn remove_record_layout_preserves_index_column_order() {
    let layout = IndexesLayout(HashMap::from([(7, vec![2, 0, 1]), (9, vec![3])]));
    let option = NewRemoveRecordOpt(&[&layout]);
    assert!(option.HasIndexesLayout());
    assert_eq!(option.GetIndexesLayout(), Some(&layout));
    assert_eq!(option.GetIndexLayout(7), Some(&vec![2, 0, 1]));
    assert_eq!(option.GetIndexLayout(9), Some(&vec![3]));
    assert_eq!(option.GetIndexLayout(11), None);
}

/// 记录 alloc 调用参数的测试用自增分配器。
#[derive(Default)]
struct RecordingAllocator {
    /// 依次记录 (count, increment, offset)。
    calls: Mutex<Vec<(u64, i64, i64)>>,
}

impl Allocator for RecordingAllocator {
    fn alloc(
        &self,
        _context: &Context,
        count: u64,
        increment: i64,
        offset: i64,
    ) -> Result<(i64, i64), AutoIdError> {
        self.calls.lock().unwrap().push((count, increment, offset));
        if count == 0 {
            return Err(AutoIdError::Canceled);
        }
        Ok(if count == 1 { (8, 9) } else { (6, 30) })
    }

    fn alloc_seq_cache(&self) -> Result<(i64, i64, i64), AutoIdError> {
        Ok((0, 0, 0))
    }

    fn rebase(
        &self,
        _context: &Context,
        _new_base: i64,
        _alloc_ids: bool,
    ) -> Result<(), AutoIdError> {
        Ok(())
    }

    fn force_rebase(&self, _new_base: i64) -> Result<(), AutoIdError> {
        Ok(())
    }

    fn rebase_seq(&self, _new_base: i64) -> Result<(i64, bool), AutoIdError> {
        Ok((0, false))
    }

    fn transfer(&self, _database_id: i64, _table_id: i64) -> Result<(), AutoIdError> {
        Ok(())
    }

    fn base(&self) -> i64 {
        0
    }

    fn end(&self) -> i64 {
        0
    }

    fn next_global_auto_id(&self) -> Result<i64, AutoIdError> {
        Ok(1)
    }

    fn get_type(&self) -> AllocatorType {
        AllocatorType::AutoIncrement
    }
}

/// 空实现的分配器上下文（无备选分配器）。
#[derive(Default)]
struct TestAllocatorContext;

impl AllocatorContext for TestAllocatorContext {
    fn AlternativeAllocators(&mut self, _table: &model::TableInfo) -> (Allocators, bool) {
        (Allocators::default(), false)
    }
}

/// 可配置步长/偏移的自增会话上下文桩。
struct TestAutoIncrementContext {
    /// AUTO_INCREMENT 步长。
    increment: i64,
    /// AUTO_INCREMENT 偏移。
    offset: i64,
    /// 嵌套的表分配器上下文。
    table_context: TestAllocatorContext,
}

impl AutoIncrementContext for TestAutoIncrementContext {
    fn AutoIncrementIncrement(&self) -> i64 {
        self.increment
    }

    fn AutoIncrementOffset(&self) -> i64 {
        self.offset
    }

    fn GetTableCtx(&mut self) -> &mut dyn AllocatorContext {
        &mut self.table_context
    }
}

/// 仅实现分配相关方法的最小 Table 桩。
struct AllocatorTable {
    /// 可选的记录型自增分配器；缺失时模拟无分配器错误。
    allocator: Option<Arc<RecordingAllocator>>,
    /// 表元信息。
    meta: model::TableInfo,
}

impl columnAPI for AllocatorTable {
    fn Cols(&self) -> Vec<Arc<crate::Column>> {
        Vec::new()
    }
    fn VisibleCols(&self) -> Vec<Arc<crate::Column>> {
        Vec::new()
    }
    fn HiddenCols(&self) -> Vec<Arc<crate::Column>> {
        Vec::new()
    }
    fn WritableCols(&self) -> Vec<Arc<crate::Column>> {
        Vec::new()
    }
    fn DeletableCols(&self) -> Vec<Arc<crate::Column>> {
        Vec::new()
    }
    fn FullHiddenColsAndVisibleCols(&self) -> Vec<Arc<crate::Column>> {
        Vec::new()
    }
}

impl Table for AllocatorTable {
    fn Indices(&self) -> Vec<Arc<dyn crate::Index>> {
        Vec::new()
    }

    fn DeletableIndices(&self) -> Vec<Arc<dyn crate::Index>> {
        Vec::new()
    }

    fn WritableConstraint(&self) -> Vec<Arc<crate::Constraint>> {
        Vec::new()
    }

    fn RecordPrefix(&self) -> Key {
        Key(Vec::new())
    }

    fn IndexPrefix(&self) -> Key {
        Key(Vec::new())
    }

    fn AddRecord(
        &self,
        _context: &mut dyn MutateContext,
        _transaction: &mut dyn Transaction,
        _row: &[Datum],
        _options: &[&dyn AddRecordOption],
    ) -> TableResult<Box<dyn Handle>> {
        unreachable!("allocation tests do not mutate records")
    }

    fn UpdateRecord(
        &self,
        _context: &mut dyn MutateContext,
        _transaction: &mut dyn Transaction,
        _handle: &dyn Handle,
        _current_data: &[Datum],
        _new_data: &[Datum],
        _touched: &[bool],
        _options: &[&dyn UpdateRecordOption],
    ) -> TableResult<()> {
        unreachable!("allocation tests do not mutate records")
    }

    fn RemoveRecord(
        &self,
        _context: &mut dyn MutateContext,
        _transaction: &mut dyn Transaction,
        _handle: &dyn Handle,
        _row: &[Datum],
        _options: &[&dyn RemoveRecordOption],
    ) -> TableResult<()> {
        unreachable!("allocation tests do not mutate records")
    }

    fn Allocators(&self, _context: &mut dyn AllocatorContext) -> Allocators {
        Allocators::new(
            true,
            self.allocator
                .iter()
                .cloned()
                .map(|allocator| allocator as Arc<dyn Allocator>)
                .collect(),
        )
    }

    fn Meta(&self) -> &model::TableInfo {
        &self.meta
    }

    fn UseNewCollate(&self) -> bool {
        false
    }

    fn Type(&self) -> Type {
        Type::NormalTable
    }

    fn GetPartitionedTable(&self) -> Option<&dyn PartitionedTable> {
        None
    }
}

/// mock 工厂：由元信息构造无分配器的 `AllocatorTable`。
fn test_mock_table_factory(table_info: &model::TableInfo) -> Box<dyn Table> {
    Box::new(AllocatorTable {
        allocator: None,
        meta: table_info.clone(),
    })
}

/// 生产工厂桩：忽略传入分配器，委托 mock 工厂。
fn test_table_factory(
    _allocators: Allocators,
    table_info: &model::TableInfo,
) -> TableResult<Box<dyn Table>> {
    Ok(test_mock_table_factory(table_info))
}

/// 可安全安装/恢复 TableFromMeta 与 MockTableFromMeta 工厂槽位。
#[test]
fn table_factories_can_be_installed_without_unsafe_globals() {
    let previous_mock = {
        let mut slot = MockTableFromMeta.write().unwrap();
        std::mem::replace(&mut *slot, Some(test_mock_table_factory))
    };
    let mock_factory = (*MockTableFromMeta.read().unwrap()).unwrap();
    assert!(
        mock_factory(&model::TableInfo::default())
            .Type()
            .IsNormalTable()
    );
    *MockTableFromMeta.write().unwrap() = previous_mock;

    let previous = {
        let mut slot = TableFromMeta.write().unwrap();
        std::mem::replace(&mut *slot, Some(test_table_factory))
    };
    let factory = (*TableFromMeta.read().unwrap()).unwrap();
    assert!(
        factory(Allocators::default(), &model::TableInfo::default())
            .unwrap()
            .Type()
            .IsNormalTable()
    );
    *TableFromMeta.write().unwrap() = previous;
}

/// 自增单值/批量分配的偏移回退与批量首值规则对齐 Go。
#[test]
fn auto_increment_allocation_matches_go_offset_and_batch_rules() {
    let allocator = Arc::new(RecordingAllocator::default());
    let table = AllocatorTable {
        allocator: Some(allocator.clone()),
        meta: model::TableInfo::default(),
    };
    let mut session = TestAutoIncrementContext {
        increment: 4,
        offset: 9,
        table_context: TestAllocatorContext,
    };
    let context = Context::background();

    assert_eq!(
        AllocAutoIncrementValue(&context, &table, &mut session).unwrap(),
        9
    );
    assert_eq!(
        AllocBatchAutoIncrementValue(&context, &table, &mut session, 6).unwrap(),
        (9, 4)
    );
    assert_eq!(*allocator.calls.lock().unwrap(), vec![(1, 4, 1), (6, 4, 1)]);

    // offset <= increment 时保留原 offset；count=0 触发 Canceled。
    session.offset = 3;
    assert_eq!(
        AllocAutoIncrementValue(&context, &table, &mut session).unwrap(),
        9
    );
    let error = AllocBatchAutoIncrementValue(&context, &table, &mut session, 0).unwrap_err();
    assert_eq!(
        error.downcast_ref::<AutoIdError>(),
        Some(&AutoIdError::Canceled)
    );
    assert_eq!(
        *allocator.calls.lock().unwrap(),
        vec![(1, 4, 1), (6, 4, 1), (1, 4, 3), (0, 4, 3)]
    );

    fn accepts_object_safe_table(_table: &dyn Table) {}
    accepts_object_safe_table(&table);

    let missing = AllocatorTable {
        allocator: None,
        meta: model::TableInfo::default(),
    };
    assert!(
        AllocAutoIncrementValue(&context, &missing, &mut session)
            .unwrap_err()
            .to_string()
            .contains("auto_increment allocator is missing")
    );
}

/// 导出错误原型保持与 MySQL errno 的映射。
#[test]
fn exported_error_prototypes_keep_mysql_codes() {
    assert_eq!(
        ErrColumnCantNull.Code(),
        i32::from(dbterror_dependency::errno::ErrBadNull)
    );
    assert_eq!(
        ErrUnknownColumn.Code(),
        i32::from(dbterror_dependency::errno::ErrBadField)
    );
    assert_eq!(
        errDuplicateColumn.Code(),
        i32::from(dbterror_dependency::errno::ErrFieldSpecifiedTwice)
    );
    assert_eq!(
        errGetDefaultFailed.Code(),
        i32::from(dbterror_dependency::errno::ErrFieldGetDefaultFailed)
    );
    assert_eq!(
        ErrNoDefaultValue.Code(),
        i32::from(dbterror_dependency::errno::ErrNoDefaultForField)
    );
    assert_eq!(
        ErrIndexOutBound.Code(),
        i32::from(dbterror_dependency::errno::ErrIndexOutBound)
    );
    assert_eq!(
        ErrUnsupportedOp.Code(),
        i32::from(dbterror_dependency::errno::ErrUnsupportedOp)
    );
    assert_eq!(
        ErrRowNotFound.Code(),
        i32::from(dbterror_dependency::errno::ErrRowNotFound)
    );
    assert_eq!(
        ErrTableStateCantNone.Code(),
        i32::from(dbterror_dependency::errno::ErrTableStateCantNone)
    );
    assert_eq!(
        ErrColumnStateCantNone.Code(),
        i32::from(dbterror_dependency::errno::ErrColumnStateCantNone)
    );
    assert_eq!(
        ErrColumnStateNonPublic.Code(),
        i32::from(dbterror_dependency::errno::ErrColumnStateNonPublic)
    );
    assert_eq!(
        ErrIndexStateCantNone.Code(),
        i32::from(dbterror_dependency::errno::ErrIndexStateCantNone)
    );
    assert_eq!(
        ErrInvalidRecordKey.Code(),
        i32::from(dbterror_dependency::errno::ErrInvalidRecordKey)
    );
    assert_eq!(
        ErrTruncatedWrongValueForField.Code(),
        i32::from(dbterror_dependency::errno::ErrTruncatedWrongValueForField)
    );
    assert_eq!(
        ErrUnknownPartition.Code(),
        i32::from(dbterror_dependency::errno::ErrUnknownPartition)
    );
    assert_eq!(
        ErrNoPartitionForGivenValue.Code(),
        i32::from(dbterror_dependency::errno::ErrNoPartitionForGivenValue)
    );
    assert_eq!(
        ErrLockOrActiveTransaction.Code(),
        i32::from(dbterror_dependency::errno::ErrLockOrActiveTransaction)
    );
    assert_eq!(
        ErrWarnNullToNotnull.Code(),
        i32::from(dbterror_dependency::errno::ErrWarnNullToNotnull)
    );
    assert_eq!(
        ErrSequenceHasRunOut.Code(),
        i32::from(dbterror_dependency::errno::ErrSequenceRunOut)
    );
    assert_eq!(
        ErrRowDoesNotMatchGivenPartitionSet.Code(),
        i32::from(dbterror_dependency::errno::ErrRowDoesNotMatchGivenPartitionSet)
    );
    assert_eq!(
        ErrTempTableFull.Code(),
        i32::from(dbterror_dependency::errno::ErrRecordFileFull)
    );
    assert_eq!(
        ErrOptOnCacheTable.Code(),
        i32::from(dbterror_dependency::errno::ErrOptOnCacheTable)
    );
    assert_eq!(
        ErrCheckConstraintViolated.Code(),
        i32::from(dbterror_dependency::errno::ErrCheckConstraintViolated)
    );
}

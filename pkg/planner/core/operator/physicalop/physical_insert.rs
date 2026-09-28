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

// INSERT/REPLACE 物理计划：目标表解析、VALUES/SELECT 输入、ON DUPLICATE、生成列与外键子计划。
//
// DML（数据操纵语言）在此阶段完成可执行表对象绑定与表达式索引解析；
// 真正的行写入由执行器完成。分区表可通过 PARTITION 子句收窄允许的物理分区集合。

#![allow(non_snake_case)]

use crate::SimpleSchemaProducer;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::LazyLock;

/// 空统计信息占位，供 Insert 在未设置 Stats 时返回稳定引用。
static EMPTY_INSERT_STATS: LazyLock<property::StatsInfo> =
    LazyLock::new(property::StatsInfo::default);

/// Catalog-backed table adapter used when the embedding runtime has not yet
/// installed Go's `table.TableFromMeta` constructor. It carries the complete
/// table/column metadata and allocator boundary required by DML planning;
/// storage mutation remains an executor responsibility and fails explicitly.
///
/// 基于目录元数据的表适配器：嵌入运行时尚未安装 `TableFromMeta` 时使用。
/// 携带 DML 规划所需的完整表/列元数据与分配器边界；存储变更仍由执行器负责，此处显式失败。
pub struct MetadataTableAdapter {
    meta: model::TableInfo,
    columns: Vec<Arc<table::Column>>,
}

impl MetadataTableAdapter {
    /// 从 TableInfo 构造列包装列表。
    pub fn New(meta: &model::TableInfo) -> Self {
        Self {
            meta: meta.Clone(),
            columns: meta
                .Columns
                .iter()
                .map(|column| table::Column::New(Box::new(column.Clone())))
                .collect(),
        }
    }

    /// 统一返回“不支持变更”错误，避免规划期误写存储。
    fn mutation_unsupported() -> table::TableResult<()> {
        Err(table::ErrUnsupportedOp.GenWithStackByArgs(&[]))
    }
}

impl table::columnAPI for MetadataTableAdapter {
    fn Cols(&self) -> Vec<Arc<table::Column>> {
        self.columns.clone()
    }
    fn VisibleCols(&self) -> Vec<Arc<table::Column>> {
        self.columns
            .iter()
            .filter(|column| !column.ColumnInfo.Hidden)
            .cloned()
            .collect()
    }
    fn HiddenCols(&self) -> Vec<Arc<table::Column>> {
        self.columns
            .iter()
            .filter(|column| column.ColumnInfo.Hidden)
            .cloned()
            .collect()
    }
    fn WritableCols(&self) -> Vec<Arc<table::Column>> {
        self.columns.clone()
    }
    fn DeletableCols(&self) -> Vec<Arc<table::Column>> {
        self.columns.clone()
    }
    fn FullHiddenColsAndVisibleCols(&self) -> Vec<Arc<table::Column>> {
        self.HiddenCols()
            .into_iter()
            .chain(self.VisibleCols())
            .collect()
    }
}

impl table::Table for MetadataTableAdapter {
    fn Indices(&self) -> Vec<Arc<dyn table::Index>> {
        Vec::new()
    }
    fn DeletableIndices(&self) -> Vec<Arc<dyn table::Index>> {
        Vec::new()
    }
    fn WritableConstraint(&self) -> Vec<Arc<table::Constraint>> {
        Vec::new()
    }
    fn RecordPrefix(&self) -> kv::Key {
        kv::Key(self.meta.ID.to_be_bytes().to_vec())
    }
    fn IndexPrefix(&self) -> kv::Key {
        let mut prefix = self.meta.ID.to_be_bytes().to_vec();
        prefix.push(b'i');
        kv::Key(prefix)
    }
    fn AddRecord(
        &self,
        _context: &mut dyn table::MutateContext,
        _transaction: &mut dyn kv::Transaction,
        _row: &[types::datum::Datum],
        _options: &[&dyn table::AddRecordOption],
    ) -> table::TableResult<Box<dyn kv::Handle>> {
        Self::mutation_unsupported()?;
        unreachable!()
    }
    fn UpdateRecord(
        &self,
        _context: &mut dyn table::MutateContext,
        _transaction: &mut dyn kv::Transaction,
        _handle: &dyn kv::Handle,
        _current_data: &[types::datum::Datum],
        _new_data: &[types::datum::Datum],
        _touched: &[bool],
        _options: &[&dyn table::UpdateRecordOption],
    ) -> table::TableResult<()> {
        Self::mutation_unsupported()
    }
    fn RemoveRecord(
        &self,
        _context: &mut dyn table::MutateContext,
        _transaction: &mut dyn kv::Transaction,
        _handle: &dyn kv::Handle,
        _row: &[types::datum::Datum],
        _options: &[&dyn table::RemoveRecordOption],
    ) -> table::TableResult<()> {
        Self::mutation_unsupported()
    }
    fn Allocators(&self, _context: &mut dyn table::AllocatorContext) -> table::AllocatorCollection {
        table::AllocatorCollection::default()
    }
    fn Meta(&self) -> &model::TableInfo {
        &self.meta
    }
    fn UseNewCollate(&self) -> bool {
        false
    }
    fn Type(&self) -> table::Type {
        table::Type::NormalTable
    }
    fn GetPartitionedTable(&self) -> Option<&dyn table::PartitionedTable> {
        None
    }
}

/// Go's `NewPartitionTableWithGivenSets` equivalent. The executable table is
/// shared, while partition routing is constrained to the IDs named by INSERT.
///
/// 等价于 Go `NewPartitionTableWithGivenSets`：共享可执行表，但分区路由限制在
/// INSERT 指名的分区 ID 集合内。
struct SelectedPartitionTable {
    /// 底层完整分区表。
    source: Arc<dyn table::Table>,
    /// 允许访问的物理分区 ID 集合。
    allowed: HashSet<i64>,
}

impl table::columnAPI for SelectedPartitionTable {
    fn Cols(&self) -> Vec<Arc<table::Column>> {
        self.source.Cols()
    }
    fn VisibleCols(&self) -> Vec<Arc<table::Column>> {
        self.source.VisibleCols()
    }
    fn HiddenCols(&self) -> Vec<Arc<table::Column>> {
        self.source.HiddenCols()
    }
    fn WritableCols(&self) -> Vec<Arc<table::Column>> {
        self.source.WritableCols()
    }
    fn DeletableCols(&self) -> Vec<Arc<table::Column>> {
        self.source.DeletableCols()
    }
    fn FullHiddenColsAndVisibleCols(&self) -> Vec<Arc<table::Column>> {
        self.source.FullHiddenColsAndVisibleCols()
    }
}

impl table::Table for SelectedPartitionTable {
    fn Indices(&self) -> Vec<Arc<dyn table::Index>> {
        self.source.Indices()
    }
    fn DeletableIndices(&self) -> Vec<Arc<dyn table::Index>> {
        self.source.DeletableIndices()
    }
    fn WritableConstraint(&self) -> Vec<Arc<table::Constraint>> {
        self.source.WritableConstraint()
    }
    fn RecordPrefix(&self) -> kv::Key {
        self.source.RecordPrefix()
    }
    fn IndexPrefix(&self) -> kv::Key {
        self.source.IndexPrefix()
    }
    fn AddRecord(
        &self,
        context: &mut dyn table::MutateContext,
        transaction: &mut dyn kv::Transaction,
        row: &[types::datum::Datum],
        options: &[&dyn table::AddRecordOption],
    ) -> table::TableResult<Box<dyn kv::Handle>> {
        self.source.AddRecord(context, transaction, row, options)
    }
    fn UpdateRecord(
        &self,
        context: &mut dyn table::MutateContext,
        transaction: &mut dyn kv::Transaction,
        handle: &dyn kv::Handle,
        current_data: &[types::datum::Datum],
        new_data: &[types::datum::Datum],
        touched: &[bool],
        options: &[&dyn table::UpdateRecordOption],
    ) -> table::TableResult<()> {
        self.source.UpdateRecord(
            context,
            transaction,
            handle,
            current_data,
            new_data,
            touched,
            options,
        )
    }
    fn RemoveRecord(
        &self,
        context: &mut dyn table::MutateContext,
        transaction: &mut dyn kv::Transaction,
        handle: &dyn kv::Handle,
        row: &[types::datum::Datum],
        options: &[&dyn table::RemoveRecordOption],
    ) -> table::TableResult<()> {
        self.source
            .RemoveRecord(context, transaction, handle, row, options)
    }
    fn Allocators(&self, context: &mut dyn table::AllocatorContext) -> table::AllocatorCollection {
        self.source.Allocators(context)
    }
    fn Meta(&self) -> &model::TableInfo {
        self.source.Meta()
    }
    fn UseNewCollate(&self) -> bool {
        self.source.UseNewCollate()
    }
    fn Type(&self) -> table::Type {
        self.source.Type()
    }
    fn GetPartitionedTable(&self) -> Option<&dyn table::PartitionedTable> {
        Some(self)
    }
}

impl table::PartitionedTable for SelectedPartitionTable {
    /// 仅当 physical_id 在允许集合内时转发到底层分区。
    fn GetPartition(&self, physical_id: i64) -> Option<&dyn table::PhysicalTable> {
        self.allowed
            .contains(&physical_id)
            .then(|| self.source.GetPartitionedTable()?.GetPartition(physical_id))
            .flatten()
    }
    /// 按行计算目标分区，并校验其落在允许集合内。
    fn GetPartitionByRow(
        &self,
        context: &dyn expression::EvalContext,
        row: &[types::datum::Datum],
    ) -> table::TableResult<&dyn table::PhysicalTable> {
        let partition = self
            .source
            .GetPartitionedTable()
            .expect("selected partition wrapper requires partitioned source")
            .GetPartitionByRow(context, row)?;
        if self.allowed.contains(&partition.GetPhysicalID()) {
            Ok(partition)
        } else {
            Err(table::ErrRowDoesNotMatchGivenPartitionSet.GenWithStackByArgs(&[]))
        }
    }
    fn GetPartitionIdxByRow(
        &self,
        context: &dyn expression::EvalContext,
        row: &[types::datum::Datum],
    ) -> table::TableResult<i32> {
        let partition = self.GetPartitionByRow(context, row)?;
        self.GetAllPartitionIDs()
            .iter()
            .position(|id| *id == partition.GetPhysicalID())
            .map(|offset| offset as i32)
            .ok_or_else(|| table::ErrRowDoesNotMatchGivenPartitionSet.GenWithStackByArgs(&[]))
    }
    fn GetAllPartitionIDs(&self) -> Vec<i64> {
        self.source
            .GetPartitionedTable()
            .expect("selected partition wrapper requires partitioned source")
            .GetAllPartitionIDs()
            .into_iter()
            .filter(|id| self.allowed.contains(id))
            .collect()
    }
    fn GetPartitionColumnIDs(&self) -> Vec<i64> {
        self.source
            .GetPartitionedTable()
            .expect("selected partition wrapper requires partitioned source")
            .GetPartitionColumnIDs()
    }
    fn GetPartitionColumnNames(&self) -> Vec<parser_ast::CIStr> {
        self.source
            .GetPartitionedTable()
            .expect("selected partition wrapper requires partitioned source")
            .GetPartitionColumnNames()
    }
    fn CheckForExchangePartition(
        &self,
        context: &dyn expression::EvalContext,
        partition_info: &model::PartitionInfo,
        row: &[types::datum::Datum],
        partition_id: i64,
        table_id: i64,
    ) -> table::TableResult<()> {
        self.source
            .GetPartitionedTable()
            .expect("selected partition wrapper requires partitioned source")
            .CheckForExchangePartition(context, partition_info, row, partition_id, table_id)
    }
}

/// Resolves the executable INSERT target through the installed production
/// constructor first, then through the mock constructor, and finally through
/// the metadata adapter used by standalone planner embeddings.
///
/// 解析 INSERT 可执行目标表：优先生产构造器，其次 mock，最后元数据适配器。
/// `partition_names` 非空时包装为 `SelectedPartitionTable`。
pub fn NewInsertTargetTable(
    meta: &model::TableInfo,
    partition_names: &[parser_ast::CIStr],
) -> Result<Arc<dyn table::Table>, expression::Error> {
    let target: Arc<dyn table::Table> = match table::BuildTableFromMeta(meta)? {
        Some(table) => Arc::from(table),
        None => Arc::new(MetadataTableAdapter::New(meta)),
    };
    if partition_names.is_empty() {
        return Ok(target);
    }
    // 将 PARTITION 子句中的名字解析为物理分区 ID，构造允许集合。
    let partition_info = meta
        .GetPartitionInfo()
        .ok_or_else(|| expression::errors::New("PARTITION clause on non partitioned table"))?;
    let mut allowed = HashSet::with_capacity(partition_names.len());
    for name in partition_names {
        let definition = partition_info
            .Definitions
            .iter()
            .find(|definition| definition.Name.L == name.L)
            .ok_or_else(|| {
                expression::errors::New(format!(
                    "Unknown partition '{}' in table '{}'",
                    name.O, meta.Name.O
                ))
            })?;
        allowed.insert(definition.ID);
    }
    if target.GetPartitionedTable().is_none() {
        return Err(expression::errors::New(format!(
            "Can't get executable partitions for table {}",
            meta.Name.O
        )));
    }
    Ok(Arc::new(SelectedPartitionTable {
        source: target,
        allowed,
    }))
}

/// Generated-column work retained by INSERT until executor construction.
///
/// INSERT 保留至执行器构建阶段的生成列表达式，以及 ON DUPLICATE 路径上的生成列赋值。
#[derive(Default)]
pub struct InsertGeneratedColumns {
    /// 普通插入路径的生成列表达式。
    pub Exprs: Vec<expression::ExprBox>,
    /// ON DUPLICATE KEY UPDATE 路径上的生成列赋值。
    pub OnDuplicates: Vec<Box<expression::Assignment>>,
}

impl Clone for InsertGeneratedColumns {
    fn clone(&self) -> Self {
        self.CloneForPlanCache()
    }
}

impl InsertGeneratedColumns {
    /// 计划缓存用深克隆：表达式与赋值均 CloneExpr/Clone。
    pub fn CloneForPlanCache(&self) -> Self {
        Self {
            Exprs: self.Exprs.iter().map(|expr| expr.CloneExpr()).collect(),
            OnDuplicates: self
                .OnDuplicates
                .iter()
                .map(|assignment| Box::new(assignment.Clone()))
                .collect(),
        }
    }

    /// 累加生成列表达式与赋值的内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.Exprs
            .iter()
            .map(|expression| expression.MemoryUsage())
            .sum::<i64>()
            + self
                .OnDuplicates
                .iter()
                .map(|assignment| assignment.MemoryUsage())
                .sum::<i64>()
    }
}

/// 外键存在性/引用检查物理子计划（Foreign Key Check）。
pub struct FKCheck {
    pub BasePhysicalPlan: crate::BasePhysicalPlan,
    /// 本表外键定义。
    pub FK: Option<Arc<model::FKInfo>>,
    pub ReferredFK: Option<Arc<model::ReferredFKInfo>>,
    pub Tbl: Option<Arc<dyn table::Table>>,
    pub Idx: Option<Arc<dyn table::Index>>,
    pub Cols: Vec<parser_ast::CIStr>,
    pub IdxIsPrimaryKey: bool,
    pub IdxIsExclusive: bool,
    pub CheckExist: bool,
    pub FailedErr: Option<expression::Error>,
}

impl FKCheck {
    /// 构造 TypeForeignKeyCheck 空壳。
    pub fn New(ctx: base::ContextRef) -> Self {
        Self {
            BasePhysicalPlan: crate::NewBasePhysicalPlan(ctx, plancodec::TypeForeignKeyCheck, 0),
            FK: None,
            ReferredFK: None,
            Tbl: None,
            Idx: None,
            Cols: Vec::new(),
            IdxIsPrimaryKey: false,
            IdxIsExclusive: false,
            CheckExist: false,
            FailedErr: None,
        }
    }
}

/// 外键级联动作类型：ON DELETE / ON UPDATE。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i8)]
pub enum FKCascadeType {
    OnDelete = 1,
    OnUpdate = 2,
}

/// 外键级联物理子计划：在父行变更时级联修改子表。
pub struct FKCascade {
    pub BasePhysicalPlan: crate::BasePhysicalPlan,
    /// 级联动作类型。
    pub Tp: FKCascadeType,
    pub ReferredFK: Option<Arc<model::ReferredFKInfo>>,
    pub ChildTable: Option<Arc<dyn table::Table>>,
    pub FK: Option<Arc<model::FKInfo>>,
    pub FKCols: Vec<Arc<model::ColumnInfo>>,
    pub FKIdx: Option<Arc<model::IndexInfo>>,
    pub CascadePlans: Vec<Box<dyn base::Plan>>,
}

impl FKCascade {
    /// 构造指定级联类型的 TypeForeignKeyCascade 空壳。
    pub fn New(ctx: base::ContextRef, cascade_type: FKCascadeType) -> Self {
        Self {
            BasePhysicalPlan: crate::NewBasePhysicalPlan(ctx, plancodec::TypeForeignKeyCascade, 0),
            Tp: cascade_type,
            ReferredFK: None,
            ChildTable: None,
            FK: None,
            FKCols: Vec::new(),
            FKIdx: None,
            CascadePlans: Vec::new(),
        }
    }
}

/// INSERT/REPLACE physical statement plan.
///
/// This keeps the complete DML state consumed by expression rewriting and the
/// executor: target metadata, VALUES/SELECT input, duplicate-key assignments,
/// generated columns, behavior flags, and foreign-key physical subplans.
///
/// INSERT/REPLACE 物理语句计划：保存表达式改写与执行器消费的完整 DML 状态，
/// 包括目标元数据、VALUES/SELECT 输入、重复键赋值、生成列、行为标志与外键子计划。
pub struct Insert {
    pub SimpleSchemaProducer: SimpleSchemaProducer,
    /// 可执行目标表。
    pub Table: Option<Arc<dyn table::Table>>,
    /// 目标表 Schema（列类型布局）。
    pub TableSchema: Option<expression::Schema>,
    /// 目标表列名。
    pub TableColNames: expression::types::NameSlice,
    /// INSERT 列清单（AST 列名）。
    pub Columns: Vec<Box<parser_ast::ColumnName>>,
    /// VALUES 多行表达式列表。
    pub Lists: Vec<Vec<expression::ExprBox>>,
    /// ON DUPLICATE KEY UPDATE 赋值列表。
    pub OnDuplicate: Vec<Box<expression::Assignment>>,
    /// ON DUPLICATE 表达式解析所用 Schema。
    pub Schema4OnDuplicate: Option<expression::Schema>,
    /// ON DUPLICATE 列名。
    pub Names4OnDuplicate: expression::types::NameSlice,
    /// 生成列相关表达式。
    pub GenCols: InsertGeneratedColumns,
    /// INSERT ... SELECT 的选择子计划。
    pub SelectPlan: Option<Box<dyn base::PhysicalPlan>>,
    /// 是否为 REPLACE 语义。
    pub IsReplace: bool,
    /// INSERT IGNORE：忽略部分错误。
    pub IgnoreErr: bool,
    /// 是否需要填充默认值。
    pub NeedFillDefaultValue: bool,
    /// 所有赋值为常量时可用于优化。
    pub AllAssignmentsAreConstant: bool,
    /// 行宽（列数）。
    pub RowLen: isize,
    /// 外键检查子计划。
    pub FKChecks: Vec<Box<FKCheck>>,
    /// 外键级联子计划。
    pub FKCascades: Vec<Box<FKCascade>>,
}

impl Insert {
    /// 构造 TypeInsert 计划，并初始化空输出 Schema。
    pub fn New(ctx: base::ContextRef) -> Self {
        let mut plan = Self {
            SimpleSchemaProducer: SimpleSchemaProducer::New(ctx, plancodec::TypeInsert, 0),
            Table: None,
            TableSchema: None,
            TableColNames: expression::types::NameSlice(Vec::new()),
            Columns: Vec::new(),
            Lists: Vec::new(),
            OnDuplicate: Vec::new(),
            Schema4OnDuplicate: None,
            Names4OnDuplicate: expression::types::NameSlice(Vec::new()),
            GenCols: InsertGeneratedColumns::default(),
            SelectPlan: None,
            IsReplace: false,
            IgnoreErr: false,
            NeedFillDefaultValue: false,
            AllAssignmentsAreConstant: false,
            RowLen: 0,
            FKChecks: Vec::new(),
            FKCascades: Vec::new(),
        };
        plan.SimpleSchemaProducer
            .SetSchema(expression::NewSchema(Vec::new()));
        plan
    }

    /// Go 克隆器对任一外键检查/级联子计划都拒绝进入计划缓存。
    pub(crate) fn HasForeignKeyPlans(&self) -> bool {
        !self.FKChecks.is_empty() || !self.FKCascades.is_empty()
    }

    /// 按表 Schema / OnDuplicate Schema 解析赋值与生成列表达式下标。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.SimpleSchemaProducer.ResolveIndices()?;
        let needs_table_schema = !self.OnDuplicate.is_empty()
            || !self.GenCols.Exprs.is_empty()
            || !self.GenCols.OnDuplicates.is_empty();
        let table_schema =
            if needs_table_schema {
                Some(self.TableSchema.as_ref().ok_or_else(|| {
                    expression::errors::New("insert table schema is not initialized")
                })?)
            } else {
                None
            };
        let needs_on_duplicate_schema =
            !self.OnDuplicate.is_empty() || !self.GenCols.OnDuplicates.is_empty();
        let on_duplicate_schema = if needs_on_duplicate_schema {
            Some(self.Schema4OnDuplicate.as_ref().ok_or_else(|| {
                expression::errors::New("insert on-duplicate schema is not initialized")
            })?)
        } else {
            None
        };
        for assignment in &mut self.OnDuplicate {
            assignment.Col = assignment.Col.ResolveIndices(
                table_schema.expect("non-empty assignments require table schema"),
            )?;
            assignment.Expr = assignment.Expr.ResolveIndices(
                on_duplicate_schema.expect("non-empty assignments require duplicate schema"),
            )?;
        }
        for generated in &mut self.GenCols.Exprs {
            *generated = generated.ResolveIndices(
                table_schema.expect("non-empty generated expressions require table schema"),
            )?;
        }
        for assignment in &mut self.GenCols.OnDuplicates {
            assignment.Col = assignment.Col.ResolveIndices(
                table_schema.expect("non-empty generated assignments require table schema"),
            )?;
            assignment.Expr = assignment.Expr.ResolveIndices(
                on_duplicate_schema
                    .expect("non-empty generated assignments require duplicate schema"),
            )?;
        }
        Ok(())
    }

    /// 累加 Schema、VALUES、赋值、子计划与外键子计划内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.SimpleSchemaProducer.MemoryUsage()
            + self
                .TableSchema
                .as_ref()
                .map_or(0, expression::Schema::MemoryUsage)
            + self
                .Schema4OnDuplicate
                .as_ref()
                .map_or(0, expression::Schema::MemoryUsage)
            + self.GenCols.MemoryUsage()
            + self
                .Lists
                .iter()
                .flatten()
                .map(|expression| expression.MemoryUsage())
                .sum::<i64>()
            + self
                .OnDuplicate
                .iter()
                .map(|assignment| assignment.MemoryUsage())
                .sum::<i64>()
            + self
                .SelectPlan
                .as_ref()
                .map_or(0, |plan| plan.memory_usage())
            + self
                .FKChecks
                .iter()
                .map(|check| check.BasePhysicalPlan.MemoryUsage())
                .sum::<i64>()
            + self
                .FKCascades
                .iter()
                .map(|cascade| cascade.BasePhysicalPlan.MemoryUsage())
                .sum::<i64>()
    }
}

impl base::Plan for Insert {
    /// 向下转型入口。
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    /// 可变向下转型入口。
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
    fn schema(&self) -> &expression::Schema {
        self.SimpleSchemaProducer
            .SchemaRef()
            .expect("Insert initializes an empty output schema")
    }
    fn id(&self) -> i32 {
        self.SimpleSchemaProducer.Plan.ID()
    }
    fn set_id(&mut self, id: i32) {
        self.SimpleSchemaProducer.Plan.SetID(id)
    }
    fn tp(&self, flags: &[bool]) -> String {
        self.SimpleSchemaProducer.Plan.TP(flags)
    }
    fn explain_id(&self, flags: &[bool]) -> Box<dyn std::fmt::Display + '_> {
        self.SimpleSchemaProducer.Plan.ExplainID(flags)
    }
    fn explain_info(&self) -> String {
        self.SimpleSchemaProducer.Plan.ExplainInfo()
    }
    fn replace_expr_columns(&mut self, replace: &HashMap<String, expression::Column>) {
        self.SimpleSchemaProducer.Plan.ReplaceExprColumns(replace)
    }
    fn s_ctx(&self) -> &base::ContextRef {
        self.SimpleSchemaProducer.Plan.SCtx()
    }
    fn stats_info(&self) -> &property::StatsInfo {
        self.SimpleSchemaProducer
            .Plan
            .StatsInfo()
            .unwrap_or(&EMPTY_INSERT_STATS)
    }
    fn output_names(&self) -> base::types::NameSlice {
        self.SimpleSchemaProducer.OutputNames()
    }
    fn set_output_names(&mut self, names: base::types::NameSlice) {
        self.SimpleSchemaProducer.SetOutputNames(names)
    }
    fn query_block_offset(&self) -> i32 {
        self.SimpleSchemaProducer.Plan.QueryBlockOffset()
    }
    /// 计划缓存克隆：含外键子计划时不可缓存；否则深克隆输入与赋值。
    fn clone_for_plan_cache(
        &self,
        new_ctx: base::ContextRef,
    ) -> (Option<Box<dyn base::Plan>>, bool) {
        // 外键检查/级联使计划不可进入缓存。
        if self.HasForeignKeyPlans() {
            return (None, false);
        }
        let select_plan = match &self.SelectPlan {
            Some(plan) => match plan.clone_physical(new_ctx.clone()) {
                Ok(plan) => Some(plan),
                Err(_) => return (None, false),
            },
            None => None,
        };
        let cloned = Insert {
            SimpleSchemaProducer: self.SimpleSchemaProducer.CloneSelfForPlanCache(new_ctx),
            Table: self.Table.clone(),
            TableSchema: self.TableSchema.as_ref().map(expression::Schema::Clone),
            TableColNames: self.TableColNames.Shallow(),
            Columns: self
                .Columns
                .iter()
                .map(|column| Box::new((**column).clone()))
                .collect(),
            Lists: self
                .Lists
                .iter()
                .map(|row| row.iter().map(|expr| expr.CloneExpr()).collect())
                .collect(),
            OnDuplicate: self
                .OnDuplicate
                .iter()
                .map(|assignment| Box::new(assignment.Clone()))
                .collect(),
            Schema4OnDuplicate: self
                .Schema4OnDuplicate
                .as_ref()
                .map(expression::Schema::Clone),
            Names4OnDuplicate: self.Names4OnDuplicate.Shallow(),
            GenCols: self.GenCols.CloneForPlanCache(),
            SelectPlan: select_plan,
            IsReplace: self.IsReplace,
            IgnoreErr: self.IgnoreErr,
            NeedFillDefaultValue: self.NeedFillDefaultValue,
            AllAssignmentsAreConstant: self.AllAssignmentsAreConstant,
            RowLen: self.RowLen,
            FKChecks: Vec::new(),
            FKCascades: Vec::new(),
        };
        (Some(Box::new(cloned)), true)
    }
    fn set_noncacheable_reason(&mut self, reason: String) {
        self.SimpleSchemaProducer.Plan.SetNoncacheableReason(reason)
    }
    fn get_noncacheable_reason(&self) -> String {
        self.SimpleSchemaProducer.Plan.GetNoncacheableReason()
    }
}

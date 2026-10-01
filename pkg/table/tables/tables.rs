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

// 表公共实现：列/索引/约束元数据、行与二级索引维护、序列（sequence）分配。
//
// 对应 Go `tables.go` 中 `TableCommon` 等核心结构：在内存中维护记录与索引键，
// 并按 SchemaState（DDL 在线变更各阶段）过滤可写/可删列与索引；同时提供
// TableScan / PartitionTableScan 与临时表辅助类型。

use crate::index::{ColumnInfo, Index, IndexInfo, SchemaState, TableInfo, is_index_writable};
use crate::mutation_checker::Datum;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

/// The constructor branch selected by Go TableFromMetaWithCollate. This is a
/// metadata validation view, not an in-memory substitute for a persistent table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MetadataTableKind {
    Common,
    Cached,
}

/// Borrow the complete catalog model without converting it to the smaller row
/// engine model (which has stricter offset checks and loses Go-wire fields).
pub struct ValidatedTableMetadata<'a> {
    pub meta: &'a model_dependency::TableInfo,
    pub kind: MetadataTableKind,
    /// ASTs resolved against the complete catalog, keyed by column position.
    pub generated_expressions: BTreeMap<usize, generatedexpr::ast::ExprNode>,
    /// Default expressions are parsed without column-name resolution, as in Go.
    pub default_expressions: BTreeMap<usize, generatedexpr::ast::ExprNode>,
    pub constraints: Vec<Box<table_dependency::constraint::Constraint>>,
}

impl ValidatedTableMetadata<'_> {
    pub fn public_columns(&self) -> impl Iterator<Item = &model_dependency::ColumnInfo> {
        self.meta
            .Columns
            .iter()
            .filter(|column| column.State == model_dependency::SchemaState::Public)
    }

    pub fn writable_columns(&self) -> impl Iterator<Item = &model_dependency::ColumnInfo> {
        self.meta.Columns.iter().filter(|column| {
            !matches!(
                column.State,
                model_dependency::SchemaState::DeleteOnly
                    | model_dependency::SchemaState::DeleteReorganization
            )
        })
    }
}

/// Go TableFromMetaWithCollate validation and expression loading over the full model.
/// CHECK loading may repair the supplied metadata before index construction. Do not call
/// TableCommon::new here: Go logs column offset mismatches and accepts offsets
/// in unconditional indexes, whereas that constructor rejects them.
pub fn table_from_meta_for_validation(
    meta: &mut model_dependency::TableInfo,
) -> Result<ValidatedTableMetadata<'_>, String> {
    use model_dependency::SchemaState;
    if meta.State == SchemaState::None {
        return Err(format!(
            "[table:8042]table '{}' state can't be none",
            meta.Name.O
        ));
    }
    let mut generated_expressions = BTreeMap::new();
    let mut default_expressions = BTreeMap::new();
    for (offset, column) in meta.Columns.iter().enumerate() {
        if column.State == SchemaState::None {
            return Err(format!(
                "[table:8046]column '{}' state can't be none",
                column.Name.O
            ));
        }
        if column.Offset != offset as isize {
            eprintln!(
                "wrong table schema: table={}, column={}, index={offset}, offset={}, columnNumber={}",
                meta.Name.O,
                column.Name.O,
                column.Offset,
                meta.Columns.len()
            );
        }
        if !column.GeneratedExprString.is_empty() {
            let expression = generatedexpr::ParseExpression(&column.GeneratedExprString)
                .and_then(|expression| generatedexpr::SimpleResolveName(expression, meta))
                .map_err(|error| error.to_string())?;
            generated_expressions.insert(offset, expression);
        }
        if column.DefaultIsExpr {
            let Some(model_dependency::DefaultValue::String(value)) = &column.DefaultValue else {
                return Err(format!(
                    "invalid expression default for column '{}'",
                    column.Name.O
                ));
            };
            let value = std::str::from_utf8(value).map_err(|error| error.to_string())?;
            let expression =
                generatedexpr::ParseExpression(value).map_err(|error| error.to_string())?;
            default_expressions.insert(offset, expression);
        }
    }
    // Go loads CHECK metadata here; executable CHECK expressions are built by
    // BuildConstraintExprWithCtx at the later evaluation stage.
    let constraints = table_dependency::constraint::LoadCheckConstraint(meta)
        .map_err(|error| error.to_string())?;
    let partition = meta.GetPartitionInfo();
    if let Some(partition) = partition {
        if partition.Definitions.is_empty() {
            return Err("[table:1735]Unknown partition".into());
        }
        // Go loads the partition expression before initializing indexes.
        // Includes reorganization expressions and adding/dropping definitions.
        return Err("[ddl:8200]partition expression loading is not supported".into());
    }
    for index in &meta.Indices {
        if index.State == SchemaState::None {
            return Err(format!(
                "[table:8044]index '{}' state can't be none",
                index.Name.O
            ));
        }
        if !index.ConditionExprString.is_empty() {
            return Err("[ddl:8200]partial index expression loading is not supported".into());
        }
    }
    Ok(ValidatedTableMetadata {
        generated_expressions,
        default_expressions,
        constraints,
        meta,
        kind: if meta.TableCacheStatusType != model_dependency::TableCacheStatusDisable {
            MetadataTableKind::Cached
        } else {
            MetadataTableKind::Common
        },
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 可执行列描述：在 ColumnInfo 之上附加偏移、SchemaState、生成列与默认值。
pub struct Column {
    pub info: ColumnInfo,
    pub offset: usize,
    pub state: SchemaState,
    pub hidden: bool,
    pub generated: bool,
    pub generated_stored: bool,
    pub primary_key: bool,
    pub common_handle: bool,
    pub default_value: Option<Datum>,
    pub origin_default_value: Option<Datum>,
}

impl Column {
    /// 是否为未物化的虚拟生成列（generated column）。
    pub fn is_virtual_generated(&self) -> bool {
        self.generated && !self.generated_stored
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// CHECK 等表级约束的名称、SchemaState 与是否启用。
pub struct Constraint {
    pub name: String,
    pub state: SchemaState,
    pub enforced: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表操作错误：列偏移、行长、记录/索引冲突与序列异常。
pub enum TableError {
    InvalidColumnOffset(usize),
    IndexCondition(String),
    RowLength { expected: usize, actual: usize },
    RecordExists(i64),
    RecordNotFound(i64),
    DuplicateIndex { index: String, handle: i64 },
    IndexStateCannotNone(String),
    SequenceMissing,
    SequenceRunOut,
    InvalidSequenceCache,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 序列元数据：步长、起止、最小/最大与是否循环。
pub struct SequenceInfo {
    pub increment: i64,
    pub start: i64,
    pub min_value: i64,
    pub max_value: i64,
    pub cycle: bool,
}

/// 序列缓存分配器：向存储申请一批号段或 rebase 到新值。
pub trait SequenceAllocator: Send {
    fn alloc_cache(&mut self) -> Result<(i64, i64, i64), TableError>;
    fn rebase(&mut self, new_value: i64) -> Result<(i64, bool), TableError>;
}

/// 序列运行时：持有本地缓存区间 [base, end] 与轮次 round。
pub struct SequenceCommon {
    pub meta: SequenceInfo,
    end: i64,
    base: i64,
    round: i64,
    allocator: Box<dyn SequenceAllocator>,
}

impl SequenceCommon {
    /// 用元数据与分配器构造；本地缓存初始为空。
    pub fn new(meta: SequenceInfo, allocator: Box<dyn SequenceAllocator>) -> Self {
        Self {
            meta,
            end: 0,
            base: 0,
            round: 0,
            allocator,
        }
    }

    /// 返回当前缓存的 (base, end, round)。
    pub fn base_end_round(&self) -> (i64, i64, i64) {
        (self.base, self.end, self.round)
    }

    /// 计算本轮序列偏移：循环轮次后取 min/max，否则取 start。
    fn offset(&self) -> i64 {
        if self.meta.cycle && self.round > 0 {
            if self.meta.increment > 0 {
                self.meta.min_value
            } else {
                self.meta.max_value
            }
        } else {
            self.meta.start
        }
    }

    /// 在当前缓存内寻找严格越过 base 的下一个序列值。
    fn seek(&self) -> Option<i64> {
        seek_sequence_value(self.base, self.meta.increment, self.offset(), self.end)
    }

    /// 取出下一个序列值；缓存耗尽时向分配器申请新号段。
    pub fn next_value(&mut self) -> Result<i64, TableError> {
        // 缓存非空时先 seek；否则向分配器申请号段后再 seek。
        let mut next = (self.base != self.end).then(|| self.seek()).flatten();
        if next.is_none() {
            let (base, end, round) = self.allocator.alloc_cache()?;
            self.base = base;
            self.end = end;
            self.round = round;
            next = self.seek();
        }
        let value = next.ok_or(TableError::SequenceRunOut)?;
        self.base = value;
        Ok(value)
    }

    /// Returns `(stored value, already below/above base)`, matching SETVAL.
    ///
    /// 对齐 SETVAL：若新值未越过当前 base 则返回 (0, true)；否则更新缓存或 rebase。
    pub fn set_value(&mut self, new_value: i64) -> Result<(i64, bool), TableError> {
        // 正向序列：new_value 须严格大于 base；反向则须严格小于 base。
        if self.meta.increment > 0 {
            if new_value <= self.base {
                return Ok((0, true));
            }
            if new_value <= self.end {
                self.base = new_value;
                return Ok((new_value, false));
            }
        } else {
            if new_value >= self.base {
                return Ok((0, true));
            }
            if new_value >= self.end {
                self.base = new_value;
                return Ok((new_value, false));
            }
        }
        self.base = self.end;
        let result = self.allocator.rebase(new_value)?;
        self.base = new_value;
        self.end = new_value;
        Ok(result)
    }
}

/// Finds the first `offset + n*increment` strictly after `base`, bounded by
/// `end`. This is the sequence-cache rule used by TiDB's allocator.
///
/// 在序列缓存规则下寻找严格大于（或递减时严格小于）base 的下一个值，且不超过 end。
pub fn seek_sequence_value(base: i64, increment: i64, offset: i64, end: i64) -> Option<i64> {
    if increment == 0 {
        return None;
    }
    let step = increment.unsigned_abs() as i128;
    let base = base as i128;
    let offset = offset as i128;
    let candidate = if increment > 0 {
        let delta = base - offset;
        let n = if delta < 0 { 0 } else { delta / step + 1 };
        offset + n * step
    } else {
        let delta = offset - base;
        let n = if delta < 0 { 0 } else { delta / step + 1 };
        offset - n * step
    };
    let candidate = i64::try_from(candidate).ok()?;
    if (increment > 0 && candidate <= end) || (increment < 0 && candidate >= end) {
        Some(candidate)
    } else {
        None
    }
}

/// 表公共状态：元数据、列/索引/约束、内存行存储与索引键映射。
pub struct TableCommon {
    table_id: i64,
    physical_table_id: i64,
    meta: TableInfo,
    columns: Vec<Column>,
    indices: Vec<Index>,
    constraints: Vec<Constraint>,
    use_new_collation: bool,
    rows: BTreeMap<i64, Vec<Datum>>,
    index_entries: HashMap<Vec<u8>, i64>,
    next_handle: i64,
    sequence: Option<Arc<Mutex<SequenceCommon>>>,
}

impl Clone for TableCommon {
    fn clone(&self) -> Self {
        Self {
            table_id: self.table_id,
            physical_table_id: self.physical_table_id,
            meta: self.meta.clone(),
            columns: self.columns.clone(),
            indices: self.indices.clone(),
            constraints: self.constraints.clone(),
            use_new_collation: self.use_new_collation,
            rows: self.rows.clone(),
            index_entries: self.index_entries.clone(),
            next_handle: self.next_handle,
            sequence: self.sequence.clone(),
        }
    }
}

impl TableCommon {
    /// 由元数据、物理 ID、列与索引定义构造内存表；校验列偏移并创建 Index。
    pub fn new(
        meta: TableInfo,
        physical_table_id: i64,
        columns: Vec<Column>,
        index_info: Vec<IndexInfo>,
        constraints: Vec<Constraint>,
        use_new_collation: bool,
    ) -> Result<Self, TableError> {
        for column in &columns {
            if column.offset >= meta.columns.len() {
                return Err(TableError::InvalidColumnOffset(column.offset));
            }
        }
        if let Some(index) = index_info
            .iter()
            .find(|index| index.state == SchemaState::None)
        {
            return Err(TableError::IndexStateCannotNone(index.name.clone()));
        }
        let indices = index_info
            .into_iter()
            .map(|info| {
                Index::new(use_new_collation, physical_table_id, meta.clone(), info).map_err(
                    |error| match error {
                        crate::index::IndexError::ColumnOffset(offset) => {
                            TableError::InvalidColumnOffset(offset)
                        }
                        other => TableError::IndexCondition(format!("{other:?}")),
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            table_id: meta.id,
            physical_table_id,
            meta,
            columns,
            indices,
            constraints,
            use_new_collation,
            rows: BTreeMap::new(),
            index_entries: HashMap::new(),
            next_handle: 0,
            sequence: None,
        })
    }

    /// 深拷贝表状态（序列句柄共享 Arc）。
    pub fn copy(&self) -> Self {
        self.clone()
    }

    /// 返回表元数据。
    pub fn meta(&self) -> &TableInfo {
        &self.meta
    }

    /// 返回物理表 ID（分区表上为分区物理 ID）。
    pub fn physical_id(&self) -> i64 {
        self.physical_table_id
    }

    /// 是否启用新排序规则（new collation）。
    pub fn use_new_collation(&self) -> bool {
        self.use_new_collation
    }

    /// Bind canonical SQL partition routing to this table instance's collation
    /// mode, as partition expression construction does through `ForTable`.
    pub fn canonical_partition_router<'a>(
        &self,
        metadata: &'a model_dependency::TableInfo,
    ) -> crate::canonical_partition::CanonicalPartitionedTable<'a> {
        crate::canonical_partition::CanonicalPartitionedTable::new(metadata, self.use_new_collation)
    }

    /// Decide whether a column can be omitted using this table's collation mode.
    pub fn can_skip(
        &self,
        column: &Column,
        value: &Datum,
        primary_index: Option<&IndexInfo>,
    ) -> bool {
        can_skip_with_collation(self.use_new_collation, column, value, primary_index)
    }

    /// 返回全部索引。
    pub fn indices(&self) -> &[Index] {
        &self.indices
    }

    /// 全部已构造索引均可删除（构造阶段已拒绝 StateNone）。
    pub fn deletable_indices(&self) -> Vec<&Index> {
        self.indices.iter().collect()
    }

    /// DDL 当前阶段允许写入的索引。
    pub fn writable_indices(&self) -> Vec<&Index> {
        self.indices
            .iter()
            .filter(|index| is_index_writable(&index.index_info))
            .collect()
    }

    /// 按名查找可写且非 DeleteOnly 的索引（忽略大小写）。
    pub fn writable_index_by_name(&self, name: &str) -> Option<&Index> {
        self.writable_indices().into_iter().find(|index| {
            index.index_info.name.eq_ignore_ascii_case(name)
                && index.index_info.state != SchemaState::DeleteOnly
        })
    }

    /// 返回全部列。
    pub fn columns(&self) -> &[Column] {
        &self.columns
    }

    /// 非隐藏列。
    pub fn visible_columns(&self) -> Vec<&Column> {
        self.columns
            .iter()
            .filter(|column| column.state == SchemaState::Public && !column.hidden)
            .collect()
    }

    /// 隐藏列。
    pub fn hidden_columns(&self) -> Vec<&Column> {
        self.columns
            .iter()
            .filter(|column| column.state == SchemaState::Public && column.hidden)
            .collect()
    }

    /// 除 DeleteOnly / DeleteReorganization 外的可写列。
    pub fn writable_columns(&self) -> Vec<&Column> {
        self.columns
            .iter()
            .filter(|column| {
                !matches!(
                    column.state,
                    SchemaState::DeleteOnly | SchemaState::DeleteReorganization
                )
            })
            .collect()
    }

    /// 全部列均可删除。
    pub fn deletable_columns(&self) -> Vec<&Column> {
        self.columns.iter().collect()
    }

    /// 当前阶段需强制的可写约束。
    pub fn writable_constraints(&self) -> Vec<&Constraint> {
        self.constraints
            .iter()
            .filter(|constraint| {
                constraint.enforced
                    && !matches!(
                        constraint.state,
                        SchemaState::DeleteOnly | SchemaState::DeleteReorganization
                    )
            })
            .collect()
    }

    /// 记录键前缀 `t{physical_id}_r`。
    pub fn record_prefix(&self) -> Vec<u8> {
        format!("t{}_r", self.physical_table_id).into_bytes()
    }

    /// 索引键前缀 `t{physical_id}_i`。
    pub fn index_prefix(&self) -> Vec<u8> {
        format!("t{}_i", self.physical_table_id).into_bytes()
    }

    /// 由 handle（行标识）构造完整记录键。
    pub fn record_key(&self, handle: i64) -> Vec<u8> {
        format!("t{}_r{handle}", self.physical_table_id).into_bytes()
    }

    /// 分配连续 handle 区间，返回 [base, end)。
    pub fn alloc_handle_ids(&mut self, count: u64) -> Result<(i64, i64), TableError> {
        let count = i64::try_from(count).map_err(|_| TableError::SequenceRunOut)?;
        let base = self.next_handle;
        let end = base.checked_add(count).ok_or(TableError::SequenceRunOut)?;
        self.next_handle = end;
        Ok((base, end))
    }

    /// 插入一行并维护唯一索引；未指定 handle 时自动分配。
    pub fn add_record(&mut self, row: Vec<Datum>, handle: Option<i64>) -> Result<i64, TableError> {
        self.check_row(&row)?;
        // 未指定 handle 时分配新 ID；再构建索引并写入行。
        let handle = match handle {
            Some(handle) => handle,
            None => self.alloc_handle_ids(1)?.1,
        };
        if self.rows.contains_key(&handle) {
            return Err(TableError::RecordExists(handle));
        }
        let new_entries = self.build_index_entries(handle, &row)?;
        self.rows.insert(handle, row);
        for (key, owner) in new_entries {
            self.index_entries.insert(key, owner);
        }
        self.next_handle = self.next_handle.max(handle);
        Ok(handle)
    }

    /// 更新一行：先删旧索引再写新索引，失败时恢复旧索引以保持原子性。
    pub fn update_record(
        &mut self,
        handle: i64,
        old_row: &[Datum],
        new_row: Vec<Datum>,
        touched: &[bool],
    ) -> Result<(), TableError> {
        self.check_row(old_row)?;
        self.check_row(&new_row)?;
        if touched.len() != self.columns.len() {
            return Err(TableError::RowLength {
                expected: self.columns.len(),
                actual: touched.len(),
            });
        }
        if !self.rows.contains_key(&handle) {
            return Err(TableError::RecordNotFound(handle));
        }
        self.remove_index_entries(handle, old_row)?;
        match self.build_index_entries(handle, &new_row) {
            Ok(entries) => {
                self.rows.insert(handle, new_row);
                self.index_entries.extend(entries);
                Ok(())
            }
            Err(error) => {
                // Restore old index state so a failed unique check is atomic.
                // 唯一性检查失败时恢复旧索引，保证更新原子性。
                if let Ok(entries) = self.build_index_entries(handle, old_row) {
                    self.index_entries.extend(entries);
                }
                Err(error)
            }
        }
    }

    /// 删除一行及其索引条目。
    pub fn remove_record(&mut self, handle: i64, row: &[Datum]) -> Result<(), TableError> {
        if !self.rows.contains_key(&handle) {
            return Err(TableError::RecordNotFound(handle));
        }
        self.remove_index_entries(handle, row)?;
        self.rows.remove(&handle);
        Ok(())
    }

    /// 按列偏移投影读取一行。
    pub fn row_with_columns(
        &self,
        handle: i64,
        columns: &[usize],
    ) -> Result<Vec<Datum>, TableError> {
        let row = self
            .rows
            .get(&handle)
            .ok_or(TableError::RecordNotFound(handle))?;
        columns
            .iter()
            .map(|offset| {
                row.get(*offset)
                    .cloned()
                    .ok_or(TableError::InvalidColumnOffset(*offset))
            })
            .collect()
    }

    /// 按 handle 有序迭代全部记录。
    pub fn iter_records(&self) -> impl Iterator<Item = (i64, &[Datum])> {
        self.rows
            .iter()
            .map(|(handle, row)| (*handle, row.as_slice()))
    }

    /// 绑定序列到本表。
    pub fn set_sequence(&mut self, sequence: SequenceCommon) {
        self.sequence = Some(Arc::new(Mutex::new(sequence)));
    }

    /// 取序列下一个值。
    pub fn sequence_next_value(&self) -> Result<i64, TableError> {
        self.sequence
            .as_ref()
            .ok_or(TableError::SequenceMissing)?
            .lock()
            .expect("sequence lock poisoned")
            .next_value()
    }

    /// SETVAL：设置序列当前值。
    pub fn set_sequence_value(&self, value: i64) -> Result<(i64, bool), TableError> {
        self.sequence
            .as_ref()
            .ok_or(TableError::SequenceMissing)?
            .lock()
            .expect("sequence lock poisoned")
            .set_value(value)
    }

    /// 校验行宽与列数一致。
    fn check_row(&self, row: &[Datum]) -> Result<(), TableError> {
        if row.len() != self.columns.len() {
            Err(TableError::RowLength {
                expected: self.columns.len(),
                actual: row.len(),
            })
        } else {
            Ok(())
        }
    }

    /// 为可写索引生成键；唯一索引冲突时返回 DuplicateIndex。
    fn build_index_entries(
        &self,
        handle: i64,
        row: &[Datum],
    ) -> Result<Vec<(Vec<u8>, i64)>, TableError> {
        let mut entries = Vec::new();
        for index in self.writable_indices() {
            #[cfg(feature = "expression-runtime")]
            if !index
                .matches_partial_condition(row)
                .map_err(|error| TableError::IndexCondition(format!("{error:?}")))?
            {
                continue;
            }
            let values = index
                .index_info
                .columns
                .iter()
                .map(|column| {
                    row.get(column.offset)
                        .cloned()
                        .ok_or(TableError::InvalidColumnOffset(column.offset))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (key, distinct) = index
                .gen_index_key(&values, handle)
                .map_err(|_| TableError::InvalidColumnOffset(row.len()))?;
            if distinct
                && let Some(owner) = self.index_entries.get(&key)
                && *owner != handle
            {
                return Err(TableError::DuplicateIndex {
                    index: index.index_info.name.clone(),
                    handle: *owner,
                });
            }
            entries.push((key, handle));
        }
        Ok(entries)
    }

    /// 删除该行在所有可删索引上的键。
    fn remove_index_entries(&mut self, handle: i64, row: &[Datum]) -> Result<(), TableError> {
        let keys = self
            .indices
            .iter()
            .filter(|index| index.index_info.state != SchemaState::None)
            .map(|index| {
                #[cfg(feature = "expression-runtime")]
                if !index
                    .matches_partial_condition(row)
                    .map_err(|error| TableError::IndexCondition(format!("{error:?}")))?
                {
                    return Ok(None);
                }
                let values = index
                    .index_info
                    .columns
                    .iter()
                    .map(|column| row.get(column.offset).cloned())
                    .collect::<Option<Vec<_>>>()
                    .ok_or(TableError::InvalidColumnOffset(row.len()))?;
                index
                    .gen_index_key(&values, handle)
                    .map(|pair| Some(pair.0))
                    .map_err(|error| TableError::IndexCondition(format!("{error:?}")))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for key in keys {
            if let Some(key) = key {
                self.index_entries.remove(&key);
            }
        }
        Ok(())
    }
}

/// 查找主键索引定义。
pub fn find_primary_index(indices: &[IndexInfo]) -> Option<&IndexInfo> {
    indices.iter().find(|index| index.primary)
}

/// 取公共主键（common handle）列 ID 列表。
pub fn try_get_common_pk_column_ids(table: &TableInfo, indices: &[IndexInfo]) -> Vec<i64> {
    find_primary_index(indices)
        .map(|index| {
            index
                .columns
                .iter()
                .filter_map(|column| table.columns.get(column.offset).map(|info| info.id))
                .collect()
        })
        .unwrap_or_default()
}

/// 取带前缀长度的主键列 ID（用于前缀索引还原）。
pub fn primary_prefix_column_ids(table: &TableInfo, indices: &[IndexInfo]) -> Vec<i64> {
    find_primary_index(indices)
        .map(|index| {
            index
                .columns
                .iter()
                .filter(|column| column.length.is_some())
                .filter_map(|column| table.columns.get(column.offset).map(|info| info.id))
                .collect()
        })
        .unwrap_or_default()
}

/// 按单列名查找 Public 状态索引。
pub fn find_index_by_column_name<'a>(indices: &'a [Index], name: &str) -> Option<&'a Index> {
    indices.iter().find(|index| {
        index.index_info.state == SchemaState::Public
            && index.index_info.columns.len() == 1
            && index.index_info.columns[0].name.eq_ignore_ascii_case(name)
    })
}

/// 判断 record_id 是否占用了分片（shard）位，导致句柄空间溢出。
pub fn overflow_shard_bits(
    record_id: i64,
    shard_row_id_bits: u64,
    type_bits_length: u64,
    reserved_sign_bit: bool,
) -> bool {
    if shard_row_id_bits == 0 || shard_row_id_bits >= type_bits_length {
        return false;
    }
    // 检查高位分片字段是否非零，表示 row id 已侵入分片位。
    let sign_bit = u64::from(reserved_sign_bit);
    let shift = type_bits_length - shard_row_id_bits - sign_bit;
    let mask = ((1_u128 << shard_row_id_bits) - 1) << shift;
    (record_id as u64 as u128) & mask != 0
}

/// 写行时可否跳过该列：主键/公共句柄完整列、无默认 NULL、或虚拟生成列。
pub fn can_skip(column: &Column, value: &Datum, primary_index: Option<&IndexInfo>) -> bool {
    can_skip_with_collation(true, column, value, primary_index)
}

pub fn can_skip_with_collation(
    use_new_collation: bool,
    column: &Column,
    value: &Datum,
    primary_index: Option<&IndexInfo>,
) -> bool {
    // 主键列、完整公共句柄列、无默认的 NULL、以及虚拟生成列可跳过存储。
    if column.primary_key {
        return true;
    }
    if column.common_handle
        && primary_index.is_some_and(|index| {
            index.columns.iter().any(|index_column| {
                index_column.offset == column.offset
                    && index_column.length.is_none()
                    && (!use_new_collation || !column.info.needs_restored_data)
            })
        })
    {
        return true;
    }
    if column.default_value.is_none()
        && column.origin_default_value.is_none()
        && matches!(value, Datum::Null)
    {
        return true;
    }
    column.is_virtual_generated()
}

/// 按主键/二级索引前缀长度截断需还原的字节列数据。
pub fn try_truncate_restored_data(
    datum: &mut Datum,
    primary_length: Option<usize>,
    secondary_length: Option<usize>,
) {
    let target = match (primary_length, secondary_length) {
        (None, _) => None,
        (_, None) => None,
        (Some(primary), Some(secondary)) => Some(primary.max(secondary)),
    };
    if let (Some(length), Datum::Bytes(value)) = (target, datum) {
        value.truncate(length.min(value.len()));
    }
}

/// 二进制排序规则下，将尾部空格个数写入整型 Datum。
pub fn convert_datum_to_tail_space_count(datum: &mut Datum, binary_collation: bool) {
    if !binary_collation {
        return;
    }
    if let Datum::Bytes(value) = datum {
        let count = value.iter().rev().take_while(|byte| **byte == b' ').count();
        *datum = Datum::Int(count as i64);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 下推到存储层的列信息（protobuf 风格）：列 ID 与默认值字节。
pub struct PbColumnInfo {
    pub id: i64,
    pub default_value: Option<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 表扫描描述：表 ID、列、主键列及是否走 TiFlash。
pub struct TableScan {
    pub table_id: i64,
    pub columns: Vec<PbColumnInfo>,
    pub primary_column_ids: Vec<i64>,
    pub primary_prefix_column_ids: Vec<i64>,
    pub tiflash: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 分区表扫描：内嵌 TableScan，并标记是否 fast scan。
pub struct PartitionTableScan {
    pub scan: TableScan,
    pub fast_scan: bool,
}

/// 由元数据构造 TableScan（TiFlash 标志由调用方指定）。
pub fn build_table_scan(
    table: &TableInfo,
    columns: &[ColumnInfo],
    indices: &[IndexInfo],
    tiflash: bool,
) -> TableScan {
    TableScan {
        table_id: table.id,
        columns: columns
            .iter()
            .map(|column| PbColumnInfo {
                id: column.id,
                default_value: None,
            })
            .collect(),
        primary_column_ids: try_get_common_pk_column_ids(table, indices),
        primary_prefix_column_ids: primary_prefix_column_ids(table, indices),
        tiflash,
    }
}

/// 构造分区表扫描；内部固定 tiflash=true。
pub fn build_partition_table_scan(
    table: &TableInfo,
    columns: &[ColumnInfo],
    indices: &[IndexInfo],
    fast_scan: bool,
) -> PartitionTableScan {
    PartitionTableScan {
        scan: build_table_scan(table, columns, indices, true),
        fast_scan,
    }
}

/// 为 PB 列填充默认值：虚拟生成列先写 NULL 占位，存在 origin_default_value 时再覆盖。
pub fn set_pb_columns_default_value(
    pb_columns: &mut [PbColumnInfo],
    columns: &[Column],
) -> Result<(), TableError> {
    if pb_columns.len() != columns.len() {
        return Err(TableError::RowLength {
            expected: columns.len(),
            actual: pb_columns.len(),
        });
    }
    for (pb, column) in pb_columns.iter_mut().zip(columns) {
        if column.is_virtual_generated() {
            pb.default_value = Some(vec![0]);
        }
        if let Some(value) = &column.origin_default_value {
            pb.default_value = Some(encode_default(value));
        }
    }
    Ok(())
}

/// 将 Datum 编码为默认值字节（带类型标签）。
fn encode_default(value: &Datum) -> Vec<u8> {
    match value {
        Datum::Null => vec![0],
        Datum::Int(value) => {
            let mut result = vec![1];
            result.extend_from_slice(&value.to_be_bytes());
            result
        }
        Datum::Uint(value) => {
            let mut result = vec![2];
            result.extend_from_slice(&value.to_be_bytes());
            result
        }
        Datum::Bytes(value) => {
            let mut result = vec![3];
            result.extend_from_slice(value);
            result
        }
    }
}

/// 临时表：原子标记是否已修改及估算大小，并持有元数据。
pub struct TemporaryTable {
    modified: AtomicBool,
    size: AtomicI64,
    meta: TableInfo,
}

impl TemporaryTable {
    /// 由元数据构造未修改、大小为 0 的临时表。
    pub fn new(meta: TableInfo) -> Self {
        Self {
            modified: AtomicBool::new(false),
            size: AtomicI64::new(0),
            meta,
        }
    }

    /// 设置是否已修改标志。
    pub fn set_modified(&self, modified: bool) {
        self.modified.store(modified, Ordering::Release);
    }

    /// 读取是否已修改。
    pub fn modified(&self) -> bool {
        self.modified.load(Ordering::Acquire)
    }

    /// 读取估算大小。
    pub fn size(&self) -> i64 {
        self.size.load(Ordering::Acquire)
    }

    /// 写入估算大小。
    pub fn set_size(&self, size: i64) {
        self.size.store(size, Ordering::Release);
    }

    /// 返回临时表元数据。
    pub fn meta(&self) -> &TableInfo {
        &self.meta
    }
}

// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// INFORMATION_SCHEMA 内存表的谓词抽取器。
//
// 从 WHERE 条件中提取表名、库名、索引名等等值/IN/LIKE 过滤，
// 写入 `ColPredicates` / `LikePatterns`，以便跳过无关元数据请求；
// `SkipRequest` 表示过滤后空集、无需再访问底层。
// InfoSchema 即信息模式，暴露库表列索引等元数据的系统视图。

use crate::{Predicate, PredicateValue};
use model_dependency::{ColumnInfo, TableInfo};
use parser_ast_dependency::CIStr;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

pub const TableSchema: &str = "table_schema";
pub const TableName: &str = "table_name";
pub const TidbTableID: &str = "tidb_table_id";
pub const PartitionName: &str = "partition_name";
pub const TidbPartitionID: &str = "tidb_partition_id";
pub const IndexName: &str = "index_name";
pub const SchemaName: &str = "schema_name";
pub const DBName: &str = "db_name";
pub const ConstraintSchema: &str = "constraint_schema";
pub const ConstraintName: &str = "constraint_name";
pub const TableID: &str = "table_id";
pub const SequenceSchema: &str = "sequence_schema";
pub const SequenceName: &str = "sequence_name";
pub const ColumnName: &str = "column_name";
pub const DDLStateName: &str = "state";

fn predicate_value_as_string(value: &PredicateValue) -> String {
    match value {
        PredicateValue::String(value) => value.to_lowercase(),
        PredicateValue::I64(value) => value.to_string(),
        PredicateValue::U64(value) => value.to_string(),
        PredicateValue::F64(value) => value.to_string(),
        PredicateValue::Bool(value) => u8::from(*value).to_string(),
    }
}

fn like_matches(pattern: &str, value: &str, escape: u8) -> bool {
    let (characters, kinds) = stringutil_dependency::string_util::CompilePattern(pattern, escape);
    // Compile before folding: an uppercase escape byte must still escape its
    // original pattern, as in Go's CompileLike2Regexp followed by ToLower.
    let mut folded = Vec::new();
    let mut folded_kinds = Vec::new();
    for (character, kind) in characters.into_iter().zip(kinds) {
        for character in character.to_lowercase() {
            folded.push(character);
            folded_kinds.push(kind);
        }
    }
    stringutil_dependency::string_util::DoMatch(&value.to_lowercase(), &folded, &folded_kinds)
}

#[derive(Clone, Debug, Default, PartialEq)]
/// InfoSchema 抽取器公共状态：列谓词集合、LIKE 模式与是否跳过请求。
pub struct InfoSchemaBaseExtractor {
    /// 列名 -> 允许取值集合（多次谓词取交集）。
    pub ColPredicates: BTreeMap<String, BTreeSet<String>>,
    /// 列名 -> LIKE 模式列表。
    pub LikePatterns: BTreeMap<String, Vec<String>>,
    /// Original patterns compiled before case folding, preserving ESCAPE semantics.
    pub LikeMatchPatterns: BTreeMap<String, Vec<String>>,
    /// Plan-time escape bytes aligned with each displayed LIKE pattern.
    pub LikeEscapes: BTreeMap<String, Vec<u8>>,
    /// 谓词交集为空时为 true，调用方可直接跳过远程/底层请求。
    pub SkipRequest: bool,
}

impl InfoSchemaBaseExtractor {
    /// 返回自身基类引用（统一接口）。
    pub fn GetBase(&self) -> &InfoSchemaBaseExtractor {
        self
    }
    /// 抽取 `columns` 白名单内的等值/IN/LIKE 谓词，返回未消费的剩余谓词。
    pub fn ExtractPredicates(
        &mut self,
        predicates: &[Predicate],
        columns: &[&str],
    ) -> Vec<Predicate> {
        self.ColPredicates.clear();
        self.LikePatterns.clear();
        self.LikeEscapes.clear();
        self.LikeMatchPatterns.clear();
        self.SkipRequest = false;
        // 仅处理白名单列；其它谓词原样退回。
        let allowed = columns
            .iter()
            .map(|column| column.to_lowercase())
            .collect::<BTreeSet<_>>();
        let mut initialized = BTreeSet::new();
        let mut remaining = Vec::new();
        for predicate in predicates {
            match predicate {
                Predicate::Like(field, pattern) if allowed.contains(&field.to_lowercase()) => {
                    self.push_like(field, pattern, b'\\');
                    remaining.push(predicate.clone());
                }
                Predicate::LikeWithEscape(field, pattern, crate::LikeEscape::Constant(escape))
                    if allowed.contains(&field.to_lowercase()) =>
                {
                    self.push_like(field, pattern, *escape);
                    remaining.push(predicate.clone());
                }
                Predicate::Ilike(field, pattern, crate::LikeEscape::Constant(escape))
                    if allowed.contains(&field.to_lowercase()) =>
                {
                    self.push_like(field, pattern, *escape);
                }
                Predicate::Eq(field, value) if allowed.contains(&field.to_lowercase()) => {
                    // 等值与 IN 合并进 ColPredicates，同列多次条件取交集。
                    self.merge(
                        field,
                        [predicate_value_as_string(value)].into_iter().collect(),
                        &mut initialized,
                    );
                }
                Predicate::In(field, values) if allowed.contains(&field.to_lowercase()) => {
                    let values = values
                        .iter()
                        .map(predicate_value_as_string)
                        .collect::<BTreeSet<_>>();
                    self.merge(field, values, &mut initialized);
                }
                _ => remaining.push(predicate.clone()),
            }
        }
        // 任一已初始化列的允许集为空 => 不可能命中，跳过请求。
        self.SkipRequest = initialized.iter().any(|field| {
            self.ColPredicates
                .get(field)
                .is_some_and(BTreeSet::is_empty)
        });
        remaining
    }
    fn push_like(&mut self, field: &str, pattern: &str, escape: u8) {
        let field = field.to_lowercase();
        self.LikePatterns
            .entry(field.clone())
            .or_default()
            .push(pattern.to_lowercase());
        self.LikeMatchPatterns
            .entry(field.clone())
            .or_default()
            .push(pattern.to_owned());
        self.LikeEscapes.entry(field).or_default().push(escape);
    }

    /// 将新取值并入列过滤集：首次赋值，再次则求交集。
    fn merge(&mut self, field: &str, values: BTreeSet<String>, initialized: &mut BTreeSet<String>) {
        let field = field.to_lowercase();
        let target = self.ColPredicates.entry(field.clone()).or_default();
        if initialized.contains(&field) {
            *target = target.intersection(&values).cloned().collect();
        } else {
            *target = values;
            initialized.insert(field);
        }
    }
    /// 生成 Explain 可读的谓词摘要。
    pub fn ExplainInfo(&self) -> String {
        if self.SkipRequest {
            return "skip_request:true".to_owned();
        }
        let mut parts = Vec::new();
        for (column, values) in &self.ColPredicates {
            if !values.is_empty() {
                parts.push(format!(
                    "{column}:[{}]",
                    values.iter().cloned().collect::<Vec<_>>().join(",")
                ));
            }
        }
        for (column, patterns) in &self.LikePatterns {
            if !patterns.is_empty() {
                parts.push(format!("{column}_pattern:[{}]", patterns.join(",")));
            }
        }
        parts.join(", ")
    }
    /// 判断字段是否允许给定值；无该列谓词时视为全部允许。
    pub fn Has(&self, field: &str, value: &str) -> bool {
        if self.SkipRequest {
            return false;
        }
        let field = field.to_lowercase();
        self.ColPredicates
            .get(&field)
            .is_none_or(|values| values.contains(&value.to_lowercase()))
            && self.LikePatterns.get(&field).is_none_or(|patterns| {
                patterns.iter().enumerate().all(|(index, pattern)| {
                    let escape = self
                        .LikeEscapes
                        .get(&field)
                        .and_then(|escapes| escapes.get(index))
                        .copied()
                        .unwrap_or(b'\\');
                    let original = self
                        .LikeMatchPatterns
                        .get(&field)
                        .and_then(|patterns| patterns.get(index))
                        .map_or(pattern.as_str(), String::as_str);
                    like_matches(original, value, escape)
                })
            })
    }

    /// 从候选名称中应用等值/IN/LIKE 过滤并按小写名排序。
    pub fn ListSchemas(&self, field: &str, candidates: &[CIStr]) -> Vec<CIStr> {
        let mut result = candidates
            .iter()
            .filter(|candidate| self.Has(field, &candidate.O))
            .cloned()
            .collect::<Vec<_>>();
        result.sort_by(|left, right| left.L.cmp(&right.L));
        result
    }
}

/// 将 schema 名与表元数据作为对齐配对排序。
/// Sorts schema names and table metadata as aligned pairs.
///
/// 对应 Go 的 `schemaTableSorter`：比较无副作用，
/// This mirrors Go's `schemaTableSorter`: comparisons are side-effect free,
/// 每次交换同时作用于两个切片，保证 `schemas[i]` 始终描述
/// while every swap is applied to both slices so `schemas[i]` always describes
/// `tables[i]`。
/// `tables[i]`.
pub(crate) struct SchemaTableSorter<'a> {
    schemas: &'a mut [CIStr],
    tables: &'a mut [TableInfo],
}

impl<'a> SchemaTableSorter<'a> {
    /// 构造排序器；schema 与 tables 长度必须一致。
    pub(crate) fn new(
        schemas: &'a mut [CIStr],
        tables: &'a mut [TableInfo],
    ) -> Result<Self, String> {
        if schemas.len() != tables.len() {
            return Err(format!(
                "schema/table length mismatch: {} schemas, {} tables",
                schemas.len(),
                tables.len()
            ));
        }
        Ok(Self { schemas, tables })
    }

    /// 元素个数。
    pub(crate) fn Len(&self) -> usize {
        self.schemas.len()
    }

    /// 先比 schema 小写名，再比表名。
    pub(crate) fn Less(&self, i: usize, j: usize) -> bool {
        match self.schemas[i].L.cmp(&self.schemas[j].L) {
            Ordering::Equal => self.tables[i].Name.L < self.tables[j].Name.L,
            Ordering::Less => true,
            Ordering::Greater => false,
        }
    }

    /// 同步交换 schema 与 table 配对。
    pub(crate) fn Swap(&mut self, i: usize, j: usize) {
        self.schemas.swap(i, j);
        self.tables.swap(i, j);
    }

    /// 按稳定置换将配对排序到目标顺序。
    pub(crate) fn sort(&mut self) {
        // 计算目标排列后，用置换把两切片同步摆到正确位置。
        let mut sorted_old_positions = (0..self.Len()).collect::<Vec<_>>();
        sorted_old_positions.sort_by(|&i, &j| {
            self.schemas[i]
                .L
                .cmp(&self.schemas[j].L)
                .then_with(|| self.tables[i].Name.L.cmp(&self.tables[j].Name.L))
        });

        let mut old_at_position = (0..self.Len()).collect::<Vec<_>>();
        let mut position_of_old = (0..self.Len()).collect::<Vec<_>>();
        for new_position in 0..self.Len() {
            let desired_old_position = sorted_old_positions[new_position];
            let current_position = position_of_old[desired_old_position];
            if current_position == new_position {
                continue;
            }
            self.Swap(new_position, current_position);
            old_at_position.swap(new_position, current_position);
            position_of_old[old_at_position[new_position]] = new_position;
            position_of_old[old_at_position[current_position]] = current_position;
        }
    }
}

/// 生成嵌入 `InfoSchemaBaseExtractor` 的具体抽取器类型及构造/抽取方法。
macro_rules! base_extractor {
    ($name:ident,$constructor:ident,[$($column:expr),* $(,)?]) => {
        #[derive(Clone,Debug,Default,PartialEq)] pub struct $name{pub Base:InfoSchemaBaseExtractor}
        impl $name{
            pub fn $constructor()->Self{Self::default()}
            pub fn GetBase(&self)->&InfoSchemaBaseExtractor{&self.Base}
            pub fn ExtractPredicates(&mut self,p:&[Predicate])->Vec<Predicate>{self.Base.ExtractPredicates(p,&[$($column),*])}
        }
    };
}

base_extractor!(
    // 各 INFORMATION_SCHEMA 视图对应的专用抽取器（列白名单不同）。
    InfoSchemaIndexesExtractor,
    NewInfoSchemaIndexesExtractor,
    [TableSchema, TableName]
);
base_extractor!(
    InfoSchemaTablesExtractor,
    NewInfoSchemaTablesExtractor,
    [TableSchema, TableName, TidbTableID]
);
/// TABLES 视图：按 table_name / table_schema 过滤。
impl InfoSchemaTablesExtractor {
    /// 是否允许给定表名。
    pub fn HasTableName(&self, name: &str) -> bool {
        self.Base.Has("table_name", name)
    }
    /// 是否允许给定库名。
    pub fn HasTableSchema(&self, name: &str) -> bool {
        self.Base.Has("table_schema", name)
    }

    /// 过滤并排序 schema/table 配对，保持两个元数据始终对齐。
    pub fn ListSchemasAndTables(
        &self,
        candidates: &[(CIStr, TableInfo)],
    ) -> (Vec<CIStr>, Vec<TableInfo>) {
        let mut pairs = candidates
            .iter()
            .filter(|(schema, table)| {
                self.HasTableSchema(&schema.O)
                    && self.HasTableName(&table.Name.O)
                    && self.Base.Has(TidbTableID, &table.ID.to_string())
            })
            .cloned()
            .collect::<Vec<_>>();
        pairs.sort_by(|left, right| {
            left.0
                .L
                .cmp(&right.0.L)
                .then_with(|| left.1.Name.L.cmp(&right.1.Name.L))
        });
        pairs.into_iter().unzip()
    }
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InfoSchemaDDLExtractor {
    pub Base: InfoSchemaBaseExtractor,
}
impl InfoSchemaDDLExtractor {
    pub fn NewInfoSchemaDDLExtractor() -> Self {
        Self::default()
    }
    pub fn GetBase(&self) -> &InfoSchemaBaseExtractor {
        &self.Base
    }
    /// DDL 读取器会用抽取结果裁剪历史任务，但 Selection 仍需保留原谓词。
    pub fn ExtractPredicates(&mut self, predicates: &[Predicate]) -> Vec<Predicate> {
        self.Base
            .ExtractPredicates(predicates, &[DBName, TableName, DDLStateName]);
        predicates.to_vec()
    }
}
base_extractor!(
    InfoSchemaViewsExtractor,
    NewInfoSchemaViewsExtractor,
    [TableSchema, TableName]
);
base_extractor!(
    InfoSchemaKeyColumnUsageExtractor,
    NewInfoSchemaKeyColumnUsageExtractor,
    [TableSchema, TableName, ConstraintName, ConstraintSchema]
);
/// KEY_COLUMN_USAGE：约束名/库名过滤。
impl InfoSchemaKeyColumnUsageExtractor {
    /// 是否允许给定约束名。
    pub fn HasConstraint(&self, name: &str) -> bool {
        self.Base.Has("constraint_name", name)
    }
    /// 是否包含 PRIMARY 约束过滤。
    pub fn HasPrimaryKey(&self) -> bool {
        self.HasConstraint("primary")
    }
    /// 是否允许给定约束所在库名。
    pub fn HasConstraintSchema(&self, name: &str) -> bool {
        self.Base.Has("constraint_schema", name)
    }
}
base_extractor!(
    InfoSchemaTableConstraintsExtractor,
    NewInfoSchemaTableConstraintsExtractor,
    [TableSchema, TableName, ConstraintName, ConstraintSchema]
);
/// TABLE_CONSTRAINTS：约束名/库名过滤。
impl InfoSchemaTableConstraintsExtractor {
    /// 是否允许给定约束名。
    pub fn HasConstraint(&self, name: &str) -> bool {
        self.Base.Has("constraint_name", name)
    }
    /// 是否包含 PRIMARY 约束过滤。
    pub fn HasPrimaryKey(&self) -> bool {
        self.HasConstraint("primary")
    }
    /// 是否允许给定约束所在库名。
    pub fn HasConstraintSchema(&self, name: &str) -> bool {
        self.Base.Has("constraint_schema", name)
    }
}
base_extractor!(
    InfoSchemaPartitionsExtractor,
    NewInfoSchemaPartitionsExtractor,
    [TableSchema, TableName, TidbPartitionID, PartitionName]
);
/// PARTITIONS：分区名与 tidb_partition_id 过滤。
impl InfoSchemaPartitionsExtractor {
    /// 是否允许给定分区名。
    pub fn HasPartition(&self, name: &str) -> bool {
        self.Base.Has("partition_name", name)
    }
    /// 是否存在 partition_name 谓词。
    pub fn HasPartitionPred(&self) -> bool {
        self.Base
            .ColPredicates
            .get(PartitionName)
            .is_some_and(|values| !values.is_empty())
    }
    /// 是否允许给定分区 ID。
    pub fn HasPartitionID(&self, id: i64) -> bool {
        self.Base.Has("tidb_partition_id", &id.to_string())
    }
    /// 是否存在 tidb_partition_id 谓词。
    pub fn HasPartitionIDPred(&self) -> bool {
        self.Base
            .ColPredicates
            .get(TidbPartitionID)
            .is_some_and(|values| !values.is_empty())
    }
}
base_extractor!(
    InfoSchemaStatisticsExtractor,
    NewInfoSchemaStatisticsExtractor,
    [TableSchema, TableName, IndexName]
);
/// STATISTICS：索引名过滤。
impl InfoSchemaStatisticsExtractor {
    /// 是否允许给定索引名。
    pub fn HasIndex(&self, name: &str) -> bool {
        self.Base.Has("index_name", name)
    }
    /// 是否包含 PRIMARY 约束过滤。
    pub fn HasPrimaryKey(&self) -> bool {
        self.HasIndex("primary")
    }
}
base_extractor!(
    InfoSchemaSchemataExtractor,
    NewInfoSchemaSchemataExtractor,
    [SchemaName]
);
base_extractor!(
    InfoSchemaCheckConstraintsExtractor,
    NewInfoSchemaCheckConstraintsExtractor,
    [ConstraintSchema, ConstraintName]
);
/// CHECK_CONSTRAINTS：约束名过滤。
impl InfoSchemaCheckConstraintsExtractor {
    /// 是否允许给定约束名。
    pub fn HasConstraint(&self, name: &str) -> bool {
        self.Base.Has("constraint_name", name)
    }
}
base_extractor!(
    InfoSchemaTiDBCheckConstraintsExtractor,
    NewInfoSchemaTiDBCheckConstraintsExtractor,
    [ConstraintSchema, TableName, TableID, ConstraintName]
);
/// TIDB_CHECK_CONSTRAINTS：约束名过滤。
impl InfoSchemaTiDBCheckConstraintsExtractor {
    /// 是否允许给定约束名。
    pub fn HasConstraint(&self, name: &str) -> bool {
        self.Base.Has("constraint_name", name)
    }
}
base_extractor!(
    InfoSchemaReferConstExtractor,
    NewInfoSchemaReferConstExtractor,
    [ConstraintSchema, TableName, ConstraintName]
);
/// REFERENTIAL_CONSTRAINTS：约束名过滤。
impl InfoSchemaReferConstExtractor {
    /// 是否允许给定约束名。
    pub fn HasConstraint(&self, name: &str) -> bool {
        self.Base.Has("constraint_name", name)
    }
}
base_extractor!(
    InfoSchemaSequenceExtractor,
    NewInfoSchemaSequenceExtractor,
    [SequenceSchema, SequenceName]
);
base_extractor!(
    InfoSchemaColumnsExtractor,
    NewInfoSchemaColumnsExtractor,
    [TableSchema, TableName, ColumnName]
);
impl InfoSchemaColumnsExtractor {
    /// 过滤指定 schema 下候选表，并按表名排序。
    pub fn ListTables(&self, candidates: &[TableInfo]) -> Vec<TableInfo> {
        let mut tables = candidates
            .iter()
            .filter(|table| self.Base.Has(TableName, &table.Name.O))
            .cloned()
            .collect::<Vec<_>>();
        tables.sort_by(|left, right| left.Name.L.cmp(&right.Name.L));
        tables
    }

    /// 返回可见且命中列谓词的列，以及以可见列为基准的一起始 ordinal。
    pub fn ListColumns<'a>(&self, table: &'a TableInfo) -> (Vec<&'a ColumnInfo>, Vec<usize>) {
        let mut columns = Vec::new();
        let mut ordinal_positions = Vec::new();
        let mut ordinal = 0;
        for column in &table.Columns {
            if column.Hidden {
                continue;
            }
            ordinal += 1;
            if self.Base.Has(ColumnName, &column.Name.O) {
                columns.push(column);
                ordinal_positions.push(ordinal);
            }
        }
        (columns, ordinal_positions)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// `tidb_index_usage` 只需索引小写名与 ID。
pub struct IndexUsageIndexInfo {
    pub Name: String,
    pub ID: i64,
}

base_extractor!(
    InfoSchemaTiDBIndexUsageExtractor,
    NewInfoSchemaTiDBIndexUsageExtractor,
    [TableSchema, TableName, IndexName]
);
impl InfoSchemaTiDBIndexUsageExtractor {
    /// 返回主键句柄与普通索引，并应用 index_name 的等值/IN/LIKE 过滤。
    pub fn ListIndexes(&self, table: &TableInfo) -> Vec<IndexUsageIndexInfo> {
        let mut indexes = Vec::with_capacity(table.Indices.len() + usize::from(table.PKIsHandle));
        if table.PKIsHandle && self.Base.Has(IndexName, "primary") {
            indexes.push(IndexUsageIndexInfo {
                Name: "primary".to_owned(),
                ID: 0,
            });
        }
        indexes.extend(
            table
                .Indices
                .iter()
                .filter(|index| self.Base.Has(IndexName, &index.Name.O))
                .map(|index| IndexUsageIndexInfo {
                    Name: index.Name.L.clone(),
                    ID: index.ID,
                }),
        );
        indexes
    }
}

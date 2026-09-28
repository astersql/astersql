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

// 索引顾问的数据模型与比较规则。
//
// 定义 workload 中的查询（Query）、可索引列（Column）、候选索引（Index）、
// 索引集合代价（IndexSetCost）以及对外推荐结果（Recommendation）。
// 名称在构造时统一小写，便于跨大小写比较与稳定键生成。

fn go_lowercase(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            character
                .to_lowercase()
                .next()
                .expect("lowercase mapping always contains a character")
        })
        .collect()
}

// 索引顾问的数据模型与比较规则。
//
// Query 表示待分析的 SQL 查询及其出现频率。
// #[derive(Clone)]
// pub struct Query { pub Alias: String, pub SchemaName: String, pub Text: String, pub Frequency: i32 }
// Key 返回查询文本；这是 Go Set[Query] 去重使用的稳定键。
// impl Query { pub fn Key(&self) -> String { self.Text.clone() } }
//
// Column 表示可用于索引的列。
// #[derive(Clone, PartialEq, Eq)]
// pub struct Column { pub SchemaName: String, pub TableName: String, pub ColumnName: String }
// NewColumn 按 Go 语义把 schema、table、column 统一转换为小写。
// pub fn NewColumn(schemaName: &str, tableName: &str, columnName: &str) -> Column { Column { SchemaName: schemaName.to_lowercase(), TableName: tableName.to_lowercase(), ColumnName: columnName.to_lowercase() } }
// NewColumns 批量构造列，并保持调用方传入的列顺序。
// pub fn NewColumns(schemaName: &str, tableName: &str, columnNames: &[&str]) -> Vec<Column> { columnNames.iter().map(|c| NewColumn(schemaName, tableName, c)).collect() }
// Key 返回 schema.table.column 形式的列键。
// impl Column { pub fn Key(&self) -> String { format!("{}.{}.{}", self.SchemaName, self.TableName, self.ColumnName) } }
//
// Index 表示一个候选索引及其有序列列表。
// #[derive(Clone, PartialEq, Eq)]
// pub struct Index { pub SchemaName: String, pub TableName: String, pub IndexName: String, pub Columns: Vec<Column> }
// NewIndex 创建单列或多列索引；列名规范化沿用 Go 的 NewColumns。
// pub fn NewIndex(schemaName: &str, tableName: &str, indexName: &str, columns: &[&str]) -> Index { Index { SchemaName: schemaName.to_lowercase(), TableName: tableName.to_lowercase(), IndexName: indexName.to_lowercase(), Columns: NewColumns(schemaName, tableName, columns) } }
// NewIndexWithColumns 从已有列推导表身份并构造索引，空列调用仍保留 Go 会 panic 的前置假设。
// pub fn NewIndexWithColumns(indexName: &str, columns: &[Column]) -> Index { let names: Vec<&str> = columns.iter().map(|c| c.ColumnName.as_str()).collect(); NewIndex(&columns[0].SchemaName, &columns[0].TableName, indexName, &names) }
// Key 返回稳定的 schema.table(column1,column2) 表示。
// impl Index { pub fn Key(&self) -> String { format!("{}.{}({})", self.SchemaName, self.TableName, self.Columns.iter().map(|c| c.ColumnName.as_str()).collect::<Vec<_>>().join(",")) } }
// PrefixContain 判断 j 是否为 i 的列前缀。
// pub fn PrefixContain(i: &Index, j: &Index) -> bool { i.SchemaName == j.SchemaName && i.TableName == j.TableName && i.Columns.len() >= j.Columns.len() && i.Columns.iter().zip(&j.Columns).all(|(a,b)| a.ColumnName == b.ColumnName) }
//
// IndexSetCost 表示索引集合对整个 workload 的估算成本。
// pub struct IndexSetCost { pub TotalWorkloadQueryCost: f64, pub TotalNumberOfIndexColumns: i32, pub IndexKeysStr: String }
// Less 保留 Go 的成本、列数、键字符串三级稳定排序规则。
// pub fn Less(c: &IndexSetCost, other: &IndexSetCost) -> bool {
//     if c.TotalWorkloadQueryCost == 0.0 { return false; }
//     if other.TotalWorkloadQueryCost == 0.0 { return true; }
//     let diff = (c.TotalWorkloadQueryCost - other.TotalWorkloadQueryCost).abs();
//     if diff > 10.0 && diff / c.TotalWorkloadQueryCost.max(other.TotalWorkloadQueryCost) > 0.001 { return c.TotalWorkloadQueryCost < other.TotalWorkloadQueryCost; }
//     if c.TotalNumberOfIndexColumns != other.TotalNumberOfIndexColumns { return c.TotalNumberOfIndexColumns < other.TotalNumberOfIndexColumns; }
//     c.IndexKeysStr < other.IndexKeysStr
// }
//
// ImpactedQuery 表示一个查询受推荐索引影响后的收益。
// pub struct ImpactedQuery { pub Query: String, pub Improvement: f64 }
// WorkloadImpact 表示整体 workload 的改善比例。
// pub struct WorkloadImpact { pub WorkloadImprovement: f64 }
// IndexDetail 保存推荐原因与估算索引大小（字节）。
// pub struct IndexDetail { pub Reason: String, pub IndexSize: u64 }
// Recommendation 是索引顾问对外返回的结果模型。
// pub struct Recommendation { pub Database: String, pub Table: String, pub IndexName: String, pub IndexColumns: Vec<String>, pub IndexDetail: Option<IndexDetail>, pub WorkloadImpact: Option<WorkloadImpact>, pub TopImpactedQueries: Vec<ImpactedQuery> }
// */
/// workload 中的一条查询：含别名、默认 schema、SQL 文本与出现频率。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Query {
    /// 查询别名，常用于存放 digest 指纹。
    pub alias: String,
    /// 默认数据库名（schema）。
    pub schema_name: String,
    /// SQL 文本（可已规范化）。
    pub text: String,
    /// 在 workload 中出现的次数，用于加权代价。
    pub frequency: usize,
}
impl Query {
    /// 返回用于集合去重的稳定键（即查询文本）。
    pub fn key(&self) -> &str {
        &self.text
    }
}

/// 可用于建索引的列，身份为 schema.table.column。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Column {
    /// 列所属库名。
    pub schema_name: String,
    /// 列所属表名。
    pub table_name: String,
    /// 列名。
    pub column_name: String,
}
impl Column {
    /// 构造列；schema/table/column 均按 Unicode 小写规则规范化以对齐 Go 语义。
    pub fn new(schema: &str, table: &str, column: &str) -> Self {
        Self {
            schema_name: go_lowercase(schema),
            table_name: go_lowercase(table),
            column_name: go_lowercase(column),
        }
    }
    /// 返回 `schema.table.column` 形式的列键。
    pub fn key(&self) -> String {
        format!(
            "{}.{}.{}",
            self.schema_name, self.table_name, self.column_name
        )
    }
}

/// 候选或已有二级索引：有序的列列表决定前缀匹配语义。
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Index {
    /// 索引所属库名。
    pub schema_name: String,
    /// 索引所属表名。
    pub table_name: String,
    /// 索引名。
    pub index_name: String,
    /// 索引列，顺序即复合索引从左到右的前缀顺序。
    pub columns: Vec<Column>,
}
impl Index {
    /// 由列名列表创建索引；名称与列均规范化为小写。
    pub fn new(
        schema: &str,
        table: &str,
        name: &str,
        columns: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Self {
        Self {
            schema_name: go_lowercase(schema),
            table_name: go_lowercase(table),
            index_name: go_lowercase(name),
            columns: columns
                .into_iter()
                .map(|column| Column::new(schema, table, column.as_ref()))
                .collect(),
        }
    }
    /// 从已有列构造索引；与 Go 一样使用首列的表身份重建全部列。
    pub fn with_columns(name: &str, columns: Vec<Column>) -> Result<Self, String> {
        let first = columns
            .first()
            .ok_or_else(|| "index requires at least one column".to_string())?;
        let schema_name = first.schema_name.clone();
        let table_name = first.table_name.clone();
        let column_names = columns.iter().map(|column| column.column_name.as_str());
        Ok(Self::new(&schema_name, &table_name, name, column_names))
    }
    /// 返回稳定键 `schema.table(col1,col2,...)`，用于集合比较与展示。
    pub fn key(&self) -> String {
        format!(
            "{}.{}({})",
            self.schema_name,
            self.table_name,
            self.columns
                .iter()
                .map(|column| column.column_name.as_str())
                .collect::<Vec<_>>()
                .join(",")
        )
    }
    /// 判断 `other` 是否为本索引的列前缀（同表且左侧列名序列一致）。
    pub fn prefix_contains(&self, other: &Self) -> bool {
        self.schema_name == other.schema_name
            && self.table_name == other.table_name
            && self.columns.len() >= other.columns.len()
            && self
                .columns
                .iter()
                .zip(&other.columns)
                .all(|(left, right)| left.column_name == right.column_name)
    }
}

/// 一组索引对整个 workload 的估算代价，用于候选集合择优。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IndexSetCost {
    /// 按频率加权后的总查询代价。
    pub total_workload_query_cost: f64,
    /// 集合内所有索引的列数之和；代价接近时偏好更窄索引。
    pub total_number_of_index_columns: usize,
    /// 排序后的索引键拼接串，作最终平局裁决。
    pub index_keys: String,
}
impl IndexSetCost {
    /// 是否优于 `other`：对齐 Go 的代价/列数/键字符串三级规则。
    pub fn less(&self, other: &Self) -> bool {
        // 代价为 0 视为无效，永远不优于对方。
        if self.total_workload_query_cost == 0.0 {
            return false;
        }
        if other.total_workload_query_cost == 0.0 {
            return true;
        }
        let delta = (self.total_workload_query_cost - other.total_workload_query_cost).abs();
        let max = self
            .total_workload_query_cost
            .max(other.total_workload_query_cost);
        // 绝对差与相对差均显著时，直接按代价比较。
        if delta > 10.0 && delta / max > 0.001 {
            return self.total_workload_query_cost < other.total_workload_query_cost;
        }
        if self.total_number_of_index_columns != other.total_number_of_index_columns {
            return self.total_number_of_index_columns < other.total_number_of_index_columns;
        }
        self.index_keys < other.index_keys
    }
}

/// 受推荐索引影响较大的单条查询及其改善比例。
#[derive(Clone, Debug, PartialEq)]
pub struct ImpactedQuery {
    /// 查询文本。
    pub query: String,
    /// 相对改善幅度（如代价下降比例）。
    pub improvement: f64,
}
/// 整个 workload 的整体改善指标。
#[derive(Clone, Debug, PartialEq)]
pub struct WorkloadImpact {
    /// workload 级别的改善比例。
    pub workload_improvement: f64,
}
/// 推荐索引的说明与估算大小。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexDetail {
    /// 推荐原因说明。
    pub reason: String,
    /// 估算索引占用字节数。
    pub index_size: u64,
}
/// 索引顾问对外返回的一条推荐结果。
#[derive(Clone, Debug, PartialEq)]
pub struct Recommendation {
    /// 目标库名。
    pub database: String,
    /// 目标表名。
    pub table: String,
    /// 建议的索引名。
    pub index_name: String,
    /// 建议的索引列名列表。
    pub index_columns: Vec<String>,
    /// 可选的原因与大小明细。
    pub index_detail: Option<IndexDetail>,
    /// 可选的整体 workload 影响。
    pub workload_impact: Option<WorkloadImpact>,
    /// 受益最大的若干查询。
    pub top_impacted_queries: Vec<ImpactedQuery>,
}

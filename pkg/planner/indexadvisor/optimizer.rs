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

// 索引顾问所需的 what-if 优化器接口与内存实现。
//
// what-if 优化指在不真正建索引的前提下，假设存在一组假设索引（hypothetical
// indexes），再估算执行计划代价。本文件提供 `Optimizer` trait 与可注入代价
// 回调的 `InMemoryOptimizer`，供算法层枚举候选时调用。

// 索引顾问所需的 what-if optimizer 接口和信息架构访问。
//
// QueryPlanCostHook 对应 Go 的全局回调，用于避免 indexadvisor 与 planner 之间的循环依赖。
// pub static mut QueryPlanCostHook: Option<fn(&SessionContext, Statement) -> Result<f64, Error>> = None;
//
// Optimizer 是 what-if 优化器接口；Go 明确说明它不是线程安全对象。
// pub trait Optimizer {
// ColumnType 返回指定列的类型。
//     fn ColumnType(&self, c: Column) -> Result<FieldType, Error>;
// PrefixContainIndex 判断候选索引是否已被现有索引覆盖。
//     fn PrefixContainIndex(&self, idx: Index) -> Result<bool, Error>;
// PossibleColumns 返回匹配列名的所有可能列。
//     fn PossibleColumns(&self, schema: &str, col_name: &str) -> Result<Vec<Column>, Error>;
// TableColumns 返回指定表的所有列。
//     fn TableColumns(&self, schema: &str, table: &str) -> Result<Vec<Column>, Error>;
// IndexNameExist 判断表内索引名是否存在。
//     fn IndexNameExist(&self, schema: &str, table: &str, index_name: &str) -> Result<bool, Error>;
// EstIndexSize 估算索引占用空间；列统计缺失时使用实时行数回退值。
//     fn EstIndexSize(&self, db: &str, table: &str, cols: &[String]) -> Result<f64, Error>;
// QueryPlanCost 在假设索引集合下计算查询计划成本。
//     fn QueryPlanCost(&self, sql: &str, hypo_indexes: &[Index]) -> Result<f64, Error>;
// }
//
// OptimizerImpl 保存 session context；它只访问已提供的 schema/统计接口，不自行建立连接。
// #[derive(Clone)] pub struct OptimizerImpl { pub sctx: SessionContext }
// NewOptimizer 创建 what-if optimizer。
// pub fn NewOptimizer(sctx: SessionContext) -> OptimizerImpl { OptimizerImpl { sctx } }
// impl OptimizerImpl {
// is 对应 Go 的类型断言 GetLatestInfoSchema，保留“必须是完整 InfoSchema”的前置假设。
//     fn is(&self) -> InfoSchema { self.sctx.latest_info_schema() }
//
// IndexNameExist 查询表索引并按 Go 的大小写不敏感名称比较。
//     pub fn IndexNameExist(&self, schema: &str, table: &str, index_name: &str) -> Result<bool, Error> { let tbl = self.is().TableByName(schema, table)?; for idx in tbl.indices() { if idx.name_lower() == index_name { return Ok(true); } } Ok(false) }
//
// TableColumns 将表元数据逐列转换为 indexadvisor::Column，保持来源 schema/table。
//     pub fn TableColumns(&self, schema: &str, table: &str) -> Result<Vec<Column>, Error> { let tbl = self.is().TableByName(schema, table)?; Ok(tbl.columns().into_iter().map(|c| Column { SchemaName:schema.into(), TableName:table.into(), ColumnName:c.name_lower() }).collect()) }
//
// PossibleColumns 先过滤 memory/system schema，再扫描 schema 下的所有表。
//     pub fn PossibleColumns(&self, schema: &str, col_name: &str) -> Result<Vec<Column>, Error> { let schema = schema.to_lowercase(); if is_mem_db(&schema) || is_system_db(&schema) { return Ok(Vec::new()); } let mut out=Vec::new(); for tbl in self.is().SchemaTableInfos(&schema)? { for col in tbl.columns() { if col.name_lower() == col_name { out.push(Column { SchemaName:schema.clone(), TableName:tbl.name_lower(), ColumnName:col.name_lower() }); } } } Ok(out) }
//
// PrefixContainIndex 逐列比较现有索引的前缀；表不存在等错误原样向上传递。
//     pub fn PrefixContainIndex(&self, idx: Index) -> Result<bool, Error> { let tbl=self.is().TableByName(&idx.SchemaName,&idx.TableName)?; for existing in tbl.indices() { if existing.columns_len() < idx.Columns.len() { continue; } if idx.Columns.iter().enumerate().all(|(i,c)| existing.column_name(i)==c.ColumnName.to_lowercase()) { return Ok(true); } } Ok(false) }
//
// ColumnType 查找列类型；找不到时生成与 Go 相同的带表名错误。
//     pub fn ColumnType(&self, c: Column) -> Result<FieldType, Error> { let tbl=self.is().TableByName(&c.SchemaName,&c.TableName)?; for col in tbl.columns() { if col.name_lower()==c.ColumnName.to_lowercase() { return Ok(col.field_type()); } } Err(Error::Message(format!("column {} not found in table {}.{}",c.ColumnName,c.SchemaName,c.TableName))) }
//
// addHypoIndex 把候选列转换为假设 IndexInfo，并写入 session 的嵌套映射；这里只保留内存状态变更形状。
//     fn addHypoIndex(&mut self, indexes: &[Index]) -> Result<(), Error> { for h in indexes { let tbl=self.is().TableByName(&h.SchemaName,&h.TableName)?; let mut cols=Vec::new(); for col in &h.Columns { let offset=tbl.columns().iter().position(|c| c.name_lower()==col.ColumnName.to_lowercase()).ok_or_else(||Error::Message(format!("column {} not found in table {}.{}",col.ColumnName,h.SchemaName,h.TableName)))?; cols.push(IndexColumn { name:col.ColumnName.clone(), offset, length:UNSPECIFIED_LENGTH }); } self.sctx.put_hypo_index(h.clone(), IndexInfo { name:h.IndexName.clone(), columns:cols, state:StatePublic, index_type:IndexTypeHypo }); } Ok(()) }
//
// QueryPlanCost 保存并恢复 fix-control、warnings、假设索引和 explain 标志；这是 Go defer 的关键迁移点。
//     pub fn QueryPlanCost(&mut self, sql: &str, indexes: &[Index]) -> Result<f64, Error> { let stmt=ParseOneSQL(sql)?; let saved=self.sctx.snapshot(); self.sctx.enable_fix43817(); self.sctx.set_explain(true); self.sctx.clear_hypo_indexes(); let result=self.addHypoIndex(indexes).and_then(|_| unsafe { QueryPlanCostHook.ok_or(Error::Message("QueryPlanCostHook is nil".into()))?(&self.sctx,stmt) }); self.sctx.restore(saved); result }
//
// EstIndexSize 使用统计列大小；未加载统计时按 8 字节乘实时行数估算。
//     pub fn EstIndexSize(&self, db: &str, table: &str, cols: &[String]) -> Result<f64, Error> { let tbl=self.is().TableByName(db,table)?; let stats=GetStatsHandle(&self.sctx).physical_table_stats(tbl.id()); let mut size=0.0; for name in cols { size += stats.column_by_name(name).map(|c|c.total_size as f64).unwrap_or(8.0*stats.realtime_count as f64); } Ok(size) }
// }
//
// 所需的外部类型占位；不会执行 IO。
// #[derive(Clone)] pub struct SessionContext; impl SessionContext { fn latest_info_schema(&self)->InfoSchema{InfoSchema} fn snapshot(&self)->Snapshot{Snapshot} fn restore(&mut self,_:Snapshot){} fn enable_fix43817(&mut self){} fn set_explain(&mut self,_:bool){} fn clear_hypo_indexes(&mut self){} fn put_hypo_index(&mut self,_:Index,_:IndexInfo){} }
// pub struct InfoSchema; impl InfoSchema { fn TableByName(&self,_:&str,_:&str)->Result<Table,Error>{Ok(Table)} fn SchemaTableInfos(&self,_:&str)->Result<Vec<Table>,Error>{Ok(Vec::new())} } pub struct Table; impl Table {fn indices(&self)->Vec<TableIndex>{Vec::new()} fn columns(&self)->Vec<TableColumn>{Vec::new()} fn id(&self)->u64{0}}
// pub struct TableIndex; impl TableIndex{fn name_lower(&self)->String{String::new()} fn columns_len(&self)->usize{0} fn column_name(&self,_:usize)->String{String::new()}} pub struct TableColumn; impl TableColumn{fn name_lower(&self)->String{String::new()} fn field_type(&self)->FieldType{FieldType}}
// #[derive(Clone)] pub struct Column{pub SchemaName:String,pub TableName:String,pub ColumnName:String} #[derive(Clone)] pub struct Index{pub SchemaName:String,pub TableName:String,pub IndexName:String,pub Columns:Vec<Column>}
// pub struct FieldType; pub struct Statement; pub struct Snapshot; pub struct IndexColumn{pub name:String,pub offset:usize,pub length:i32} pub struct IndexInfo{pub name:String,pub columns:Vec<IndexColumn>,pub state:i32,pub index_type:i32} const StatePublic:i32=1; const IndexTypeHypo:i32=2; const UNSPECIFIED_LENGTH:i32=-1;
// pub enum Error{Message(String)} fn is_mem_db(_: &str)->bool{false} fn is_system_db(_: &str)->bool{false} fn ParseOneSQL(_: &str)->Result<Statement,Error>{Ok(Statement)} fn GetStatsHandle(_: &SessionContext)->Stats{Stats} pub struct Stats; impl Stats{fn physical_table_stats(&self,_:u64)->TableStats{TableStats}} pub struct TableStats{realtime_count:u64} impl TableStats{fn column_by_name(&self,_:&str)->Option<StatColumn>{None}} pub struct StatColumn{total_size:u64}
// */
use crate::model::{Column, Index};
use std::collections::BTreeMap;
use std::sync::Arc;

/// 简化的列字段类型，用于判断列是否适合建二级索引。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldType {
    Integer,
    Float,
    Decimal,
    DateTime,
    String,
    Bytes,
    Json,
    Blob,
    Geometry,
    Vector,
}

/// what-if 优化器接口：提供元数据查询与假设索引下的计划代价估算。
///
/// 对齐 Go 说明：实现一般不保证线程安全，调用方应串行使用。
pub trait Optimizer {
    /// 返回指定列的字段类型。
    fn column_type(&self, column: &Column) -> Result<FieldType, String>;
    /// 判断候选索引是否已被现有索引前缀覆盖。
    fn prefix_contain_index(&self, index: &Index) -> Result<bool, String>;
    /// 在给定 schema 下查找同名列可能归属的所有表列。
    fn possible_columns(&self, schema: &str, column: &str) -> Result<Vec<Column>, String>;
    /// 返回表的全部列。
    fn table_columns(&self, schema: &str, table: &str) -> Result<Vec<Column>, String>;
    /// 判断表内是否已存在同名索引。
    fn index_name_exists(&self, schema: &str, table: &str, name: &str) -> Result<bool, String>;
    /// 估算索引占用空间；缺统计时按行数 × 8 字节回退。
    fn estimate_index_size(
        &self,
        schema: &str,
        table: &str,
        columns: &[String],
    ) -> Result<f64, String>;
    /// 在假设索引集合下计算 SQL 的执行计划代价。
    fn query_plan_cost(&self, sql: &str, hypothetical_indexes: &[Index]) -> Result<f64, String>;
}

/// 内存中的表元数据：列类型、已有索引、行数与列总大小统计。
#[derive(Clone, Debug)]
pub struct TableMetadata {
    /// 列名 → 字段类型。
    pub columns: BTreeMap<String, FieldType>,
    /// 表上已有（或模拟）索引。
    pub indexes: Vec<Index>,
    /// 实时行数估算，用于缺统计时的索引大小回退。
    pub row_count: u64,
    /// 列名 → 该列存储总字节数。
    pub column_total_size: BTreeMap<String, u64>,
}
/// 代价回调：由外部注入真实 planner 代价或测试桩。
type CostHook = dyn Fn(&str, &[Index]) -> Result<f64, String> + Send + Sync;

/// 基于内存表元数据的 `Optimizer` 实现，便于单测与算法联调。
#[derive(Clone)]
pub struct InMemoryOptimizer {
    tables: BTreeMap<(String, String), TableMetadata>,
    cost_hook: Arc<CostHook>,
}

impl InMemoryOptimizer {
    /// 用表元数据映射与代价钩子构造优化器。
    pub fn new(
        tables: BTreeMap<(String, String), TableMetadata>,
        hook: impl Fn(&str, &[Index]) -> Result<f64, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            tables,
            cost_hook: Arc::new(hook),
        }
    }
    /// 按小写 schema.table 查找表元数据。
    fn table(&self, schema: &str, table: &str) -> Result<&TableMetadata, String> {
        self.tables
            .get(&(schema.to_lowercase(), table.to_lowercase()))
            .ok_or_else(|| format!("table {schema}.{table} not found"))
    }
}

impl Optimizer for InMemoryOptimizer {
    fn column_type(&self, column: &Column) -> Result<FieldType, String> {
        self.table(&column.schema_name, &column.table_name)?
            .columns
            .get(&column.column_name.to_lowercase())
            .cloned()
            .ok_or_else(|| format!("column {} not found", column.key()))
    }
    fn prefix_contain_index(&self, index: &Index) -> Result<bool, String> {
        let table = self.table(&index.schema_name, &index.table_name)?;
        Ok(table.indexes.iter().any(|existing| {
            existing.columns.len() >= index.columns.len()
                && existing
                    .columns
                    .iter()
                    .zip(&index.columns)
                    .all(|(left, right)| left.column_name == right.column_name.to_lowercase())
        }))
    }
    fn possible_columns(&self, schema: &str, column: &str) -> Result<Vec<Column>, String> {
        // 系统库不参与候选列扫描。
        if is_system_schema(schema) {
            return Ok(Vec::new());
        }
        let schema = schema.to_lowercase();
        Ok(self
            .tables
            .iter()
            .filter(|((db, _), table)| db == &schema && table.columns.contains_key(column))
            .map(|((db, table), _)| Column {
                schema_name: db.clone(),
                table_name: table.clone(),
                column_name: column.to_owned(),
            })
            .collect())
    }
    fn table_columns(&self, schema: &str, table: &str) -> Result<Vec<Column>, String> {
        Ok(self
            .table(schema, table)?
            .columns
            .keys()
            .map(|column| Column {
                schema_name: schema.to_owned(),
                table_name: table.to_owned(),
                column_name: column.clone(),
            })
            .collect())
    }
    fn index_name_exists(&self, schema: &str, table: &str, name: &str) -> Result<bool, String> {
        Ok(self
            .table(schema, table)?
            .indexes
            .iter()
            .any(|index| index.index_name == name))
    }
    fn estimate_index_size(
        &self,
        schema: &str,
        table: &str,
        columns: &[String],
    ) -> Result<f64, String> {
        let table = self.table(schema, table)?;
        // 缺列统计时用 row_count*8 作为 Go 侧实时行数回退。
        Ok(columns
            .iter()
            .map(|column| {
                table
                    .column_total_size
                    .get(column)
                    .copied()
                    .unwrap_or(table.row_count.saturating_mul(8)) as f64
            })
            .sum())
    }
    fn query_plan_cost(&self, sql: &str, indexes: &[Index]) -> Result<f64, String> {
        // 先校验假设索引列均存在，再交给代价钩子。
        for index in indexes {
            let table = self.table(&index.schema_name, &index.table_name)?;
            for column in &index.columns {
                if !table
                    .columns
                    .contains_key(&column.column_name.to_lowercase())
                {
                    return Err(format!("column {} not found", column.key()));
                }
            }
        }
        (self.cost_hook)(sql, indexes)
    }
}

/// 判断是否为系统/内存 schema，此类库不参与索引推荐。
fn is_system_schema(schema: &str) -> bool {
    matches!(
        schema.to_ascii_lowercase().as_str(),
        "mysql" | "information_schema" | "performance_schema" | "metrics_schema" | "sys"
    )
}

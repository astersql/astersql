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

// 索引顾问对外入口：从 SQL 文本构建 workload、调用推荐算法并生成建议结果。
//
// 流程：规范化/归并 SQL → 过滤系统表查询 → 收集可索引列 →
// [`advise_indexes`] → [`prepare_recommendations`]（估算改善幅度、索引大小、
// 受影响查询，并分配不冲突的索引名）。
//
// 文件中大块注释保留了 Go 版 AdviseIndexes 管线的机械翻译草稿，供对照。

// use std::time::Duration;
// use super::{Column, Index, IndexDetail, Optimizer, Query, Recommendation, WorkloadImpact};
//
// TestKey 生成测试上下文中 query set 的固定键。
// pub fn TestKey(key: &str) -> String { format!("__test_index_advisor_{}", key) }
//
// Option 对应 Go 的索引顾问参数；SpecifiedSQLs 为空时从 statement summary 取 workload。
// #[derive(Clone, Default)]
// pub struct Option { pub MaxNumIndexes: i32, pub MaxIndexWidth: i32, pub MaxNumQuery: i32, pub Timeout: Duration, pub SpecifiedSQLs: Vec<String> }
//
// AdviseIndexes 是外部入口：填充参数后进入带 option 的实际流程。
// pub fn AdviseIndexes(ctx: Context, sctx: SessionContext, user_sqls: Vec<String>, user_options: Vec<RecommendIndexOption>) -> Result<Vec<Recommendation>, Error> {
//     let mut option = Option { SpecifiedSQLs: user_sqls, ..Default::default() };
//     fillOption(&sctx, &mut option, user_options)?;
//     adviseIndexesWithOption(ctx, sctx, option)
// }
//
// adviseIndexesWithOption 保留 Go 的 nil 检查、panic recover、查询准备、候选识别和结果保存顺序。
// fn adviseIndexesWithOption(ctx: Context, sctx: SessionContext, option: Option) -> Result<Vec<Recommendation>, Error> {
//     if ctx.is_nil() || sctx.is_nil() { return Err(Error::Message("nil input".into())); }
// Go defer/recover 在 实现中以“错误边界”注释表示；真实模块接线时由调用层统一转换 panic。
//     let opt = NewOptimizer(sctx.clone());
//     let default_db = sctx.current_db();
//     let query_set = prepareQuerySet(ctx.clone(), sctx.clone(), default_db, opt.clone(), &option)?;
//     let indexable = CollectIndexableColumnsForQuerySet(opt.clone(), query_set.clone())?;
//     let (indexes, all_candidates) = adviseIndexes(query_set.clone(), indexable.clone(), opt.clone(), &option)?;
//     let mut results = prepareRecommendation(indexes, query_set, opt.clone())?;
//     saveRecommendations(sctx.clone(), &results);
//     if results.is_empty() { // Go 这里追加 warning，保留诊断信息而不改变推荐结果。
//         let _explanation = format!("Considered {} indexable columns, {} candidates", indexable.size(), all_candidates.size());
//         sctx.append_warning(_explanation);
//     }
//     Ok(std::mem::take(&mut results))
// }
//
// prepareQuerySet 优先使用用户 SQL，否则读取 statement summary，并依次过滤系统表、非法 SQL。
// fn prepareQuerySet(ctx: Context, sctx: SessionContext, default_db: String, opt: Optimizer, option: &Option) -> Result<Set<Query>, Error> {
//     let mut queries = Set::new();
//     if !option.SpecifiedSQLs.is_empty() { for sql in &option.SpecifiedSQLs { queries.add(Query { Alias:String::new(), SchemaName:default_db.clone(), Text:sql.clone(), Frequency:1 }); } }
//     else { queries = loadQuerySetFromStmtSummary(sctx.clone(), option)?; if queries.is_empty() { return Err(Error::Message("can't get any queries from statements_summary".into())); } }
// RestoreSchemaName、FilterSQLAccessingSystemTables、FilterInvalidQueries 都可能解析 SQL 并返回错误。
//     queries = RestoreSchemaName(default_db, queries, option.SpecifiedSQLs.is_empty())?;
//     queries = FilterSQLAccessingSystemTables(queries, option.SpecifiedSQLs.is_empty())?;
//     queries = FilterInvalidQueries(opt, queries, option.SpecifiedSQLs.is_empty())?;
//     if queries.is_empty() { return Err(Error::Message("empty query set after filtering invalid queries".into())); }
//     let _ = ctx; Ok(queries)
// }
//
// loadQuerySetFromStmtSummary 保留 Go 的聚合 SQL 文本和按执行次数排序/限制语义；exec 仅为尚未接通的占位。
// fn loadQuerySetFromStmtSummary(sctx: SessionContext, option: &Option) -> Result<Set<Query>, Error> {
//     let template = "SELECT any_value(schema_name), any_value(query_sample_text), sum(exec_count) FROM information_schema.statements_summary_history GROUP BY digest ORDER BY sum(exec_count) DESC LIMIT %?";
//     let rows = exec(sctx, template, vec![option.MaxNumQuery.to_string()])?;
//     let mut out = Set::new();
//     for row in rows { out.add(Query { Alias:String::new(), SchemaName:row.get(0), Text:row.get(1), Frequency:row.get(2).parse().unwrap_or_default() }); }
//     Ok(out)
// }
//
// prepareRecommendation 计算每个索引的 workload 改善、大小与受影响查询，并过滤无收益结果。
// fn prepareRecommendation(indexes: Set<Index>, queries: Set<Query>, optimizer: Optimizer) -> Result<Vec<Recommendation>, Error> {
//     let mut results = Vec::new();
//     for idx in indexes.to_list() {
//         let cols: Vec<String> = idx.Columns.iter().map(|c| c.ColumnName.trim_matches(['\'', '"', ' ']).to_string()).collect();
//         let before = evaluateWorkloadCost(queries.clone(), optimizer.clone(), Set::new())?;
//         let mut with_idx = Set::new(); with_idx.add(idx.clone());
//         let after = evaluateWorkloadCost(queries.clone(), optimizer.clone(), with_idx)?;
//         let improvement = round((before - after) / before, 6);
//         if improvement < 0.000001 { continue; }
//         let name = gracefulIndexName(optimizer.clone(), &idx.SchemaName, &idx.TableName, &cols);
//         results.push(Recommendation { Database:idx.SchemaName, Table:idx.TableName, IndexName:name, IndexColumns:cols, IndexDetail:Some(IndexDetail { Reason:"Equal or Range Predicate".into(), IndexSize:0 }), WorkloadImpact:Some(WorkloadImpact { WorkloadImprovement:improvement }), TopImpactedQueries:Vec::new() });
//     }
//     Ok(results)
// }
//
// round 保留 Go math.Round + 10^n 的显示精度规则。
// fn round(v: f64, n: i32) -> f64 { let p = 10_f64.powi(n); (v * p).round() / p }
// gracefulIndexName 依次尝试完整列名、首列名和 30 个编号，规避既有索引名。
// fn gracefulIndexName(opt: Optimizer, schema: &str, table: &str, cols: &[String]) -> String {
//     let mut name = format!("idx_{}", cols.join("_")); name.truncate(64);
//     if !opt.IndexNameExist(schema, table, &name.to_lowercase()).unwrap_or(false) { return name; }
//     name = format!("idx_{}", cols[0]); name.truncate(64);
//     if !opt.IndexNameExist(schema, table, &name.to_lowercase()).unwrap_or(false) { return name; }
//     for i in 0..30 { name = format!("idx_{}_{}", cols[0], i); name.truncate(64); if !opt.IndexNameExist(schema, table, &name.to_lowercase()).unwrap_or(false) { return name; } }
//     name
// }
//
// saveRecommendations 保留 JSON 序列化、INSERT ... ON DUPLICATE KEY UPDATE 的资源/错误处理形状。
// fn saveRecommendations(sctx: SessionContext, results: &[Recommendation]) { for r in results { let q = jsonMarshal(&r.TopImpactedQueries); let w = jsonMarshal(&r.WorkloadImpact); let d = jsonMarshal(&r.IndexDetail); if q.is_err() || w.is_err() || d.is_err() { continue; } let _ = exec(sctx.clone(), "insert into mysql.index_advisor_results ... on duplicate key update ...", vec![r.Database.clone(), r.Table.clone(), r.IndexName.clone()]); } }
//
// 外部依赖和跨文件函数以占位类型保留；它们不会产生真实 IO。
// #[derive(Clone)] pub struct Context; impl Context { fn is_nil(&self)->bool { false } }
// #[derive(Clone)] pub struct SessionContext; impl SessionContext { fn is_nil(&self)->bool { false } fn current_db(&self)->String { String::new() } fn append_warning(&self, _:String){} }
// pub struct RecommendIndexOption; pub enum Error { Message(String) }
// #[derive(Clone, Default)] pub struct Set<T>(Vec<T>); impl<T:Clone+PartialEq> Set<T>{ fn new()->Self{Self(Vec::new())} fn add(&mut self,v:T){if !self.0.contains(&v){self.0.push(v)}} fn to_list(&self)->Vec<T>{self.0.clone()} fn size(&self)->usize{self.0.len()} fn is_empty(&self)->bool{self.0.is_empty()} }
// fn fillOption(_: &SessionContext, _: &mut Option, _: Vec<RecommendIndexOption>)->Result<(),Error>{Ok(())}
// fn adviseIndexes(_:Set<Query>, _:Set<Column>, _:Optimizer, _: &Option)->Result<(Set<Index>,Set<Index>),Error>{Ok((Set::new(),Set::new()))}
// fn CollectIndexableColumnsForQuerySet(_:Optimizer, _:Set<Query>)->Result<Set<Column>,Error>{Ok(Set::new())}
// fn RestoreSchemaName(_:String,q:Set<Query>,_:bool)->Result<Set<Query>,Error>{Ok(q)} fn FilterSQLAccessingSystemTables(q:Set<Query>,_:bool)->Result<Set<Query>,Error>{Ok(q)} fn FilterInvalidQueries(_:Optimizer,q:Set<Query>,_:bool)->Result<Set<Query>,Error>{Ok(q)}
// fn exec(_:SessionContext, _: &str, _:Vec<String>)->Result<Vec<Row>,Error>{Ok(Vec::new())} struct Row; impl Row{fn get(&self,_:usize)->String{String::new()}}
// fn evaluateWorkloadCost(_:Set<Query>,_:Optimizer,_:Set<Index>)->Result<f64,Error>{Ok(0.0)} fn jsonMarshal<T>(_:&T)->Result<String,Error>{Ok(String::new())} fn NewOptimizer(_:SessionContext)->Optimizer{Optimizer}
// #[derive(Clone)] pub struct Optimizer; impl Optimizer{fn IndexNameExist(&self,_:&str,_:&str,_:&str)->Result<bool,Error>{Ok(false)}}
// */
use crate::algorithm::advise_indexes;
use crate::model::{ImpactedQuery, Index, IndexDetail, Query, Recommendation, WorkloadImpact};
use crate::optimizer::Optimizer;
use crate::options::AdvisorOptions;
use crate::utils::{
    collect_indexable_columns, filter_system_queries, normalize_digest, restore_schema_name,
};
use std::collections::{BTreeMap, BTreeSet};

/// 从用户提供的 SQL 列表生成索引推荐。
///
/// 按 digest 归并相同规范化 SQL 并累计频率，补全默认 schema、过滤系统表后，
/// 收集可索引列并调用算法，最后整理为 `Recommendation` 列表。
pub fn advise_indexes_for_sql(
    optimizer: &dyn Optimizer,
    sqls: &[String],
    default_schema: &str,
    options: &AdvisorOptions,
) -> Result<Vec<Recommendation>, String> {
    let mut grouped: BTreeMap<String, Query> = BTreeMap::new();
    // Go 仅对 statement summary 查询应用 MaxNumQuery；用户显式指定的 SQL 全部参与。
    for sql in sqls {
        let (normalized, digest) = normalize_digest(sql);
        grouped
            .entry(digest.clone())
            .and_modify(|query| query.frequency += 1)
            .or_insert(Query {
                alias: digest,
                schema_name: default_schema.to_ascii_lowercase(),
                text: normalized,
                frequency: 1,
            });
    }
    let queries = filter_system_queries(
        restore_schema_name(default_schema, grouped.into_values().collect(), true)?,
        true,
    )?;
    if queries.is_empty() {
        return Ok(Vec::new());
    }
    let mut columns = BTreeSet::new();
    for query in &queries {
        columns.extend(collect_indexable_columns(query, optimizer)?);
    }
    let indexes = advise_indexes(&queries, &columns, optimizer, options)?;
    prepare_recommendations(&indexes, &queries, optimizer)
}

/// 将推荐索引集合转为对外 `Recommendation`：整体/单查询改善、大小与命名。
pub fn prepare_recommendations(
    indexes: &BTreeSet<Index>,
    queries: &BTreeSet<Query>,
    optimizer: &dyn Optimizer,
) -> Result<Vec<Recommendation>, String> {
    let mut results = Vec::new();
    for index in indexes {
        // Go evaluates each recommendation independently: compare the plan
        // without this index with the plan using only this index.
        let columns = index
            .columns
            .iter()
            .map(|column| {
                column
                    .column_name
                    .trim_matches(['\'', '"', ' '])
                    .to_string()
            })
            .collect::<Vec<_>>();
        let before_indexes = Vec::new();
        let after_indexes = vec![index.clone()];
        // Go 在代价评估前生成名称并查询索引大小，因此元数据错误不能
        // 被后续的“无收益”过滤吞掉。
        let name = graceful_index_name(optimizer, &index.schema_name, &index.table_name, &columns)?;
        let size =
            optimizer.estimate_index_size(&index.schema_name, &index.table_name, &columns)? as u64;
        let mut impacted = Vec::new();
        let mut workload_before = 0.0;
        let mut workload_after = 0.0;
        for query in queries {
            let mut old = optimizer.query_plan_cost(&query.text, &before_indexes)?;
            let mut new = optimizer.query_plan_cost(&query.text, &after_indexes)?;
            if old == 0.0 {
                old += 0.1;
                new += 0.1;
            }
            workload_before += old * query.frequency as f64;
            workload_after += new * query.frequency as f64;
            let delta = round((old - new) / old, 6);
            if delta >= 0.0001 {
                impacted.push(ImpactedQuery {
                    query: query.text.clone(),
                    improvement: delta,
                });
            }
        }
        if workload_before == 0.0 {
            workload_before += 0.1;
            workload_after += 0.1;
        }
        let workload_improvement = round((workload_before - workload_after) / workload_before, 6);
        impacted.sort_by(|left, right| right.improvement.total_cmp(&left.improvement));
        impacted.truncate(3);
        if workload_improvement < 0.000001 || impacted.is_empty() {
            continue;
        }
        let normalized = normalize_digest(&impacted[0].query).0;
        let reason = format!(
            "Column [{}] appear in Equal or Range Predicate clause(s) in query: {}",
            columns.join(" "),
            normalized
        );
        results.push(Recommendation {
            database: index.schema_name.clone(),
            table: index.table_name.clone(),
            index_name: name,
            index_columns: columns,
            index_detail: Some(IndexDetail {
                reason,
                index_size: size,
            }),
            workload_impact: Some(WorkloadImpact {
                workload_improvement,
            }),
            top_impacted_queries: impacted,
        });
    }
    results.sort_by(|left, right| {
        (&left.database, &left.table, &left.index_name).cmp(&(
            &right.database,
            &right.table,
            &right.index_name,
        ))
    });
    Ok(results)
}

/// 按执行频率加权汇总 workload 在给定索引集下的总估算代价。
fn workload_cost(
    queries: &BTreeSet<Query>,
    optimizer: &dyn Optimizer,
    indexes: &[Index],
) -> Result<f64, String> {
    queries.iter().try_fold(0.0, |total, query| {
        optimizer
            .query_plan_cost(&query.text, indexes)
            .map(|cost| total + cost * query.frequency as f64)
    })
}

/// 代价改善比例（相对旧代价）；旧代价非正时视为 0。
fn improvement(old: f64, new: f64) -> f64 {
    if old <= 0.0 { 0.0 } else { (old - new) / old }
}

/// 按指定小数位四舍五入。
fn round(value: f64, digits: u32) -> f64 {
    let factor = 10_f64.powi(digits as i32);
    (value * factor).round() / factor
}

/// 分配不与现有索引冲突的索引名，保持 Go 的三段回退顺序。
pub fn graceful_index_name(
    optimizer: &dyn Optimizer,
    schema: &str,
    table: &str,
    columns: &[String],
) -> Result<String, String> {
    let mut name = truncate_index_name(&format!("idx_{}", columns.join("_")));
    if !index_name_exists(optimizer, schema, table, &name) {
        return Ok(name);
    }
    if let Some(first) = columns.first() {
        name = truncate_index_name(&format!("idx_{first}"));
        if !index_name_exists(optimizer, schema, table, &name) {
            return Ok(name);
        }
        for suffix in 0..30 {
            name = truncate_index_name(&format!("idx_{first}_{suffix}"));
            if !index_name_exists(optimizer, schema, table, &name) {
                return Ok(name);
            }
        }
    }
    Ok(name)
}

fn index_name_exists(optimizer: &dyn Optimizer, schema: &str, table: &str, name: &str) -> bool {
    optimizer
        .index_name_exists(schema, table, &name.to_ascii_lowercase())
        .unwrap_or(false)
}

fn truncate_index_name(name: &str) -> String {
    let mut result = name.to_string();
    if result.len() > 64 {
        result.truncate(64);
    }
    result
}

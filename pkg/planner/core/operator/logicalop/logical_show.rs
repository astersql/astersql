// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

// 逻辑算子：SHOW 语句（LogicalShow）。
//
// 对应 `SHOW ...` 的逻辑计划节点。对 `SHOW STATS_META` 支持将
// db_name/table_name 等值谓词抽取到 Extractor，便于执行期过滤。
// 统计信息使用假估计（每列 NDV=1、行数=1）。

use crate::*;
use std::any::Any;
use std::collections::{HashMap, HashSet};

/// 字符串集合，用作 SHOW 过滤集合。
pub type StringSet = HashSet<String>;

/// SHOW 谓词抽取器：从 WHERE 中提取可下推的展示过滤条件。
pub trait ShowPredicateExtractor {
    /// 克隆为新的 trait 对象。
    fn CloneBox(&self) -> Box<dyn ShowPredicateExtractor>;
    /// 是否已抽取到有效过滤条件。
    fn Extract(&self) -> bool;
    /// EXPLAIN 展示信息。
    fn ExplainInfo(&self) -> String;
    /// 关联的字段名列表。
    fn Field(&self) -> String;
    /// LIKE 模式（若有）。
    fn FieldPatternLike(&self) -> Option<String>;
}

impl Clone for Box<dyn ShowPredicateExtractor> {
    fn clone(&self) -> Self {
        self.CloneBox()
    }
}

/// SHOW STATS_META 的库表名过滤抽取结果。
#[derive(Clone, Default)]
pub struct ShowStatsMetaPredicateExtractor {
    /// 库名过滤集合。
    pub DB: StringSet,
    /// 表名过滤集合。
    pub Table: StringSet,
}
impl ShowPredicateExtractor for ShowStatsMetaPredicateExtractor {
    fn CloneBox(&self) -> Box<dyn ShowPredicateExtractor> {
        Box::new(self.clone())
    }
    fn Extract(&self) -> bool {
        false
    }
    fn ExplainInfo(&self) -> String {
        String::new()
    }
    fn Field(&self) -> String {
        String::new()
    }
    fn FieldPatternLike(&self) -> Option<String> {
        None
    }
}
impl ShowStatsMetaPredicateExtractor {
    /// 库名过滤集合只读访问。
    pub fn StatsMetaDBFilters(&self) -> &StringSet {
        &self.DB
    }
    /// 表名过滤集合只读访问。
    pub fn StatsMetaTableFilters(&self) -> &StringSet {
        &self.Table
    }
}

/// SHOW 语句细分类别。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ShowKind {
    #[default]
    /// 其它 SHOW 类型。
    Other,
    /// SHOW STATS_META。
    StatsMeta,
}
/// SHOW 语句内容参数（库名、分区、索引、资源组等）。
#[derive(Clone, Default)]
pub struct ShowContents {
    /// SHOW 细分类别。
    pub Tp: ShowKind,
    /// 目标库名。
    pub DBName: String,
    /// 分区名。
    pub Partition: parser_ast::CIStr,
    /// 索引名。
    pub IndexName: parser_ast::CIStr,
    /// 资源组名。
    pub ResourceGroupName: String,
    /// 标志位。
    pub Flag: i32,
    /// 是否统计 warnings/errors。
    pub CountWarningsOrErrors: bool,
    /// FULL 修饰。
    pub Full: bool,
    /// IF NOT EXISTS。
    pub IfNotExists: bool,
    /// 全局作用域。
    pub GlobalScope: bool,
    /// EXTENDED 修饰。
    pub Extended: bool,
    /// 导入任务 ID。
    pub ImportJobID: Option<i64>,
    /// 导入分组键。
    pub ImportGroupKey: String,
    /// 分布任务 ID。
    pub DistributionJobID: Option<i64>,
}
/// ShowContents 结构体静态大小，用于 MemoryUsage。
pub const EMPTY_SHOW_CONTENTS_SIZE: i64 = std::mem::size_of::<ShowContents>() as i64;
impl ShowContents {
    /// 估算本结构占用内存（含字符串容量）。
    pub fn MemoryUsage(&self) -> i64 {
        EMPTY_SHOW_CONTENTS_SIZE
            + (self.DBName.len()
                + self.Partition.O.len()
                + self.Partition.L.len()
                + self.IndexName.O.len()
                + self.IndexName.L.len()) as i64
    }
}

/// SHOW 逻辑算子。
#[derive(Default)]
pub struct LogicalShow {
    /// Schema 与基类逻辑计划。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// SHOW 语句内容。
    pub ShowContents: ShowContents,
    /// 可选谓词抽取器。
    pub Extractor: Option<Box<dyn ShowPredicateExtractor>>,
}
impl LogicalShow {
    /// 初始化为 Show 节点。
    pub fn Init(mut self, ctx: base::ContextRef) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "Show", 0);
        self
    }
    /// 谓词下推：仅 StatsMeta 抽取 db_name/table_name 过滤。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        if self.ShowContents.Tp != ShowKind::StatsMeta {
            return Ok(predicates);
        }
        let predicates_len = predicates.len();
        // 先抽库名（转小写），再抽表名；多谓词取交集。
        let (remaining, db) = extractStatsMetaFilters(
            self.LogicalSchemaProducer
                .BaseLogicalPlan
                .SCtx()
                .map(|ctx| ctx.GetExprCtx().GetEvalCtx()),
            self.Schema(),
            self.OutputNames(),
            predicates,
            "db_name",
            true,
        );
        let (remaining, table) = extractStatsMetaFilters(
            self.LogicalSchemaProducer
                .BaseLogicalPlan
                .SCtx()
                .map(|ctx| ctx.GetExprCtx().GetEvalCtx()),
            self.Schema(),
            self.OutputNames(),
            remaining,
            "table_name",
            false,
        );
        if remaining.len() != predicates_len {
            self.Extractor = Some(Box::new(ShowStatsMetaPredicateExtractor {
                DB: db,
                Table: table,
            }));
        }
        Ok(remaining)
    }
    /// 推导假统计信息。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let stats = getFakeStats(self.Schema());
        self.SetStats(stats.clone());
        Ok((stats, true))
    }
}
impl LogicalPlan for LogicalShow {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn PredicatePushDown(&mut self, p: Vec<Expression>) -> Result<Vec<Expression>> {
        LogicalShow::PredicatePushDown(self, p)
    }
    fn DeriveStats(&mut self, r: bool) -> Result<(StatsInfo, bool)> {
        LogicalShow::DeriveStats(self, r)
    }
}

/// 构造行数=1、各列 NDV=1 的假统计。
pub fn getFakeStats(schema: &Schema) -> StatsInfo {
    StatsInfo {
        RowCount: 1.0,
        ColNDVs: schema
            .Columns
            .iter()
            .map(|c| (c.UniqueID, 1.0))
            .collect::<HashMap<_, _>>(),
        ..Default::default()
    }
}
/// 从谓词中抽取指定列的等值/IN 过滤集合，返回剩余谓词与值集合。
pub fn extractStatsMetaFilters(
    eval_ctx: Option<&dyn expression::EvalContext>,
    schema: &Schema,
    names: &NameSlice,
    predicates: Vec<Expression>,
    column_name: &str,
    to_lower: bool,
) -> (Vec<Expression>, StringSet) {
    let ids = findShowColumnIDs(schema, names, column_name);
    if ids.is_empty() {
        return (predicates, StringSet::new());
    }
    let mut extracted = Vec::new();
    let mut found = None::<StringSet>;
    for (index, predicate) in predicates.iter().enumerate() {
        if let Some(values) = extractStatsMetaFilterValues(eval_ctx, predicate, &ids) {
            extracted.push(index);
            let values = values
                .into_iter()
                .map(|v| if to_lower { v.to_lowercase() } else { v })
                .collect::<StringSet>();
            // 多个可抽取谓词取交集，缩小过滤集合。
            found = Some(found.map_or(values.clone(), |old| {
                old.intersection(&values).cloned().collect()
            }));
        }
    }
    if extracted.is_empty() || found.as_ref().is_some_and(HashSet::is_empty) {
        return (predicates, StringSet::new());
    }
    let extracted = extracted.into_iter().collect::<HashSet<_>>();
    let remaining = predicates
        .into_iter()
        .enumerate()
        .filter_map(|(index, predicate)| (!extracted.contains(&index)).then_some(predicate))
        .collect();
    (remaining, found.unwrap_or_default())
}
/// 按列显示名查找对应 UniqueID 集合。
pub fn findShowColumnIDs(schema: &Schema, names: &NameSlice, column_name: &str) -> HashSet<i64> {
    names
        .0
        .iter()
        .zip(&schema.Columns)
        .filter_map(|(name, column)| {
            name.as_ref()
                .filter(|name| name.ColName.L == column_name)
                .map(|_| column.UniqueID)
        })
        .collect()
}
/// 从单个谓词抽取等值/IN/OR 组合的字符串常量值。
pub fn extractStatsMetaFilterValues(
    eval_ctx: Option<&dyn expression::EvalContext>,
    expression: &Expression,
    column_ids: &HashSet<i64>,
) -> Option<Vec<String>> {
    let function = expression.as_scalar_function()?;
    match function.FuncName.L.as_str() {
        "eq" => extractStatsMetaEQValue(eval_ctx, function, column_ids).map(|v| vec![v]),
        "in" => extractStatsMetaINValues(eval_ctx, function, column_ids),
        "or" => {
            let mut values = Vec::new();
            for arg in function.GetArgs() {
                values.extend(extractStatsMetaFilterValues(eval_ctx, arg, column_ids)?);
            }
            Some(values)
        }
        _ => None,
    }
}
/// 从 `col = const` 或 `const = col` 抽取字符串常量。
pub fn extractStatsMetaEQValue(
    eval_ctx: Option<&dyn expression::EvalContext>,
    function: &expression::ScalarFunction,
    column_ids: &HashSet<i64>,
) -> Option<String> {
    let args = function.GetArgs();
    if args.len() != 2 {
        return None;
    }
    if args[0]
        .as_column()
        .is_some_and(|c| column_ids.contains(&c.UniqueID))
    {
        getStringValueFromConstant(eval_ctx, &args[1])
    } else if args[1]
        .as_column()
        .is_some_and(|c| column_ids.contains(&c.UniqueID))
    {
        getStringValueFromConstant(eval_ctx, &args[0])
    } else {
        None
    }
}
/// 从 `col IN (c1, c2, ...)` 抽取全部字符串常量。
pub fn extractStatsMetaINValues(
    eval_ctx: Option<&dyn expression::EvalContext>,
    function: &expression::ScalarFunction,
    column_ids: &HashSet<i64>,
) -> Option<Vec<String>> {
    let args = function.GetArgs();
    if !args
        .first()?
        .as_column()
        .is_some_and(|c| column_ids.contains(&c.UniqueID))
    {
        return None;
    }
    args[1..]
        .iter()
        .map(|argument| getStringValueFromConstant(eval_ctx, argument))
        .collect()
}
/// 从常量表达式取出字符串值；延迟表达式拒绝抽取，参数标记按当前上下文求值。
pub fn getStringValueFromConstant(
    eval_ctx: Option<&dyn expression::EvalContext>,
    expression: &Expression,
) -> Option<String> {
    let constant = expression.as_constant()?;
    if constant.DeferredExpr.is_some() {
        return None;
    }
    let value = if let Some(marker) = &constant.ParamMarker {
        marker.GetUserVar(eval_ctx?).ok()?
    } else {
        constant.Value.clone()
    };
    value.ToString().ok()
}

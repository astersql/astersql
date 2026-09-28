// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software distributed
// under the License is distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND.

// 逻辑 CTE 算子（LogicalCTE）与共享定义 CTEClass。
//
// CTE（Common Table Expression）对应 SQL WITH 子句；可含种子计划与可选递归计划。
// 多个引用点通过 CTEClassRef 共享同一 CTE 定义（谓词下推、Limit、关联列等状态）。

use crate::*;
use base::PlanContext as BasePlanContext;
use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock, RwLock};

/// Dependency-inversion boundary for optimizing a detached CTE seed. The
/// concrete rule pipeline lives in planner core, above this operator crate.
pub type OptimizeCTESeed = fn(u64, &mut LogicalPlanRef) -> Result<()>;

static OPTIMIZE_CTE_SEED: OnceLock<OptimizeCTESeed> = OnceLock::new();

/// Bridges core's object-safe plan context into cardinality's minimal view.
struct CTECardinalityContext<'a>(&'a dyn BasePlanContext);

impl cardinality::CardinalityContext for CTECardinalityContext<'_> {
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        self.0.GetSessionVars()
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.0.GetExprCtx()
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        self.0.GetRangerCtx()
    }
}

/// Install the planner-core CTE seed optimizer once.
pub fn InstallOptimizeCTESeed(
    optimizer: OptimizeCTESeed,
) -> std::result::Result<(), OptimizeCTESeed> {
    OPTIMIZE_CTE_SEED.set(optimizer)
}

/// 共享 CTE 定义的引用计数句柄（内部可变）。
pub type CTEClassRef = Rc<RefCell<CTEClass>>;

#[derive(Default)]
/// CTE 定义本体：种子/递归逻辑计划、存储 ID、Limit 与谓词下推缓冲等。
pub struct CTEClass {
    /// 是否 DISTINCT CTE。
    pub IsDistinct: bool,
    /// 非递归种子子计划。
    pub SeedPartLogicalPlan: Option<LogicalPlanRef>,
    /// 递归部分子计划；None 表示非递归 CTE。
    pub RecursivePartLogicalPlan: Option<LogicalPlanRef>,
    /// 物化/存储标识，与 LogicalCTETable 对齐。
    pub IDForStorage: i32,
    /// 优化标志位集合。
    pub OptFlag: u64,
    /// 种子逻辑计划是否已经按其优化标志完成优化。
    pub SeedPartLogicalOptimized: bool,
    /// 是否带有 Limit 范围。
    pub HasLimit: bool,
    /// Limit 起始偏移。
    pub LimitBeg: u64,
    /// Limit 结束位置。
    pub LimitEnd: u64,
    /// 是否位于 Apply（关联子查询展开）上下文中。
    pub IsInApply: bool,
    /// 已收集、待推入种子计划的谓词。
    pub PushDownPredicates: Vec<Expression>,
    /// 列名到 Column 的映射。
    pub ColumnMap: HashMap<i64, Column>,
    /// 是否为最外层 CTE（影响谓词下推条件）。
    pub IsOuterMostCTE: bool,
}

impl CTEClass {
    /// 估算本定义及谓词/列映射占用的近似内存。
    pub fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self
                .PushDownPredicates
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + self
                .ColumnMap
                .iter()
                .map(|(_, value)| value.MemoryUsage())
                .sum::<i64>()
    }
}

/// 逻辑 CTE 算子：挂接共享 CTEClass，并维护别名与种子统计。
pub struct LogicalCTE {
    /// schema 生产者嵌入。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 共享 CTE 定义。
    pub Cte: CTEClassRef,
    /// CTE AS 别名。
    pub CteAsName: parser_ast::CIStr,
    /// CTE 原名。
    pub CteName: parser_ast::CIStr,
    /// 种子统计，可与 LogicalCTETable 共享。
    pub SeedStat: Arc<RwLock<StatsInfo>>,
    /// 若为 true，仅作为存储形态参与计划，不做完整 CTE 展开。
    pub OnlyUsedAsStorage: bool,
}

/// Replace consumer columns with the corresponding seed columns without
/// pulling the core rule crate into the logical-operator crate.
fn resolve_cte_expression(origin: Expression, replace: &HashMap<i64, Column>) -> Expression {
    if let Some(column) = origin.as_any().downcast_ref::<Column>() {
        return replace
            .get(&column.UniqueID)
            .map(|replacement| Box::new(replacement.Clone()) as Expression)
            .unwrap_or(origin);
    }
    if let Some(correlated) = origin.as_any().downcast_ref::<CorrelatedColumn>() {
        let Some(replacement) = replace.get(&correlated.column.UniqueID) else {
            return origin;
        };
        let mut correlated = correlated.Clone();
        correlated.column = replacement.Clone();
        return Box::new(correlated);
    }
    let Some(function) = origin.as_any().downcast_ref::<expression::ScalarFunction>() else {
        return origin;
    };
    let mut function = function.clone_scalar();
    for argument in function.GetArgsMut() {
        *argument = resolve_cte_expression(argument.CloneExpr(), replace);
    }
    function.CleanHashCode();
    Box::new(function)
}

/// 默认空 LogicalCTE。
impl Default for LogicalCTE {
    fn default() -> Self {
        Self {
            LogicalSchemaProducer: LogicalSchemaProducer::default(),
            Cte: Rc::new(RefCell::new(CTEClass::default())),
            CteAsName: parser_ast::CIStr::default(),
            CteName: parser_ast::CIStr::default(),
            SeedStat: Arc::new(RwLock::new(StatsInfo::default())),
            OnlyUsedAsStorage: false,
        }
    }
}

impl LogicalCTE {
    /// 初始化基类，类型标记为 CTE。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "CTE", offset);
        self
    }

    /// 对最外层非递归 CTE 收集可下推谓词；Apply 内保留关联列谓词，否则过滤掉关联列。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        let mut cte = self.Cte.borrow_mut();
        // 仅最外层且无递归部分时才缓冲谓词，避免破坏递归语义。
        if cte.RecursivePartLogicalPlan.is_none() && cte.IsOuterMostCTE {
            let pushed = if cte.IsInApply {
                predicates.clone()
            } else {
                predicates
                    .iter()
                    .filter(|expr| expression::ExtractCorColumns(expr.as_ref()).is_empty())
                    .cloned()
                    .collect()
            };
            if pushed.is_empty() {
                cte.PushDownPredicates.push(Box::new(expression::NewOne()));
            } else {
                let resolved = pushed
                    .into_iter()
                    .map(|predicate| resolve_cte_expression(predicate, &cte.ColumnMap))
                    .collect::<Vec<_>>();
                if let Some(condition) = expression::ComposeCNFCondition(
                    self.SCtx()
                        .expect("outermost CTE predicate pushdown requires context")
                        .GetExprCtx(),
                    &resolved,
                ) {
                    cte.PushDownPredicates.push(condition);
                }
            }
        }
        Ok(predicates)
    }

    /// CTE 列裁剪占位：当前不裁剪，保持与 Go 侧保守行为一致。
    pub fn PruneColumns(&mut self, _columns: &[Column]) -> Result<()> {
        Ok(())
    }

    /// 将本 CTE 挂到 TopN 之下并上浮 TopN；对应 TopN 下推边界。
    pub fn PushDownTopN(&mut self, mut top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        if let Some(node) = top_n.as_mut() {
            let current = std::mem::replace(self, LogicalCTE::default());
            node.SetChildren(vec![Box::new(current)]);
        }
        top_n
    }

    /// 合并种子与递归子计划统计，写回 SeedStat 并缓存到本节点。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let mut seed_stats = self
            .SeedStat
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let visible_schema = self.Schema().Clone();
        let mut stats = StatsInfo::default();
        let mut seed = self.Cte.borrow_mut().SeedPartLogicalPlan.take();
        if let Some(mut seed_plan) = seed {
            // Go combines predicates collected from all outer references as a
            // DNF, extracts common factors, and injects those factors as a
            // normal Selection above the shared seed before optimizing it.
            let push_down = std::mem::take(&mut self.Cte.borrow_mut().PushDownPredicates);
            if !push_down.is_empty() {
                if let Some(context) = seed_plan.SCtx().cloned() {
                    let dnf = expression::ComposeDNFCondition(context.GetExprCtx(), &push_down);
                    let conditions = dnf.map_or_else(Vec::new, |condition| {
                        let mut build_context =
                            expression::exprctx::CtxWithTruncateResult::Original(
                                context.GetExprCtx(),
                            );
                        expression::ExtractFiltersFromDNFs(&mut build_context, vec![condition])
                    });
                    if !conditions.is_empty() {
                        // The shared seed is reordered before consumer-local
                        // filters are costed in Go.  Keep the filter attached
                        // for physical push-down/rendering, but retain the
                        // unfiltered seed cardinality for producer ordering.
                        let seed_stats = seed_plan.DeriveStats(reload)?.0;
                        let schema = seed_plan.Schema().Clone();
                        let names = seed_plan.OutputNames().Shallow();
                        let mut selection = LogicalSelection {
                            Conditions: conditions,
                            ..Default::default()
                        }
                        .Init(context, seed_plan.QueryBlockOffset());
                        selection.SetSchema(schema);
                        selection.SetOutputNames(names);
                        selection.SetChildren(vec![seed_plan]);
                        selection.SetStats(seed_stats);
                        seed_plan = Box::new(selection);
                        let mut cte = self.Cte.borrow_mut();
                        cte.OptFlag = rule_util::SetPredicatePushDownFlag(cte.OptFlag);
                    }
                }
            }
            let (seed_optimized, opt_flag) = {
                let cte = self.Cte.borrow();
                (cte.SeedPartLogicalOptimized, cte.OptFlag)
            };
            if !seed_optimized && let Some(optimize) = OPTIMIZE_CTE_SEED.get() {
                // Go's LogicalCTE::DeriveStats calls DoOptimize for the seed
                // before exposing its cardinality to a consumer. In
                // particular, a nested CTE must contribute the statistics of
                // its reordered/aggregated seed to the outer join graph.
                optimize(opt_flag, &mut seed_plan)
                    .map_err(|error| crate::PlannerError(error.to_string()))?;
                self.Cte.borrow_mut().SeedPartLogicalOptimized = true;
            }
            seed_stats = seed_plan
                .StatsInfo()
                .cloned()
                .unwrap_or(seed_plan.DeriveStats(reload)?.0);
            stats.RowCount = seed_stats.RowCount;
            for (visible, source) in visible_schema
                .Columns
                .iter()
                .zip(&seed_plan.Schema().Columns)
            {
                stats.ColNDVs.insert(
                    visible.UniqueID,
                    seed_stats
                        .ColNDVs
                        .get(&source.UniqueID)
                        .copied()
                        .unwrap_or_default(),
                );
            }
            seed = Some(seed_plan);
        } else {
            stats = seed_stats.clone();
        }
        self.Cte.borrow_mut().SeedPartLogicalPlan = seed;
        // 递归部分行数与 NDV 累加到种子统计之上。
        let is_distinct = self.Cte.borrow().IsDistinct;
        if let Some(recursive) = self.Cte.borrow_mut().RecursivePartLogicalPlan.as_mut() {
            let recursive_stats = recursive.DeriveStats(reload)?.0;
            for (visible, source) in visible_schema
                .Columns
                .iter()
                .zip(&recursive.Schema().Columns)
            {
                *stats.ColNDVs.entry(visible.UniqueID).or_default() += recursive_stats
                    .ColNDVs
                    .get(&source.UniqueID)
                    .copied()
                    .unwrap_or_default();
            }
            if is_distinct {
                let context = self
                    .SCtx()
                    .map(|context| CTECardinalityContext(context.as_ref()));
                stats.RowCount = cardinality::EstimateColsNDVWithMatchedLen(
                    context
                        .as_ref()
                        .map(|context| context as &dyn cardinality::CardinalityContext),
                    &visible_schema.Columns,
                    &visible_schema,
                    &stats,
                )
                .0;
            } else {
                stats.RowCount += recursive_stats.RowCount;
            }
        }
        *self
            .SeedStat
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = seed_stats;
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 汇总种子/递归子树是否均可下推 TiFlash，作为可能物理属性提示。
    pub fn PreparePossibleProperties(&self) -> PossiblePropertiesInfo {
        let cte = self.Cte.borrow();
        PossiblePropertiesInfo {
            Orders: Vec::new(),
            HasTiFlash: cte
                .SeedPartLogicalPlan
                .as_ref()
                .is_some_and(|plan| plan.base().PreparePossiblePropertiesValue()),
        }
    }

    /// 从种子与递归子计划收集关联列（CorrelatedColumn，引用外层查询的列）。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        let cte = self.Cte.borrow();
        [
            cte.SeedPartLogicalPlan.as_ref(),
            cte.RecursivePartLogicalPlan.as_ref(),
        ]
        .into_iter()
        .flatten()
        .flat_map(|plan| plan.base().ExtractCorrelatedCols())
        .collect()
    }
}

/// LogicalPlan 适配：将 trait 方法转发到本类型实现。
impl LogicalPlan for LogicalCTE {
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
        LogicalCTE::PredicatePushDown(self, p)
    }
    fn PruneColumns(&mut self, c: &[Column]) -> Result<()> {
        LogicalCTE::PruneColumns(self, c)
    }
    fn PushDownTopN(&mut self, t: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        LogicalCTE::PushDownTopN(self, t)
    }
    fn DeriveStats(&mut self, r: bool) -> Result<(StatsInfo, bool)> {
        LogicalCTE::DeriveStats(self, r)
    }
}

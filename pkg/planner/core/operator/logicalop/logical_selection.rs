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

// 逻辑算子：选择/过滤（LogicalSelection / Selection）。
//
// 对应 WHERE/HAVING：Conditions 为 CNF（合取范式）下由 AND 连接的谓词项。
// 负责谓词下推、恒假转 TableDual、列裁剪、键信息推导、统计缩放，
// 以及 Selection(Window(DataSource)) 场景下推导分区 TopN。

use crate::{
    BaseLogicalPlan, BoundType, ByItems, Column, CorrelatedColumn, DataSource, Expression,
    FrameType, LogicalAggregation, LogicalJoin, LogicalPlan, LogicalPlanRef, LogicalProjection,
    LogicalTableDual, LogicalTopN, LogicalWindow, NewBaseLogicalPlan, PossiblePropertiesInfo,
    PredicatePushDownPlan, Result, StatsInfo,
};
use expression::ScalarFunction;
use std::any::Any;
use std::collections::{HashMap, HashSet};

/// WHERE/HAVING operator. Conditions are the CNF terms joined by AND.
/// WHERE/HAVING 过滤算子：Conditions 为 AND 连接的 CNF 谓词项。
#[derive(Default)]
pub struct LogicalSelection {
    /// 基类逻辑计划。
    pub BaseLogicalPlan: BaseLogicalPlan,
    /// 过滤条件列表（CNF 项）。
    pub Conditions: Vec<Expression>,
}

impl LogicalSelection {
    /// 初始化为 Selection（TypeSel）节点。
    pub fn Init(mut self, ctx: base::ContextRef, qb_offset: i32) -> Self {
        self.BaseLogicalPlan = NewBaseLogicalPlan(ctx, plancodec::TypeSel, qb_offset);
        self
    }

    /// 生成 EXPLAIN 中的条件列表（排序后拼接，保证稳定）。
    pub fn ExplainInfo(&self) -> String {
        let Some(context) = self.SCtx() else {
            return String::new();
        };
        let eval_context = context.GetExprCtx().GetEvalCtx();
        let mut conditions = self
            .Conditions
            .iter()
            .map(|condition| condition.StringWithCtx(Some(eval_context), ""))
            .collect::<Vec<_>>();
        conditions.sort();
        conditions.join(", ")
    }

    /// 按哈希映射替换条件中的列引用。
    pub fn ReplaceExprColumns(&mut self, replace: &HashMap<Vec<u8>, Column>) {
        for condition in &mut self.Conditions {
            *condition = replaceExpressionColumns(condition.clone(), replace);
        }
    }

    /// 计算计划哈希：类型、查询块偏移、条件哈希（条件哈希先排序）。
    pub fn HashCode(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(12 + self.Conditions.len() * 25);
        result.extend_from_slice(
            &(plancodec::TypeStringToPhysicalID(self.TP()) as u32).to_be_bytes(),
        );
        result.extend_from_slice(&(self.QueryBlockOffset() as u32).to_be_bytes());
        result.extend_from_slice(&(self.Conditions.len() as u32).to_be_bytes());
        let mut hashes = self
            .Conditions
            .iter()
            .map(|condition| condition.HashCode())
            .collect::<Vec<_>>();
        hashes.sort();
        for hash in hashes {
            result.extend_from_slice(&(hash.len() as u32).to_be_bytes());
            result.extend(hash);
        }
        result
    }

    /// 谓词下推：简化条件、恒假转 Dual、拆分 set/get_var、向子节点下推。
    pub fn PredicatePushDown(
        &mut self,
        mut predicates: Vec<Expression>,
    ) -> Result<Vec<Expression>> {
        // 去掉恒真谓词，避免无意义过滤。
        predicates.retain(|condition| !isConstTrue(condition.as_ref()));
        if let Some(context) = self.SCtx().cloned() {
            self.Conditions = rule_util::ApplyPredicateSimplification(
                context,
                std::mem::take(&mut self.Conditions),
                false,
                None,
            );
        }
        // A constant-false condition owned by this Selection makes only its
        // existing subtree empty. Predicates supplied by a parent still have
        // to be returned so the parent can retain them above the Dual, which
        // matches TiDB's predicate-push-down plan replacement semantics.
        if crate::Conds2TableDual(&self.Conditions) {
            let predicates = if let Some(aggregation) = self
                .Children()
                .first()
                .and_then(|child| child.as_any().downcast_ref::<LogicalAggregation>())
            {
                let (mut pushable, retained) = aggregation.splitCondForAggregation(predicates);
                pushable.extend(retained);
                pushable
            } else {
                predicates
            };
            let context = self
                .SCtx()
                .cloned()
                .ok_or_else(|| crate::PlannerError("Selection has no plan context".into()))?;
            let mut dual = LogicalTableDual {
                RowCount: 0,
                ..LogicalTableDual::default()
            }
            .Init(context, self.QueryBlockOffset());
            dual.SetSchema(self.Schema().Clone());
            dual.SetOutputNames(self.OutputNames().Shallow());
            self.SetChildren(vec![Box::new(dual)]);
            self.Conditions.clear();
            return Ok(predicates);
        }
        // 拆出含 set_var/get_var 的条件（副作用，不可下推），其余与父谓词一并下推。
        let (mut pushable, mut retained) = splitSetGetVarFunc(std::mem::take(&mut self.Conditions));
        pushable.append(&mut predicates);
        let mut outer_conditions = Vec::new();
        let selection_schema = self.Schema().Clone();
        if let Some(child) = self.Children_mut().first_mut() {
            let mut child_retained = PredicatePushDownPlan(child, pushable)?;
            child_retained.append(&mut retained);
            // 本地可保留：无相关列且列均在本 Selection schema 内；否则上抛。
            let (local, outer): (Vec<_>, Vec<_>) =
                child_retained.into_iter().partition(|condition| {
                    expression::ExtractCorColumns(condition.as_ref()).is_empty()
                        && expression::ExtractColumns(condition.as_ref())
                            .iter()
                            .all(|column| selection_schema.Contains(column))
                });
            self.Conditions = local;
            outer_conditions = outer;
        } else {
            pushable.append(&mut retained);
            self.Conditions = pushable;
        }

        if !self.Conditions.is_empty()
            && let Some(context) = self.SCtx().cloned()
        {
            self.Conditions = rule_util::ApplyPredicateSimplification(
                context,
                std::mem::take(&mut self.Conditions),
                true,
                None,
            );
        }

        if crate::Conds2TableDual(&self.Conditions) {
            let context = self
                .SCtx()
                .cloned()
                .ok_or_else(|| crate::PlannerError("Selection has no plan context".into()))?;
            let mut dual = LogicalTableDual {
                RowCount: 0,
                ..LogicalTableDual::default()
            }
            .Init(context, self.QueryBlockOffset());
            dual.SetSchema(self.Schema().Clone());
            self.SetChildren(vec![Box::new(dual)]);
            self.Conditions.clear();
        }
        Ok(outer_conditions)
    }

    /// 根节点谓词下推：可返回替换子树（如 Dual 或直接子节点）。
    pub fn PredicatePushDownRoot(
        &mut self,
        mut predicates: Vec<Expression>,
    ) -> Result<(Vec<Expression>, Option<LogicalPlanRef>)> {
        predicates.retain(|condition| !isConstTrue(condition.as_ref()));
        if let Some(context) = self.SCtx().cloned() {
            self.Conditions = rule_util::ApplyPredicateSimplification(
                context,
                std::mem::take(&mut self.Conditions),
                false,
                None,
            );
        }
        if crate::Conds2TableDual(&self.Conditions) {
            let predicates = if let Some(child) = self.Children().first() {
                rewrite_predicates_for_empty_subtree(child.as_ref(), predicates)
            } else {
                predicates
            };
            let context = self
                .SCtx()
                .cloned()
                .ok_or_else(|| crate::PlannerError("Selection has no plan context".into()))?;
            let mut dual = LogicalTableDual {
                RowCount: 0,
                ..LogicalTableDual::default()
            }
            .Init(context, self.QueryBlockOffset());
            dual.SetSchema(self.Schema().Clone());
            dual.SetOutputNames(self.OutputNames().Shallow());
            return Ok((predicates, Some(Box::new(dual))));
        }
        let (mut pushable, mut retained) = splitSetGetVarFunc(std::mem::take(&mut self.Conditions));
        pushable.append(&mut predicates);
        let mut returned = if let Some(child) = self.Children_mut().first_mut() {
            PredicatePushDownPlan(child, pushable)?
        } else {
            pushable
        };
        returned.append(&mut retained);
        if !returned.is_empty() {
            if let Some(context) = self.SCtx().cloned() {
                returned = rule_util::ApplyPredicateSimplification(context, returned, true, None);
            }
            let selection_schema = self.Schema().Clone();
            let (local, outer): (Vec<_>, Vec<_>) = returned.into_iter().partition(|condition| {
                expression::ExtractCorColumns(condition.as_ref()).is_empty()
                    && expression::ExtractColumns(condition.as_ref())
                        .iter()
                        .all(|column| selection_schema.Contains(column))
            });
            if crate::Conds2TableDual(&local) {
                let context = self
                    .SCtx()
                    .cloned()
                    .ok_or_else(|| crate::PlannerError("Selection has no plan context".into()))?;
                let mut dual = LogicalTableDual {
                    RowCount: 0,
                    ..LogicalTableDual::default()
                }
                .Init(context, self.QueryBlockOffset());
                dual.SetSchema(self.Schema().Clone());
                dual.SetOutputNames(self.OutputNames().Shallow());
                return Ok((outer, Some(Box::new(dual))));
            }
            self.Conditions = local;
            if self.Conditions.is_empty() && self.Children().len() == 1 {
                return Ok((outer, Some(self.TakeChildren().remove(0))));
            }
            return Ok((outer, None));
        }
        self.Conditions.clear();
        let replacement = (self.Children().len() == 1).then(|| self.TakeChildren().remove(0));
        Ok((Vec::new(), replacement))
    }

    /// 列裁剪：父用列并上条件引用列，再下推到子节点。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let mut used = parent_used_cols.to_vec();
        used.extend(
            expression::ExtractColumnsFromExpressions(&self.Conditions, None)
                .into_iter()
                .cloned(),
        );
        if let Some(child) = self.Children_mut().first_mut() {
            child.PruneColumns(&used)?;
        }
        Ok(())
    }

    /// 推导键信息；等值常量/相关列条件可证明 MaxOneRow。
    pub fn BuildKeyInfo(&mut self) {
        self.BaseLogicalPlan.BuildKeyInfo();
        // Go's BaseLogicalPlan Schema() delegates to its first child.  Rust
        // stores the Selection schema explicitly, so refresh the child-owned
        // key metadata before applying Selection's max-one-row derivation.
        if let Some(child) = self.Children().first() {
            let schema = child.Schema().Clone();
            self.SetSchema(schema);
        }
        if self.MaxOneRow() {
            return;
        }
        let equal_columns = self
            .Conditions
            .iter()
            .filter_map(|condition| condition.as_any().downcast_ref::<ScalarFunction>())
            .filter(|function| function.FuncName.L == parser_ast::EQ)
            .flat_map(|function| {
                let args = function.GetArgs();
                (args.len() == 2)
                    .then_some(args)
                    .into_iter()
                    .flat_map(|args| {
                        (0..2).filter_map(move |index| {
                            let column = args[index].as_any().downcast_ref::<Column>()?;
                            let peer = &args[1 - index];
                            (peer.as_any().is::<expression::Constant>()
                                || peer.as_any().is::<CorrelatedColumn>())
                            .then_some(column.UniqueID)
                        })
                    })
            })
            .collect::<HashSet<_>>();
        let max_one_row = self
            .Children()
            .first()
            .is_some_and(|child| rule_util::CheckMaxOneRowCond(&equal_columns, child.Schema()));
        self.SetMaxOneRow(max_one_row);
    }

    /// Derives a partition TopN for Selection(Window(DataSource)).
    /// 为 Selection(Window(DataSource)) 推导分区 TopN。
    pub fn DeriveTopN(&mut self) -> bool {
        let Some(limit) = self.windowIsTopN() else {
            return false;
        };
        let Some(window) = self
            .Children_mut()
            .first_mut()
            .and_then(|child| child.as_any_mut().downcast_mut::<LogicalWindow>())
        else {
            return false;
        };
        let Some(grand_child) = window.TakeChildren().into_iter().next() else {
            return false;
        };
        let Some(context) = grand_child.SCtx().cloned() else {
            window.SetChildren(vec![grand_child]);
            return false;
        };
        let offset = grand_child.QueryBlockOffset();
        let schema = grand_child.Schema().Clone();
        let by_items = window
            .OrderBy
            .iter()
            .map(|item| ByItems {
                Expr: Box::new(item.Col.Clone()),
                Desc: item.Desc,
            })
            .collect();
        let mut top_n = LogicalTopN {
            Count: limit,
            ByItems: by_items,
            PartitionBy: window.GetPartitionBy().to_vec(),
            ..LogicalTopN::default()
        }
        .Init(context, offset);
        top_n.SetSchema(schema);
        top_n.SetChildren(vec![grand_child]);
        window.SetChildren(vec![Box::new(top_n)]);
        true
    }

    /// 对本节点条件做谓词简化，再递归子节点。
    pub fn PredicateSimplification(&mut self) {
        if let Some(context) = self.SCtx().cloned() {
            self.Conditions = rule_util::ApplyPredicateSimplification(
                context,
                std::mem::take(&mut self.Conditions),
                false,
                None,
            );
        }
        self.BaseLogicalPlan.PredicateSimplification();
    }

    /// 上拉可上推的列-常量比较谓词。
    pub fn PullUpConstantPredicates(&self) -> Vec<Expression> {
        let Some(context) = self.SCtx() else {
            return Vec::new();
        };
        let eval_context = context.GetExprCtx().GetEvalCtx();
        self.Conditions
            .iter()
            .filter(|condition| validCompareConstantPredicate(eval_context, condition.as_ref()))
            .cloned()
            .collect()
    }

    /// 推导统计：子行数乘以 SelectionFactor，清空 GroupNDVs。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = LogicalPlan::StatsInfo(self) {
            return Ok((stats.clone(), false));
        }
        let child_stats = if let Some(child) = self.Children_mut().first_mut() {
            child.DeriveStats(reload)?.0
        } else {
            StatsInfo::default()
        };
        let mut stats = if let Some(context) = self.SCtx() {
            child_stats.Scale(
                context.GetSessionVars(),
                cost::factors_thresholds::SelectionFactor,
            )
        } else {
            child_stats
        };
        stats.GroupNDVs.clear();
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 继承子节点可能排序属性。
    pub fn PreparePossibleProperties(
        &mut self,
        children: &[PossiblePropertiesInfo],
    ) -> PossiblePropertiesInfo {
        let Some(first) = children.first() else {
            self.BaseLogicalPlan.PreparePossibleProperties(&[]);
            return PossiblePropertiesInfo::default();
        };
        self.BaseLogicalPlan
            .PreparePossibleProperties(&[first.HasTiFlash]);
        first.clone()
    }

    /// 抽取条件中的相关列（外层引用）。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.Conditions
            .iter()
            .flat_map(|condition| expression::ExtractCorColumns(condition.as_ref()))
            .map(CorrelatedColumn::Clone)
            .collect()
    }

    /// 从条件抽取函数依赖（FD）：非空、常量、等价类，再投影到输出列。
    pub fn ExtractFD(&mut self) -> fd::FDSet {
        let context = self.SCtx().cloned();
        let conditions = self.Conditions.clone();
        let output_columns = self
            .Children()
            .first()
            .and_then(|child| child.as_any().downcast_ref::<LogicalJoin>())
            .and_then(|join| join.FullSchema.as_ref())
            .map(|schema| schema.Columns.clone())
            .unwrap_or_else(|| self.Schema().Columns.clone());
        let mut fds = self.BaseLogicalPlan.ExtractFD().clone();
        if let Some(context) = context {
            let mut output_ids = intset::NewFastIntSet(Vec::new());
            for column in output_columns {
                output_ids.Insert(column.UniqueID as i32);
            }
            let not_null = planner_util::ExtractNotNullFromConds(&conditions, context.as_ref());
            let constants =
                planner_util::ExtractConstantCols(&conditions, context.as_ref(), &mut fds);
            let equivalences =
                planner_util::ExtractEquivalenceCols(&conditions, context.as_ref(), &mut fds);
            fds.MakeNotNull(not_null);
            fds.AddConstants(constants);
            for [left, right] in equivalences {
                fds.AddEquivalence(left, right);
            }
            fds.ProjectCols(output_ids);
        }
        self.BaseLogicalPlan.SetFDs(fds.clone());
        fds
    }

    /// 结合本节点条件尝试将外连接转为内连接。
    pub fn ConvertOuterToInnerJoin(&mut self, mut predicates: Vec<Expression>) {
        predicates.extend(self.Conditions.clone());
        self.BaseLogicalPlan.ConvertOuterToInnerJoin(&predicates);
    }

    /// 识别 `row_number() ... WHERE rn <= N` 模式，返回 N。
    fn windowIsTopN(&self) -> Option<u64> {
        let window = self
            .Children()
            .first()?
            .as_any()
            .downcast_ref::<LogicalWindow>()?;
        if self.Conditions.len() != 1 {
            return None;
        }
        let (column, limit) = expression::FindUpperBound(self.Conditions[0].as_ref());
        let column = column?;
        if limit <= 0 {
            return None;
        }
        let eval_context = self.SCtx()?.GetExprCtx().GetEvalCtx();
        let result_columns = window.GetWindowResultColumns();
        if result_columns.len() != 1 || !column.Equal(eval_context, &result_columns[0]) {
            return None;
        }
        let source = window
            .Children()
            .first()?
            .as_any()
            .downcast_ref::<DataSource>()?;
        if source
            .AllPossibleAccessPaths
            .iter()
            .any(|path| path.StoreType == kv::StoreType::TiFlash)
        {
            return None;
        }
        let current_row_frame = window.Frame.as_ref().is_none_or(|frame| {
            frame.Type == FrameType::Rows
                && frame
                    .Start
                    .as_ref()
                    .is_some_and(|bound| bound.Type == BoundType::CurrentRow)
                && frame
                    .End
                    .as_ref()
                    .is_some_and(|bound| bound.Type == BoundType::CurrentRow)
        });
        (window.WindowFuncDescs.len() == 1
            && window.WindowFuncDescs[0].Name == "row_number"
            && current_row_frame
            && checkPartitionBy(window, source))
        .then_some(limit as u64)
    }
}

/// 空子树场景重写谓词：穿透 Projection/Aggregation 拆分后递归。
fn rewrite_predicates_for_empty_subtree(
    plan: &dyn LogicalPlan,
    predicates: Vec<Expression>,
) -> Vec<Expression> {
    if let Some(projection) = plan.as_any().downcast_ref::<LogicalProjection>() {
        let (pushable, mut retained) = crate::breakDownPredicates(projection, predicates);
        let mut rewritten = if let Some(child) = projection.Children().first() {
            rewrite_predicates_for_empty_subtree(child.as_ref(), pushable)
        } else {
            pushable
        };
        rewritten.append(&mut retained);
        return rewritten;
    }
    if let Some(aggregation) = plan.as_any().downcast_ref::<LogicalAggregation>() {
        let (pushable, mut retained) = aggregation.splitCondForAggregation(predicates);
        let mut rewritten = if let Some(child) = aggregation.Children().first() {
            rewrite_predicates_for_empty_subtree(child.as_ref(), pushable)
        } else {
            pushable
        };
        rewritten.append(&mut retained);
        return rewritten;
    }
    predicates
}

impl LogicalPlan for LogicalSelection {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.BaseLogicalPlan
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.BaseLogicalPlan
    }
    fn ExplainInfo(&self) -> String {
        Self::ExplainInfo(self)
    }
    fn HashCode(&self) -> Vec<u8> {
        Self::HashCode(self)
    }
    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        Self::PredicatePushDown(self, predicates)
    }
    fn PredicatePushDownRoot(
        &mut self,
        predicates: Vec<Expression>,
    ) -> Result<(Vec<Expression>, Option<LogicalPlanRef>)> {
        Self::PredicatePushDownRoot(self, predicates)
    }
    fn ConvertOuterToInner(&mut self, mut predicates: Vec<Expression>) {
        predicates.extend(self.Conditions.iter().cloned());
        for child in self.Children_mut() {
            child.ConvertOuterToInner(predicates.iter().cloned().collect());
        }
    }
    fn PredicateSimplification(&mut self) {
        Self::PredicateSimplification(self)
    }
    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        Self::PruneColumns(self, columns)
    }
    fn BuildKeyInfo(&mut self) {
        Self::BuildKeyInfo(self)
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        Self::DeriveStats(self, reload)
    }
}

/// 递归替换表达式中的列/相关列为映射目标。
fn replaceExpressionColumns(
    expression: Expression,
    replace: &HashMap<Vec<u8>, Column>,
) -> Expression {
    if let Some(column) = expression.as_any().downcast_ref::<Column>() {
        return replace
            .get(&expression::Expression::CanonicalHashCode(column))
            .map(|column| Box::new(column.Clone()) as Expression)
            .unwrap_or(expression);
    }
    if let Some(correlated) = expression.as_any().downcast_ref::<CorrelatedColumn>() {
        let key = expression::Expression::CanonicalHashCode(&correlated.column);
        let Some(column) = replace.get(&key) else {
            return expression;
        };
        let mut replaced = correlated.Clone();
        replaced.column = column.Clone();
        return Box::new(replaced);
    }
    let Some(function) = expression.as_any().downcast_ref::<ScalarFunction>() else {
        return expression;
    };
    let mut function = function.clone_scalar();
    for argument in function.GetArgsMut() {
        *argument = replaceExpressionColumns(argument.clone(), replace);
    }
    function.CleanHashCode();
    Box::new(function)
}

/// 拆分不含/含 set_var、get_var 的过滤条件。
pub fn splitSetGetVarFunc(filters: Vec<Expression>) -> (Vec<Expression>, Vec<Expression>) {
    filters
        .into_iter()
        .partition(|expression| !hasGetSetVarFunc(expression.as_ref()))
}

/// 表达式树是否包含 set_var/get_var（会话变量副作用）。
fn hasGetSetVarFunc(expression: &dyn expression::Expression) -> bool {
    let Some(function) = expression.as_any().downcast_ref::<ScalarFunction>() else {
        return false;
    };
    matches!(
        function.FuncName.L.as_str(),
        parser_ast::SetVar | parser_ast::GetVar
    ) || function
        .GetArgs()
        .iter()
        .any(|argument| hasGetSetVarFunc(argument.as_ref()))
}

/// 判断是否为非空、非参数的数值型恒真常量。
fn isConstTrue(expression: &dyn expression::Expression) -> bool {
    let Some(constant) = expression.as_any().downcast_ref::<expression::Constant>() else {
        return false;
    };
    if constant.DeferredExpr.is_some() || constant.ParamMarker.is_some() || constant.Value.IsNull()
    {
        return false;
    }
    match constant.Value.Kind() {
        types::datum::KindInt64 => constant.Value.GetInt64() != 0,
        types::datum::KindUint64 => constant.Value.GetUint64() != 0,
        types::datum::KindFloat32 | types::datum::KindFloat64 => constant.Value.GetFloat64() != 0.0,
        _ => false,
    }
}

/// 窗口 PARTITION BY 是否与数据源句柄列前缀一致（可推 TopN）。
pub fn checkPartitionBy(window: &LogicalWindow, source: &DataSource) -> bool {
    if window.PartitionBy.is_empty() {
        return true;
    }
    let Some(handle_columns) = source.HandleCols.as_ref() else {
        return false;
    };
    window.PartitionBy.len() <= handle_columns.NumCols()
        && window.PartitionBy.iter().enumerate().all(|(index, item)| {
            handle_columns
                .GetCol(index)
                .is_some_and(|column| item.Col.EqualColumn(column))
        })
}

/// 列与常量比较且校对规则一致时，视为可上拉常量谓词。
fn validCompareConstantPredicate(
    context: &dyn expression::EvalContext,
    candidate: &dyn expression::Expression,
) -> bool {
    let Some(function) = candidate.as_any().downcast_ref::<ScalarFunction>() else {
        return false;
    };
    if !matches!(
        function.FuncName.L.as_str(),
        parser_ast::GT | parser_ast::GE | parser_ast::LT | parser_ast::LE | parser_ast::EQ
    ) || function.GetArgs().len() != 2
    {
        return false;
    }
    let args = function.GetArgs();
    [(0, 1), (1, 0)]
        .into_iter()
        .any(|(column_index, constant_index)| {
            let Some(column) = args[column_index].as_any().downcast_ref::<Column>() else {
                return false;
            };
            let Some(constant) = args[constant_index]
                .as_any()
                .downcast_ref::<expression::Constant>()
            else {
                return false;
            };
            constant.GetType(context).is_some_and(|field_type| {
                field_type.GetCollate() == column.GetStaticType().GetCollate()
            })
        })
}

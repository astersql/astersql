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

// 逻辑 Join 算子：内连接、外连接、半连接及其谓词分类与下推。
//
// 将 ON/WHERE 条件划分为等值、左右单侧与其它条件；支持外连接转内连接、
// 键信息推导、基数估计与函数依赖（FD）提取。

use crate::*;
use base::PlanContext as BasePlanContext;
use std::any::Any;
use std::collections::HashMap;

/// Return Go's effective NDV for join costing: zero in pseudo statistics is
/// an unknown placeholder and falls back to the input cardinality.
pub(crate) fn effective_join_column_ndv(stats: &StatsInfo, column_id: i64) -> f64 {
    stats
        .ColNDVs
        .get(&column_id)
        .copied()
        .filter(|ndv| *ndv > 0.0)
        .unwrap_or((stats.RowCount * 0.8).min(8_000.0))
}

/// Bridges the object-safe core context into cardinality's minimal context.
/// The estimate only needs the shared session, expression, and ranger services.
struct JoinCardinalityContext<'a>(&'a dyn BasePlanContext);

impl cardinality::CardinalityContext for JoinCardinalityContext<'_> {
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

/// 再导出连接类型枚举（Inner/LeftOuter/Semi 等）。
pub use base::JoinType;

/// Hint 位：偏好 Hash Join。
const PREFER_HASH_JOIN: u64 = 1 << 0;
/// Hint 位：偏好 Merge Join。
const PREFER_MERGE_JOIN: u64 = 1 << 1;
/// Hint 位：偏好 Index Join。
const PREFER_INDEX_JOIN: u64 = 1 << 2;
const PREFER_NO_INDEX_JOIN: u64 = 1 << 9;
const PREFER_NO_INDEX_HASH_JOIN: u64 = 1 << 10;
const PREFER_NO_INDEX_MERGE_JOIN: u64 = 1 << 11;

/// 逻辑连接算子：连接类型、条件分组、偏好 Hint 与冗余列映射。
pub struct LogicalJoin {
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    pub JoinType: JoinType,
    pub Reordered: bool,
    pub StraightJoin: bool,
    pub PreferJoinType: u64,
    pub PreferJoinOrder: bool,
    pub InternalPreferJoinOrder: bool,
    pub LeftPreferJoinType: u64,
    pub RightPreferJoinType: u64,
    pub EqualConditions: Vec<Expression>,
    pub NAEQConditions: Vec<Expression>,
    pub LeftConditions: Vec<Expression>,
    pub RightConditions: Vec<Expression>,
    pub OtherConditions: Vec<Expression>,
    pub LeftProperties: Vec<Vec<Column>>,
    pub RightProperties: Vec<Vec<Column>>,
    pub FullSchema: Option<Schema>,
    pub FullNames: NameSlice,
    pub RedundantColsToOutputIdx: HashMap<i64, usize>,
    pub PreferCorrelate: bool,
    pub EqualCondOutCnt: f64,
    pub FromDecorrelatedApply: bool,
    /// The InnerJoin was synthesized from a SemiJoin.
    pub FromSemiJoinRewrite: bool,
    /// IN-subquery expansion uses the same join shape but its equality must
    /// retain Go's empty-aware handling during predicate pushdown.
    pub FromInSubqueryRewrite: bool,
}

/// 默认内连接、无条件、无偏好。
impl Default for LogicalJoin {
    fn default() -> Self {
        Self {
            LogicalSchemaProducer: LogicalSchemaProducer::default(),
            JoinType: JoinType::InnerJoin,
            Reordered: false,
            StraightJoin: false,
            PreferJoinType: 0,
            PreferJoinOrder: false,
            InternalPreferJoinOrder: false,
            LeftPreferJoinType: 0,
            RightPreferJoinType: 0,
            EqualConditions: Vec::new(),
            NAEQConditions: Vec::new(),
            LeftConditions: Vec::new(),
            RightConditions: Vec::new(),
            OtherConditions: Vec::new(),
            LeftProperties: Vec::new(),
            RightProperties: Vec::new(),
            FullSchema: None,
            FullNames: NameSlice(Vec::new()),
            RedundantColsToOutputIdx: HashMap::new(),
            PreferCorrelate: false,
            EqualCondOutCnt: 0.0,
            FromDecorrelatedApply: false,
            FromSemiJoinRewrite: false,
            FromInSubqueryRewrite: false,
        }
    }
}

impl LogicalJoin {
    /// 初始化基类逻辑计划，算子名为 Join。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "Join", offset);
        self
    }

    /// 生成 EXPLAIN：连接类型与各类条件个数。
    pub fn ExplainInfo(&self) -> String {
        let mut result = self.JoinType.to_string();
        for (label, conditions) in [
            ("equal", &self.EqualConditions),
            ("left cond", &self.LeftConditions),
            ("right cond", &self.RightConditions),
            ("other cond", &self.OtherConditions),
        ] {
            if !conditions.is_empty() {
                result.push_str(&format!(", {label}:{}", conditions.len()));
            }
        }
        result
    }

    /// 按 UniqueID 映射替换全部连接条件中的列引用。
    pub fn ReplaceExprColumns(&mut self, replacements: &HashMap<i64, Column>) {
        for condition in self.all_conditions_mut() {
            *condition = replace_join_expr(condition, replacements);
        }
    }

    /// 可变迭代全部连接条件列表。
    fn all_conditions_mut(&mut self) -> impl Iterator<Item = &mut Expression> {
        self.EqualConditions
            .iter_mut()
            .chain(&mut self.LeftConditions)
            .chain(&mut self.RightConditions)
            .chain(&mut self.OtherConditions)
            .chain(&mut self.NAEQConditions)
    }
    /// 只读迭代全部连接条件列表。
    fn all_conditions(&self) -> impl Iterator<Item = &Expression> {
        self.EqualConditions
            .iter()
            .chain(&self.LeftConditions)
            .chain(&self.RightConditions)
            .chain(&self.OtherConditions)
            .chain(&self.NAEQConditions)
    }

    /// 按表达式引用列判断条件属于左/右/双侧或常量。
    fn side_of(
        schema_left: &Schema,
        schema_right: &Schema,
        expression: &Expression,
    ) -> ConditionSide {
        let columns = expression::ExtractColumns(expression.as_ref());
        if columns.is_empty() {
            return ConditionSide::Constant;
        }
        let left = columns.iter().all(|column| schema_left.Contains(column));
        let right = columns.iter().all(|column| schema_right.Contains(column));
        match (left, right) {
            (true, false) => ConditionSide::Left,
            (false, true) => ConditionSide::Right,
            _ => ConditionSide::Both,
        }
    }

    /// 将条件列表按左右 Schema 分类为等值/左/右/其它。
    pub fn ExtractOnCondition(
        &self,
        conditions: Vec<Expression>,
    ) -> (
        Vec<Expression>,
        Vec<Expression>,
        Vec<Expression>,
        Vec<Expression>,
    ) {
        let [left, right] = self.Children() else {
            return (Vec::new(), Vec::new(), Vec::new(), conditions);
        };
        extract_on_condition(conditions, left.Schema(), right.Schema())
    }

    /// 分类后追加到本 Join 的对应条件列表。
    pub fn AttachOnConds(&mut self, conditions: Vec<Expression>) {
        let (mut equal, mut left, mut right, mut other) = self.ExtractOnCondition(conditions);
        equal.append(&mut self.EqualConditions);
        left.append(&mut self.LeftConditions);
        right.append(&mut self.RightConditions);
        other.append(&mut self.OtherConditions);
        self.EqualConditions = equal;
        self.LeftConditions = left;
        self.RightConditions = right;
        self.OtherConditions = other;
    }

    /// 合并另一个 Join 的全部条件（用于重排/合并）。
    pub fn AppendJoinConds(&mut self, other: &mut LogicalJoin) {
        self.EqualConditions.append(&mut other.EqualConditions);
        self.NAEQConditions.append(&mut other.NAEQConditions);
        self.LeftConditions.append(&mut other.LeftConditions);
        self.RightConditions.append(&mut other.RightConditions);
        self.OtherConditions.append(&mut other.OtherConditions);
    }

    /// 由等值/比较条件推导可下推的 IS NOT NULL（内连接/外连接保留侧）。
    fn derive_inner_join_not_null_conditions(&self) -> Result<(Vec<Expression>, Vec<Expression>)> {
        if self.FromInSubqueryRewrite {
            return Ok((Vec::new(), Vec::new()));
        }
        let derive_left = matches!(
            self.JoinType,
            JoinType::InnerJoin
                | JoinType::RightOuterJoin
                | JoinType::SemiJoin
                | JoinType::LeftOuterSemiJoin
        );
        let derive_right = matches!(
            self.JoinType,
            JoinType::InnerJoin
                | JoinType::LeftOuterJoin
                | JoinType::SemiJoin
                | JoinType::LeftOuterSemiJoin
        );
        if !derive_left && !derive_right {
            return Ok((Vec::new(), Vec::new()));
        }
        let Some([left, right]) = self.Children().get(..2) else {
            return Ok((Vec::new(), Vec::new()));
        };
        let Some(context) = self.SCtx() else {
            return Ok((Vec::new(), Vec::new()));
        };
        // A semi-join rewrite may expose a computed key through its grouping
        // aggregate. Only a direct source column may receive a new filter.
        fn direct_rewrite_key(plan: &dyn LogicalPlan, column: &Column) -> bool {
            if let Some(projection) = plan.as_any().downcast_ref::<crate::LogicalProjection>() {
                let Some(index) = projection.Schema().ColumnIndex(column) else {
                    return false;
                };
                let Some(source) = projection
                    .Exprs
                    .get(index)
                    .and_then(|expr| expr.as_column())
                else {
                    return false;
                };
                return projection
                    .Children()
                    .first()
                    .is_some_and(|child| direct_rewrite_key(child.as_ref(), source));
            }
            if let Some(aggregate) = plan.as_any().downcast_ref::<crate::LogicalAggregation>() {
                let Some(index) = aggregate.Schema().ColumnIndex(column) else {
                    return false;
                };
                let Some(source) = aggregate
                    .GroupByItems
                    .get(index)
                    .and_then(|expr| expr.as_column())
                else {
                    return false;
                };
                return aggregate
                    .Children()
                    .first()
                    .is_some_and(|child| direct_rewrite_key(child.as_ref(), source));
            }
            true
        }
        let mut left_columns = HashMap::<i64, Column>::new();
        let mut right_columns = HashMap::<i64, Column>::new();
        // Go's ExtractOnCondition derives NOT NULL from col-op-col predicates
        // before classifying them.  Include equality conditions for outer joins
        // as well, while `derive_left`/`derive_right` still restrict derivation
        // to the null-producing side.
        let conditions: Box<dyn Iterator<Item = &Expression>> = match self.JoinType {
            JoinType::InnerJoin
            | JoinType::LeftOuterJoin
            | JoinType::RightOuterJoin
            | JoinType::SemiJoin
            | JoinType::LeftOuterSemiJoin => {
                Box::new(self.EqualConditions.iter().chain(&self.OtherConditions))
            }
            _ => Box::new(std::iter::empty()),
        };
        for condition in conditions {
            let Some(function) = condition
                .as_any()
                .downcast_ref::<expression::ScalarFunction>()
                .filter(|function| {
                    matches!(
                        function.FuncName.L.as_str(),
                        parser_ast::EQ
                            | parser_ast::NE
                            | parser_ast::LT
                            | parser_ast::LE
                            | parser_ast::GT
                            | parser_ast::GE
                    )
                })
            else {
                continue;
            };
            if !expression::IsColOpCol(function).2 {
                continue;
            }
            let (Some(first), Some(second)) = expression::ExtractColumnsFromColOpCol(function)
            else {
                continue;
            };
            let nullable = |column: &Column| {
                column
                    .RetType
                    .as_ref()
                    .is_none_or(|field_type| !mysql::r#type::HasNotNullFlag(field_type.GetFlag()))
            };
            let sides = if left.Schema().Contains(first) && right.Schema().Contains(second) {
                Some((first, second))
            } else if left.Schema().Contains(second) && right.Schema().Contains(first) {
                Some((second, first))
            } else {
                None
            };
            let Some((left_expression_column, right_expression_column)) = sides else {
                continue;
            };
            // Go's ExtractOnCondition keeps IN-derived equality in OtherConditions
            // without deriving IS NOT NULL; EXISTS equality remains null rejecting.
            if expression::IsEQCondFromIn(condition.as_ref()) {
                continue;
            }
            let left_column = left
                .Schema()
                .RetrieveColumn(left_expression_column)
                .unwrap_or(left_expression_column);
            let right_column = right
                .Schema()
                .RetrieveColumn(right_expression_column)
                .unwrap_or(right_expression_column);
            let accepts_derived_predicate = |child: &dyn LogicalPlan| {
                child
                    .as_any()
                    .downcast_ref::<LogicalCTE>()
                    .is_none_or(|cte| cte.Cte.borrow().IsOuterMostCTE)
            };
            if derive_left
                && accepts_derived_predicate(left.as_ref())
                && nullable(left_column)
                && (!self.FromSemiJoinRewrite || direct_rewrite_key(left.as_ref(), left_column))
            {
                left_columns.insert(left_column.UniqueID, left_column.Clone());
            }
            if derive_right
                && accepts_derived_predicate(right.as_ref())
                && nullable(right_column)
                && (!self.FromSemiJoinRewrite || direct_rewrite_key(right.as_ref(), right_column))
            {
                right_columns.insert(right_column.UniqueID, right_column.Clone());
            }
        }
        let build = |column: Column| -> Result<Expression> {
            let tiny = *expression::types::NewFieldType(expression::mysql::TypeTiny);
            let is_null = expression::NewFunction(
                context.GetExprCtx(),
                parser_ast::IsNull,
                tiny.clone(),
                vec![Box::new(column)],
            )
            .map_err(|error| PlannerError(error.to_string()))?;
            expression::NewFunction(
                context.GetExprCtx(),
                parser_ast::UnaryNot,
                tiny,
                vec![is_null],
            )
            .map_err(|error| PlannerError(error.to_string()))
        };
        Ok((
            left_columns
                .into_values()
                .map(&build)
                .collect::<Result<Vec<_>>>()?,
            right_columns
                .into_values()
                .map(build)
                .collect::<Result<Vec<_>>>()?,
        ))
    }

    /// 谓词下推：外连转内连、DNF 提取、按侧分流并刷新等值条件。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        let [left, right] = self.Children() else {
            return Ok(predicates);
        };
        let left_schema = left.Schema().Clone();
        let right_schema = right.Schema().Clone();
        let marker_id = self
            .Schema()
            .Columns
            .last()
            .filter(|column| !left_schema.Contains(column) && !right_schema.Contains(column))
            .map(|column| column.UniqueID);
        let mut predicates = predicates;
        if self.JoinType == JoinType::LeftOuterSemiJoin
            && let Some(marker_id) = marker_id
            && predicates.iter().any(|predicate| {
                predicate
                    .as_column()
                    .is_some_and(|column| column.UniqueID == marker_id)
            })
        {
            // A positive filter on the boolean marker produced for EXISTS is
            // exactly a semi join. Go performs this during predicate push-down
            // and removes the now-redundant marker selection.
            // EXISTS 布尔标记上的正过滤等价于半连接；下推时去掉冗余标记选择。
            predicates.retain(|predicate| {
                !predicate
                    .as_column()
                    .is_some_and(|column| column.UniqueID == marker_id)
            });
            self.JoinType = JoinType::SemiJoin;
            self.MergeSchema();
        }
        if let Some(context) = self.SCtx().cloned() {
            let reject_left = predicates.iter().any(|predicate| {
                planner_util::IsNullRejected(context.as_ref(), &left_schema, predicate.CloneExpr())
            });
            let reject_right = predicates.iter().any(|predicate| {
                planner_util::IsNullRejected(context.as_ref(), &right_schema, predicate.CloneExpr())
            });
            self.ConvertOuterToInnerJoin(reject_left, reject_right);
        }
        if matches!(self.JoinType, JoinType::InnerJoin | JoinType::SemiJoin) {
            // Keep Go's ordering here: DNF common-factor extraction sees both
            // existing ON conditions and incoming WHERE predicates before the
            // conditions are classified for either child.
            // 保持 Go 顺序：DNF 公因子提取先看到 ON+WHERE，再向子节点分类。
            let mut combined = Vec::new();
            combined.append(&mut self.LeftConditions);
            combined.append(&mut self.RightConditions);
            combined.append(&mut self.EqualConditions);
            combined.append(&mut self.OtherConditions);
            combined.append(&mut predicates);
            if let Some(context) = self.SCtx() {
                let mut build_context =
                    expression::exprctx::CtxWithTruncateResult::Original(context.GetExprCtx());
                predicates = expression::ExtractFiltersFromDNFs(&mut build_context, combined);
                predicates = rule_util::ApplyPredicateSimplificationForJoin(
                    context.clone(),
                    predicates,
                    &left_schema,
                    &right_schema,
                    true,
                    None,
                );
            } else {
                predicates = combined;
            }
        } else if matches!(
            self.JoinType,
            JoinType::LeftOuterJoin
                | JoinType::RightOuterJoin
                | JoinType::LeftOuterSemiJoin
                | JoinType::AntiLeftOuterSemiJoin
        ) && let Some(context) = self.SCtx()
        {
            let mut build_context =
                expression::exprctx::CtxWithTruncateResult::Original(context.GetExprCtx());
            predicates = expression::ExtractFiltersFromDNFs(&mut build_context, predicates);
        }
        let mut left_push = Vec::new();
        let mut right_push = Vec::new();
        let mut retained = Vec::new();
        let mut join_conditions = Vec::new();
        // 按连接类型与条件所属侧决定下推左/右、保留或并入 Join ON。
        for predicate in predicates {
            match (
                self.JoinType,
                Self::side_of(&left_schema, &right_schema, &predicate),
            ) {
                (JoinType::InnerJoin | JoinType::SemiJoin, ConditionSide::Left) => {
                    left_push.push(predicate)
                }
                (JoinType::InnerJoin | JoinType::SemiJoin, ConditionSide::Right) => {
                    right_push.push(predicate)
                }
                (JoinType::InnerJoin | JoinType::SemiJoin, ConditionSide::Both) => {
                    if let Some(context) = self.SCtx() {
                        let mut build_context =
                            expression::exprctx::CtxWithTruncateResult::Original(
                                context.GetExprCtx(),
                            );
                        if let Some(relaxed) =
                            derive_relaxed_dnf(&mut build_context, predicate.as_ref(), &left_schema)
                        {
                            left_push.push(relaxed);
                        }
                        if let Some(relaxed) = derive_relaxed_dnf(
                            &mut build_context,
                            predicate.as_ref(),
                            &right_schema,
                        ) {
                            right_push.push(relaxed);
                        }
                    }
                    join_conditions.push(predicate)
                }
                (JoinType::InnerJoin | JoinType::SemiJoin, ConditionSide::Constant) => {
                    self.OtherConditions.push(predicate)
                }
                (
                    JoinType::LeftOuterJoin
                    | JoinType::AntiSemiJoin
                    | JoinType::LeftOuterSemiJoin
                    | JoinType::AntiLeftOuterSemiJoin,
                    ConditionSide::Left,
                ) => left_push.push(predicate),
                (JoinType::RightOuterJoin, ConditionSide::Right) => right_push.push(predicate),
                (JoinType::LeftOuterJoin, ConditionSide::Both) => {
                    if let Some(context) = self.SCtx() {
                        let mut build_context =
                            expression::exprctx::CtxWithTruncateResult::Original(
                                context.GetExprCtx(),
                            );
                        if let Some(relaxed) =
                            derive_relaxed_dnf(&mut build_context, predicate.as_ref(), &left_schema)
                        {
                            left_push.push(relaxed);
                        }
                    }
                    retained.push(predicate);
                }
                (JoinType::RightOuterJoin, ConditionSide::Both) => {
                    if let Some(context) = self.SCtx() {
                        let mut build_context =
                            expression::exprctx::CtxWithTruncateResult::Original(
                                context.GetExprCtx(),
                            );
                        if let Some(relaxed) = derive_relaxed_dnf(
                            &mut build_context,
                            predicate.as_ref(),
                            &right_schema,
                        ) {
                            right_push.push(relaxed);
                        }
                    }
                    retained.push(predicate);
                }
                _ => retained.push(predicate),
            }
        }
        self.AttachOnConds(join_conditions);
        if let Some(context) = self.SCtx() {
            let mut build_context =
                expression::exprctx::CtxWithTruncateResult::Original(context.GetExprCtx());
            let (derive_left, derive_right) = match self.JoinType {
                JoinType::LeftOuterJoin
                | JoinType::LeftOuterSemiJoin
                | JoinType::AntiLeftOuterSemiJoin => (false, true),
                JoinType::RightOuterJoin => (true, false),
                _ => (false, false),
            };
            for condition in &self.OtherConditions {
                if derive_left
                    && let Some(relaxed) =
                        derive_relaxed_dnf(&mut build_context, condition.as_ref(), &left_schema)
                {
                    left_push.push(relaxed);
                }
                if derive_right
                    && let Some(relaxed) =
                        derive_relaxed_dnf(&mut build_context, condition.as_ref(), &right_schema)
                {
                    right_push.push(relaxed);
                }
            }
        }
        if self.JoinType == JoinType::LeftOuterJoin {
            right_push.append(&mut self.RightConditions);
        } else if self.JoinType == JoinType::InnerJoin {
            right_push.extend(self.RightConditions.iter().cloned());
        }
        if self.JoinType == JoinType::RightOuterJoin {
            left_push.append(&mut self.LeftConditions);
        } else if self.JoinType == JoinType::InnerJoin {
            left_push.extend(self.LeftConditions.iter().cloned());
        }
        let (left_not_null, right_not_null) = self.derive_inner_join_not_null_conditions()?;
        left_push.extend(left_not_null);
        right_push.extend(right_not_null);
        left_push = expression::RemoveDupExprs(left_push);
        right_push = expression::RemoveDupExprs(right_push);
        let (left_retained, right_retained) = if let [left, right] = self.Children_mut() {
            (
                PredicatePushDownPlan(left, left_push)?,
                PredicatePushDownPlan(right, right_push)?,
            )
        } else {
            (Vec::new(), Vec::new())
        };
        if let [left, right] = self.Children_mut() {
            AttachSelectionToPlan(left, left_retained)?;
            AttachSelectionToPlan(right, right_retained)?;
        }
        // Go refreshes equality conditions and key information after PPD has
        // moved an inner JOIN's Selection predicates onto the join.
        // 谓词下推后刷新等值条件与键信息（与 Go 一致）。
        self.updateEQCond();
        for child in self.Children_mut() {
            child.BuildKeyInfo();
        }
        self.BuildKeyInfo();
        Ok(retained)
    }

    /// 根入口：NOT 下推与简化；恒假则替换为 TableDual。
    pub fn PredicatePushDownRoot(
        &mut self,
        predicates: Vec<Expression>,
    ) -> Result<(Vec<Expression>, Option<LogicalPlanRef>)> {
        let predicates = self.SCtx().cloned().map_or(predicates.clone(), |context| {
            let mut build_context =
                expression::exprctx::CtxWithTruncateResult::Original(context.GetExprCtx());
            let predicates = predicates
                .into_iter()
                .map(|predicate| expression::PushDownNot(&mut build_context, predicate))
                .collect();
            rule_util::ApplyPredicateSimplification(context, predicates, false, None)
        });
        if crate::Conds2TableDual(&predicates) {
            let context = self
                .SCtx()
                .cloned()
                .ok_or_else(|| PlannerError("LogicalJoin has no plan context".into()))?;
            let mut dual = LogicalTableDual {
                RowCount: 0,
                ..LogicalTableDual::default()
            }
            .Init(context, self.QueryBlockOffset());
            dual.SetSchema(self.Schema().Clone());
            dual.SetOutputNames(self.OutputNames().Shallow());
            return Ok((Vec::new(), Some(Box::new(dual))));
        }
        Ok((self.PredicatePushDown(predicates)?, None))
    }

    /// 汇总父用列与连接条件引用，拆成左右子树所需列。
    pub fn ExtractUsedCols(&self, parent_used_cols: &[Column]) -> (Vec<Column>, Vec<Column>) {
        let Some([left, right]) = self.Children().get(..2) else {
            return (Vec::new(), Vec::new());
        };
        let mut all = parent_used_cols.to_vec();
        for condition in self.all_conditions() {
            all.extend(
                expression::ExtractColumns(condition.as_ref())
                    .into_iter()
                    .cloned(),
            );
        }
        let mut left_cols = Vec::new();
        let mut right_cols = Vec::new();
        for column in all {
            if plan_can_resolve_used_col(left.as_ref(), &column) {
                left_cols.push(column);
            } else if plan_can_resolve_used_col(right.as_ref(), &column) {
                right_cols.push(column);
            }
        }
        // Go carries equal-condition keys as directional left/right pairs.
        // Alias schemas can temporarily expose the same UniqueID on both
        // sides; the generic resolver above would then assign both keys left
        // and let right-side aggregation prune its firstrow join output.
        if self.JoinType == JoinType::InnerJoin && right.as_any().is::<crate::LogicalAggregation>()
        {
            for equality in self.EqualConditions.iter().chain(&self.NAEQConditions) {
                let Some(function) = equality.as_scalar_function() else {
                    continue;
                };
                for argument in function.GetArgs() {
                    for column in expression::ExtractColumns(argument.as_ref()) {
                        if plan_can_resolve_used_col(left.as_ref(), column) {
                            left_cols.push(column.Clone());
                        }
                        if plan_can_resolve_used_col(right.as_ref(), column) {
                            right_cols.push(column.Clone());
                        }
                    }
                }
            }
        }
        // Keep the same column occurrence multiplicity as Go.  A projection
        // may intentionally expose the same UniqueID more than once (for
        // example a CTE output plus its join key); deduplicating here drops
        // those implicit columns from the physical plan tree.
        (left_cols, right_cols)
    }

    /// 分别裁剪左右子树后合并 Schema，再按父用列收紧输出。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let full_schema = self.FullSchema.as_ref().map(Schema::Clone);
        let (left_cols, right_cols) = self.ExtractUsedCols(parent_used_cols);
        // Preserve the pre-prune full schema.  Go keeps this information on
        // the Join so an upper Join can still resolve a key hidden by an
        // earlier column-prune pass; rebuilding it only from pruned children
        // loses that cross-level wiring.
        if let [left, right] = self.Children_mut() {
            left.PruneColumns(&left_cols)?;
            right.PruneColumns(&right_cols)?;
        }
        self.MergeSchema();
        if full_schema.is_some() {
            self.FullSchema = full_schema;
        }
        // Go invokes InlineProjection after child pruning.  It preserves the
        // join's visible schema policy (including the empty-use fallback and
        // key metadata) instead of reimplementing only the column filter.
        self.LogicalSchemaProducer
            .InlineProjection(parent_used_cols);
        Ok(())
    }

    /// 按连接类型合并左右 Schema（半连接仅左；外半连接附带标记列）。
    pub fn MergeSchema(&mut self) {
        let Some([left, right]) = self.Children().get(..2) else {
            return;
        };
        let left_schema = left.Schema().Clone();
        let right_schema = right.Schema().Clone();
        let full = merge_schema(&left_schema, &right_schema);
        let schema = match self.JoinType {
            JoinType::SemiJoin | JoinType::AntiSemiJoin => left_schema,
            JoinType::LeftOuterSemiJoin | JoinType::AntiLeftOuterSemiJoin => {
                let mut schema = left_schema;
                if let Some(marker) = self
                    .Schema()
                    .Columns
                    .last()
                    .filter(|column| !schema.Contains(column))
                    .cloned()
                {
                    schema.Columns.push(marker);
                }
                schema
            }
            _ => full,
        };
        self.SetSchema(schema);
    }

    /// 按连接类型从子键与等值键推导输出 PKOrUK。
    pub fn BuildKeyInfo(&mut self) {
        self.base_mut().BuildKeyInfo();
        let Some([left, right]) = self.Children().get(..2) else {
            return;
        };
        let left_keys = left.Schema().PKOrUK.clone();
        let right_keys = right.Schema().PKOrUK.clone();
        let (join_left, join_right) = self.GetJoinKeys();
        self.Schema_mut().PKOrUK.clear();
        match self.JoinType {
            JoinType::SemiJoin
            | JoinType::AntiSemiJoin
            | JoinType::LeftOuterSemiJoin
            | JoinType::AntiLeftOuterSemiJoin => self.Schema_mut().PKOrUK = left_keys,
            JoinType::InnerJoin => {
                // Keep Go's cartesian-product guard: without a column equality there is no
                // one-to-one relationship from which either child's keys can be derived.
                // 无列等值时无法建立一一关系，不能继承任一侧键（笛卡尔积守卫）。
                if self.EqualConditions.is_empty() {
                    return;
                }
                if right_keys.iter().any(|key| {
                    key.iter().all(|column| {
                        join_right
                            .iter()
                            .any(|join| join.UniqueID == column.UniqueID)
                    })
                }) {
                    self.Schema_mut().PKOrUK.extend(left_keys.clone());
                }
                if left_keys.iter().any(|key| {
                    key.iter().all(|column| {
                        join_left
                            .iter()
                            .any(|join| join.UniqueID == column.UniqueID)
                    })
                }) {
                    self.Schema_mut().PKOrUK.extend(right_keys);
                }
            }
            JoinType::LeftOuterJoin => {
                if self.EqualConditions.is_empty() {
                    return;
                }
                if right_keys.iter().any(|key| {
                    key.iter().all(|column| {
                        join_right
                            .iter()
                            .any(|join| join.UniqueID == column.UniqueID)
                    })
                }) {
                    self.Schema_mut().PKOrUK.extend(left_keys);
                }
            }
            JoinType::RightOuterJoin => {
                if self.EqualConditions.is_empty() {
                    return;
                }
                if left_keys.iter().any(|key| {
                    key.iter().all(|column| {
                        join_left
                            .iter()
                            .any(|join| join.UniqueID == column.UniqueID)
                    })
                }) {
                    self.Schema_mut().PKOrUK.extend(right_keys);
                }
            }
        }
    }

    /// 推导连接基数：内连接用 NDV 分母；外/半连接另有下界或衰减。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        let (left_keys, right_keys) = self.GetJoinKeys();
        let (left_na_join_keys, right_na_join_keys) = self.GetNAJoinKeys();
        let is_cartesian = self.EqualConditions.is_empty();
        let (mut left_stats, mut right_stats, left_schema, right_schema) = {
            let [left, right] = self.Children_mut() else {
                return Err(PlannerError("LogicalJoin requires two children".to_owned()));
            };
            let left_stats = left.DeriveStats(reload)?.0;
            let right_stats = right.DeriveStats(reload)?.0;
            (
                left_stats,
                right_stats,
                left.Schema().Clone(),
                right.Schema().Clone(),
            )
        };
        // A zero pseudo NDV means "unknown". Feed the cardinality estimator
        // the same effective fallback used by Go instead of allowing an
        // equality join to degenerate into a Cartesian row-count estimate.
        for column in &left_schema.Columns {
            let ndv = left_stats.ColNDVs.entry(column.UniqueID).or_default();
            if *ndv <= 0.0 {
                *ndv = (left_stats.RowCount * 0.8).min(8_000.0);
            }
        }
        for column in &right_schema.Columns {
            let ndv = right_stats.ColNDVs.entry(column.UniqueID).or_default();
            if *ndv <= 0.0 {
                *ndv = (right_stats.RowCount * 0.8).min(8_000.0);
            }
        }
        let inner_count = if is_cartesian {
            left_stats.RowCount * right_stats.RowCount
        } else {
            let context = self
                .SCtx()
                .cloned()
                .ok_or_else(|| PlannerError("LogicalJoin has no plan context".to_owned()))?;
            cardinality::EstimateFullJoinRowCount(
                &JoinCardinalityContext(context.as_ref()),
                false,
                &left_stats,
                &right_stats,
                &left_keys,
                &right_keys,
                &left_schema,
                &right_schema,
                Some(&left_na_join_keys),
                Some(&right_na_join_keys),
            )
        }
        .max(0.0);
        self.EqualCondOutCnt = inner_count;
        let row_count = match self.JoinType {
            JoinType::InnerJoin => inner_count,
            JoinType::LeftOuterJoin => inner_count.max(left_stats.RowCount),
            JoinType::RightOuterJoin => inner_count.max(right_stats.RowCount),
            JoinType::SemiJoin
            | JoinType::AntiSemiJoin
            | JoinType::LeftOuterSemiJoin
            | JoinType::AntiLeftOuterSemiJoin => left_stats.RowCount * 0.8,
        };
        let mut stats = StatsInfo {
            RowCount: row_count,
            StatsVersion: left_stats.StatsVersion.max(right_stats.StatsVersion),
            ..StatsInfo::default()
        };
        if matches!(self.JoinType, JoinType::SemiJoin | JoinType::AntiSemiJoin) {
            // Go reduces both the retained row count and each retained column
            // NDV by SelectionFactor for semi joins. Capping at output rows
            // loses this second selectivity adjustment when an NDV already
            // equals the input row count (TPC-H Q22).
            stats.ColNDVs = left_stats
                .ColNDVs
                .iter()
                .map(|(&column_id, &ndv)| {
                    (column_id, ndv * cost::factors_thresholds::SelectionFactor)
                })
                .collect();
            self.SetStats(stats.clone());
            return Ok((stats, true));
        }
        // Pseudo table statistics may carry a zero placeholder for a column
        // NDV. Go treats that as unavailable rather than as a real zero and,
        // for equality keys, propagates the smaller effective NDV from both
        // sides. Keeping zero here makes a following GROUP BY collapse to one
        // row (for example TPC-H Q13's customer/order key aggregation).
        let mut equality_ndvs = HashMap::new();
        for (left_key, right_key) in left_keys.iter().zip(&right_keys) {
            let left_ndv = effective_join_column_ndv(&left_stats, left_key.UniqueID);
            let right_ndv = effective_join_column_ndv(&right_stats, right_key.UniqueID);
            let equality_ndv = match self.JoinType {
                JoinType::LeftOuterJoin => left_ndv,
                JoinType::RightOuterJoin => right_ndv,
                _ => left_ndv.min(right_ndv),
            }
            .min(row_count);
            equality_ndvs.insert(left_key.UniqueID, equality_ndv);
            equality_ndvs.insert(right_key.UniqueID, equality_ndv);
        }
        for column in &self.Schema().Columns {
            let ndv = equality_ndvs
                .get(&column.UniqueID)
                .copied()
                .unwrap_or_else(|| {
                    let (source_stats, source_row_count) = if left_schema.Contains(column) {
                        (&left_stats, left_stats.RowCount)
                    } else if right_schema.Contains(column) {
                        (&right_stats, right_stats.RowCount)
                    } else {
                        (&left_stats, row_count)
                    };
                    source_stats
                        .ColNDVs
                        .get(&column.UniqueID)
                        .copied()
                        .filter(|ndv| *ndv > 0.0)
                        .unwrap_or(source_row_count)
                        .min(row_count)
                });
            stats.ColNDVs.insert(column.UniqueID, ndv);
        }
        stats.GroupNDVs = self.getGroupNDVs(&left_stats, &right_stats);
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 从等值条件提取左右连接键列。
    pub fn GetJoinKeys(&self) -> (Vec<Column>, Vec<Column>) {
        self.extract_keys_from(&self.EqualConditions)
    }
    /// 从 Null-Aware 等值条件提取连接键。
    pub fn GetNAJoinKeys(&self) -> (Vec<Column>, Vec<Column>) {
        self.extract_keys_from(&self.NAEQConditions)
    }
    /// GetJoinKeys 的别名。
    pub fn ExtractJoinKeys(&self) -> (Vec<Column>, Vec<Column>) {
        self.GetJoinKeys()
    }
    /// 从 eq/nulleq 列对条件中抽取左右键。
    fn extract_keys_from(&self, conditions: &[Expression]) -> (Vec<Column>, Vec<Column>) {
        let Some([left, right]) = self.Children().get(..2) else {
            return (Vec::new(), Vec::new());
        };
        let mut left_keys = Vec::new();
        let mut right_keys = Vec::new();
        for condition in conditions {
            let Some(function) = condition.as_scalar_function() else {
                continue;
            };
            if !matches!(function.FuncName.L.as_str(), "eq" | "nulleq")
                || function.GetArgs().len() != 2
            {
                continue;
            }
            let Some(first) = function.GetArgs()[0].as_column() else {
                continue;
            };
            let Some(second) = function.GetArgs()[1].as_column() else {
                continue;
            };
            if left.Schema().Contains(first) && right.Schema().Contains(second) {
                left_keys.push(first.clone());
                right_keys.push(second.clone());
            } else if left.Schema().Contains(second) && right.Schema().Contains(first) {
                left_keys.push(second.clone());
                right_keys.push(first.clone());
            }
        }
        (left_keys, right_keys)
    }

    /// 从全部连接条件提取关联列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.all_conditions()
            .flat_map(|condition| {
                expression::ExtractCorColumns(condition.as_ref())
                    .into_iter()
                    .map(CorrelatedColumn::Clone)
            })
            .collect()
    }
    /// 保留完全落在某一子树 Schema 内的列组。
    pub fn ExtractColGroups(&self, groups: &[Vec<Column>]) -> Vec<Vec<Column>> {
        groups
            .iter()
            .filter(|group| {
                self.Children()
                    .iter()
                    .any(|child| group.iter().all(|column| child.Schema().Contains(column)))
            })
            .cloned()
            .collect()
    }
    /// 潜在分区键：左右连接键并集。
    pub fn GetPotentialPartitionKeys(&self) -> Vec<Column> {
        let (left, right) = self.GetJoinKeys();
        left.into_iter().chain(right).collect()
    }
    /// 委托基类准备可能物理属性。
    pub fn PreparePossibleProperties(&mut self, children_have_tiflash: &[bool]) -> bool {
        self.base_mut()
            .PreparePossibleProperties(children_have_tiflash)
    }

    /// 按连接类型合并子 FD，内连接额外加入等值等价类后投影到输出列。
    pub fn ExtractFD(&mut self) -> fd::FDSet {
        let child_sets = self
            .Children_mut()
            .iter_mut()
            .map(|child| child.base_mut().ExtractFD().clone())
            .collect::<Vec<_>>();
        let mut result = match self.JoinType {
            JoinType::LeftOuterJoin
            | JoinType::SemiJoin
            | JoinType::AntiSemiJoin
            | JoinType::LeftOuterSemiJoin
            | JoinType::AntiLeftOuterSemiJoin => child_sets.first().cloned().unwrap_or_default(),
            JoinType::RightOuterJoin => child_sets.get(1).cloned().unwrap_or_default(),
            JoinType::InnerJoin => {
                let mut set = fd::FDSet::default();
                for child in &child_sets {
                    set.AddFrom(child);
                }
                set
            }
        };
        if self.JoinType == JoinType::InnerJoin {
            let (left, right) = self.GetJoinKeys();
            for (left, right) in left.iter().zip(right) {
                result.AddEquivalence(
                    fd::intset::NewFastIntSet(vec![left.UniqueID as i32]),
                    fd::intset::NewFastIntSet(vec![right.UniqueID as i32]),
                );
            }
        }
        let mut output = fd::intset::NewFastIntSet(Vec::new());
        for column in &self.Schema().Columns {
            output.Insert(column.UniqueID as i32);
        }
        result.ProjectCols(output);
        self.base_mut().SetFDs(result.clone());
        result
    }
    /// 临时按内连接提取 FD。
    pub fn ExtractFDForInnerJoin(&mut self) -> fd::FDSet {
        let original = self.JoinType;
        self.JoinType = JoinType::InnerJoin;
        let result = self.ExtractFD();
        self.JoinType = original;
        result
    }
    /// 外连接 FD（当前委托 ExtractFD）。
    pub fn ExtractFDForOuterJoin(&mut self) -> fd::FDSet {
        self.ExtractFD()
    }
    /// 半连接 FD（当前委托 ExtractFD）。
    pub fn ExtractFDForSemiJoin(&mut self) -> fd::FDSet {
        self.ExtractFD()
    }

    /// 常量传播入口（委托基类）。
    pub fn ConstantPropagation(&mut self) {
        self.base_mut().ConstantPropagation(&[]);
    }
    /// 替换条件中的全部列引用。
    pub fn ColumnSubstituteAll(&mut self, replacements: &HashMap<i64, Column>) {
        self.ReplaceExprColumns(replacements);
    }
    /// 标记本 Join 来自去关联 Apply。
    pub fn Decorrelate(&mut self) {
        self.FromDecorrelatedApply = true;
    }
    /// 半连接改写后关闭 PreferCorrelate。
    pub fn SemiJoinRewrite(&mut self) {
        if self.JoinType == JoinType::SemiJoin {
            self.PreferCorrelate = false;
        }
    }
    /// 设置物理连接算法偏好（Hash/Merge/Index）。
    pub fn SetPreferredJoinType(&mut self, prefer: u64) {
        self.PreferJoinType = prefer
            & (PREFER_HASH_JOIN
                | PREFER_MERGE_JOIN
                | PREFER_INDEX_JOIN
                | PREFER_NO_INDEX_JOIN
                | PREFER_NO_INDEX_HASH_JOIN
                | PREFER_NO_INDEX_MERGE_JOIN);
    }
    /// 同时设置算法偏好与连接顺序偏好。
    pub fn SetPreferredJoinTypeAndOrder(&mut self, prefer: u64, order: bool) {
        self.SetPreferredJoinType(prefer);
        self.PreferJoinOrder = order;
    }
    /// 登记冗余列 UniqueID 到输出列下标的映射。
    pub fn RegisterRedundantColumnMapping(&mut self, redundant: i64, output: usize) {
        self.RedundantColsToOutputIdx.insert(redundant, output);
    }
    /// 将冗余列解析为实际输出列。
    pub fn ResolveRedundantColumn(&self, column: &Column) -> Option<&Column> {
        if self.JoinType != JoinType::InnerJoin {
            return None;
        }
        self.RedundantColsToOutputIdx
            .get(&column.UniqueID)
            .filter(|index| **index < self.OutputNames().0.len())
            .and_then(|index| self.Schema().Columns.get(*index))
            .filter(|output| self.redundantColumnRemapTypesMatch(column, output))
    }
    /// 从 OtherConditions 再提取可升级的等值/单侧条件。
    pub fn updateEQCond(&mut self) {
        let conditions = std::mem::take(&mut self.OtherConditions);
        let (equal, left, right, other) = self.ExtractOnCondition(conditions);
        self.EqualConditions.extend(equal);
        self.LeftConditions.extend(left);
        self.RightConditions.extend(right);
        self.OtherConditions = other;
    }
    /// 将 Other 中的常量表达式同时推入左右条件。
    pub fn pushDownConstExpr(&mut self) {
        let constants = self
            .OtherConditions
            .iter()
            .filter(|expr| expression::ExtractColumns(expr.as_ref()).is_empty())
            .cloned()
            .collect::<Vec<_>>();
        self.LeftConditions.extend(constants.iter().cloned());
        self.RightConditions.extend(constants);
    }
    /// TopN 下推委托基类。
    pub fn pushDownTopNToChild(&mut self, top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        self.base_mut().PushDownTopN(top_n)
    }
    /// 外连接常量传播入口。
    pub fn outerJoinPropConst(&mut self) {
        self.ConstantPropagation();
    }
    /// 是否浅拷贝节点（固定 false）。
    pub fn Shallow(&self) -> bool {
        false
    }
    /// 返回基类逻辑计划引用。
    pub fn base(&self) -> &BaseLogicalPlan {
        &self.LogicalSchemaProducer.BaseLogicalPlan
    }
    /// 获取左右子节点统计与 Schema。
    pub fn GetJoinChildStatsAndSchema(
        &self,
    ) -> Option<((&StatsInfo, &Schema), (&StatsInfo, &Schema))> {
        self.base().GetJoinChildStatsAndSchema()
    }
    /// 将候选谓词并入 ON 条件。
    pub fn addCandidateSelection(&mut self, predicates: Vec<Expression>) {
        self.AttachOnConds(predicates);
    }
    /// 生成连接键隐式类型转换警告文案。
    pub fn appendImplicitJoinKeyConversionWarning(&self, left: &Column, right: &Column) -> String {
        format!(
            "join key {} is implicitly converted to match {}",
            left.UniqueID, right.UniqueID
        )
    }
    /// 合并并返回逻辑 Join Schema。
    pub fn buildLogicalJoinSchema(&mut self) -> Schema {
        self.MergeSchema();
        self.Schema().Clone()
    }
    /// 是否同时偏好多种冲突的物理连接算法。
    pub fn containDifferentJoinTypes(&self) -> bool {
        self.PreferJoinType.count_ones() > 1
            || self.LeftPreferJoinType.count_ones() > 1
            || self.RightPreferJoinType.count_ones() > 1
    }
    /// 若对保留侧 NULL 拒绝，则将外连接降为内连接。
    pub fn ConvertOuterToInnerJoin(&mut self, null_reject_left: bool, null_reject_right: bool) {
        self.JoinType = match self.JoinType {
            JoinType::LeftOuterJoin if null_reject_right => JoinType::InnerJoin,
            JoinType::RightOuterJoin if null_reject_left => JoinType::InnerJoin,
            other => other,
        };
    }
    /// 连接键列集合（用于 NOT NULL 推导）。
    pub fn deriveNotNullExpr(&self) -> Vec<Column> {
        self.GetJoinKeys()
            .0
            .into_iter()
            .chain(self.GetJoinKeys().1)
            .collect()
    }
    /// 返回 OtherConditions 副本。
    pub fn deriveOtherConditions(&self) -> Vec<Expression> {
        self.OtherConditions.clone()
    }
    /// 从列或 CAST(列) 中取出源列。
    pub fn extractCastSourceColumn(&self, expression: &Expression) -> Option<Column> {
        if let Some(column) = expression.as_column() {
            return Some(column.clone());
        }
        let function = expression.as_scalar_function()?;
        (function.FuncName.L == "cast")
            .then(|| {
                function
                    .GetArgs()
                    .first()
                    .and_then(|arg| arg.as_column())
                    .cloned()
            })
            .flatten()
    }
    /// 返回直接子节点 Schema 列表。
    pub fn getAllJoinLeaf(&self) -> Vec<Schema> {
        self.Children()
            .iter()
            .map(|child| child.Schema().Clone())
            .collect()
    }
    /// 列所在子节点下标。
    pub fn getProj(&self, column: &Column) -> Option<usize> {
        self.Children()
            .iter()
            .position(|child| child.Schema().Contains(column))
    }
    /// 给定 UniqueID 是否全部落在同一子节点。
    pub fn isAllUniqueIDInTheSameLeaf(&self, ids: &[i64]) -> bool {
        self.Children().iter().any(|child| {
            ids.iter().all(|id| {
                child
                    .Schema()
                    .Columns
                    .iter()
                    .any(|column| column.UniqueID == *id)
            })
        })
    }
    /// 是否为 CAST(列) 形式的连接键包装。
    pub fn isCastWrappedJoinKey(&self, expression: &Expression) -> bool {
        expression.as_scalar_function().is_some_and(|function| {
            function.FuncName.L == "cast"
                && function
                    .GetArgs()
                    .first()
                    .is_some_and(|arg| arg.as_column().is_some())
        })
    }
    /// 是否含 Null-Aware 等值条件。
    pub fn IsNAJoin(&self) -> bool {
        !self.NAEQConditions.is_empty()
    }
    /// IsNAJoin 别名（Null-Aware Anti Join 相关）。
    pub fn IsNAAJ(&self) -> bool {
        self.IsNAJoin()
    }
    /// 常量传播表达式是否不含关联列。
    pub fn isVaildConstantPropagationExpression(&self, expression: &Expression) -> bool {
        expression::ExtractCorColumns(expression.as_ref()).is_empty()
    }
    /// 左外/反半连接：常量传播表达式是否仅引用左子树列。
    pub fn isVaildConstantPropagationExpressionForLeftOuterJoinAndAntiSemiJoin(
        &self,
        expression: &Expression,
    ) -> bool {
        self.Children().first().is_some_and(|child| {
            expression::ExtractColumns(expression.as_ref())
                .iter()
                .all(|column| child.Schema().Contains(column))
        })
    }
    /// 右外连接：常量传播表达式是否仅引用右子树列。
    pub fn isVaildConstantPropagationExpressionForRightOuterJoin(
        &self,
        expression: &Expression,
    ) -> bool {
        self.Children().get(1).is_some_and(|child| {
            expression::ExtractColumns(expression.as_ref())
                .iter()
                .all(|column| child.Schema().Contains(column))
        })
    }
    /// 内连接/半连接的常量传播合法性（委托通用检查）。
    pub fn isVaildConstantPropagationExpressionWithInnerJoinOrSemiJoin(
        &self,
        expression: &Expression,
    ) -> bool {
        self.isVaildConstantPropagationExpression(expression)
    }
    /// 合并全部 ON 侧条件为一份列表。
    pub fn mergeOnClausePredicates(&self) -> Vec<Expression> {
        self.all_conditions().cloned().collect()
    }
    /// 任一子节点 Schema 是否包含该列。
    pub fn planCanResolveUsedCol(&self, column: &Column) -> bool {
        self.Children()
            .iter()
            .any(|child| child.Schema().Contains(column))
    }
    /// 是否设置了任一物理连接偏好。
    pub fn PreferAny(&self) -> bool {
        self.PreferJoinType != 0 || self.LeftPreferJoinType != 0 || self.RightPreferJoinType != 0
    }
    /// TopN 下推入口。
    pub fn PushDownTopN(&mut self, top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        self.pushDownTopNToChild(top_n)
    }
    /// 冗余列与输出列类型是否匹配。
    pub fn redundantColumnRemapTypesMatch(&self, redundant: &Column, output: &Column) -> bool {
        match (&redundant.RetType, &output.RetType) {
            (Some(left), Some(right)) => left.Equal(right),
            _ => false,
        }
    }
    /// 设置单侧（左或右）连接算法偏好。
    pub fn setPreferredJoinTypeFromOneSide(&mut self, left: bool, prefer: u64) {
        if left {
            self.LeftPreferJoinType = prefer;
        } else {
            self.RightPreferJoinType = prefer;
        }
    }
    /// 外连接简化：等价于 ConvertOuterToInnerJoin。
    pub fn SimplifyOuterJoin(&mut self, null_reject_left: bool, null_reject_right: bool) {
        self.ConvertOuterToInnerJoin(null_reject_left, null_reject_right);
    }
    /// 按连接类型继承或清空分组 NDV。
    fn getGroupNDVs(&self, left: &StatsInfo, right: &StatsInfo) -> Vec<property::GroupNDV> {
        match self.JoinType {
            JoinType::LeftOuterJoin
            | JoinType::SemiJoin
            | JoinType::AntiSemiJoin
            | JoinType::LeftOuterSemiJoin
            | JoinType::AntiLeftOuterSemiJoin => left.GroupNDVs.clone(),
            JoinType::RightOuterJoin => right.GroupNDVs.clone(),
            JoinType::InnerJoin => Vec::new(),
        }
    }
}

/// Go's `planCanResolveUsedCol`: a column hidden by a unary wrapper is still
/// resolvable by its child, while a Join may expose it through FullSchema for
/// an upper predicate.  Plain recursive descent is intentionally avoided for
/// operators (such as Projection) that can rewrite or drop columns.
fn plan_can_resolve_used_col(plan: &dyn LogicalPlan, column: &Column) -> bool {
    if plan.Schema().Contains(column) {
        return true;
    }
    if let Some(join) = plan.as_any().downcast_ref::<LogicalJoin>() {
        return join
            .FullSchema
            .as_ref()
            .is_some_and(|schema| schema.Contains(column));
    }
    if let Some(apply) = plan.as_any().downcast_ref::<LogicalApply>() {
        return apply
            .Children()
            .iter()
            .any(|child| plan_can_resolve_used_col(child.as_ref(), column));
    }
    let unary = plan.as_any().is::<LogicalSelection>()
        || plan.as_any().is::<LogicalLimit>()
        || plan.as_any().is::<LogicalTopN>()
        || plan.as_any().is::<LogicalSort>()
        || plan.as_any().is::<LogicalMaxOneRow>();
    unary
        .then(|| plan.Children().first())
        .flatten()
        .is_some_and(|child| plan_can_resolve_used_col(child.as_ref(), column))
}

/// 从 DNF 推导仅引用给定 Schema 的放宽过滤，并展平重组 LogicOr。
fn derive_relaxed_dnf(
    context: &mut dyn expression::exprctx::BuildContext,
    predicate: &dyn expression::Expression,
    schema: &Schema,
) -> Option<Expression> {
    let relaxed = expression::DeriveRelaxedFiltersFromDNF(context, predicate, schema)?;
    let Some(function) = relaxed
        .as_any()
        .downcast_ref::<expression::ScalarFunction>()
        .filter(|function| function.FuncName.L == expression::ast::LogicOr)
    else {
        return Some(relaxed);
    };
    // Go's join simplification presents the relaxed expression as one flat
    // DNF before composing it. Recompose the canonical helper's result from
    // its leaves so nested DNF items retain the same balanced tree shape.
    // Go：放宽表达式先展平 DNF 再重组，保持与规范助手相同的平衡树形态。
    expression::ComposeDNFCondition(context, &expression::FlattenDNFConditions(function))
}

#[derive(Clone, Copy)]
/// 条件相对左右 Schema 的归属侧。
enum ConditionSide {
    Left,
    Right,
    Both,
    Constant,
}

/// 将条件分为跨侧等值、左仅、右仅与其它。
fn extract_on_condition(
    conditions: Vec<Expression>,
    left: &Schema,
    right: &Schema,
) -> (
    Vec<Expression>,
    Vec<Expression>,
    Vec<Expression>,
    Vec<Expression>,
) {
    let mut equal = Vec::new();
    let mut left_only = Vec::new();
    let mut right_only = Vec::new();
    let mut other = Vec::new();
    for condition in conditions {
        let side = LogicalJoin::side_of(left, right, &condition);
        let is_cross_equality = condition.as_scalar_function().is_some_and(|function| {
            matches!(function.FuncName.L.as_str(), "eq" | "nulleq")
                && function.GetArgs().len() == 2
                && function.GetArgs()[0]
                    .as_column()
                    .is_some_and(|column| left.Contains(column) || right.Contains(column))
                && function.GetArgs()[1]
                    .as_column()
                    .is_some_and(|column| left.Contains(column) || right.Contains(column))
        }) && matches!(side, ConditionSide::Both);
        if is_cross_equality {
            equal.push(condition);
        } else {
            match side {
                ConditionSide::Left => left_only.push(condition),
                ConditionSide::Right => right_only.push(condition),
                ConditionSide::Both | ConditionSide::Constant => other.push(condition),
            }
        }
    }
    (equal, left_only, right_only, other)
}

/// 公开的 Schema 合并入口。
pub fn MergeSchema(left: &Schema, right: &Schema) -> Schema {
    merge_schema(left, right)
}
/// 拼接列与键信息生成合并 Schema。
fn merge_schema(left: &Schema, right: &Schema) -> Schema {
    let mut schema = left.Clone();
    schema.Columns.extend(right.Columns.clone());
    schema.PKOrUK.extend(right.PKOrUK.clone());
    schema.NullableUK.extend(right.NullableUK.clone());
    schema
}

/// 递归替换表达式树中的列引用。
fn replace_join_expr(expr: &Expression, replacements: &HashMap<i64, Column>) -> Expression {
    if let Some(column) = expr.as_column() {
        return replacements
            .get(&column.UniqueID)
            .cloned()
            .map(|column| Box::new(column) as Expression)
            .unwrap_or_else(|| expr.clone());
    }
    if let Some(function) = expr.as_scalar_function() {
        let mut result = function.clone_scalar();
        for arg in result.GetArgsMut() {
            *arg = replace_join_expr(arg, replacements);
        }
        result.CleanHashCode();
        return Box::new(result);
    }
    expr.clone()
}

/// LogicalPlan trait 委托；含外连接转内连接的递归入口。
impl LogicalPlan for LogicalJoin {
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
    fn ExplainInfo(&self) -> String {
        Self::ExplainInfo(self)
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
    fn ConvertOuterToInner(&mut self, predicates: Vec<Expression>) {
        if let Some(context) = self.SCtx().cloned()
            && let [left, right] = self.Children()
        {
            let reject_left = predicates.iter().any(|predicate| {
                planner_util::IsNullRejected(context.as_ref(), left.Schema(), predicate.CloneExpr())
            });
            let reject_right = predicates.iter().any(|predicate| {
                planner_util::IsNullRejected(
                    context.as_ref(),
                    right.Schema(),
                    predicate.CloneExpr(),
                )
            });
            LogicalJoin::ConvertOuterToInnerJoin(self, reject_left, reject_right);
        }
        for child in self.Children_mut() {
            child.ConvertOuterToInner(predicates.iter().cloned().collect());
        }
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

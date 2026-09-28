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

// 物理聚合算子公共基座与物理计划枚举。
// 提供 HashAgg/StreamAgg 共享逻辑、两阶段拆分、下推检查及代价因子。

use aggregation::{AggFuncDesc, CompleteMode, FinalMode};
use base::{ContextRef, PhysicalPlan, Plan as _, PlanContext};
use expression::{CorrelatedColumn, ExprBox, Schema};
use property::{MPPPartitionColumn, PhysicalProperty, StatsInfo};

use crate::{
    BasePhysicalPlan, PhysicalHashAgg, PhysicalProjection, PhysicalSchemaProducer,
    PhysicalStreamAgg,
};

fn resolve_cte_column_ids(
    context: &ContextRef,
    expression: ExprBox,
    schema: &Schema,
) -> Result<ExprBox, expression::Error> {
    match expression.ResolveIndices(schema) {
        Ok(resolved) => Ok(resolved),
        Err(original_error) => {
            let columns = expression::ExtractColumns(expression.as_ref())
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            let replacements = columns
                .iter()
                .map(|column| {
                    if column.Index >= 0
                        && let Some(candidate) = schema.Columns.get(column.Index as usize)
                    {
                        return candidate.Clone();
                    }
                    let mut matches = schema
                        .Columns
                        .iter()
                        .filter(|candidate| candidate.String() == column.String());
                    let first = matches.next();
                    if first.is_some() && matches.next().is_none() {
                        first.expect("checked unique CTE column").Clone()
                    } else if column.OrigName.is_empty() {
                        let mut generated_matches = schema.Columns.iter().filter(|candidate| {
                            candidate.OrigName.is_empty()
                                && candidate.RetType.as_ref().is_some_and(|candidate_type| {
                                    column.RetType.as_ref().is_some_and(|column_type| {
                                        candidate_type.Equal(column_type)
                                    })
                                })
                        });
                        let generated = generated_matches.next();
                        if generated.is_some() && generated_matches.next().is_none() {
                            generated.expect("checked unique generated column").Clone()
                        } else {
                            column.Clone()
                        }
                    } else {
                        column.Clone()
                    }
                })
                .collect::<Vec<_>>();
            let remapped = expression::ColumnSubstitute(
                context.GetExprCtx(),
                expression,
                &expression::NewSchema(columns),
                &expression::Column2Exprs(&replacements),
            );
            remapped.ResolveIndices(schema).map_err(|_| {
                expression::errors::New(format!(
                    "resolve aggregate expression against child schema [{}]: {original_error}",
                    schema
                        .Columns
                        .iter()
                        .map(|column| format!("{}#{}", column.String(), column.UniqueID))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// MPP（大规模并行处理）聚合执行模式：单阶段、两阶段、回退 TiDB 或标量聚合。
pub enum AggMppRunMode {
    #[default]
    NoMpp,
    Mpp1Phase,
    Mpp2Phase,
    MppTiDB,
    MppScalar,
}

/// Mirrors Go `tryToGetMppHashAggs` for a scalar aggregation: DISTINCT or
/// ordered aggregates gather on one TiFlash node, while ordinary scalar
/// aggregates keep their final stage in TiDB.
pub(crate) fn scalar_mpp_run_mode(has_distinct_or_order_by: bool) -> AggMppRunMode {
    if has_distinct_or_order_by {
        AggMppRunMode::MppScalar
    } else {
        AggMppRunMode::MppTiDB
    }
}

/// 物理聚合算子公共基座：聚合函数、分组项与 MPP 运行模式。
pub struct BasePhysicalAgg {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    pub AggFuncs: Vec<AggFuncDesc>,
    pub GroupByItems: Vec<ExprBox>,
    pub MppRunMode: AggMppRunMode,
    pub MppPartitionCols: Vec<MPPPartitionColumn>,
}

impl BasePhysicalAgg {
    /// 构造空聚合函数与分组项的基座。
    pub fn New(producer: PhysicalSchemaProducer) -> Self {
        Self {
            PhysicalSchemaProducer: producer,
            AggFuncs: Vec::new(),
            GroupByItems: Vec::new(),
            MppRunMode: AggMppRunMode::NoMpp,
            MppPartitionCols: Vec::new(),
        }
    }

    /// 初始化基础物理计划节点并挂上统计信息。
    pub fn Init(mut self, _ctx: ContextRef, stats: StatsInfo, _offset: i32) -> Self {
        // `NewPhysicalHashAgg` already owns the concrete candidate's base plan.
        // Go's InitForHash mutates that candidate in place and allocates exactly
        // one PlanID; replacing it here allocated a second, unused ID.
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        self
    }

    /// 组装为 HashAgg：设置 Schema 与子节点所需物理属性。
    pub fn InitForHash(
        self,
        ctx: ContextRef,
        stats: StatsInfo,
        offset: i32,
        schema: Schema,
        props: Vec<Box<PhysicalProperty>>,
    ) -> PhysicalHashAgg {
        let mut base = self.Init(ctx, stats, offset);
        // Keep the concrete physical operator name in the plan tree.  The
        // shared initializer uses the protobuf executor name `Aggregation`,
        // but Go EXPLAIN distinguishes HashAgg from StreamAgg.
        base.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetTP("HashAgg");
        base.PhysicalSchemaProducer.SetSchema(schema);
        base.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(props);
        PhysicalHashAgg {
            BasePhysicalAgg: base,
            TiflashPreAggMode: String::new(),
        }
    }

    /// 组装为 StreamAgg：要求子节点提供与分组键匹配的有序属性。
    pub fn InitForStream(
        self,
        ctx: ContextRef,
        stats: StatsInfo,
        offset: i32,
        schema: Schema,
        props: Vec<Box<PhysicalProperty>>,
    ) -> PhysicalStreamAgg {
        let mut base = self.Init(ctx, stats, offset);
        base.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetTP("StreamAgg");
        base.PhysicalSchemaProducer.SetSchema(schema);
        base.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(props);
        PhysicalStreamAgg {
            BasePhysicalAgg: base,
        }
    }

    /// 判断是否为最终阶段聚合（Final/Complete 模式）。
    pub fn IsFinalAgg(&self) -> bool {
        self.AggFuncs
            .first()
            .is_some_and(|function| matches!(function.Mode, FinalMode | CompleteMode))
    }

    /// 计划缓存场景下克隆。
    pub fn CloneForPlanCacheWithSelf(&self, new_ctx: ContextRef) -> Option<Self> {
        self.CloneWithSelf(new_ctx).ok()
    }

    /// 深拷贝聚合函数、分组项与 MPP 分区列并换绑上下文。
    pub fn CloneWithSelf(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut producer = PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(new_ctx)?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
            AggFuncs: self.AggFuncs.iter().map(AggFuncDesc::Clone).collect(),
            GroupByItems: self
                .GroupByItems
                .iter()
                .map(|item| item.CloneExpr())
                .collect(),
            MppRunMode: self.MppRunMode,
            MppPartitionCols: self
                .MppPartitionCols
                .iter()
                .map(MPPPartitionColumn::Clone)
                .collect(),
        })
    }

    /// 统计带 DISTINCT 的聚合函数个数。
    pub fn NumDistinctFunc(&self) -> usize {
        self.AggFuncs.iter().filter(|f| f.HasDistinct).count()
    }

    /// 按聚合函数种类累加代价因子，供代价模型加权。
    pub fn GetAggFuncCostFactor(&self, is_mpp: bool) -> f64 {
        if self.AggFuncs.is_empty() {
            return if is_mpp { 0.1 } else { 1.0 };
        }
        self.AggFuncs
            .iter()
            .map(|f| match f.Name.as_str() {
                parser_ast::AggFuncCount
                | parser_ast::AggFuncSum
                | parser_ast::AggFuncSumInt
                | parser_ast::AggFuncMax
                | parser_ast::AggFuncMin
                | parser_ast::AggFuncGroupConcat => 1.0,
                parser_ast::AggFuncAvg => 2.0,
                parser_ast::AggFuncFirstRow => 0.1,
                parser_ast::AggFuncBitOr
                | parser_ast::AggFuncBitXor
                | parser_ast::AggFuncBitAnd => 0.9,
                parser_ast::AggFuncVarPop
                | parser_ast::AggFuncVarSamp
                | parser_ast::AggFuncStddevPop
                | parser_ast::AggFuncStddevSamp => 3.0,
                _ => 1.5,
            })
            .sum()
    }

    /// 从分组项与聚合参数中抽取相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.GroupByItems
            .iter()
            .chain(self.AggFuncs.iter().flat_map(|f| f.Args.iter()))
            .flat_map(|expr| {
                expression::ExtractCorColumns(expr.as_ref())
                    .into_iter()
                    .map(CorrelatedColumn::Clone)
            })
            .collect()
    }

    /// 估算聚合描述、分组表达式与分区列的内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .AggFuncs
                .iter()
                .map(AggFuncDesc::MemoryUsage)
                .sum::<i64>()
            + self
                .GroupByItems
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + self.MppPartitionCols.len() as i64 * std::mem::size_of::<MPPPartitionColumn>() as i64
    }

    /// Convert AVG to COUNT + SUM and return the projection that reconstructs
    /// AVG.  TiFlash and TiDB use different aggregate-state layouts, so Go
    /// also keeps an identity projection for a final MPP aggregate without
    /// AVG.
    pub fn ConvertAvgForMPP(&mut self) -> Result<Option<PhysicalProjection>, expression::Error> {
        let mut original_schema = self.PhysicalSchemaProducer.Schema().Clone();
        for column in original_schema.Columns.iter_mut().take(self.AggFuncs.len()) {
            if column.OrigName.is_empty() || column.OrigName == "Column" {
                column.OrigName = format!("Column#{}", column.UniqueID);
            }
        }
        let mut new_schema = expression::NewSchema(Vec::new());
        new_schema.PKOrUK = original_schema.PKOrUK.clone();
        new_schema.NullableUK = original_schema.NullableUK.clone();
        let mut new_functions = Vec::with_capacity(self.AggFuncs.len() * 2);
        let mut expressions = Vec::with_capacity(original_schema.Len() * 2);
        let context = self.PhysicalSchemaProducer.BasePhysicalPlan.s_ctx().clone();
        let expression_context = context.GetExprCtx();

        for (index, function) in self.AggFuncs.iter().enumerate() {
            if function.Name == parser_ast::AggFuncAvg {
                let mut count = function.Clone();
                count.Name = parser_ast::AggFuncCount.to_owned();
                count.TypeInfer(expression_context)?;
                let count_type = count
                    .RetTp
                    .clone()
                    .ok_or_else(|| expression::errors::New("COUNT return type is required"))?;
                let count_column = expression::Column::new(
                    count_type.clone(),
                    0,
                    expression_context.AllocPlanColumnID(),
                    new_schema.Len() as isize,
                );
                new_functions.push(count);
                new_schema.Append([count_column.Clone()]);

                let mut sum = function.Clone();
                sum.Name = parser_ast::AggFuncSum.to_owned();
                let avg_type = function
                    .RetTp
                    .as_ref()
                    .ok_or_else(|| expression::errors::New("AVG return type is required"))?;
                sum.TypeInfer4AvgSum(expression_context.GetEvalCtx(), avg_type)?;
                let sum_type = sum
                    .RetTp
                    .clone()
                    .ok_or_else(|| expression::errors::New("SUM return type is required"))?;
                let mut sum_column = expression::Column::new(
                    sum_type.clone(),
                    0,
                    original_schema.Columns[index].UniqueID,
                    new_schema.Len() as isize,
                );
                sum_column.OrigName = original_schema.Columns[index].OrigName.clone();
                new_functions.push(sum);
                new_schema.Append([sum_column.Clone()]);

                let equality = expression::NewFunctionInternal(
                    expression_context,
                    parser_ast::EQ,
                    *expression::types::NewFieldType(expression::mysql::TypeTiny),
                    vec![
                        Box::new(count_column.Clone()),
                        Box::new(expression::NewZero()),
                    ],
                )
                .ok_or_else(|| expression::errors::New("build AVG zero check"))?;
                let case_when = expression::NewFunctionInternal(
                    expression_context,
                    parser_ast::Case,
                    count_type,
                    vec![
                        equality,
                        Box::new(expression::NewOne()),
                        Box::new(count_column),
                    ],
                )
                .ok_or_else(|| expression::errors::New("build AVG divisor"))?;
                let mut divisor_type =
                    *expression::types::NewFieldType(expression::mysql::TypeNewDecimal);
                divisor_type.SetFlen(20);
                divisor_type.SetDecimal(0);
                divisor_type.AddFlag(expression::mysql::BinaryFlag);
                divisor_type.SetCharset("binary".to_owned());
                divisor_type.SetCollate("binary".to_owned());
                let case_when = expression::formal_registry::BuildCastFunction(
                    expression_context,
                    &case_when,
                    &divisor_type,
                );
                let mut divide = expression::NewFunctionInternal(
                    expression_context,
                    expression::ast::Div,
                    sum_type,
                    vec![Box::new(sum_column), case_when],
                )
                .ok_or_else(|| expression::errors::New("build AVG division"))?;
                if let Some(function) = divide
                    .as_any_mut()
                    .downcast_mut::<expression::ScalarFunction>()
                {
                    function.RetType = original_schema.Columns[index].RetType.clone();
                }
                expressions.push(divide);
            } else {
                new_functions.push(function.Clone());
                new_schema.Append([original_schema.Columns[index].Clone()]);
                expressions.push(Box::new(original_schema.Columns[index].Clone()) as ExprBox);
            }
        }
        if original_schema.Len() == new_schema.Len() && !self.IsFinalAgg() {
            return Ok(None);
        }
        expressions.extend(
            original_schema.Columns[self.AggFuncs.len()..]
                .iter()
                .cloned()
                .map(|column| Box::new(column) as ExprBox),
        );

        self.AggFuncs = new_functions;
        self.PhysicalSchemaProducer.SetSchema(new_schema);
        let child_property = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetChildReqProps(0)
            .CloneEssentialFields();
        let mut projection = PhysicalProjection::New(context.clone()).Init(
            context,
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .stats_info()
                .clone(),
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .query_block_offset(),
            vec![Box::new(child_property)],
        );
        projection.Exprs = expressions;
        projection.PhysicalSchemaProducer.SetSchema(original_schema);
        Ok(Some(projection))
    }

    /// 拆出部分聚合与最终聚合两段描述（两阶段聚合）。
    pub fn NewPartialAggregate(
        &self,
        is_mpp: bool,
    ) -> Result<(AggInfo, AggInfo), expression::Error> {
        BuildFinalModeAggregation(self, is_mpp)
    }

    /// 判断无分组且单一 DISTINCT 聚合是否可走三阶段扩展。
    pub fn Scale3StageForDistinctAgg(&self) -> bool {
        if !self.GroupByItems.is_empty() || self.NumDistinctFunc() != 1 {
            return false;
        }
        self.AggFuncs
            .iter()
            .all(|f| f.OrderByItems.is_empty() && f.Mode == CompleteMode)
    }

    /// 生成 EXPLAIN 用的聚合描述字符串。
    pub fn ExplainInfo(&self) -> String {
        self.explain(false)
    }
    /// 生成规范化（忽略别名等）的 EXPLAIN 描述。
    pub fn ExplainNormalizedInfo(&self) -> String {
        self.explain(true)
    }

    /// 拼接 group by 与 funcs 的可解释文本。
    fn explain(&self, normalized: bool) -> String {
        let eval = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetExprCtx()
            .GetEvalCtx();
        let mut result = String::new();
        if !self.GroupByItems.is_empty() {
            result.push_str("group by:");
            if normalized {
                result.push_str(&String::from_utf8_lossy(
                    &expression::SortedExplainNormalizedExpressionList(&self.GroupByItems),
                ));
            } else {
                result.push_str(&String::from_utf8_lossy(
                    &expression::SortedExplainExpressionList(eval, &self.GroupByItems),
                ));
            }
            result.push_str(", ");
        }
        for (index, function) in self.AggFuncs.iter().enumerate() {
            result.push_str("funcs:");
            result.push_str(&aggregation::ExplainAggFunc(eval, function, normalized));
            result.push_str("->");
            let output = &self
                .PhysicalSchemaProducer
                .SchemaRef()
                .expect("physical aggregation output schema")
                .Columns[index];
            if normalized {
                result.push_str(&output.ExplainNormalizedInfo());
            } else {
                result.push_str(&output.ExplainInfo(eval));
            }
            if index + 1 < self.AggFuncs.len() {
                result.push_str(", ");
            }
        }
        let stream_count = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount;
        if stream_count > 0 {
            result.push_str(&format!(", stream_count: {stream_count}"));
        }
        result
    }

    /// 将聚合参数与分组项下标解析到子节点 Schema。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        let Some(child) = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .copied()
        else {
            return Ok(());
        };
        let schema = child.schema().Clone();
        let context = self.PhysicalSchemaProducer.BasePhysicalPlan.s_ctx().clone();
        let mut projected_expressions = child
            .as_any()
            .downcast_ref::<PhysicalProjection>()
            .map(|projection| {
                projection
                    .Exprs
                    .iter()
                    .zip(&projection.schema().Columns)
                    .map(|(expression, column)| (expression.CloneExpr(), column.Clone()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if let Some(hash) = child.as_any().downcast_ref::<PhysicalHashAgg>() {
            projected_expressions.extend(
                hash.BasePhysicalAgg
                    .GroupByItems
                    .iter()
                    .zip(
                        hash.schema()
                            .Columns
                            .iter()
                            .skip(hash.BasePhysicalAgg.AggFuncs.len()),
                    )
                    .map(|(expression, column)| (expression.CloneExpr(), column.Clone())),
            );
        } else if let Some(stream) = child.as_any().downcast_ref::<PhysicalStreamAgg>() {
            projected_expressions.extend(
                stream
                    .BasePhysicalAgg
                    .GroupByItems
                    .iter()
                    .zip(
                        stream
                            .schema()
                            .Columns
                            .iter()
                            .skip(stream.BasePhysicalAgg.AggFuncs.len()),
                    )
                    .map(|(expression, column)| (expression.CloneExpr(), column.Clone())),
            );
        }
        let remap_projection_output = |value: &ExprBox| {
            let normalized = value.StringWithCtx(None, expression::errors::RedactLogDisable);
            let mut matching = projected_expressions.iter().filter(|(candidate, _)| {
                candidate.Equal(context.GetExprCtx().GetEvalCtx(), value.as_ref())
                    || candidate.StringWithCtx(None, expression::errors::RedactLogDisable)
                        == normalized
            });
            let first = matching.next();
            if let Some((_, column)) = first
                && matching.next().is_none()
            {
                Box::new(column.Clone()) as ExprBox
            } else if value.as_any().is::<expression::ScalarFunction>() {
                let value_type = value.GetType(context.GetExprCtx().GetEvalCtx());
                let mut generated = schema.Columns.iter().filter(|column| {
                    (column.OrigName.is_empty()
                        || column.OrigName == format!("Column#{}", column.UniqueID))
                        && column
                            .RetType
                            .as_ref()
                            .is_some_and(|field| field.Equal(value_type))
                });
                let first = generated.next();
                if let Some(column) = first
                    && generated.next().is_none()
                {
                    Box::new(column.Clone()) as ExprBox
                } else {
                    value.CloneExpr()
                }
            } else {
                value.CloneExpr()
            }
        };
        let child_group_columns = child
            .as_any()
            .downcast_ref::<PhysicalHashAgg>()
            .map(|hash| {
                hash.schema()
                    .Columns
                    .iter()
                    .skip(hash.BasePhysicalAgg.AggFuncs.len())
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .or_else(|| {
                child
                    .as_any()
                    .downcast_ref::<PhysicalStreamAgg>()
                    .map(|stream| {
                        stream
                            .schema()
                            .Columns
                            .iter()
                            .skip(stream.BasePhysicalAgg.AggFuncs.len())
                            .cloned()
                            .collect::<Vec<_>>()
                    })
            })
            .filter(|columns| columns.len() == self.GroupByItems.len());
        for function in &mut self.AggFuncs {
            for argument in &mut function.Args {
                *argument =
                    resolve_cte_column_ids(&context, remap_projection_output(argument), &schema)?;
            }
            for item in &mut function.OrderByItems {
                item.Expr = remap_projection_output(&item.Expr).ResolveIndices(&schema)?;
            }
        }
        for (index, item) in self.GroupByItems.iter_mut().enumerate() {
            let remapped = child_group_columns
                .as_ref()
                .and_then(|columns| columns.get(index))
                .map(|column| Box::new(column.Clone()) as ExprBox)
                .unwrap_or_else(|| remap_projection_output(item));
            *item = resolve_cte_column_ids(&context, remapped, &schema)?;
        }
        Ok(())
    }
}

/// 一段聚合的函数列表、分组项与输出 Schema。
pub struct AggInfo {
    pub AggFuncs: Vec<AggFuncDesc>,
    pub GroupByItems: Vec<ExprBox>,
    pub Schema: Schema,
}

/// 将完整聚合拆成 partial/final：AVG 产生两列，分组列追加到 partial Schema。
pub fn BuildFinalModeAggregation(
    agg: &BasePhysicalAgg,
    is_mpp: bool,
) -> Result<(AggInfo, AggInfo), expression::Error> {
    let mut final_schema = agg
        .PhysicalSchemaProducer
        .SchemaRef()
        .map(Schema::Clone)
        .unwrap_or_else(|| expression::NewSchema(Vec::new()));
    for column in final_schema.Columns.iter_mut().take(agg.AggFuncs.len()) {
        if column.OrigName.is_empty() {
            column.OrigName = format!("Column#{}", column.UniqueID);
        }
    }
    let context = agg.PhysicalSchemaProducer.BasePhysicalPlan.s_ctx().clone();
    let mut partial_schema_columns = Vec::new();
    let mut partial_funcs = Vec::with_capacity(agg.AggFuncs.len());
    let mut final_funcs = Vec::with_capacity(agg.AggFuncs.len());
    let mut partial_group_by = agg
        .GroupByItems
        .iter()
        .map(|item| item.CloneExpr())
        .collect::<Vec<_>>();
    // Go constructs the partial group schema before splitting aggregate
    // functions.  Preserve that allocation order: computed grouping keys
    // must receive their IDs before partial aggregate result columns.
    let mut partial_group_columns = Vec::with_capacity(partial_group_by.len());
    for (index, item) in partial_group_by.iter().enumerate() {
        let mut column = item
            .as_any()
            .downcast_ref::<expression::Column>()
            .map(expression::Column::Clone)
            .unwrap_or_else(|| {
                expression::Column::new(
                    item.GetType(context.GetExprCtx().GetEvalCtx()).clone(),
                    0,
                    context.GetExprCtx().AllocPlanColumnID(),
                    index as isize,
                )
            });
        column.Index = index as isize;
        partial_group_columns.push(column);
    }
    let mut distinct_arg_indices = Vec::new();
    let mut first_row_group_indices = Vec::new();

    // 逐个聚合函数拆成 partial/final。DISTINCT 参数作为 partial
    // group key 去重，final 保留 DISTINCT 语义，与 Go 的两阶段拆分一致。
    for (offset, function) in agg.AggFuncs.iter().enumerate() {
        if function.HasDistinct {
            let mut final_desc = function.Clone();
            final_desc.Args.clear();
            for argument in &function.Args {
                let index = partial_group_by
                    .iter()
                    .position(|group| {
                        group.Equal(context.GetExprCtx().GetEvalCtx(), argument.as_ref())
                            && group
                                .GetType(context.GetExprCtx().GetEvalCtx())
                                .Equal(argument.GetType(context.GetExprCtx().GetEvalCtx()))
                    })
                    .unwrap_or_else(|| {
                        partial_group_by.push(argument.CloneExpr());
                        partial_group_by.len() - 1
                    });
                distinct_arg_indices.push((final_funcs.len(), index));
            }
            final_funcs.push(final_desc);
            continue;
        }
        // Group keys already carry the values needed by firstrow in the
        // final MPP aggregate. Go removes the redundant partial firstrow and
        // rewrites the final argument to the corresponding partial group
        // output column.
        if !agg.GroupByItems.is_empty()
            && function.Name == parser_ast::AggFuncFirstRow
            && let Some(group_index) = function.Args.first().and_then(|argument| {
                agg.GroupByItems.iter().position(|group| {
                    argument.Equal(context.GetExprCtx().GetEvalCtx(), group.as_ref())
                        || argument.StringWithCtx(None, expression::errors::RedactLogDisable)
                            == group.StringWithCtx(None, expression::errors::RedactLogDisable)
                })
            })
        {
            let mut final_desc = function.Clone();
            final_desc.Mode = FinalMode;
            final_desc.Args.clear();
            first_row_group_indices.push((final_funcs.len(), group_index));
            final_funcs.push(final_desc);
            continue;
        }
        let output_count = if function.Name == parser_ast::AggFuncAvg {
            2
        } else {
            1
        };
        let start = partial_schema_columns.len();
        let ordinals = (start..start + output_count)
            .map(|index| index as isize)
            .collect::<Vec<_>>();
        let (partial, mut final_desc) = function.Split(&ordinals);
        for output in 0..output_count {
            let field_type = final_desc
                .Args
                .get(output)
                .and_then(|argument| argument.as_any().downcast_ref::<expression::Column>())
                .and_then(|column| column.RetType.clone())
                .or_else(|| {
                    final_schema
                        .Columns
                        .get(offset)
                        .and_then(|column| column.RetType.clone())
                })
                .ok_or_else(|| expression::errors::New("aggregate output type is required"))?;
            let mut partial_column = expression::Column::new(
                field_type,
                0,
                context.GetExprCtx().AllocPlanColumnID(),
                (start + output) as isize,
            );
            partial_column.OrigName = format!("Column#{}", partial_column.UniqueID);
            partial_schema_columns.push(partial_column);
        }
        for argument in &mut final_desc.Args {
            if let Some(column) = argument.as_any().downcast_ref::<expression::Column>()
                && let Some(partial_column) = partial_schema_columns.get(column.Index as usize)
            {
                *argument = Box::new(partial_column.Clone());
            }
        }
        if is_mpp && function.Name == parser_ast::AggFuncCount && !function.HasDistinct {
            final_desc.Name = parser_ast::AggFuncSum.to_owned();
        }
        if function.Name == parser_ast::AggFuncAvg {
            let mut count = function.Clone();
            count.Name = parser_ast::AggFuncCount.to_owned();
            count.Mode = partial.Mode;
            count.RetTp = Some(*expression::types::NewFieldType(
                expression::mysql::TypeLonglong,
            ));
            let mut sum = function.Clone();
            sum.Name = parser_ast::AggFuncSum.to_owned();
            sum.Mode = partial.Mode;
            partial_funcs.push(count);
            partial_funcs.push(sum);
        } else {
            partial_funcs.push(partial);
        }
        if !is_mpp && function.Name == parser_ast::AggFuncCount && !function.HasDistinct {
            context.GetExprCtx().AllocPlanColumnID();
        }
        final_funcs.push(final_desc);
    }

    // 分组列写入 partial Schema，final 阶段以列引用回指。
    for item in partial_group_by.iter().skip(partial_group_columns.len()) {
        let column = item
            .as_any()
            .downcast_ref::<expression::Column>()
            .map(expression::Column::Clone)
            .unwrap_or_else(|| {
                expression::Column::new(
                    item.GetType(context.GetExprCtx().GetEvalCtx()).clone(),
                    0,
                    context.GetExprCtx().AllocPlanColumnID(),
                    0,
                )
            });
        partial_group_columns.push(column);
    }
    for column in &mut partial_group_columns {
        column.Index = partial_schema_columns.len() as isize;
        partial_schema_columns.push(column.Clone());
    }
    for (function_index, group_index) in distinct_arg_indices {
        if let (Some(function), Some(column)) = (
            final_funcs.get_mut(function_index),
            partial_group_columns.get(group_index),
        ) {
            function.Args.push(Box::new(column.Clone()));
        }
    }
    for (function_index, group_index) in first_row_group_indices {
        if let (Some(function), Some(column)) = (
            final_funcs.get_mut(function_index),
            partial_group_columns.get(group_index),
        ) {
            function.Args.push(Box::new(column.Clone()));
        }
    }
    let final_group_by = partial_group_columns
        .iter()
        .take(agg.GroupByItems.len())
        .map(|column| Box::new(column.Clone()) as ExprBox)
        .collect();

    let partial = AggInfo {
        AggFuncs: partial_funcs,
        GroupByItems: partial_group_by,
        Schema: expression::NewSchema(partial_schema_columns),
    };
    let final_info = AggInfo {
        AggFuncs: final_funcs,
        GroupByItems: final_group_by,
        Schema: final_schema,
    };
    Ok((partial, final_info))
}

/// 检查聚合是否可下推到 Coprocessor/存储引擎：无虚拟列、无相关列且函数支持。
pub fn CheckAggCanPushCop(
    ctx: &dyn PlanContext,
    functions: &[AggFuncDesc],
    groups: &[ExprBox],
    store: kv::StoreType,
) -> bool {
    functions.iter().all(|function| {
        let order_by = function
            .OrderByItems
            .iter()
            .map(|item| item.Expr.CloneExpr())
            .collect::<Vec<_>>();
        !expression::ContainVirtualColumn(&function.Args)
            && !expression::ContainCorrelatedColumn(&function.Args)
            && !expression::ContainVirtualColumn(&order_by)
            && !expression::ContainCorrelatedColumn(&order_by)
            && aggregation::CheckAggPushDown(ctx.GetExprCtx().GetEvalCtx(), function, store)
    }) && !expression::ContainVirtualColumn(groups)
        && !expression::ContainCorrelatedColumn(groups)
}

/// 删除参数已经由非常量 GROUP BY 键输出的冗余 first_row。
///
/// 与 Go 一致，不能仅因存在 GROUP BY 就删除 first_row：参数不匹配、以及
/// `SELECT DISTINCT constant` 形成的常量分组键都仍需要该聚合输出。
pub fn RemoveUnnecessaryFirstRow(
    functions: Vec<AggFuncDesc>,
    group_by: &[ExprBox],
) -> Vec<AggFuncDesc> {
    functions
        .into_iter()
        .filter(|function| {
            if function.Name != parser_ast::AggFuncFirstRow {
                return true;
            }
            let Some(argument) = function.Args.first() else {
                return true;
            };
            !group_by.iter().any(|group| {
                !group.as_any().is::<expression::Constant>()
                    && match (
                        group.as_any().downcast_ref::<expression::Column>(),
                        argument.as_any().downcast_ref::<expression::Column>(),
                    ) {
                        (Some(group), Some(argument)) => group.UniqueID == argument.UniqueID,
                        _ => {
                            group.StringWithCtx(None, expression::errors::RedactLogDisable)
                                == argument
                                    .StringWithCtx(None, expression::errors::RedactLogDisable)
                        }
                    }
            })
        })
        .collect()
}

/// 递归检测计划树是否引用启用了表缓存的数据源。
fn contains_cached_table(plan: &dyn logicalop::LogicalPlan) -> bool {
    let data_source_cached = plan
        .as_any()
        .downcast_ref::<logicalop::DataSource>()
        .is_some_and(|source| {
            source.TableInfo.TableCacheStatusType == expression::model::TableCacheStatusEnable
        });
    let table_scan_cached = plan
        .as_any()
        .downcast_ref::<logicalop::LogicalTableScan>()
        .and_then(|scan| scan.Source.as_ref())
        .is_some_and(|source| {
            source.borrow().TableInfo.TableCacheStatusType
                == expression::model::TableCacheStatusEnable
        });
    data_source_cached
        || table_scan_cached
        || plan
            .Children()
            .iter()
            .any(|child| contains_cached_table(child.as_ref()))
}

/// 为逻辑聚合枚举物理候选：HashAgg 与（有序匹配时）StreamAgg。
pub fn ExhaustPhysicalPlans4LogicalAggregation(
    logical: &logicalop::LogicalAggregation,
    property: &PhysicalProperty,
) -> Vec<Box<dyn base::PhysicalPlan>> {
    use logicalop::LogicalPlan as _;

    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    fn prefers_root_index_join(plan: &dyn logicalop::LogicalPlan) -> bool {
        if let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() {
            let left_rows = join
                .Children()
                .first()
                .and_then(|child| child.StatsInfo())
                .map(|stats| stats.RowCount);
            let right_rows = join
                .Children()
                .get(1)
                .and_then(|child| child.StatsInfo())
                .map(|stats| stats.RowCount);
            return join.JoinType == base::JoinType::InnerJoin
                && left_rows.zip(right_rows).is_some_and(|(left, right)| {
                    let smaller = left.min(right);
                    let larger = left.max(right);
                    smaller <= 2_000_000.0
                        && larger / smaller.max(1.0) >= 1_000.0
                        && join
                            .Children()
                            .iter()
                            .any(|child| child.as_any().is::<logicalop::LogicalJoin>())
                });
        }
        plan.Children()
            .iter()
            .any(|child| prefers_root_index_join(child.as_ref()))
    }
    fn has_small_root_join(plan: &dyn logicalop::LogicalPlan) -> bool {
        if let Some(join) = plan.as_any().downcast_ref::<logicalop::LogicalJoin>() {
            let row_counts = join
                .Children()
                .first()
                .and_then(|child| child.StatsInfo())
                .map(|stats| stats.RowCount)
                .zip(
                    join.Children()
                        .get(1)
                        .and_then(|child| child.StatsInfo())
                        .map(|stats| stats.RowCount),
                );
            if join.JoinType == base::JoinType::InnerJoin
                && row_counts.is_some_and(|(left, right)| {
                    left.min(right) <= 512.0 && left.max(right) > 128.0
                })
            {
                return true;
            }
        }
        plan.Children()
            .iter()
            .any(|child| has_small_root_join(child.as_ref()))
    }
    fn subtree_has_tiflash(plan: &dyn logicalop::LogicalPlan) -> bool {
        logicalop::GetHasTiFlash(Some(plan))
            || plan
                .Children()
                .iter()
                .any(|child| subtree_has_tiflash(child.as_ref()))
    }
    fn subtree_is_for_update(plan: &dyn logicalop::LogicalPlan) -> bool {
        plan.as_any().is::<logicalop::LogicalLock>()
            || plan
                .as_any()
                .downcast_ref::<logicalop::DataSource>()
                .is_some_and(|source| source.IsForUpdateRead)
            || plan
                .Children()
                .iter()
                .any(|child| subtree_is_for_update(child.as_ref()))
    }
    fn subtree_has_decorrelated_apply_join(plan: &dyn logicalop::LogicalPlan) -> bool {
        plan.as_any()
            .downcast_ref::<logicalop::LogicalJoin>()
            .is_some_and(|join| {
                matches!(
                    join.JoinType,
                    base::JoinType::SemiJoin
                        | base::JoinType::AntiSemiJoin
                        | base::JoinType::LeftOuterSemiJoin
                        | base::JoinType::AntiLeftOuterSemiJoin
                )
            })
            || plan
                .as_any()
                .downcast_ref::<logicalop::LogicalApply>()
                .is_some_and(|apply| {
                    matches!(
                        apply.LogicalJoin.JoinType,
                        base::JoinType::SemiJoin
                            | base::JoinType::AntiSemiJoin
                            | base::JoinType::LeftOuterSemiJoin
                            | base::JoinType::AntiLeftOuterSemiJoin
                    )
                })
            || plan
                .Children()
                .iter()
                .any(|child| subtree_has_decorrelated_apply_join(child.as_ref()))
    }
    fn expression_has_runtime_scalar(expression: &dyn expression::Expression) -> bool {
        if let Some(constant) = expression.as_any().downcast_ref::<expression::Constant>() {
            return constant.SubqueryRefID > 0;
        }
        if let Some(column) = expression.as_any().downcast_ref::<expression::Column>() {
            return column.UniqueID == 0 || column.String().starts_with("ScalarQueryCol#");
        }
        if expression.as_any().is::<expression::CorrelatedColumn>() {
            return false;
        }
        if let Some(function) = expression
            .as_any()
            .downcast_ref::<expression::ScalarFunction>()
        {
            return function
                .GetArgs()
                .iter()
                .any(|argument| expression_has_runtime_scalar(argument.as_ref()));
        }
        true
    }
    fn subtree_has_runtime_scalar(plan: &dyn logicalop::LogicalPlan) -> bool {
        plan.as_any()
            .downcast_ref::<logicalop::LogicalSelection>()
            .is_some_and(|selection| {
                selection
                    .Conditions
                    .iter()
                    .any(|condition| expression_has_runtime_scalar(condition.as_ref()))
                    || selection.ExplainInfo().contains("ScalarQueryCol#")
            })
            || plan
                .Children()
                .iter()
                .any(|child| subtree_has_runtime_scalar(child.as_ref()))
    }
    let tiflash_root_index = logical.Children().first().is_some_and(|child| {
        prefers_root_index_join(child.as_ref()) && logicalop::GetHasTiFlash(Some(child.as_ref()))
    });
    let root_index = logical.Children().first().is_some_and(|child| {
        (prefers_root_index_join(child.as_ref()) && !tiflash_root_index)
            || (has_small_root_join(child.as_ref())
                && (property.NoCopPushDown
                    || subtree_is_for_update(child.as_ref())
                    || subtree_has_decorrelated_apply_join(child.as_ref())))
    });
    if property.TaskTp == property::MppTaskType && root_index {
        return Vec::new();
    }
    let decorrelated_apply_child = logical.Children().first().is_some_and(|child| {
        subtree_has_decorrelated_apply_join(child.as_ref())
            || subtree_has_runtime_scalar(child.as_ref())
    });
    if property.TaskTp == property::MppTaskType && decorrelated_apply_child {
        return Vec::new();
    }
    let stats = logical.StatsInfo().cloned().unwrap_or_default();
    let schema = logical.Schema().Clone();
    let offset = logical.QueryBlockOffset();
    let mut plans: Vec<Box<dyn base::PhysicalPlan>> = Vec::new();
    // 标量聚合挂在 Limit 下、TiFlash approx_distinct 或缓存表时抑制 HashAgg。
    let scalar_limit_child = logical.GroupByItems.is_empty()
        && logical.Children().first().is_some_and(|child| {
            child.as_any().is::<logicalop::LogicalLimit>()
                || child.as_any().is::<logicalop::LogicalTopN>()
        });
    let tiflash_scalar_approx_distinct = logical.GroupByItems.is_empty()
        && !logical.AggFuncs.is_empty()
        && logical
            .AggFuncs
            .iter()
            .all(|function| function.Name == parser_ast::AggFuncApproxCountDistinct)
        && ctx
            .GetSessionVars()
            .GetIsolationReadEngines()
            .contains(&kv::StoreType::TiFlash);
    let cached_table_child = logical
        .Children()
        .first()
        .is_some_and(|child| contains_cached_table(child.as_ref()));
    if (!scalar_limit_child || property.TaskTp == property::MppTaskType)
        && !tiflash_scalar_approx_distinct
        && !cached_table_child
        && property.SortItems.is_empty()
    {
        // A required MPP property is already proof that the parent selected
        // a TiFlash subtree. Derived-table rewrites do not always preserve
        // BaseLogicalPlan.hasTiFlash in Rust, so do not reject the aggregate
        // before its child gets a chance to validate the storage path.
        let can_push_down_to_mpp = ctx.GetSessionVars().IsMPPAllowed()
            && !subtree_is_for_update(logical)
            && (property.TaskTp == property::MppTaskType
                || planner_util::ShouldCheckTiFlashPushDown(
                    ctx.as_ref(),
                    logicalop::GetHasTiFlash(Some(logical)),
                )
                || logical
                    .Children()
                    .first()
                    .is_some_and(|child| subtree_has_tiflash(child.as_ref())))
            && logical
                .AggFuncs
                .iter()
                .all(|function| function.Name != parser_ast::AggFuncApproxCountDistinct);
        if property.TaskTp == property::MppTaskType && !can_push_down_to_mpp {
            return Vec::new();
        }
        let task_types = if property.TaskTp == property::MppTaskType {
            vec![property::MppTaskType]
        } else if property.NoCopPushDown || root_index || decorrelated_apply_child {
            vec![property::RootTaskType]
        } else if tiflash_root_index {
            vec![
                property::CopSingleReadTaskType,
                property::CopMultiReadTaskType,
                property::RootTaskType,
                property::MppTaskType,
            ]
        } else {
            let mut task_types = vec![
                property::CopSingleReadTaskType,
                property::CopMultiReadTaskType,
                property::RootTaskType,
            ];
            // Go enumerates an MPP aggregate even when the parent requests a
            // Root task.  The selected MPP fragment is converted to a
            // TableReader at the attachment boundary.  Omitting this
            // alternative forced TPC-H grouped aggregates onto TiKV/root.
            if can_push_down_to_mpp
                && !decorrelated_apply_child
                && !logical
                    .Children()
                    .first()
                    .is_some_and(|child| prefers_root_index_join(child.as_ref()))
            {
                task_types.push(property::MppTaskType);
            }
            crate::AdmitIndexJoinTypes(task_types, property)
        };
        for task_type in task_types {
            if task_type == property::MppTaskType {
                let mut partition_columns = logical
                    .GetPotentialPartitionKeys()
                    .into_iter()
                    .map(|column| {
                        let collate = column
                            .RetType
                            .as_ref()
                            .map_or("binary", |field| field.GetCollate());
                        let collate_id = property::GetCollateIDByNameForPartition(collate);
                        property::MPPPartitionColumn {
                            Col: column,
                            CollateID: collate_id,
                        }
                    })
                    .collect::<Vec<_>>();
                if partition_columns.is_empty() {
                    partition_columns = logical
                        .GroupByItems
                        .iter()
                        .filter_map(|item| item.as_any().downcast_ref::<expression::Column>())
                        .map(|column| {
                            let collate = column
                                .RetType
                                .as_ref()
                                .map_or("binary", |field| field.GetCollate());
                            property::MPPPartitionColumn {
                                Col: column.Clone(),
                                CollateID: property::GetCollateIDByNameForPartition(collate),
                            }
                        })
                        .collect();
                }
                if property.MPPPartitionTp == property::HashType {
                    if let Some(matches) = property.IsSubsetOf(&partition_columns) {
                        partition_columns =
                            property::ChoosePartitionKeys(&partition_columns, &matches);
                    } else {
                        let remapped = property
                            .MPPPartitionCols
                            .iter()
                            .filter_map(|required| {
                                partition_columns.iter().find(|candidate| {
                                    candidate.Col.String() == required.Col.String()
                                        && candidate.Col.RetType.as_ref().is_some_and(|left| {
                                            required
                                                .Col
                                                .RetType
                                                .as_ref()
                                                .is_some_and(|right| left.Equal(right))
                                        })
                                })
                            })
                            .map(property::MPPPartitionColumn::Clone)
                            .collect::<Vec<_>>();
                        if remapped.len() != property.MPPPartitionCols.len() {
                            if !property.CanAddEnforcer {
                                continue;
                            }
                        } else {
                            partition_columns = remapped;
                        }
                    }
                } else if !matches!(
                    property.MPPPartitionTp,
                    property::AnyType | property::SinglePartitionType
                ) {
                    continue;
                }

                // A shared CTE producer with several grouping dimensions must
                // pre-aggregate before repartitioning. Treating the root
                // request as a one-phase candidate sends the full join result
                // across the network and diverges from Go's CTE plan choice;
                // single-key nested producers can still remain one phase.
                if !tiflash_root_index
                    && !partition_columns.is_empty()
                    && property.MPPPartitionTp != property::SinglePartitionType
                    && !(property.CTEProducerStatus == property::AllCTECanMpp
                        && logical.GroupByItems.len() > 1)
                {
                    let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                        ctx.clone(),
                        "HashAgg",
                        offset,
                    ));
                    if let Ok(mut one_phase) = crate::NewPhysicalHashAgg(logical, producer) {
                        one_phase.BasePhysicalAgg.MppRunMode = AggMppRunMode::Mpp1Phase;
                        let mut child = PhysicalProperty::default();
                        child.TaskTp = property::MppTaskType;
                        child.ExpectedCnt = f64::MAX;
                        child.MPPPartitionTp = property::HashType;
                        child.MPPPartitionCols = partition_columns
                            .iter()
                            .map(property::MPPPartitionColumn::Clone)
                            .collect();
                        child.CanAddEnforcer = true;
                        child.CTEProducerStatus = property.CTEProducerStatus;
                        child.NoCopPushDown = property.NoCopPushDown;
                        plans.push(Box::new(one_phase.BasePhysicalAgg.InitForHash(
                            ctx.clone(),
                            stats.clone(),
                            offset,
                            schema.Clone(),
                            vec![Box::new(child)],
                        )));
                    }
                }

                if !logical
                    .AggFuncs
                    .first()
                    .is_some_and(|function| function.Mode == aggregation::FinalMode)
                {
                    let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                        ctx.clone(),
                        "HashAgg",
                        offset,
                    ));
                    if let Ok(mut two_phase) = crate::NewPhysicalHashAgg(logical, producer) {
                        two_phase.BasePhysicalAgg.MppRunMode = if logical.GroupByItems.is_empty() {
                            scalar_mpp_run_mode(logical.HasDistinct() || logical.HasOrderBy())
                        } else {
                            AggMppRunMode::Mpp2Phase
                        };
                        // Go's MppTiDB attachment always converts the final
                        // aggregate to a RootTask. It cannot satisfy an MPP
                        // property requested by an enclosing operator.
                        if property.TaskTp == property::MppTaskType
                            && two_phase.BasePhysicalAgg.MppRunMode == AggMppRunMode::MppTiDB
                        {
                            continue;
                        }
                        two_phase.BasePhysicalAgg.MppPartitionCols = partition_columns
                            .iter()
                            .map(property::MPPPartitionColumn::Clone)
                            .collect();
                        let mut child = PhysicalProperty::default();
                        child.TaskTp = property::MppTaskType;
                        child.ExpectedCnt = f64::MAX;
                        child.MPPPartitionTp = property::AnyType;
                        child.CTEProducerStatus = property.CTEProducerStatus;
                        child.NoCopPushDown = property.NoCopPushDown;
                        plans.push(Box::new(two_phase.BasePhysicalAgg.InitForHash(
                            ctx.clone(),
                            stats.clone(),
                            offset,
                            schema.Clone(),
                            vec![Box::new(child)],
                        )));
                    }

                    // Go also keeps a Root-only alternative whose partial
                    // aggregate runs in TiFlash and whose final aggregate
                    // remains in TiDB.  This boundary is distinct from the
                    // all-MPP two-phase candidate above.
                    if property.TaskTp == property::RootTaskType
                        && !tiflash_root_index
                        && !decorrelated_apply_child
                        && !logical.GroupByItems.is_empty()
                    {
                        let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                            ctx.clone(),
                            "HashAgg",
                            offset,
                        ));
                        if let Ok(mut tidb_final) = crate::NewPhysicalHashAgg(logical, producer) {
                            tidb_final.BasePhysicalAgg.MppRunMode = AggMppRunMode::MppTiDB;
                            let mut child = PhysicalProperty::default();
                            child.TaskTp = property::MppTaskType;
                            child.ExpectedCnt = f64::MAX;
                            child.MPPPartitionTp = property::AnyType;
                            child.CTEProducerStatus = property.CTEProducerStatus;
                            child.NoCopPushDown = property.NoCopPushDown;
                            plans.push(Box::new(tidb_final.BasePhysicalAgg.InitForHash(
                                ctx.clone(),
                                stats.clone(),
                                offset,
                                schema.Clone(),
                                vec![Box::new(child)],
                            )));
                        }
                    }
                }
                continue;
            }
            let producer =
                PhysicalSchemaProducer::New(BasePhysicalPlan::New(ctx.clone(), "HashAgg", offset));
            let Ok(hash) = crate::NewPhysicalHashAgg(logical, producer) else {
                continue;
            };
            let mut child = PhysicalProperty::default();
            child.TaskTp = task_type;
            child.ExpectedCnt = f64::MAX;
            child.CTEProducerStatus = property.CTEProducerStatus;
            child.NoCopPushDown = property.NoCopPushDown || root_index;
            child.PreferTiFlash = logical.GroupByItems.is_empty()
                && logical
                    .AggFuncs
                    .iter()
                    .any(|function| function.Name == parser_ast::AggFuncAvg);
            plans.push(Box::new(hash.BasePhysicalAgg.InitForHash(
                ctx.clone(),
                stats.clone(),
                offset,
                schema.Clone(),
                vec![Box::new(child)],
            )));
        }
    }

    let hash_count = plans.len();
    let group_columns = logical
        .GroupByItems
        .iter()
        .map(|item| item.as_any().downcast_ref::<expression::Column>().cloned())
        .collect::<Option<Vec<_>>>();
    // StreamAgg：要求排序方向一致且排序列被分组列覆盖。
    if property.TaskTp != property::MppTaskType
        && !root_index
        && !logical.AggFuncs.iter().any(|function| {
            function.Name == parser_ast::AggFuncGroupConcat && !function.OrderByItems.is_empty()
        })
        && let Some(group_columns) = group_columns.filter(|_columns| {
            !logical
                .AggFuncs
                .iter()
                .all(|function| function.Name == parser_ast::AggFuncFirstRow)
                || logical.PreferAggToCop
                || cached_table_child
                || scalar_limit_child
                || tiflash_scalar_approx_distinct
        })
    {
        let same_direction = property
            .SortItems
            .windows(2)
            .all(|items| items[0].Desc == items[1].Desc);
        let required_matches_group = property.SortItems.iter().all(|item| {
            group_columns
                .iter()
                .any(|column| column.UniqueID == item.Col.UniqueID)
        });
        if same_direction && required_matches_group {
            let desc = property.SortItems.first().is_some_and(|item| item.Desc);
            let task_types = if scalar_limit_child || property.NoCopPushDown || root_index {
                vec![property::RootTaskType]
            } else {
                crate::AdmitIndexJoinTypes(
                    vec![property::CopSingleReadTaskType, property::RootTaskType],
                    property,
                )
            };
            // Go getStreamAggs only enumerates CopSingleRead and Root child
            // properties. TiFlash does not execute StreamAgg, including when
            // a scalar aggregate has no grouping keys.
            for task_type in task_types {
                let child =
                    property::NewPhysicalProperty(task_type, &group_columns, desc, f64::MAX, false);
                let producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                    ctx.clone(),
                    "StreamAgg",
                    offset,
                ));
                if let Ok(stream) = crate::NewPhysicalHashAgg(logical, producer) {
                    plans.push(Box::new(stream.BasePhysicalAgg.InitForStream(
                        ctx.clone(),
                        stats.clone(),
                        offset,
                        schema.Clone(),
                        vec![child],
                    )));
                }
            }
        }
    }
    // Go ExhaustPhysicalPlans4LogicalAggregation returns only the hinted
    // family when it has a viable candidate. Keep both when hints conflict.
    let prefer_hash = logical.PreferAggType & (1_u64 << 25) != 0;
    let prefer_stream = logical.PreferAggType & (1_u64 << 26) != 0;
    if prefer_hash && !prefer_stream && hash_count > 0 {
        plans.truncate(hash_count);
    } else if prefer_stream && !prefer_hash && plans.len() > hash_count {
        plans.drain(..hash_count);
    }
    plans
}

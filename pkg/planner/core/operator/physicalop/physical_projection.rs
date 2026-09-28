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

// Projection（投影）物理算子：按表达式列表计算输出列，对应 SQL SELECT 列表。
//
// 在满足推送策略时可下推到 TiKV/TiFlash；Attach2Task 时若子任务是可推送的
// TableReader，会把投影嵌入读侧以减少回传列。

use base::{ContextRef, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::{CorrelatedColumn, ExprBox};

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// 将表达式 FieldType 映射为 infer_pushdown 策略使用的简化类型。
fn policy_field_type(
    field: &expression::types::FieldType,
) -> expression::infer_pushdown::FieldType {
    use expression::infer_pushdown::{FieldKind, FieldType};
    let kind = match field.GetType() {
        expression::mysql::TypeBit => FieldKind::Bit,
        expression::mysql::TypeSet => FieldKind::Set,
        expression::mysql::TypeEnum => FieldKind::Enum,
        expression::mysql::TypeGeometry => FieldKind::Geometry,
        expression::mysql::TypeJSON => FieldKind::Json,
        expression::mysql::TypeNewDecimal => FieldKind::Decimal,
        expression::mysql::TypeDuration => FieldKind::Duration,
        expression::mysql::TypeYear => FieldKind::Year,
        expression::mysql::TypeFloat | expression::mysql::TypeDouble => FieldKind::Real,
        expression::mysql::TypeVarchar
        | expression::mysql::TypeVarString
        | expression::mysql::TypeString => FieldKind::String,
        expression::mysql::TypeTiDBVectorFloat32 => FieldKind::Vector,
        expression::mysql::TypeDate
        | expression::mysql::TypeDatetime
        | expression::mysql::TypeTimestamp => FieldKind::Time,
        expression::mysql::TypeUnspecified => FieldKind::Unspecified,
        _ => FieldKind::Int,
    };
    let mut result = FieldType::new(kind);
    result.flen = field.GetFlen() as i32;
    result.decimal = field.GetDecimal() as i32;
    result.unsigned = expression::mysql::HasUnsignedFlag(field.GetFlag());
    result.hybrid = field.Hybrid();
    result.charset = field.GetCharset().to_owned();
    result.collation = field.GetCollate().to_owned();
    result
}

/// 将运行时表达式转为 pushdown 策略 AST，用于判断能否下推到存储。
fn policy_expression(
    expression: &dyn expression::Expression,
    eval: &dyn expression::EvalContext,
) -> expression::infer_pushdown::Expression {
    use expression::infer_pushdown as policy;
    // 常量统一视为 Null Datum，仅关心类型兼容性。
    if expression.as_constant().is_some() {
        return policy::Expression::Constant(policy::Datum::Null);
    }
    if let Some(column) = expression.as_column() {
        let mut policy_column = policy::Column::new(
            column.UniqueID,
            column.Index.max(0) as usize,
            policy_field_type(expression.GetType(eval)),
        );
        policy_column.encodable = column.VirtualExpr.is_none();
        return policy::Expression::Column(policy_column);
    }
    if let Some(function) = expression.as_scalar_function() {
        let mut return_type = policy_field_type(expression.GetType(eval));
        // regexp 系列返回值字符集/排序规则跟随第一个参数。
        if matches!(
            function.FuncName.L.as_str(),
            "regexp" | "regexp_like" | "regexp_instr" | "regexp_substr" | "regexp_replace"
        ) && let Some(argument) = function.Function.getArgs().first()
        {
            let argument_type = argument.GetType(eval);
            return_type.charset = argument_type.GetCharset().to_owned();
            return_type.collation = argument_type.GetCollate().to_owned();
        }
        return policy::Expression::ScalarFunction(policy::ScalarFunction::new(
            function.FuncName.L.clone(),
            policy::Signature::Generic(function.FuncName.L.clone()),
            function
                .Function
                .getArgs()
                .iter()
                .map(|argument| policy_expression(argument.as_ref(), eval))
                .collect(),
            return_type,
        ));
    }
    policy::Expression::Unsupported(expression.ExplainInfo(eval))
}

/// 判断投影表达式是否可下推到 TiFlash。
pub(crate) fn CanProjectionPushToTiFlash(projection: &PhysicalProjection) -> bool {
    can_projection_push_to_store(projection, kv::StoreType::TiFlash)
}

/// 通用下推判定：含虚拟列则拒绝，否则交给 infer_pushdown 策略。
pub(crate) fn can_projection_push_to_store(
    projection: &PhysicalProjection,
    store: kv::StoreType,
) -> bool {
    // 虚拟生成列依赖计算，不能直接下推到存储引擎。
    if expression::ContainVirtualColumn(&projection.Exprs) {
        return false;
    }
    let eval = projection
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .s_ctx()
        .GetExprCtx()
        .GetEvalCtx();
    // TiFlash cannot evaluate the implicit JSON casts inserted for AVG, SUM,
    // and GROUP_CONCAT. Go consequently keeps that projection in TiDB and
    // reads the raw JSON column through a cop TableReader.
    if store == kv::StoreType::TiFlash
        && projection.Exprs.iter().any(|value| {
            value.as_scalar_function().is_some_and(|function| {
                function.FuncName.L == "cast"
                    && function.GetArgs().first().is_some_and(|argument| {
                        argument.GetType(eval).GetType() == expression::mysql::TypeJSON
                    })
            })
        })
    {
        return false;
    }
    let expressions = projection
        .Exprs
        .iter()
        .map(|expression| policy_expression(expression.as_ref(), eval))
        .collect();
    expression::infer_pushdown::can_exprs_push_down(
        &expression::infer_pushdown::PushDownContext::new(false, None, None, 0),
        expressions,
        match store {
            kv::StoreType::TiKV => expression::infer_pushdown::StoreType::TiKV,
            kv::StoreType::TiFlash => expression::infer_pushdown::StoreType::TiFlash,
            kv::StoreType::TiDB => expression::infer_pushdown::StoreType::TiDB,
            kv::StoreType::UnSpecified => expression::infer_pushdown::StoreType::Unspecified,
        },
    )
}

/// 对应 Go `PhysicalProjection`：输出表达式列表及执行期行为开关。
pub struct PhysicalProjection {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 投影表达式（SELECT 列表）。
    pub Exprs: Vec<ExprBox>,
    /// 为 true 时不延迟计算（立即求值）。
    pub CalculateNoDelay: bool,
    /// 避免使用列求值器的特殊路径开关。
    pub AvoidColumnEvaluator: bool,
}

impl PhysicalProjection {
    /// 构造 TypeProj 空壳。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeProj,
                0,
            )),
            Exprs: Vec::new(),
            CalculateNoDelay: false,
            AvoidColumnEvaluator: false,
        }
    }

    /// 绑定统计、查询块偏移与子属性需求。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        props: Vec<Box<property::PhysicalProperty>>,
    ) -> Self {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetSCtx(ctx);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetTP(plancodec::TypeProj);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .Plan
            .SetQueryBlockOffset(offset);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(props);
        self
    }

    /// 深克隆表达式列表到新上下文。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
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
            Exprs: self.Exprs.iter().map(|expr| expr.CloneExpr()).collect(),
            CalculateNoDelay: self.CalculateNoDelay,
            AvoidColumnEvaluator: self.AvoidColumnEvaluator,
        })
    }

    /// 从各投影表达式中收集关联列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.Exprs
            .iter()
            .flat_map(|expr| expression::ExtractCorColumns(expr.as_ref()))
            .map(CorrelatedColumn::Clone)
            .collect()
    }

    /// EXPLAIN：表达式列表，可选附带 TiFlash 细粒度 shuffle 流数。
    pub fn ExplainInfo(&self) -> String {
        let eval = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetExprCtx()
            .GetEvalCtx();
        let redact = eval.GetTiDBRedactLog();
        let statement_context = &self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetSessionVars()
            .StmtCtx;
        // StmtCtx is the stable source here: BuildPBCtx may already have been
        // restored by the time the physical tree is rendered.
        let remove_column_numbers = statement_context
            .ExplainFormat
            .trim()
            .eq_ignore_ascii_case(expression::types::ExplainFormatPlanTree);
        let mut result = expression::ExplainExpressionListWithColumnNumbers(
            eval,
            &self.Exprs,
            self.PhysicalSchemaProducer
                .SchemaRef()
                .unwrap_or_else(|| self.PhysicalSchemaProducer.BasePhysicalPlan.schema()),
            &redact,
            remove_column_numbers,
        );
        let stream_count = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount;
        if stream_count > 0 {
            result.push_str(&format!(", stream_count: {stream_count}"));
        }
        result
    }

    /// 归一化并排序后的表达式 EXPLAIN。
    pub fn ExplainNormalizedInfo(&self) -> String {
        let explained = if vardef::IgnoreInlistPlanDigest.Load() {
            expression::SortedExplainExpressionListIgnoreInlist(&self.Exprs)
        } else {
            expression::SortedExplainNormalizedExpressionList(&self.Exprs)
        };
        String::from_utf8_lossy(&explained).into_owned()
    }

    /// 按子节点 Schema 解析各投影表达式列下标。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        let Some(schema) = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .map(|child| child.schema().Clone())
        else {
            return Ok(());
        };
        for expression in &mut self.Exprs {
            *expression = expression.ResolveIndices(&schema).map_err(|error| {
                expression::errors::New(format!(
                    "resolve projection expression against child schema [{}]: {error}",
                    schema
                        .Columns
                        .iter()
                        .map(|column| format!("{}#{}", column.String(), column.UniqueID))
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        }
        Ok(())
    }

    /// 挂接任务：若可下推到 TableReader 则把投影塞进读侧，否则作为 Root 投影。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        if self.get_child_req_props(0).TaskTp != property::RootTaskType
            && can_projection_push_to_store(self, kv::StoreType::TiKV)
            && let Some(task) = tasks.first()
            && expression::ProjectionBenefitsFromPushedDown(&self.Exprs, task.plan().schema().Len())
        {
            let context = self.s_ctx().clone();
            if let Some(reader) = task
                .plan()
                .as_any()
                .downcast_ref::<crate::PhysicalIndexReader>()
                && let Some(index_plan) = reader.IndexPlan.as_deref()
            {
                let mut projection = self.Clone(context.clone()).expect("clone cop projection");
                projection.set_children(vec![
                    index_plan
                        .clone_physical(context.clone())
                        .expect("clone index plan"),
                ]);
                let mut reader = reader.Clone(context).expect("clone index reader");
                reader.SetChildren(vec![Box::new(projection)]);
                reader
                    .PhysicalSchemaProducer
                    .SetSchema(self.schema().Clone());
                return Box::new(crate::RootTask::New(Box::new(reader), None));
            }
            if let Some(reader) = task
                .plan()
                .as_any()
                .downcast_ref::<crate::PhysicalIndexLookUpReader>()
                && let Some(table_plan) = reader.TablePlan.as_deref()
            {
                let mut projection = self.Clone(context.clone()).expect("clone cop projection");
                projection.set_children(vec![
                    table_plan
                        .clone_physical(context.clone())
                        .expect("clone lookup table plan"),
                ]);
                let mut reader = reader.Clone(context).expect("clone lookup reader");
                reader.TablePlan = Some(Box::new(projection));
                reader
                    .PhysicalSchemaProducer
                    .SetSchema(self.schema().Clone());
                return Box::new(crate::RootTask::New(Box::new(reader), None));
            }
        }
        // 尝试把投影推入 TableReader，减少从存储拉回的列。
        if self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetChildReqProps(0)
            .TaskTp
            != property::RootTaskType
            && let Some(task) = tasks.first()
            && let Some(reader) = task
                .plan()
                .as_any()
                .downcast_ref::<crate::PhysicalTableReader>()
            && can_projection_push_to_store(self, reader.StoreType)
            && (reader.StoreType != kv::StoreType::TiKV
                || expression::ProjectionBenefitsFromPushedDown(
                    &self.Exprs,
                    reader
                        .TablePlan
                        .as_deref()
                        .map_or(0, |plan| plan.schema().Len()),
                ))
            && let Some(table_plan) = reader.GetTablePlan()
        {
            let context = self.PhysicalSchemaProducer.BasePhysicalPlan.s_ctx().clone();
            let mut projection = self
                .Clone(context.clone())
                .expect("physical projection clone for table reader pushdown");
            projection.set_children(vec![
                table_plan
                    .clone_physical(context.clone())
                    .expect("table plan clone for projection pushdown"),
            ]);
            let mut reader = reader
                .Clone(context)
                .expect("table reader clone for projection pushdown");
            reader.SetChildren(vec![Box::new(projection)]);
            reader
                .PhysicalSchemaProducer
                .SetSchema(self.schema().Clone());
            reader.set_output_names(self.output_names());
            return Box::new(crate::RootTask::New(Box::new(reader), None));
        }
        let children = tasks
            .iter()
            .map(|task| task.plan().clone_physical(task.plan().s_ctx().clone()))
            .collect::<Result<Vec<_>, _>>()
            .expect("physical projection child task plan clone");
        let mut projection = self
            .Clone(self.s_ctx().clone())
            .expect("physical projection clone");
        if let Some(child) = children.first() {
            projection
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(child.stats_info().clone());
        }
        projection.set_children(children);
        Box::new(crate::RootTask::New(Box::new(projection), None))
    }

    /// 代价模型 v1。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

    /// 代价模型 v2。
    pub fn GetPlanCostVer2(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
        inl: &[bool],
    ) -> Result<CostVer2, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer2(task, option, inl)
    }

    /// 序列化为 tipb::Projection；仅允许推到 TiKV 或 TiFlash。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        if !matches!(store, kv::StoreType::TiKV | kv::StoreType::TiFlash) {
            return Err(expression::errors::New(
                "projection can only be pushed down to TiKV or TiFlash",
            ));
        }
        let client = ctx
            .GetClient()
            .ok_or_else(|| expression::errors::New("PB client is required"))?;
        let mut projection = tipb::Projection::new();
        projection.set_exprs(
            expression::ProjectionExpressionsToPBList(
                ctx.GetExprCtx().GetEvalCtx(),
                &self.Exprs,
                client.as_ref(),
            )?
            .into(),
        );
        let child = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .ok_or_else(|| expression::errors::New("projection requires one child"))?
            .to_pb(ctx, store)?;
        projection.set_child(*child);
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeProjection);
        executor.set_projection(projection);
        executor.set_executor_id(self.explain_id(&[]).to_string());
        Ok(Box::new(executor))
    }

    /// 估算表达式与两个布尔开关的内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .Exprs
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + std::mem::size_of::<bool>() as i64 * 2
    }
}

/// 从 LogicalProjection 枚举物理候选，必要时附加可下推的 MPP 变体。
pub fn ExhaustPhysicalPlans4LogicalProjection(
    logical: &logicalop::LogicalProjection,
    property: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    use logicalop::LogicalPlan as _;
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    let (Some(child), true) = logical.TryToGetChildProp(property) else {
        return Vec::new();
    };
    let mut child = child;
    child.NoCopPushDown = property.NoCopPushDown;
    if property.NoCopPushDown {
        child.TaskTp = property::RootTaskType;
    }
    // The Rust router materializes IndexJoin child properties one level
    // earlier than Go; retain Go's permission to enforce that child order.
    if !child.SortItems.is_empty() {
        child.CanAddEnforcer = true;
    }
    let Some(child) = crate::AdmitIndexJoinProp(Box::new(child), property) else {
        return Vec::new();
    };
    let transformed_child = child.CloneEssentialFields();
    let stats = logical
        .StatsInfo()
        .map(|stats| stats.ScaleByExpectCnt(ctx.GetSessionVars(), property.ExpectedCnt))
        .unwrap_or_default();
    let policy_checkpoint = ctx.plan_id_checkpoint();
    let policy_probe = PhysicalProjection {
        PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
            ctx.clone(),
            plancodec::TypeProj,
            logical.QueryBlockOffset(),
        )),
        Exprs: logical.Exprs.iter().map(|expr| expr.CloneExpr()).collect(),
        CalculateNoDelay: logical.CalculateNoDelay,
        AvoidColumnEvaluator: false,
    };
    let projection_can_push_to_tiflash = CanProjectionPushToTiFlash(&policy_probe);
    let projection_can_push_to_tikv =
        can_projection_push_to_store(&policy_probe, kv::StoreType::TiKV);
    if let Some(checkpoint) = policy_checkpoint {
        ctx.restore_plan_id_checkpoint(checkpoint);
    }
    let child_is_table_dual = logical
        .Children()
        .first()
        .is_some_and(|child| child.as_any().is::<logicalop::LogicalTableDual>());
    fn contains_cte_reference(plan: &dyn logicalop::LogicalPlan) -> bool {
        plan.as_any().is::<logicalop::LogicalCTE>()
            || plan.as_any().is::<logicalop::LogicalCTETable>()
            || plan
                .Children()
                .iter()
                .any(|child| contains_cte_reference(child.as_ref()))
    }
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
                    (smaller <= 512.0 && larger > 128.0)
                        || (smaller <= 2_000_000.0
                            && larger / smaller.max(1.0) >= 1_000.0
                            && join
                                .Children()
                                .iter()
                                .any(|child| child.as_any().is::<logicalop::LogicalJoin>()))
                });
        }
        plan.Children()
            .iter()
            .any(|child| prefers_root_index_join(child.as_ref()))
    }
    fn contains_semi_join(plan: &dyn logicalop::LogicalPlan) -> bool {
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
                .any(|child| contains_semi_join(child.as_ref()))
    }
    let child_contains_cte = logical
        .Children()
        .first()
        .is_some_and(|child| contains_cte_reference(child.as_ref()));
    let child_prefers_index = logical
        .Children()
        .first()
        .is_some_and(|child| prefers_root_index_join(child.as_ref()));
    let child_contains_semi_join = logical
        .Children()
        .first()
        .is_some_and(|child| contains_semi_join(child.as_ref()));
    let child_has_json_aggregation = logical.Children().first().is_some_and(|child| {
        child
            .as_any()
            .downcast_ref::<logicalop::LogicalAggregation>()
            .is_some_and(|aggregation| {
                let eval = ctx.GetExprCtx().GetEvalCtx();
                aggregation.AggFuncs.iter().any(|function| {
                    function.Args.iter().any(|argument| {
                        argument.GetType(eval).GetType() == expression::mysql::TypeJSON
                    })
                })
            })
    });
    let child_is_scalar_aggregation = logical.Children().first().is_some_and(|child| {
        child
            .as_any()
            .downcast_ref::<logicalop::LogicalAggregation>()
            .is_some_and(|aggregation| aggregation.GroupByItems.is_empty())
    });
    if property.TaskTp == property::MppTaskType && child_contains_semi_join {
        return Vec::new();
    }
    let can_use_mpp = ctx.GetSessionVars().IsMPPAllowed()
        && planner_util::ShouldCheckTiFlashPushDown(
            ctx.as_ref(),
            logicalop::GetHasTiFlash(Some(logical)),
        )
        && !child_is_table_dual
        && !child_has_json_aggregation
        && projection_can_push_to_tiflash;
    // 强制 MPP 且投影可下推时，非 MPP 属性不产生候选。
    let mut children = if ctx.GetSessionVars().IsMPPEnforced()
        && can_use_mpp
        && property.TaskTp != property::MppTaskType
        && !property.NoCopPushDown
        && !child_contains_cte
        && !child_is_scalar_aggregation
    {
        Vec::new()
    } else {
        vec![child]
    };
    // 额外枚举一条 MPP 子属性，供 TiFlash 路径选用。
    // Keep the final SELECT-list projection above a Root TopN. The TopN itself
    // still enumerates an MPP child and pushes a partial TopN into TiFlash,
    // while Go retains the final merge TopN and its output projection in
    // TiDB. Enumerating a second MPP projection here makes the simplified Rust
    // task-cost bridge choose an all-MPP alternative and lose that boundary.
    let child_is_root_ordering_boundary = logical.Children().first().is_some_and(|child| {
        child.as_any().is::<logicalop::LogicalTopN>()
            || child.as_any().is::<logicalop::LogicalSort>()
    });
    if property.TaskTp != property::MppTaskType
        && !property.NoCopPushDown
        && can_use_mpp
        && !child_prefers_index
        && !child_contains_semi_join
        && !child_is_root_ordering_boundary
    {
        // Go clones `newProp` here, after projection expressions have mapped
        // output properties back to child columns. Cloning the parent property
        // leaks projected column IDs below the projection and produces an
        // ExchangeSender whose hash keys cannot resolve against the child.
        let mut mpp = transformed_child.CloneEssentialFields();
        mpp.TaskTp = property::MppTaskType;
        children.push(Box::new(mpp));
    }
    if transformed_child.TaskTp != property::CopSingleReadTaskType
        && !property.NoCopPushDown
        && ctx
            .GetSessionVars()
            .GetSystemVar(vardef::TiDBOptProjectionPushDown)
            .map_or(vardef::DefOptEnableProjectionPushDown, |value| {
                matches!(value.to_ascii_lowercase().as_str(), "1" | "on" | "true")
            })
        && projection_can_push_to_tikv
        && logical.Children().first().is_some_and(|child| {
            expression::ProjectionBenefitsFromPushedDown(&logical.Exprs, child.Schema().Len())
        })
    {
        let mut cop = transformed_child;
        cop.TaskTp = property::CopSingleReadTaskType;
        children.push(Box::new(cop));
    }
    children
        .into_iter()
        .map(|child| {
            let mut projection = PhysicalProjection::New(ctx.clone());
            projection.Exprs = logical.Exprs.iter().map(|expr| expr.CloneExpr()).collect();
            projection.CalculateNoDelay = logical.CalculateNoDelay;
            projection
                .PhysicalSchemaProducer
                .SetSchema(logical.Schema().Clone());
            Box::new(projection.Init(
                ctx.clone(),
                stats.clone(),
                logical.QueryBlockOffset(),
                vec![child],
            )) as Box<dyn PhysicalPlan>
        })
        .collect()
}

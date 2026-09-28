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

// 物理选择（Selection / Filter）算子：按谓词过滤孩子输出行。
//
// 对应 SQL 中的 WHERE/HAVING 过滤；条件以表达式列表保存，可下推到 TiKV/TiFlash 执行。

use base::{ContextRef, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::{CorrelatedColumn, ExprBox};

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

fn resolve_cte_column_ids(
    context: &ContextRef,
    expression: ExprBox,
    schema: &expression::Schema,
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
            remapped.ResolveIndices(schema).map_err(|_| original_error)
        }
    }
}

/// 物理 Selection：内嵌 Schema 生产者，保存过滤条件及是否来自数据源下推标记。
pub struct PhysicalSelection {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 过滤谓词表达式列表；全部为真的行才会保留。
    pub Conditions: Vec<ExprBox>,
    /// 标记该 Selection 是否由数据源（如存储引擎）侧产生/下推。
    pub FromDataSource: bool,
}

impl PhysicalSelection {
    /// 用会话上下文构造空条件的 Selection，计划类型为 TypeSel。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeSel,
                0,
            )),
            Conditions: Vec::new(),
            FromDataSource: false,
        }
    }

    /// 绑定统计信息、查询块偏移与孩子所需物理属性后返回自身。
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
            .SetTP(plancodec::TypeSel);
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

    /// 克隆基础计划、条件及数据源标记；成本比较会克隆候选计划。
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
            Conditions: self
                .Conditions
                .iter()
                .map(|expr| expr.CloneExpr())
                .collect(),
            FromDataSource: self.FromDataSource,
        })
    }

    /// 从所有过滤条件中抽取相关列（CorrelatedColumn：引用外层查询的列）。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.Conditions
            .iter()
            .flat_map(|expr| expression::ExtractCorColumns(expr.as_ref()))
            .map(CorrelatedColumn::Clone)
            .collect()
    }

    /// 生成 EXPLAIN 文本：排序后的表达式列表，必要时附带 TiFlash 细粒度 Shuffle 流数。
    pub fn ExplainInfo(&self) -> String {
        let eval = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetExprCtx()
            .GetEvalCtx();
        let mut result = String::from_utf8_lossy(&expression::SortedExplainExpressionList(
            eval,
            &self.Conditions,
        ))
        .into_owned();
        let stream_count = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount;
        if stream_count > 0 {
            result.push_str(&format!(", stream_count: {stream_count}"));
        }
        result
    }

    /// 生成归一化 EXPLAIN 信息（用于计划缓存键等稳定比较场景）。
    pub fn ExplainNormalizedInfo(&self) -> String {
        let explained = if vardef::IgnoreInlistPlanDigest.Load() {
            expression::SortedExplainExpressionListIgnoreInlist(&self.Conditions)
        } else {
            expression::SortedExplainNormalizedExpressionList(&self.Conditions)
        };
        String::from_utf8_lossy(&explained).into_owned()
    }

    /// 先解析基础计划，再将各条件表达式的列下标对齐到孩子 Schema。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        let Some(child) = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .map(|child| child.schema().Clone())
        else {
            return Ok(());
        };
        let context = self.s_ctx().clone();
        for condition in &mut self.Conditions {
            *condition = resolve_cte_column_ids(&context, condition.CloneExpr(), &child).map_err(
                |error| {
                    expression::errors::New(format!(
                        "resolve selection condition against child schema [{}]: {error}",
                        child
                            .Columns
                            .iter()
                            .map(|column| format!("{}#{}", column.String(), column.UniqueID))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                },
            )?;
        }
        Ok(())
    }

    /// 将本算子挂接到执行任务树（Task：调度单元，对应 Root/Cop/MPP 等）。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
    }

    /// 计算 v1 代价模型下的计划成本。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

    /// 计算 v2 代价模型下的计划成本。
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

    /// 序列化为 tipb Selection Executor；TiFlash 时还需嵌入孩子子树。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let mut selection = tipb::Selection::new();
        if !self.Conditions.is_empty() {
            let client = ctx
                .GetClient()
                .ok_or_else(|| expression::errors::New("PB client is required"))?;
            selection.set_conditions(
                expression::ExpressionsToPBList(
                    ctx.GetExprCtx().GetEvalCtx(),
                    &self.Conditions,
                    client.as_ref(),
                )?
                .into(),
            );
        }
        let mut executor_id = String::new();
        // TiFlash 下推需要把孩子执行器一并编码进 Selection。
        if store == kv::StoreType::TiFlash {
            let child = self
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .Children()
                .first()
                .ok_or_else(|| expression::errors::New("selection requires one child"))?
                .to_pb(ctx, store)?;
            selection.set_child(*child);
            executor_id = self.explain_id(&[]).to_string();
        }
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeSelection);
        executor.set_selection(selection);
        executor.set_executor_id(executor_id);
        Ok(Box::new(executor))
    }

    /// 估算内存：Schema 生产者 + 各条件表达式 + FromDataSource 布尔字段。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .Conditions
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + std::mem::size_of::<bool>() as i64
    }
}

/// 由逻辑 Selection 穷举生成物理 Selection 候选（含可选 MPP 孩子属性）。
pub fn ExhaustPhysicalPlans4LogicalSelection(
    logical: &logicalop::LogicalSelection,
    property: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    use logicalop::LogicalPlan as _;
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    fn contains_runtime_scalar_subquery(expression: &dyn expression::Expression) -> bool {
        if let Some(constant) = expression.as_any().downcast_ref::<expression::Constant>() {
            return constant.SubqueryRefID > 0;
        }
        if let Some(column) = expression.as_any().downcast_ref::<expression::Column>() {
            // An uncorrelated scalar subquery is replaced with a synthetic
            // runtime column before physical enumeration.  It is populated by
            // TiDB and therefore is not available inside an MPP fragment.
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
                .any(|argument| contains_runtime_scalar_subquery(argument.as_ref()));
        }
        // Planner-owned expressions such as ScalarSubQueryExpr cannot be
        // serialized by the storage expression crate. Treat every unknown
        // expression implementation as a root-only runtime expression.
        true
    }
    let has_runtime_scalar_subquery = logical
        .Conditions
        .iter()
        .any(|condition| contains_runtime_scalar_subquery(condition.as_ref()))
        || String::from_utf8_lossy(&expression::SortedExplainExpressionList(
            ctx.GetExprCtx().GetEvalCtx(),
            &logical.Conditions,
        ))
        .contains("ScalarQueryCol#");
    if property.TaskTp == property::MppTaskType
        && (has_runtime_scalar_subquery
            || !planner_util::ShouldCheckTiFlashPushDown(
                ctx.as_ref(),
                logicalop::GetHasTiFlash(Some(logical)),
            ))
    {
        return Vec::new();
    }
    let mut child_property = property.CloneEssentialFields();
    child_property.CanAddEnforcer = property.CanAddEnforcer;
    let mut children = vec![Box::new(child_property)];
    // 非 MPP 请求且允许 MPP、可推 TiFlash、条件无虚拟列时，额外尝试 MPP 孩子属性。
    if property.TaskTp != property::MppTaskType
        && ctx.GetSessionVars().IsMPPAllowed()
        && planner_util::ShouldCheckTiFlashPushDown(
            ctx.as_ref(),
            logicalop::GetHasTiFlash(Some(logical)),
        )
        && !expression::ContainVirtualColumn(&logical.Conditions)
        && !has_runtime_scalar_subquery
    {
        let mut mpp = property.CloneEssentialFields();
        mpp.TaskTp = property::MppTaskType;
        children.push(Box::new(mpp));
    }
    let children = crate::AdmitIndexJoinProps(children, property);
    let stats = logical
        .StatsInfo()
        .map(|stats| stats.ScaleByExpectCnt(ctx.GetSessionVars(), property.ExpectedCnt))
        .unwrap_or_default();
    children
        .into_iter()
        .map(|child| {
            let mut selection = PhysicalSelection::New(ctx.clone());
            selection.Conditions = logical
                .Conditions
                .iter()
                .map(|expr| expr.CloneExpr())
                .collect();
            selection
                .PhysicalSchemaProducer
                .SetSchema(logical.Schema().Clone());
            Box::new(selection.Init(
                ctx.clone(),
                stats.clone(),
                logical.QueryBlockOffset(),
                vec![child],
            )) as Box<dyn PhysicalPlan>
        })
        .collect()
}

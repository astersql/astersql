// Copyright 2026 AsterSQL.
/*
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


/// PhysicalExpand 把输入行扩展成多个 grouping set 所需的布局。
pub struct PhysicalExpand {
    pub physical_schema_producer: PhysicalSchemaProducer,
    /// GroupingIDCol 是旧版 Expand 为每个展开结果生成的 grouping-id 列。
    pub GroupingIDCol: Option<expression::Column>,
    /// GroupingSets 定义旧版执行器要求的分组布局。
    pub GroupingSets: expression::GroupingSets,
    /// LevelExprs 是新版按 grouping level 展开的投影表达式列表。
    pub LevelExprs: Vec<Vec<expression::Expression>>,
    /// ExtraGroupingColNames 保存新版生成列名，例如 grouping_id。
    pub ExtraGroupingColNames: Vec<String>,
}

impl PhysicalExpand {
    /// Init 对应 Go 初始化逻辑，安装具体 self、查询块偏移和唯一子属性。
    pub fn Init(
        mut self,
        ctx: base::PlanContext,
        stats: &property::StatsInfo,
        offset: i32,
        props: Vec<property::PhysicalProperty>,
    ) -> Self {
        self.physical_schema_producer.BasePhysicalPlan =
            NewBasePhysicalPlan(ctx, plancodec::TypeExpand, &self, offset);
        self.physical_schema_producer.SetChildrenReqProps(props);
        self.physical_schema_producer.SetStats(stats);
        self
    }

    /// Clone 根据 LevelExprs 是否存在选择 Go 的 v1 或 v2 克隆路径。
    pub fn Clone(
        &self,
        new_ctx: base::PlanContext,
    ) -> Result<Box<dyn base::PhysicalPlan>, errors::Error> {
        if !self.LevelExprs.is_empty() {
            return self.cloneV2(new_ctx);
        }

        let mut cloned = PhysicalExpand::default();
        cloned.physical_schema_producer.SetSCtx(new_ctx.clone());
        cloned.physical_schema_producer = self
            .physical_schema_producer
            .CloneWithSelf(new_ctx, &mut cloned)
            .map_err(errors::Trace)?;

        // v1 的 grouping-id 列和每个 GroupingSet 都需要独立克隆，避免改写逻辑计划对象。
        cloned.GroupingIDCol = self.GroupingIDCol.as_ref().map(|column| column.Clone());
        cloned.GroupingSets = self
            .GroupingSets
            .iter()
            .map(|grouping_set| grouping_set.Clone())
            .collect();
        Ok(Box::new(cloned))
    }

    /// cloneV2 深克隆每层投影表达式和生成列名。
    fn cloneV2(
        &self,
        new_ctx: base::PlanContext,
    ) -> Result<Box<dyn base::PhysicalPlan>, errors::Error> {
        let mut cloned = PhysicalExpand::default();
        cloned.physical_schema_producer = self
            .physical_schema_producer
            .CloneWithSelf(new_ctx, &mut cloned)
            .map_err(errors::Trace)?;
        for level_exprs in &self.LevelExprs {
            cloned.LevelExprs.push(util::CloneExprs(level_exprs));
        }
        for name in &self.ExtraGroupingColNames {
            cloned.ExtraGroupingColNames.push(name.clone());
        }
        Ok(Box::new(cloned))
    }

    /// MemoryUsage 保留 Go v1 的统计范围：基类、GroupingSets 容器、各 set 和 grouping-id 列。
    pub fn MemoryUsage(&self) -> i64 {
        let mut sum = self.physical_schema_producer.MemoryUsage()
            + size::SizeOfSlice
            + self.GroupingSets.capacity() as i64 * size::SizeOfPointer;
        for grouping_set in &self.GroupingSets {
            sum += grouping_set.MemoryUsage();
        }
        if let Some(grouping_id) = &self.GroupingIDCol {
            sum += grouping_id.MemoryUsage();
        }
        sum
    }

    /// explainInfoV2 按 level 顺序打印投影，并附上输出 schema。
    fn explainInfoV2(&self) -> String {
        let eval_ctx = self.physical_schema_producer.SCtx().GetExprCtx().GetEvalCtx();
        let enable_redact = self
            .physical_schema_producer
            .SCtx()
            .GetSessionVars()
            .EnableRedactLog;
        let schema = self.physical_schema_producer.Schema();
        let mut text = String::new();
        for (index, level) in self.LevelExprs.iter().enumerate() {
            if index == 0 {
                text.push_str("level-projection:[");
            } else {
                text.push_str(",[");
            }
            text.push_str(&expression::ExplainExpressionList(
                eval_ctx,
                level,
                schema,
                enable_redact,
            ));
            text.push(']');
        }
        text.push_str("; schema: [");
        let columns: Vec<String> = schema
            .Columns
            .iter()
            .map(|column| column.StringWithCtx(eval_ctx, errors::RedactLogDisable))
            .collect();
        text.push_str(&columns.join(","));
        text.push(']');
        text
    }

    /// ExplainInfo 根据表示版本输出 level projection 或旧版 grouping set 信息。
    pub fn ExplainInfo(&self) -> String {
        if !self.LevelExprs.is_empty() {
            return self.explainInfoV2();
        }

        let eval_ctx = self.physical_schema_producer.SCtx().GetExprCtx().GetEvalCtx();
        let mut text = format!("group set num:{}", self.GroupingSets.len());
        if let Some(grouping_id) = &self.GroupingIDCol {
            text.push_str(", groupingID:");
            text.push_str(&grouping_id.StringWithCtx(eval_ctx, errors::RedactLogDisable));
            text.push_str(", ");
        }
        text.push_str(&self.GroupingSets.StringWithCtx(eval_ctx, errors::RedactLogDisable));
        text
    }

    /// ResolveIndicesItself 依次解析 v1 grouping set 与 v2 level projection 的列索引。
    pub fn ResolveIndicesItself(&mut self) -> Result<(), errors::Error> {
        let input_schema = self.physical_schema_producer.Children()[0].Schema();
        for grouping_set in &mut self.GroupingSets {
            for grouping_exprs in grouping_set {
                for expression in grouping_exprs {
                    // 原 Go 代码就地覆盖；任一解析错误都终止，已完成的前缀保持更新状态。
                    *expression = expression.ResolveIndices(input_schema)?;
                }
            }
        }
        for level in &mut self.LevelExprs {
            for expression in level {
                // v2 level 只应包含列引用和字面常量，但仍通过统一接口完成索引解析。
                *expression = expression.ResolveIndices(input_schema)?;
            }
        }
        Ok(())
    }

    /// ResolveIndices 先递归处理基类计划，再解析 Expand 自身表达式。
    pub fn ResolveIndices(&mut self) -> Result<(), errors::Error> {
        self.physical_schema_producer.ResolveIndices()?;
        self.ResolveIndicesItself()
    }

    /// ToPB 根据 LevelExprs 选择 Expand 或 Expand2 消息。
    pub fn ToPB(
        &self,
        ctx: &base::BuildPBContext,
        store_type: kv::StoreType,
    ) -> Result<tipb::Executor, errors::Error> {
        if !self.LevelExprs.is_empty() {
            return self.toPBV2(ctx, store_type);
        }

        let grouping_sets_pb = self.GroupingSets.ToPB(
            ctx.GetExprCtx().GetEvalCtx(),
            ctx.GetClient(),
        )?;
        let mut expand = tipb::Expand {
            GroupingSets: grouping_sets_pb,
            ..Default::default()
        };
        let mut executor_id = String::new();
        if store_type == kv::TiFlash {
            // 只有 TiFlash 计划把子 executor 内嵌进 Expand；其他存储保持 Go 的空 child。
            expand.Child = Some(
                self.physical_schema_producer.Children()[0]
                    .ToPB(ctx, store_type)
                    .map_err(errors::Trace)?,
            );
            executor_id = self.physical_schema_producer.ExplainID().String();
        }
        Ok(tipb::Executor {
            Tp: tipb::ExecType_TypeExpand,
            Expand: Some(expand),
            ExecutorId: Some(executor_id),
            ..Default::default()
        })
    }

    /// toPBV2 把每层投影编码为 ExprSlice，并携带生成列名。
    fn toPBV2(
        &self,
        ctx: &base::BuildPBContext,
        store_type: kv::StoreType,
    ) -> Result<tipb::Executor, errors::Error> {
        let mut projections = Vec::with_capacity(self.LevelExprs.len());
        let eval_ctx = ctx.GetExprCtx().GetEvalCtx();
        for expressions in &self.LevelExprs {
            let encoded = expression::ExpressionsToPBList(
                eval_ctx,
                expressions,
                ctx.GetClient(),
            )?;
            projections.push(tipb::ExprSlice { Exprs: encoded });
        }
        let mut expand = tipb::Expand2 {
            ProjExprs: projections,
            GeneratedOutputNames: self.ExtraGroupingColNames.clone(),
            ..Default::default()
        };
        let mut executor_id = String::new();
        if store_type == kv::TiFlash {
            expand.Child = Some(
                self.physical_schema_producer.Children()[0]
                    .ToPB(ctx, store_type)
                    .map_err(errors::Trace)?,
            );
            executor_id = self.physical_schema_producer.ExplainID().String();
        }
        Ok(tipb::Executor {
            Tp: tipb::ExecType_TypeExpand2,
            Expand2: Some(expand),
            ExecutorId: Some(executor_id),
            ..Default::default()
        })
    }

    /// Attach2Task 委托 utilfuncp，避免在 physicalop 与 task 实现之间形成循环依赖。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn base::Task>>) -> Box<dyn base::Task> {
        utilfuncp::Attach2Task4PhysicalExpand(self, tasks)
    }
}

/// ExhaustPhysicalPlans4LogicalExpand 从 LogicalExpand 枚举 Root/MPP 候选计划。
/// 返回 bool 保留 Go 的“是否允许添加属性 enforcer”约定。
pub fn ExhaustPhysicalPlans4LogicalExpand(
    logical: &logicalop::LogicalExpand,
    prop: &property::PhysicalProperty,
) -> Result<(Vec<Box<dyn base::PhysicalPlan>>, bool), errors::Error> {
    if !prop.IsSortItemEmpty() {
        // Expand 无法保持排序；false 允许上层另加 Sort enforcer。
        return Ok((Vec::new(), false));
    }
    if prop.TaskTp != property::RootTaskType && prop.TaskTp != property::MppTaskType {
        return Ok((Vec::new(), true));
    }
    if prop.TaskTp == property::MppTaskType && prop.MPPPartitionTp != property::AnyType {
        // 当前 Expand 不承诺保留输入分区，因此拒绝上层指定的 MPP 分区布局。
        return Ok((Vec::new(), true));
    }

    let mut plans: Vec<Box<dyn base::PhysicalPlan>> = Vec::new();
    if logical.SCtx().GetSessionVars().IsMPPAllowed() {
        let mut child_prop = prop.CloneEssentialFields();
        child_prop.TaskTp = property::MppTaskType;
        let mut expand = PhysicalExpand {
            GroupingSets: logical.RollupGroupingSets.clone(),
            LevelExprs: logical.LevelExprs.clone(),
            ExtraGroupingColNames: logical.ExtraGroupingColNames.clone(),
            ..Default::default()
        }
        .Init(
            logical.SCtx(),
            &logical.StatsInfo().ScaleByExpectCnt(
                logical.SCtx().GetSessionVars(),
                prop.ExpectedCnt,
            ),
            logical.QueryBlockOffset(),
            vec![child_prop],
        );
        expand.physical_schema_producer.SetSchema(logical.Schema());
        plans.push(Box::new(expand));
        if prop.TaskTp == property::MppTaskType {
            // 调用方明确要求 MPP 时，不再生成 TiDB 上执行的候选。
            return Ok((plans, true));
        }
    }

    // Root 属性可接受多种子任务；每种 child task type 都保留一个 TiDB Expand 候选。
    let task_types = [
        property::CopSingleReadTaskType,
        property::CopMultiReadTaskType,
        property::MppTaskType,
        property::RootTaskType,
    ];
    for task_type in task_types {
        let mut child_prop = prop.CloneEssentialFields();
        child_prop.TaskTp = task_type;
        let mut expand = PhysicalExpand {
            GroupingSets: logical.RollupGroupingSets.clone(),
            LevelExprs: logical.LevelExprs.clone(),
            ExtraGroupingColNames: logical.ExtraGroupingColNames.clone(),
            ..Default::default()
        }
        .Init(
            logical.SCtx(),
            &logical.StatsInfo().ScaleByExpectCnt(
                logical.SCtx().GetSessionVars(),
                prop.ExpectedCnt,
            ),
            logical.QueryBlockOffset(),
            vec![child_prop],
        );
        expand.physical_schema_producer.SetSchema(logical.Schema());
        plans.push(Box::new(expand));
    }
    Ok((plans, true))
}
*/

// Expand 物理算子：把输入行按 grouping set / level 投影展开，
// 供 ROLLUP/CUBE/GROUPING SETS 生成多组聚合输入；当前可推送实现面向 MPP/TiFlash。

use crate::physical_common_plans::{
    Datum, PartitionType, PhysicalExpr, PhysicalKind, PhysicalPlanNode, PhysicalProperty, Stats,
    TaskType,
};

#[derive(Clone, Debug, Default, PartialEq)]
/// 按多层投影展开输入行，并携带生成列名与 grouping 元数据。
pub struct PhysicalExpand {
    /// 各 grouping level 的投影表达式。
    pub levels: Vec<Vec<PhysicalExpr>>,
    /// 生成列名（如 grouping_id）。
    pub generated_column_names: Vec<String>,
    /// 各展开结果对应的 grouping id。
    pub grouping_ids: Vec<u64>,
    /// grouping 相关位置元数据。
    pub grouping_pos: Vec<usize>,
    /// 输出列 ID。
    pub schema: Vec<i64>,
    /// 唯一孩子计划。
    pub child: Option<PhysicalPlanNode>,
}
impl PhysicalExpand {
    /// 估算 levels 与生成列名容量。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self.levels.iter().flatten().count() as i64
                * std::mem::size_of::<PhysicalExpr>() as i64
            + self
                .generated_column_names
                .iter()
                .map(|name| name.capacity() as i64)
                .sum::<i64>()
    }
    /// 按 level 打印投影表达式摘要。
    pub fn explain_info(&self) -> String {
        let levels = self
            .levels
            .iter()
            .map(|level| {
                format!(
                    "[{}]",
                    level.iter().map(format_expr).collect::<Vec<_>>().join(",")
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        let schema = self
            .schema
            .iter()
            .map(|id| format!("Column#{id}"))
            .collect::<Vec<_>>()
            .join(",");
        format!("level-projection:{levels}; schema: [{schema}]")
    }
    /// 相对孩子 schema 解析各层表达式列索引。
    pub fn resolve_indices(&mut self) -> Result<(), String> {
        let schema = self
            .child
            .as_ref()
            .map(|child| child.schema.as_slice())
            .unwrap_or(&[]);
        self.levels
            .iter_mut()
            .flatten()
            .try_for_each(|expr| expr.resolve_indices(schema))
    }
    /// 封装 ExpandExecutor；是否内嵌孩子由调用方的存储类型决定。
    pub fn to_pb(&self, _store: TaskType) -> Result<ExpandExecutor, String> {
        Ok(ExpandExecutor {
            levels: self.levels.clone(),
            generated_output_names: self.generated_column_names.clone(),
            grouping_ids: self.grouping_ids.clone(),
            grouping_pos: self.grouping_pos.clone(),
        })
    }
}
/// 将物理表达式格式化为 EXPLAIN 片段。
fn format_expr(expr: &PhysicalExpr) -> String {
    match expr {
        PhysicalExpr::Column(id) => format!("Column#{id}"),
        PhysicalExpr::Constant(Datum::Null) => "NULL".into(),
        PhysicalExpr::Constant(value) => format!("{value:?}"),
        PhysicalExpr::Scalar { function, .. } => function.clone(),
        PhysicalExpr::CorrelatedColumn(id) => format!("CorrelatedColumn#{id}"),
        PhysicalExpr::Default { .. } => "DEFAULT".into(),
    }
}
#[derive(Clone, Debug, PartialEq)]
/// 下推到 TiFlash 的 Expand 执行描述。
pub struct ExpandExecutor {
    pub levels: Vec<Vec<PhysicalExpr>>,
    pub generated_output_names: Vec<String>,
    pub grouping_ids: Vec<u64>,
    pub grouping_pos: Vec<usize>,
}
/// 从逻辑 Expand 枚举 Root/MPP 候选；Expand 自身不保持排序或 MPP 分区。
pub fn exhaust_physical_expand(
    expand: PhysicalExpand,
    property: &PhysicalProperty,
    stats: Stats,
) -> (Vec<PhysicalPlanNode>, bool) {
    if !property.sort_items.is_empty() {
        // 与 Go 的 false 一致：上层可以添加 Sort enforcer。
        return (Vec::new(), false);
    }
    if property.task_type != TaskType::Root && property.task_type != TaskType::Mpp {
        return (Vec::new(), true);
    }
    if property.task_type == TaskType::Mpp && property.partition_type != PartitionType::Any {
        return (Vec::new(), true);
    }

    let make_plan = |task_type| PhysicalPlanNode {
        id: 0,
        kind: PhysicalKind::Expand,
        schema: expand.schema.clone(),
        children: expand.child.clone().into_iter().collect(),
        stats: stats.clone(),
        required_properties: vec![PhysicalProperty {
            task_type,
            ..PhysicalProperty::default()
        }],
    };

    if property.task_type == TaskType::Mpp {
        return (vec![make_plan(TaskType::Mpp)], true);
    }

    // 简化模型把 Go 的 CopSingleRead/CopMultiRead 合并为 Cop，因此保留两次枚举。
    (
        [TaskType::Mpp, TaskType::Cop, TaskType::Mpp, TaskType::Root]
            .into_iter()
            .map(make_plan)
            .collect(),
        true,
    )
}

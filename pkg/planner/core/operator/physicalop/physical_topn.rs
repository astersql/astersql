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

// 物理 TopN 算子：在排序键上取前 N 行（可带 OFFSET）。
//
// TopN 比完整 Sort+Limit 更省：用有界堆维护候选。可下推到 Cop/MPP，
// 也可在有序输入上退化为 Limit。向量检索（vector search）场景可走 MPP 专用属性。

use std::sync::Arc;

use base::{ContextRef, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::Expression as _;
use logicalop::LogicalPlan as _;
use protobuf::ProtobufEnum as _;

use crate::{BasePhysicalPlan, PhysicalLimit, PhysicalSchemaProducer};

/// 物理 TopN：按 ByItems 排序后保留 Count 行，跳过 Offset。
pub struct PhysicalTopN {
    /// Schema/统计等公共物理计划字段。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 排序键（表达式 + 升/降序）。
    pub ByItems: Vec<planner_util::ByItems>,
    /// 窗口式分区键：在每个分区内分别做 TopN。
    pub PartitionBy: Vec<property::SortItem>,
    /// 跳过的行数（对应 SQL OFFSET）。
    pub Offset: u64,
    /// 保留的行数（对应 SQL LIMIT）。
    pub Count: u64,
    /// 前缀列优化：可用列前缀有序性加速 TopN。
    pub PrefixCol: Option<expression::Column>,
    /// 前缀列可用字节/字符长度。
    pub PrefixLen: usize,
}

impl PhysicalTopN {
    /// 构造空排序键的 TopN 骨架。
    pub fn New(ctx: ContextRef, offset: u64, count: u64) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeTopN,
                0,
            )),
            ByItems: Vec::new(),
            PartitionBy: Vec::new(),
            Offset: offset,
            Count: count,
            PrefixCol: None,
            PrefixLen: 0,
        }
    }

    /// 写入统计、查询块偏移与子节点所需物理属性。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        props: Vec<Box<property::PhysicalProperty>>,
    ) -> Self {
        self.PhysicalSchemaProducer.BasePhysicalPlan =
            BasePhysicalPlan::New(ctx, plancodec::TypeTopN, offset);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(props);
        self
    }

    /// 返回分区排序项切片。
    pub fn GetPartitionBy(&self) -> &[property::SortItem] {
        &self.PartitionBy
    }

    /// 深拷贝表达式与 Schema，换绑 PlanContext。
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
            ByItems: self
                .ByItems
                .iter()
                .map(planner_util::ByItems::Clone)
                .collect(),
            PartitionBy: self
                .PartitionBy
                .iter()
                .map(property::SortItem::Clone)
                .collect(),
            Offset: self.Offset,
            Count: self.Count,
            PrefixCol: self.PrefixCol.as_ref().map(expression::Column::Clone),
            PrefixLen: self.PrefixLen,
        })
    }

    /// 从排序表达式抽取相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<expression::CorrelatedColumn> {
        self.ByItems
            .iter()
            .flat_map(|item| expression::ExtractCorColumns(item.Expr.as_ref()))
            .map(expression::CorrelatedColumn::Clone)
            .collect()
    }

    /// 估算本节点内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .ByItems
                .iter()
                .map(planner_util::ByItems::MemoryUsage)
                .sum::<i64>()
            + self
                .PartitionBy
                .iter()
                .map(property::SortItem::MemoryUsage)
                .sum::<i64>()
            + self
                .PrefixCol
                .as_ref()
                .map_or(0, expression::Column::MemoryUsage)
            + (std::mem::size_of::<u64>() * 2 + std::mem::size_of::<usize>()) as i64
    }

    /// 生成 Explain：partition by、order by、offset/count（尊重 redact）。
    pub fn ExplainInfo(&self) -> String {
        let eval = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetExprCtx()
            .GetEvalCtx();
        let mut text = String::new();
        property::ExplainPartitionBy(eval, &mut text, &self.PartitionBy, false);
        if !self.PartitionBy.is_empty() {
            text.push(' ');
            if !self.ByItems.is_empty() {
                text.push_str("order by ");
            }
        }
        planner_util::ExplainByItems(eval, &mut text, &self.ByItems);
        let redact = eval.GetTiDBRedactLog();
        match redact.as_str() {
            expression::errors::RedactLogEnable => text.push_str(", offset:?, count:?"),
            expression::errors::RedactLogMarker => text.push_str(&format!(
                ", offset:‹{}›, count:‹{}›",
                self.Offset, self.Count
            )),
            _ => text.push_str(&format!(", offset:{}, count:{}", self.Offset, self.Count)),
        }
        if let Some(column) = &self.PrefixCol {
            match redact.as_str() {
                expression::errors::RedactLogEnable => {
                    text.push_str(", prefix_col:?, prefix_len:?")
                }
                expression::errors::RedactLogMarker => text.push_str(&format!(
                    ", prefix_col:‹{}›, prefix_len:‹{}›",
                    column.ExplainInfo(eval),
                    self.PrefixLen
                )),
                _ => text.push_str(&format!(
                    ", prefix_col:{}, prefix_len:{}",
                    column.ExplainInfo(eval),
                    self.PrefixLen
                )),
            }
        }
        text
    }

    /// 归一化 Explain（省略具体 offset/count 数值）。
    pub fn ExplainNormalizedInfo(&self) -> String {
        let eval = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetExprCtx()
            .GetEvalCtx();
        let mut text = String::new();
        property::ExplainPartitionBy(eval, &mut text, &self.PartitionBy, true);
        if !self.PartitionBy.is_empty() {
            text.push(' ');
            if !self.ByItems.is_empty() {
                text.push_str("order by ");
            }
        }
        for (index, item) in self.ByItems.iter().enumerate() {
            text.push_str(&item.Expr.ExplainNormalizedInfo());
            if item.Desc {
                text.push_str(":desc");
            }
            if index + 1 < self.ByItems.len() {
                text.push_str(", ");
            }
        }
        text
    }

    /// 堆式 TopN 代价：CPU ~ n·log(heap) + 内存因子。
    pub fn GetCost(&self, count: f64, root: bool) -> f64 {
        let heap_size = (self.Offset.wrapping_add(self.Count) as f64).max(2.0);
        let vars = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetSessionVars();
        let cpu_factor = if root {
            vars.GetCPUFactor()
        } else {
            vars.GetCopCPUFactor()
        };
        count * heap_size.log2() * cpu_factor + heap_size * vars.GetMemoryFactor()
    }

    /// 按子节点 Schema 解析排序/分区/前缀列下标。
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
        for item in &mut self.ByItems {
            item.Expr = item.Expr.ResolveIndices(&schema)?;
        }
        for item in &mut self.PartitionBy {
            item.Col = item.Col.ResolveIndices(&schema)?;
        }
        if let Some(prefix) = &mut self.PrefixCol {
            *prefix = prefix.ResolveIndices(&schema)?;
        }
        Ok(())
    }

    /// 将算子挂到执行任务（root/cop/mpp）上。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
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

    /// 序列化为 tipb::TopN；TiFlash 还需嵌入子计划。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let client = ctx
            .GetClient()
            .ok_or_else(|| expression::errors::New("PB client is required"))?;
        let expression_context = ctx.GetExprCtx();
        let eval = expression_context.GetEvalCtx();
        let mut topn = tipb::TopN::new();
        topn.set_limit(self.Count);
        topn.set_order_by(
            self.ByItems
                .iter()
                .map(|item| {
                    expression::SortByItemToPB(eval, client.as_ref(), item.Expr.as_ref(), item.Desc)
                        .ok_or_else(|| {
                            expression::errors::New("TopN expression cannot be pushed down")
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        );
        topn.set_partition_by(
            self.PartitionBy
                .iter()
                .map(|item| {
                    expression::SortByItemToPB(eval, client.as_ref(), &item.Col, item.Desc)
                        .ok_or_else(|| {
                            expression::errors::New(
                                "TopN partition expression cannot be pushed down",
                            )
                        })
                })
                .collect::<Result<Vec<_>, _>>()?
                .into(),
        );
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeTopN);
        if store == kv::StoreType::TiFlash {
            let child = self
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .Children()
                .first()
                .ok_or_else(|| expression::errors::New("TopN requires one child"))?
                .to_pb(ctx, store)?;
            topn.set_child(*child);
            executor.set_executor_id(
                self.PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .explain_id(&[])
                    .to_string(),
            );
        }
        executor.set_top_n(topn);
        Ok(Box::new(executor))
    }
}

/// Returns the two Go preference slices separately: TopN candidates first,
/// order-preserving Limit candidates second.
/// 枚举逻辑 TopN 的物理实现：优先 TopN 候选，其次保序 Limit。
pub fn ExhaustPhysicalPlans4LogicalTopN(
    logical: &logicalop::LogicalTopN,
    required: &property::PhysicalProperty,
) -> Vec<Vec<Box<dyn PhysicalPlan>>> {
    if !crate::MatchItems(required, &logical.ByItems) {
        return Vec::new();
    }
    vec![
        getPhysTopN(logical, required),
        getPhysLimits(logical, required),
    ]
}

/// 从逻辑 TopN 复制字段并 Init 一个物理节点。
fn new_topn(
    logical: &logicalop::LogicalTopN,
    child: property::PhysicalProperty,
) -> Option<PhysicalTopN> {
    let ctx = logical.SCtx()?.clone();
    // Re-derive the output cardinality from the current child after rewrites.
    // Go's TopN candidates receive logical stats after `DeriveLimitStats`; a
    // stale cached logical count must not leak through into the physical plan.
    let stats = logical
        .Children()
        .first()
        .and_then(|child| child.StatsInfo())
        .map(|child_stats| property::DeriveLimitStats(child_stats, logical.Count as f64))
        .or_else(|| logical.StatsInfo().cloned())
        .unwrap_or_default();
    let mut topn = PhysicalTopN::New(ctx.clone(), logical.Offset, logical.Count);
    topn.ByItems = logical
        .ByItems
        .iter()
        .map(planner_util::ByItems::Clone)
        .collect();
    topn.PartitionBy = logical
        .PartitionBy
        .iter()
        .map(property::SortItem::Clone)
        .collect();
    topn.PhysicalSchemaProducer
        .SetSchema(logical.Schema().Clone());
    Some(topn.Init(
        ctx,
        stats,
        logical.QueryBlockOffset(),
        vec![Box::new(child)],
    ))
}

/// 枚举各 TaskType 下的 TopN；含部分有序索引与向量检索路径。
fn getPhysTopN(
    logical: &logicalop::LogicalTopN,
    required: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    let Some(ctx) = logical.SCtx() else {
        return Vec::new();
    };
    let mut result = Vec::new();
    if canUsePartialOrder4TopN(logical) {
        // 部分有序索引：要求 CopMultiRead，并带 PartialOrderInfo。
        let mut sort_items = Vec::new();
        for item in &logical.ByItems {
            let Some(column) = item.Expr.as_any().downcast_ref::<expression::Column>() else {
                sort_items.clear();
                break;
            };
            sort_items.push(property::SortItem {
                Col: column.Clone(),
                Desc: item.Desc,
            });
        }
        if !sort_items.is_empty() {
            let mut child = property::PhysicalProperty::default();
            child.TaskTp = property::CopMultiReadTaskType;
            child.ExpectedCnt = f64::MAX;
            child.PartialOrderInfo = Some(property::PartialOrderInfo {
                SortItems: sort_items,
            });
            child.CTEProducerStatus = required.CTEProducerStatus;
            child.NoCopPushDown = required.NoCopPushDown;
            if let Some(plan) = new_topn(logical, child) {
                result.push(Box::new(plan) as Box<dyn PhysicalPlan>);
            }
        }
    }
    let mut task_types = vec![
        property::CopSingleReadTaskType,
        property::CopMultiReadTaskType,
        property::RootTaskType,
    ];
    if ctx.GetSessionVars().IsMPPAllowed() {
        task_types.push(property::MppTaskType);
    }
    let advisory = logical
        .Children()
        .first()
        .and_then(|child| child.as_any().downcast_ref::<logicalop::DataSource>())
        .and_then(|_| crate::GetPropByOrderByItems(&logical.ByItems))
        .map(|prop| prop.SortItems);
    for task_type in task_types {
        let mut child = property::PhysicalProperty::default();
        child.TaskTp = task_type;
        child.ExpectedCnt = f64::MAX;
        child.CTEProducerStatus = required.CTEProducerStatus;
        child.NoCopPushDown = required.NoCopPushDown;
        if let Some(plan) = new_topn(logical, child.CloneEssentialFields()) {
            result.push(Box::new(plan));
        }
        if task_type == property::CopMultiReadTaskType
            && advisory.as_ref().is_some_and(|items| !items.is_empty())
        {
            child.AdvisorySortItems = advisory
                .as_ref()
                .unwrap()
                .iter()
                .map(property::SortItem::Clone)
                .collect();
            if let Some(plan) = new_topn(logical, child) {
                result.push(Box::new(plan));
            }
        }
    }
    if ctx.GetSessionVars().IsMPPAllowed()
        // 向量 TopK：单升序距离表达式且无下推过滤时，走 MPP VectorProp。
        && logical.ByItems.len() == 1
        && !logical.ByItems[0].Desc
        && logical
            .Children()
            .first()
            .and_then(|child| child.as_any().downcast_ref::<logicalop::DataSource>())
            .is_some_and(|source| source.PushedDownConds.is_empty())
        && let Some(info) = interpret_vector_search(logical.ByItems[0].Expr.as_ref())
    {
        let mut child = property::PhysicalProperty::default();
        child.TaskTp = property::MppTaskType;
        child.ExpectedCnt = f64::MAX;
        child.CTEProducerStatus = required.CTEProducerStatus;
        child.VectorProp.VSInfo = Some(property::VectorSearchInfo {
            DistanceFnName: info.DistanceFnName,
            FnPbCode: info.FnPbCode,
            Vec: info.Vec,
            Column: info.Column,
        });
        child.VectorProp.TopK = logical.Count.wrapping_add(logical.Offset) as u32;
        if let Some(plan) = new_topn(logical, child) {
            result.push(Box::new(plan));
        }
    }
    result
}

/// 当子节点已能提供 ORDER BY 有序性时，用 Limit 替代 TopN。
fn getPhysLimits(
    logical: &logicalop::LogicalTopN,
    required: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    let Some(order) = crate::GetPropByOrderByItems(&logical.ByItems) else {
        return Vec::new();
    };
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    let stats = logical
        .Children()
        .first()
        .and_then(|child| child.StatsInfo())
        .map(|child_stats| property::DeriveLimitStats(child_stats, logical.Count as f64))
        .or_else(|| logical.StatsInfo().cloned())
        .unwrap_or_default();
    [
        property::CopSingleReadTaskType,
        property::CopMultiReadTaskType,
        property::RootTaskType,
    ]
    .into_iter()
    .map(|task_type| {
        let mut child = property::PhysicalProperty::default();
        child.TaskTp = task_type;
        child.ExpectedCnt = logical.Count.wrapping_add(logical.Offset) as f64;
        child.SortItems = order
            .SortItems
            .iter()
            .map(property::SortItem::Clone)
            .collect();
        child.CTEProducerStatus = required.CTEProducerStatus;
        child.NoCopPushDown = required.NoCopPushDown;
        let mut limit = PhysicalLimit::New(ctx.clone(), logical.Offset, logical.Count);
        limit.OffsetParam = logical.OffsetParam;
        limit.CountParam = logical.CountParam;
        limit.PartitionBy = logical
            .PartitionBy
            .iter()
            .map(property::SortItem::Clone)
            .collect();
        limit
            .PhysicalSchemaProducer
            .SetSchema(logical.Schema().Clone());
        Box::new(limit.Init(
            ctx.clone(),
            stats.clone(),
            logical.QueryBlockOffset(),
            vec![Box::new(child)],
        )) as Box<dyn PhysicalPlan>
    })
    .collect()
}

/// 会话开关开启且子树符合 DataSource(+Selection/Projection) 模式时可用部分有序。
fn canUsePartialOrder4TopN(logical: &logicalop::LogicalTopN) -> bool {
    logical
        .SCtx()
        .is_some_and(|ctx| ctx.GetSessionVars().IsPartialOrderedIndexForTopNEnabled())
        && !logical.ByItems.is_empty()
        && logical
            .Children()
            .first()
            .is_some_and(|child| checkPartialOrderPattern(child.as_ref()))
}

/// 检查是否为 DataSource 上仅经 Selection/Projection 的一元链。
fn checkPartialOrderPattern(plan: &dyn logicalop::LogicalPlan) -> bool {
    if plan.as_any().is::<logicalop::DataSource>() {
        return true;
    }
    if plan.as_any().is::<logicalop::LogicalSelection>()
        || plan.as_any().is::<logicalop::LogicalProjection>()
    {
        return plan.Children().len() == 1 && checkPartialOrderPattern(plan.Children()[0].as_ref());
    }
    false
}

/// 识别向量距离函数 + 列/常量，构造 VectorSearchInfo。
fn interpret_vector_search(
    expression: &dyn expression::Expression,
) -> Option<property::VectorSearchInfo> {
    let function = expression
        .as_any()
        .downcast_ref::<expression::ScalarFunction>()?;
    let name = function.FuncName.L.as_str();
    if ![
        expression::ast::VecL1Distance,
        expression::ast::VecL2Distance,
        expression::ast::VecCosineDistance,
        expression::ast::VecNegativeInnerProduct,
    ]
    .iter()
    .any(|candidate| candidate.eq_ignore_ascii_case(name))
    {
        return None;
    }
    let mut column = None;
    let mut vector = None;
    for argument in function.GetArgs() {
        if let Some(candidate) = argument.as_any().downcast_ref::<expression::Column>() {
            if candidate.RetType.as_ref()?.GetType() != expression::mysql::TypeTiDBVectorFloat32
                || column.is_some()
            {
                return None;
            }
            column = Some(candidate.Clone());
        } else if let Some(candidate) = argument.as_any().downcast_ref::<expression::Constant>() {
            if candidate.RetType.as_ref()?.GetType() != expression::mysql::TypeTiDBVectorFloat32
                || vector.is_some()
            {
                return None;
            }
            vector = Some(candidate.Value.GetVectorFloat32());
        }
    }
    Some(property::VectorSearchInfo {
        DistanceFnName: function.FuncName.O.clone(),
        FnPbCode: tipb::ScalarFuncSig::from_i32(function.Function.PbCode())?,
        Vec: Arc::new(vector?),
        Column: column?,
    })
}

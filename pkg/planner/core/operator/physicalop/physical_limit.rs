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

// Limit 物理算子：实现 SQL 的 LIMIT/OFFSET，支持分区内截断与前缀键截断。
//
// `PartitionBy` 表示按分组键各自取前 N 行；`PrefixCol`/`PrefixLen` 用于
// 按前缀键截断（例如推送到存储引擎的 truncate key 表达式）。

use base::{ContextRef, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::{Column, CorrelatedColumn};
use property::SortItem;

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// 对应 Go `PhysicalLimit`：offset/count 截断，可选分区与前缀截断元数据。
pub struct PhysicalLimit {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 分区内 Limit 的分组排序键。
    pub PartitionBy: Vec<SortItem>,
    /// 跳过的行数（OFFSET）。
    pub Offset: u64,
    /// 最多返回的行数（LIMIT count）。
    pub Count: u64,
    pub OffsetParam: Option<usize>,
    pub CountParam: Option<usize>,
    /// Cop-side copy uses COUNT+OFFSET while retaining the root markers.
    pub CountIncludesOffset: bool,
    /// 可选前缀列，用于生成 truncate key 表达式。
    pub PrefixCol: Option<Column>,
    /// 前缀长度。
    pub PrefixLen: usize,
}

impl PhysicalLimit {
    /// 以给定 offset/count 构造 TypeLimit 空壳。
    pub fn New(ctx: ContextRef, offset: u64, count: u64) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeLimit,
                0,
            )),
            PartitionBy: Vec::new(),
            Offset: offset,
            Count: count,
            OffsetParam: None,
            CountParam: None,
            CountIncludesOffset: false,
            PrefixCol: None,
            PrefixLen: 0,
        }
    }

    /// 绑定统计、查询块偏移与子节点物理属性需求。
    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        qb_offset: i32,
        props: Vec<Box<property::PhysicalProperty>>,
    ) -> Self {
        self.PhysicalSchemaProducer.BasePhysicalPlan =
            BasePhysicalPlan::New(ctx, plancodec::TypeLimit, qb_offset);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .set_stats(stats);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(props);
        self
    }

    /// 返回分区键切片。
    pub fn GetPartitionBy(&self) -> &[SortItem] {
        &self.PartitionBy
    }

    /// 深克隆 PartitionBy / PrefixCol 等到新上下文。
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
            PartitionBy: self.PartitionBy.iter().map(SortItem::Clone).collect(),
            Offset: self.Offset,
            Count: self.Count,
            OffsetParam: self.OffsetParam,
            CountParam: self.CountParam,
            CountIncludesOffset: self.CountIncludesOffset,
            PrefixCol: self.PrefixCol.as_ref().map(Column::Clone),
            PrefixLen: self.PrefixLen,
        })
    }

    /// Limit 本身不含关联列，返回空。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        Vec::new()
    }

    /// EXPLAIN：分区键、offset/count，并按 redact 策略脱敏字面量。
    pub fn ExplainInfo(&self) -> String {
        let eval = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetExprCtx()
            .GetEvalCtx();
        let redact = eval.GetTiDBRedactLog();
        let mut result = String::new();
        property::ExplainPartitionBy(eval, &mut result, &self.PartitionBy, false);
        if !result.is_empty() {
            result.push_str(", ");
        }
        // 按会话 redact 日志级别决定是否隐藏 offset/count 具体数值。
        match redact.as_str() {
            expression::errors::RedactLogEnable => result.push_str("offset:?, count:?"),
            expression::errors::RedactLogMarker => {
                result.push_str(&format!("offset:‹{}›, count:‹{}›", self.Offset, self.Count))
            }
            _ => result.push_str(&format!("offset:{}, count:{}", self.Offset, self.Count)),
        }
        if let Some(column) = &self.PrefixCol {
            match redact.as_str() {
                expression::errors::RedactLogEnable => {
                    result.push_str(", prefix_col:?, prefix_len:?")
                }
                expression::errors::RedactLogMarker => result.push_str(&format!(
                    ", prefix_col:‹{}›, prefix_len:‹{}›",
                    column.ExplainInfo(eval),
                    self.PrefixLen
                )),
                _ => result.push_str(&format!(
                    ", prefix_col:{}, prefix_len:{}",
                    column.ExplainInfo(eval),
                    self.PrefixLen
                )),
            }
        }
        result
    }

    /// 归一化 EXPLAIN：分区键归一化，offset/count 固定为占位符。
    pub fn ExplainNormalizedInfo(&self) -> String {
        let mut result = String::new();
        property::ExplainPartitionBy(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .s_ctx()
                .GetExprCtx()
                .GetEvalCtx(),
            &mut result,
            &self.PartitionBy,
            true,
        );
        if !result.is_empty() {
            result.push_str(", ");
        }
        result.push_str("offset:?, count:?");
        result
    }

    /// 按第一个孩子 Schema 解析 PartitionBy 与 PrefixCol 列下标。
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
        for item in &mut self.PartitionBy {
            item.Col = item.Col.ResolveIndices(&schema)?;
        }
        // Go's resolveIndexForInlineProjection walks both ordered schemas in
        // lockstep so duplicate output columns bind to distinct child slots.
        let mut output_schema = self.PhysicalSchemaProducer.Schema().Clone();
        let mut output_index = 0;
        let mut child_index = 0;
        while output_index < output_schema.Columns.len() && child_index < schema.Columns.len() {
            if output_schema.Columns[output_index].UniqueID != schema.Columns[child_index].UniqueID
            {
                child_index += 1;
                continue;
            }
            output_schema.Columns[output_index].Index = child_index as isize;
            output_index += 1;
            child_index += 1;
        }
        if output_index < output_schema.Columns.len() {
            return Err(expression::errors::New(format!(
                "some columns of {} cannot find the reference from its child",
                self.PhysicalSchemaProducer.BasePhysicalPlan.explain_id(&[])
            )));
        }
        self.PhysicalSchemaProducer.SetSchema(output_schema);
        if let Some(column) = &mut self.PrefixCol {
            *column = column.ResolveIndices(&schema)?;
        }
        Ok(())
    }

    /// 挂接到任务树。
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

    /// 序列化为 tipb::Limit；TiFlash 时额外嵌入子 Executor。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        let mut limit = tipb::Limit::new();
        limit.set_limit(self.Count);
        let expression_context = ctx.GetExprCtx();
        let eval = expression_context.GetEvalCtx();
        let client = ctx.GetClient();
        let partition = self
            .PartitionBy
            .iter()
            .map(|item| {
                let client = client
                    .as_ref()
                    .ok_or_else(|| expression::errors::New("PB client is required"))?;
                expression::SortByItemToPB(eval, client.as_ref(), &item.Col, item.Desc).ok_or_else(
                    || expression::errors::New("partition expression cannot be pushed down"),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        limit.set_partition_by(partition.into());
        if let Some(prefix) = &self.PrefixCol {
            let client = client
                .as_ref()
                .ok_or_else(|| expression::errors::New("PB client is required"))?;
            let prefix_expressions: Vec<expression::ExprBox> = vec![Box::new(prefix.Clone())];
            limit.set_truncate_key_expr(
                expression::ExpressionsToPBList(eval, &prefix_expressions, client.as_ref())?.into(),
            );
        }
        let mut executor_id = String::new();
        // TiFlash 下推要求完整子树一并编码进 PB。
        if store == kv::StoreType::TiFlash {
            let child = self
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .Children()
                .first()
                .ok_or_else(|| expression::errors::New("limit requires one child"))?
                .to_pb(ctx, store)?;
            limit.set_child(*child);
            executor_id = self
                .PhysicalSchemaProducer
                .BasePhysicalPlan
                .explain_id(&[])
                .to_string();
        }
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeLimit);
        executor.set_limit(limit);
        executor.set_executor_id(executor_id);
        Ok(Box::new(executor))
    }

    /// 按 Go 实现计入 PrefixCol 指针/内容及 offset、count、prefixLen 标量。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + std::mem::size_of::<*const Column>() as i64
            + self.PrefixCol.as_ref().map_or(0, Column::MemoryUsage)
            + std::mem::size_of::<u64>() as i64 * 2
            + std::mem::size_of::<usize>() as i64
    }
}

/// 从 LogicalLimit 枚举多种 TaskType（Cop/Root/可选 MPP）下的物理 Limit。
pub fn ExhaustPhysicalPlans4LogicalLimit(
    logical: &logicalop::LogicalLimit,
    property: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    use logicalop::LogicalPlan as _;
    // 上层仍要求排序时，单独的 Limit 无法满足，交由 TopN 等算子。
    if !property.SortItems.is_empty() {
        return Vec::new();
    }
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    let can_push_down_to_mpp = planner_util::ShouldCheckTiFlashPushDown(
        ctx.as_ref(),
        logicalop::GetHasTiFlash(Some(logical)),
    ) && ctx.GetSessionVars().IsMPPAllowed();
    if property.TaskTp == property::MppTaskType && !can_push_down_to_mpp {
        return Vec::new();
    }
    let mut task_types = vec![
        property::CopSingleReadTaskType,
        property::CopMultiReadTaskType,
        property::RootTaskType,
    ];
    if property.TaskTp == property::MppTaskType {
        task_types = vec![property::MppTaskType];
    } else if can_push_down_to_mpp {
        task_types.push(property::MppTaskType);
    }
    crate::AdmitIndexJoinTypes(task_types, property)
        .into_iter()
        .map(|task_type| {
            let mut child = property::PhysicalProperty::default();
            child.TaskTp = task_type;
            // 子节点至少需要 offset+count 行才能填满 Limit。
            child.ExpectedCnt = logical.Offset.wrapping_add(logical.Count) as f64;
            child.CTEProducerStatus = property.CTEProducerStatus;
            child.NoCopPushDown = property.NoCopPushDown;
            let mut limit = PhysicalLimit::New(ctx.clone(), logical.Offset, logical.Count);
            limit.OffsetParam = logical.OffsetParam;
            limit.CountParam = logical.CountParam;
            limit.PartitionBy = logical.PartitionBy.iter().map(SortItem::Clone).collect();
            limit
                .PhysicalSchemaProducer
                .SetSchema(logical.Schema().Clone());
            Box::new(limit.Init(
                ctx.clone(),
                logical.StatsInfo().cloned().unwrap_or_default(),
                logical.QueryBlockOffset(),
                vec![Box::new(child)],
            )) as Box<dyn PhysicalPlan>
        })
        .collect()
}

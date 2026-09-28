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

// 物理 UnionScan：把事务本地未提交修改与子节点读结果合并。
//
// 在 MVCC（多版本并发控制）事务中，当前语句可能已写入但尚未提交；
// UnionScan 用 HandleCols 定位行，按 Conditions 过滤后与存储扫描结果合并。
// 仅能在 Root 任务执行，不可下推到存储层。

use base::{ContextRef, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::{CorrelatedColumn, ExprBox};
use planner_util::HandleCols;

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// 合并事务本地行与子节点已读行（见上英文说明）。
/// Merges transaction-local rows with the rows read by its child.
pub struct PhysicalUnionScan {
    /// Schema/统计等公共字段。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 应用于本地变更行的过滤条件。
    pub Conditions: Vec<ExprBox>,
    /// 行句柄列（整数 handle 或 common handle），用于定位/去重。
    pub HandleCols: Box<dyn HandleCols>,
}

impl PhysicalUnionScan {
    /// 构造默认整数 Handle 的 UnionScan。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeUnionScan,
                0,
            )),
            Conditions: Vec::new(),
            HandleCols: Box::new(planner_util::IntHandleCols::default()),
        }
    }

    /// 写入统计、偏移与子属性。
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
            .SetTP(plancodec::TypeUnionScan);
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

    /// 深拷贝条件与 HandleCols。
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
            HandleCols: self.HandleCols.CloneHandleCols(),
        })
    }

    /// 从条件抽取相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.Conditions
            .iter()
            .flat_map(|expr| expression::ExtractCorColumns(expr.as_ref()))
            .map(CorrelatedColumn::Clone)
            .collect()
    }

    /// Explain：排序后的条件表达式列表。
    pub fn ExplainInfo(&self) -> String {
        let eval = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetExprCtx()
            .GetEvalCtx();
        String::from_utf8_lossy(&expression::SortedExplainExpressionList(
            eval,
            &self.Conditions,
        ))
        .into_owned()
    }

    /// 归一化 Explain。
    pub fn ExplainNormalizedInfo(&self) -> String {
        String::from_utf8_lossy(&expression::SortedExplainNormalizedExpressionList(
            &self.Conditions,
        ))
        .into_owned()
    }

    /// 按子节点 Schema 解析条件与 Handle 列下标。
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
        for condition in &mut self.Conditions {
            *condition = condition.ResolveIndices(&schema)?;
        }
        self.HandleCols = self.HandleCols.ResolveIndices(&schema)?;
        Ok(())
    }

    /// 挂接到 Root 任务。
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

    /// 禁止下推：UnionScan 只能在 TiDB 侧执行。
    pub fn ToPB(
        &self,
        _ctx: &mut base::BuildPBContext,
        _store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        Err(expression::errors::New(
            "union scan is a root-only operator and cannot be pushed down",
        ))
    }

    /// 估算内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + std::mem::size_of::<Vec<ExprBox>>() as i64
            + self
                .Conditions
                .iter()
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + self.HandleCols.MemoryUsage()
    }
}

/// 枚举逻辑 UnionScan：Flash/MPP 不支持；需通过 AdmitIndexJoinProp。
pub fn ExhaustPhysicalPlans4LogicalUnionScan(
    logical: &logicalop::LogicalUnionScan,
    property: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    use logicalop::LogicalPlan as _;
    if property.IsFlashProp() {
        // TiFlash/MPP 路径尚不支持 UnionScan，发出警告并返回空候选。
        if let Some(context) = logical.SCtx() {
            context.GetSessionVars().RaiseWarningWhenMPPEnforced(
                "MPP mode may be blocked because operator `UnionScan` is not supported now.",
            );
        }
        return Vec::new();
    }
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    let Some(child) =
        crate::AdmitIndexJoinProp(Box::new(property.CloneEssentialFields()), property)
    else {
        return Vec::new();
    };
    let mut union_scan = PhysicalUnionScan::New(ctx.clone());
    union_scan.Conditions = logical
        .Conditions
        .iter()
        .map(|condition| condition.CloneExpr())
        .collect();
    union_scan.HandleCols = logical.HandleCols.CloneHandleCols();
    union_scan
        .PhysicalSchemaProducer
        .SetSchema(logical.Schema().Clone());
    vec![Box::new(union_scan.Init(
        ctx,
        logical.StatsInfo().cloned().unwrap_or_default(),
        logical.QueryBlockOffset(),
        vec![child],
    ))]
}

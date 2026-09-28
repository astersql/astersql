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

// 物理算子：索引回表阅读器（PhysicalIndexLookUpReader）。
//
// 作为 IndexLookUp 执行树的根：索引计划产出 handle，表计划按 handle 回表；
// 支持分页（Paging）、下推 Limit、公共句柄列与保序。可标记 IndexLookUpPushDown。

use base::{ContextRef, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::{Column, CorrelatedColumn};

use crate::{BasePhysicalPlan, PhysPlanPartInfo, PhysicalSchemaProducer, RootTask};

/// IndexLookUp 阅读器：索引/表两侧计划、分页、句柄列与期望行数。
pub struct PhysicalIndexLookUpReader {
    /// Schema 与基座计划；输出 Schema 通常跟随表侧。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 是否将 IndexLookUp 下推到存储侧执行。
    pub IndexLookUpPushDown: bool,
    /// 索引侧子计划。
    pub IndexPlan: Option<Box<dyn PhysicalPlan>>,
    /// 表侧（回表）子计划。
    pub TablePlan: Option<Box<dyn PhysicalPlan>>,
    /// 是否启用分页读取。
    pub Paging: bool,
    /// 额外句柄列（整数 handle 场景）。
    pub ExtraHandleCol: Option<Column>,
    /// 下推到存储的 Limit。
    pub PushedLimit: Option<crate::physical_plan_misc::PushedDownLimit>,
    /// 公共句柄（聚簇索引主键）列。
    pub CommonHandleCols: Vec<Column>,
    /// 分区计划信息。
    pub PlanPartInfo: Option<PhysPlanPartInfo>,
    /// 期望返回行数（用于代价/截断提示）。
    pub ExpectedCnt: u64,
    /// 是否保持索引顺序输出。
    pub KeepOrder: bool,
}

impl PhysicalIndexLookUpReader {
    /// 创建空的 IndexLookUpReader。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeIndexLookUp,
                0,
            )),
            IndexLookUpPushDown: false,
            IndexPlan: None,
            TablePlan: None,
            Paging: false,
            ExtraHandleCol: None,
            PushedLimit: None,
            CommonHandleCols: Vec::new(),
            PlanPartInfo: None,
            ExpectedCnt: 0,
            KeepOrder: false,
        }
    }

    /// 重设基座计划；若有表计划则用其 Schema。
    pub fn Init(mut self, ctx: ContextRef, offset: i32) -> Self {
        let plan = &mut self.PhysicalSchemaProducer.BasePhysicalPlan;
        plan.Plan.SetSCtx(ctx);
        plan.SetTP(plancodec::TypeIndexLookUp);
        plan.Plan.SetQueryBlockOffset(offset);
        if let Some(table_plan) = &self.TablePlan {
            self.PhysicalSchemaProducer
                .SetSchema(table_plan.schema().Clone());
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(table_plan.stats_info().clone());
        }
        self
    }

    /// 深拷贝两侧子计划并切换上下文。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut producer = PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(new_ctx.clone())?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
            IndexLookUpPushDown: self.IndexLookUpPushDown,
            IndexPlan: self
                .IndexPlan
                .as_ref()
                .map(|plan| plan.clone_physical(new_ctx.clone()))
                .transpose()?,
            TablePlan: self
                .TablePlan
                .as_ref()
                .map(|plan| plan.clone_physical(new_ctx))
                .transpose()?,
            Paging: self.Paging,
            ExtraHandleCol: self.ExtraHandleCol.as_ref().map(Column::Clone),
            PushedLimit: self.PushedLimit,
            CommonHandleCols: self.CommonHandleCols.iter().map(Column::Clone).collect(),
            PlanPartInfo: self.PlanPartInfo.as_ref().map(|info| *info.Clone()),
            ExpectedCnt: self.ExpectedCnt,
            KeepOrder: self.KeepOrder,
        })
    }

    /// 合并索引侧与表侧相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        let mut columns = self
            .TablePlan
            .as_ref()
            .map_or_else(Vec::new, |plan| plan.extract_correlated_cols());
        if let Some(plan) = &self.IndexPlan {
            columns.extend(plan.extract_correlated_cols());
        }
        columns
    }

    /// 估算索引侧网络数据量。
    pub fn GetIndexNetDataSize(&self) -> f64 {
        self.IndexPlan.as_ref().map_or(0.0, |plan| {
            plan.stats_count() * plan.schema().Len() as f64 * 8.0
        })
    }

    /// 估算表侧平均行宽。
    pub fn GetAvgTableRowSize(&self) -> f64 {
        self.TablePlan
            .as_ref()
            .map_or(0.0, |plan| plan.schema().Len() as f64 * 8.0)
    }

    /// 访问对象描述固定为 index lookup。
    pub fn AccessObject(&self, _ctx: &dyn base::PlanContext) -> String {
        "index lookup".to_owned()
    }

    /// EXPLAIN 仅报告内嵌 Limit；孩子关系由树形符号表达。
    pub fn ExplainInfo(&self) -> String {
        fn embedded_limit(plan: &dyn PhysicalPlan) -> Option<crate::PushedDownLimit> {
            plan.as_any()
                .downcast_ref::<crate::PhysicalLimit>()
                .map(|limit| crate::PushedDownLimit {
                    Offset: limit.Offset,
                    Count: limit.Count,
                })
                .or_else(|| {
                    (plan.tp(&[]) == "Limit").then(|| crate::PushedDownLimit {
                        Offset: 0,
                        Count: plan.stats_count().max(0.0) as u64,
                    })
                })
                .or_else(|| plan.children().into_iter().find_map(embedded_limit))
        }
        self.PushedLimit
            .or_else(|| self.IndexPlan.as_deref().and_then(embedded_limit))
            .map_or_else(String::new, |limit| {
                format!(
                    "limit embedded(offset:{}, count:{})",
                    limit.Offset, limit.Count
                )
            })
    }

    /// 归一化 EXPLAIN。
    pub fn ExplainNormalizedInfo(&self) -> String {
        String::new()
    }

    /// 旧版代价：两侧代价 + 索引网传 + 表行宽。
    pub fn GetCost(&self, index_cost: f64, table_cost: f64) -> f64 {
        index_cost + table_cost + self.GetIndexNetDataSize() + self.GetAvgTableRowSize()
    }

    /// 克隆自身并包装为 RootTask。
    pub fn BuildIndexLookUpTask(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        let plan = base::PhysicalPlan::clone_physical(
            self,
            self.PhysicalSchemaProducer.BasePhysicalPlan.s_ctx().clone(),
        )
        .expect("index lookup clone");
        Box::new(RootTask::New(plan, tasks.into_iter().next()))
    }

    /// 解析 Schema 生产器及两侧子计划列下标。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        if let Some(plan) = &mut self.IndexPlan {
            plan.resolve_indices()?;
        }
        if let Some(plan) = &mut self.TablePlan {
            plan.resolve_indices()?;
        }
        Ok(())
    }

    /// 加载表统计占位（当前为空实现）。
    pub fn LoadTableStats(&self) {}

    /// 计划代价 V1。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

    /// 计划代价 V2。
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

    /// 优先编码表侧，否则编码索引侧为 tipb Executor。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.TablePlan
            .as_ref()
            .or(self.IndexPlan.as_ref())
            .ok_or_else(|| expression::errors::New("index lookup has no child plan"))?
            .to_pb(ctx, store)
    }

    /// 估算阅读器、两侧计划与句柄列内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + (3 * std::mem::size_of::<bool>() + std::mem::size_of::<u64>()) as i64
            + self
                .IndexPlan
                .as_ref()
                .map_or(0, |plan| plan.memory_usage())
            + self
                .TablePlan
                .as_ref()
                .map_or(0, |plan| plan.memory_usage())
            + self
                .CommonHandleCols
                .iter()
                .map(Column::MemoryUsage)
                .sum::<i64>()
            + self.ExtraHandleCol.as_ref().map_or(0, Column::MemoryUsage)
            + self
                .PushedLimit
                .as_ref()
                .map_or(0, |limit| limit.MemoryUsage())
            + self
                .PlanPartInfo
                .as_ref()
                .map_or(0, PhysPlanPartInfo::MemoryUsage)
    }
}

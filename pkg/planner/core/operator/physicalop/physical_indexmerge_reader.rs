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

// IndexMerge 物理读算子：并行探测多条索引路径，再合并命中行（或经表回表取完整行）。
//
// IndexMerge（索引合并）允许优化器对同一张表使用多个索引范围扫描，再按
// intersection（交集）或 union（并集）合并 handle，从而覆盖单索引无法高效表达的谓词组合。
// MVIndex 表示多值索引访问路径。

use base::{ContextRef, PhysicalPlan, Plan};
use costusage::{CostVer2, PlanCostOption};
use expression::CorrelatedColumn;

use crate::{
    BasePhysicalPlan, FlattenListPushDownPlan, PhysPlanPartInfo, PhysicalIndexScan,
    PhysicalSchemaProducer, PhysicalTableScan,
};

/// 对应 Go `PhysicalIndexMergeReader`：持有部分索引计划、可选表计划与合并语义。
pub struct PhysicalIndexMergeReader {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 为 true 时按交集合并各 partial 路径的 handle，否则按并集。
    pub IsIntersectionType: bool,
    /// 是否走多值索引（Multi-Valued Index，一列可对应多个索引键）路径。
    pub AccessMVIndex: bool,
    /// 已下推到存储侧的 Limit（offset/count），用于提前截断。
    pub PushedLimit: Option<crate::physical_plan_misc::PushedDownLimit>,
    /// 保序或排序相关的 ByItems。
    pub ByItems: Vec<planner_util::ByItems>,
    /// 各条索引 partial 子计划（通常为 IndexScan / Selection 等）。
    pub PartialPlansRaw: Vec<Box<dyn PhysicalPlan>>,
    /// 可选的表侧回表计划；无表计划时直接输出索引侧结果。
    pub TablePlan: Option<Box<dyn PhysicalPlan>>,
    /// 分区剪枝信息（Partition pruning）：哪些分区会被访问。
    pub PlanPartInfo: Option<PhysPlanPartInfo>,
    /// 是否需要保持输出顺序。
    pub KeepOrder: bool,
}

impl PhysicalIndexMergeReader {
    /// 构造空壳算子，计划类型为 `TypeIndexMerge`。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeIndexMerge,
                0,
            )),
            IsIntersectionType: false,
            AccessMVIndex: false,
            PushedLimit: None,
            ByItems: Vec::new(),
            PartialPlansRaw: Vec::new(),
            TablePlan: None,
            PlanPartInfo: None,
            KeepOrder: false,
        }
    }

    /// 写入查询块偏移，并按 Go 语义从表侧或首条 partial 路径初始化统计与 Schema。
    pub fn Init(mut self, ctx: ContextRef, offset: i32) -> Self {
        let plan = &mut self.PhysicalSchemaProducer.BasePhysicalPlan;
        plan.Plan.SetSCtx(ctx.clone());
        plan.SetTP(plancodec::TypeIndexMerge);
        plan.Plan.SetQueryBlockOffset(offset);
        if let Some(table_plan) = self.TablePlan.as_deref() {
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(table_plan.stats_info().clone());
            self.PhysicalSchemaProducer
                .SetSchema(table_plan.schema().Clone());
        } else if let Some(first_partial) = self.PartialPlansRaw.first().map(Box::as_ref) {
            let total_row_count = self
                .PartialPlansRaw
                .iter()
                .map(|plan| plan.stats_count())
                .sum();
            let mut stats = first_partial
                .stats_info()
                .ScaleByExpectCnt(ctx.GetSessionVars(), total_row_count);
            stats.StatsVersion = first_partial.stats_info().StatsVersion;
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .set_stats(stats);

            let first_flattened = FlattenListPushDownPlan(first_partial)
                .into_iter()
                .next()
                .expect("a partial plan always flattens to at least its root");
            let schema = if let Some(index_scan) =
                first_flattened.as_any().downcast_ref::<PhysicalIndexScan>()
            {
                index_scan
                    .DataSourceSchema
                    .as_ref()
                    .unwrap_or_else(|| index_scan.schema())
                    .Clone()
            } else if first_flattened.as_any().is::<PhysicalTableScan>() {
                first_flattened.schema().Clone()
            } else {
                panic!("index merge partial plan must flatten to a table or index scan")
            };
            self.PhysicalSchemaProducer.SetSchema(schema);
        }
        self
    }

    /// 深克隆到新 PlanContext，递归克隆 partial 与表侧子计划。
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
            IsIntersectionType: self.IsIntersectionType,
            AccessMVIndex: self.AccessMVIndex,
            PushedLimit: self.PushedLimit,
            ByItems: self
                .ByItems
                .iter()
                .map(planner_util::ByItems::Clone)
                .collect(),
            PartialPlansRaw: self
                .PartialPlansRaw
                .iter()
                .map(|plan| plan.clone_physical(new_ctx.clone()))
                .collect::<Result<Vec<_>, _>>()?,
            TablePlan: self
                .TablePlan
                .as_ref()
                .map(|plan| plan.clone_physical(new_ctx))
                .transpose()?,
            PlanPartInfo: self.PlanPartInfo.as_ref().map(|info| *info.Clone()),
            KeepOrder: self.KeepOrder,
        })
    }

    /// 粗估表侧平均行宽（列数 × 8），用于代价模型网络传输估算。
    pub fn GetAvgTableRowSize(&self) -> f64 {
        self.TablePlan
            .as_ref()
            .map_or(0.0, |plan| plan.schema().Len() as f64 * 8.0)
    }

    /// 估算某条 partial 读路径的网络数据量：行数 × 列数 × 8。
    pub fn GetPartialReaderNetDataSize(&self, plan: &dyn PhysicalPlan) -> f64 {
        plan.stats_count() * plan.schema().Len() as f64 * 8.0
    }

    /// 返回访问对象描述：intersection / union 合并类型。
    pub fn AccessObject(&self, _ctx: &dyn base::PlanContext) -> String {
        format!(
            "index merge type:{}",
            if self.IsIntersectionType {
                "intersection"
            } else {
                "union"
            }
        )
    }

    /// EXPLAIN 详细信息：合并类型与可选的内嵌 Limit。
    pub fn ExplainInfo(&self) -> String {
        let mut result = format!(
            "type: {}",
            if self.IsIntersectionType {
                "intersection"
            } else {
                "union"
            }
        );
        if let Some(limit) = self.PushedLimit {
            result.push_str(&format!(
                ", limit embedded(offset:{}, count:{})",
                limit.Offset, limit.Count
            ));
        }
        result
    }

    /// Go 未为该算子增加 normalized 细节，沿用空信息。
    pub fn ExplainNormalizedInfo(&self) -> String {
        String::new()
    }

    /// 收集子计划中的关联列（Correlated Column，外层查询引用）。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        let mut columns = Vec::new();
        if let Some(plan) = &self.TablePlan {
            for child in FlattenListPushDownPlan(plan.as_ref()) {
                columns.extend(child.extract_correlated_cols());
            }
        }
        for plan in &self.PartialPlansRaw {
            columns.extend(plan.extract_correlated_cols());
            for child in FlattenListPushDownPlan(plan.as_ref()) {
                columns.extend(child.extract_correlated_cols());
            }
        }
        columns
    }

    /// 将表达式中的列引用解析为子 Schema 中的下标索引。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        for plan in &mut self.PartialPlansRaw {
            plan.resolve_indices()?;
        }
        if let Some(plan) = &mut self.TablePlan {
            plan.resolve_indices()?;
        }
        Ok(())
    }

    /// 加载表统计信息的占位钩子（迁移基线中尚未实现）。
    pub fn LoadTableStats(&self) {}

    /// 代价模型 v1：委托基类计算。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

    /// 代价模型 v2：委托基类计算 `CostVer2`。
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

    /// 序列化为 tipb Executor；优先表计划，否则取首个 partial。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.TablePlan
            .as_ref()
            .or_else(|| self.PartialPlansRaw.first())
            .ok_or_else(|| expression::errors::New("index merge has no child plan"))?
            .to_pb(ctx, store)
    }

    /// 递归估算本算子及子计划占用内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .PartialPlansRaw
                .iter()
                .map(|plan| {
                    plan.memory_usage()
                        + FlattenListPushDownPlan(plan.as_ref())
                            .into_iter()
                            .map(PhysicalPlan::memory_usage)
                            .sum::<i64>()
                })
                .sum::<i64>()
            + self.TablePlan.as_ref().map_or(0, |plan| {
                plan.memory_usage()
                    + FlattenListPushDownPlan(plan.as_ref())
                        .into_iter()
                        .map(PhysicalPlan::memory_usage)
                        .sum::<i64>()
            })
            + self
                .PlanPartInfo
                .as_ref()
                .map_or(0, PhysPlanPartInfo::MemoryUsage)
    }
}

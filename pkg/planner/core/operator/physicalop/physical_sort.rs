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

// 物理排序（Sort）算子：按 ORDER BY 项对孩子输出排序。
//
// 支持全量排序与部分排序（Partial Sort，可下推）；代价模型会考虑 OOM 落盘（spill）成本。

use base::{ContextRef, PhysicalPlan, Plan, Task};
use costusage::{CostVer2, PlanCostOption};
use expression::Expression as _;
use logicalop::LogicalPlan as _;

use crate::{BasePhysicalPlan, NominalSort, PhysicalSchemaProducer};

/// 物理 Sort：Schema 生产者 + 排序项列表 + 是否为部分排序。
pub struct PhysicalSort {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// ORDER BY 表达式与升/降序标记列表。
    pub ByItems: Vec<planner_util::ByItems>,
    /// 为 true 时表示部分排序，可下推到存储层（ToPB 要求此项）。
    pub IsPartialSort: bool,
}

impl PhysicalSort {
    /// 构造空排序项的 TypeSort 物理节点。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeSort,
                0,
            )),
            ByItems: Vec::new(),
            IsPartialSort: false,
        }
    }

    /// 绑定统计、偏移与孩子所需物理属性。
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
            .SetTP(plancodec::TypeSort);
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

    /// 克隆 Schema、ByItems 与 IsPartialSort 到新上下文。
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
            IsPartialSort: self.IsPartialSort,
        })
    }

    /// 从排序表达式中抽取相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<expression::CorrelatedColumn> {
        self.ByItems
            .iter()
            .flat_map(|item| expression::ExtractCorColumns(item.Expr.as_ref()))
            .map(expression::CorrelatedColumn::Clone)
            .collect()
    }

    /// 估算内存：生产者 + 各 ByItems + IsPartialSort。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + std::mem::size_of::<Vec<planner_util::ByItems>>() as i64
            + (self.ByItems.capacity() * std::mem::size_of::<*const planner_util::ByItems>()) as i64
            + self
                .ByItems
                .iter()
                .map(planner_util::ByItems::MemoryUsage)
                .sum::<i64>()
            + std::mem::size_of::<bool>() as i64
    }

    /// EXPLAIN：输出排序项文本，必要时附带 TiFlash Shuffle 流数。
    pub fn ExplainInfo(&self) -> String {
        let mut text = String::new();
        planner_util::ExplainByItems(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .s_ctx()
                .GetExprCtx()
                .GetEvalCtx(),
            &mut text,
            &self.ByItems,
        );
        let streams = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .TiFlashFineGrainedShuffleStreamCount;
        if streams > 0 {
            text.push_str(&format!(", stream_count: {streams}"));
        }
        text
    }

    /// 估算排序代价：CPU（n log n）+ 内存；超配额且允许临时存储时计入磁盘 spill。
    pub fn GetCost(&self, count: f64, _schema: &expression::Schema) -> f64 {
        let count = count.max(2.0);
        let vars = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .s_ctx()
            .GetSessionVars();
        let cpu_cost = count * count.log2() * vars.GetCPUFactor();
        let mut memory_cost = count * vars.GetMemoryFactor();
        let columns = _schema.Columns.iter().collect::<Vec<_>>();
        // 优先用直方图估算行宽，否则按列类型宽度求和。
        let row_size = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .stats_info()
            .HistColl
            .as_deref()
            .and_then(|histograms| histograms.downcast_ref::<statistics::HistColl>())
            .map(|histograms| cardinality::GetAvgRowSizeDataInDiskByRows(histograms, &columns))
            .unwrap_or_else(|| {
                columns
                    .iter()
                    .map(|column| chunk::EstimateTypeWidth(column.GetStaticType()) as f64)
                    .sum()
            })
            .max(0.0);
        let memory_quota = vars
            .StmtCtx
            .MemTracker
            .as_deref()
            .map_or(0, |tracker| tracker.GetBytesLimit());
        // OOM 时若开启临时存储，则部分内存成本转为磁盘成本。
        let spill = vardef::EnableTmpStorageOnOOM.Load()
            && memory_quota > 0
            && row_size * count > memory_quota as f64;
        let disk_cost = if spill {
            memory_cost *= memory_quota as f64 / (row_size * count);
            count * vars.GetDiskFactor() * row_size
        } else {
            0.0
        };
        cpu_cost + memory_cost + disk_cost
    }

    /// 解析基础计划后，将各排序表达式对齐到孩子 Schema。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        let Some(schema) = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .map(|p| p.schema().Clone())
        else {
            return Ok(());
        };
        for item in &mut self.ByItems {
            item.Expr = item.Expr.ResolveIndices(&schema)?;
        }
        Ok(())
    }

    /// 挂接到执行任务树。
    pub fn Attach2Task(&self, tasks: Vec<Box<dyn Task>>) -> Box<dyn Task> {
        base::PhysicalPlan::attach_to_task(self, tasks)
    }

    /// v1 代价模型转发。
    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

    /// v2 代价模型转发。
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

    /// 仅部分排序可下推：编码 tipb Sort，并设置细粒度 Shuffle 参数。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        if !self.IsPartialSort {
            return Err(expression::errors::New(
                "non-partial sort cannot be pushed down",
            ));
        }
        let client = ctx
            .GetClient()
            .ok_or_else(|| expression::errors::New("PB client is required"))?;
        let expression_context = ctx.GetExprCtx();
        let eval = expression_context.GetEvalCtx();
        let by_items = self
            .ByItems
            .iter()
            .map(|item| {
                expression::SortByItemToPB(eval, client.as_ref(), item.Expr.as_ref(), item.Desc)
                    .ok_or_else(|| expression::errors::New("sort expression cannot be pushed down"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let child = self
            .PhysicalSchemaProducer
            .BasePhysicalPlan
            .Children()
            .first()
            .ok_or_else(|| expression::errors::New("sort requires one child"))?
            .to_pb(ctx, store)?;
        let mut sort = tipb::Sort::new();
        sort.set_by_items(by_items.into());
        sort.set_is_partial_sort(true);
        sort.set_child(*child);
        let mut executor = tipb::Executor::new();
        executor.set_tp(tipb::ExecType::TypeSort);
        executor.set_sort(sort);
        executor.set_executor_id(self.explain_id(&[]).to_string());
        executor.set_fine_grained_shuffle_stream_count(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .TiFlashFineGrainedShuffleStreamCount,
        );
        executor.set_fine_grained_shuffle_batch_size(ctx.TiFlashFineGrainedShuffleBatchSize);
        Ok(Box::new(executor))
    }
}

/// 由逻辑 Sort 穷举物理计划：Root 下生成 Sort（及可选 NominalSort），MPP 下仅 NominalSort。
pub fn ExhaustPhysicalPlans4LogicalSort(
    logical: &logicalop::LogicalSort,
    required: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    match required.TaskTp {
        property::RootTaskType if MatchItems(required, &logical.ByItems) => {
            let mut child = property::PhysicalProperty::default();
            child.TaskTp = required.TaskTp;
            child.ExpectedCnt = f64::MAX;
            child.CTEProducerStatus = required.CTEProducerStatus;
            child.NoCopPushDown = required.NoCopPushDown;
            let stats = logical
                .StatsInfo()
                .map(|s| s.ScaleByExpectCnt(ctx.GetSessionVars(), required.ExpectedCnt))
                .unwrap_or_default();
            let mut sort = PhysicalSort::New(ctx.clone());
            sort.ByItems = logical
                .ByItems
                .iter()
                .map(planner_util::ByItems::Clone)
                .collect();
            sort.PhysicalSchemaProducer
                .SetSchema(logical.Schema().Clone());
            let mut plans = vec![Box::new(sort.Init(
                ctx.clone(),
                stats.clone(),
                logical.QueryBlockOffset(),
                vec![Box::new(child)],
            )) as Box<dyn PhysicalPlan>];
            if let Some(nominal) = NominalSort::FromLogical(logical, required, false) {
                plans.push(Box::new(nominal));
            }
            plans
        }
        property::MppTaskType => NominalSort::FromLogical(logical, required, true)
            .map(|plan| vec![Box::new(plan) as Box<dyn PhysicalPlan>])
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// 从 ORDER BY 项推导物理排序属性；标量函数须可抽出单列，否则返回失败。
/// 第二个返回值表示是否“仅列引用、不含标量函数”。
pub fn GetPropByOrderByItemsContainScalarFunc(
    items: &[planner_util::ByItems],
) -> (Option<property::PhysicalProperty>, bool) {
    let mut property = property::PhysicalProperty::default();
    let mut only_columns = true;
    for item in items {
        if let Some(column) = item.Expr.as_any().downcast_ref::<expression::Column>() {
            property.SortItems.push(property::SortItem {
                Col: column.Clone(),
                Desc: item.Desc,
            });
        } else if let Some(function) = item
            .Expr
            .as_any()
            .downcast_ref::<expression::ScalarFunction>()
        {
            let (column, desc) = function.GetSingleColumn(item.Desc);
            let Some(column) = column else {
                return (None, false);
            };
            property.SortItems.push(property::SortItem {
                Col: column.Clone(),
                Desc: desc,
            });
            only_columns = false;
        } else {
            return (None, false);
        }
    }
    (Some(property), only_columns)
}

/// 判断请求的排序前缀是否与 ByItems 前缀匹配（方向与列相等）。
pub fn MatchItems(required: &property::PhysicalProperty, items: &[planner_util::ByItems]) -> bool {
    required.SortItems.len() <= items.len()
        && required
            .SortItems
            .iter()
            .zip(items)
            .all(|(expected, actual)| {
                expected.Desc == actual.Desc
                    && actual
                        .Expr
                        .as_any()
                        .downcast_ref::<expression::Column>()
                        .is_some_and(|column| expected.Col.EqualColumn(column))
            })
}

/// 仅当 ORDER BY 全为列引用时返回物理排序属性。
pub fn GetPropByOrderByItems(
    items: &[planner_util::ByItems],
) -> Option<property::PhysicalProperty> {
    let (property, only_columns) = GetPropByOrderByItemsContainScalarFunc(items);
    only_columns.then_some(property).flatten()
}

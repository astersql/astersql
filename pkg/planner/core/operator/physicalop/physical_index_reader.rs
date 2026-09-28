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

// 物理算子：索引阅读器（PhysicalIndexReader）。
//
// 作为 coprocessor / 下推树的根，只读索引侧计划并返回索引列；
// 不回表（与 IndexLookUp 相对）。输出 Schema 通常来自内嵌 IndexPlan。

use base::{ContextRef, PhysicalPlan};
use costusage::{CostVer2, PlanCostOption};
use expression::{Column, CorrelatedColumn, Schema};

use crate::{BasePhysicalPlan, PhysPlanPartInfo, PhysicalIndexScan, PhysicalSchemaProducer};

/// 索引阅读器：包装 IndexPlan，向外暴露输出列与分区信息。
pub struct PhysicalIndexReader {
    /// Schema 与基座物理计划生产器。
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 下推的索引侧子计划树。
    pub IndexPlan: Option<Box<dyn PhysicalPlan>>,
    /// 对外输出列（通常等于 IndexPlan Schema）。
    pub OutputColumns: Vec<Column>,
    /// 分区裁剪/路由相关信息。
    pub PlanPartInfo: Option<PhysPlanPartInfo>,
}

impl PhysicalIndexReader {
    /// 创建空的 IndexReader。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeIndexReader,
                0,
            )),
            IndexPlan: None,
            OutputColumns: Vec::new(),
            PlanPartInfo: None,
        }
    }

    /// 按查询块偏移重设基座计划 ID/类型。
    pub fn Init(mut self, ctx: ContextRef, offset: i32) -> Self {
        let plan = &mut self.PhysicalSchemaProducer.BasePhysicalPlan;
        plan.Plan.SetSCtx(ctx);
        plan.SetTP(plancodec::TypeIndexReader);
        plan.Plan.SetQueryBlockOffset(offset);
        self
    }

    /// 深拷贝并切换上下文。
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
            IndexPlan: self
                .IndexPlan
                .as_ref()
                .map(|plan| plan.clone_physical(new_ctx))
                .transpose()?,
            OutputColumns: self.OutputColumns.iter().map(Column::Clone).collect(),
            PlanPartInfo: self.PlanPartInfo.as_ref().map(|info| *info.Clone()),
        })
    }

    /// 与 Go 一致：聚合/投影输出根计划 Schema，其余索引链输出扫描前数据源 Schema。
    pub fn SetSchema(&mut self, _schema: Option<Schema>) {
        let schema = self.IndexPlan.as_deref().and_then(|plan| {
            if plan.as_any().is::<crate::PhysicalHashAgg>()
                || plan.as_any().is::<crate::PhysicalStreamAgg>()
                || plan.as_any().is::<crate::PhysicalProjection>()
            {
                return Some(plan.schema().Clone());
            }
            find_index_scan(Some(plan))
                .and_then(|scan| scan.DataSourceSchema.as_ref())
                .map(Schema::Clone)
        });
        if let Some(schema) = schema {
            self.OutputColumns = schema.Columns.iter().map(Column::Clone).collect();
            self.PhysicalSchemaProducer.SetSchema(schema);
        }
    }

    /// 第一个孩子作为 IndexPlan，其余交给基座；并刷新 Schema。
    pub fn SetChildren(&mut self, mut children: Vec<Box<dyn PhysicalPlan>>) {
        self.IndexPlan = children.drain(..).next();
        self.SetSchema(None);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildren(Vec::new());
    }

    /// 从 IndexPlan 抽取相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        let mut columns = Vec::new();
        collect_correlated_columns(self.IndexPlan.as_deref(), &mut columns);
        columns
    }

    /// 访问对象描述：递归定位 IndexScan 后委托其 AccessObject。
    pub fn AccessObject(&self, _ctx: &dyn base::PlanContext) -> String {
        find_index_scan(self.IndexPlan.as_deref())
            .map(PhysicalIndexScan::AccessObject)
            .unwrap_or_default()
    }

    /// EXPLAIN：index:<explain_id>。
    pub fn ExplainInfo(&self) -> String {
        self.IndexPlan.as_ref().map_or_else(String::new, |plan| {
            format!("index:{}", plan.explain_id(&[]))
        })
    }

    /// 归一化 EXPLAIN：index:<tp>。
    pub fn ExplainNormalizedInfo(&self) -> String {
        self.IndexPlan
            .as_ref()
            .map_or_else(String::new, |plan| format!("index:{}", plan.tp(&[])))
    }

    /// 估算网络传输数据量：行数 × 列数 × 8。
    pub fn GetNetDataSize(&self) -> f64 {
        self.IndexPlan.as_ref().map_or(0.0, |plan| {
            plan.stats_count() * plan.schema().Len() as f64 * 8.0
        })
    }

    /// 触发定位 IndexScan 以加载表统计（当前仅探测，结果丢弃）。
    pub fn LoadTableStats(&self) {
        let _ = find_index_scan(self.IndexPlan.as_deref());
    }

    /// 解析子计划与 OutputColumns 下标。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        let Some(plan) = self.IndexPlan.as_mut() else {
            return Ok(());
        };
        plan.resolve_indices()?;
        let schema = plan.schema().Clone();
        for column in &mut self.OutputColumns {
            match column.ResolveIndices(&schema) {
                Ok(resolved) => *column = resolved,
                Err(error) => {
                    let (resolved, ok) = column.ResolveIndicesByVirtualExpr(
                        self.PhysicalSchemaProducer
                            .BasePhysicalPlan
                            .Plan
                            .SCtx()
                            .GetExprCtx()
                            .GetEvalCtx(),
                        &schema,
                    );
                    if !ok {
                        return Err(error);
                    }
                    *column = resolved;
                }
            }
        }
        Ok(())
    }

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

    /// 直接委托 IndexPlan 编码 protobuf。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.IndexPlan
            .as_ref()
            .ok_or_else(|| expression::errors::New("index reader has no index plan"))?
            .to_pb(ctx, store)
    }

    /// 估算阅读器及子计划内存。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .IndexPlan
                .as_ref()
                .map_or(0, |plan| subtree_memory_usage(plan.as_ref()))
            + self
                .OutputColumns
                .iter()
                .map(Column::MemoryUsage)
                .sum::<i64>()
            + self
                .PlanPartInfo
                .as_ref()
                .map_or(0, PhysPlanPartInfo::MemoryUsage)
    }
}

/// 在计划树中深度优先查找第一个 PhysicalIndexScan。
fn find_index_scan(plan: Option<&dyn PhysicalPlan>) -> Option<&PhysicalIndexScan> {
    let plan = plan?;
    if let Some(scan) = plan.as_any().downcast_ref::<PhysicalIndexScan>() {
        return Some(scan);
    }
    plan.children()
        .into_iter()
        .find_map(|child| find_index_scan(Some(child)))
}

/// Go 的 IndexPlans 是叶到根的扁平列表；Rust 保持树所有权并递归得到同一语义。
fn collect_correlated_columns(plan: Option<&dyn PhysicalPlan>, output: &mut Vec<CorrelatedColumn>) {
    let Some(plan) = plan else { return };
    output.extend(plan.extract_correlated_cols());
    for child in plan.children() {
        collect_correlated_columns(Some(child), output);
    }
}

/// 汇总下推树每个节点自身的内存，与 Go 遍历 IndexPlans 等价。
fn subtree_memory_usage(plan: &dyn PhysicalPlan) -> i64 {
    plan.memory_usage()
        + plan
            .children()
            .into_iter()
            .map(subtree_memory_usage)
            .sum::<i64>()
}

/// 按 Schema/统计/所需属性构造已初始化的 IndexReader。
pub fn GetPhysicalIndexReader(
    ctx: ContextRef,
    schema: Schema,
    stats: property::StatsInfo,
    props: Vec<Box<property::PhysicalProperty>>,
) -> PhysicalIndexReader {
    let mut reader = PhysicalIndexReader::New(ctx);
    reader.PhysicalSchemaProducer.SetSchema(schema);
    reader
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .set_stats(stats);
    reader
        .PhysicalSchemaProducer
        .BasePhysicalPlan
        .SetChildrenReqProps(props);
    reader
}

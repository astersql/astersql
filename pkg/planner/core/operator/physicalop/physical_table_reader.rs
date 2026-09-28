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

// 物理 TableReader：封装下推到存储侧的表计划，作为 TiDB 与 TiKV/TiFlash 之间的读取边界。
//
// 按存储类型选择 Cop / BatchCop / MPP 读请求；内部 `TablePlan` 通常含 TableScan 及可下推算子。

use base::{ContextRef, PhysicalPlan};
use costusage::{CostVer2, PlanCostOption};
use expression::{CorrelatedColumn, Schema};

use crate::{BasePhysicalPlan, PhysPlanPartInfo, PhysicalSchemaProducer, PhysicalTableScan};

/// 读请求类型：决定走 Coprocessor、批量 Cop 还是 MPP。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ReadReqType {
    #[default]
    /// 单 Region Coprocessor 请求（TiKV 默认）。
    Cop,
    /// 批量 Coprocessor（TiFlash 常用）。
    BatchCop,
    /// MPP（Massively Parallel Processing）分布式读。
    MPP,
}

impl ReadReqType {
    /// 返回 EXPLAIN/诊断用的短名称字符串。
    pub fn Name(self) -> &'static str {
        match self {
            Self::Cop => "cop",
            Self::BatchCop => "batchCop",
            Self::MPP => "mpp",
        }
    }
}

/// 物理 TableReader：持有下推子计划、存储类型、读请求类型与分区信息。
pub struct PhysicalTableReader {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 下推到存储引擎执行的物理子计划树。
    pub TablePlan: Option<Box<dyn PhysicalPlan>>,
    /// 目标存储：TiKV 或 TiFlash 等。
    pub StoreType: kv::StoreType,
    /// 当前选用的读请求类型。
    pub ReadReqType: ReadReqType,
    /// 是否使用公共句柄（clustered index / common handle）。
    pub IsCommonHandle: bool,
    /// 动态裁剪等场景下的物理分区计划信息。
    pub PlanPartInfo: Option<PhysPlanPartInfo>,
}

impl PhysicalTableReader {
    /// 构造默认 TiKV + Cop 的空 TableReader。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeTableReader,
                0,
            )),
            TablePlan: None,
            StoreType: kv::StoreType::TiKV,
            ReadReqType: ReadReqType::Cop,
            IsCommonHandle: false,
            PlanPartInfo: None,
        }
    }

    /// 绑定偏移并按 StoreType 调整读请求类型。
    pub fn Init(mut self, ctx: ContextRef, offset: i32) -> Self {
        let plan = &mut self.PhysicalSchemaProducer.BasePhysicalPlan;
        plan.Plan.SetSCtx(ctx);
        plan.SetTP(plancodec::TypeTableReader);
        plan.Plan.SetQueryBlockOffset(offset);
        self.ReadReqType = ReadReqType::Cop;
        if let Some(table_plan) = &self.TablePlan {
            self.PhysicalSchemaProducer
                .SetSchema(table_plan.schema().Clone());
            self.adjust_read_request_type();
        }
        self
    }

    /// TiFlash ExchangeSender 使用 MPP；其余请求先保持 Cop。
    fn adjust_read_request_type(&mut self) {
        self.ReadReqType = if self.StoreType != kv::StoreType::TiFlash {
            ReadReqType::Cop
        } else if self
            .TablePlan
            .as_deref()
            .is_some_and(|plan| plan.as_any().is::<crate::PhysicalExchangeSender>())
        {
            ReadReqType::MPP
        } else {
            ReadReqType::Cop
        };
    }

    /// 深克隆 Schema、TablePlan、存储与分区信息到新上下文。
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
            TablePlan: self
                .TablePlan
                .as_ref()
                .map(|plan| plan.clone_physical(new_ctx))
                .transpose()?,
            StoreType: self.StoreType,
            ReadReqType: self.ReadReqType,
            IsCommonHandle: self.IsCommonHandle,
            PlanPartInfo: self.PlanPartInfo.as_ref().map(|info| *info.Clone()),
        })
    }

    /// 测试辅助：仅设置 TablePlan，与 Go 一样不触发初始化副作用。
    pub fn SetTablePlanForTest(&mut self, plan: Box<dyn PhysicalPlan>) {
        self.TablePlan = Some(plan);
    }

    /// 返回内部表计划引用。
    pub fn GetTablePlan(&self) -> Option<&dyn PhysicalPlan> {
        self.TablePlan.as_deref()
    }

    /// 递归收集 TablePlan 子树中所有 PhysicalTableScan。
    pub fn GetTableScans(&self) -> Vec<&PhysicalTableScan> {
        let mut scans = Vec::new();
        collect_table_scans(self.TablePlan.as_deref(), &mut scans);
        scans
    }

    /// 要求恰好一个 TableScan，否则报错。
    pub fn GetTableScan(&self) -> Result<&PhysicalTableScan, expression::Error> {
        let scans = self.GetTableScans();
        if scans.len() != 1 {
            return Err(expression::errors::New("the count of table scan != 1"));
        }
        Ok(scans[0])
    }

    /// 粗估平均行宽：列数 × 8 字节。
    pub fn GetAvgRowSize(&self) -> f64 {
        self.TablePlan
            .as_ref()
            .map_or(0.0, |plan| plan.schema().Len() as f64 * 8.0)
    }

    /// 估算经网络传输的数据量：行数 × 平均行宽。
    pub fn GetNetDataSize(&self) -> f64 {
        self.TablePlan
            .as_ref()
            .map_or(0.0, |plan| plan.stats_count() * self.GetAvgRowSize())
    }

    /// 汇总各 TableScan 的访问对象描述（库表等）。
    pub fn AccessObject(&self, _ctx: &dyn base::PlanContext) -> String {
        self.GetTableScans()
            .iter()
            .map(|scan| scan.AccessObject())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// EXPLAIN：展示内部 data 计划 ID；MPP 时附加版本前缀。
    pub fn ExplainInfo(&self) -> String {
        self.TablePlan.as_ref().map_or_else(String::new, |plan| {
            let explain_id = plan
                .as_any()
                .downcast_ref::<crate::PhysicalTableScan>()
                .map_or_else(
                    || plan.explain_id(&[]).to_string(),
                    crate::PhysicalTableScan::ExplainID,
                );
            let data = format!("data:{explain_id}");
            if self.ReadReqType == ReadReqType::MPP {
                format!(
                    "MppVersion: {}, {data}",
                    kv::GetNewestMppVersion().ToInt64()
                )
            } else {
                data
            }
        })
    }

    /// 归一化 EXPLAIN 当前为空（与 Go 对齐占位）。
    pub fn ExplainNormalizedInfo(&self) -> String {
        String::new()
    }

    /// 算子信息只展示 data 计划，不附带 MPP 版本前缀。
    pub fn OperatorInfo(&self, _verbose: bool) -> String {
        self.TablePlan.as_ref().map_or_else(String::new, |plan| {
            let explain_id = plan
                .as_any()
                .downcast_ref::<crate::PhysicalTableScan>()
                .map_or_else(
                    || plan.explain_id(&[]).to_string(),
                    crate::PhysicalTableScan::ExplainID,
                );
            format!("data:{explain_id}")
        })
    }

    /// 返回读请求类型名称。
    pub fn Name(&self) -> &'static str {
        self.ReadReqType.Name()
    }

    /// 将首个孩子设为 TablePlan，并同步输出 Schema。
    pub fn SetChildren(&mut self, mut children: Vec<Box<dyn PhysicalPlan>>) {
        self.TablePlan = children.drain(..).next();
        if let Some(plan) = &self.TablePlan {
            self.PhysicalSchemaProducer.SetSchema(plan.schema().Clone());
            if self.StoreType == kv::StoreType::TiFlash && self.ReadReqType != ReadReqType::MPP {
                if plan.as_any().is::<crate::PhysicalExchangeSender>() {
                    self.ReadReqType = ReadReqType::MPP;
                    return;
                }
                fn contains_batch_cop_operator(plan: &dyn PhysicalPlan) -> bool {
                    plan.as_any().is::<crate::PhysicalHashAgg>()
                        || plan.as_any().is::<crate::PhysicalStreamAgg>()
                        || plan.as_any().is::<crate::PhysicalTopN>()
                        || plan.children().into_iter().any(contains_batch_cop_operator)
                }

                fn contains_ordered_scan(plan: &dyn PhysicalPlan) -> bool {
                    plan.as_any()
                        .downcast_ref::<crate::PhysicalTableScan>()
                        .is_some_and(|scan| scan.KeepOrder)
                        || plan.children().into_iter().any(contains_ordered_scan)
                }

                self.ReadReqType = if contains_batch_cop_operator(plan.as_ref())
                    && !contains_ordered_scan(plan.as_ref())
                {
                    ReadReqType::BatchCop
                } else {
                    ReadReqType::Cop
                };
            }
        }
    }

    /// 从内部表计划抽取相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.TablePlan
            .as_ref()
            .map_or_else(Vec::new, |plan| plan.extract_correlated_cols())
    }

    /// 解析自身与 TablePlan 的列下标。
    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()?;
        if let Some(plan) = &mut self.TablePlan {
            plan.resolve_indices()?;
        }
        Ok(())
    }

    /// 触发收集 TableScan 以加载表统计（副作用入口）。
    pub fn LoadTableStats(&self) {
        let _ = self.GetTableScans();
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

    /// 将内部 TablePlan 序列化为 tipb Executor。
    pub fn ToPB(
        &self,
        ctx: &mut base::BuildPBContext,
        store: kv::StoreType,
    ) -> Result<Box<tipb::Executor>, expression::Error> {
        self.TablePlan
            .as_ref()
            .ok_or_else(|| expression::errors::New("table reader has no table plan"))?
            .to_pb(ctx, store)
    }

    /// 估算内存：生产者 + TablePlan + PlanPartInfo。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self
                .TablePlan
                .as_ref()
                .map_or(0, |plan| plan.memory_usage())
            + self
                .PlanPartInfo
                .as_ref()
                .map_or(0, PhysPlanPartInfo::MemoryUsage)
    }
}

/// 深度优先遍历，收集所有 PhysicalTableScan 节点。
fn collect_table_scans<'a>(
    plan: Option<&'a dyn PhysicalPlan>,
    output: &mut Vec<&'a PhysicalTableScan>,
) {
    let Some(plan) = plan else { return };
    if let Some(scan) = plan.as_any().downcast_ref::<PhysicalTableScan>() {
        output.push(scan);
    }
    for child in plan.children() {
        collect_table_scans(Some(child), output);
    }
}

/// 工厂：按给定 Schema、统计与孩子属性构造已初始化的 TableReader。
pub fn GetPhysicalTableReader(
    ctx: ContextRef,
    schema: Schema,
    stats: property::StatsInfo,
    props: Vec<Box<property::PhysicalProperty>>,
) -> PhysicalTableReader {
    let mut reader = PhysicalTableReader::New(ctx);
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

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

// CTE（公用表表达式）物理计划：种子/递归子计划、storage/sink/source 形态，
// 以及选优枚举入口；CTE 结果写入临时存储后供多次引用读取。

// 任务选择和 protobuf 构造保留原控制流。
//
/// PhysicalCTE 保存 CTE 的种子/递归计划、逻辑共享状态及原名/别名。
// pub struct PhysicalCTE {
//     pub physical_schema_producer: PhysicalSchemaProducer,
//     pub seed_plan: Option<Box<dyn base::PhysicalPlan>>,
//     pub recur_plan: Option<Box<dyn base::PhysicalPlan>>,
//     pub cte: Box<logicalop::CTEClass>,
//     pub cte_as_name: ast::CIStr,
//     pub cte_name: ast::CIStr,
// }
//
/// LogicalCTE 枚举物理计划时构造 storage 形态，并只把必要的子属性克隆给它。
// pub fn exhaust_physical_plans4_logical_cte(
//     p: &logicalop::LogicalCTE,
//     prop: &property::PhysicalProperty,
// ) -> Result<(Vec<Box<dyn base::PhysicalPlan>>, bool), Error> {
//     let mut pcte = PhysicalCTE::new(p.Cte.clone()).init(p.SCtx(), p.StatsInfo().clone());
//     pcte.SetSchema(p.Schema().clone());
//     pcte.SetChildrenReqProps(vec![prop.CloneEssentialFields()]);
//     Ok((vec![Box::new(PhysicalCTEStorage(pcte))], true))
// }
//
// impl PhysicalCTE {
//     pub fn new(cte: Box<logicalop::CTEClass>) -> Self { Self { cte, ..Default::default() } }
//
/// Init 只安装类型、上下文和统计信息，种子与递归子计划已在统计推导阶段生成。
//     pub fn init(mut self, ctx: base::PlanContext, stats: Box<property::StatsInfo>) -> Self {
//         self.physical_schema_producer.base_physical_plan =
//             NewBasePhysicalPlan(ctx, plancodec::TypeCTE, &self, 0);
//         self.SetStats(stats);
//         self
//     }
//
/// 合并种子计划与可选递归计划中的关联列。
//     pub fn extract_correlated_cols(&self) -> Vec<Box<expression::CorrelatedColumn>> {
//         let mut cols = self.seed_plan.as_ref().map_or_else(Vec::new, |p| coreusage::ExtractCorrelatedCols4PhysicalPlan(p));
//         if let Some(plan) = &self.recur_plan { cols.extend(coreusage::ExtractCorrelatedCols4PhysicalPlan(plan)); }
//         cols
//     }
//
//     pub fn operator_info(&self, _normalized: bool) -> String {
//         format!("data:{}", CTEDefinition::from(self).explain_id())
//     }
//
//     pub fn explain_info(&self) -> String {
//         format!("{}, {}", self.access_object().String(), self.operator_info(false))
//     }
//
/// 会话要求忽略 ExplainID 后缀时只返回类型，否则使用 Type_ID。
//     pub fn explain_id(&self) -> String {
//         if self.SCtx().is_some() && self.SCtx().unwrap().GetSessionVars().StmtCtx.IgnoreExplainIDSuffix {
//             self.TP().to_owned()
//         } else { format!("{}_{}", self.TP(), self.ID()) }
//     }
//
/// 深克隆 schema producer、种子与递归计划；CTEClass 与原 Go 一样共享。
//     pub fn clone_plan(&self, new_ctx: base::PlanContext) -> Result<Box<dyn base::PhysicalPlan>, Error> {
//         let mut cloned = PhysicalCTE::new(self.cte.clone());
//         cloned.SetSCtx(new_ctx.clone());
//         cloned.physical_schema_producer = self.physical_schema_producer.CloneWithSelf(new_ctx.clone(), &cloned)?;
//         if let Some(plan) = &self.seed_plan { cloned.seed_plan = Some(plan.Clone(new_ctx.clone())?); }
//         if let Some(plan) = &self.recur_plan { cloned.recur_plan = Some(plan.Clone(new_ctx)?); }
//         cloned.cte_as_name = self.cte_as_name.clone();
//         cloned.cte_name = self.cte_name.clone();
//         Ok(Box::new(cloned))
//     }
//
//     pub fn memory_usage(&self) -> i64 {
//         self.physical_schema_producer.MemoryUsage() + self.cte_as_name.MemoryUsage()
//             + self.seed_plan.as_ref().map_or(0, |p| p.MemoryUsage())
//             + self.recur_plan.as_ref().map_or(0, |p| p.MemoryUsage())
//             + self.cte.MemoryUsage()
//     }
//
//     pub fn get_plan_cost_ver2(&self, task: property::TaskType, option: &costusage::PlanCostOption, _force: &[bool]) -> Result<costusage::CostVer2, Error> {
//         utilfuncp::GetPlanCostVer24PhysicalCTE(self, task, option)
//     }
//
/// EXPLAIN 访问对象仅显示 CTE 名；别名不同则输出 `CTE:name AS alias`。
//     pub fn access_object(&self) -> access::OtherAccessObject {
//         if self.cte_name == self.cte_as_name {
//             access::OtherAccessObject(format!("CTE:{}", self.cte_name.L))
//         } else { access::OtherAccessObject(format!("CTE:{} AS {}", self.cte_name.L, self.cte_as_name.L)) }
//     }
// }
// */

// --- 可运行的 CTE 兼容数据模型 ---
use crate::physical_common_plans::{
    PhysicalExpr, PhysicalKind, PhysicalPlanNode, PhysicalProperty, Stats,
};
use crate::{BasePhysicalPlan, PhysicalSchemaProducer};
use base::ContextRef;
use costusage::{CostVer2, PlanCostOption};
use expression::CorrelatedColumn;
use property::StatsInfo;
use std::sync::Arc;

/// Shared, typed CTE producer. Its seed and recursive plans are independent
/// roots and are never children of a consumer in the main plan tree.
pub struct PhysicalCTEDefinition {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    pub IDForStorage: i64,
    pub SeedPlan: Box<dyn base::PhysicalPlan>,
    pub RecurPlan: Option<Box<dyn base::PhysicalPlan>>,
}

impl PhysicalCTEDefinition {
    pub fn New(
        ctx: ContextRef,
        storage_id: i64,
        seed: Box<dyn base::PhysicalPlan>,
        recursive: Option<Box<dyn base::PhysicalPlan>>,
    ) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeCTEDefinition,
                0,
            )),
            IDForStorage: storage_id,
            SeedPlan: seed,
            RecurPlan: recursive,
        }
    }

    pub fn Clone(&self, ctx: ContextRef) -> Result<Self, expression::Error> {
        Ok(Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(
                self.PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .CloneWithNewCtx(ctx.clone())?,
            ),
            IDForStorage: self.IDForStorage,
            SeedPlan: self.SeedPlan.clone_physical(ctx.clone())?,
            RecurPlan: self
                .RecurPlan
                .as_ref()
                .map(|p| p.clone_physical(ctx.clone()))
                .transpose()?,
        })
    }
}

/// A main-tree reference to one shared CTE definition.
pub struct PhysicalCTE {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    pub CTE: Arc<PhysicalCTEDefinition>,
}

impl PhysicalCTE {
    pub fn New(ctx: ContextRef, definition: Arc<PhysicalCTEDefinition>) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeCTE,
                0,
            )),
            CTE: definition,
        }
    }

    pub fn Clone(&self, ctx: ContextRef) -> Result<Self, expression::Error> {
        Ok(Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(
                self.PhysicalSchemaProducer
                    .BasePhysicalPlan
                    .CloneWithNewCtx(ctx)?,
            ),
            CTE: Arc::clone(&self.CTE),
        })
    }
}

/// Runtime physical CTE scan used by the canonical optimizer route.
///
/// The logical CTE class owns the seed plan and is shared by all references;
/// this leaf keeps the physical optimizer honest about the consumer side while
/// the EXPLAIN runtime renders the shared seed producer separately.
pub struct PhysicalCteScan {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    pub ExplainText: String,
}

impl PhysicalCteScan {
    pub fn New(ctx: ContextRef, tp: &str, explain: String, schema: expression::Schema) -> Self {
        let mut producer = PhysicalSchemaProducer::New(BasePhysicalPlan::New(ctx, tp, 0));
        producer.SetSchema(schema);
        Self {
            PhysicalSchemaProducer: producer,
            ExplainText: explain,
        }
    }

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
            ExplainText: self.ExplainText.clone(),
        })
    }

    pub fn ExplainInfo(&self) -> String {
        self.ExplainText.clone()
    }

    pub fn ResolveIndices(&mut self) -> Result<(), expression::Error> {
        self.PhysicalSchemaProducer.ResolveIndices()
    }

    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage() + self.ExplainText.len() as i64
    }

    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        Vec::new()
    }

    pub fn GetPlanCostVer1(
        &mut self,
        task: property::TaskType,
        option: &PlanCostOption,
    ) -> Result<f64, expression::Error> {
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .GetPlanCostVer1(task, option)
    }

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
}

#[derive(Clone, Debug, PartialEq)]
/// CTE 生产者侧物理节点：保存种子/递归计划、去重标记与分段统计。
pub struct PhysicalCte {
    /// 关联临时存储编号。
    pub id_for_storage: i64,
    /// 非递归种子计划。
    pub seed_plan: Option<PhysicalPlanNode>,
    /// 可选递归计划。
    pub recursive_plan: Option<PhysicalPlanNode>,
    /// 递归联合是否去重（UNION vs UNION ALL）。
    pub distinct: bool,
    /// 种子段统计。
    pub seed_statistics: Stats,
    /// 递归段统计。
    pub recursive_statistics: Stats,
    /// 最终结果统计。
    pub result_statistics: Stats,
}
impl PhysicalCte {
    /// 合并种子与递归子树中的相关列 ID。
    pub fn correlated_columns(&self) -> Vec<i64> {
        self.seed_plan
            .iter()
            .chain(&self.recursive_plan)
            .flat_map(|plan| collect_correlated(plan))
            .collect()
    }
    /// EXPLAIN：存储 id、是否递归、是否 distinct。
    pub fn explain_info(&self) -> String {
        format!(
            "cte:{}, recursive:{}, distinct:{}",
            self.id_for_storage,
            self.recursive_plan.is_some(),
            self.distinct
        )
    }
    /// 结构体 + 子计划内存。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self
                .seed_plan
                .as_ref()
                .map_or(0, PhysicalPlanNode::memory_usage)
            + self
                .recursive_plan
                .as_ref()
                .map_or(0, PhysicalPlanNode::memory_usage)
    }
    /// 代价占位（当前返回 0）。
    pub fn plan_cost_v2(&self) -> f64 {
        0.0
    }
    /// 访问对象仅显示 CTE 存储 id。
    pub fn access_object(&self) -> String {
        format!("CTE:{}", self.id_for_storage)
    }
}
/// 递归收集 Selection/Projection 中的 CorrelatedColumn。
fn collect_correlated(plan: &PhysicalPlanNode) -> Vec<i64> {
    let own = match &plan.kind {
        PhysicalKind::Selection { predicates }
        | PhysicalKind::Projection {
            expressions: predicates,
        } => predicates
            .iter()
            .flat_map(collect_correlated_expr)
            .collect(),
        _ => Vec::new(),
    };
    plan.children
        .iter()
        .flat_map(collect_correlated)
        .chain(own)
        .collect()
}

fn collect_correlated_expr(expr: &PhysicalExpr) -> Vec<i64> {
    match expr {
        PhysicalExpr::CorrelatedColumn(id) => vec![*id],
        PhysicalExpr::Scalar { args, .. } => {
            args.iter().flat_map(collect_correlated_expr).collect()
        }
        _ => Vec::new(),
    }
}

#[derive(Clone, Debug, PartialEq)]
/// EXPLAIN 专用 CTE 定义视图。
pub struct CteDefinition(pub PhysicalCte);
impl CteDefinition {
    /// 展示定义是否递归。
    pub fn explain_info(&self) -> String {
        if self.0.recursive_plan.is_some() {
            "Recursive CTE".into()
        } else {
            "Non-Recursive CTE".into()
        }
    }
    /// 以 CTE_<id> 形式展示定义 ID。
    pub fn explain_id(&self) -> String {
        format!("CTE_{}", self.0.id_for_storage)
    }
    /// 委托内嵌 PhysicalCte。
    pub fn memory_usage(&self) -> i64 {
        self.0.memory_usage()
    }
}
#[derive(Clone, Debug, PartialEq)]
/// CTE 存储/生产者包装，复用 PhysicalCte 数据。
pub struct PhysicalCteStorage(pub PhysicalCte);
impl PhysicalCteStorage {
    /// 与 Go PhysicalCTEStorage 一致，固定展示非递归 CTE 存储。
    pub fn explain_info(&self) -> &'static str {
        "Non-Recursive CTE Storage"
    }
    /// 当前实现直接返回原任务计划。
    pub fn attach_to_task(&self, task: PhysicalPlanNode) -> PhysicalPlanNode {
        task
    }
}

#[derive(Debug, PartialEq)]
/// CTE 写入端：把子计划结果写入指定存储，并记录 MPP 任务拓扑。
pub struct PhysicalCteSink {
    /// 目标 CTE 存储编号。
    pub id_for_storage: i64,
    /// 交换/写入压缩模式名。
    pub compression_mode: String,
    /// 本端 MPP 任务 ID。
    pub self_tasks: Vec<u64>,
    /// 目标任务 ID（接口兼容字段）。
    pub target_tasks: Vec<u64>,
    /// fragment split 后填充的本地 sink 数量。
    pub cte_sink_num: u32,
    /// fragment split 后填充的本地 source 数量。
    pub cte_source_num: u32,
    /// 唯一子计划。
    pub child: PhysicalPlanNode,
}
impl Clone for PhysicalCteSink {
    fn clone(&self) -> Self {
        Self {
            id_for_storage: self.id_for_storage,
            compression_mode: self.compression_mode.clone(),
            self_tasks: Vec::new(),
            target_tasks: Vec::new(),
            cte_sink_num: self.cte_sink_num,
            cte_source_num: self.cte_source_num,
            child: self.child.clone(),
        }
    }
}
impl PhysicalCteSink {
    /// 追加目标任务 ID。
    pub fn append_target_tasks(&mut self, tasks: &[u64]) {
        self.target_tasks.extend_from_slice(tasks);
    }
    /// 累计任务切片容量与子计划。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + ((self.self_tasks.capacity() + self.target_tasks.capacity()) * 8) as i64
            + self.child.memory_usage()
    }
    /// 编码为 kind=sink 的 CteExecutor。
    pub fn to_pb(&self) -> Result<CteExecutor, String> {
        Ok(CteExecutor {
            kind: "sink".into(),
            storage_id: self.id_for_storage as u32,
            source_count: self.cte_source_num,
            sink_count: self.cte_sink_num,
            field_columns: self.child.schema.clone(),
            compression: self.compression_mode.clone(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
/// CTE 消费端：无孩子，从存储读取并携带本地 source/sink 计数。
pub struct PhysicalCteSource {
    /// 源 CTE 存储编号。
    pub id_for_storage: i64,
    /// 本地 source 数量。
    pub cte_source_num: u32,
    /// 本地 sink 数量。
    pub cte_sink_num: u32,
    /// 输出列 ID。
    pub schema: Vec<i64>,
}
impl PhysicalCteSource {
    /// 编码为 kind=source 的 CteExecutor。
    pub fn to_pb(&self) -> CteExecutor {
        CteExecutor {
            kind: "source".into(),
            storage_id: self.id_for_storage as u32,
            source_count: self.cte_source_num,
            sink_count: self.cte_sink_num,
            field_columns: self.schema.clone(),
            compression: String::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
/// Sink/Source 共用的简化执行描述（对应 tipb CTE 消息的字段子集）。
pub struct CteExecutor {
    /// "sink" 或 "source"。
    pub kind: String,
    /// CTE 存储 ID。
    pub storage_id: u32,
    /// source 数量。
    pub source_count: u32,
    /// sink 数量。
    pub sink_count: u32,
    /// 输出字段列 ID。
    pub field_columns: Vec<i64>,
    /// 压缩模式（source 可为空）。
    pub compression: String,
}

/// 枚举 Root 且无序属性下的 CTE 物理候选。
pub fn exhaust_physical_cte(
    id: i64,
    property: &PhysicalProperty,
    stats: Stats,
) -> (Vec<PhysicalPlanNode>, bool) {
    (
        vec![PhysicalPlanNode {
            id,
            kind: PhysicalKind::CteStorage { id },
            schema: Vec::new(),
            children: Vec::new(),
            stats,
            required_properties: vec![property.clone()],
        }],
        true,
    )
}

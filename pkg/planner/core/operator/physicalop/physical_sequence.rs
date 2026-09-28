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

// 物理 Sequence 算子：在计划树中标记 CTE（公用表表达式）producer，并串联主查询。
//
// 前若干孩子为 CTE producer，最后一个孩子为主查询；输出 Schema 取自主查询。
// 下方大段注释保留 Go 完整语义迁移参考；当前可执行骨架见文件后半。

// CTE producer 标记节点的初始化、解释标识、schema 传播及 MPP/Root 子属性组合枚举。
// utilfuncp、plancodec 与 stringutil。
//
// PhysicalSequence is the physical representation of LogicalSequence. Used to mark the CTE producers in the plan tree.
// PhysicalSequence 对应 Go 同名结构体，本身不增加字段，只组合 PhysicalSchemaProducer。
// pub struct PhysicalSequence {
//     pub PhysicalSchemaProducer: PhysicalSchemaProducer,
// }
//
// impl PhysicalSequence {
// Init initializes PhysicalSequence.
// 初始化只绑定节点类型、统计信息与子属性，不执行 CTE producer 或主查询。
//     pub fn Init(
//         mut self,
//         ctx: base::PlanContext,
//         stats: property::StatsInfo,
//         block_offset: i32,
//         props: Vec<property::PhysicalProperty>,
//     ) -> Self {
//         self.BasePhysicalPlan =
//             NewBasePhysicalPlan(ctx, plancodec::TypeSequence, &self, block_offset);
//         self.SetStats(stats);
//         self.SetChildrenReqProps(props);
//         self
//     }
//
// MemoryUsage returns the memory usage of the PhysicalSequence.
// Rust 引用不表达 Go nil 接收者；Sequence 无额外字段，直接沿用 schema producer 的口径。
//     pub fn MemoryUsage(&self) -> i64 {
//         self.PhysicalSchemaProducer.MemoryUsage()
//     }
//
// ExplainID overrides the ExplainID.
// Go 返回延迟求值的 fmt.Stringer；这里以闭包占位保留读取会话变量的时机。
//     pub fn ExplainID(&self, _normalized: &[bool]) -> stringutil::StringerFunc {
//         stringutil::StringerFunc::new(|| {
// IgnoreExplainIDSuffix 用于稳定 EXPLAIN 输出；上下文为空时仍保留 ID 后缀。
//             if self.SCtx().is_some()
//                 && self
//                     .SCtx()
//                     .unwrap()
//                     .GetSessionVars()
//                     .StmtCtx
//                     .IgnoreExplainIDSuffix
//             {
//                 return self.TP();
//             }
//             format!("{}_{}", self.TP(), self.ID())
//         })
//     }
//
// ExplainInfo overrides the ExplainInfo.
//     pub fn ExplainInfo(&self) -> String {
//         "Sequence Node".to_string()
//     }
//
// Clone implements op.PhysicalPlan interface.
// 先保留值拷贝形状，再由 CloneWithSelf 以新上下文重建嵌入基类并传播错误。
//     pub fn Clone(&self, new_ctx: base::PlanContext) -> Result<base::PhysicalPlan, error> {
//         let mut cloned = self.shallow_clone();
//         cloned.SetSCtx(new_ctx);
//         let base = self
//             .PhysicalSchemaProducer
//             .CloneWithSelf(new_ctx, &mut cloned)?;
//         cloned.PhysicalSchemaProducer = *base;
//         Ok(cloned)
//     }
//
// Schema returns its last child(which is the main query tree)'s schema.
// 前面的孩子是 CTE producers，最后一个孩子才是主查询；Go 假设孩子非空，保留此前置条件。
//     pub fn Schema(&self) -> &expression::Schema {
//         self.Children()[self.Children().len() - 1].Schema()
//     }
//
// Attach2Task implements the PhysicalPlan interface.
// 任务拼接交给共享 helper；本文件不会实际调度 producer 或主查询任务。
//     pub fn Attach2Task(&self, tasks: Vec<base::Task>) -> base::Task {
//         utilfuncp::Attach2Task4PhysicalSequence(self, tasks)
//     }
// }
//
// ExhaustPhysicalPlans4LogicalSequence generates PhysicalSequence plans from LogicalSequence.
// 该函数按请求任务类型和 CTE MPP 状态生成子属性组合，再为每种组合创建 Sequence 候选。
// pub fn ExhaustPhysicalPlans4LogicalSequence(
//     super_plan: base::LogicalPlan,
//     prop: &property::PhysicalProperty,
// ) -> Result<(Vec<base::PhysicalPlan>, bool), error> {
//     let (g, ls) = base::GetGEAndLogicalOp::<logicalop::LogicalSequence>(super_plan);
//     let mut possible_children_props: Vec<Vec<property::PhysicalProperty>> = Vec::with_capacity(2);
//     let mut any_type = property::PhysicalProperty {
//         TaskTp: property::MppTaskType,
//         ExpectedCnt: f64::MAX,
//         MPPPartitionTp: property::AnyType,
//         CanAddEnforcer: true,
//         CTEProducerStatus: prop.CTEProducerStatus,
//         NoCopPushDown: prop.NoCopPushDown,
//         ..Default::default()
//     };
//
//     if prop.TaskTp == property::MppTaskType {
//         if prop.CTEProducerStatus == property::SomeCTEFailedMpp {
// 已知某个 CTE 不能走 MPP 时，不再生成 MPP Sequence 候选。
//             return Ok((Vec::new(), true));
//         }
//         any_type.CTEProducerStatus = property::AllCTECanMpp;
//         possible_children_props.push(vec![any_type.clone(), prop.CloneEssentialFields()]);
//     } else {
// Root 请求下，producer 和主查询都标记 SomeCTEFailedMpp，阻止错误的混合下推。
//         let mut copied = prop.CloneEssentialFields();
//         copied.CTEProducerStatus = property::SomeCTEFailedMpp;
//         possible_children_props.push(vec![
//             property::PhysicalProperty {
//                 TaskTp: property::RootTaskType,
//                 ExpectedCnt: f64::MAX,
//                 CTEProducerStatus: property::SomeCTEFailedMpp,
//                 ..Default::default()
//             },
//             copied,
//         ]);
//     }
//
//     if prop.TaskTp != property::MppTaskType
//         && prop.CTEProducerStatus != property::SomeCTEFailedMpp
//         && ls.SCtx().GetSessionVars().IsMPPAllowed()
//         && prop.IsSortItemEmpty()
//     {
// 无排序要求且允许 MPP 时，额外尝试 producer 与主查询都使用任意 MPP 分区。
//         possible_children_props.push(vec![any_type.clone(), any_type.CloneEssentialFields()]);
//     }
//
//     let seq_schema = if g.is_some() {
// memo GroupExpr 从最后一个输入取得主查询 schema。
//         let group = g.unwrap();
//         group.GetInputSchema(group.InputsLen() - 1)
//     } else {
//         ls.Children()[ls.ChildLen() - 1].Schema()
//     };
//
//     let mut seqs = Vec::with_capacity(possible_children_props.len());
//     for prop_choice in possible_children_props {
//         let mut child_reqs = Vec::with_capacity(ls.ChildLen());
// 除最后一个主查询外，所有 CTE producer 各自克隆第一种属性，避免共享后续修改。
//         for _ in 0..ls.ChildLen() - 1 {
//             child_reqs.push(prop_choice[0].CloneEssentialFields());
//         }
//         child_reqs.push(prop_choice[1].clone());
//         let mut seq = PhysicalSequence {
//             PhysicalSchemaProducer: Default::default(),
//         }
//         .Init(
//             ls.SCtx(),
//             ls.StatsInfo(),
//             ls.QueryBlockOffset(),
//             child_reqs,
//         );
//         seq.SetSchema(seq_schema.clone());
//         seqs.push(seq);
//     }
//     Ok((seqs, true))
// }
// */
use crate::physical_common_plans::{
    CteProducerStatus, PartitionType, PhysicalKind, PhysicalPlanNode, PhysicalProperty, Stats,
    TaskType,
};

/// 当前可执行的 Sequence 骨架：孩子列表、列 ID Schema、查询块偏移。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhysicalSequence {
    /// CTE producer 与主查询孩子节点。
    pub children: Vec<PhysicalPlanNode>,
    /// 输出列 UniqueID 列表（简化 Schema）。
    pub schema: Vec<i64>,
    /// 查询块偏移，用于 EXPLAIN ID。
    pub block_offset: i32,
    /// 对应逻辑 Sequence 会话上下文中的 MPP 开关。
    pub mpp_allowed: bool,
}
impl PhysicalSequence {
    /// 设置构造该候选时会话是否允许 MPP。
    pub fn set_mpp_allowed(&mut self, allowed: bool) {
        self.mpp_allowed = allowed;
    }
    /// 估算内存：结构体体积 + 各孩子占用。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + (self.schema.capacity() * std::mem::size_of::<i64>()) as i64
            + self
                .children
                .iter()
                .map(PhysicalPlanNode::memory_usage)
                .sum::<i64>()
    }
    /// 生成 EXPLAIN 用标识 `Sequence_{offset}`。
    pub fn explain_id(&self) -> String {
        format!("Sequence_{}", self.block_offset)
    }
    /// 返回与 Go `ExplainInfo` 相同的固定描述。
    pub fn explain_info(&self) -> &'static str {
        "Sequence Node"
    }
    /// Sequence 的输出 schema 始终取最后一个孩子（主查询）。
    pub fn output_schema(&self) -> Option<&[i64]> {
        self.children.last().map(|child| child.schema.as_slice())
    }
    /// 组装为 PhysicalPlanNode：统计取自最后一个孩子（主查询），ID 取孩子最大 ID+1。
    pub fn attach_to_tasks(self) -> Result<PhysicalPlanNode, String> {
        if self.children.is_empty() {
            return Err("PhysicalSequence requires at least one child".into());
        }
        // Schema/统计跟随主查询（最后一个孩子）。
        let main_query = self.children.last().unwrap();
        let stats = main_query.stats.clone();
        let schema = main_query.schema.clone();
        Ok(PhysicalPlanNode {
            id: self
                .children
                .iter()
                .map(|child| child.id)
                .max()
                .unwrap_or(0)
                + 1,
            kind: PhysicalKind::Sequence,
            schema,
            children: self.children,
            stats,
            required_properties: Vec::new(),
        })
    }
}
/// 由 Sequence 穷举物理候选：仅 Root 任务；最后一个孩子继承请求属性，其余用默认属性。
pub fn exhaust_physical_sequence(
    sequence: PhysicalSequence,
    property: &PhysicalProperty,
    stats: Stats,
) -> (Vec<PhysicalPlanNode>, bool) {
    if sequence.children.is_empty() {
        return (Vec::new(), true);
    }

    if property.task_type == TaskType::Mpp
        && property.cte_producer_status == CteProducerStatus::SomeFailedMpp
    {
        return (Vec::new(), true);
    }

    let mut any_mpp = PhysicalProperty {
        task_type: TaskType::Mpp,
        expected_count: f64::MAX,
        partition_type: PartitionType::Any,
        can_add_enforcer: true,
        cte_producer_status: property.cte_producer_status,
        no_cop_push_down: property.no_cop_push_down,
        ..PhysicalProperty::default()
    };

    let mut choices = Vec::with_capacity(2);
    if property.task_type == TaskType::Mpp {
        any_mpp.cte_producer_status = CteProducerStatus::AllCanMpp;
        choices.push((any_mpp.clone(), property.clone()));
    } else {
        let root_producer = PhysicalProperty {
            task_type: TaskType::Root,
            expected_count: f64::MAX,
            cte_producer_status: CteProducerStatus::SomeFailedMpp,
            ..PhysicalProperty::default()
        };
        let mut main_query = property.clone();
        main_query.cte_producer_status = CteProducerStatus::SomeFailedMpp;
        choices.push((root_producer, main_query));

        if property.cte_producer_status != CteProducerStatus::SomeFailedMpp
            && sequence.mpp_allowed
            && property.sort_items.is_empty()
        {
            choices.push((any_mpp.clone(), any_mpp));
        }
    }

    let schema = sequence.children.last().unwrap().schema.clone();
    let child_count = sequence.children.len();
    let plans = choices
        .into_iter()
        .map(|(producer_property, main_query_property)| {
            let mut required_properties = vec![producer_property; child_count.saturating_sub(1)];
            required_properties.push(main_query_property);
            PhysicalPlanNode {
                id: sequence.block_offset as i64,
                kind: PhysicalKind::Sequence,
                schema: schema.clone(),
                children: sequence.children.clone(),
                stats: stats.clone(),
                required_properties,
            }
        })
        .collect();
    (plans, true)
}

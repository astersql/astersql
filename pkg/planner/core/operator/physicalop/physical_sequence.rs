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

/// Go's concrete Sequence occurrence retained by the physical-plan RU bridge.
pub struct TypedPhysicalSequence {
    pub PhysicalSchemaProducer: crate::PhysicalSchemaProducer,
}
impl TypedPhysicalSequence {
    pub fn New(ctx: base::ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: crate::PhysicalSchemaProducer::New(
                crate::BasePhysicalPlan::New(ctx, "Sequence", 0),
            ),
        }
    }
    pub fn Clone(&self, ctx: base::ContextRef) -> Result<Self, expression::Error> {
        let mut producer = crate::PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(ctx)?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
        })
    }
}

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

// SELECT ... FOR UPDATE / SHARE 的 PhysicalLock 物理算子。
//
// 在读路径上对命中行加行锁（行级锁），保证后续更新可见性与冲突检测；
// MPP（大规模并行处理）模式下当前不支持 Lock，枚举时会告警并返回空候选。

use crate::physical_common_plans::{
    PhysicalKind, PhysicalPlanNode, PhysicalProperty, Stats, TaskType,
};
use crate::{BasePhysicalPlan, PhysicalSchemaProducer};
use base::{ContextRef, PhysicalPlan};
use logicalop::LogicalPlan as _;
use std::collections::BTreeMap;

/// Legacy optimizer representation of SELECT locking reads.
pub struct LegacyPhysicalLock {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    pub LockType: String,
    pub WaitSeconds: u64,
}

impl LegacyPhysicalLock {
    pub fn New(ctx: ContextRef, lock_type: String, wait_seconds: u64) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeLock,
                0,
            )),
            LockType: lock_type,
            WaitSeconds: wait_seconds,
        }
    }

    pub fn Init(
        mut self,
        ctx: ContextRef,
        stats: property::StatsInfo,
        offset: i32,
        child: property::PhysicalProperty,
    ) -> Self {
        self.PhysicalSchemaProducer.BasePhysicalPlan =
            BasePhysicalPlan::New(ctx, plancodec::TypeLock, offset);
        base::PhysicalPlan::set_stats(&mut self.PhysicalSchemaProducer.BasePhysicalPlan, stats);
        self.PhysicalSchemaProducer
            .BasePhysicalPlan
            .SetChildrenReqProps(vec![Box::new(child)]);
        self
    }

    pub fn ExplainInfo(&self) -> String {
        format!("{} {}", self.LockType, self.WaitSeconds)
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
            LockType: self.LockType.clone(),
            WaitSeconds: self.WaitSeconds,
        })
    }

    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage() + self.LockType.capacity() as i64
    }
}

pub fn ExhaustPhysicalPlans4LogicalLock(
    logical: &logicalop::LogicalLock,
    required: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    if required.IsFlashProp() {
        if let Some(ctx) = logical.SCtx() {
            ctx.GetSessionVars().RaiseWarningWhenMPPEnforced(
                "MPP mode may be blocked because operator `Lock` is not supported now.",
            );
        }
        return Vec::new();
    }
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    let child = required.CloneEssentialFields();
    let lock_type = match logical.Lock.LockType {
        parser_ast::SelectLockType::ForUpdate => "for update",
        parser_ast::SelectLockType::ForUpdateNoWait => "for update nowait",
        parser_ast::SelectLockType::ForUpdateWaitN => "for update wait",
        parser_ast::SelectLockType::ForShare => "for share",
        parser_ast::SelectLockType::ForShareNoWait => "for share nowait",
        _ => "none",
    };
    let mut plan = LegacyPhysicalLock::New(ctx.clone(), lock_type.to_owned(), logical.Lock.WaitSec);
    plan.PhysicalSchemaProducer
        .SetSchema(logical.Schema().Clone());
    vec![Box::new(
        plan.Init(
            ctx.clone(),
            logical
                .StatsInfo()
                .map(|stats| stats.ScaleByExpectCnt(ctx.GetSessionVars(), required.ExpectedCnt))
                .unwrap_or_default(),
            logical.QueryBlockOffset(),
            child,
        ),
    )]
}

/// 锁元数据：锁类型（如 FOR UPDATE）与等待秒数。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LockInfo {
    pub lock_type: String,
    /// 锁等待超时秒数；对应 Go `wait_sec`。
    pub wait_seconds: i64,
}
/// 活跃实现的 PhysicalLock：按表记录 handle 列，并对子计划输出加锁。
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalLock {
    pub lock: LockInfo,
    /// 表 ID → 用于加锁的 handle 列 ID 列表。
    pub table_id_to_handles: BTreeMap<i64, Vec<i64>>,
    /// 表 ID → 物理表 ID 列（分区场景 ExtraPhysTblID）。
    pub table_id_to_physical_id_column: BTreeMap<i64, i64>,
    pub child: PhysicalPlanNode,
}
impl PhysicalLock {
    /// 估算结构体与 handle 映射占用内存。
    pub fn memory_usage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self.lock.lock_type.capacity() as i64
            + self
                .table_id_to_handles
                .values()
                .map(|values| (values.capacity() * 8) as i64)
                .sum::<i64>()
            + self.child.memory_usage()
    }
    /// EXPLAIN：输出锁类型与等待秒数。
    pub fn explain_info(&self) -> String {
        format!("{} {}", self.lock.lock_type, self.lock.wait_seconds)
    }
    /// 按 Go `ResolveIndices` 仅校验 handle 列；物理表 ID 列由逻辑裁剪阶段保留。
    pub fn resolve_indices(&self) -> Result<(), String> {
        for column in self.table_id_to_handles.values().flatten() {
            if !self.child.schema.contains(column) {
                return Err(format!("lock handle column {column} is absent"));
            }
        }
        Ok(())
    }
    /// 计划缓存克隆（当前实现为整体 Clone）。
    pub fn clone_for_plan_cache(&self) -> Self {
        self.clone()
    }
}
/// 从锁语义枚举物理节点；MPP 任务类型下返回告警且无候选。
pub fn exhaust_physical_lock(
    lock: PhysicalLock,
    property: &PhysicalProperty,
    stats: Stats,
) -> (Vec<PhysicalPlanNode>, bool, Vec<String>) {
    // MPP 不支持 Lock：枚举完成（true）但候选为空，并附带告警文案。
    if property.task_type == TaskType::Mpp {
        return (
            Vec::new(),
            true,
            vec!["MPP mode may be blocked because operator `Lock` is not supported now.".into()],
        );
    }
    let schema = lock.child.schema.clone();
    (
        vec![PhysicalPlanNode {
            id: lock.child.id + 1,
            kind: PhysicalKind::Lock,
            schema,
            children: vec![lock.child],
            stats,
            required_properties: vec![property.clone()],
        }],
        true,
        Vec::new(),
    )
}

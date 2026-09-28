// Copyright 2026 AsterSQL.
/*
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

// 保留属性强制入口、MPP Exchange 插入和 Sort 补偿的规划期控制流。

/// EnforceProperty 把请求的物理属性强制施加到任务上，是排序与 MPP 分区补偿的统一入口。
pub fn enforce_property(
    property: &PhysicalProperty,
    mut task: TaskRef,
    ctx: PlanContextRef,
    fd: &FDSet,
) -> TaskRef {
    if property.task_type == TaskType::Mpp {
        // Go 先断言 *MppTask；类型不符或任务无效都不能继续插入 Exchange。
        let Some(mpp_task) = task.as_mpp_task() else {
            return invalid_task();
        };
        if mpp_task.invalid() {
            return invalid_task();
        }
        if !property.is_sort_item_all_for_partition() {
            // 当前 MPP 不支持普通 Sort，保留原警告文本并使该候选任务失效。
            ctx.session_vars().raise_warning_when_mpp_enforced(
                "MPP mode may be blocked because operator `Sort` is not supported now.",
            );
            return invalid_task();
        }
        task = mpp_task.enforce_exchanger(property, fd).into_task();
    }

    // Double-Cop IndexMerge 的 plan 可能在 indexPlanFinished=false 时暂为空，
    // 因此这里按 Go 语义只检查排序项与 Task.Invalid，不用 plan 是否为空判定任务有效性。
    if property.is_sort_item_empty() || task.invalid() {
        return task;
    }
    if property.task_type != TaskType::Mpp {
        task = task.convert_to_root_task(ctx.clone());
    }

    // 补偿 Sort 自身要求 root task，并用 MaxFloat64 表示不限制期望行数。
    let sort_req_prop = PhysicalProperty {
        task_type: TaskType::Root,
        sort_items: property.sort_items.clone(),
        expected_count: f64::MAX,
        ..PhysicalProperty::default()
    };
    let task_plan = task.plan();
    let mut sort = PhysicalSort {
        by_items: Vec::with_capacity(property.sort_items.len()),
        is_partial_sort: property.is_sort_item_all_for_partition(),
        ..PhysicalSort::default()
    }
    .init(
        ctx,
        task_plan.stats_info(),
        task_plan.query_block_offset(),
        PhysicalPropertyRef::new(sort_req_prop),
    );

    for item in &property.sort_items {
        // 列表达式和降序标志保持原顺序，避免改变复合排序键的优先级。
        sort.by_items.push(ByItems {
            expression: item.column.clone(),
            descending: item.descending,
        });
    }
    sort.attach2_task(task)
}

impl MppTask {
    /// EnforceExchanger 仅在当前分区与目标属性不兼容时复制任务并插入 Exchange。
    pub fn enforce_exchanger(&self, property: &PhysicalProperty, fd: &FDSet) -> Self {
        if !need_enforce_exchanger(
            self.partition_type,
            &self.hash_columns,
            property,
            fd,
        ) {
            return self.clone();
        }
        // Go 的 Copy 避免改写仍可能被其它候选计划共享的原任务。
        self.copy().enforce_exchanger_impl(property)
    }

    /// EnforceExchangerImpl 在 MPP 任务顶部依次加入 ExchangeSender 和 ExchangeReceiver。
    pub fn enforce_exchanger_impl(mut self, property: &PhysicalProperty) -> Self {
        if new_collation_enabled()
            && !self
                .plan
                .sctx()
                .session_vars()
                .hash_exchange_with_new_collation
            && property.mpp_partition_type == MppPartitionType::Hash
        {
            for column in &property.mpp_partition_columns {
                if is_string_type(column.column.ret_type().type_code()) {
                    // 新排序规则下字符串 Hash Exchange 尚不受支持，返回零值 MppTask 代表无效候选。
                    self.plan.sctx().session_vars().raise_warning_when_mpp_enforced(
                        "MPP mode may be blocked because when `new_collation_enabled` is true, HashJoin or HashAgg with string key is not supported now.",
                    );
                    return MppTask::default();
                }
            }
        }

        let ctx = self.plan.sctx();
        let mut sender = PhysicalExchangeSender {
            exchange_type: property.mpp_partition_type.to_exchange_type(),
            hash_columns: property.mpp_partition_columns.clone(),
            ..PhysicalExchangeSender::default()
        }
        .init(ctx.clone(), self.plan.stats_info());

        // MPP v1 起才协商 Exchange 压缩模式；旧版本保持 sender 的默认值。
        if ctx.session_vars().choose_mpp_version() >= MPP_VERSION_V1 {
            sender.compression_mode = ctx
                .session_vars()
                .choose_mpp_exchange_compression_mode();
        }

        sender.set_children(vec![self.plan.clone()]);
        let mut receiver = PhysicalExchangeReceiver::default().init(ctx, self.plan.stats_info());
        receiver.set_children(vec![sender.into_plan()]);

        let mut new_task = MppTask {
            plan: receiver.into_plan(),
            partition_type: property.mpp_partition_type,
            hash_columns: property.mpp_partition_columns.clone(),
            ..MppTask::default()
        };
        // 警告集合也属于候选计划状态，必须从旧任务完整复制到新任务。
        new_task.warnings.copy_from(&self.warnings);
        new_task
    }
}
*/

// 物理属性强制（enforce）：在规划期把请求的排序与 MPP 分区属性补偿到候选任务上。
//
// MPP（Massively Parallel Processing）指跨 TiFlash 节点并行执行；Exchange 是 MPP
// 片段之间重分布数据的发送/接收算子。本模块对应 Go 侧 `EnforceProperty` /
// `EnforceExchanger` 的简化可运行骨架。

use crate::physical_common_plans::{
    PartitionType, PhysicalKind, PhysicalPlanNode, PhysicalProperty, TaskType,
};
use std::collections::{BTreeMap, BTreeSet};

/// 携带物理计划与任务元信息的候选执行任务。
///
/// `invalid` 为真表示该候选在属性强制阶段已被否决；`partition_type` /
/// `hash_columns` 描述当前 MPP 分区状态，供 Exchange 兼容性判断使用。
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalTask {
    pub plan: PhysicalPlanNode,
    pub task_type: TaskType,
    pub invalid: bool,
    pub partition_type: PartitionType,
    pub hash_columns: Vec<i64>,
    pub warnings: Vec<String>,
}

/// 属性强制所需的会话侧开关与列类型提示。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnforceContext {
    pub allow_mpp: bool,
    pub new_collation: bool,
    pub hash_exchange_with_new_collation: bool,
    pub string_columns: Vec<i64>,
    pub mpp_version: u32,
    pub compression: String,
}

/// 把请求的物理属性强制施加到任务上：先按需插入 MPP Exchange，再补偿 Sort。
pub fn enforce_property(
    property: &PhysicalProperty,
    mut task: PhysicalTask,
    context: &EnforceContext,
    equivalences: &BTreeMap<i64, i64>,
) -> PhysicalTask {
    if property.task_type == TaskType::Mpp {
        // 非 MPP 任务、已失效任务或不允许 MPP 时直接否决该候选。
        if task.task_type != TaskType::Mpp || task.invalid || !context.allow_mpp {
            task.invalid = true;
            return task;
        }
        // 当前 MPP 只支持与分区列完全一致的 Sort；保留警告并使任务失效。
        if !sort_items_all_for_partition(property) {
            task.warnings.push(
                "MPP mode may be blocked because operator `Sort` is not supported now.".into(),
            );
            task.invalid = true;
            return task;
        }
        task = enforce_exchanger(task, property, context, equivalences);
    }
    // 无排序需求或任务已失效时跳过 Sort 补偿。
    if property.sort_items.is_empty() || task.invalid {
        return task;
    }
    // 非 MPP 路径上 Sort 要求 Root 任务。
    if property.task_type != TaskType::Mpp {
        task.task_type = TaskType::Root;
    }
    let schema = task.plan.schema.clone();
    let stats = task.plan.stats.clone();
    // 在计划树顶部包一层 Sort；partial 表示仅按分区键排序。
    task.plan = PhysicalPlanNode {
        id: task.plan.id + 1,
        kind: PhysicalKind::Sort {
            by: property.sort_items.clone(),
            partial: sort_items_all_for_partition(property),
        },
        schema,
        children: vec![task.plan],
        stats,
        required_properties: vec![PhysicalProperty {
            task_type: TaskType::Root,
            sort_items: property.sort_items.clone(),
            ..PhysicalProperty::default()
        }],
    };
    task
}

/// 在分区不兼容时复制任务并插入 ExchangeSender / ExchangeReceiver。
///
/// 列等价集 `equivalences` 允许用函数依赖中的等价列判定哈希分区是否已满足。
pub fn enforce_exchanger(
    mut task: PhysicalTask,
    property: &PhysicalProperty,
    context: &EnforceContext,
    equivalences: &BTreeMap<i64, i64>,
) -> PhysicalTask {
    if !need_enforce_exchanger(&task, property, equivalences) {
        return task;
    }
    // 新排序规则下字符串 Hash Exchange 尚不受支持。
    if context.new_collation
        && !context.hash_exchange_with_new_collation
        && property.partition_type == PartitionType::Hash
        && property
            .partition_columns
            .iter()
            .any(|column| context.string_columns.contains(column))
    {
        task.warnings.push("MPP mode may be blocked because when `new_collation_enabled` is true, HashJoin or HashAgg with string key is not supported now.".into());
        task.invalid = true;
        return task;
    }
    let schema = task.plan.schema.clone();
    let stats = task.plan.stats.clone();
    // MPP v1 起才协商 Exchange 压缩；旧版本保持空压缩串。
    let sender = PhysicalPlanNode {
        id: task.plan.id + 1,
        kind: PhysicalKind::ExchangeSender {
            partition_type: property.partition_type,
            columns: property.partition_columns.clone(),
            compression: if context.mpp_version >= 1 {
                context.compression.clone()
            } else {
                String::new()
            },
        },
        schema: schema.clone(),
        children: vec![task.plan],
        stats: stats.clone(),
        required_properties: Vec::new(),
    };
    // Receiver 挂在 Sender 之上，并同步任务的分区元数据。
    task.plan = PhysicalPlanNode {
        id: sender.id + 1,
        kind: PhysicalKind::ExchangeReceiver,
        schema,
        children: vec![sender],
        stats,
        required_properties: Vec::new(),
    };
    task.partition_type = property.partition_type;
    task.hash_columns = property.partition_columns.clone();
    task
}

/// 简化属性用分区列承载 Go `SortItemsForPartition` 的列序。
fn sort_items_all_for_partition(property: &PhysicalProperty) -> bool {
    property.sort_items.is_empty()
        || property.sort_items.len() == property.partition_columns.len()
            && property
                .sort_items
                .iter()
                .zip(&property.partition_columns)
                .all(|(sort_item, partition_column)| sort_item.column == *partition_column)
}

/// 与 Go `property.NeedEnforceExchanger` 保持一致的分区判定。
fn need_enforce_exchanger(
    task: &PhysicalTask,
    property: &PhysicalProperty,
    equivalences: &BTreeMap<i64, i64>,
) -> bool {
    match property.partition_type {
        // Any 接受任意已有分区，Broadcast 则总是要求新的广播边界。
        PartitionType::Any => false,
        PartitionType::Broadcast => true,
        PartitionType::Single => task.partition_type != PartitionType::Single,
        PartitionType::Hash => {
            if task.partition_type != PartitionType::Hash {
                return true;
            }
            // Go 在 FD 存在且已有 Hash 键时使用子集定理：当前每个键只需
            // 落入某个需求键的等价闭包，不要求两边等长。
            if !equivalences.is_empty() && !task.hash_columns.is_empty() {
                return task.hash_columns.iter().any(|supplied| {
                    !property
                        .partition_columns
                        .iter()
                        .any(|required| equivalent(*supplied, *required, equivalences))
                });
            }
            task.hash_columns != property.partition_columns
        }
    }
}

/// `BTreeMap` 以边集表示等价关系；按无向图求闭包以覆盖间接等价。
fn equivalent(left: i64, right: i64, equivalences: &BTreeMap<i64, i64>) -> bool {
    if left == right {
        return true;
    }
    let mut visited = BTreeSet::from([left]);
    let mut pending = vec![left];
    while let Some(column) = pending.pop() {
        for (&from, &to) in equivalences {
            if from == column && visited.insert(to) {
                if to == right {
                    return true;
                }
                pending.push(to);
            }
            if to == column && visited.insert(from) {
                if from == right {
                    return true;
                }
                pending.push(from);
            }
        }
    }
    false
}

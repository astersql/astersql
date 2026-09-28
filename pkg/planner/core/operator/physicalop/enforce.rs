// Copyright 2026 AsterSQL.

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

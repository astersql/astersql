// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// FindBestTask 代价枚举与强制排序（enforcer）相关单元测试。
//
// 覆盖：代价溢出不使 Task 失效、仅在允许时插入 Sort enforcer、
// Hint 强制选用计划、以及 Hint 不适用时回退并告警。

use crate::find_best_task::{
    AccessPath, DataSource, LogicalPlan, PhysicalProperty, SortItem, findBestTask, getTaskPlanCost,
    mockLogicalPlan4Test,
};
use crate::task::{PlanKind, StatsInfo, StoreType, Task, TaskType};

/// 构造带排序项与 enforcer 开关的物理属性。
fn property(items: &[(usize, bool)], can_add_enforcer: bool) -> PhysicalProperty {
    PhysicalProperty {
        sort_items: items
            .iter()
            .map(|(column, desc)| SortItem {
                column: *column,
                desc: *desc,
            })
            .collect(),
        can_add_enforcer,
        ..PhysicalProperty::default()
    }
}

/// 从 Root Task 取出根计划节点；非 Root 则 panic。
fn root_plan(task: &Task) -> &crate::task::PlanNode {
    let Task::Root {
        plan: Some(plan), ..
    } = task
    else {
        panic!("expected a valid root task");
    };
    plan
}

/// 代价溢出时应夹到 f64::MAX，且 Task 仍有效。
#[test]
fn cost_overflow_does_not_turn_a_task_invalid() {
    let mut logical = mockLogicalPlan4Test {
        costOverflow: true,
        ..mockLogicalPlan4Test::Init()
    };
    let task = logical.FindBestTask(&PhysicalProperty::default(), 1);

    assert!(!task.is_invalid());
    let (cost, invalid) = getTaskPlanCost(&task, 1);
    assert!(!invalid);
    assert_eq!(cost, f64::MAX);
}

/// 不允许加 enforcer 时混合排序属性导致无效；允许时插入 Sort。
#[test]
fn enforced_property_is_used_only_when_allowed() {
    let mixed_order = [(1, false), (2, true)];
    let mut logical = mockLogicalPlan4Test::Init();
    let task = logical.FindBestTask(&property(&mixed_order, false), 1);
    assert!(task.is_invalid());

    let mut logical = mockLogicalPlan4Test::Init();
    let task = logical.FindBestTask(&property(&mixed_order, true), 1);
    let plan = root_plan(&task);
    assert_eq!(plan.kind, PlanKind::Sort);
    assert_eq!(plan.by_items.len(), 2);
    assert_eq!(plan.by_items[0].column, Some(1));
    assert_eq!(plan.by_items[0].name, "asc");
    assert_eq!(plan.by_items[1].column, Some(2));
    assert_eq!(plan.by_items[1].name, "desc");
}

/// 有 Hint 指向 plan2 且可生成时，即使禁止普通 enforcer 也会强制 Sort+plan2。
#[test]
fn hinted_plan_is_enforced_even_when_the_property_disallows_a_normal_enforcer() {
    for can_add_enforcer in [true, false] {
        let mut logical = mockLogicalPlan4Test {
            hasHintForPlan2: true,
            canGeneratePlan2: true,
            ..mockLogicalPlan4Test::Init()
        };
        let task = logical.FindBestTask(&property(&[(1, false)], can_add_enforcer), 1);
        let sort = root_plan(&task);
        assert_eq!(sort.kind, PlanKind::Sort);
        assert_eq!(sort.children.len(), 1);
        assert_eq!(
            sort.children[0].kind,
            PlanKind::Other("mockPhysicalPlan2".to_owned())
        );
    }
}

/// Hint 指向不可生成的 plan2 时告警一次，并回退到可匹配的 plan1。
#[test]
fn inapplicable_hint_warns_and_falls_back_to_matching_plan() {
    for can_add_enforcer in [false, true] {
        let mut logical = mockLogicalPlan4Test {
            hasHintForPlan2: true,
            canGeneratePlan2: false,
            ..mockLogicalPlan4Test::Init()
        };
        let task = logical.FindBestTask(&property(&[(1, false)], can_add_enforcer), 1);
        assert_eq!(logical.warning_count, 1);
        assert_eq!(
            root_plan(&task).kind,
            PlanKind::Other("mockPhysicalPlan1".to_owned())
        );
    }
}

/// 不同 MPP 分区要求不能命中同一个 FindBestTask 缓存项。
#[test]
fn cache_key_distinguishes_mpp_partition_requirement() {
    let mut logical = LogicalPlan {
        data_source: Some(DataSource {
            paths: vec![AccessPath {
                store: Some(StoreType::TiFlash),
                count_after_access: 1.0,
                ..AccessPath::default()
            }],
            stats: StatsInfo {
                row_count: 1.0,
                ..StatsInfo::default()
            },
            mpp_allowed: true,
            ..DataSource::default()
        }),
        ..LogicalPlan::default()
    };
    let any_partition = PhysicalProperty {
        task_type: TaskType::Mpp,
        mpp_partition_any: true,
        ..PhysicalProperty::default()
    };
    let required_partition = PhysicalProperty {
        mpp_partition_any: false,
        ..any_partition.clone()
    };

    assert!(!findBestTask(&mut logical, &any_partition, 1).is_invalid());
    assert!(findBestTask(&mut logical, &required_partition, 1).is_invalid());
}

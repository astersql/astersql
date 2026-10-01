// Copyright 2026 AsterSQL.

use crate::physical_common_plans::{PhysicalProperty, Stats, TaskType};
use crate::physical_cte_table::{PhysicalCteTable, find_best_task_for_cte_table};

fn table() -> PhysicalCteTable {
    PhysicalCteTable {
        id_for_storage: 42,
        seed_statistics: Stats {
            row_count: 8.0,
            version: 3,
        },
        schema: vec![1, 2],
    }
}

/// Go 的 `findBestTask4LogicalCTETable` 不按 TaskTp 拒绝候选；它最终总是挂到 RootTask。
#[test]
fn non_root_property_still_builds_root_cte_table_like_go() {
    for task_type in [TaskType::Cop, TaskType::Mpp] {
        let property = PhysicalProperty {
            task_type,
            ..PhysicalProperty::default()
        };

        let plan = find_best_task_for_cte_table(&table(), &property)
            .expect("CTE table selection is infallible")
            .expect("Go builds a RootTask regardless of the requested TaskTp");
        assert_eq!(plan.id, 42);
        assert_eq!(plan.schema, vec![1, 2]);
        assert_eq!(plan.stats.row_count, 8.0);
        assert!(plan.children.is_empty());
        assert!(plan.required_properties.is_empty());
    }
}

/// CTE 表自身不提供顺序；带排序项时必须与 Go 一样返回无效任务。
#[test]
fn ordered_property_is_rejected() {
    let property = PhysicalProperty {
        sort_items: vec![crate::physical_common_plans::SortItem {
            column: 1,
            descending: false,
        }],
        ..PhysicalProperty::default()
    };

    assert_eq!(
        find_best_task_for_cte_table(&table(), &property).unwrap(),
        None
    );
}

#[test]
fn ru_concrete_cte_table_clone_retains_storage_and_plan_id() {
    use base::Plan as _;
    let ctx = crate::physical_window_test::ru_orchestration_context();
    let plan = crate::PhysicalCTETable::New(ctx.clone(), 17);
    let clone = plan.Clone(ctx).unwrap();
    assert_eq!(clone.id(), plan.id());
    assert_eq!(clone.IDForStorage, 17);
    assert_eq!(clone.explain_info(), "Scan on CTE_17");
}

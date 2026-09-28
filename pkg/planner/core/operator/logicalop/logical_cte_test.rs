// Copyright 2026 AsterSQL.

use super::*;

fn column(id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = id;
    column.ID = id;
    column
}

fn plan_with_stats(schema_column: i64, row_count: f64, ndv: f64) -> LogicalPlanRef {
    let mut plan = LogicalTableDual::default();
    plan.SetSchema(expression::NewSchema(vec![column(schema_column)]));
    plan.SetStats(StatsInfo {
        RowCount: row_count,
        ColNDVs: [(schema_column, ndv)].into_iter().collect(),
        ..Default::default()
    });
    Box::new(plan)
}

#[test]
fn recursive_distinct_cte_estimates_union_cardinality_from_combined_ndv() {
    let visible = column(10);
    let seed = plan_with_stats(20, 100.0, 3.0);
    let recursive = plan_with_stats(30, 50.0, 4.0);
    let mut cte = LogicalCTE::default();
    cte.SetSchema(expression::NewSchema(vec![visible]));
    {
        let mut class = cte.Cte.borrow_mut();
        class.IsDistinct = true;
        class.SeedPartLogicalPlan = Some(seed);
        class.RecursivePartLogicalPlan = Some(recursive);
    }

    let (stats, reloaded) = cte.DeriveStats(false).unwrap();

    assert!(reloaded);
    assert_eq!(stats.ColNDVs[&10], 7.0);
    assert_eq!(stats.RowCount, 7.0);
}

#[test]
fn cte_possible_properties_fall_back_to_seed_capability() {
    let mut seed = plan_with_stats(20, 1.0, 1.0);
    seed.base_mut().PreparePossibleProperties(&[true]);
    let mut recursive = plan_with_stats(30, 1.0, 1.0);
    recursive.base_mut().PreparePossibleProperties(&[false]);
    let mut cte = LogicalCTE::default();
    {
        let mut class = cte.Cte.borrow_mut();
        class.SeedPartLogicalPlan = Some(seed);
        class.RecursivePartLogicalPlan = Some(recursive);
    }

    let properties = cte.PreparePossibleProperties();

    assert!(properties.Orders.is_empty());
    assert!(properties.HasTiFlash);
}

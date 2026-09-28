// Copyright 2026 AsterSQL.

use crate::*;
use std::any::Any;

fn column(id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = id;
    column
}

struct StatsPlan {
    base: BaseLogicalPlan,
}

impl StatsPlan {
    fn new(columns: Vec<Column>, row_count: f64, ndvs: &[(i64, f64)]) -> Self {
        let mut plan = Self {
            base: BaseLogicalPlan::default(),
        };
        plan.SetSchema(expression::NewSchema(columns));
        let mut stats = StatsInfo {
            RowCount: row_count,
            ..StatsInfo::default()
        };
        stats.ColNDVs.extend(ndvs.iter().copied());
        plan.SetStats(stats);
        plan
    }
}

impl LogicalPlan for StatsPlan {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn base(&self) -> &BaseLogicalPlan {
        &self.base
    }

    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.base
    }
}

fn apply_with_children(join_type: JoinType) -> LogicalApply {
    let outer = column(1);
    let inner = column(2);
    let marker = column(3);
    let mut apply = LogicalApply {
        LogicalJoin: LogicalJoin {
            JoinType: join_type,
            ..LogicalJoin::default()
        },
        ..LogicalApply::default()
    };
    apply.SetSchema(expression::NewSchema(vec![
        outer.clone(),
        inner.clone(),
        marker,
    ]));
    apply.SetChildren(vec![
        Box::new(StatsPlan::new(vec![outer], 100.0, &[(1, 40.0)])),
        Box::new(StatsPlan::new(vec![inner], 20.0, &[(2, 10.0)])),
    ]);
    apply
}

#[test]
fn extract_col_groups_only_propagates_outer_join_preserved_groups() {
    let group = vec![column(1)];
    let inner = apply_with_children(JoinType::InnerJoin);
    assert!(
        inner
            .ExtractColGroups(std::slice::from_ref(&group))
            .is_empty()
    );

    let left_outer = apply_with_children(JoinType::LeftOuterJoin);
    let extracted = left_outer.ExtractColGroups(&[group]);
    assert_eq!(extracted.len(), 1);
    assert_eq!(extracted[0].len(), 1);
    assert_eq!(extracted[0][0].UniqueID, 1);
}

#[test]
fn left_outer_semi_keeps_outer_cardinality_and_boolean_marker_ndv() {
    let mut apply = apply_with_children(JoinType::LeftOuterSemiJoin);
    let (stats, changed) = apply.DeriveStats(false).expect("derive apply statistics");

    assert!(changed);
    assert_eq!(stats.RowCount, 100.0);
    assert_eq!(stats.ColNDVs.get(&1), Some(&40.0));
    assert_eq!(stats.ColNDVs.get(&3), Some(&2.0));
}

#[test]
fn apply_owned_corcols_are_not_reported_as_external_dependencies() {
    let mut apply = apply_with_children(JoinType::InnerJoin);
    apply.CorCols.push(CorrelatedColumn {
        column: column(99),
        data: None,
    });

    assert!(apply.ExtractCorrelatedCols().is_empty());
}

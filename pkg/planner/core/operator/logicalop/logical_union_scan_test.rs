// Copyright 2026 AsterSQL.

use crate::*;
use std::any::Any;

#[derive(Default)]
struct PruningChild {
    base: BaseLogicalPlan,
}

impl LogicalPlan for PruningChild {
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

    fn PruneColumns(&mut self, _parent_used_cols: &[Column]) -> Result<()> {
        let mut replacement = Column::default();
        replacement.UniqueID = 99;
        self.SetSchema(expression::NewSchema(vec![replacement]));
        self.SetOutputNames(NameSlice(Vec::new()));
        Ok(())
    }
}

fn column(id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = id;
    column
}

#[test]
fn prune_preserves_union_scan_output_metadata_like_go() {
    let mut scan = LogicalUnionScan::default();
    scan.SetSchema(expression::NewSchema(vec![column(7)]));
    scan.SetOutputNames(NameSlice(vec![None]));
    scan.SetChildren(vec![Box::new(PruningChild::default())]);

    scan.PruneColumns(&[column(7)]).unwrap();

    assert_eq!(scan.Schema().Columns[0].UniqueID, 7);
    assert_eq!(scan.OutputNames().0.len(), 1);
    assert_eq!(scan.Children()[0].Schema().Columns[0].UniqueID, 99);
}

#[test]
#[should_panic]
fn prune_requires_a_child_like_go() {
    LogicalUnionScan::default().PruneColumns(&[]).unwrap();
}

#[test]
#[should_panic]
fn predicate_pushdown_requires_a_child_like_go() {
    LogicalUnionScan::default()
        .PredicatePushDown(Vec::new())
        .unwrap();
}

#[test]
fn possible_properties_clear_cached_tiflash_like_go() {
    let mut scan = LogicalUnionScan::default();
    scan.base_mut().PreparePossibleProperties(&[true]);

    let properties = scan.PreparePossibleProperties(
        &expression::NewSchema(Vec::new()),
        &[Some(UnionScanProperties {
            Orders: vec![vec![column(3)]],
            HasTiFlash: true,
        })],
    );

    assert_eq!(properties.Orders[0][0].UniqueID, 3);
    assert!(!properties.HasTiFlash);
    assert!(!scan.base().PreparePossiblePropertiesValue());
}

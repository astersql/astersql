// Copyright 2026 AsterSQL.

use super::*;
use logicalop::LogicalPlan;

#[test]
fn shared_cte_seed_prepares_tiflash_before_consumers() {
    let mut source = logicalop::DataSource::default();
    source.TableInfo.TiFlashReplica = Some(model_dependency::TiFlashReplicaInfo {
        Count: 1,
        Available: true,
        ..Default::default()
    });
    let mut first = logicalop::LogicalCTE::default();
    first.Cte.borrow_mut().SeedPartLogicalPlan = Some(Box::new(source));
    let mut second = logicalop::LogicalCTE::default();
    second.Cte = first.Cte.clone();
    let mut parent = logicalop::LogicalJoin::default();
    parent.SetChildren(vec![Box::new(first), Box::new(second)]);

    assert!(prepare_tiflash_availability(&mut parent));
    for child in parent.Children() {
        assert!(child.base().PreparePossiblePropertiesValue());
    }
}

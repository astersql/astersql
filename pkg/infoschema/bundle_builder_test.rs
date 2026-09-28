// Copyright 2026 AsterSQL.

use std::collections::HashMap;
use std::sync::Arc;

use crate::bundle_builder::{
    BundleSchema, PartitionBundleSpec, TableBundleSpec, bundleInfoBuilder,
};
use crate::{CiString, PolicyInfo};

#[derive(Default)]
struct TestSchema {
    policies: HashMap<i64, Arc<PolicyInfo>>,
    tables: HashMap<i64, TableBundleSpec>,
}

impl BundleSchema for TestSchema {
    fn policy_by_id(&self, policy_id: i64) -> Option<Arc<PolicyInfo>> {
        self.policies.get(&policy_id).cloned()
    }

    fn table_bundle_spec(&self, table_id: i64) -> Option<TableBundleSpec> {
        self.tables.get(&table_id).cloned()
    }

    fn all_table_bundle_specs(&self) -> Vec<TableBundleSpec> {
        self.tables.values().cloned().collect()
    }
}

fn policy(id: i64, name: &str) -> Arc<PolicyInfo> {
    Arc::new(PolicyInfo {
        id,
        name: CiString::new(name),
    })
}

#[test]
fn changed_partition_policy_does_not_mark_its_table_for_delta_update() {
    let mut schema = TestSchema::default();
    schema.policies.insert(7, policy(7, "before"));
    schema.tables.insert(
        1,
        TableBundleSpec {
            table_id: 1,
            policy_id: None,
            partitions: vec![PartitionBundleSpec {
                partition_id: 11,
                policy_id: Some(7),
            }],
        },
    );

    let mut builder = bundleInfoBuilder::new();
    assert!(builder.updateInfoSchemaBundles(&schema).is_empty());
    assert_eq!(builder.bundles()[&11].rules, ["policy:7:before"]);

    schema.policies.insert(7, policy(7, "after"));
    builder.SetDeltaUpdateBundles();
    builder.initBundleInfoBuilder();
    builder.markBundlesReferPolicyShouldUpdate(7);
    assert!(builder.updateInfoSchemaBundles(&schema).is_empty());

    assert_eq!(builder.bundles()[&11].rules, ["policy:7:before"]);
}

#[test]
fn bundle_errors_do_not_stop_remaining_partitions_or_tables() {
    let mut schema = TestSchema::default();
    schema.policies.insert(9, policy(9, "valid"));
    schema.tables.insert(
        1,
        TableBundleSpec {
            table_id: 1,
            policy_id: Some(404),
            partitions: vec![PartitionBundleSpec {
                partition_id: 11,
                policy_id: Some(9),
            }],
        },
    );
    schema.tables.insert(
        2,
        TableBundleSpec {
            table_id: 2,
            policy_id: None,
            partitions: vec![
                PartitionBundleSpec {
                    partition_id: 21,
                    policy_id: Some(405),
                },
                PartitionBundleSpec {
                    partition_id: 22,
                    policy_id: Some(9),
                },
            ],
        },
    );

    let mut builder = bundleInfoBuilder::new();
    let errors = builder.updateInfoSchemaBundles(&schema);

    assert_eq!(errors.len(), 2);
    assert_eq!(builder.bundles()[&11].rules, ["policy:9:valid"]);
    assert_eq!(builder.bundles()[&22].rules, ["policy:9:valid"]);
}

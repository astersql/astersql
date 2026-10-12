// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 基于 meta / PlacementSettings 构造 Bundle 的单元测试。
//
// 验证由 Placement Options（主区域、副本区域、Follower 数、调度策略等）
// 生成的 Bundle（规则组）规则数量与 Leader DC（Leader 所在可用区）提取正确。
// 文件前半保留 Go 侧依赖 mockstore/meta 的表、分区 Bundle 构造用例形状。

use super::*;

use std::collections::HashMap;

use meta_model::group_4::{PartitionInfo, PolicyRefInfo};

#[derive(Default)]
struct TestPolicyGetter {
    policies: HashMap<i64, model::PolicyInfo>,
}

impl PolicyGetter for TestPolicyGetter {
    fn GetPolicy(&self, policy_id: i64) -> Result<model::PolicyInfo, Error> {
        self.policies
            .get(&policy_id)
            .cloned()
            .ok_or_else(|| Error::new(format!("policy {policy_id} not found")))
    }
}

struct MetaBundleSuite {
    getter: TestPolicyGetter,
    table1: model::TableInfo,
    table2: model::TableInfo,
    table3: model::TableInfo,
    table4: model::TableInfo,
}

fn policy_ref(id: i64) -> PolicyRefInfo {
    PolicyRefInfo {
        ID: id,
        ..Default::default()
    }
}

fn partition(id: i64, policy_id: Option<i64>) -> model::PartitionDefinition {
    model::PartitionDefinition {
        ID: id,
        PlacementPolicyRef: policy_id.map(policy_ref),
        ..Default::default()
    }
}

fn create_meta_bundle_suite() -> MetaBundleSuite {
    let policies = [
        model::PolicyInfo {
            ID: 11,
            PlacementSettings: model::PlacementSettings {
                PrimaryRegion: "r1".into(),
                Regions: "r1,r2".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        model::PolicyInfo {
            ID: 12,
            PlacementSettings: model::PlacementSettings {
                PrimaryRegion: "r2".into(),
                Regions: "r1,r2".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        model::PolicyInfo {
            ID: 13,
            PlacementSettings: model::PlacementSettings {
                LeaderConstraints: "[+region=bj]".into(),
                ..Default::default()
            },
            ..Default::default()
        },
    ]
    .into_iter()
    .map(|policy| (policy.ID, policy))
    .collect();

    MetaBundleSuite {
        getter: TestPolicyGetter { policies },
        table1: model::TableInfo {
            ID: 101,
            PlacementPolicyRef: Some(policy_ref(11)),
            Partition: Some(PartitionInfo {
                Definitions: vec![
                    partition(1000, None),
                    partition(1001, Some(12)),
                    partition(1002, None),
                ],
                ..Default::default()
            }),
            ..Default::default()
        },
        table2: model::TableInfo {
            ID: 102,
            Partition: Some(PartitionInfo {
                Definitions: vec![
                    partition(1000, Some(11)),
                    partition(1001, None),
                    partition(1002, None),
                ],
                ..Default::default()
            }),
            ..Default::default()
        },
        table3: model::TableInfo {
            ID: 103,
            PlacementPolicyRef: Some(policy_ref(13)),
            ..Default::default()
        },
        table4: model::TableInfo {
            ID: 104,
            ..Default::default()
        },
    }
}

fn encoded_table_prefix(id: i64) -> String {
    hex::encode(crate::codec::EncodeBytes(
        Vec::new(),
        crate::tablecodec::GenTablePrefix(id).as_ref(),
    ))
}

fn assert_rule_range(rule: &pd::Rule, id: i64, index: i32, group_id: &str) {
    assert_eq!(rule.GroupID, group_id);
    assert_eq!(rule.Index, index);
    assert_eq!(rule.StartKeyHex, encoded_table_prefix(id));
    assert_eq!(rule.EndKeyHex, encoded_table_prefix(id + 1));
}

fn expected_bundle(
    suite: &MetaBundleSuite,
    policy_id: i64,
    bundle_index: i32,
    ids: &[i64],
) -> Bundle {
    let policy = suite.getter.policies.get(&policy_id).unwrap();
    let base_rules = NewBundleFromOptions(Some(&policy.PlacementSettings))
        .unwrap()
        .unwrap()
        .Rules;
    let group_id = GroupID(ids[0]);
    let mut rules = Vec::with_capacity(base_rules.len() * ids.len());

    for (id_index, id) in ids.iter().copied().enumerate() {
        for (rule_index, base_rule) in base_rules.iter().enumerate() {
            let mut rule = base_rule.clone();
            let prefix = if bundle_index == RuleIndexPartition || id_index > 0 {
                "partition"
            } else {
                "table"
            };
            rule.ID = format!("{prefix}_rule_{id}_{rule_index}");
            rule.GroupID = group_id.clone();
            rule.Index = if id_index == 0 {
                RuleIndexTable
            } else {
                RuleIndexPartition
            };
            rule.StartKeyHex = encoded_table_prefix(id);
            rule.EndKeyHex = encoded_table_prefix(id + 1);
            rules.push(rule);
        }
    }

    Bundle {
        ID: group_id,
        Index: bundle_index,
        Override: true,
        Rules: rules,
    }
}

#[test]
fn new_table_bundle_matches_go_meta_suite() {
    let suite = create_meta_bundle_suite();

    let bundle = NewTableBundle(&suite.getter, &suite.table1)
        .unwrap()
        .unwrap();
    assert_eq!(
        bundle,
        expected_bundle(&suite, 11, RuleIndexTable, &[101, 1000, 1001, 1002])
    );
    assert_eq!(bundle.ID, GroupID(101));
    assert_eq!(bundle.Index, RuleIndexTable);
    assert!(bundle.Override);
    assert_eq!(bundle.Rules.len(), 12);
    for rule in &bundle.Rules[0..3] {
        assert_rule_range(rule, 101, RuleIndexTable, &bundle.ID);
    }
    for (rules, id) in bundle.Rules[3..].chunks(3).zip([1000, 1001, 1002]) {
        for rule in rules {
            assert_rule_range(rule, id, RuleIndexPartition, &bundle.ID);
        }
    }

    assert!(
        NewTableBundle(&suite.getter, &suite.table2)
            .unwrap()
            .is_none()
    );
    let table3 = NewTableBundle(&suite.getter, &suite.table3)
        .unwrap()
        .unwrap();
    assert_eq!(table3, expected_bundle(&suite, 13, RuleIndexTable, &[103]));
    assert_eq!(table3.ID, GroupID(103));
    assert_eq!(table3.Rules.len(), 2);
    assert!(
        NewTableBundle(&suite.getter, &suite.table4)
            .unwrap()
            .is_none()
    );
}

#[test]
fn new_partition_bundle_matches_go_meta_suite() {
    let suite = create_meta_bundle_suite();
    let definitions = &suite.table1.Partition.as_ref().unwrap().Definitions;
    assert!(
        NewPartitionBundle(&suite.getter, &definitions[0])
            .unwrap()
            .is_none()
    );

    let bundle = NewPartitionBundle(&suite.getter, &definitions[1])
        .unwrap()
        .unwrap();
    assert_eq!(
        bundle,
        expected_bundle(&suite, 12, RuleIndexPartition, &[1001])
    );
    assert_eq!(bundle.ID, GroupID(1001));
    assert_eq!(bundle.Index, RuleIndexPartition);
    assert!(bundle.Override);
    assert_eq!(bundle.Rules.len(), 3);
    for rule in &bundle.Rules {
        assert_rule_range(rule, 1001, RuleIndexTable, &bundle.ID);
    }
}

#[test]
fn new_partition_list_bundles_matches_go_meta_suite() {
    let suite = create_meta_bundle_suite();
    let definitions = &suite.table1.Partition.as_ref().unwrap().Definitions;
    let bundles = NewPartitionListBundles(&suite.getter, definitions).unwrap();
    assert_eq!(bundles.len(), 1);
    assert_eq!(
        bundles[0],
        expected_bundle(&suite, 12, RuleIndexPartition, &[1001])
    );
    assert!(
        NewPartitionListBundles(&suite.getter, &[])
            .unwrap()
            .is_empty()
    );
    assert!(
        NewPartitionListBundles(
            &suite.getter,
            &[definitions[0].clone(), definitions[2].clone()]
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn new_full_table_bundles_matches_go_meta_suite() {
    let suite = create_meta_bundle_suite();
    let table1 = NewFullTableBundles(&suite.getter, &suite.table1).unwrap();
    assert_eq!(
        table1,
        vec![
            expected_bundle(&suite, 11, RuleIndexTable, &[101, 1000, 1001, 1002]),
            expected_bundle(&suite, 12, RuleIndexPartition, &[1001]),
        ]
    );

    let table2 = NewFullTableBundles(&suite.getter, &suite.table2).unwrap();
    assert_eq!(
        table2,
        vec![expected_bundle(&suite, 11, RuleIndexPartition, &[1000])]
    );

    let table3 = NewFullTableBundles(&suite.getter, &suite.table3).unwrap();
    assert_eq!(
        table3,
        vec![expected_bundle(&suite, 13, RuleIndexTable, &[103])]
    );

    assert!(
        NewFullTableBundles(&suite.getter, &suite.table4)
            .unwrap()
            .is_empty()
    );
}

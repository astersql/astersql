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

use crate::placement_policy::{
    PlacementObject, PlacementPolicyCatalog, PlacementSettings, PolicyError, PolicyInfo, PolicyRef,
    PolicyState,
};

/// 构造与 Go `testPlacementPolicyInfo` 等价的策略元信息。
fn test_placement_policy_info(id: i64, name: &str, settings: &PlacementSettings) -> PolicyInfo {
    PolicyInfo {
        id,
        name: name.into(),
        settings: settings.clone(),
        state: PolicyState::None,
    }
}

/// 提交与 Go `testCreatePlacementPolicy` 相同的创建动作，并校验公开状态。
fn test_create_placement_policy(catalog: &mut PlacementPolicyCatalog, policy: PolicyInfo) {
    let expected_name = policy.name.clone();
    catalog.create(policy, false).unwrap();
    assert_eq!(
        PolicyState::Public,
        catalog.by_name(&expected_name).unwrap().state
    );
}

/// 构造与 Go `testTableInfoWithPartition` 等价的单 range 分区对象。
fn test_table_info_with_partition(table_id: i64, partition_id: i64) -> PlacementObject {
    PlacementObject {
        id: table_id,
        policy_ref: None,
        partitions: vec![PlacementObject {
            id: partition_id,
            policy_ref: None,
            partitions: vec![],
        }],
    }
}

fn policy_ref(policy: &PolicyInfo) -> Option<PolicyRef> {
    Some(PolicyRef {
        id: policy.id,
        name: policy.name.clone(),
    })
}

#[test]
fn placement_policy_in_use_matches_info_schema_and_meta() {
    let settings = PlacementSettings {
        primary_region: "r1".into(),
        regions: "r1,r2".into(),
        ..PlacementSettings::default()
    };
    let policies: Vec<_> = (1..=5)
        .map(|id| test_placement_policy_info(id, &format!("p{id}"), &settings))
        .collect();

    // Go 分别从 InfoSchema 与事务内 Meta 检查同一组引用。这里建立两个独立目录快照，
    // 避免一次检查的状态意外影响另一次检查。
    let mut info_schema = PlacementPolicyCatalog::default();
    let mut meta = PlacementPolicyCatalog::default();
    for policy in &policies {
        test_create_placement_policy(&mut info_schema, policy.clone());
        test_create_placement_policy(&mut meta, policy.clone());
    }

    let databases = vec![
        PlacementObject {
            id: 101,
            policy_ref: None,
            partitions: vec![],
        },
        PlacementObject {
            id: 102,
            policy_ref: None,
            partitions: vec![],
        },
        PlacementObject {
            id: 103,
            policy_ref: policy_ref(&policies[3]),
            partitions: vec![],
        },
    ];
    let mut partitioned = test_table_info_with_partition(204, 304);
    partitioned.partitions[0].policy_ref = policy_ref(&policies[4]);
    let tables = vec![
        PlacementObject {
            id: 201,
            policy_ref: policy_ref(&policies[0]),
            partitions: vec![],
        },
        PlacementObject {
            id: 202,
            policy_ref: policy_ref(&policies[0]),
            partitions: vec![],
        },
        PlacementObject {
            id: 203,
            policy_ref: policy_ref(&policies[1]),
            partitions: vec![],
        },
        partitioned,
    ];
    info_schema.databases = databases.clone();
    info_schema.tables = tables.clone();
    meta.databases = databases;
    meta.tables = tables;

    for policy in [&policies[0], &policies[1], &policies[3], &policies[4]] {
        assert_eq!(
            Err(PolicyError::InUse),
            info_schema.check_not_in_use(policy.id)
        );
        assert_eq!(Err(PolicyError::InUse), meta.check_not_in_use(policy.id));
    }

    assert_eq!(Ok(()), info_schema.check_not_in_use(policies[2].id));
    assert_eq!(Ok(()), meta.check_not_in_use(policies[2].id));

    assert_eq!(
        (vec![], vec![], vec![201, 202]),
        info_schema.depended_object_ids(policies[0].id).unwrap()
    );
    assert_eq!(
        (vec![], vec![], vec![203]),
        info_schema.depended_object_ids(policies[1].id).unwrap()
    );
    assert_eq!(
        (vec![103], vec![], vec![]),
        info_schema.depended_object_ids(policies[3].id).unwrap()
    );
    assert_eq!(
        (vec![], vec![304], vec![]),
        info_schema.depended_object_ids(policies[4].id).unwrap()
    );
    assert_eq!(
        (vec![], vec![], vec![]),
        info_schema.depended_object_ids(policies[2].id).unwrap()
    );
}

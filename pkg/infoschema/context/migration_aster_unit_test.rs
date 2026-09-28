// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// InfoSchema 上下文模块的迁移单元测试。
//
// 对照 Go 可见性规则验证特殊属性过滤器（TTL / TiFlash / 放置策略 / 分区 /
// 表锁 / 亲和性），以及 `DBInfoAsInfoSchema` 适配器的按名查找与 `Arc` 共享语义。

use super::*;
use std::sync::Arc;

/// 构造带固定 ID/名称的放置策略引用，供过滤器用例复用。
fn policy_ref() -> model::PolicyRefInfo {
    model::PolicyRefInfo {
        ID: 7,
        Name: ast::NewCIStr("policy"),
    }
}

/// 验证各特殊属性过滤器与 Go 的可见性/Enable 语义一致。
#[test]
fn special_attribute_filters_match_go_visibility_rules() {
    let mut table = model::TableInfo::default();
    assert!(!HasSpecialAttributes(&table));

    table.TTLInfo = Some(model::TTLInfo::default());
    assert!(!TTLAttribute(&table), "non-public TTL tables are hidden");
    table.State = model::StatePublic;
    assert!(TTLAttribute(&table));
    assert!(AllSpecialAttribute(&table));

    table.TTLInfo = None;
    table.TiFlashReplica = Some(model::TiFlashReplicaInfo::default());
    assert!(TiFlashAttribute(&table));

    table.TiFlashReplica = None;
    table.PlacementPolicyRef = Some(policy_ref());
    assert!(PlacementPolicyAttribute(&table));
    assert!(AllPlacementPolicyAttribute(&table));

    table.PlacementPolicyRef = None;
    table.Partition = Some(model::PartitionInfo {
        Enable: false,
        Definitions: vec![model::PartitionDefinition {
            PlacementPolicyRef: Some(policy_ref()),
            ..Default::default()
        }],
        ..Default::default()
    });
    // Enable=false：PlacementPolicyAttribute / PartitionAttribute 不命中，但 AllPlacementPolicyAttribute 仍命中。
    assert!(!PlacementPolicyAttribute(&table));
    assert!(AllPlacementPolicyAttribute(&table));
    assert!(!PartitionAttribute(&table));

    table.Partition.as_mut().unwrap().Enable = true;
    assert!(PlacementPolicyAttribute(&table));
    assert!(PartitionAttribute(&table));

    table.Partition = None;
    table.Lock = Some(model::TableLockInfo::default());
    assert!(TableLockAttribute(&table));

    table.Lock = None;
    table.Affinity = Some(model::TableAffinityInfo::default());
    assert!(AffinityAttribute(&table));
    assert!(HasSpecialAttributes(&table));
}

/// 验证 DBInfo 适配器：共享 Arc 引用、命中返回表列表、未命中返回空而非错误。
#[test]
fn db_info_adapter_matches_go_lookup_and_shared_metadata_behavior() {
    let table = Arc::new(model::TableInfo {
        ID: 42,
        Name: ast::NewCIStr("orders"),
        ..Default::default()
    });
    let db = Arc::new(model::DBInfo {
        ID: 9,
        Name: ast::NewCIStr("shop"),
        Deprecated: model::DeprecatedDBInfo {
            Tables: vec![Arc::clone(&table)],
        },
        ..Default::default()
    });
    let schemas = DBInfoAsInfoSchema(vec![Arc::clone(&db)]);

    let all = schemas.AllSchemas();
    assert_eq!(all.len(), 1);
    assert!(Arc::ptr_eq(&all[0], &db));

    let found = schemas
        .SchemaTableInfos(&(), &ast::NewCIStr("shop"))
        .expect("DBInfo adapter is infallible");
    assert_eq!(found.len(), 1);
    assert!(Arc::ptr_eq(&found[0], &table));

    let missing = schemas
        .SchemaTableInfos(&(), &ast::NewCIStr("missing"))
        .expect("missing schemas are not errors");
    assert!(missing.is_empty());
}

/// 验证 context 直接复用 meta/model 的正式 DBInfo 类型，而不是维护平行定义。
#[test]
fn db_info_adapter_uses_canonical_meta_model_identity() {
    fn accept_context_db_info(_: model::DBInfo) {}

    accept_context_db_info(meta_model::group_1::DBInfo::default());
}

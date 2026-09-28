// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// SchemaCatalog 单元测试。
//
// 覆盖建库、修改字符集/Placement、分阶段删库与恢复，以及字符集校验与

use crate::schema::{SchemaCatalog, SchemaError, SchemaState, SchemaTable, schema_physical_ids};

#[test]
/// 覆盖建库（含 IF NOT EXISTS）、改字符集与 Placement、分阶段删库再恢复。
fn schema_create_modify_drop_and_recover_preserve_identity() {
    let mut catalog = SchemaCatalog::default();
    let id = catalog
        .create_schema("Test", "utf8mb4", "utf8mb4_bin", None, false)
        .unwrap()
        .unwrap();
    assert_eq!(
        None,
        catalog
            .create_schema("test", "utf8mb4", "utf8mb4_bin", None, true)
            .unwrap()
    );
    assert_eq!(
        Err(SchemaError::AlreadyExists),
        catalog.create_schema("TEST", "utf8mb4", "utf8mb4_bin", None, false)
    );

    // 修改默认字符集/排序规则与 Placement Policy。
    assert!(
        catalog
            .modify_charset_and_collation("test", "latin1", "latin1_bin")
            .unwrap()
    );
    assert!(
        catalog
            .modify_placement("test", Some("regional".into()))
            .unwrap()
    );
    assert_eq!("latin1", catalog.schema("test").unwrap().charset);

    assert_eq!(
        SchemaState::WriteOnly,
        catalog.drop_schema_step("test").unwrap()
    );
    // Public → WriteOnly → DeleteOnly → None，再按 ID 恢复。
    assert_eq!(
        SchemaState::DeleteOnly,
        catalog.drop_schema_step("test").unwrap()
    );
    assert_eq!(SchemaState::None, catalog.drop_schema_step("test").unwrap());
    assert!(catalog.schema("test").is_none());
    catalog.recover_schema(id).unwrap();
    assert_eq!(id, catalog.schema("test").unwrap().id);
    assert_eq!(SchemaState::Public, catalog.schema("test").unwrap().state);
}

#[test]
/// 校验非法字符集/空 Placement，以及 schema_physical_ids 收集表与分区 ID。
fn schema_validation_and_physical_ids_match_go_helpers() {
    let mut catalog = SchemaCatalog::default();
    assert_eq!(
        Err(SchemaError::InvalidCharsetCollation),
        catalog.create_schema("bad", "utf8", "latin1_bin", None, false)
    );
    assert_eq!(
        Err(SchemaError::InvalidPlacementPolicy),
        catalog.create_schema("bad", "utf8", "utf8_bin", Some(" ".into()), false)
    );
    assert_eq!(
        // 表 10 带分区 11/12，表 20 无分区：期望 [10,11,12,20]。
        vec![10, 11, 12, 20],
        schema_physical_ids(&[
            SchemaTable {
                id: 10,
                partition_ids: vec![11, 12]
            },
            SchemaTable {
                id: 20,
                partition_ids: vec![]
            },
        ])
    );
}

#[test]
fn schema_modify_reports_missing_schema_before_invalid_new_values_like_go() {
    let mut catalog = SchemaCatalog::default();

    assert_eq!(
        Err(SchemaError::NotFound),
        catalog.modify_charset_and_collation("missing", "unknown", "also_unknown")
    );
    assert_eq!(
        Err(SchemaError::NotFound),
        catalog.modify_placement("missing", Some(" ".into()))
    );
}

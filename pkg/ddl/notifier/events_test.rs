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

// Schema 变更事件序列化与可读字符串格式的单元测试。
//
// 覆盖 `SchemaChangeEvent::String` 对表/分区/列/索引字段的拼接格式，
// 以及 `MarshalJSON` / `UnmarshalJSON` 往返后载荷保持一致。

use crate::{JsonSchemaChangeEvent, SchemaChangeEvent, ast, model};

#[test]
/// 构造含表、旧表、增减分区、列与索引的事件，断言 `String()` 输出格式。
fn test_event_string() {
    let event = SchemaChangeEvent {
        inner: Some(JsonSchemaChangeEvent {
            Tp: model::ACTION_ADD_COLUMN,
            TableInfo: Some(Box::new(model::TableInfo {
                ID: 1,
                Name: ast::NewCIStr("Table1"),
                ..Default::default()
            })),
            OldTableInfo: Some(Box::new(model::TableInfo {
                ID: 4,
                Name: ast::NewCIStr("Table2"),
                ..Default::default()
            })),
            AddedPartInfo: Some(Box::new(model::PartitionInfo {
                Definitions: vec![
                    model::PartitionDefinition {
                        ID: 2,
                        ..Default::default()
                    },
                    model::PartitionDefinition {
                        ID: 3,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })),
            DroppedPartInfo: Some(Box::new(model::PartitionInfo {
                Definitions: vec![
                    model::PartitionDefinition {
                        ID: 5,
                        ..Default::default()
                    },
                    model::PartitionDefinition {
                        ID: 6,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            })),
            Columns: vec![
                Box::new(model::ColumnInfo {
                    ID: 7,
                    Name: ast::NewCIStr("Column1"),
                    ..Default::default()
                }),
                Box::new(model::ColumnInfo {
                    ID: 8,
                    Name: ast::NewCIStr("Column2"),
                    ..Default::default()
                }),
            ],
            Indexes: vec![
                Box::new(model::IndexInfo {
                    ID: 9,
                    Name: ast::NewCIStr("Index1"),
                    ..Default::default()
                }),
                Box::new(model::IndexInfo {
                    ID: 10,
                    Name: ast::NewCIStr("Index2"),
                    ..Default::default()
                }),
            ],
            ..Default::default()
        }),
    };
    assert_eq!(
        event.String(),
        "(Event Type: add column, Table ID: 1, Table Name: Table1, Old Table ID: 4, Old Table Name: Table2, Partition ID: 2, Partition ID: 3, Dropped Partition ID: 5, Dropped Partition ID: 6, Column ID: 7, Column Name: Column1, Column ID: 8, Column Name: Column2, Index ID: 9, Index Name: Index1, Index ID: 10, Index Name: Index2)"
    );
}

#[test]
/// 验证 `NewAddColumnEvent` 经 JSON 编解码后仍能取出加列信息且与原事件相等。
fn event_json_round_trip_preserves_notifier_payload() {
    let original = crate::NewAddColumnEvent(
        Some(Box::new(model::TableInfo {
            ID: 41,
            Name: ast::NewCIStr("t"),
            Columns: vec![model::ColumnInfo {
                ID: 7,
                Name: ast::NewCIStr("old"),
                ..Default::default()
            }],
            ..Default::default()
        })),
        vec![Box::new(model::ColumnInfo {
            ID: 8,
            Name: ast::NewCIStr("new"),
            ..Default::default()
        })],
    );
    let bytes = original.MarshalJSON().unwrap();
    let mut decoded = SchemaChangeEvent::default();
    decoded.UnmarshalJSON(&bytes).unwrap();
    let (table, columns) = decoded.GetAddColumnInfo();
    assert_eq!(table.unwrap().ID, 41);
    assert_eq!(columns[0].Name.O, "new");
    assert_eq!(decoded, original);
}

#[test]
fn event_string_uses_model_action_name_for_other_action_types() {
    let event = SchemaChangeEvent {
        inner: Some(JsonSchemaChangeEvent {
            Tp: model::ACTION_DROP_INDEX,
            ..Default::default()
        }),
    };
    assert_eq!(event.String(), "(Event Type: drop index)");
}

#[test]
fn event_json_preserves_complete_go_model_payload() {
    let mut field_type = model::types::NewFieldType(model::mysql::TypeVarchar);
    field_type.SetFlen(128);
    field_type.SetCharset("utf8mb4".to_owned());
    field_type.SetCollate("utf8mb4_bin".to_owned());
    let table = model::TableInfo {
        ID: 42,
        Name: ast::NewCIStr("orders"),
        Charset: "utf8mb4".to_owned(),
        Collate: "utf8mb4_bin".to_owned(),
        Comment: "complete table payload".to_owned(),
        State: model::StatePublic,
        Columns: vec![model::ColumnInfo {
            ID: 7,
            Name: ast::NewCIStr("customer"),
            FieldType: field_type,
            DefaultIsExpr: true,
            Comment: "column comment".to_owned(),
            Hidden: true,
            ..Default::default()
        }],
        Indices: vec![model::IndexInfo {
            ID: 8,
            Name: ast::NewCIStr("idx_customer"),
            Unique: true,
            Primary: true,
            Columns: vec![model::IndexColumn {
                Name: ast::NewCIStr("customer"),
                Offset: 0,
                Length: 16,
                ..Default::default()
            }],
            ..Default::default()
        }],
        Partition: Some(model::PartitionInfo {
            Expr: "`id`".to_owned(),
            Definitions: vec![model::PartitionDefinition {
                ID: 9,
                Name: ast::NewCIStr("p0"),
                LessThan: vec!["100".to_owned()],
                Comment: "partition comment".to_owned(),
                ..Default::default()
            }],
            ..Default::default()
        }),
        PlacementPolicyRef: Some(model::PolicyRefInfo {
            ID: 10,
            Name: ast::NewCIStr("primary"),
        }),
        TTLInfo: Some(model::TTLInfo {
            ColumnName: ast::NewCIStr("created_at"),
            IntervalExprStr: "7".to_owned(),
            IntervalTimeUnit: 5,
            Enable: true,
            JobInterval: "1h".to_owned(),
        }),
        ..Default::default()
    };
    let original = crate::NewCreateTableEvent(Some(Box::new(table)));

    let encoded = original.MarshalJSON().unwrap();
    let json: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(json["table_info"]["id"], 42);
    assert_eq!(json["table_info"]["charset"], "utf8mb4");
    assert_eq!(json["table_info"]["cols"][0]["comment"], "column comment");
    assert_eq!(json["table_info"]["index_info"][0]["is_unique"], true);
    assert_eq!(
        json["table_info"]["partition"]["definitions"][0]["less_than"][0],
        "100"
    );
    assert_eq!(json["table_info"]["policy_ref_info"]["id"], 10);
    assert_eq!(json["table_info"]["ttl_info"]["interval_expr"], "7");

    let mut decoded = SchemaChangeEvent::default();
    decoded.UnmarshalJSON(&encoded).unwrap();
    let table = decoded.GetCreateTableInfo().unwrap();
    assert_eq!(table.Charset, "utf8mb4");
    assert_eq!(table.Comment, "complete table payload");
    assert_eq!(table.State, model::StatePublic);
    assert_eq!(table.Columns[0].FieldType.GetFlen(), 128);
    assert!(table.Columns[0].DefaultIsExpr);
    assert_eq!(table.Columns[0].Comment, "column comment");
    assert!(table.Columns[0].Hidden);
    assert!(table.Indices[0].Unique);
    assert!(table.Indices[0].Primary);
    assert_eq!(table.Indices[0].Columns[0].Length, 16);
    assert_eq!(
        table.Partition.unwrap().Definitions[0].LessThan,
        vec!["100"]
    );
    assert_eq!(table.PlacementPolicyRef.unwrap().Name.O, "primary");
    assert_eq!(table.TTLInfo.unwrap().JobInterval, "1h");
}

#[test]
fn event_json_null_and_empty_mini_slices_match_go() {
    let mut decoded = crate::NewCreateTableEvent(None);
    decoded.UnmarshalJSON(b"null").unwrap();
    assert_eq!(decoded.GetType(), model::ActionNone);
    assert_eq!(decoded.String(), "(Event Type: none)");

    let drop_schema = crate::NewDropSchemaEvent(
        &model::DBInfo {
            ID: 1,
            Name: ast::NewCIStr("empty"),
            ..Default::default()
        },
        Vec::new(),
    );
    let json: serde_json::Value =
        serde_json::from_slice(&drop_schema.MarshalJSON().unwrap()).unwrap();
    assert!(json["mini_db_info"].get("tables").is_none());
}

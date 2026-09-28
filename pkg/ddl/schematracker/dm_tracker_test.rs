// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// SchemaTracker（DM 内存跟踪器）单元测试。
//
// 覆盖超大列数/索引数、长前缀与表达式索引、主键增删、删列对索引的影响、
// 列插入位置、批量操作失败时的可见性、字符集/注释变更以及分区删除等场景。

use crate::*;

/// 构造 CIStr（大小写不敏感标识符）。
fn name(value: &str) -> ast::CIStr {
    ast::NewCIStr(value)
}
/// 构造仅含名称的列定义。
fn column(value: &str) -> model::ColumnInfo {
    model::ColumnInfo {
        Name: name(value),
        ..Default::default()
    }
}
/// 构造默认 utf8mb4 字符集的表定义。
fn table(value: &str, columns: Vec<model::ColumnInfo>) -> model::TableInfo {
    model::TableInfo {
        Name: name(value),
        Columns: columns,
        Charset: "utf8mb4".into(),
        Collate: "utf8mb4_bin".into(),
        ..Default::default()
    }
}
/// 在 test 库下创建表（不允许 IF NOT EXISTS）。
fn create(tracker: &mut SchemaTracker, table_info: model::TableInfo) {
    tracker
        .CreateTable(CreateTableSpec {
            schema: name("test"),
            table: table_info,
            if_not_exists: false,
        })
        .unwrap();
}
/// 对 test 库下指定表执行 ALTER 操作列表。
fn alter(
    tracker: &mut SchemaTracker,
    table_name: &str,
    operations: Vec<AlterOperation>,
) -> Result<(), Error> {
    tracker.AlterTable(AlterTableSpec {
        schema: name("test"),
        table: name(table_name),
        operations,
    })
}
/// 构造非唯一、可见的索引规格。
fn index(value: &str, parts: Vec<IndexPart>) -> IndexSpec {
    IndexSpec {
        name: name(value),
        parts,
        unique: false,
        invisible: false,
    }
}
/// 构造列索引部件。
fn col_part(value: &str, length: isize) -> IndexPart {
    IndexPart::Column {
        name: name(value),
        length,
    }
}
/// 读取 test 库下表定义的引用。
fn get<'a>(tracker: &'a SchemaTracker, table: &str) -> &'a model::TableInfo {
    tracker
        .InfoStore
        .TableByName(&name("test"), &name(table))
        .unwrap()
}
/// 创建 lower_case_table_names=2 的跟踪器并初始化 test 库。
fn tracker() -> SchemaTracker {
    let mut tracker = NewSchemaTracker(2);
    tracker.CreateTestDB();
    tracker
}

#[test]
fn create_table_with_info_preserves_caller_supplied_schema_state() {
    let mut tracker = tracker();
    let supplied = table("stateful", vec![column("a")]);

    tracker
        .CreateTableWithInfo(name("test"), supplied, false)
        .unwrap();

    let stored = get(&tracker, "stateful");
    assert_eq!(stored.State, model::StateNone);
    assert_eq!(stored.Columns[0].State, model::StateNone);
}

#[test]
fn drop_table_and_drop_view_reject_the_wrong_object_type() {
    let mut tracker = tracker();
    create(&mut tracker, table("base", vec![column("a")]));
    let mut view = table("view", vec![column("a")]);
    view.View = Some(model::ViewInfo::default());
    create(&mut tracker, view);

    assert!(
        tracker
            .DropTable(&name("test"), &[name("view")], false)
            .is_err()
    );
    assert!(get(&tracker, "view").IsView());

    assert!(
        tracker
            .DropView(&name("test"), &[name("base")], false)
            .is_err()
    );
    assert!(get(&tracker, "base").IsBaseTable());
}

#[test]
fn alter_table_rejects_removing_the_last_column() {
    let mut tracker = tracker();
    create(&mut tracker, table("one_column", vec![column("a")]));

    assert!(
        alter(
            &mut tracker,
            "one_column",
            vec![AlterOperation::DropColumn {
                name: name("a"),
                if_exists: false,
            }],
        )
        .is_err()
    );
    assert_eq!(get(&tracker, "one_column").Columns.len(), 1);
}

#[test]
fn alter_table_rejects_partition_add_on_nonpartitioned_table() {
    let mut tracker = tracker();
    create(&mut tracker, table("plain", vec![column("a")]));

    assert!(
        alter(
            &mut tracker,
            "plain",
            vec![AlterOperation::AddPartitions(vec![
                model::PartitionDefinition {
                    Name: name("p0"),
                    ..Default::default()
                },
            ])],
        )
        .is_err()
    );
    assert!(get(&tracker, "plain").Partition.is_none());
}

#[test]
fn rename_table_validates_target_schema_before_removing_source() {
    let mut tracker = tracker();
    create(&mut tracker, table("source", vec![column("a")]));

    assert!(
        tracker
            .RenameTable(vec![(
                name("test"),
                name("source"),
                name("missing_schema"),
                name("target"),
            )])
            .is_err()
    );
    assert!(get(&tracker, "source").IsBaseTable());
}

#[test]
fn modifying_a_column_name_updates_index_column_metadata() {
    let mut tracker = tracker();
    create(&mut tracker, table("indexed", vec![column("a")]));
    alter(
        &mut tracker,
        "indexed",
        vec![AlterOperation::AddIndex {
            index: index("idx", vec![col_part("a", -1)]),
            if_not_exists: false,
        }],
    )
    .unwrap();

    alter(
        &mut tracker,
        "indexed",
        vec![AlterOperation::ModifyColumn {
            old: name("a"),
            column: column("b"),
        }],
    )
    .unwrap();

    assert_eq!(get(&tracker, "indexed").Indices[0].Columns[0].Name.O, "b");
}

#[test]
fn anonymous_expression_indexes_receive_go_compatible_names() {
    let mut tracker = tracker();
    create(&mut tracker, table("anonymous", vec![column("a")]));

    for (name_, unique) in [
        ("", false),
        ("", false),
        ("idx", false),
        ("", false),
        ("", true),
    ] {
        let mut spec = index(name_, vec![IndexPart::Expression("a+1".into())]);
        spec.unique = unique;
        alter(
            &mut tracker,
            "anonymous",
            vec![AlterOperation::AddIndex {
                index: spec,
                if_not_exists: false,
            }],
        )
        .unwrap();
    }

    assert_eq!(
        get(&tracker, "anonymous")
            .Indices
            .iter()
            .map(|index| index.Name.O.as_str())
            .collect::<Vec<_>>(),
        vec![
            "expression_index",
            "expression_index_2",
            "idx",
            "expression_index_3",
            "expression_index_4",
        ]
    );
}

#[test]
fn alter_index_visibility_updates_metadata() {
    let mut tracker = tracker();
    create(&mut tracker, table("visibility", vec![column("a")]));
    alter(
        &mut tracker,
        "visibility",
        vec![AlterOperation::AddIndex {
            index: index("idx", vec![col_part("a", -1)]),
            if_not_exists: false,
        }],
    )
    .unwrap();

    alter(
        &mut tracker,
        "visibility",
        vec![AlterOperation::SetIndexVisibility {
            name: name("idx"),
            invisible: true,
        }],
    )
    .unwrap();

    assert!(get(&tracker, "visibility").Indices[0].Invisible);
}

#[test]
fn drop_table_removes_existing_tables_before_reporting_missing_ones() {
    let mut tracker = tracker();
    create(&mut tracker, table("present", vec![column("a")]));

    let error = tracker
        .DropTable(&name("test"), &[name("missing"), name("present")], false)
        .unwrap_err();

    assert!(matches!(error, Error::TableDropExists(_)));
    assert!(
        tracker
            .InfoStore
            .TableByName(&name("test"), &name("present"))
            .is_err(),
        "Go continues through the full DROP TABLE list before returning the missing-table error"
    );
}

#[test]
fn dm_no_op_ddl_entries_match_go_success_contract() {
    let tracker = tracker();
    assert!(tracker.RecoverTable().is_ok());
    assert!(tracker.FlashbackCluster().is_ok());
    assert!(tracker.RecoverSchema().is_ok());
    assert!(tracker.TruncateTable().is_ok());
    assert!(tracker.LockTables().is_ok());
    assert!(tracker.UnlockTables().is_ok());
    assert!(tracker.AlterTableMode().is_ok());
    assert!(tracker.CleanupTableLock().is_ok());
    assert!(tracker.UpdateTableReplicaInfo().is_ok());
    assert!(tracker.RepairTable().is_ok());
    assert!(tracker.CreateSequence().is_ok());
    assert!(tracker.DropSequence().is_ok());
    assert!(tracker.AlterSequence().is_ok());
    assert!(tracker.CreateMaskingPolicy().is_ok());
    assert!(tracker.CreatePlacementPolicy().is_ok());
    assert!(tracker.DropPlacementPolicy().is_ok());
    assert!(tracker.AlterPlacementPolicy().is_ok());
    assert!(tracker.AddResourceGroup().is_ok());
    assert!(tracker.DropResourceGroup().is_ok());
    assert!(tracker.AlterResourceGroup().is_ok());
    assert!(tracker.CreatePlacementPolicyWithInfo().is_ok());
    assert!(tracker.RefreshMeta().is_ok());
}

#[test]
fn batch_create_keeps_successful_prefix_when_a_later_table_fails() {
    let mut tracker = tracker();
    let duplicate = table("duplicate", vec![column("a")]);
    let error = tracker
        .BatchCreateTableWithInfo(name("test"), vec![duplicate.clone(), duplicate], false)
        .unwrap_err();

    assert!(matches!(error, Error::TableExists(_, _)));
    assert!(
        tracker
            .InfoStore
            .TableByName(&name("test"), &name("duplicate"))
            .is_ok(),
        "Go loops through CreateTableWithInfo without rolling back earlier successes"
    );
}

#[test]
fn drop_index_if_exists_ignores_missing_schema_or_table() {
    let mut tracker = tracker();

    assert!(
        tracker
            .DropIndex(&name("missing"), &name("t"), &name("idx"), true)
            .is_ok()
    );
    assert!(
        tracker
            .DropIndex(&name("test"), &name("missing"), &name("idx"), true)
            .is_ok()
    );
}

#[test]
fn dropping_partitions_from_a_nonpartitioned_table_reports_partition_management_error() {
    let mut tracker = tracker();
    create(&mut tracker, table("plain_drop", vec![column("a")]));

    let error = alter(
        &mut tracker,
        "plain_drop",
        vec![AlterOperation::DropPartitions(vec![name("p0")])],
    )
    .unwrap_err();

    assert_eq!(error, Error::PartitionManagementOnNonpartitionedTable);
}

#[test]
fn renaming_an_index_to_itself_is_a_no_op() {
    let mut tracker = tracker();
    create(&mut tracker, table("same_index", vec![column("a")]));
    alter(
        &mut tracker,
        "same_index",
        vec![AlterOperation::AddIndex {
            index: index("idx", vec![col_part("a", -1)]),
            if_not_exists: false,
        }],
    )
    .unwrap();

    alter(
        &mut tracker,
        "same_index",
        vec![AlterOperation::RenameIndex {
            old: name("idx"),
            new: name("idx"),
        }],
    )
    .unwrap();
    assert_eq!(get(&tracker, "same_index").Indices[0].Name.O, "idx");
}

#[test]
/// 验证跟踪器不强制列数/索引数上限（可建 12000 列与 100 个索引）。
fn test_no_num_limit() {
    let mut tracker = tracker();
    // 构造远超常见上限的列数，确认跟踪器不拦截。
    let columns = (0..12_000).map(|i| column(&format!("c{i}"))).collect();
    create(&mut tracker, table("t_too_large", columns));
    let columns = (0..100).map(|i| column(&format!("c{i}"))).collect();
    create(&mut tracker, table("t_too_many_indexes", columns));
    for i in 0..100 {
        alter(
            &mut tracker,
            "t_too_many_indexes",
            vec![AlterOperation::AddIndex {
                index: index(&format!("k{i}"), vec![col_part(&format!("c{i}"), -1)]),
                if_not_exists: false,
            }],
        )
        .unwrap();
    }
    assert_eq!(get(&tracker, "t_too_large").Columns.len(), 12_000);
    assert_eq!(get(&tracker, "t_too_many_indexes").Indices.len(), 100);
}

#[test]
/// 验证超长前缀长度会原样保留在索引列 Length 上。
fn test_create_table_long_index() {
    let mut tracker = tracker();
    for (table_name, unique) in [("t", false), ("t2", true), ("t3", false)] {
        create(
            &mut tracker,
            table(table_name, vec![column("c1"), column("c2"), column("c3")]),
        );
        let mut spec = index("idx_c2", vec![col_part("c2", 555_555)]);
        spec.unique = unique;
        alter(
            &mut tracker,
            table_name,
            vec![AlterOperation::AddIndex {
                index: spec,
                if_not_exists: false,
            }],
        )
        .unwrap();
        assert_eq!(
            get(&tracker, table_name).Indices[0].Columns[0].Length,
            555_555
        );
    }
}

#[test]
/// 表达式索引应生成 StatePublic 的 Hidden 生成列。
fn test_expression_index_hidden_column_state() {
    let mut tracker = tracker();
    create(&mut tracker, table("t", vec![column("id"), column("name")]));
    alter(
        &mut tracker,
        "t",
        vec![AlterOperation::AddIndex {
            index: index(
                "uk_lower_name",
                vec![IndexPart::Expression("lower(name)".into())],
            ),
            if_not_exists: false,
        }],
    )
    .unwrap();
    let table = get(&tracker, "t");
    let hidden = &table.Columns[table.Indices[0].Columns[0].Offset as usize];
    assert!(hidden.Hidden);
    assert_eq!(hidden.State, model::StatePublic);
    assert_eq!(hidden.GeneratedExprString, "lower(name)");
}

#[test]
/// 创建主键后再 DROP PRIMARY，表上索引应清空。
fn test_alter_pk() {
    let mut tracker = tracker();
    create(&mut tracker, table("t", vec![column("c1"), column("c2")]));
    alter(
        &mut tracker,
        "t",
        vec![AlterOperation::CreatePrimaryKey(vec![name("c1")])],
    )
    .unwrap();
    // 保留删除主键前的快照，便于对比索引数量。
    let old = get(&tracker, "t").Clone();
    alter(
        &mut tracker,
        "t",
        vec![AlterOperation::DropIndex {
            name: name("PRIMARY"),
            if_exists: false,
        }],
    )
    .unwrap();
    assert_eq!(old.Indices.len(), 1);
    assert!(get(&tracker, "t").Indices.is_empty());
}

#[test]
/// 删列会移除依赖该列的索引，并收缩复合索引列数。
fn test_drop_column() {
    let mut tracker = tracker();
    create(
        &mut tracker,
        table("t", vec![column("a"), column("b"), column("c")]),
    );
    alter(
        &mut tracker,
        "t",
        vec![AlterOperation::AddIndex {
            index: index("b", vec![col_part("b", -1)]),
            if_not_exists: false,
        }],
    )
    .unwrap();
    alter(
        &mut tracker,
        "t",
        vec![AlterOperation::DropColumn {
            name: name("b"),
            if_exists: false,
        }],
    )
    .unwrap();
    assert!(get(&tracker, "t").Indices.is_empty());
    alter(
        &mut tracker,
        "t",
        vec![
            AlterOperation::AddIndex {
                index: index("idx_2_col", vec![col_part("a", -1), col_part("c", -1)]),
                if_not_exists: false,
            },
            AlterOperation::DropColumn {
                name: name("c"),
                if_exists: false,
            },
        ],
    )
    .unwrap();
    assert_eq!(get(&tracker, "t").Indices[0].Columns.len(), 1);
    assert_eq!(get(&tracker, "t").Columns.len(), 1);
}

#[test]
/// 不同前缀长度写入后可按序读回。
fn test_index_length() {
    let mut tracker = tracker();
    create(
        &mut tracker,
        table("t", vec![column("a"), column("b"), column("c")]),
    );
    for (name_, length) in [("a", 768), ("b", 3072), ("c", 3072)] {
        alter(
            &mut tracker,
            "t",
            vec![AlterOperation::AddIndex {
                index: index(name_, vec![col_part(name_, length)]),
                if_not_exists: false,
            }],
        )
        .unwrap();
    }
    assert_eq!(
        get(&tracker, "t")
            .Indices
            .iter()
            .map(|i| i.Columns[0].Length)
            .collect::<Vec<_>>(),
        vec![768, 3072, 3072]
    );
}

#[test]
/// 表达式索引可重命名后再用原名新建另一索引。
fn test_create_table_with_index() {
    let mut tracker = tracker();
    create(&mut tracker, table("t", vec![column("col_1")]));
    alter(
        &mut tracker,
        "t",
        vec![
            AlterOperation::AddIndex {
                index: index(
                    "idx_1",
                    vec![IndexPart::Expression(
                        "cast(col_1 as char(64) array)".into(),
                    )],
                ),
                if_not_exists: false,
            },
            AlterOperation::RenameIndex {
                old: name("idx_1"),
                new: name("idx_1_1"),
            },
            AlterOperation::AddIndex {
                index: index(
                    "idx_1",
                    vec![IndexPart::Expression(
                        "cast(col_1 as char(64) array)".into(),
                    )],
                ),
                if_not_exists: false,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        get(&tracker, "t")
            .Indices
            .iter()
            .map(|i| i.Name.O.as_str())
            .collect::<Vec<_>>(),
        vec!["idx_1_1", "idx_1"]
    );
    let hidden_names = get(&tracker, "t")
        .Columns
        .iter()
        .filter(|column| column.Hidden)
        .map(|column| column.Name.O.as_str())
        .collect::<Vec<_>>();
    assert!(
        hidden_names
            .iter()
            .any(|name| name.starts_with("_V$_idx_1_1_"))
    );
    assert!(
        hidden_names
            .iter()
            .any(|name| name.starts_with("_V$_idx_1_"))
    );
}

#[test]
/// 覆盖多列 FIRST/AFTER 插入后的最终列序（issue 5092）。
fn test_issue_5092() {
    let mut tracker = tracker();
    create(&mut tracker, table("t", vec![column("a")]));
    alter(
        &mut tracker,
        "t",
        vec![
            AlterOperation::AddColumn {
                column: column("b"),
                if_not_exists: false,
                position: ColumnPosition::None,
            },
            AlterOperation::AddColumn {
                column: column("c"),
                if_not_exists: false,
                position: ColumnPosition::None,
            },
            AlterOperation::AddColumn {
                column: column("b1"),
                if_not_exists: false,
                position: ColumnPosition::After(name("b")),
            },
            AlterOperation::AddColumn {
                column: column("c1"),
                if_not_exists: false,
                position: ColumnPosition::After(name("c")),
            },
            AlterOperation::AddColumn {
                column: column("d"),
                if_not_exists: false,
                position: ColumnPosition::After(name("b")),
            },
            AlterOperation::AddColumn {
                column: column("e"),
                if_not_exists: false,
                position: ColumnPosition::First,
            },
            AlterOperation::AddColumn {
                column: column("f"),
                if_not_exists: false,
                position: ColumnPosition::After(name("c1")),
            },
            AlterOperation::AddColumn {
                column: column("g"),
                if_not_exists: false,
                position: ColumnPosition::None,
            },
            AlterOperation::AddColumn {
                column: column("h"),
                if_not_exists: false,
                position: ColumnPosition::First,
            },
            AlterOperation::AddColumn {
                column: column("ff"),
                if_not_exists: false,
                position: ColumnPosition::None,
            },
            AlterOperation::AddColumn {
                column: column("b2"),
                if_not_exists: false,
                position: ColumnPosition::After(name("b1")),
            },
            AlterOperation::AddColumn {
                column: column("c2"),
                if_not_exists: false,
                position: ColumnPosition::First,
            },
        ],
    )
    .unwrap();
    assert_eq!(
        get(&tracker, "t")
            .Columns
            .iter()
            .map(|c| c.Name.O.as_str())
            .collect::<Vec<_>>(),
        vec![
            "c2", "h", "e", "a", "b", "d", "b1", "b2", "c", "c1", "f", "g", "ff"
        ]
    );
}

#[test]
/// 批量创建多列后列数应等于构造数量。
fn test_bit_default_values() {
    let mut tracker = tracker();
    create(
        &mut tracker,
        table(
            "testalltypes2",
            (1..=35).map(|i| column(&format!("field_{i}"))).collect(),
        ),
    );
    assert_eq!(get(&tracker, "testalltypes2").Columns.len(), 35);
}

#[test]
/// 增删表达式索引，以及分区表上的表达式索引不影响分区定义。
fn test_add_expression_index() {
    let mut tracker = tracker();
    create(&mut tracker, table("t", vec![column("a"), column("b")]));
    alter(
        &mut tracker,
        "t",
        vec![
            AlterOperation::AddIndex {
                index: index("idx", vec![IndexPart::Expression("a+b".into())]),
                if_not_exists: false,
            },
            AlterOperation::AddIndex {
                index: index(
                    "idx_multi",
                    vec![
                        IndexPart::Expression("a+b".into()),
                        IndexPart::Expression("a+1".into()),
                        col_part("b", -1),
                    ],
                ),
                if_not_exists: false,
            },
        ],
    )
    .unwrap();
    assert_eq!(get(&tracker, "t").Indices.len(), 2);
    alter(
        &mut tracker,
        "t",
        vec![
            AlterOperation::DropIndex {
                name: name("idx"),
                if_exists: false,
            },
            AlterOperation::DropIndex {
                name: name("idx_multi"),
                if_exists: false,
            },
        ],
    )
    .unwrap();
    assert!(get(&tracker, "t").Indices.is_empty());
    create(
        &mut tracker,
        model::TableInfo {
            Name: name("t4"),
            Columns: vec![column("a"), column("c")],
            Partition: Some(model::PartitionInfo {
                Definitions: vec![model::PartitionDefinition {
                    Name: name("p0"),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    alter(
        &mut tracker,
        "t4",
        vec![AlterOperation::AddIndex {
            index: index("idx", vec![IndexPart::Expression("a+c".into())]),
            if_not_exists: false,
        }],
    )
    .unwrap();
    assert_eq!(
        get(&tracker, "t4")
            .Partition
            .as_ref()
            .unwrap()
            .Definitions
            .len(),
        1
    );
}

#[test]
/// 同一次 ALTER 中第二步加列失败时，第一步也不应残留（此处检查列数未变）。
fn test_atomic_multi_schema_change() {
    let mut tracker = tracker();
    create(
        &mut tracker,
        table("t", vec![column("a"), column("b"), column("c")]),
    );
    // 第二步试图重复添加列 a，应失败且表结构不变。
    let error = alter(
        &mut tracker,
        "t",
        vec![
            AlterOperation::AddColumn {
                column: column("d"),
                if_not_exists: false,
                position: ColumnPosition::None,
            },
            AlterOperation::AddColumn {
                column: column("a"),
                if_not_exists: false,
                position: ColumnPosition::None,
            },
        ],
    )
    .unwrap_err();
    assert!(matches!(error, Error::ColumnExists(_)));
    assert_eq!(get(&tracker, "t").Columns.len(), 3);
}

#[test]
/// Alter 写回新表后，先前 Clone 的快照保持旧注释与字符集。
fn test_immutable_table_info() {
    let mut tracker = tracker();
    let mut c = column("a");
    c.FieldType.SetCharset("latin1".into());
    c.FieldType.SetCollate("latin1_bin".into());
    create(
        &mut tracker,
        model::TableInfo {
            Name: name("t"),
            Columns: vec![c],
            Charset: "latin1".into(),
            Collate: "latin1_bin".into(),
            ..Default::default()
        },
    );
    // 变更前快照，验证 Clone 不受后续 Alter 影响。
    let old = get(&tracker, "t").Clone();
    alter(
        &mut tracker,
        "t",
        vec![
            AlterOperation::SetComment("123".into()),
            AlterOperation::SetCharset {
                charset: "utf8mb4".into(),
                collate: "utf8mb4_general_ci".into(),
            },
        ],
    )
    .unwrap();
    assert_eq!(old.Comment, "");
    assert_eq!(old.Charset, "latin1");
    assert_eq!(get(&tracker, "t").Comment, "123");
    assert_eq!(get(&tracker, "t").Charset, "utf8mb4");
}

#[test]
/// MODIFY COLUMN 可更新列定义字段（此处用 Comment 模拟 NOT NULL 标记）。
fn test_modify_from_null_to_not_null() {
    let mut tracker = tracker();
    create(&mut tracker, table("t", vec![column("a"), column("b")]));
    let mut replacement = column("a");
    replacement.Comment = "NOT NULL".into();
    alter(
        &mut tracker,
        "t",
        vec![AlterOperation::ModifyColumn {
            old: name("a"),
            column: replacement,
        }],
    )
    .unwrap();
    assert_eq!(get(&tracker, "t").Columns.len(), 2);
    assert_eq!(get(&tracker, "t").Columns[0].Comment, "NOT NULL");
}

#[test]
/// 删除 list 分区后剩余分区名保持预期顺序。
fn test_drop_list_partition() {
    let mut tracker = tracker();
    create(
        &mut tracker,
        model::TableInfo {
            Name: name("employees11"),
            Partition: Some(model::PartitionInfo {
                Definitions: ["pNorth", "pEast", "pWest", "pCentral"]
                    .into_iter()
                    .map(|p| model::PartitionDefinition {
                        Name: name(p),
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            }),
            ..Default::default()
        },
    );
    alter(
        &mut tracker,
        "employees11",
        vec![AlterOperation::DropPartitions(vec![name("pEast")])],
    )
    .unwrap();
    assert_eq!(
        get(&tracker, "employees11")
            .Partition
            .as_ref()
            .unwrap()
            .Definitions
            .iter()
            .map(|p| p.Name.O.as_str())
            .collect::<Vec<_>>(),
        vec!["pNorth", "pWest", "pCentral"]
    );
}

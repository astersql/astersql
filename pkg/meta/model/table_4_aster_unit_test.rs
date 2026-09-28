// Copyright 2024 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TableInfo / PartitionInfo / FKInfo / TTL 等行为的单元测试。
//
// 对齐 Go 语义：public 列 Offset 槽位、MoveColumnInfo 级联更新、隐式主键选择、
// 分区 DDL 重叠与忽略 ID、外键 SHOW CREATE 片段，以及 TTL/亲和性/零值 String。

use super::*;

/// 构造带指定 ID/名/Offset/状态/标志的测试列。
fn column(id: i64, name: &str, offset: isize, state: SchemaState, flags: usize) -> ColumnInfo {
    let mut column = ColumnInfo {
        ID: id,
        Name: ast::NewCIStr(name),
        Offset: offset,
        State: state,
        ..ColumnInfo::default()
    };
    column.SetFlag(flags);
    column
}

/// 构造测试索引；columns 为 (列名, Offset) 列表。
fn index(id: i64, name: &str, columns: &[(&str, isize)], primary: bool, unique: bool) -> IndexInfo {
    IndexInfo {
        ID: id,
        Name: ast::NewCIStr(name),
        Columns: columns
            .iter()
            .map(|(name, offset)| IndexColumn {
                Name: ast::NewCIStr(name),
                Offset: *offset,
                ..IndexColumn::default()
            })
            .collect(),
        Primary: primary,
        Unique: unique,
        ..IndexInfo::default()
    }
}

/// 对齐 Go checkOffsets：同时校验列顺序、自身 Offset 及所有同名索引列 Offset。
fn assert_offsets(table: &TableInfo, ids: &[i64]) {
    assert_eq!(table.Columns.len(), ids.len());
    for (offset, id) in ids.iter().enumerate() {
        let expected_name = format!("c_{id}");
        assert_eq!(table.Columns[offset].Name.L, expected_name);
        assert_eq!(table.Columns[offset].Offset, offset as isize);
        for index in &table.Indices {
            for index_column in &index.Columns {
                if index_column.Name.L == expected_name {
                    assert_eq!(index_column.Offset, offset as isize);
                }
            }
        }
    }
}

/// 构造仅含 ID/名的分区定义。
fn partition_definition(id: i64, name: &str) -> PartitionDefinition {
    PartitionDefinition {
        ID: id,
        Name: ast::NewCIStr(name),
        LessThan: Vec::new(),
        InValues: Vec::new(),
        PlacementPolicyRef: None,
        Comment: String::new(),
    }
}

#[test]
/// public 列按 Offset 占槽；非 public 列对应槽位为 None。
fn table_4_public_columns_keep_go_offset_slots() {
    let mut table = TableInfo::default();
    table.Columns = vec![
        column(1, "a", 0, StatePublic, 0),
        column(2, "changing", 1, StateWriteOnly, 0),
        column(3, "c", 2, StatePublic, 0),
    ];

    let columns = table.Cols();
    assert_eq!(columns.len(), 3);
    assert_eq!(columns[0].map(|column| column.Name.L.as_str()), Some("a"));
    assert!(columns[1].is_none());
    assert_eq!(columns[2].map(|column| column.Name.L.as_str()), Some("c"));
}

#[test]
/// MoveColumnInfo 同步列 Offset、索引列 Offset 与变更依赖 Offset。
fn table_4_move_column_updates_all_offset_references() {
    let mut table = TableInfo::default();
    table.Columns = (0..5)
        .map(|id| column(id, &format!("c_{id}"), id as isize, StatePublic, 0))
        .collect();
    table.Indices = vec![
        index(
            0,
            "i_0",
            &[("c_0", 0), ("c_1", 1), ("c_2", 2), ("c_3", 3), ("c_4", 4)],
            false,
            false,
        ),
        index(1, "i_1", &[("c_4", 4), ("c_2", 2)], false, false),
        index(2, "i_2", &[("c_0", 0), ("c_4", 4)], false, false),
        index(
            3,
            "i_3",
            &[("c_1", 1), ("c_2", 2), ("c_3", 3)],
            false,
            false,
        ),
        index(
            4,
            "i_4",
            &[("c_3", 3), ("c_2", 2), ("c_1", 1)],
            false,
            false,
        ),
    ];

    for (from, to, expected) in [
        (4, 0, [4, 0, 1, 2, 3]),
        (2, 3, [4, 0, 2, 1, 3]),
        (3, 2, [4, 0, 1, 2, 3]),
        (0, 4, [0, 1, 2, 3, 4]),
        (2, 2, [0, 1, 2, 3, 4]),
        (0, 0, [0, 1, 2, 3, 4]),
        (1, 4, [0, 2, 3, 4, 1]),
        (3, 0, [4, 0, 2, 3, 1]),
    ] {
        table.MoveColumnInfo(from, to);
        assert_offsets(&table, &expected);
    }

    let mut changing = column(5, "changing", 4, StatePublic, 0);
    changing.ChangeStateInfo = Some(ChangeStateInfo {
        DependencyColumnOffset: 1,
    });
    let mut dependent_table = TableInfo::default();
    dependent_table.Columns = vec![
        column(1, "a", 0, StatePublic, 0),
        column(2, "b", 1, StatePublic, 0),
        column(3, "c", 2, StatePublic, 0),
        column(4, "d", 3, StatePublic, 0),
        changing,
    ];
    dependent_table.Indices = vec![index(
        1,
        "idx",
        &[("a", 0), ("b", 1), ("changing", 4)],
        false,
        false,
    )];
    dependent_table.Indices[0].AffectColumn = Some(vec![IndexColumn {
        Name: ast::NewCIStr("c"),
        Offset: 2,
        ..IndexColumn::default()
    }]);

    dependent_table.MoveColumnInfo(4, 0);

    assert_eq!(
        dependent_table
            .Columns
            .iter()
            .map(|column| column.Name.L.as_str())
            .collect::<Vec<_>>(),
        vec!["changing", "a", "b", "c", "d"]
    );
    assert_eq!(
        dependent_table
            .Columns
            .iter()
            .map(|column| column.Offset)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3, 4]
    );
    assert_eq!(
        dependent_table.Indices[0]
            .Columns
            .iter()
            .map(|column| column.Offset)
            .collect::<Vec<_>>(),
        vec![1, 2, 0]
    );
    assert_eq!(
        dependent_table.Indices[0].AffectColumn.as_ref().unwrap()[0].Offset,
        3
    );
    assert_eq!(
        dependent_table.Columns[0]
            .ChangeStateInfo
            .as_ref()
            .unwrap()
            .DependencyColumnOffset,
        2
    );
}

#[test]
/// 显式 Primary 优先；否则选首个全 NOT NULL 非隐藏 UNIQUE 为隐式主键。
fn table_4_primary_key_matches_explicit_and_implicit_go_rules() {
    let mut table = TableInfo::default();
    table.Columns = vec![
        column(1, "nullable", 0, StatePublic, 0),
        column(2, "required", 1, StatePublic, mysql::NotNullFlag),
        column(3, "hidden", 2, StatePublic, mysql::NotNullFlag),
    ];
    table.Columns[2].Hidden = true;
    table.Indices = vec![
        index(10, "nullable_unique", &[("nullable", 0)], false, true),
        index(11, "hidden_unique", &[("hidden", 2)], false, true),
        index(12, "implicit", &[("required", 1)], false, true),
    ];
    assert_eq!(table.GetPrimaryKey().map(|index| index.ID), Some(12));

    table
        .Indices
        .push(index(13, "explicit", &[("nullable", 0)], true, false));
    assert_eq!(table.GetPrimaryKey().map(|index| index.ID), Some(13));
}

#[test]
/// 分区重叠回退下标与 IDsInDDLToIgnore 随 Action/State 变化。
fn table_4_partition_overlap_and_ignore_ids_follow_ddl_state() {
    let p0 = partition_definition(10, "p0");
    let p1 = partition_definition(11, "p1");
    let p2 = partition_definition(12, "p2");
    let mut partition = PartitionInfo::default();
    partition.Type = ast::PartitionTypeRange;
    partition.Definitions = vec![p0.clone(), p1.clone(), p2.clone()];
    partition.DroppingDefinitions = vec![p0.clone(), p1.clone()];
    partition.DDLAction = ActionDropTablePartition;
    partition.DDLState = StateWriteOnly;
    assert_eq!(partition.GetOverlappingDroppingPartitionIdx(0), 2);
    assert_eq!(partition.GetOverlappingDroppingPartitionIdx(2), 2);

    partition.Type = ast::PartitionTypeList;
    partition.Definitions[0].InValues = vec![vec!["1".to_owned()]];
    partition.Definitions[1].InValues = vec![vec!["2".to_owned()]];
    partition.Definitions[2].InValues = Vec::new();
    assert_eq!(partition.GetOverlappingDroppingPartitionIdx(0), 2);
    assert_eq!(partition.GetOverlappingDroppingPartitionIdx(2), 2);
    partition.DroppingDefinitions.push(p2.clone());
    assert_eq!(partition.GetOverlappingDroppingPartitionIdx(2), -1);
    partition.DroppingDefinitions = vec![p0, p1];

    partition.DDLAction = ActionTruncateTablePartition;
    partition.DDLState = StateWriteOnly;
    partition.NewPartitionIDs = vec![20, 21];
    assert_eq!(partition.IDsInDDLToIgnore(), vec![20, 21]);
    partition.DDLState = StateDeleteOnly;
    assert_eq!(partition.IDsInDDLToIgnore(), vec![10, 11]);
    partition.DDLAction = ActionAddTablePartition;
    partition.AddingDefinitions = vec![partition_definition(30, "p3")];
    assert_eq!(partition.IDsInDDLToIgnore(), vec![30]);
}

#[test]
/// FKInfo::String 对齐 SHOW CREATE TABLE 外键片段格式。
fn table_4_foreign_key_string_matches_show_create_format() {
    let mut foreign_key = FKInfo::default();
    foreign_key.Name = ast::NewCIStr("fk_child");
    foreign_key.Cols = vec![ast::NewCIStr("parent_id"), ast::NewCIStr("tenant_id")];
    foreign_key.RefSchema = ast::NewCIStr("app");
    foreign_key.RefTable = ast::NewCIStr("parent");
    foreign_key.RefCols = vec![ast::NewCIStr("id"), ast::NewCIStr("tenant_id")];
    foreign_key.OnDelete = 2;
    foreign_key.OnUpdate = 1;

    assert_eq!(
        foreign_key.String("app", "child"),
        "`app`.`child`, CONSTRAINT `fk_child` FOREIGN KEY (`parent_id`, `tenant_id`) REFERENCES `parent` (`id`, `tenant_id`) ON DELETE CASCADE ON UPDATE RESTRICT"
    );

    foreign_key.RefSchema = ast::NewCIStr("shared");
    assert!(
        foreign_key
            .String("app", "child")
            .contains("REFERENCES `shared`.`parent`")
    );
}

#[test]
/// TTL 间隔默认/解析、亲和性规范化与若干 String 未知值行为。
fn table_4_ttl_affinity_and_zero_value_strings_match_go() {
    let ttl = TTLInfo::default();
    assert_eq!(
        ttl.GetJobInterval().unwrap(),
        std::time::Duration::from_secs(3600)
    );
    let ttl = TTLInfo {
        JobInterval: "200h".to_owned(),
        ..TTLInfo::default()
    };
    assert_eq!(
        ttl.GetJobInterval().unwrap(),
        std::time::Duration::from_secs(200 * 3600)
    );
    let ttl = TTLInfo {
        JobInterval: "not-a-duration".to_owned(),
        ..TTLInfo::default()
    };
    assert!(ttl.GetJobInterval().is_err());

    assert!(NewTableAffinityInfoWithLevel("").unwrap().is_none());
    assert_eq!(
        NewTableAffinityInfoWithLevel("TABLE")
            .unwrap()
            .unwrap()
            .Level,
        "table"
    );
    assert!(NewTableAffinityInfoWithLevel("rack").is_err());

    assert_eq!(TableCacheStatusType(99).String(), "");
    assert_eq!(TempTableType(99).String(), "");
    assert_eq!(TableLockState(99).String(), "none");
    assert_eq!(WindowRepeatType(99).String(), "");
}

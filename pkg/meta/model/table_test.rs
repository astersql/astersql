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

// TableInfo / ColumnInfo 等元数据模型的单元测试（对照 Go `table_test.go`）。
//
// 覆盖列移动后索引 Offset 同步、主键与外键基本字段、TTL（生存时间）信息克隆与
// job 间隔解析，以及分区 reorg（重组）中间态清理。`GO_REFERENCE` 保留 Go 测试全文供对照。

use crate::group_1 as production;
use crate::group_1::*;
use crate::group_2::serde_json;

use std::sync::Arc;
use std::time::Duration;

fn new_column_for_test(id: i64, offset: isize) -> ColumnInfo {
    ColumnInfo {
        ID: id,
        Name: ast::NewCIStr(&format!("c_{id}")),
        Offset: offset,
        ..Default::default()
    }
}

fn new_index_for_test(id: i64, columns: &[&ColumnInfo]) -> IndexInfo {
    IndexInfo {
        ID: id,
        Name: ast::NewCIStr(&format!("i_{id}")),
        Columns: columns
            .iter()
            .map(|column| IndexColumn {
                Name: column.Name.clone(),
                Offset: column.Offset,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    }
}

fn check_offsets(table: &TableInfo, ids: &[i64]) {
    assert_eq!(ids.len(), table.Columns.len());
    for (offset, id) in ids.iter().enumerate() {
        assert_eq!(format!("c_{id}"), table.Columns[offset].Name.L);
        assert_eq!(offset as isize, table.Columns[offset].Offset);
    }

    for column in &table.Columns {
        for index in &table.Indices {
            for index_column in &index.Columns {
                if column.Name.L == index_column.Name.L {
                    assert_eq!(column.Offset, index_column.Offset);
                }
            }
        }
    }
}

#[test]
fn test_move_column_info() {
    let c0 = new_column_for_test(0, 0);
    let c1 = new_column_for_test(1, 1);
    let c2 = new_column_for_test(2, 2);
    let c3 = new_column_for_test(3, 3);
    let c4 = new_column_for_test(4, 4);

    let i0 = new_index_for_test(0, &[&c0, &c1, &c2, &c3, &c4]);
    let i1 = new_index_for_test(1, &[&c4, &c2]);
    let i2 = new_index_for_test(2, &[&c0, &c4]);
    let i3 = new_index_for_test(3, &[&c1, &c2, &c3]);
    let i4 = new_index_for_test(4, &[&c3, &c2, &c1]);

    let mut table = TableInfo {
        ID: 1,
        Name: ast::NewCIStr("t"),
        Columns: vec![c0, c1, c2, c3, c4],
        Indices: vec![i0, i1, i2, i3, i4],
        ..Default::default()
    };

    table.MoveColumnInfo(4, 0);
    check_offsets(&table, &[4, 0, 1, 2, 3]);
    table.MoveColumnInfo(2, 3);
    check_offsets(&table, &[4, 0, 2, 1, 3]);
    table.MoveColumnInfo(3, 2);
    check_offsets(&table, &[4, 0, 1, 2, 3]);
    table.MoveColumnInfo(0, 4);
    check_offsets(&table, &[0, 1, 2, 3, 4]);
    table.MoveColumnInfo(2, 2);
    check_offsets(&table, &[0, 1, 2, 3, 4]);
    table.MoveColumnInfo(0, 0);
    check_offsets(&table, &[0, 1, 2, 3, 4]);
    table.MoveColumnInfo(1, 4);
    check_offsets(&table, &[0, 2, 3, 4, 1]);
    table.MoveColumnInfo(3, 0);
    check_offsets(&table, &[4, 0, 2, 3, 1]);
}

#[test]
fn test_model_basic() {
    let mut column = ColumnInfo {
        ID: 1,
        Name: ast::NewCIStr("c"),
        Offset: 0,
        DefaultValue: Some(DefaultValue::Int(0)),
        Hidden: true,
        ..Default::default()
    };
    column.AddFlag(mysql::PriKeyFlag);

    let index = IndexInfo {
        Name: ast::NewCIStr("key"),
        Table: ast::NewCIStr("t"),
        Columns: vec![IndexColumn {
            Name: ast::NewCIStr("c"),
            Offset: 0,
            Length: 10,
            ..Default::default()
        }],
        Unique: true,
        Primary: true,
        ..Default::default()
    };
    let foreign_key = FKInfo {
        RefCols: vec![ast::NewCIStr("a")],
        Cols: vec![ast::NewCIStr("a")],
        ..Default::default()
    };
    let sequence = SequenceInfo {
        Increment: 1,
        MinValue: 1,
        MaxValue: 100,
        ..Default::default()
    };

    let mut table = TableInfo {
        ID: 1,
        Name: ast::NewCIStr("t"),
        Charset: "utf8".to_owned(),
        Collate: "utf8_bin".to_owned(),
        Columns: vec![column.clone()],
        Indices: vec![index],
        ForeignKeys: vec![foreign_key],
        PKIsHandle: true,
        ..Default::default()
    };
    let sequence_table = TableInfo {
        ID: 2,
        Name: ast::NewCIStr("s"),
        Sequence: Some(sequence),
        ..Default::default()
    };

    let db_table = Arc::new(production::TableInfo {
        ID: 1,
        Name: production::ast::NewCIStr("t"),
        ..Default::default()
    });
    let mut db_info = production::DBInfo {
        ID: 1,
        Name: production::ast::NewCIStr("test"),
        Charset: "utf8".to_owned(),
        Collate: "utf8_bin".to_owned(),
        ..Default::default()
    };
    db_info.Deprecated.Tables = vec![db_table];
    let cloned_db = db_info.Clone();
    assert_eq!(db_info.ID, cloned_db.ID);
    assert_eq!(db_info.Name, cloned_db.Name);
    assert_eq!(db_info.Charset, cloned_db.Charset);
    assert_eq!(db_info.Collate, cloned_db.Collate);
    assert_eq!(
        db_info.Deprecated.Tables[0].ID,
        cloned_db.Deprecated.Tables[0].ID
    );
    assert!(!Arc::ptr_eq(
        &db_info.Deprecated.Tables[0],
        &cloned_db.Deprecated.Tables[0]
    ));

    assert_eq!(ast::NewCIStr("c"), table.GetPkName());
    let primary_column = table.GetPkColInfo().expect("primary key column");
    assert!(primary_column.Hidden);
    assert_eq!(column.ID, primary_column.ID);
    assert!(table.ColumnIsInIndex(&column));
    assert_eq!(1, table.ForeignKeys.len());

    assert_eq!("BTREE", production::ast::model::IndexTypeBtree.to_string());
    assert_eq!("HASH", production::ast::model::IndexTypeHash.to_string());
    assert_eq!("", production::ast::model::IndexType(100_000).to_string());

    let production_index = production::IndexInfo {
        Columns: vec![production::IndexColumn {
            Name: production::ast::NewCIStr("c"),
            Offset: 0,
            Length: 10,
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(production_index.HasPrefixIndex());
    assert_eq!(TSConvert2Time(table.UpdateTS), table.GetUpdateTime());
    assert!(sequence_table.IsSequence());
    assert!(!sequence_table.IsBaseTable());

    column.ToggleFlag(mysql::PriKeyFlag);
    table.Columns[0] = column.clone();
    assert_eq!(ast::NewCIStr(""), table.GetPkName());
    assert!(table.GetPkColInfo().is_none());
    assert!(!table.ColumnIsInIndex(&ColumnInfo {
        Name: ast::NewCIStr("d"),
        ..Default::default()
    }));
    assert!(!production::IndexInfo::default().HasPrefixIndex());

    let mut production_column = production::ColumnInfo::New(1, production::ast::NewCIStr("c"));
    production_column.AddFlag(production::mysql::PriKeyFlag);
    assert!(production::mysql::HasPriKeyFlag(
        production_column.GetFlag()
    ));
    production_column.ToggleFlag(production::mysql::PriKeyFlag);
    assert!(!production::mysql::HasPriKeyFlag(
        production_column.GetFlag()
    ));

    let extra_primary_key = production::NewExtraHandleColInfo();
    assert_eq!(
        production::mysql::NotNullFlag | production::mysql::PriKeyFlag,
        extra_primary_key.GetFlag()
    );
    assert_eq!(
        production::charset::CharsetBin,
        extra_primary_key.GetCharset()
    );
    assert_eq!(
        production::charset::CollationBin,
        extra_primary_key.GetCollate()
    );
}

#[test]
fn test_ttl_info_clone() {
    let ttl_info = TTLInfo {
        ColumnName: ast::NewCIStr("test"),
        IntervalExprStr: "test_expr".to_owned(),
        IntervalTimeUnit: 5,
        Enable: true,
        ..Default::default()
    };

    let mut cloned = ttl_info.Clone();
    cloned.ColumnName = ast::NewCIStr("test_2");
    cloned.IntervalExprStr = "test_expr_2".to_owned();
    cloned.IntervalTimeUnit = 9;
    cloned.Enable = false;

    assert_eq!("test", ttl_info.ColumnName.O);
    assert_eq!("test_expr", ttl_info.IntervalExprStr);
    assert_eq!(5, ttl_info.IntervalTimeUnit);
    assert!(ttl_info.Enable);
}

#[test]
fn test_ttl_job_interval() {
    let mut ttl_info = TTLInfo::default();
    assert_eq!(
        Duration::from_secs(60 * 60),
        ttl_info.GetJobInterval().unwrap()
    );

    ttl_info.JobInterval = "200h".to_owned();
    assert_eq!(
        Duration::from_secs(200 * 60 * 60),
        ttl_info.GetJobInterval().unwrap()
    );
}

#[test]
fn test_clear_reorg_intermediate_info() {
    let mut partition = PartitionInfo {
        DDLAction: ActionAddTablePartition,
        DDLState: StateWriteOnly,
        DDLType: ast::PartitionTypeHash,
        DDLExpr: "Test DDL Expr".to_owned(),
        DDLColumns: vec![ast::NewCIStr("c")],
        NewTableID: 1111,
        DDLChangedIndex: [(1, true)].into_iter().collect(),
        ..Default::default()
    };

    partition.ClearReorgIntermediateInfo();
    assert_eq!(ActionNone, partition.DDLAction);
    assert_eq!(StateNone, partition.DDLState);
    assert_eq!(ast::PartitionTypeNone, partition.DDLType);
    assert_eq!("", partition.DDLExpr);
    assert!(partition.DDLColumns.is_empty());
    assert_eq!(0, partition.NewTableID);
    assert!(partition.DDLChangedIndex.is_empty());
}

#[test]
fn test_ttl_default_job_interval() {
    assert_eq!(
        Duration::from_secs(24 * 60 * 60),
        duration::ParseDuration(DefaultTTLJobInterval).unwrap()
    );
    assert_eq!(
        Duration::from_secs(60 * 60),
        duration::ParseDuration(OldDefaultTTLJobInterval).unwrap()
    );
}

#[test]
fn table_metadata_json_matches_go_field_names_and_embedding() {
    let table_name = serde_json::to_value(TableNameInfo {
        ID: 7,
        Name: ast::NewCIStr("t"),
    })
    .unwrap();
    assert!(table_name.get("id").is_some());
    assert!(table_name.get("name").is_some());
    assert!(table_name.get("ID").is_none());

    let view = serde_json::to_value(ViewInfo {
        SelectStmt: "select 1".to_owned(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(view["view_select"], "select 1");
    assert!(view.get("SelectStmt").is_none());

    let referred = serde_json::to_value(ReferredFKInfo {
        ChildSchema: ast::NewCIStr("parent"),
        ChildTable: ast::NewCIStr("child"),
        ChildFKName: ast::NewCIStr("fk"),
        ..Default::default()
    })
    .unwrap();
    assert!(referred.get("child_schema").is_some());
    assert!(referred.get("child_table").is_some());
    assert!(referred.get("child_fk_name").is_some());
    assert!(referred.get("ChildSchema").is_none());

    let load_item = serde_json::to_value(StatsLoadItem {
        TableItemID: TableItemID {
            TableID: 11,
            ID: 12,
            IsIndex: true,
            IsSyncLoadFailed: false,
        },
        FullLoad: true,
    })
    .unwrap();
    assert_eq!(load_item["TableID"], 11);
    assert_eq!(load_item["ID"], 12);
    assert_eq!(load_item["FullLoad"], true);
    assert!(load_item.get("TableItemID").is_none());

    let stats = serde_json::to_value(StatsOptions {
        StatsWindowSettings: Some(StatsWindowSettings {
            RepeatType: Week,
            RepeatInterval: 2,
            ..Default::default()
        }),
        ..NewStatsOptions()
    })
    .unwrap();
    assert_eq!(stats["repeat_type"], 2);
    assert_eq!(stats["repeat_interval"], 2);
    assert!(stats.get("StatsWindowSettings").is_none());
    assert!(stats.get("auto_recalc").is_some());

    let stats_without_window: StatsOptions = serde_json::from_value(serde_json::json!({
        "auto_recalc": true,
        "column_choice": 0,
        "column_list": [],
        "sample_num": 0,
        "sample_rate": 0.0,
        "buckets": 0,
        "topn": 0,
        "concurrency": 0
    }))
    .unwrap();
    assert!(stats_without_window.StatsWindowSettings.is_none());
}
use crate::{ColumnInfo, IndexColumn, IndexInfo, TableInfo, ast};

/// 移动列后，索引中同名列的 Offset 必须与列定义保持一致。
#[test]
fn moving_columns_updates_index_offsets() {
    // 构造三列与覆盖全部列的索引，Offset 与列序初始对齐。
    let columns = (0..3)
        .map(|id| ColumnInfo {
            ID: id,
            Name: ast::NewCIStr(&format!("c_{id}")),
            Offset: id as isize,
            ..Default::default()
        })
        .collect::<Vec<_>>();
    let index = IndexInfo {
        Columns: columns
            .iter()
            .map(|column| IndexColumn {
                Name: column.Name.clone(),
                Offset: column.Offset,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let mut table = TableInfo {
        Columns: columns,
        Indices: vec![index],
        ..Default::default()
    };
    // 将末列移到开头，期望列 ID 顺序变为 [2,0,1]，且索引 Offset 随之更新。
    table.MoveColumnInfo(2, 0);
    assert_eq!(
        table
            .Columns
            .iter()
            .map(|column| column.ID)
            .collect::<Vec<_>>(),
        vec![2, 0, 1]
    );
    for column in &table.Columns {
        let indexed = table.Indices[0]
            .Columns
            .iter()
            .find(|item| item.Name == column.Name)
            .unwrap();
        assert_eq!(indexed.Offset, column.Offset);
    }
}

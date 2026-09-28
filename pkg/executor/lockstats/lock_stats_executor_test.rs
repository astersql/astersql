// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// LOCK STATS 元数据解析辅助函数的单元测试。
//
// 覆盖：按分区名解析 ID，以及整表展开时生成分区完整显示名。

use super::lock_stats_executor::{
    CIStr, Error, InfoSchema, PartitionDefinition, TableMeta, TableName,
    populatePartitionIDAndNames, populateTableAndPartitionIDs,
};

/// 固定返回 `test.t` 及其分区 P1/P2 的 InfoSchema 桩，并提供非分区表 `t2`。
struct MockInfoSchema;
impl InfoSchema for MockInfoSchema {
    fn TableByName(&self, schema: &str, table: &str) -> Result<TableMeta, Error> {
        if schema != "test" {
            return Err(Error("table not found".into()));
        }
        if table == "t2" {
            return Ok(TableMeta {
                ID: 4,
                Partitions: None,
            });
        }
        if table != "t" {
            return Err(Error("table not found".into()));
        }
        Ok(TableMeta {
            ID: 1,
            Partitions: Some(vec![
                PartitionDefinition {
                    ID: 2,
                    Name: CIStr::new("P1"),
                },
                PartitionDefinition {
                    ID: 3,
                    Name: CIStr::new("P2"),
                },
            ]),
        })
    }
}

/// 对应 Go 的 TestPopulatePartitionIDAndNames：校验分区 ID 展开。
#[test]
fn populate_partition_id_and_names() {
    let table = TableName {
        Schema: CIStr::new("test"),
        Name: CIStr::new("t"),
        PartitionNames: vec![CIStr::new("p1"), CIStr::new("p2")],
    };

    let (table_id, partitions) =
        populatePartitionIDAndNames(&table, &table.PartitionNames, &MockInfoSchema).unwrap();
    assert_eq!(table_id, 1);
    assert_eq!(partitions.get(&2).map(String::as_str), Some("p1"));
    assert_eq!(partitions.get(&3).map(String::as_str), Some("p2"));

    // Empty partition names must be rejected, matching the Go test.
    let error = populatePartitionIDAndNames(&table, &[], &MockInfoSchema).unwrap_err();
    assert_eq!(error, Error("partition list should not be empty".into()));
}

/// Go 的 `FindPartitionByName` 使用 `EqualFold`，不能依赖元数据中的 `L` 已规范化。
#[test]
fn populate_partition_id_and_names_matches_case_insensitively() {
    struct MixedCaseInfoSchema;
    impl InfoSchema for MixedCaseInfoSchema {
        fn TableByName(&self, _schema: &str, _table: &str) -> Result<TableMeta, Error> {
            Ok(TableMeta {
                ID: 1,
                Partitions: Some(vec![PartitionDefinition {
                    ID: 2,
                    Name: CIStr {
                        O: "P1".into(),
                        L: "P1".into(),
                    },
                }]),
            })
        }
    }

    let table = TableName {
        Schema: CIStr::new("test"),
        Name: CIStr::new("t"),
        PartitionNames: vec![CIStr::new("p1")],
    };
    let (_, partitions) =
        populatePartitionIDAndNames(&table, &table.PartitionNames, &MixedCaseInfoSchema).unwrap();
    assert_eq!(partitions.get(&2).map(String::as_str), Some("p1"));
}

/// 对应 Go 的 TestPopulateTableAndPartitionIDs：校验整表及分区展开。
#[test]
fn populate_table_and_partition_ids() {
    let tables = [
        TableName {
            Schema: CIStr::new("test"),
            Name: CIStr::new("t"),
            PartitionNames: vec![CIStr::new("p0"), CIStr::new("p1")],
        },
        TableName {
            Schema: CIStr::new("test"),
            Name: CIStr::new("t2"),
            PartitionNames: vec![],
        },
    ];

    let all = populateTableAndPartitionIDs(&tables[..1], &MockInfoSchema).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[&1].FullName, "test.t");
    assert_eq!(all[&1].PartitionInfo[&2], "test.t partition (p1)");
    assert_eq!(all[&1].PartitionInfo[&3], "test.t partition (p2)");

    // A non-partitioned table has an empty PartitionInfo map.
    let no_partitions = populateTableAndPartitionIDs(&tables[1..], &MockInfoSchema).unwrap();
    assert_eq!(no_partitions[&4].FullName, "test.t2");
    assert!(no_partitions[&4].PartitionInfo.is_empty());

    // Empty table list must be rejected, matching the Go test.
    let error = populateTableAndPartitionIDs(&[], &MockInfoSchema).unwrap_err();
    assert_eq!(error, Error("table list should not be empty".into()));
}

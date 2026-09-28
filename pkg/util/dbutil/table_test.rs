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

// 表解析、结构比较与表模式保护的单元测试。
//
// 对应 Go `table_test.go`：覆盖 `TableInfo` 解析、`EqualTableInfo`、schemacmp 编码，
// 以及 `CheckTableModeIsNormal` 对 Import/Restore 的拒绝。含 Go 源码存档与可编译断言。

/// 存档 Go 侧 `table_test.go` 源码片段，便于对照迁移期测试意图；不参与编译执行。
const GO_REFERENCE: &str = r################"
// 这段逻辑只描述 TableInfo 解析、比较和 schemacmp 编码测试，不会解析真实 SQL 或连接数据库。

// testCase 对应 Go 同名结构体：记录建表 SQL、期望列名、期望索引名、索引列长度和查列结果。
struct TestCase {
    sql: &'static str,
    columns: &'static [&'static str],
    indexs: &'static [&'static str],
    col_len: &'static [&'static [i32]],
    col_name: &'static str,
    fine_col: bool,
}

// TestTable 对应 Go 测试：检查 parser 生成的列、索引和 FindColumnByName 行为。
#[test]
fn test_table() {
    let test_cases = vec![
        TestCase {
            sql: r#"
            CREATE TABLE htest (
                a int(11) PRIMARY KEY
            ) ENGINE=InnoDB DEFAULT CHARSET=latin1 COLLATE=latin1_bin
            "#,
            columns: &["a"],
            indexs: &[mysql::PrimaryKeyName],
            col_len: &[&[types::UnspecifiedLength]],
            col_name: "c",
            fine_col: false,
        },
        TestCase {
            sql: r#"
            CREATE TABLE itest (a int(11) NOT NULL,
                b double NOT NULL DEFAULT '2',
                c varchar(10) NOT NULL,
                d time DEFAULT NULL,
                PRIMARY KEY (a, b),
                UNIQUE KEY d (d))
            "#,
            columns: &["a", "b", "c", "d"],
            indexs: &[mysql::PrimaryKeyName, "d"],
            col_len: &[&[types::UnspecifiedLength, types::UnspecifiedLength], &[types::UnspecifiedLength]],
            col_name: "a",
            fine_col: true,
        },
        TestCase {
            sql: r#"
            CREATE TABLE jtest (
                a int(11) NOT NULL,
                b varchar(10) DEFAULT NULL,
                c varchar(255) DEFAULT NULL,
                PRIMARY KEY (a)
            ) ENGINE=InnoDB DEFAULT CHARSET=latin1 COLLATE=latin1_bin
            "#,
            columns: &["a", "b", "c"],
            indexs: &[mysql::PrimaryKeyName],
            col_len: &[&[types::UnspecifiedLength]],
            col_name: "c",
            fine_col: true,
        },
        TestCase {
            sql: r#"
            CREATE TABLE mtest (
                a int(24),
                KEY test (a))
            "#,
            columns: &["a"],
            indexs: &["test"],
            col_len: &[&[types::UnspecifiedLength]],
            col_name: "d",
            fine_col: false,
        },
        TestCase {
            sql: r#"
            CREATE TABLE ntest (
                a int(24) PRIMARY KEY CLUSTERED
            )
            "#,
            columns: &["a"],
            indexs: &[mysql::PrimaryKeyName],
            col_len: &[&[types::UnspecifiedLength]],
            col_name: "d",
            fine_col: false,
        },
        TestCase {
            sql: r#"
            CREATE TABLE otest (
                a int(11) NOT NULL,
                b varchar(10) DEFAULT NULL,
                c varchar(255) DEFAULT NULL,
                PRIMARY KEY (a)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci
            "#,
            columns: &["a", "b", "c"],
            indexs: &[mysql::PrimaryKeyName],
            col_len: &[&[types::UnspecifiedLength]],
            col_name: "c",
            fine_col: true,
        },
    ];

    for test_case in test_cases {
        // Go 原测试通过 dbutiltest.GetTableInfoBySQL(parser.New()) 构造 TableInfo；这里保留解析入口。
        let (table_info, err) = dbutiltest::GetTableInfoBySQL(test_case.sql, parser::New());
        assert!(err.is_none());
        for (i, column) in table_info.Columns.iter().enumerate() {
            assert_eq!(column.Name.O, test_case.columns[i]);
        }

        assert_eq!(table_info.Indices.len(), test_case.indexs.len());
        for (j, index) in table_info.Indices.iter().enumerate() {
            assert_eq!(index.Name.O, test_case.indexs[j]);
            for (k, index_col) in index.Columns.iter().enumerate() {
                assert_eq!(test_case.col_len[j][k], index_col.Length);
            }
        }

        let col = dbutil::FindColumnByName(table_info.Columns, test_case.col_name);
        assert_eq!(col.is_some(), test_case.fine_col);
    }
}

// EqualTableInfo returns true if this two table info have same columns and indices
// EqualTableInfo 对应 Go 辅助函数：逐项比较列名、列类型、索引名和索引列顺序。
fn EqualTableInfo(table_info1: &model::TableInfo, table_info2: &model::TableInfo) -> (bool, String) {
    // check columns
    if table_info1.Columns.len() != table_info2.Columns.len() {
        return (
            false,
            format!(
                "column num not equal, one is {} another is {}",
                table_info1.Columns.len(),
                table_info2.Columns.len()
            ),
        );
    }

    for (j, col) in table_info1.Columns.iter().enumerate() {
        if col.Name.O != table_info2.Columns[j].Name.O {
            return (
                false,
                format!(
                    "column name not equal, one is {} another is {}",
                    col.Name.O, table_info2.Columns[j].Name.O
                ),
            );
        }
        if col.GetType() != table_info2.Columns[j].GetType() {
            return (
                false,
                format!(
                    "column {}'s type not equal, one is {:?} another is {:?}",
                    col.Name.O,
                    col.GetType(),
                    table_info2.Columns[j].GetType()
                ),
            );
        }
    }

    // check index：Go 先比较索引数量，再把 tableInfo2 的索引按名称建 map 以支持顺序无关比较。
    if table_info1.Indices.len() != table_info2.Indices.len() {
        return (
            false,
            format!(
                "index num not equal, one is {} another is {}",
                table_info1.Indices.len(),
                table_info2.Indices.len()
            ),
        );
    }

    let mut index2_map = HashMap::new();
    for index in &table_info2.Indices {
        index2_map.insert(index.Name.O.clone(), index);
    }

    for index1 in &table_info1.Indices {
        let Some(index2) = index2_map.get(&index1.Name.O) else {
            return (false, format!("index {} not exists", index1.Name.O));
        };

        if index1.Columns.len() != index2.Columns.len() {
            return (
                false,
                format!(
                    "index {}'s columns num not equal, one is {} another is {}",
                    index1.Name.O,
                    index1.Columns.len(),
                    index2.Columns.len()
                ),
            );
        }
        for (j, col) in index1.Columns.iter().enumerate() {
            if col.Name.O != index2.Columns[j].Name.O {
                return (
                    false,
                    format!(
                        "index {}'s column not equal, one has {} another has {}",
                        index1.Name.O, col.Name.O, index2.Columns[j].Name.O
                    ),
                );
            }
        }
    }

    (true, String::new())
}

// TestTableStructEqual 对应 Go 测试：NOT NULL 差异不影响 EqualTableInfo，主键和唯一索引差异会返回 false。
#[test]
fn test_table_struct_equal() {
    let create_table_sql1 = "CREATE TABLE `test`.`atest` (`id` int(24), `name` varchar(24), `birthday` datetime, `update_time` time, `money` decimal(20,2), primary key(`id`))";
    let (table_info1, err) = dbutiltest::GetTableInfoBySQL(create_table_sql1, parser::New());
    assert!(err.is_none());

    let create_table_sql2 = "CREATE TABLE `test`.`atest` (`id` int(24) NOT NULL, `name` varchar(24), `birthday` datetime, `update_time` time, `money` decimal(20,2), primary key(`id`))";
    let (table_info2, err) = dbutiltest::GetTableInfoBySQL(create_table_sql2, parser::New());
    assert!(err.is_none());

    let create_table_sql3 = r#"CREATE TABLE "test"."atest" ("id" int(24), "name" varchar(24), "birthday" datetime, "update_time" time, "money" decimal(20,2), unique key("id"))"#;
    let mut p = parser::New();
    p.SetSQLMode(mysql::ModeANSIQuotes);
    let (table_info3, err) = dbutiltest::GetTableInfoBySQL(create_table_sql3, p);
    assert!(err.is_none());

    let (equal, _) = EqualTableInfo(table_info1, table_info2);
    assert_eq!(true, equal);

    let (equal, _) = EqualTableInfo(table_info1, table_info3);
    assert_eq!(false, equal);
}

// TestSchemaCmpEncode 对应 Go 测试：schemacmp.Encode 应输出规范化建表语句。
#[test]
fn test_schema_cmp_encode() {
    let create_table_sql = "CREATE TABLE `test`.`atest` (`id` int(24), primary key(`id`))";
    let (table_info, err) = dbutiltest::GetTableInfoBySQL(create_table_sql, parser::New());
    assert!(err.is_none());

    let table = schemacmp::Encode(table_info);
    assert_eq!(
        "CREATE TABLE `tbl`(`id` INT(24) NOT NULL, PRIMARY KEY (`id`)) COLLATE utf8mb4_bin",
        table.String()
    );
}
"################;

use crate::table::FindColumnByName;
use crate::table::{CheckTableModeIsNormal, TableMode};
use astersql_infoschema::infoschema::CiString;
use astersql_infoschema::infoschema::{ColumnInfo, TableInfo};

/// Import/Restore 模式应报错且错误文案带表名；Normal 通过。
#[test]
fn protected_table_modes_are_rejected_with_table_identity() {
    let table = CiString::new("Orders");
    assert!(CheckTableModeIsNormal(&table, TableMode::Normal).is_ok());
    let error = CheckTableModeIsNormal(&table, TableMode::Import).unwrap_err();
    assert_eq!(
        error,
        "ErrProtectedTableMode: Table Orders is in mode Import"
    );
    let error = CheckTableModeIsNormal(&table, TableMode::Restore).unwrap_err();
    assert_eq!(
        error,
        "ErrProtectedTableMode: Table Orders is in mode Restore"
    );
}

#[test]
fn find_column_by_name_is_case_insensitive_and_preserves_missing_semantics() {
    let table = TableInfo {
        columns: vec![
            ColumnInfo {
                name: CiString::new("UserID"),
                ..Default::default()
            },
            ColumnInfo {
                name: CiString::new("name"),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(
        FindColumnByName(&table.columns, "userid")
            .unwrap()
            .name
            .original,
        "UserID"
    );
    assert_eq!(
        FindColumnByName(&table.columns, "NAME")
            .unwrap()
            .name
            .original,
        "name"
    );
    assert!(FindColumnByName(&table.columns, "missing").is_none());
}

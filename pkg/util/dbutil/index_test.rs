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

// 索引发现辅助函数的单元测试。
//
// 对应 Go `pkg/util/dbutil` 索引相关测试：校验 `FindAllIndex` /
// `FindAllColumnWithIndex` 按主键、唯一索引、普通索引顺序收集索引与列。
// 本文件含 Go 原测试逻辑的字符串存档，以及可编译的 Rust 等价断言。

/// 存档 Go 侧 `index_test.go` 源码片段，便于对照迁移期测试意图；不参与编译执行。
const GO_REFERENCE: &str = r################"
// 这段逻辑只描述索引发现函数的测试数据和断言顺序，不会解析真实 SQL 或连接数据库。

// indexCase 对应 Go 匿名结构体：一段 CREATE TABLE SQL 以及期望的索引名、索引列名顺序。
struct IndexCase {
    sql: &'static str,
    indices: &'static [&'static str],
    cols: &'static [&'static str],
}

// TestIndex 对应 Go 测试：通过 parser 构造 TableInfo 后检查 FindAllIndex 与 FindAllColumnWithIndex。
#[test]
fn test_index() {
    let test_cases = vec![
        IndexCase {
            sql: r#"
             CREATE TABLE itest (a int(11) NOT NULL,
             b double NOT NULL DEFAULT '2',
             c varchar(10) NOT NULL,
             d time DEFAULT NULL,
             PRIMARY KEY (a, b),
             UNIQUE KEY d(d))
             "#,
            indices: &["PRIMARY", "d"],
            cols: &["a", "b", "d"],
        },
        IndexCase {
            sql: r#"
             CREATE TABLE jtest (
                 a int(11) NOT NULL,
                 b varchar(10) DEFAULT NULL,
                 c varchar(255) DEFAULT NULL,
                 KEY c(c),
                 UNIQUE KEY b(b, c),
                 PRIMARY KEY (a)
             ) ENGINE=InnoDB DEFAULT CHARSET=latin1 COLLATE=latin1_bin
             "#,
            indices: &["PRIMARY", "b", "c"],
            cols: &["a", "b", "c"],
        },
        IndexCase {
            sql: r#"
             CREATE TABLE mtest (
                 a int(24),
                 KEY test (a))
             "#,
            indices: &["test"],
            cols: &["a"],
        },
        IndexCase {
            sql: r#"
             CREATE TABLE mtest (
                a int(24),
                b int(24),
                KEY test1 (a),
                KEY test2 (b))
             "#,
            indices: &["test1", "test2"],
            cols: &["a", "b"],
        },
        IndexCase {
            sql: r#"
             CREATE TABLE mtest (
                a int(24),
                b int(24),
                UNIQUE KEY test1 (a),
                UNIQUE KEY test2 (b))
             "#,
            indices: &["test1", "test2"],
            cols: &["a", "b"],
        },
    ];

    for test_case in test_cases {
        // Go 这里调用 dbutiltest.GetTableInfoBySQL(parser.New())；保留解析入口和错误断言。
        let (table_info, err) = dbutiltest::GetTableInfoBySQL(test_case.sql, parser::New());
        assert!(err.is_none());

        let indices = dbutil::FindAllIndex(table_info);
        for (i, index) in indices.iter().enumerate() {
            assert_eq!(test_case.indices[i], index.Name.O);
        }

        // 列收集顺序也按 Go 测试逐项比较，覆盖普通索引、唯一索引和主键。
        let cols = dbutil::FindAllColumnWithIndex(table_info);
        for (j, col) in cols.iter().enumerate() {
            assert_eq!(test_case.cols[j], col.Name.O);
        }
    }
}
"################;

use crate::index::{FindAllColumnWithIndex, FindAllIndex, IndexedTable, TableIndexInfo};
use crate::index::{FindSuitableColumnWithIndex, ShowIndex};
use crate::{DbError, QueryExecutor, QueryResult, Value};
use astersql_infoschema::infoschema::{CiString, ColumnInfo};
use std::sync::Mutex;

/// 构造含普通/主键/唯一三类索引的表，断言索引与列按主键→唯一→普通顺序返回。
#[test]
fn indices_and_columns_follow_primary_unique_normal_order() {
    let table = IndexedTable {
        columns: ["a", "b", "c"]
            .into_iter()
            .enumerate()
            .map(|(id, name)| ColumnInfo {
                id: id as i64,
                name: CiString::new(name),
                ..Default::default()
            })
            .collect(),
        indices: vec![
            TableIndexInfo {
                name: "normal".into(),
                columns: vec![2],
                ..Default::default()
            },
            TableIndexInfo {
                name: "primary".into(),
                columns: vec![0],
                primary: true,
                ..Default::default()
            },
            TableIndexInfo {
                name: "unique".into(),
                columns: vec![1],
                unique: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    assert_eq!(
        FindAllIndex(&table)
            .iter()
            .map(|index| index.name.as_str())
            .collect::<Vec<_>>(),
        vec!["primary", "unique", "normal"]
    );
    assert_eq!(
        FindAllColumnWithIndex(&table)
            .iter()
            .map(|column| column.name.original.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b", "c"]
    );
}

struct IndexFixture {
    result: Mutex<QueryResult>,
}

impl QueryExecutor for IndexFixture {
    fn QueryContext(&self, _query: &str, _args: &[Value]) -> Result<QueryResult, DbError> {
        Ok(self.result.lock().unwrap().clone())
    }
}

#[test]
fn show_index_and_cardinality_selection_preserve_go_order_and_errors() {
    let fixture = IndexFixture {
        result: Mutex::new(QueryResult {
            columns: [
                "Table",
                "Non_unique",
                "Key_name",
                "Seq_in_index",
                "Column_name",
                "Cardinality",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rows: vec![
                vec![
                    "t".into(),
                    "1".into(),
                    "idx_a".into(),
                    "1".into(),
                    "a".into(),
                    10_i64.into(),
                ],
                vec![
                    "t".into(),
                    "1".into(),
                    "idx_b".into(),
                    "1".into(),
                    "b".into(),
                    20_i64.into(),
                ],
            ],
        }),
    };
    let indices = ShowIndex(&fixture, "test", "t").unwrap();
    assert_eq!(indices.len(), 2);
    assert_eq!(indices[1].ColumnName, "b");

    let table = IndexedTable {
        name: "t".into(),
        columns: ["a", "b"]
            .into_iter()
            .map(|name| ColumnInfo {
                name: CiString::new(name),
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    assert_eq!(
        FindSuitableColumnWithIndex(&fixture, "test", &table)
            .unwrap()
            .unwrap()
            .name
            .original,
        "b"
    );

    let invalid = IndexFixture {
        result: Mutex::new(QueryResult {
            columns: vec!["Table".into()],
            rows: vec![vec!["t".into()]],
        }),
    };
    assert!(ShowIndex(&invalid, "test", "t").is_err());
}

#[test]
fn cardinality_selection_errors_when_the_best_index_column_is_missing() {
    let fixture = IndexFixture {
        result: Mutex::new(QueryResult {
            columns: [
                "Table",
                "Non_unique",
                "Key_name",
                "Seq_in_index",
                "Column_name",
                "Cardinality",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rows: vec![vec![
                "t".into(),
                "1".into(),
                "idx_missing".into(),
                "1".into(),
                "missing".into(),
                42_i64.into(),
            ]],
        }),
    };
    let table = IndexedTable {
        name: "t".into(),
        columns: vec![ColumnInfo {
            name: CiString::new("present"),
            ..Default::default()
        }],
        ..Default::default()
    };

    let error = FindSuitableColumnWithIndex(&fixture, "test", &table).unwrap_err();
    assert!(error.message.contains("column missing in test.t"));
}

#[test]
fn show_index_accepts_go_int_domain_and_columns_deduplicate_by_name() {
    let fixture = IndexFixture {
        result: Mutex::new(QueryResult {
            columns: [
                "Table",
                "Non_unique",
                "Key_name",
                "Seq_in_index",
                "Column_name",
                "Cardinality",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            rows: vec![vec![
                "t".into(),
                "1".into(),
                "idx_a".into(),
                (-1_i64).into(),
                "a".into(),
                (-2_i64).into(),
            ]],
        }),
    };
    let parsed = ShowIndex(&fixture, "test", "t").unwrap();
    assert_eq!(parsed[0].SeqInIndex, -1);
    assert_eq!(parsed[0].Cardinality, -2);

    let table = IndexedTable {
        columns: vec![
            ColumnInfo {
                name: CiString::new("same"),
                ..Default::default()
            },
            ColumnInfo {
                name: CiString::new("same"),
                ..Default::default()
            },
        ],
        indices: vec![TableIndexInfo {
            name: "idx".into(),
            columns: vec![0, 1],
            ..Default::default()
        }],
        ..Default::default()
    };
    assert_eq!(FindAllColumnWithIndex(&table).len(), 1);
}

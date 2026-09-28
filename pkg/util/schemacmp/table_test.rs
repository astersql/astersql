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

// 表 schema 格的 join / String 还原单元测试。
//
// 对应 Go `table_test.go`：用 CREATE TABLE 语句构建 `TableInfo`，验证
// Compare/Join 与 `Encode().String()` 输出。

use super::*;
use astersql_ddl::BuildTableInfoFromAST;
use astersql_meta_metabuild::NewNonStrictContext;
use astersql_parser_ast as parse_ast;

#[derive(Clone, Debug)]
/// 一组 schema join 用例：两侧 DDL、期望比较结果与 join 结果/错误。
struct JoinSchemaCase {
    name: &'static str,
    a: &'static str,
    b: &'static str,
    cmp: Option<i32>,
    cmp_err: Option<&'static str>,
    join: Option<&'static str>,
    join_err: Option<&'static str>,
}

/// 解析 CREATE TABLE 并经 metabuild 得到 `TableInfo`。
fn to_table_info(create_table_stmt: &str) -> model::TableInfo {
    let mut parser = astersql_parser::New();
    let node = parser
        .ParseOneStmt(create_table_stmt, "", "")
        .unwrap_or_else(|error| panic!("parse {create_table_stmt}: {error}"));
    let create = node
        .as_any()
        .downcast_ref::<parse_ast::CreateTableStmt>()
        .unwrap_or_else(|| panic!("not a create table statement: {create_table_stmt}"));
    BuildTableInfoFromAST(&NewNonStrictContext(), create)
        .unwrap_or_else(|error| panic!("build table info for {create_table_stmt}: {error}"))
}

/// 断言编码后再解码的列类型与原始 `ColumnInfo` 一致。
fn check_decode_field_types(info: &model::TableInfo, encoded: &Table) {
    let field_types = DecodeColumnFieldTypes(encoded);
    assert_eq!(field_types.len(), info.Columns.len());
    for column in &info.Columns {
        let typ = field_types
            .get(&column.Name.O)
            .unwrap_or_else(|| panic!("missing column {}", column.Name.O));
        assert_eq!(*typ, column.FieldType);
    }
}

/// DM 相关 schema 兼容场景的用例表。
fn join_schema_cases() -> Vec<JoinSchemaCase> {
    macro_rules! success {
        ($name:expr, $a:expr, $b:expr, $cmp:expr, $join:expr) => {
            JoinSchemaCase {
                name: $name,
                a: $a,
                b: $b,
                cmp: Some($cmp),
                cmp_err: None,
                join: Some($join),
                join_err: None,
            }
        };
    }

    macro_rules! compare_error {
        ($name:expr, $a:expr, $b:expr, $cmp_err:expr, $join:expr) => {
            JoinSchemaCase {
                name: $name,
                a: $a,
                b: $b,
                cmp: None,
                cmp_err: Some($cmp_err),
                join: Some($join),
                join_err: None,
            }
        };
    }

    macro_rules! errors {
        ($name:expr, $a:expr, $b:expr, $cmp_err:expr, $join_err:expr) => {
            JoinSchemaCase {
                name: $name,
                a: $a,
                b: $b,
                cmp: None,
                cmp_err: Some($cmp_err),
                join: None,
                join_err: Some($join_err),
            }
        };
    }

    vec![
        success!(
            "DM_002/1",
            "CREATE TABLE tb1 (col1 INT)",
            "CREATE TABLE tb2 (col1 INT, new_col1 INT)",
            -1,
            "CREATE TABLE tb3 (col1 INT, new_col1 INT)"
        ),
        success!(
            "DM_002/1/unordered",
            "CREATE TABLE tb1 (col1 INT)",
            "CREATE TABLE tb2 (new_col1 INT, col1 INT)",
            -1,
            "CREATE TABLE tb3 (new_col1 INT, col1 INT)"
        ),
        success!(
            "DM_002/2",
            "CREATE TABLE tb1 (col1 INT, new_col1 INT)",
            "CREATE TABLE tb2 (col1 INT, new_col1 INT)",
            0,
            "CREATE TABLE tb3 (col1 INT, new_col1 INT)"
        ),
        success!(
            "DM_002/2/unordered",
            "CREATE TABLE tb1 (col1 INT, new_col1 INT)",
            "CREATE TABLE tb2 (new_col1 INT, col1 INT)",
            0,
            "CREATE TABLE tb3 (col1 INT, new_col1 INT)"
        ),
        success!(
            "DM_010",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT, new_col2 INT)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10))",
            1,
            "CREATE TABLE tb3 (a INT, b VARCHAR(10), new_col1 INT, new_col2 INT)"
        ),
        errors!(
            "DM_011",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT, new_col2 INT)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), new_col1 FLOAT)",
            "incompatible mysql type",
            "incompatible mysql type"
        ),
        compare_error!(
            "DM_014",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), new_col2 INT)",
            "combining contradicting orders",
            "CREATE TABLE tb3 (a INT, b VARCHAR(10), new_col1 INT, new_col2 INT)"
        ),
        errors!(
            "DM_031/VARCHAR",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), new_col1 VARCHAR(10))",
            "incompatible mysql type",
            "incompatible mysql type"
        ),
        errors!(
            "DM_031/TEXT",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), new_col1 TEXT)",
            "incompatible mysql type",
            "incompatible mysql type"
        ),
        errors!(
            "DM_031/JSON",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), new_col1 JSON)",
            "incompatible mysql type",
            "incompatible mysql type"
        ),
        compare_error!(
            "DM_033",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), c FLOAT NOT NULL)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10))",
            "column with no default value cannot be missing",
            "CREATE TABLE tb3 (a INT, b VARCHAR(10), c FLOAT NOT NULL DEFAULT 0)"
        ),
        errors!(
            "DM_034",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT UNIQUE AUTO_INCREMENT)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10))",
            "combining contradicting orders",
            "auto type but not defined as a key"
        ),
        success!(
            "DM_035",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), col1 INT, col2 INT)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), col2 INT, col1 INT)",
            0,
            "CREATE TABLE tb3 (a INT, b VARCHAR(10), col1 INT, col2 INT)"
        ),
        errors!(
            "DM_037",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), col1 INT DEFAULT 0)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), col1 INT DEFAULT -1)",
            "distinct singletons",
            "distinct singletons"
        ),
        success!(
            "DM_039/1",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), col1 VARCHAR(10) CHARSET utf8 COLLATE utf8_bin)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10))",
            1,
            "CREATE TABLE tb3 (a INT, b VARCHAR(10), col1 VARCHAR(10) CHARSET utf8 COLLATE utf8_bin)"
        ),
        success!(
            "DM_039/2",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), col1 VARCHAR(10) CHARSET utf8 COLLATE utf8_bin)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), col1 VARCHAR(10) CHARSET utf8 COLLATE utf8_bin)",
            0,
            "CREATE TABLE tb3 (a INT, b VARCHAR(10), col1 VARCHAR(10) CHARSET utf8 COLLATE utf8_bin)"
        ),
        success!(
            "DM_040",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), col1 VARCHAR(10) CHARSET utf8 COLLATE utf8_bin)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), col1 VARCHAR(10) CHARSET utf8mb4 COLLATE utf8mb4_bin)",
            -1,
            "CREATE TABLE tb3 (a INT, b VARCHAR(10), col1 VARCHAR(10) CHARSET utf8mb4 COLLATE utf8mb4_bin)"
        ),
        success!(
            "latin1_to_utf8mb4",
            "CREATE TABLE tb1 (a INT, col1 VARCHAR(10) CHARSET latin1 COLLATE latin1_bin)",
            "CREATE TABLE tb2 (a INT, col1 VARCHAR(10) CHARSET utf8mb4 COLLATE utf8mb4_bin)",
            -1,
            "CREATE TABLE tb3 (a INT, col1 VARCHAR(10) CHARSET utf8mb4 COLLATE utf8mb4_bin)"
        ),
        success!(
            "DM_041/1",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT AS (a + 1))",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10))",
            1,
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT AS (a + 1))"
        ),
        success!(
            "DM_041/2",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT AS (a + 1))",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), new_col1 INT AS (a + 1))",
            0,
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT AS (a + 1))"
        ),
        success!(
            "DM_042",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT AS (a + 1) STORED)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10))",
            1,
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT AS (a + 1) STORED)"
        ),
        errors!(
            "DM_043",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT AS (a + 1))",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), new_col1 INT AS (a + 2))",
            "distinct singletons",
            "distinct singletons"
        ),
        errors!(
            "DM_044",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), new_col1 INT AS (a + 1) VIRTUAL)",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), new_col1 INT AS (a + 1) STORED)",
            "distinct singletons",
            "distinct singletons"
        ),
        compare_error!(
            "DM_052",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10))",
            "CREATE TABLE tb2 (c BIGINT, b VARCHAR(10))",
            "combining contradicting orders",
            "CREATE TABLE tb3 (a INT, b VARCHAR(10), c BIGINT)"
        ),
        errors!(
            "DM_053",
            "CREATE TABLE tb1 (c BIGINT, b VARCHAR(10))",
            "CREATE TABLE tb2 (c DOUBLE, b VARCHAR(10))",
            "incompatible mysql type",
            "incompatible mysql type"
        ),
        success!(
            "DM_055",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10))",
            "CREATE TABLE tb2 (a BIGINT, b VARCHAR(10))",
            -1,
            "CREATE TABLE tb2 (a BIGINT, b VARCHAR(10))"
        ),
        compare_error!(
            "DM_057",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10))",
            "CREATE TABLE tb2 (c INT DEFAULT 1, b VARCHAR(10))",
            "combining contradicting orders",
            "CREATE TABLE tb3 (a INT, b VARCHAR(10), c INT DEFAULT 1)"
        ),
        success!(
            "DM_061",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10))",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10) CHARSET utf8)",
            1,
            "CREATE TABLE tb3 (a INT, b VARCHAR(10))"
        ),
        success!(
            "DM_066",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10))",
            "CREATE TABLE tb2 (a INT DEFAULT 1, b VARCHAR(10))",
            -1,
            "CREATE TABLE tb3 (a INT DEFAULT 1, b VARCHAR(10))"
        ),
        success!(
            "DM_078",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10))",
            "CREATE TABLE tb2 (a INT PRIMARY KEY, b VARCHAR(10))",
            1,
            "CREATE TABLE tb3 (a INT, b VARCHAR(10))"
        ),
        success!(
            "DM_080/1",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), UNIQUE KEY idx_a(a), UNIQUE KEY idx_b(b), UNIQUE KEY idx_ab(a, b))",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10))",
            -1,
            "CREATE TABLE tb3 (a INT, b VARCHAR(10))"
        ),
        success!(
            "DM_080/2",
            "CREATE TABLE tb1 (a INT, b VARCHAR(10), UNIQUE KEY idx_a(a), UNIQUE KEY idx_b(b), UNIQUE KEY idx_ab(a, b))",
            "CREATE TABLE tb2 (a INT, b VARCHAR(10), UNIQUE KEY idx_a(a), UNIQUE KEY idx_b(b))",
            -1,
            "CREATE TABLE tb3 (a INT, b VARCHAR(10), UNIQUE KEY idx_a(a), UNIQUE KEY idx_b(b))"
        ),
        compare_error!(
            "Different index components",
            "CREATE TABLE tbl1 (a INT, b INT, KEY i(a))",
            "CREATE TABLE tbl2 (a INT, b INT, KEY i(b))",
            "combining contradicting orders",
            "CREATE TABLE tbl3 (a INT, b INT)"
        ),
        compare_error!(
            "Different index order",
            "CREATE TABLE tbl1 (a INT, b INT, KEY i(a, b))",
            "CREATE TABLE tbl2 (a INT, b INT, KEY i(b, a))",
            "combining contradicting orders",
            "CREATE TABLE tbl3 (a INT, b INT)"
        ),
        compare_error!(
            "Different index length",
            "CREATE TABLE tbl1 (a TEXT, KEY i(a(14)))",
            "CREATE TABLE tbl2 (a TEXT, KEY i(a(15)))",
            "distinct singletons",
            "CREATE TABLE tbl3 (a TEXT)"
        ),
        errors!(
            "Cannot drop key tied to AUTO_INC column",
            "CREATE TABLE tbl1(a INT AUTO_INCREMENT, b INT, KEY i(a))",
            "CREATE TABLE tbl2(a INT AUTO_INCREMENT, b INT, KEY i(a, b))",
            "distinct singletons",
            "auto type but not defined as a key"
        ),
        compare_error!(
            "not-null column with special types",
            "CREATE TABLE tbl1(a1 INT NOT NULL, b1 DECIMAL NOT NULL, c1 VARCHAR(20) NOT NULL, d1 DATETIME(3) NOT NULL, e1 ENUM('abc', 'def') NOT NULL)",
            "CREATE TABLE tbl2(a2 TIME NOT NULL, b2 DATE NOT NULL, c2 BINARY(50) NOT NULL, d2 YEAR(4) NOT NULL, e2 SET('abc', 'def') NOT NULL)",
            "column with no default value cannot be missing",
            "CREATE TABLE tbl3(a1 INT NOT NULL DEFAULT 0, b1 DECIMAL NOT NULL DEFAULT 0, c1 VARCHAR(20) NOT NULL DEFAULT '', d1 DATETIME(3) NOT NULL DEFAULT '0000-00-00 00:00:00', e1 ENUM('abc', 'def') NOT NULL DEFAULT 'abc', a2 TIME NOT NULL DEFAULT '00:00:00', b2 DATE NOT NULL DEFAULT '0000-00-00', c2 BINARY(50) NOT NULL DEFAULT '', d2 YEAR(4) NOT NULL DEFAULT '0000', e2 SET('abc', 'def') NOT NULL DEFAULT '')"
        ),
        success!(
            "test case 2020-03-17",
            "CREATE TABLE bar (id INT PRIMARY KEY)",
            "CREATE TABLE bar (id INT PRIMARY KEY, c1 INT)",
            -1,
            "CREATE TABLE bar (id INT PRIMARY KEY, c1 INT)"
        ),
        success!(
            "test case 2020-03-17-alt",
            "CREATE TABLE bar (id VARCHAR(10) PRIMARY KEY)",
            "CREATE TABLE bar (id VARCHAR(10) PRIMARY KEY, c1 INT)",
            -1,
            "CREATE TABLE bar (id VARCHAR(10) PRIMARY KEY, c1 INT)"
        ),
        success!(
            "test case 2020-03-17-alt-2",
            "CREATE TABLE bar (id INT PRIMARY KEY)",
            "CREATE TABLE bar (id INT, c1 INT)",
            -1,
            "CREATE TABLE bar (id INT, c1 INT)"
        ),
        compare_error!(
            "test case 2020-03-17-alt-3",
            "CREATE TABLE bar (id1 INT PRIMARY KEY, id2 INT)",
            "CREATE TABLE bar (id1 INT, id2 INT PRIMARY KEY)",
            "combining contradicting orders",
            "CREATE TABLE bar (id1 INT, id2 INT)"
        ),
        success!(
            "test case 2020-04-28-blob",
            "CREATE TABLE tb1 (a BLOB, b VARCHAR(10))",
            "CREATE TABLE tb2 (a LONGBLOB, b VARCHAR(10))",
            -1,
            "CREATE TABLE tb2 (a LONGBLOB, b VARCHAR(10))"
        ),
        success!(
            "join equal single primary key",
            "CREATE TABLE t(a INT, b INT, PRIMARY KEY(a))",
            "CREATE TABLE t(a INT, b INT, PRIMARY KEY(a))",
            0,
            "CREATE TABLE t(a INT, b INT, PRIMARY KEY(a))"
        ),
        success!(
            "join equal composite primary key",
            "CREATE TABLE t(a INT, b INT, c INT, PRIMARY KEY(a, b))",
            "CREATE TABLE t(a INT, b INT, c INT, PRIMARY KEY(a, b))",
            0,
            "CREATE TABLE t(a INT, b INT, c INT, PRIMARY KEY(a, b))"
        ),
        success!(
            "join equal single index",
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, INDEX idx_b(b))",
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, INDEX idx_b(b))",
            0,
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, INDEX idx_b(b))"
        ),
        success!(
            "join equal unique index",
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, UNIQUE KEY uni_b(b))",
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, UNIQUE KEY uni_b(b))",
            0,
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, UNIQUE KEY uni_b(b))"
        ),
        success!(
            "join equal composite index",
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, INDEX idx_bc(b, c))",
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, INDEX idx_bc(b, c))",
            0,
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, INDEX idx_bc(b, c))"
        ),
        success!(
            "join equal composite unique index",
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, UNIQUE INDEX idx_bc(b, c))",
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, UNIQUE INDEX idx_bc(b, c))",
            0,
            "CREATE TABLE t(a INT PRIMARY KEY, b INT, c INT, UNIQUE INDEX idx_bc(b, c))"
        ),
    ]
}

// TestJoinSchemas 对应 Go 的 TestJoinSchemas。
#[test]
/// 跑完所有 join 用例，校验 Compare/Join 与字段类型 round-trip。
fn test_join_schemas() {
    for tc in join_schema_cases() {
        let tia = to_table_info(tc.a);
        let tib = to_table_info(tc.b);
        let a = Encode(&tia);
        let b = Encode(&tib);
        check_decode_field_types(&tia, &a);
        check_decode_field_types(&tib, &b);

        match (tc.cmp, tc.cmp_err) {
            (Some(expected), None) => {
                assert_eq!(a.Compare(b.clone()).unwrap(), expected, "{}", tc.name);
                assert_eq!(b.Compare(a.clone()).unwrap(), -expected, "{}", tc.name);
            }
            (None, Some(needle)) => {
                let err = match a.Compare(b.clone()) {
                    Ok(result) => {
                        panic!("{} compare unexpectedly succeeded: {result}", tc.name)
                    }
                    Err(error) => error.to_string(),
                };
                assert!(
                    err.contains(needle),
                    "{} compare err `{err}` missing `{needle}`",
                    tc.name
                );
                let reverse_err = match b.Compare(a.clone()) {
                    Ok(result) => {
                        panic!(
                            "{} reverse compare unexpectedly succeeded: {result}",
                            tc.name
                        )
                    }
                    Err(error) => error.to_string(),
                };
                assert!(
                    reverse_err.contains(needle),
                    "{} reverse compare err `{reverse_err}` missing `{needle}`",
                    tc.name
                );
            }
            _ => panic!("{} invalid compare expectation", tc.name),
        }

        match (tc.join, tc.join_err) {
            (Some(join_sql), None) => {
                let expected = Encode(&to_table_info(join_sql));
                let joined = a.Join(b.clone()).unwrap();
                let reverse_joined = b.Join(a.clone()).unwrap();
                assert_eq!(
                    joined.Compare(expected.clone()).unwrap(),
                    0,
                    "{} join mismatch\njoined={}\nexpected={}",
                    tc.name,
                    joined.String(),
                    expected.String()
                );
                assert_eq!(
                    reverse_joined.Compare(expected.clone()).unwrap(),
                    0,
                    "{} reverse join mismatch\njoined={}\nexpected={}",
                    tc.name,
                    reverse_joined.String(),
                    expected.String()
                );
                assert!(joined.Compare(a.clone()).unwrap() >= 0, "{}", tc.name);
                assert!(joined.Compare(b.clone()).unwrap() >= 0, "{}", tc.name);
            }
            (None, Some(needle)) => {
                let err = match a.Join(b.clone()) {
                    Ok(_) => panic!("{} join unexpectedly succeeded", tc.name),
                    Err(error) => error.to_string(),
                };
                assert!(
                    err.contains(needle),
                    "{} join err `{err}` missing `{needle}`",
                    tc.name
                );
                let reverse_err = match b.Join(a) {
                    Ok(_) => panic!("{} reverse join unexpectedly succeeded", tc.name),
                    Err(error) => error.to_string(),
                };
                assert!(
                    reverse_err.contains(needle),
                    "{} reverse join err `{reverse_err}` missing `{needle}`",
                    tc.name
                );
            }
            _ => panic!("{} invalid join expectation", tc.name),
        }
    }
}

// TestTableString 对应 Go 的 TestTableString。
#[test]
/// 校验 `Encode().String()` 与期望的小写 SQL（含 charset/collate 规范化）。
fn test_table_string() {
    let cases = [
        (
            "CREATE TABLE tb (a INT, b INT)",
            "create table `tbl`(`a` int(11), `b` int(11)) collate utf8mb4_bin",
        ),
        (
            "CREATE TABLE tb (a VARCHAR(20) CHARACTER SET utf8, b INT)",
            "create table `tbl`(`a` varchar(20) character set utf8 collate utf8_bin, `b` int(11)) collate utf8mb4_bin",
        ),
        (
            "CREATE TABLE tb (a VARCHAR(20), b INT) COLLATE utf8mb4_general_ci",
            "create table `tbl`(`a` varchar(20) character set utf8mb4 collate utf8mb4_general_ci, `b` int(11)) collate utf8mb4_general_ci",
        ),
        (
            "CREATE TABLE tb (a VARCHAR(20) CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci, b INT)",
            "create table `tbl`(`a` varchar(20) character set utf8mb4 collate utf8mb4_general_ci, `b` int(11)) collate utf8mb4_bin",
        ),
        (
            "CREATE TABLE tb (a VARCHAR(20)) CHARSET=binary",
            "create table `tbl`(`a` varbinary(20)) collate binary",
        ),
        (
            "CREATE TABLE tb (a VARCHAR(20)) COLLATE=binary",
            "create table `tbl`(`a` varbinary(20)) collate binary",
        ),
        (
            "CREATE TABLE tb (a VARCHAR(20)) CHARSET=binary COLLATE=binary",
            "create table `tbl`(`a` varbinary(20)) collate binary",
        ),
    ];

    for (input, expect) in cases {
        let encoded = Encode(&to_table_info(input));
        assert_eq!(encoded.String().to_lowercase(), expect, "input={input}");
    }
}

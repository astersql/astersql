// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// SQL 限制规则单元测试。
//
// 用真实 parser 解析语句后套用各 `SQLRule`，对齐 Go 表驱动用例（TTL、ATTRIBUTES、
// SELECT INTO OUTFILE、本地 IMPORT/LOAD DATA 等）。

/// 表驱动：对每条 SQL 新建 parser，断言规则布尔结果与 expected 一致。
#[test]
fn test_sql_rules() {
    struct TestCase {
        rule: SQLRule,
        stmt: &'static str,
        expected: bool,
    }

    let testCases = vec![
        TestCase {
            rule: TimeToLiveSQLRule,
            stmt: "CREATE TABLE t (a DATETIME) TTL = a + INTERVAL 1 DAY",
            expected: true,
        },
        TestCase {
            rule: TimeToLiveSQLRule,
            stmt: "ALTER TABLE t TTL = a + INTERVAL 1 DAY",
            expected: true,
        },
        TestCase {
            rule: TimeToLiveSQLRule,
            stmt: "ALTER TABLE t REMOVE TTL",
            expected: true,
        },
        TestCase {
            rule: AlterTableAttributesRule,
            stmt: "ALTER TABLE t ATTRIBUTES 'merge_option=deny'",
            expected: true,
        },
        TestCase {
            rule: ImportWithExternalIDRule,
            stmt: "IMPORT INTO xxxx FROM 's3://xxx/xxx?external-id=xxx'",
            expected: false,
        },
        TestCase {
            rule: SelectIntoFileRule,
            stmt: "SELECT * FROM t1 INTO OUTFILE '/tmp/t1.txt' ",
            expected: true,
        },
        TestCase {
            rule: ImportFromLocalRule,
            stmt: "IMPORT INTO t1 FROM '/bucket/path/to/file.csv'",
            expected: true,
        },
        TestCase {
            rule: ImportFromLocalRule,
            stmt: "IMPORT INTO t1 FROM 'file:///bucket/path/to/file.csv'",
            expected: true,
        },
        TestCase {
            rule: ImportFromLocalRule,
            stmt: "LOAD DATA INFILE '/bucket/path/to/file.csv' INTO TABLE t1",
            expected: true,
        },
        TestCase {
            rule: ImportFromLocalRule,
            stmt: "LOAD DATA INFILE 'file:///bucket/path/to/file.csv' INTO TABLE t1",
            expected: true,
        },
        TestCase {
            rule: ImportFromLocalRule,
            stmt: "LOAD DATA LOCAL INFILE 'file:///bucket/path/to/file.csv' INTO TABLE t1",
            expected: false,
        },
    ];

    let charset = parser::mysql::DefaultCharset;
    let collate = parser::mysql::DefaultCollationName;
    for c in testCases {
        // Go 每个 case 新建 parser.New()，避免 parser 内部状态跨 case 复用。
        let mut p = parser::New();

        let stmt = match p.ParseOneStmt(c.stmt, charset, collate) {
            Ok(stmt) => stmt,
            Err(err) => {
                // 原 Go 测试直接 panic(err)，这里保留解析失败即中止测试的语义。
                panic!("{:?}", err);
            }
        };
        let result = (c.rule)(stmt.as_ref());
        assert_eq!(
            c.expected, result,
            "SQL rule failed for statement: {}",
            c.stmt
        );
    }
}

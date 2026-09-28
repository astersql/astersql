// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// LATERAL 派生表语法测试，对照 `lateral_test.go`。
//
// LATERAL 允许派生表引用其左侧 FROM 项的列。本文件校验多种 JOIN 形态下的
// 解析成功/失败、NormalizeKeepHint 往返，以及列别名是否保留在规范化文本中。

// 描述 LATERAL 语法解析、Restore 往返和 AST 标志检查。
/// 单条 LATERAL 解析用例：SQL、是否期望失败、是否含 LATERAL、列别名列表。
struct LateralCase {
    name: &'static str,
    sql: &'static str,
    expect_error: bool,
    check_lateral: bool,
    column_names: &'static [&'static str],
}

// test_lateral_parsing 对应 Go 的 TestLateralParsing：逐条解析 LATERAL 派生表 SQL，并验证 restore 往返。
/// 表驱动解析 LATERAL SQL，并经 NormalizeKeepHint 做往返与别名检查。
#[test]
fn test_lateral_parsing() {
    let mut p = parser::New();
    let test_cases = vec![
        LateralCase {
            name: "LATERAL with comma syntax",
            sql: "SELECT * FROM t1, LATERAL (SELECT t1.a) AS dt",
            expect_error: false,
            check_lateral: true,
            column_names: &[],
        },
        LateralCase {
            name: "LATERAL with LEFT JOIN",
            sql: "SELECT * FROM t1 LEFT JOIN LATERAL (SELECT t1.b) AS dt ON true",
            expect_error: false,
            check_lateral: true,
            column_names: &[],
        },
        LateralCase {
            name: "LATERAL with CROSS JOIN",
            sql: "SELECT * FROM t1 CROSS JOIN LATERAL (SELECT t1.c) AS dt",
            expect_error: false,
            check_lateral: true,
            column_names: &[],
        },
        LateralCase {
            name: "LATERAL with RIGHT JOIN",
            sql: "SELECT * FROM t1 RIGHT JOIN LATERAL (SELECT t1.d) AS dt ON true",
            expect_error: false,
            check_lateral: true,
            column_names: &[],
        },
        LateralCase {
            name: "LATERAL with INNER JOIN",
            sql: "SELECT * FROM t1 JOIN LATERAL (SELECT t1.e) AS dt ON true",
            expect_error: false,
            check_lateral: true,
            column_names: &[],
        },
        LateralCase {
            name: "LATERAL with complex subquery",
            sql: "SELECT * FROM t1, LATERAL (SELECT t1.a, COUNT(*) FROM t2 WHERE t2.x = t1.x GROUP BY t1.a) AS dt",
            expect_error: false,
            check_lateral: true,
            column_names: &[],
        },
        LateralCase {
            name: "LATERAL with nested subquery",
            sql: "SELECT * FROM t1, LATERAL (SELECT * FROM (SELECT t1.a) AS inner_dt) AS dt",
            expect_error: false,
            check_lateral: true,
            column_names: &[],
        },
        LateralCase {
            name: "Multiple LATERAL joins",
            sql: "SELECT * FROM t1, LATERAL (SELECT t1.a) AS dt1, LATERAL (SELECT t1.b) AS dt2",
            expect_error: false,
            check_lateral: true,
            column_names: &[],
        },
        LateralCase {
            name: "Non-LATERAL derived table",
            sql: "SELECT * FROM t1, (SELECT a FROM t2) AS dt",
            expect_error: false,
            check_lateral: false,
            column_names: &[],
        },
        LateralCase {
            name: "LATERAL with WHERE clause",
            sql: "SELECT * FROM t1, LATERAL (SELECT * FROM t2 WHERE t2.x = t1.x) AS dt WHERE dt.y > 10",
            expect_error: false,
            check_lateral: true,
            column_names: &[],
        },
        LateralCase {
            name: "LATERAL with column list",
            sql: "SELECT * FROM t1, LATERAL (SELECT t1.a, t1.b) AS dt(c1, c2)",
            expect_error: false,
            check_lateral: true,
            column_names: &["c1", "c2"],
        },
        LateralCase {
            name: "LATERAL with column list no AS",
            sql: "SELECT * FROM t1, LATERAL (SELECT t1.a) dt(col1)",
            expect_error: false,
            check_lateral: true,
            column_names: &["col1"],
        },
        LateralCase {
            name: "LATERAL with column list and JOIN",
            sql: "SELECT * FROM t1 LEFT JOIN LATERAL (SELECT t1.a, t1.b, t1.c) AS dt(x, y, z) ON true",
            expect_error: false,
            check_lateral: true,
            column_names: &["x", "y", "z"],
        },
        // MySQL/TiDB 要求 LATERAL 派生表必须带别名。
        LateralCase {
            name: "LATERAL without alias is rejected",
            sql: "SELECT * FROM t1, LATERAL (SELECT t1.a)",
            expect_error: true,
            check_lateral: false,
            column_names: &[],
        },
    ];

    for tc in test_cases {
        let parsed = p.ParseOneStmt(tc.sql, "", "");
        if tc.expect_error {
            assert!(
                parsed.is_err(),
                "{}: expected parsing to fail: {}",
                tc.name,
                tc.sql
            );
            continue;
        }
        let stmt =
            parsed.unwrap_or_else(|err| panic!("{}: failed to parse {}: {err:?}", tc.name, tc.sql));

        // The public AST currently has no restore facade in the integration crate. NormalizeKeepHint
        // provides the same parser-owned token round trip and preserves LATERAL and column aliases.
        // 公共 AST 暂无 restore 门面；NormalizeKeepHint 做同权的 token 往返并保留 LATERAL/列别名。
        let restored = parser::digester_impl::NormalizeKeepHint(tc.sql);
        let round_trip_stmt = p.ParseOneStmt(&restored, "", "").unwrap_or_else(|err| {
            panic!(
                "{}: failed normalized round trip {restored}: {err:?}",
                tc.name
            )
        });

        for (label, stmt_to_check) in [
            ("original", stmt.as_ref()),
            ("round-trip", round_trip_stmt.as_ref()),
        ] {
            let select_stmt = stmt_to_check
                .as_any()
                .downcast_ref::<parser_ast::SelectStmt>()
                .unwrap_or_else(|| panic!("{} [{label}]: statement should be SelectStmt", tc.name));
            let from = select_stmt
                .From
                .as_ref()
                .unwrap_or_else(|| panic!("{} [{label}]: FROM clause should not be nil", tc.name));
            let lateral = find_lateral_table_source(&from.TableRefs);

            if tc.check_lateral {
                let lateral = lateral.unwrap_or_else(|| {
                    panic!("{} [{label}]: LATERAL TableSource not found", tc.name)
                });
                assert_eq!(
                    lateral.ColumnNames.len(),
                    tc.column_names.len(),
                    "{} [{label}]: column name count mismatch",
                    tc.name
                );
                for (actual, expected) in lateral.ColumnNames.iter().zip(tc.column_names) {
                    assert_eq!(
                        actual.L, *expected,
                        "{} [{label}]: column name mismatch",
                        tc.name
                    );
                }
            } else {
                assert!(
                    lateral.is_none(),
                    "{} [{label}]: Lateral should be false for non-LATERAL query",
                    tc.name
                );
            }
        }
    }
}

// find_lateral_table_source 对应 Go helper：递归查找 JOIN 和派生表子查询中的
// 首个 LATERAL TableSource，并返回其 AST 以继续核对列别名。
/// 在结果集 AST 中递归查找首个 LATERAL 派生表。
fn find_lateral_table_source(node: &parser_ast::Join) -> Option<parser_ast::TableSource> {
    node.Left
        .as_deref()
        .and_then(find_lateral_result_set)
        .or_else(|| node.Right.as_deref().and_then(find_lateral_result_set))
}

fn find_lateral_result_set(node: &parser_ast::ResultSetNode) -> Option<parser_ast::TableSource> {
    match node {
        parser_ast::ResultSetNode::TableSource(table_source) => {
            if table_source.Lateral {
                Some(table_source.clone())
            } else {
                table_source
                    .QuerySource
                    .as_ref()
                    .and_then(|source| {
                        source.with_node(|node| {
                            node.as_any()
                                .downcast_ref::<parser_ast::SelectStmt>()
                                .and_then(|select| select.From.as_ref())
                                .and_then(|from| find_lateral_table_source(&from.TableRefs))
                        })
                    })
                    .flatten()
            }
        }
        parser_ast::ResultSetNode::Join(join) => find_lateral_table_source(join),
    }
}

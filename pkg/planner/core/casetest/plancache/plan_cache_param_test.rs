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

// Plan cache 参数化 SQL（parameterized SQL）语义测试。
//
// 参数化把常量替换为 `?` 占位符并抽出 Datum 列表，使结构相同、字面量不同的
// 语句共享同一缓存计划键；还原（Restore）再用参数回填 AST/SQL。

/// 对齐 Go `TestParameterize` 的全部表驱动场景。
#[test]
fn parameterize_matches_go_cases() {
    use astersql_planner_core::{Datum, ParameterizeAST};

    let cases = [
        (
            "select * from t where a<10",
            "SELECT * FROM `t` WHERE `a`<?",
            vec![Datum::Int(10)],
        ),
        ("select * from t", "SELECT * FROM `t`", vec![]),
        (
            "select * from t where a<10 and b<20 and c=30 and d>40",
            "SELECT * FROM `t` WHERE `a`<? AND `b`<? AND `c`=? AND `d`>?",
            vec![
                Datum::Int(10),
                Datum::Int(20),
                Datum::Int(30),
                Datum::Int(40),
            ],
        ),
        (
            "select * from t where a='a' and b='bbbbbbbbbbbbbbbbbbbbbbbb'",
            "SELECT * FROM `t` WHERE `a`=? AND `b`=?",
            vec![
                Datum::String("a".into()),
                Datum::String("bbbbbbbbbbbbbbbbbbbbbbbb".into()),
            ],
        ),
        (
            "select 1, 2, 3 from t where a<10",
            "SELECT 1,2,3 FROM `t` WHERE `a`<?",
            vec![Datum::Int(10)],
        ),
        (
            "select a+1 from t where a<10",
            "SELECT a+1 FROM `t` WHERE `a`<?",
            vec![Datum::Int(10)],
        ),
        (
            r#"select a+ "a b c" from t"#,
            r#"SELECT a+ "a b c" FROM `t`"#,
            vec![],
        ),
        (
            r#"select a + 'a b c'+"x" from t"#,
            r#"SELECT a + 'a b c'+"x" FROM `t`"#,
            vec![],
        ),
        (
            r#"select a + 'a b c'+"x" as 'xxx' from t"#,
            r#"SELECT a + 'a b c'+"x" as 'xxx' FROM `t`"#,
            vec![],
        ),
        (
            "insert into t (a, B, c) values (1, 2, 3), (4, 5, 6)",
            "INSERT INTO `t` (`a`,`B`,`c`) VALUES (?,?,?),(?,?,?)",
            vec![
                Datum::Int(1),
                Datum::Int(2),
                Datum::Int(3),
                Datum::Int(4),
                Datum::Int(5),
                Datum::Int(6),
            ],
        ),
        (
            "select * from t where a < date_format('2020-02-02', '%Y-%m-%d')",
            "SELECT * FROM `t` WHERE `a`<date_format(?, '%Y-%m-%d')",
            vec![Datum::String("2020-02-02".into())],
        ),
        (
            "select * from `txu#p#p1`",
            "SELECT * FROM `txu#p#p1`",
            vec![],
        ),
        (
            "select * from t limit 10",
            "SELECT * FROM `t` LIMIT 10",
            vec![],
        ),
        (
            "select * from t limit 10, 20",
            "SELECT * FROM `t` LIMIT 10,20",
            vec![],
        ),
    ];

    for (sql, expected_sql, expected_params) in cases {
        let (parameterized, params) = ParameterizeAST(sql);
        assert_eq!(parameterized, expected_sql, "SQL: {sql}");
        assert_eq!(params, expected_params, "SQL: {sql}");
    }
}

/// 对齐 Go `TestGetParamSQLFromASTConcurrently`：50 个不同 AST 各并发参数化
/// 100 次，不共享可变状态，也不会交叉污染参数值或顺序。
#[test]
fn get_param_sql_from_ast_matches_go_concurrency_contract() {
    use astersql_planner_core::{Datum, GetParamSQLFromAST};
    use std::{thread, time::Duration};

    let workers = (0..50)
        .map(|id| {
            thread::spawn(move || {
                let sql = format!(
                    "insert into t values ({}, {}, {})",
                    id * 3,
                    id * 3 + 1,
                    id * 3 + 2
                );
                for iteration in 0..100 {
                    let (parameterized, values) = GetParamSQLFromAST(&sql);
                    assert_eq!(parameterized, "INSERT INTO `t` VALUES (?,?,?)");
                    assert_eq!(
                        values,
                        vec![
                            Datum::Int(id * 3),
                            Datum::Int(id * 3 + 1),
                            Datum::Int(id * 3 + 2),
                        ]
                    );
                    thread::sleep(Duration::from_micros(1000 + (iteration + id) as u64));
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("parameterization worker");
    }
}

/// 验证 WHERE 参数保留顺序、Params2Expressions 恒等，以及还原/再解析往返。
#[test]
fn parameterized_sql_preserves_order_and_restores_values() {
    use astersql_planner_core::{
        Datum, GetParamSQLFromAST, Params2Expressions, ParseParameterizedSQL, RestoreASTWithParams,
    };

    let (parameterized, values) =
        GetParamSQLFromAST("select * from orders where customer_id = 1 and region_id = 42");
    assert_eq!(
        parameterized,
        "SELECT * FROM `orders` WHERE `customer_id`=? AND `region_id`=?"
    );
    assert_eq!(values, vec![Datum::Int(1), Datum::Int(42)]);
    assert_eq!(Params2Expressions(&values), values);
    assert_eq!(
        RestoreASTWithParams(&parameterized, &values).unwrap(),
        "SELECT * FROM `orders` WHERE `customer_id`=1 AND `region_id`=42"
    );
    assert_eq!(
        ParseParameterizedSQL(&parameterized).unwrap(),
        parameterized
    );
}

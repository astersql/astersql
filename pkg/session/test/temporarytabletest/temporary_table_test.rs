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

// 本地/全局临时表事务与 DDL 解析测试。
//
// 可执行测试校验 `TEMPORARY`/`GLOBAL TEMPORARY` 解析标志，以及 `PlanUpdate`/`PlanDelete`
// 从 WHERE 抽取点查谓词（列/运算符/字面量）并匹配行。
use astersql_parser as parser;
use astersql_parser_ast::{ColumnOptionType, TemporaryKeyword};
use astersql_session::dml_runtime::{MatchesPredicate, PlanDelete, PlanUpdate};
use astersql_testkit::{DbValue, NewTestKit, Rows, mockstore::CreateMockStoreAndDomain};
use std::collections::HashMap;

/// 解析单条 SQL 为 AST 节点；失败则 panic，便于测试内联断言。
fn parse_one(sql: &str) -> Box<dyn parser::ast::Node> {
    parser::New()
        .ParseOneStmt(sql, "", "")
        .unwrap_or_else(|error| panic!("parse {sql:?}: {error}"))
}

/// 将 SQL 解析为 `CreateTableStmt`，用于检查 TemporaryKeyword / OnCommitDelete 等标志。
fn create_table(sql: &str) -> parser::ast::CreateTableStmt {
    *parse_one(sql)
        .into_any()
        .downcast::<parser::ast::CreateTableStmt>()
        .unwrap_or_else(|_| panic!("{sql:?} did not parse as CreateTableStmt"))
}

// 对应 TestLocalTemporaryTableUpdate/TestLocalTemporaryTableDelete 建表前置条件：
// `create temporary table` 必须被解析为 TemporaryLocal，且列级 primary key / unique
// 约束要能在 AST 上被识别，供后续 DDL/DML 逻辑区分本地临时表与普通表。
#[test]
fn local_temporary_table_ddl_parses_temporary_local_keyword_and_column_constraints() {
    let stmt =
        create_table("create temporary table tmp1 (id int primary key, u int unique, v int)");

    assert_eq!(stmt.TemporaryKeyword, TemporaryKeyword::Local);
    assert!(!stmt.OnCommitDelete);
    assert_eq!(stmt.Table.Name.L, "tmp1");
    assert_eq!(stmt.Cols.len(), 3);

    let id_column = &stmt.Cols[0];
    assert_eq!(id_column.Name.Name.L, "id");
    assert!(
        id_column
            .Options
            .iter()
            .any(|option| option.Tp == ColumnOptionType::PrimaryKey)
    );

    let u_column = &stmt.Cols[1];
    assert_eq!(u_column.Name.Name.L, "u");
    assert!(
        u_column
            .Options
            .iter()
            .any(|option| option.Tp == ColumnOptionType::UniqueKey)
    );
}

// 对应 TestSchemaCheckerTempTable 建表前置条件：`create global temporary table ... on commit
// delete rows` 必须解析为 TemporaryGlobal 且 OnCommitDelete=true，这是全局临时表区别于
// 本地临时表、且事务提交后清空数据的关键标志位。
#[test]
fn global_temporary_table_ddl_parses_on_commit_delete_rows_flag() {
    let stmt = create_table(
        "create global temporary table temp_table (id int primary key, c int) on commit delete rows",
    );

    assert_eq!(stmt.TemporaryKeyword, TemporaryKeyword::Global);
    assert!(stmt.OnCommitDelete);
    assert_eq!(stmt.Table.Name.L, "temp_table");
}

// 当前语法要求 GLOBAL TEMPORARY 必须与 ON COMMIT DELETE ROWS 同时出现，省略该子句应报语法
// 错误，而不是静默退化为某个默认值；这与 Go 语法保持一致（GLOBAL TEMPORARY 建表必须显式声明
// 提交行为）。
#[test]
fn global_temporary_table_requires_on_commit_delete_rows_clause() {
    let error = match parser::New().ParseOneStmt(
        "create global temporary table temp_table2 (id int primary key, c int)",
        "",
        "",
    ) {
        Ok(_) => panic!("GLOBAL TEMPORARY without ON COMMIT DELETE ROWS must fail to parse"),
        Err(error) => error,
    };

    assert!(
        error
            .to_string()
            .contains("GLOBAL TEMPORARY and ON COMMIT DELETE ROWS must appear together")
    );
}

// 对应 TestLocalTemporaryTableUpdate/TestLocalTemporaryTableDelete 中大量 UpdateCase/
// assert_delete 用例共享的底层机制：真实的 `dml_runtime::PlanUpdate`/`PlanDelete` 必须能从
// WHERE 子句里正确抽取 column/op/literal 三元组，且 `MatchesPredicate` 要能按该三元组
// 正确筛选行——这正是临时表点查/范围更新删除路径依赖的核心谓词逻辑。
#[test]
fn plan_update_and_delete_extract_where_equals_predicate_for_point_lookups() {
    let update = parse_one("update tmp1 set v=999 where id=1")
        .into_any()
        .downcast::<parser::ast::UpdateStmt>()
        .unwrap_or_else(|_| panic!("expected UpdateStmt"));
    let update_plan = PlanUpdate(&update).expect("point-get UPDATE should plan successfully");
    assert_eq!(update_plan.Table, "tmp1");
    assert_eq!(update_plan.PredicateColumn, "id");
    assert_eq!(update_plan.PredicateOp, "=");
    assert_eq!(update_plan.Key, "1");

    let delete = parse_one("delete from tmp1 where u=102")
        .into_any()
        .downcast::<parser::ast::DeleteStmt>()
        .unwrap_or_else(|_| panic!("expected DeleteStmt"));
    let delete_plan = PlanDelete(&delete).expect("point-get DELETE should plan successfully");
    assert_eq!(delete_plan.Table, "tmp1");
    assert_eq!(delete_plan.PredicateColumn, "u");
    assert_eq!(delete_plan.PredicateOp, "=");
    assert_eq!(delete_plan.Key, "102");

    let matching_row = HashMap::from([
        ("id".to_owned(), Some("1".to_owned())),
        ("u".to_owned(), Some("101".to_owned())),
    ]);
    let other_row = HashMap::from([
        ("id".to_owned(), Some("2".to_owned())),
        ("u".to_owned(), Some("102".to_owned())),
    ]);
    assert!(
        MatchesPredicate(
            &matching_row,
            &update_plan.PredicateColumn,
            &update_plan.PredicateOp,
            &update_plan.Key,
        )
        .unwrap()
    );
    assert!(
        !MatchesPredicate(
            &other_row,
            &update_plan.PredicateColumn,
            &update_plan.PredicateOp,
            &update_plan.Key,
        )
        .unwrap()
    );
    assert!(
        MatchesPredicate(
            &other_row,
            &delete_plan.PredicateColumn,
            &delete_plan.PredicateOp,
            &delete_plan.Key,
        )
        .unwrap()
    );
}

// 对应 Go 单表 UPDATE：临时表与普通关系表一样保留 ORDER BY/LIMIT，由关系执行器按顺序
// 选择待更新行；只有专用会话 KV 表不能走该路径。
#[test]
fn plan_update_preserves_order_and_limit_for_relational_temporary_tables() {
    let update = parse_one("update tmp1 set v=1 order by id limit 1")
        .into_any()
        .downcast::<parser::ast::UpdateStmt>()
        .unwrap_or_else(|_| panic!("expected UpdateStmt"));

    let plan = PlanUpdate(&update).expect("relational ORDER BY + LIMIT UPDATE must be planned");
    assert_eq!(plan.Order.len(), 1);
    assert!(plan.Limit.is_some());
}

// Go `TestLocalTemporaryTableUpdate` 的真实 SQL 最小回归：本地临时表的行属于会话，
// 在显式事务中更新后提交可见，并且不会被普通表路径替代。
#[test]
fn local_temporary_table_update_commits_session_local_rows() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    let mut peer = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create temporary table tmp1 (id int primary key, u int unique, v int)",
        Vec::new(),
    );
    tk.MustExec("insert into tmp1 values (1, 101, 1001)", Vec::new());

    tk.MustExec("begin", Vec::new());
    tk.MustExec("update tmp1 set v = 999 where id = 1", Vec::new());
    tk.MustExec("commit", Vec::new());

    tk.MustQuery("select * from tmp1", Vec::new())
        .Check(Rows(&["1 101 999"]));
    peer.MustQueryToErr("select * from tmp1");

    peer.MustExec(
        "create temporary table tmp1 (id int primary key, u int unique, v int)",
        Vec::new(),
    );
    peer.MustExec("insert into tmp1 values (2, 202, 2002)", Vec::new());
    peer.MustQuery("select * from tmp1", Vec::new())
        .Check(Rows(&["2 202 2002"]));
    tk.MustQuery("select * from tmp1", Vec::new())
        .Check(Rows(&["1 101 999"]));

    tk.MustExec("drop table tmp1", Vec::new());
    tk.MustQueryToErr("select * from tmp1");
    peer.MustQuery("select * from tmp1", Vec::new())
        .Check(Rows(&["2 202 2002"]));
}

fn insert_temporary_rows(tk: &mut astersql_testkit::TestKit, ids: &[i32]) {
    for id in ids {
        tk.MustExec(
            "insert into tmp1 values (?, ?, ?)",
            vec![
                DbValue::I64(i64::from(*id)),
                DbValue::I64(i64::from(id + 100)),
                DbValue::I64(i64::from(id + 1000)),
            ],
        );
    }
}

fn local_temporary_rows(ids: &[i32]) -> Vec<String> {
    ids.iter()
        .map(|id| format!("{id} {} {}", id + 100, id + 1000))
        .collect()
}

fn assert_temporary_rows(tk: &mut astersql_testkit::TestKit, expected: &[String]) {
    let expected = expected.iter().map(String::as_str).collect::<Vec<_>>();
    tk.MustQuery("select * from tmp1", Vec::new())
        .Check(Rows(&expected));
}

fn assert_temporary_rows_and_unique_index(
    tk: &mut astersql_testkit::TestKit,
    expected_ids: &[i32],
) {
    assert_temporary_rows(tk, &local_temporary_rows(expected_ids));
    let expected_unique_values = expected_ids
        .iter()
        .map(|id| (id + 100).to_string())
        .collect::<Vec<_>>();
    let expected_unique_values = expected_unique_values
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    tk.MustQuery(
        "select /*+ use_index(tmp1, u) */ u from tmp1 order by u",
        Vec::new(),
    )
    .Check(Rows(&expected_unique_values));
}

fn update_expected_rows(updates: &[&str], deleted: &[i32]) -> Vec<String> {
    let mut changed = std::collections::BTreeMap::<i32, Option<String>>::new();
    for update in updates {
        let id = update
            .split_whitespace()
            .next()
            .expect("update row must start with an ID")
            .parse::<i32>()
            .expect("update row ID must be numeric");
        changed.insert(id, Some((*update).to_owned()));
    }
    for id in deleted {
        changed.insert(*id, None);
    }
    let mut result = Vec::new();
    for id in 1..=9 {
        match changed.remove(&id) {
            Some(Some(row)) => result.push(row),
            Some(None) => {}
            None => result.push(format!("{id} {} {}", id + 100, id + 1000)),
        }
    }
    result.extend(changed.into_values().flatten());
    result
}

/// Run a contiguous subset of Go `TestLocalTemporaryTableUpdate` cases while
/// retaining the complete four-transaction-boundary matrix for every case.
fn run_local_temporary_table_update_cases(case_start: usize, case_end: usize) {
    struct Case {
        sql: &'static str,
        updates: &'static [&'static str],
        deleted: &'static [i32],
        duplicate: bool,
        old_unique_values: &'static [&'static str],
    }
    let cases = [
        Case {
            sql: "update tmp1 set v=999 where id=1",
            updates: &["1 101 999"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set id=12 where id=1",
            updates: &["12 101 1001"],
            deleted: &[1],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set id=1 where id=1",
            updates: &[],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=101 where id=1",
            updates: &[],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set v=999 where id=100",
            updates: &[],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=102 where id=100",
            updates: &[],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=21 where id=1",
            updates: &["1 21 1001"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &["101"],
        },
        Case {
            sql: "update tmp1 set id=2 where id=1",
            updates: &[],
            deleted: &[],
            duplicate: true,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=102 where id=1",
            updates: &[],
            deleted: &[],
            duplicate: true,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set v=v+1000 where id in (1, 3, 5)",
            updates: &["1 101 2001", "3 103 2003", "5 105 2005"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=u+1 where id in (9, 100)",
            updates: &["9 110 1009"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=101 where id in (100, 101)",
            updates: &[],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set id=id+1 where id in (8, 9)",
            updates: &[],
            deleted: &[],
            duplicate: true,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=u+1 where id in (8, 9)",
            updates: &[],
            deleted: &[],
            duplicate: true,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set id=id+20 where id in (1, 3, 5)",
            updates: &["21 101 1001", "23 103 1003", "25 105 1005"],
            deleted: &[1, 3, 5],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=u+100 where id in (1, 3, 5)",
            updates: &["1 201 1001", "3 203 1003", "5 205 1005"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &["101", "103", "105"],
        },
        Case {
            sql: "update tmp1 set v=888 where u=101",
            updates: &["1 101 888"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set id=21 where u=101",
            updates: &["21 101 1001"],
            deleted: &[1],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set v=888 where u=201",
            updates: &[],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=201 where u=101",
            updates: &["1 201 1001"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set id=2 where u=101",
            updates: &[],
            deleted: &[],
            duplicate: true,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=102 where u=101",
            updates: &[],
            deleted: &[],
            duplicate: true,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set v=v+1000 where u in (101, 103)",
            updates: &["1 101 2001", "3 103 2003"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set v=v+1000 where u in (201, 203)",
            updates: &[],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set v=v+1000 where u in (101, 110)",
            updates: &["1 101 2001"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set id=id+1 where u in (108, 109)",
            updates: &[],
            deleted: &[],
            duplicate: true,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set v=v+1000 where id<3",
            updates: &["1 101 2001", "2 102 2002"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update /*+ use_index(tmp1, u) */ tmp1 set v=v+1000 where u>107",
            updates: &["8 108 2008", "9 109 2009"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set v=v+1000 where v>=1007 or v<=1002",
            updates: &[
                "1 101 2001",
                "2 102 2002",
                "7 107 2007",
                "8 108 2008",
                "9 109 2009",
            ],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set v=v+1000 where id>=10",
            updates: &[],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set id=id+1 where id>7",
            updates: &[],
            deleted: &[],
            duplicate: true,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set id=id+1 where id>8",
            updates: &["10 109 1009"],
            deleted: &[9],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=u+1 where u>107",
            updates: &[],
            deleted: &[],
            duplicate: true,
            old_unique_values: &[],
        },
        Case {
            sql: "update tmp1 set u=u+1 where u>108",
            updates: &["9 110 1009"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
        Case {
            sql: "update /*+ use_index(tmp1, u) */ tmp1 set v=v+1000 where u>108 or u<102",
            updates: &["1 101 2001", "9 109 2009"],
            deleted: &[],
            duplicate: false,
            old_unique_values: &[],
        },
    ];
    assert_eq!(
        cases.len(),
        35,
        "update case groups must cover every Go case"
    );
    assert!(case_start < case_end);
    assert!(case_end <= cases.len());
    let ids = [1, 2, 3, 4, 5, 6, 7, 8, 9];
    let store = CreateMockStoreAndDomain().0;
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create temporary table tmp1 (id int primary key, u int unique, v int)",
        Vec::new(),
    );

    for case in cases
        .into_iter()
        .skip(case_start)
        .take(case_end - case_start)
    {
        let expected = update_expected_rows(case.updates, case.deleted);
        let execute = |tk: &mut astersql_testkit::TestKit| {
            if case.duplicate {
                let error = match tk.Exec(case.sql, Vec::new()) {
                    Err(error) => error,
                    Ok(result) => panic!(
                        "{:?} should reject a duplicate key, got {result:?}",
                        case.sql
                    ),
                };
                assert!(
                    error.to_string().contains("[kv:1062]Duplicate entry"),
                    "{}",
                    error
                );
                assert_temporary_rows(tk, &local_temporary_rows(&ids));
            } else {
                tk.MustExec(case.sql, Vec::new());
                tk.MustQuery("show warnings", Vec::new()).Check(Rows(&[]));
                assert_temporary_rows(tk, &expected);
                if !case.old_unique_values.is_empty() {
                    tk.MustQuery(
                        &format!(
                            "select /*+ use_index(tmp1, u) */ * from tmp1 where u in ({})",
                            case.old_unique_values.join(", ")
                        ),
                        Vec::new(),
                    )
                    .Check(Rows(&[]));
                    tk.MustQuery("show warnings", Vec::new()).Check(Rows(&[]));
                }
            }
        };

        tk.MustExec("begin", Vec::new());
        insert_temporary_rows(&mut tk, &ids);
        execute(&mut tk);
        tk.MustExec("rollback", Vec::new());
        assert_temporary_rows(&mut tk, &[]);

        insert_temporary_rows(&mut tk, &ids);
        execute(&mut tk);
        tk.MustExec("delete from tmp1", Vec::new());

        insert_temporary_rows(&mut tk, &ids);
        tk.MustExec("begin", Vec::new());
        execute(&mut tk);
        tk.MustExec("rollback", Vec::new());
        assert_temporary_rows(&mut tk, &local_temporary_rows(&ids));

        tk.MustExec("begin", Vec::new());
        execute(&mut tk);
        tk.MustExec("commit", Vec::new());
        if case.duplicate {
            assert_temporary_rows(&mut tk, &local_temporary_rows(&ids));
        } else {
            assert_temporary_rows(&mut tk, &expected);
        }
        tk.MustExec("delete from tmp1", Vec::new());
        assert_temporary_rows(&mut tk, &[]);
    }
}

/// Go `TestLocalTemporaryTableUpdate`：主键/唯一键、点查/范围扫描，以及四种事务边界。
#[test]
fn local_temporary_table_update_matches_go_transaction_matrix_cases_01_05() {
    run_local_temporary_table_update_cases(0, 5);
}

#[test]
fn local_temporary_table_update_matches_go_transaction_matrix_cases_06_10() {
    run_local_temporary_table_update_cases(5, 10);
}

#[test]
fn local_temporary_table_update_matches_go_transaction_matrix_cases_11_15() {
    run_local_temporary_table_update_cases(10, 15);
}

#[test]
fn local_temporary_table_update_matches_go_transaction_matrix_cases_16_20() {
    run_local_temporary_table_update_cases(15, 20);
}

#[test]
fn local_temporary_table_update_matches_go_transaction_matrix_cases_21_25() {
    run_local_temporary_table_update_cases(20, 25);
}

#[test]
fn local_temporary_table_update_matches_go_transaction_matrix_cases_26_30() {
    run_local_temporary_table_update_cases(25, 30);
}

#[test]
fn local_temporary_table_update_matches_go_transaction_matrix_cases_31_35() {
    run_local_temporary_table_update_cases(30, 35);
}

/// Go `TestLocalTemporaryTableDelete`：同一事务矩阵覆盖主键、唯一键与范围删除。
#[test]
fn local_temporary_table_delete_matches_go_transaction_matrix() {
    let cases: [(&str, &[i32]); 14] = [
        ("delete from tmp1 where id=1", &[1]),
        ("delete from tmp1 where id in (1, 3, 5)", &[1, 3, 5]),
        ("delete from tmp1 where u=102", &[2]),
        ("delete from tmp1 where u in (103, 107, 108)", &[3, 7, 8]),
        ("delete from tmp1 where id=10", &[]),
        ("delete from tmp1 where id in (10, 12)", &[]),
        ("delete from tmp1 where u=110", &[]),
        ("delete from tmp1 where u in (111, 112)", &[]),
        ("delete from tmp1 where id in (1, 11, 5)", &[1, 5]),
        ("delete from tmp1 where u in (102, 121, 106)", &[2, 6]),
        ("delete from tmp1 where id<3", &[1, 2]),
        ("delete from tmp1 where u>107", &[8, 9]),
        (
            "delete /*+ use_index(tmp1, u) */ from tmp1 where u>105 and u<107",
            &[6],
        ),
        (
            "delete from tmp1 where v>=1006 or v<=1002",
            &[1, 2, 6, 7, 8, 9],
        ),
    ];
    let ids = [1, 2, 3, 4, 5, 6, 7, 8, 9];
    let store = CreateMockStoreAndDomain().0;
    let mut tk = NewTestKit(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create temporary table tmp1 (id int primary key, u int unique, v int)",
        Vec::new(),
    );

    for (sql, deleted) in cases {
        let kept = ids
            .iter()
            .filter(|id| !deleted.contains(id))
            .copied()
            .collect::<Vec<_>>();
        tk.MustExec("begin", Vec::new());
        insert_temporary_rows(&mut tk, &ids);
        tk.MustExec(sql, Vec::new());
        tk.MustQuery("show warnings", Vec::new()).Check(Rows(&[]));
        assert_temporary_rows_and_unique_index(&mut tk, &kept);
        tk.MustExec("rollback", Vec::new());
        assert_temporary_rows_and_unique_index(&mut tk, &[]);

        insert_temporary_rows(&mut tk, &ids);
        tk.MustExec(sql, Vec::new());
        assert_temporary_rows_and_unique_index(&mut tk, &kept);
        insert_temporary_rows(&mut tk, deleted);
        tk.MustExec("begin", Vec::new());
        tk.MustExec(sql, Vec::new());
        assert_temporary_rows_and_unique_index(&mut tk, &kept);
        tk.MustExec("rollback", Vec::new());
        assert_temporary_rows_and_unique_index(&mut tk, &ids);

        tk.MustExec("begin", Vec::new());
        tk.MustExec(sql, Vec::new());
        tk.MustExec("commit", Vec::new());
        assert_temporary_rows_and_unique_index(&mut tk, &kept);
        tk.MustExec("delete from tmp1", Vec::new());
        assert_temporary_rows_and_unique_index(&mut tk, &[]);
    }
}

/// Go `TestSchemaCheckerTempTable` 的核心回归：全局临时表的 DDL 与事务交错时，
/// 当前事务仍能写入/加锁读取，且 `ON COMMIT DELETE ROWS` 会在提交后清空其数据。
#[test]
fn global_temporary_table_schema_change_keeps_transaction_usable() {
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk1 = NewTestKit(store.clone());
    let mut tk2 = NewTestKit(store);
    for tk in [&mut tk1, &mut tk2] {
        tk.MustExec("use test", Vec::new());
    }
    tk1.MustExec("create table normal_table (id int, c int)", Vec::new());
    tk1.MustExec(
        "create global temporary table temp_table (id int primary key, c int) on commit delete rows",
        Vec::new(),
    );

    tk1.MustExec("begin", Vec::new());
    tk2.MustExec("alter table temp_table modify column c tinyint", Vec::new());
    tk1.MustExec("insert into temp_table values(3, 3)", Vec::new());
    tk1.MustExec("commit", Vec::new());
    tk1.MustQuery("select * from temp_table", Vec::new())
        .Check(Rows(&[]));

    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("alter table temp_table modify column c int", Vec::new());
    tk1.MustQuery("select * from temp_table for update", Vec::new())
        .Check(Rows(&[]));
    tk1.MustExec("commit", Vec::new());

    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec(
        "alter table temp_table modify column c smallint",
        Vec::new(),
    );
    tk1.MustExec("insert into temp_table values(3, 4)", Vec::new());
    tk1.MustQuery("select * from temp_table for update", Vec::new())
        .Check(Rows(&["3 4"]));
    tk1.MustExec("commit", Vec::new());
    tk1.MustQuery("select * from temp_table", Vec::new())
        .Check(Rows(&[]));

    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("alter table temp_table modify column c bigint", Vec::new());
    tk1.MustQuery("select * from temp_table where id=1 for update", Vec::new())
        .Check(Rows(&[]));
    tk1.MustExec("commit", Vec::new());

    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec(
        "alter table temp_table modify column c smallint",
        Vec::new(),
    );
    tk1.MustExec(
        "insert into temp_table values (1, 2), (2, 3), (4, 5)",
        Vec::new(),
    );
    tk1.MustQuery("select * from temp_table where id=1 for update", Vec::new())
        .Check(Rows(&["1 2"]));
    tk1.MustExec("commit", Vec::new());

    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("alter table temp_table modify column c int", Vec::new());
    tk1.MustQuery("select * from temp_table where id=1 for update", Vec::new())
        .Check(Rows(&[]));
    tk1.MustExec("commit", Vec::new());

    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("alter table temp_table modify column c bigint", Vec::new());
    tk1.MustQuery(
        "select * from temp_table where id in (1, 2, 3) for update",
        Vec::new(),
    )
    .Check(Rows(&[]));
    tk1.MustExec("commit", Vec::new());

    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("alter table temp_table modify column c int", Vec::new());
    tk1.MustExec(
        "insert into temp_table values (1, 2), (2, 3), (4, 5)",
        Vec::new(),
    );
    tk1.MustQuery(
        "select * from temp_table where id in (1, 2, 3) for update",
        Vec::new(),
    )
    .Check(Rows(&["1 2", "2 3"]));
    tk1.MustExec("commit", Vec::new());

    tk1.MustExec("insert into normal_table values(1, 2)", Vec::new());
    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("alter table temp_table modify column c int", Vec::new());
    tk1.MustExec("insert into temp_table values(1, 5)", Vec::new());
    tk1.MustQuery("select * from temp_table for update", Vec::new())
        .Check(Rows(&["1 5"]));
    tk1.MustQuery(
        "select * from temp_table, normal_table where temp_table.id = normal_table.id for update",
        Vec::new(),
    )
    .Check(Rows(&["1 5 1 2"]));
    tk1.MustExec("commit", Vec::new());

    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec(
        "alter table normal_table modify column c bigint",
        Vec::new(),
    );
    tk1.MustQuery(
        "select * from temp_table, normal_table where temp_table.id = normal_table.id for update",
        Vec::new(),
    )
    .Check(Rows(&[]));
    tk1.MustExec("commit", Vec::new());

    tk1.MustExec("begin", Vec::new());
    tk2.MustExec("truncate table temp_table", Vec::new());
    tk1.MustExec("insert into temp_table values(3, 3)", Vec::new());
    tk1.MustExec("commit", Vec::new());

    tk1.MustExec("begin", Vec::new());
    tk2.MustExec(
        "alter table normal_table modify column c bigint",
        Vec::new(),
    );
    tk1.MustExec("insert into temp_table values(3, 3)", Vec::new());
    tk1.MustExec("insert into normal_table values(3, 3)", Vec::new());
    let error = tk1
        .Exec("commit", Vec::new())
        .expect_err("normal-table schema change must reject optimistic commit");
    assert!(
        error
            .to_string()
            .starts_with("[domain:8028]Information schema is changed"),
        "{error}"
    );

    tk1.MustExec("begin pessimistic", Vec::new());
    tk2.MustExec("alter table normal_table modify column c int", Vec::new());
    tk1.MustExec("insert into temp_table values(1, 6)", Vec::new());
    tk1.MustQuery(
        "select * from temp_table, normal_table where temp_table.id = normal_table.id for update",
        Vec::new(),
    )
    .Check(Rows(&["1 6 1 2"]));
    let error = tk1
        .Exec("commit", Vec::new())
        .expect_err("normal-table schema change must reject pessimistic commit");
    assert!(
        error
            .to_string()
            .starts_with("[domain:8028]Information schema is changed"),
        "{error}"
    );
    // A rejected commit must discard temporary rows and leave the session usable.
    tk1.MustQuery("select * from temp_table", Vec::new())
        .Check(Rows(&[]));
    tk1.MustExec("begin pessimistic", Vec::new());
    tk1.MustQuery("select * from normal_table for update", Vec::new())
        .Check(Rows(&["1 2"]));
    tk1.MustExec("commit", Vec::new());
}

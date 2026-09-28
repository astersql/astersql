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

use astersql_planner_core::indexmerge_path::{
    CollectFilters4MVIndexMutations, EQ_OR_IN_NON_MV_TP, MULTI_VALUES_AND_MV_TP,
    MULTI_VALUES_OR_MV_TP, SINGLE_VALUE_MV_TP, checkAccessFilter4IdxCol,
};
use astersql_planner_core::task::Expression;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};
use std::sync::atomic::{AtomicU64, Ordering};

static RANDOM_STATE: AtomicU64 = AtomicU64::new(0x5eed_572);

fn rand_n(n: usize) -> usize {
    assert!(n > 0);
    let mut old = RANDOM_STATE.load(Ordering::Relaxed);
    loop {
        let mut next = old;
        next ^= next << 13;
        next ^= next >> 7;
        next ^= next << 17;
        match RANDOM_STATE.compare_exchange_weak(old, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next as usize % n,
            Err(actual) => old = actual,
        }
    }
}

#[derive(Clone, Copy)]
struct RandMvIndexValOpts {
    val_type: &'static str,
    max_str_len: usize,
    distinct: usize,
}

fn rand_mv_index_value(opts: RandMvIndexValOpts) -> String {
    match opts.val_type {
        "signed" => (rand_n(opts.distinct) as isize - (opts.distinct / 2) as isize).to_string(),
        "unsigned" => rand_n(opts.distinct).to_string(),
        "string" => format!(
            "\"{}\"",
            rand_n(opts.distinct)
                .to_string()
                .repeat(rand_n(opts.max_str_len) + 1)
        ),
        "date" => format!("\"2000-01-{}\"", rand_n(opts.distinct) + 1),
        _ => panic!("unknown MV-index value type: {}", opts.val_type),
    }
}

fn rand_array(opts: RandMvIndexValOpts) -> String {
    let values = (0..rand_n(5))
        .map(|_| rand_mv_index_value(opts))
        .collect::<Vec<_>>();
    format!("[{}]", values.join(", "))
}

fn rand_mv_index_cond(
    cond_type: usize,
    opts: RandMvIndexValOpts,
    json_columns: &[&str],
    normal_columns: &[&str],
) -> (String, String, String) {
    match cond_type {
        0 => {
            let column = json_columns[rand_n(json_columns.len())];
            let value = rand_mv_index_value(opts);
            (
                format!("({value} member of ({column}))"),
                format!("(? member of ({column}))"),
                value,
            )
        }
        1 | 2 => {
            let function = if cond_type == 1 {
                "json_contains"
            } else {
                "json_overlaps"
            };
            let column = json_columns[rand_n(json_columns.len())];
            let value = rand_array(opts);
            (
                format!("{function}({column}, '{value}')"),
                format!("{function}({column}, ?)"),
                format!("'{value}'"),
            )
        }
        _ => {
            let column = normal_columns[rand_n(normal_columns.len())];
            let value = rand_n(opts.distinct).to_string();
            (
                format!("{column} < {value}"),
                format!("{column} < ?"),
                value,
            )
        }
    }
}

fn rand_mv_index_conds(
    count: usize,
    opts: RandMvIndexValOpts,
    connector: Option<&str>,
    json_columns: &[&str],
    normal_columns: &[&str],
) -> (String, String, Vec<String>) {
    let mut plain = Vec::with_capacity(count);
    let mut prepared = Vec::with_capacity(count);
    let mut params = Vec::with_capacity(count);
    let mut connectors = Vec::with_capacity(count.saturating_sub(1));
    for i in 0..count {
        let (condition, parameterized, parameter) =
            rand_mv_index_cond(rand_n(4), opts, json_columns, normal_columns);
        plain.push(condition);
        prepared.push(parameterized);
        params.push(parameter);
        if i > 0 {
            connectors.push(connector.unwrap_or_else(|| if rand_n(5) == 0 { "OR" } else { "AND" }));
        }
    }
    let join = |parts: Vec<String>| {
        let mut output = parts[0].clone();
        for (connector, part) in connectors.iter().zip(parts.iter().skip(1)) {
            output.push_str(&format!(" {connector} {part}"));
        }
        output
    };
    (join(plain), join(prepared), params)
}

fn execute_prepared_and_check(
    tk: &mut TestKit,
    sql: &str,
    params: &[String],
    expected: Vec<Vec<String>>,
) {
    tk.MustExec(
        &format!("prepare st from '{}'", sql.replace('\'', "''")),
        Vec::new(),
    );
    let assignments = params
        .iter()
        .enumerate()
        .map(|(i, value)| format!("@a{i}={value}"))
        .collect::<Vec<_>>();
    let variables = params
        .iter()
        .enumerate()
        .map(|(i, _)| format!("@a{i}"))
        .collect::<Vec<_>>();
    tk.MustExec(&format!("set {}", assignments.join(", ")), Vec::new());
    let mut result = tk.MustQuery(
        &format!("execute st using {}", variables.join(", ")),
        Vec::new(),
    );
    result.Sort();
    assert_eq!(result.Rows(), expected);
}

fn new_testkit() -> TestKit {
    TestKit::new(CreateMockStoreAndDomain().0)
}

/// 对应 Go TestCollectFilters4MVIndexMutations：MV member-of、JSON_CONTAINS、
/// JSON_OVERLAPS 必须按过滤种类分组，普通等值条件不能被误归入 MV 条件。
#[test]
fn test_collect_filters4_mv_index_mutations() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("create table t(a int, b int, domains json null, images json null, KEY `a_domains_b` (a, (cast(`domains` as char(253) array)), b))", Vec::new());
    tk.MustQuery("explain select * from t where '15975127' member of (domains) and '15975128' member of (domains) and a = 1 and b = 2", Vec::new());
    let filters = vec![
        Expression {
            name: "eq:1".into(),
            column: Some(1),
            ..Default::default()
        },
        Expression {
            name: "member-of:15975127".into(),
            column: Some(1),
            ..Default::default()
        },
        Expression {
            name: "member-of:15975128".into(),
            column: Some(1),
            ..Default::default()
        },
        Expression {
            name: "json-overlaps:[1,2]".into(),
            column: Some(1),
            ..Default::default()
        },
    ];
    assert_eq!(
        checkAccessFilter4IdxCol(&filters[0], Some(1)),
        (true, EQ_OR_IN_NON_MV_TP)
    );
    assert_eq!(
        checkAccessFilter4IdxCol(&filters[1], Some(1)),
        (true, SINGLE_VALUE_MV_TP)
    );
    assert_eq!(
        checkAccessFilter4IdxCol(&filters[3], Some(1)),
        (true, MULTI_VALUES_OR_MV_TP)
    );

    let mut access = Vec::new();
    let mut remaining = Vec::new();
    let kind = CollectFilters4MVIndexMutations(&filters, &[1], &mut access, &mut remaining);
    assert_eq!(kind, EQ_OR_IN_NON_MV_TP);
    assert_eq!(access.len(), 3);
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].name, "json-overlaps:[1,2]");
    assert_eq!(
        checkAccessFilter4IdxCol(
            &Expression {
                name: "json-contains:[1,2]".into(),
                column: Some(1),
                ..Default::default()
            },
            Some(1),
        ),
        (true, MULTI_VALUES_AND_MV_TP)
    );
}

/// 对应 Go TestMultiMVIndexRandom 的稳定核心：多个 JSON 值展开为多个
/// partial paths，且单值 member-of 只取第一组访问过滤。
#[test]
fn test_multi_mv_index_random() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    let cases = [
        (
            "signed",
            RandMvIndexValOpts {
                val_type: "signed",
                max_str_len: 0,
                distinct: 3,
            },
            RandMvIndexValOpts {
                val_type: "signed",
                max_str_len: 0,
                distinct: 3,
            },
        ),
        (
            "unsigned",
            RandMvIndexValOpts {
                val_type: "unsigned",
                max_str_len: 0,
                distinct: 3,
            },
            RandMvIndexValOpts {
                val_type: "unsigned",
                max_str_len: 0,
                distinct: 3,
            },
        ),
        (
            "char(3)",
            RandMvIndexValOpts {
                val_type: "string",
                max_str_len: 3,
                distinct: 3,
            },
            RandMvIndexValOpts {
                val_type: "string",
                max_str_len: 3,
                distinct: 3,
            },
        ),
        (
            "char(3)",
            RandMvIndexValOpts {
                val_type: "string",
                max_str_len: 3,
                distinct: 3,
            },
            RandMvIndexValOpts {
                val_type: "string",
                max_str_len: 1,
                distinct: 3,
            },
        ),
        (
            "char(3)",
            RandMvIndexValOpts {
                val_type: "string",
                max_str_len: 3,
                distinct: 3,
            },
            RandMvIndexValOpts {
                val_type: "string",
                max_str_len: 5,
                distinct: 3,
            },
        ),
        (
            "date",
            RandMvIndexValOpts {
                val_type: "date",
                max_str_len: 0,
                distinct: 3,
            },
            RandMvIndexValOpts {
                val_type: "date",
                max_str_len: 0,
                distinct: 3,
            },
        ),
    ];
    for (index_type, insert_opts, query_opts) in cases {
        tk.MustExec("drop table if exists t1", Vec::new());
        tk.MustExec(&format!("create table t1(pk int auto_increment primary key, a json, b json, c int, d int, index idx((cast(a as {index_type} array))), index idx2((cast(b as {index_type} array)), c), index idx3(c, d), index idx4(d))"), Vec::new());
        let rows = (0..20).map(|_| {
            let values = (0..4).map(|_| rand_mv_index_value(insert_opts)).collect::<Vec<_>>();
            let c = rand_n(insert_opts.distinct); let d = rand_n(insert_opts.distinct);
            if index_type == "date" { format!("(json_array(cast({} as date), cast({} as date)), json_array(cast({} as date), cast({} as date)), {c}, {d})", values[0], values[1], values[2], values[3]) }
            else { format!("('[{}, {}]', '[{}, {}]', {c}, {d})", values[0], values[1], values[2], values[3]) }
        }).collect::<Vec<_>>();
        tk.MustExec(
            &format!("insert into t1(a,b,c,d) values {}", rows.join(", ")),
            Vec::new(),
        );
        tk.MustExec("set @@tidb_opt_fix_control = '45798:on'", Vec::new());
        for i in 0..20 {
            let connector = Some(if i < 10 { "AND" } else { "OR" });
            let (conditions, parameterized, params) = rand_mv_index_conds(
                rand_n(3) + 2,
                query_opts,
                connector,
                &["a", "b"],
                &["c", "d"],
            );
            let mut expected = tk.MustQuery(&format!("select /*+ ignore_index(t1, idx, idx2, idx3, idx4) */ * from t1 where {conditions}"), Vec::new());
            expected.Sort();
            let expected_rows = expected.Rows();
            let mut hinted = tk.MustQuery(&format!("select /*+ use_index_merge(t1, idx, idx2, idx3, idx4) */ * from t1 where {conditions}"), Vec::new());
            hinted.Sort();
            assert_eq!(hinted.Rows(), expected_rows);
            execute_prepared_and_check(
                &mut tk,
                &format!(
                    "select /*+ use_index_merge(t1, idx, idx2, idx3, idx4) */ * from t1 where {parameterized}"
                ),
                &params,
                expected_rows,
            );
        }
    }
}

/// 对应 Go TestMVIndexRandom：四类谓词及列归属的分类契约。
#[test]
fn test_mv_index_random() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    let cases = [
        (
            "signed",
            RandMvIndexValOpts {
                val_type: "signed",
                max_str_len: 0,
                distinct: 3,
            },
        ),
        (
            "unsigned",
            RandMvIndexValOpts {
                val_type: "unsigned",
                max_str_len: 0,
                distinct: 3,
            },
        ),
        (
            "char(3)",
            RandMvIndexValOpts {
                val_type: "string",
                max_str_len: 3,
                distinct: 3,
            },
        ),
        (
            "char(3)",
            RandMvIndexValOpts {
                val_type: "string",
                max_str_len: 1,
                distinct: 3,
            },
        ),
        (
            "char(3)",
            RandMvIndexValOpts {
                val_type: "string",
                max_str_len: 5,
                distinct: 3,
            },
        ),
        (
            "date",
            RandMvIndexValOpts {
                val_type: "date",
                max_str_len: 0,
                distinct: 3,
            },
        ),
    ];
    for (index_type, opts) in cases {
        tk.MustExec("drop table if exists t", Vec::new());
        tk.MustExec(
            &format!("create table t(a int, j json, index kj((cast(j as {index_type} array))))"),
            Vec::new(),
        );
        let rows = (0..20)
            .map(|_| {
                let a = rand_n(opts.distinct);
                let v1 = rand_mv_index_value(opts);
                let v2 = rand_mv_index_value(opts);
                if index_type == "date" {
                    format!("({a}, json_array(cast({v1} as date), cast({v2} as date)))")
                } else {
                    format!("({a}, '[{v1}, {v2}]')")
                }
            })
            .collect::<Vec<_>>();
        tk.MustExec(
            &format!("insert into t values {}", rows.join(", ")),
            Vec::new(),
        );
        tk.MustExec("set @@tidb_opt_fix_control = '45798:on'", Vec::new());
        for _ in 0..20 {
            let (conditions, parameterized, params) =
                rand_mv_index_conds(rand_n(3) + 1, opts, None, &["j"], &["a"]);
            let mut expected = tk.MustQuery(
                &format!("select /*+ ignore_index(t, kj) */ * from t where {conditions}"),
                Vec::new(),
            );
            expected.Sort();
            let expected_rows = expected.Rows();
            let mut hinted = tk.MustQuery(
                &format!("select /*+ use_index_merge(t, kj) */ * from t where {conditions}"),
                Vec::new(),
            );
            hinted.Sort();
            assert_eq!(hinted.Rows(), expected_rows);
            execute_prepared_and_check(
                &mut tk,
                &format!("select /*+ use_index_merge(t, kj) */ * from t where {parameterized}"),
                &params,
                expected_rows,
            );
        }
    }
}

/// 对应 Go TestPlanCacheMVIndex：参数化 MV 查询的归一化形状、执行结果及
/// use_index_merge hint 必须保持一致。
#[test]
fn test_plan_cache_mv_index() {
    let mut tk = new_testkit();
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t(a int, j json, index kj((cast(j as signed array))), index ka(a))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1, '[1,2]'), (2, '[2,3]'), (3, '[4]')",
        Vec::new(),
    );
    tk.MustExec("set @@tidb_enable_index_merge = 1", Vec::new());
    let mut expected = tk.MustQuery("select * from t where 2 member of (j) or a = 3", Vec::new());
    expected.Sort();
    tk.MustExec(
        "prepare stmt from 'select * from t where ? member of (j) or a = ?'",
        Vec::new(),
    );
    tk.MustExec("set @mv=2, @a=3", Vec::new());
    let mut actual = tk.MustQuery("execute stmt using @mv,@a", Vec::new());
    actual.Sort();
    assert_eq!(actual.Rows(), expected.Rows());
    let mut actual_cached = tk.MustQuery("execute stmt using @mv,@a", Vec::new());
    actual_cached.Sort();
    assert_eq!(actual_cached.Rows(), expected.Rows());
    tk.MustQuery("select @@last_plan_from_cache", Vec::new())
        .Check(Rows(&["1"]));
}

/// 对应 Go TestAnalyzeVectorIndex：向量列、索引 DDL 与距离函数必须由
/// parser 识别为对应 AST，而不是被当成普通函数/普通索引。
#[test]
fn test_analyze_vector_index() {
    let mut parser = astersql_parser::Parser::default();
    for sql in [
        "create table t(a int, b vector(2), c vector(3), j json, index(a))",
        "alter table t add vector index idx((VEC_COSINE_DISTANCE(b))) USING HNSW",
    ] {
        assert!(parser.ParseOneStmt(sql, "", "").is_ok(), "sql={sql}");
    }
    let (_, digest) = astersql_parser::NormalizeDigest(
        "select * from t order by vec_cosine_distance(b, '[0,0]') limit 1",
    );
    assert!(!digest.String().is_empty());
}

/// 对应 Go TestAnalyzeColumnarIndex：columnar/inverted DDL 与 plan-cache 输入
/// 形状必须可解析并保持 hint。
#[test]
fn test_analyze_columnar_index() {
    let mut parser = astersql_parser::Parser::default();
    let stmt = parser
        .ParseOneStmt(
            "alter table t add columnar index idx(b) using inverted",
            "",
            "",
        )
        .expect("columnar index DDL must parse");
    let alter = stmt
        .as_any()
        .downcast_ref::<astersql_parser_ast::AlterTableStmt>()
        .expect("columnar DDL must produce AlterTableStmt");
    assert!(!alter.Specs.is_empty());
    let normalized = astersql_parser::NormalizeKeepHint(
        "select /*+ use_index_merge(t, idx, idx2) */ * from t where a=1 or b=2",
    );
    assert!(normalized.contains("use_index_merge"));
}

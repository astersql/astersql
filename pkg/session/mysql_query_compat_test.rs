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

// MySQL 单表查询兼容性回归测试。
//
// 测试通过规范会话执行常见筛选、表达式、排序和分页查询，并同时核对列名、
// MySQL 类型码与字符串化结果行，避免只验证数据值而遗漏协议元数据差异。

use crate::runtime::{ConcreteRecordSet, ConcreteSession, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

/// 一条查询及其完整预期结果，包括客户端可见的元数据。
struct QueryCase {
    /// 待执行的查询语句。
    sql: &'static str,
    /// 按返回顺序排列的列名。
    columns: &'static [&'static str],
    /// 与各列对应的 MySQL 协议类型码。
    types: &'static [u8],
    /// 结果集的字符串表示；SQL NULL 由测试结果集统一表示为 `<nil>`。
    rows: Vec<Vec<&'static str>>,
}

/// 执行建库、建表和写入等准备语句，并保留失败语句以便定位夹具问题。
fn execute(session: &ConcreteSession, sql: &str) {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("query compatibility setup failed: {sql}: {error}"));
}

/// 执行查询并依次核对列元数据、类型码和全部结果行。
fn assert_query(session: &ConcreteSession, case: QueryCase) {
    let mut result = session
        .execute(case.sql)
        .unwrap_or_else(|error| {
            panic!(
                "query compatibility statement failed: {}: {error}",
                case.sql
            )
        })
        .remove(0);
    assert_eq!(
        result.Columns(),
        case.columns,
        "column names for {}",
        case.sql
    );
    let actual_types = result
        .result_fields()
        .iter()
        .map(|field| {
            field
                .as_ref()
                .unwrap_or_else(|| panic!("missing result metadata for {}", case.sql))
                .column
                .GetType()
        })
        .collect::<Vec<_>>();
    assert_eq!(actual_types, case.types, "column types for {}", case.sql);

    // 必须通过结果集接口持续拉取到 EOF，才能同时覆盖行转换与游标推进逻辑。
    let mut actual_rows = Vec::new();
    while let Some(row) = result
        .Next()
        .unwrap_or_else(|error| panic!("read query compatibility row: {}: {error}", case.sql))
    {
        actual_rows.push(row);
    }
    let expected_rows = case
        .rows
        .into_iter()
        .map(|row| row.into_iter().map(str::to_owned).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    assert_eq!(actual_rows, expected_rows, "result rows for {}", case.sql);
}

#[test]
fn common_mysql_single_table_queries_and_expressions_match_expected_rows() {
    use astersql_parser_mysql::r#type::{TypeLong, TypeLonglong, TypeVarString, TypeVarchar};

    let (_domain, session) = CreateAnalyzeSession().expect("canonical query session");
    execute(&session, "create database query_compat");
    execute(&session, "use query_compat");
    execute(
        &session,
        "create table query_cases (\
         id int not null,\
         category varchar(16) not null,\
         label varchar(32) null,\
         score int null,\
         delta int not null,\
         primary key (id))",
    );
    execute(
        &session,
        "insert into query_cases (id, category, label, score, delta) values \
         (1, 'alpha', 'Apple', 10, -5),\
         (2, 'alpha', null, 20, 0),\
         (3, 'beta', 'Berry', null, 5),\
         (4, 'beta', 'apricot', 20, -2),\
         (5, 'gamma', 'Zed', -3, 8),\
         (6, 'gamma', null, null, 8),\
         (7, 'alpha', 'Apple', 10, -5)",
    );

    // 用同一份含重复值、负数和 NULL 的夹具覆盖投影、三值逻辑、标量函数、
    // 排序去重及越界分页，并显式固定每类表达式推导出的 MySQL 类型码。
    let cases = vec![
        QueryCase {
            sql: "select q.* from query_cases as q where q.id = 1",
            columns: &["id", "category", "label", "score", "delta"],
            types: &[TypeLong, TypeVarchar, TypeVarchar, TypeLong, TypeLong],
            rows: vec![vec!["1", "alpha", "Apple", "10", "-5"]],
        },
        QueryCase {
            sql: "select q.id as item_id, q.category, 7 as marker \
                  from query_cases as q where q.id = 1",
            columns: &["item_id", "category", "marker"],
            types: &[TypeLong, TypeVarchar, TypeLonglong],
            rows: vec![vec!["1", "alpha", "7"]],
        },
        QueryCase {
            sql: "select id, label from query_cases \
                  where (score between 10 and 20 and category in ('alpha', 'beta')) \
                  or label like 'Z%' order by id",
            columns: &["id", "label"],
            types: &[TypeLong, TypeVarchar],
            rows: vec![
                vec!["1", "Apple"],
                vec!["2", "<nil>"],
                vec!["4", "apricot"],
                vec!["5", "Zed"],
                vec!["7", "Apple"],
            ],
        },
        QueryCase {
            sql: "select id, score is null as missing from query_cases \
                  where score is null order by id",
            columns: &["id", "missing"],
            types: &[TypeLong, TypeLonglong],
            rows: vec![vec!["3", "1"], vec!["6", "1"]],
        },
        QueryCase {
            sql: "select id, score + delta as total, \
                  score >= 10 and delta < 1 as flagged \
                  from query_cases where id in (1, 2, 5) order by id",
            columns: &["id", "total", "flagged"],
            types: &[TypeLong, TypeLonglong, TypeLonglong],
            rows: vec![
                vec!["1", "5", "1"],
                vec!["2", "20", "1"],
                vec!["5", "5", "0"],
            ],
        },
        QueryCase {
            sql: "select id, \
                  case when score is null then 'missing' \
                       when score < 0 then 'negative' \
                       else 'nonnegative' end as score_class, \
                  coalesce(label, 'fallback') as display_label, \
                  upper(category) as category_upper, \
                  concat(category, ':', id) as tag, \
                  abs(delta) as magnitude, \
                  length(coalesce(label, '')) as label_length \
                  from query_cases where id in (1, 3, 5, 6) order by id",
            columns: &[
                "id",
                "score_class",
                "display_label",
                "category_upper",
                "tag",
                "magnitude",
                "label_length",
            ],
            types: &[
                TypeLong,
                TypeVarString,
                TypeVarchar,
                TypeVarString,
                TypeVarString,
                TypeLonglong,
                TypeLonglong,
            ],
            rows: vec![
                vec!["1", "nonnegative", "Apple", "ALPHA", "alpha:1", "5", "5"],
                vec!["3", "missing", "Berry", "BETA", "beta:3", "5", "5"],
                vec!["5", "negative", "Zed", "GAMMA", "gamma:5", "8", "3"],
                vec!["6", "missing", "fallback", "GAMMA", "gamma:6", "8", "0"],
            ],
        },
        QueryCase {
            sql: "select id, category, score from query_cases \
                  order by category asc, score desc, id asc limit 4 offset 1",
            columns: &["id", "category", "score"],
            types: &[TypeLong, TypeVarchar, TypeLong],
            rows: vec![
                vec!["1", "alpha", "10"],
                vec!["7", "alpha", "10"],
                vec!["4", "beta", "20"],
                vec!["3", "beta", "<nil>"],
            ],
        },
        QueryCase {
            sql: "select distinct category, score from query_cases \
                  order by category asc, score desc",
            columns: &["category", "score"],
            types: &[TypeVarchar, TypeLong],
            rows: vec![
                vec!["alpha", "20"],
                vec!["alpha", "10"],
                vec!["beta", "20"],
                vec!["beta", "<nil>"],
                vec!["gamma", "-3"],
                vec!["gamma", "<nil>"],
            ],
        },
        QueryCase {
            sql: "select id, label from query_cases \
                  where id < 0 order by id limit 5",
            columns: &["id", "label"],
            types: &[TypeLong, TypeVarchar],
            rows: vec![],
        },
        QueryCase {
            sql: "select label from query_cases where label is null order by id",
            columns: &["label"],
            types: &[TypeVarchar],
            rows: vec![vec!["<nil>"], vec!["<nil>"]],
        },
        QueryCase {
            sql: "select id from query_cases order by id limit 3 offset 99",
            columns: &["id"],
            types: &[TypeLong],
            rows: vec![],
        },
    ];

    for case in cases {
        assert_query(&session, case);
    }
}

#[test]
fn wordpress_transient_cleanup_only_deletes_expired_pair() {
    use astersql_parser_mysql::r#type::TypeVarchar;

    let (_domain, session) = CreateAnalyzeSession().expect("canonical query session");
    execute(&session, "create database wordpress_cleanup_compat");
    execute(&session, "use wordpress_cleanup_compat");
    execute(
        &session,
        "create table options (id int primary key, option_name varchar(191), option_value varchar(255))",
    );
    execute(
        &session,
        "insert into options values \
         (1, 'siteurl', 'http://site'), \
         (2, 'home', 'http://site'), \
         (3, '_transient_test', 'value'), \
         (4, '_transient_timeout_test', '1')",
    );
    assert_query(
        &session,
        QueryCase {
            sql: "select b.option_name from options a, options b \
                  where a.option_name = '_transient_test' \
                  and b.option_name = concat('_transient_timeout_', substring(a.option_name, 12))",
            columns: &["option_name"],
            types: &[TypeVarchar],
            rows: vec![vec!["_transient_timeout_test"]],
        },
    );
    execute(
        &session,
        "delete a, b from options a, options b \
         where a.option_name like '_transient_%' \
         and a.option_name not like '_transient_timeout_%' \
         and b.option_name = concat('_transient_timeout_', substring(a.option_name, 12)) \
         and b.option_value < 100",
    );
    assert_query(
        &session,
        QueryCase {
            sql: "select option_name from options order by id",
            columns: &["option_name"],
            types: &[TypeVarchar],
            rows: vec![vec!["siteurl"], vec!["home"]],
        },
    );
}

#[test]
fn string_function_comparisons_use_string_values_like_go() {
    let (_domain, session) = CreateAnalyzeSession().expect("canonical query session");
    let string_cases = [
        "upper('abc')",
        "lower('ABC')",
        "concat('a', 'b')",
        "substring('abc', 2)",
        "hex('A')",
        "json_type('[]')",
        "unhex('4142')",
        "repeat('a', 2)",
        "space(2)",
        "lpad('a', 3, 'x')",
        "elt(1, 'abc', 'def')",
        "coalesce(null, 'abc')",
        "ifnull(null, 'abc')",
        "nullif('abc', 'def')",
        "if(1, 'abc', 'def')",
        "greatest('abc', 'def')",
        "least('abc', 'def')",
        "case when 1 then 'abc' else 'def' end",
    ];
    for expression in string_cases {
        let sql = format!("select 'other' = {expression}");
        let mut result = session
            .execute(&sql)
            .unwrap_or_else(|error| panic!("execute {sql}: {error}"))
            .remove(0);
        assert_eq!(
            result.Next().expect("string comparison row"),
            Some(vec!["0".to_owned()]),
            "{sql} must compare as strings",
        );
    }

    let mut numeric = session
        .execute("select 'other' = abs(0)")
        .expect("numeric comparison")
        .remove(0);
    assert_eq!(
        numeric.Next().expect("numeric comparison row"),
        Some(vec!["1".to_owned()]),
    );

    execute(&session, "create database string_result_compat");
    execute(&session, "use string_result_compat");
    execute(&session, "create table values_for_type (value varchar(20))");
    execute(&session, "insert into values_for_type values ('abc')");
    for sql in [
        "select 'other' = min(value) from values_for_type",
        "select 'other' = max(value) from values_for_type",
        "select 'other' = any_value(value) from values_for_type",
    ] {
        let mut result = session
            .execute(sql)
            .unwrap_or_else(|error| panic!("execute {sql}: {error}"))
            .remove(0);
        assert_eq!(
            result.Next().expect("aggregate comparison row"),
            Some(vec!["0".to_owned()]),
            "{sql} must compare as strings",
        );
    }
}

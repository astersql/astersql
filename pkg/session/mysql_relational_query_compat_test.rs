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

// MySQL 关系查询兼容性回归测试。
//
// 通过真实会话构造关联数据，覆盖聚合、连接、子查询与集合运算；除结果行外，
// 还校验 MySQL 列类型、字段来源元数据和重复列名的歧义诊断。

use crate::runtime::{ConcreteRecordSet, ConcreteSession, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;
use astersql_parser_mysql::r#type::TypeLong;

/// 执行建库、建表及写入语句；初始化失败时保留原 SQL，便于定位测试夹具问题。
fn execute(session: &ConcreteSession, sql: &str) {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("relational query setup failed: {sql}: {error}"));
}

/// 执行查询并同时核对列名、MySQL 类型及按顺序拉取的全部结果行。
///
/// 返回字段元数据，供需要进一步验证来源库、原始表与表别名的用例复用。
fn query(
    session: &ConcreteSession,
    sql: &str,
    columns: &[&str],
    types: &[u8],
    rows: &[&[&str]],
) -> Vec<Option<crate::runtime::ConcreteResultField>> {
    let mut result = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("relational query failed: {sql}: {error}"))
        .remove(0);
    assert_eq!(result.Columns(), columns, "column names for {sql}");
    let fields = result.result_fields().to_vec();
    assert_eq!(
        fields
            .iter()
            .map(|field| field.as_ref().expect("result field").column.GetType())
            .collect::<Vec<_>>(),
        types,
        "column types for {sql}"
    );
    let mut actual = Vec::new();
    while let Some(row) = result
        .Next()
        .unwrap_or_else(|error| panic!("read relational query row: {sql}: {error}"))
    {
        actual.push(row);
    }
    let expected = rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|cell| (*cell).to_owned())
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "result rows for {sql}");
    fields
}

#[test]
fn common_mysql_aggregate_join_subquery_and_union_queries_match_expected_rows() {
    use astersql_parser_mysql::r#type::{TypeLong, TypeLonglong, TypeNewDecimal, TypeVarchar};

    let (_domain, session) = CreateAnalyzeSession().expect("relational query session");
    execute(&session, "create database relational_compat");
    execute(&session, "use relational_compat");
    execute(
        &session,
        "create table departments (\
         id int not null,\
         name varchar(16) not null,\
         primary key (id))",
    );
    execute(
        &session,
        "create table employees (\
         id int not null,\
         department_id int null,\
         salary int null,\
         primary key (id))",
    );
    execute(
        &session,
        "create table bonuses (\
         employee_id int not null,\
         amount int not null)",
    );
    execute(
        &session,
        "insert into departments values (1, 'engineering'), (2, 'sales'), (3, 'operations')",
    );
    execute(
        &session,
        "insert into employees values \
         (1, 1, 100), (2, 1, 100), (3, 2, 80), (4, null, 50), (5, 2, null)",
    );
    execute(
        &session,
        "insert into bonuses values (1, 10), (1, 5), (3, 8), (6, 7)",
    );

    // 聚合用例同时固定 NULL 是否参与计数、求和及平均值的 MySQL 语义。
    query(
        &session,
        "select count(*) as rows_total, count(salary) as salaries, \
         sum(salary) as salary_sum, avg(salary) as salary_avg, \
         min(salary) as salary_min, max(salary) as salary_max \
         from employees",
        &[
            "rows_total",
            "salaries",
            "salary_sum",
            "salary_avg",
            "salary_min",
            "salary_max",
        ],
        &[
            TypeLonglong,
            TypeLonglong,
            TypeNewDecimal,
            TypeNewDecimal,
            TypeLong,
            TypeLong,
        ],
        &[&["5", "4", "330", "82.5000", "50", "100"]],
    );
    query(
        &session,
        "select department_id, count(*) as headcount, sum(salary) as payroll \
         from employees group by department_id \
         having headcount >= 2 order by headcount desc, department_id",
        &["department_id", "headcount", "payroll"],
        &[TypeLong, TypeLonglong, TypeNewDecimal],
        &[&["1", "2", "200"], &["2", "2", "80"]],
    );

    // 内连接除了结果值，还需保留投影列的原始表及 SQL 表别名信息。
    let join_fields = query(
        &session,
        "select e.id as employee_id, d.name as department \
         from employees e inner join departments d on e.department_id = d.id \
         order by e.id",
        &["employee_id", "department"],
        &[TypeLong, TypeVarchar],
        &[
            &["1", "engineering"],
            &["2", "engineering"],
            &["3", "sales"],
            &["5", "sales"],
        ],
    );
    let employee_id = join_fields[0].as_ref().expect("employee id metadata");
    assert_eq!(employee_id.db_name.L, "relational_compat");
    assert_eq!(employee_id.table_name.L, "employees");
    assert_eq!(employee_id.table_as_name.L, "e");
    assert_eq!(employee_id.column.Name.L, "id");
    let department = join_fields[1].as_ref().expect("department metadata");
    assert_eq!(department.table_name.L, "departments");
    assert_eq!(department.table_as_name.L, "d");
    assert_eq!(department.column.Name.L, "name");

    // 外连接与交叉连接分别验证未匹配行的 NULL 补齐和笛卡尔积基数。
    query(
        &session,
        "select d.id as department_id, e.id as employee_id \
         from departments d left join employees e on d.id = e.department_id \
         order by d.id, e.id",
        &["department_id", "employee_id"],
        &[TypeLong, TypeLong],
        &[
            &["1", "1"],
            &["1", "2"],
            &["2", "3"],
            &["2", "5"],
            &["3", "<nil>"],
        ],
    );
    query(
        &session,
        "select b.employee_id, e.id as matched_employee \
         from employees e right join bonuses b on e.id = b.employee_id \
         order by b.employee_id, b.amount",
        &["employee_id", "matched_employee"],
        &[TypeLong, TypeLong],
        &[&["1", "1"], &["1", "1"], &["3", "3"], &["6", "<nil>"]],
    );
    query(
        &session,
        "select count(*) as pair_count from departments cross join bonuses",
        &["pair_count"],
        &[TypeLonglong],
        &[&["12"]],
    );

    // 子查询组覆盖相关标量子查询、EXISTS，以及 NULL 参与 NOT IN 时的三值逻辑。
    query(
        &session,
        "select e.id, \
         (select max(b.amount) from bonuses b where b.employee_id = e.id) as max_bonus \
         from employees e order by e.id",
        &["id", "max_bonus"],
        &[TypeLong, TypeLong],
        &[
            &["1", "10"],
            &["2", "<nil>"],
            &["3", "8"],
            &["4", "<nil>"],
            &["5", "<nil>"],
        ],
    );
    query(
        &session,
        "select e.id from employees e \
         where e.department_id in (select d.id from departments d where d.name <> 'operations') \
         and exists (select 1 from bonuses b where b.employee_id = e.id) \
         order by e.id",
        &["id"],
        &[TypeLong],
        &[&["1"], &["3"]],
    );
    query(
        &session,
        "select e.id from employees e \
         where not exists (select 1 from bonuses b where b.employee_id = e.id) \
         order by e.id",
        &["id"],
        &[TypeLong],
        &[&["2"], &["4"], &["5"]],
    );
    query(
        &session,
        "select e.id from employees e \
         where e.id not in (select b.employee_id from bonuses b union all select null) \
         order by e.id",
        &["id"],
        &[TypeLong],
        &[],
    );

    // UNION 与 UNION ALL 使用同一数据集，明确去重与保留重复行的差异。
    query(
        &session,
        "select department_id as value from employees where department_id = 1 \
         union select id from departments where id = 1",
        &["value"],
        &[TypeLong],
        &[&["1"]],
    );
    query(
        &session,
        "select department_id as value from employees where department_id = 1 \
         union all select id from departments where id = 1",
        &["value"],
        &[TypeLong],
        &[&["1"], &["1"], &["1"]],
    );
}

#[test]
fn unqualified_duplicate_column_in_on_join_remains_ambiguous() {
    let (_domain, session) = CreateAnalyzeSession().expect("relational query session");
    execute(&session, "create database relational_join_ambiguity");
    execute(&session, "use relational_join_ambiguity");
    execute(&session, "create table left_t (id int not null)");
    execute(&session, "create table right_t (id int not null)");

    // JOIN 两侧存在同名列时，未限定的投影列必须在解析阶段报告歧义。
    let error =
        match session.execute("select id from left_t join right_t on left_t.id = right_t.id") {
            Ok(_) => panic!("an unqualified duplicate column in JOIN ... ON must be ambiguous"),
            Err(error) => error,
        };
    assert!(
        error
            .to_string()
            .contains("Column 'id' in field list is ambiguous"),
        "unexpected ambiguity error: {error}"
    );
}

#[test]
fn correlated_subquery_local_column_shadows_ambiguous_outer_column() {
    let (_domain, session) = CreateAnalyzeSession().expect("relational query session");
    execute(&session, "create database correlated_shadowing");
    execute(&session, "use correlated_shadowing");
    execute(&session, "create table t1 (a int not null)");
    execute(&session, "create table t2 (a int not null)");
    execute(&session, "create table t3 (a int not null)");
    execute(&session, "insert into t1 values (1), (2)");
    execute(&session, "insert into t2 values (1), (2)");
    execute(&session, "insert into t3 values (1)");

    query(
        &session,
        "select t1.a from t1 join t2 on t1.a = t2.a \
         where t1.a in (select a from t3) order by t1.a",
        &["a"],
        &[TypeLong],
        &[&["1"]],
    );
}

#[test]
fn parenthesized_join_wildcard_preserves_each_tables_column_values() {
    let (_domain, session) = CreateAnalyzeSession().expect("relational query session");
    execute(&session, "create database nested_join_columns");
    execute(&session, "use nested_join_columns");
    for table in ["t1", "t2", "t3", "t4"] {
        execute(&session, &format!("create table {table} (a int, b int)"));
    }
    execute(&session, "insert into t1 values (1, 10)");
    execute(&session, "insert into t2 values (1, 100)");
    execute(&session, "insert into t3 values (1, 1000)");
    execute(&session, "insert into t4 values (1, 10000)");

    query(
        &session,
        "select * from (t1 join t2 on t1.a = t2.a), \
         (t3 join t4 on t3.a = t4.a) order by t1.a, t3.a",
        &["a", "b", "a", "b", "a", "b", "a", "b"],
        &[TypeLong; 8],
        &[&["1", "10", "1", "100", "1", "1000", "1", "10000"]],
    );
}

#[test]
fn grouped_projection_preserves_declared_identifier_case() {
    use astersql_parser_mysql::r#type::TypeLonglong;

    let (_domain, session) = CreateAnalyzeSession().expect("relational query session");
    execute(&session, "create database wordpress_column_case");
    execute(&session, "use wordpress_column_case");
    execute(
        &session,
        "create table comments (\
         comment_ID bigint not null,\
         comment_post_ID bigint not null,\
         comment_approved varchar(20) not null,\
         comment_type varchar(20) not null)",
    );
    execute(
        &session,
        "insert into comments values (1, 6, '0', 'comment'), (2, 6, '1', 'comment')",
    );

    query(
        &session,
        "select comment_post_ID, count(comment_ID) as num_comments \
         from comments where comment_post_ID in ('1', '6') \
         and comment_approved = '0' and comment_type != 'note' \
         group by comment_post_ID",
        &["comment_post_ID", "num_comments"],
        &[TypeLonglong, TypeLonglong],
        &[&["6", "1"]],
    );
}

#[test]
fn count_extrema_sql_duplicates_nulls_and_sliding_window() {
    use astersql_parser_mysql::r#type::TypeLonglong;
    let (_domain, session) = CreateAnalyzeSession().unwrap();
    execute(&session, "create database count_extrema_regression");
    execute(&session, "use count_extrema_regression");
    execute(&session, "create table t(id int, a int)");
    execute(
        &session,
        "insert into t values (1,1),(2,1),(3,2),(4,2),(5,null),(6,2),(7,1)",
    );
    query(
        &session,
        "select max_count(a) as mx, min_count(a) as mn from t",
        &["mx", "mn"],
        &[TypeLonglong, TypeLonglong],
        &[&["3", "3"]],
    );
    query(
        &session,
        "select max_count(a) as mx, min_count(a) as mn from t where a is null",
        &["mx", "mn"],
        &[TypeLonglong, TypeLonglong],
        &[&["0", "0"]],
    );
    query(
        &session,
        "select max_count(a) over () as mx, min_count(a) over () as mn from t limit 1",
        &["mx", "mn"],
        &[TypeLonglong, TypeLonglong],
        &[&["3", "3"]],
    );
    query(
        &session,
        "select id,max_count(a) over (order by id rows between 1 preceding and current row) as mx,min_count(a) over (order by id rows between 1 preceding and current row) as mn from t order by id",
        &["id", "mx", "mn"],
        &[TypeLong, TypeLonglong, TypeLonglong],
        &[
            &["1", "1", "1"],
            &["2", "2", "2"],
            &["3", "1", "1"],
            &["4", "2", "2"],
            &["5", "1", "1"],
            &["6", "1", "1"],
            &["7", "1", "1"],
        ],
    );
    query(
        &session,
        "select max_count(all a) as mx,min_count(all a) as mn from t",
        &["mx", "mn"],
        &[TypeLonglong, TypeLonglong],
        &[&["3", "3"]],
    );
    query(
        &session,
        "select max_count(a) as mx,min_count(a) as mn from t where id < 0",
        &["mx", "mn"],
        &[TypeLonglong, TypeLonglong],
        &[&["0", "0"]],
    );
    query(
        &session,
        "select a,max_count(a) as mx,min_count(a) as mn from t group by a order by a",
        &["a", "mx", "mn"],
        &[TypeLong, TypeLonglong, TypeLonglong],
        &[&["<nil>", "0", "0"], &["1", "3", "3"], &["2", "3", "3"]],
    );
    for name in ["max_count", "min_count"] {
        assert!(
            session
                .execute(&format!("select {name}(distinct a) from t"))
                .is_err()
        );
    }
}

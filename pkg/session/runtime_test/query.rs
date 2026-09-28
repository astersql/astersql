// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
//
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// 查询与写入执行路径的端到端回归测试。
//
// 覆盖表达式类型语义、information_schema 的客户端兼容结果，以及
// INSERT SELECT 在冲突处理、原子性、时间戳复用和外连接场景下的行为。

use super::*;

// CTE 投影产生的是字符串；这里同时用数值比较作对照，防止常量传播错误地套用数值强制转换。
#[test]
fn cte_string_projection_uses_string_comparison_semantics() {
    let (_domain, session) =
        crate::runtime::CreateAnalyzeSession().expect("canonical comparison session");
    session
        .execute("create database constant_propagation_comparison")
        .expect("create comparison database");
    session
        .execute("use constant_propagation_comparison")
        .expect("select comparison database");
    session
        .execute("create table t (value decimal(30,30) not null)")
        .expect("create decimal source table");
    session
        .execute("insert into t values (0.000000000000000000000000000000)")
        .expect("insert decimal source row");

    let mut cte = session
        .execute(
            "with cte (text_value) as (select mid(value, 6, 9) from t) \
             select 1 from cte where text_value != ''",
        )
        .expect("execute typed CTE comparison")
        .remove(0);
    assert_eq!(
        cte.Next().expect("read typed CTE comparison"),
        Some(vec!["1".to_owned()])
    );

    let mut numeric = session
        .execute("select 0 = ''")
        .expect("execute numeric coercion comparison")
        .remove(0);
    assert_eq!(
        numeric.Next().expect("read numeric coercion comparison"),
        Some(vec!["1".to_owned()])
    );
}

// TiDB/MySQL coerces a mixed string/integer comparison to DOUBLE. Both values
// below round to the same IEEE-754 value even though their exact integers differ.
#[test]
fn mixed_string_integer_comparison_uses_double_semantics() {
    let (_domain, session) =
        crate::runtime::CreateAnalyzeSession().expect("canonical comparison session");
    let mut result = session
        .execute("select '9007199254740993' = 9007199254740992")
        .expect("execute mixed string/integer comparison")
        .remove(0);

    assert_eq!(
        result.Next().expect("read mixed comparison result"),
        Some(vec!["1".to_owned()])
    );
}

// Connector/J 会组合查询表、列、主键及空的存储过程元数据，本测试固定这些兼容字段的形态。
#[test]
fn concrete_information_schema_answers_connector_j_metadata_queries() {
    let (_domain, session) =
        crate::runtime::CreateAnalyzeSession().expect("canonical metadata session");
    session
        .execute("create database metadata_demo")
        .expect("create metadata database");
    session
        .execute("use metadata_demo")
        .expect("select metadata database");
    session
        .execute(
            "create table metadata_records (\
             id bigint auto_increment key,\
             name varchar(64) not null,\
             score int)",
        )
        .expect("create metadata table");

    let mut tables = session
        .execute(
            "select table_name, \
             case when table_type='BASE TABLE' then 'TABLE' else table_type end as table_type \
             from information_schema.tables \
             where table_schema='metadata_demo' and table_name='metadata_records'",
        )
        .expect("Connector/J table metadata query")
        .remove(0);
    assert_eq!(
        tables.Next().expect("read table metadata"),
        Some(vec!["metadata_records".to_owned(), "TABLE".to_owned()])
    );

    let mut columns = session
        .execute(
            "select column_name, \
             case when data_type='bigint' then -5 else 12 end as data_type, \
             upper(data_type) as type_name, ordinal_position, is_nullable, \
             case when extra like '%auto_increment%' then 'YES' else 'NO' end \
                 as is_autoincrement \
             from information_schema.columns \
             where table_schema='metadata_demo' and table_name like 'metadata_records'",
        )
        .expect("Connector/J column metadata query")
        .remove(0);
    assert_eq!(
        columns.Next().expect("read id metadata"),
        Some(vec![
            "id".to_owned(),
            "-5".to_owned(),
            "BIGINT".to_owned(),
            "1".to_owned(),
            "NO".to_owned(),
            "YES".to_owned(),
        ])
    );
    assert_eq!(
        columns.Next().expect("read name metadata"),
        Some(vec![
            "name".to_owned(),
            "12".to_owned(),
            "VARCHAR".to_owned(),
            "2".to_owned(),
            "NO".to_owned(),
            "NO".to_owned(),
        ])
    );
    assert_eq!(
        columns.Next().expect("read score metadata"),
        Some(vec![
            "score".to_owned(),
            "4".to_owned(),
            "INTEGER".to_owned(),
            "3".to_owned(),
            "YES".to_owned(),
            "NO".to_owned(),
        ])
    );
    assert_eq!(columns.Next().expect("column metadata exhausted"), None);

    let mut primary_keys = session
        .execute(
            "select table_name, column_name, seq_in_index as key_seq, 'PRIMARY' as pk_name \
             from information_schema.statistics \
             where table_schema='metadata_demo' and table_name='metadata_records' \
             and index_name='PRIMARY'",
        )
        .expect("Connector/J primary-key metadata query")
        .remove(0);
    assert_eq!(
        primary_keys.Next().expect("read primary key metadata"),
        Some(vec![
            "metadata_records".to_owned(),
            "id".to_owned(),
            "1".to_owned(),
            "PRIMARY".to_owned(),
        ])
    );

    for query in [
        "select routine_name from information_schema.routines \
         where routine_schema='metadata_demo'",
        "select parameter_name from information_schema.parameters \
         where specific_schema='metadata_demo'",
    ] {
        let mut result = session
            .execute(query)
            .unwrap_or_else(|error| panic!("empty metadata query failed: {error}"))
            .remove(0);
        assert_eq!(result.Next().expect("empty metadata result"), None);
    }
}

// 标准视图与 TiDB 扩展视图应从同一约束元数据生成一致的名称和表达式信息。
#[test]
fn concrete_information_schema_exposes_check_constraints() {
    let (_domain, session) =
        crate::runtime::CreateAnalyzeSession().expect("canonical metadata session");
    session
        .execute("create database constraint_metadata")
        .expect("create constraint metadata database");
    session
        .execute(
            "create table constraint_metadata.records (\
             value int, constraint value_is_positive check (value > 0))",
        )
        .expect("create table with CHECK constraint");

    let mut standard = session
        .execute(
            "select t.* from information_schema.CHECK_CONSTRAINTS t \
             where constraint_schema='constraint_metadata' limit 501",
        )
        .expect("query standard CHECK_CONSTRAINTS")
        .remove(0);
    let row = standard
        .Next()
        .expect("read CHECK_CONSTRAINTS")
        .expect("CHECK constraint row");
    assert_eq!(
        &row[..3],
        &["def", "constraint_metadata", "value_is_positive"]
    );
    let compact_check_clause: String = row[3]
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    assert!(
        compact_check_clause.contains("value") && compact_check_clause.contains(">0"),
        "unexpected CHECK_CLAUSE: {}",
        row[3],
    );
    assert_eq!(standard.Next().expect("standard rows exhausted"), None);

    let mut extended = session
        .execute(
            "select constraint_catalog, constraint_schema, constraint_name, \
             check_clause, table_name, table_id \
             from information_schema.TIDB_CHECK_CONSTRAINTS \
             where constraint_schema='constraint_metadata'",
        )
        .expect("query TiDB CHECK_CONSTRAINTS")
        .remove(0);
    let row = extended
        .Next()
        .expect("read TIDB_CHECK_CONSTRAINTS")
        .expect("TiDB CHECK constraint row");
    assert_eq!(
        &row[..3],
        &["def", "constraint_metadata", "value_is_positive"]
    );
    assert_eq!(row[4], "records");
    assert!(row[5].parse::<i64>().is_ok(), "TABLE_ID must be numeric");
    assert_eq!(extended.Next().expect("extended rows exhausted"), None);
}

// 派生表、UNION ALL、笛卡尔积、过滤和排序需在 INSERT SELECT 中按查询语义完整执行。
#[test]
fn insert_select_executes_derived_union_cross_join_filter_and_order_expressions() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database insert_select_runtime")
        .expect("create insert-select database");
    session
        .execute("use insert_select_runtime")
        .expect("select insert-select database");
    session
        .execute("create table numbers (n int unsigned not null primary key)")
        .expect("create target table");

    session
        .execute(
            "insert into numbers (n)
             select ones.n + tens.n * 10 + hundreds.n * 100
             from
               (select 0 n union all select 1 union all select 2) ones
             cross join
               (select 0 n union all select 1) tens
             cross join
               (select 0 n union all select 1) hundreds
             where ones.n + tens.n * 10 + hundreds.n * 100 < 112",
        )
        .expect("execute derived UNION ALL and CROSS JOIN insert-select");

    let mut result = session
        .execute("select n from numbers order by n")
        .expect("read generated numbers")
        .remove(0);
    let mut rows = Vec::new();
    while let Some(row) = result.Next().expect("read generated number") {
        rows.push(row);
    }
    assert_eq!(
        rows,
        vec![
            vec!["0".to_owned()],
            vec!["1".to_owned()],
            vec!["2".to_owned()],
            vec!["10".to_owned()],
            vec!["11".to_owned()],
            vec!["12".to_owned()],
            vec!["100".to_owned()],
            vec!["101".to_owned()],
            vec!["102".to_owned()],
            vec!["110".to_owned()],
            vec!["111".to_owned()],
        ]
    );
}

// 模拟订单装载器常用的字符串、数值、日期函数组合，验证投影值落表后的精度与格式。
#[test]
fn insert_select_executes_order_loader_projection() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database order_loader_runtime")
        .expect("create order-loader database");
    session
        .execute("use order_loader_runtime")
        .expect("select order-loader database");
    session
        .execute("create table load_numbers (n int unsigned not null primary key)")
        .expect("create helper table");
    session
        .execute("insert into load_numbers values (0),(1),(2)")
        .expect("seed helper table");
    session
        .execute(
            "create table orders (
                order_id bigint unsigned not null primary key,
                order_no varchar(32) not null,
                user_id bigint unsigned not null,
                amount decimal(12,2) not null,
                shipping_city varchar(32) not null,
                created_at datetime(3) not null
            )",
        )
        .expect("create orders table");

    session
        .execute(
            "insert into orders (
                order_id, order_no, user_id, amount, shipping_city, created_at
             )
             select
                100 + n,
                concat('ORD', lpad(100 + n, 5, '0')),
                mod(100 + n, 10) + 1,
                cast(mod((100 + n) * 97, 10000000) / 100 as decimal(12,2)),
                elt(mod(100 + n, 2) + 1, 'Shanghai', 'Beijing'),
                timestampadd(
                    second,
                    mod(100 + n, 31536000),
                    '2024-01-01 00:00:00.000'
                )
             from load_numbers
             where n < 2",
        )
        .expect("execute order-loader insert-select");

    let mut result = session
        .execute(
            "select order_id, order_no, user_id, amount, shipping_city, created_at
             from orders order by order_id",
        )
        .expect("read generated orders")
        .remove(0);
    let mut rows = Vec::new();
    while let Some(row) = result.Next().expect("read generated order") {
        rows.push(row);
    }
    assert_eq!(
        rows,
        vec![
            vec![
                "100".to_owned(),
                "ORD00100".to_owned(),
                "1".to_owned(),
                "97.00".to_owned(),
                "Shanghai".to_owned(),
                "2024-01-01 00:01:40.000".to_owned(),
            ],
            vec![
                "101".to_owned(),
                "ORD00101".to_owned(),
                "2".to_owned(),
                "97.97".to_owned(),
                "Beijing".to_owned(),
                "2024-01-01 00:01:41.000".to_owned(),
            ],
        ]
    );
}

// 多行 INSERT 的冲突检查必须共享同一个开始时间戳，因此整条语句只请求开始和提交两个时间戳。
#[test]
fn multi_row_insert_reuses_one_conflict_snapshot_timestamp() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database insert_snapshot_runtime")
        .expect("create insert snapshot database");
    session
        .execute("use insert_snapshot_runtime")
        .expect("select insert snapshot database");
    session
        .execute("create table target (id bigint primary key, payload varchar(32))")
        .expect("create insert snapshot target");

    let before = session
        .domain()
        .storage()
        .with_storage(|store| store.TSORequestCountForTest());
    session
        .execute("insert into target values (1,'a'),(2,'b'),(3,'c')")
        .expect("insert one multi-row statement");
    let after = session
        .domain()
        .storage()
        .with_storage(|store| store.TSORequestCountForTest());

    assert_eq!(
        after - before,
        2,
        "one INSERT statement must reuse one start timestamp and request one commit timestamp"
    );
}

// 批量装载仅可在数据库、表名完全匹配且不是 INSERT SELECT 时跳过已提交主键检查。
#[test]
fn bulk_load_primary_key_skip_requires_an_exact_plain_insert_table() {
    assert!(
        crate::runtime::ConcreteSession::bulk_load_skips_committed_primary_key_check(
            Some("load_test.orders_500m"),
            "load_test",
            "orders_500m",
            false,
        )
    );
    assert!(
        !crate::runtime::ConcreteSession::bulk_load_skips_committed_primary_key_check(
            Some("load_test.orders_500m"),
            "load_test",
            "other_orders",
            false,
        )
    );
    assert!(
        !crate::runtime::ConcreteSession::bulk_load_skips_committed_primary_key_check(
            Some("load_test.orders_500m"),
            "load_test",
            "orders_500m",
            true,
        )
    );
}

// INSERT SELECT 必须复用普通 INSERT 的冲突策略，并在失败时保证语句级原子性、不留下前缀行。
#[test]
fn insert_select_reuses_insert_conflict_and_atomicity_semantics() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database insert_select_atomicity")
        .expect("create atomicity database");
    session
        .execute("use insert_select_atomicity")
        .expect("select atomicity database");
    session
        .execute("create table target (id int primary key, payload int not null)")
        .expect("create target table");
    session
        .execute("create table source (id int primary key, payload int not null)")
        .expect("create source table");
    session
        .execute("insert into target values (2,20)")
        .expect("seed target row");
    session
        .execute("insert into source values (1,10),(2,22)")
        .expect("seed source rows");

    let error = session
        .execute("insert into target select id,payload from source order by id")
        .err()
        .expect("duplicate insert-select must fail");
    assert!(error.to_string().contains("Duplicate entry"));
    let mut result = session
        .execute("select id,payload from target order by id")
        .expect("read target after failed insert-select")
        .remove(0);
    assert_eq!(
        result.Next().expect("read preserved target row"),
        Some(vec!["2".to_owned(), "20".to_owned()])
    );
    assert_eq!(
        result.Next().expect("failed statement inserted no prefix"),
        None
    );

    session
        .execute("insert ignore into target select id,payload from source order by id")
        .expect("INSERT IGNORE SELECT");
    session
        .execute(
            "insert into target select id,payload from source order by id
             on duplicate key update payload=values(payload)",
        )
        .expect("INSERT SELECT ON DUPLICATE KEY UPDATE");
    let mut result = session
        .execute("select id,payload from target order by id")
        .expect("read conflict-handled target")
        .remove(0);
    assert_eq!(
        result.Next().expect("read first target row"),
        Some(vec!["1".to_owned(), "10".to_owned()])
    );
    assert_eq!(
        result.Next().expect("read updated target row"),
        Some(vec!["2".to_owned(), "22".to_owned()])
    );
    assert_eq!(result.Next().expect("target exhausted"), None);
}

// 外连接无法使用流式插入时应安全降级，并保留未匹配侧的 NULL 扩展行。
#[test]
fn insert_select_outer_joins_fall_back_from_streaming() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database insert_select_outer_join")
        .expect("create outer join database");
    session
        .execute("use insert_select_outer_join")
        .expect("select outer join database");
    session
        .execute("create table lhs (id int primary key, payload int)")
        .expect("create left source table");
    session
        .execute("create table rhs (id int primary key, payload int)")
        .expect("create right source table");
    session
        .execute("create table left_result (id int primary key, payload int)")
        .expect("create left join target");
    session
        .execute("create table right_result (id int primary key, payload int)")
        .expect("create right join target");
    session
        .execute("insert into lhs values (1,10),(2,20)")
        .expect("seed left source");
    session
        .execute("insert into rhs values (2,200),(3,300)")
        .expect("seed right source");

    session
        .execute(
            "insert into left_result \
             select lhs.id,rhs.payload from lhs left join rhs on lhs.id=rhs.id",
        )
        .expect("INSERT SELECT LEFT JOIN");
    session
        .execute(
            "insert into right_result \
             select rhs.id,lhs.payload from lhs right join rhs on lhs.id=rhs.id",
        )
        .expect("INSERT SELECT RIGHT JOIN");

    let mut left_rows = session
        .execute("select id,payload from left_result order by id")
        .expect("read left join result")
        .remove(0);
    assert_eq!(
        left_rows.Next().expect("read unmatched left row"),
        Some(vec!["1".to_owned(), "<nil>".to_owned()])
    );
    assert_eq!(
        left_rows.Next().expect("read matched left row"),
        Some(vec!["2".to_owned(), "200".to_owned()])
    );
    assert_eq!(left_rows.Next().expect("left result exhausted"), None);

    let mut right_rows = session
        .execute("select id,payload from right_result order by id")
        .expect("read right join result")
        .remove(0);
    assert_eq!(
        right_rows.Next().expect("read matched right row"),
        Some(vec!["2".to_owned(), "20".to_owned()])
    );
    assert_eq!(
        right_rows.Next().expect("read unmatched right row"),
        Some(vec!["3".to_owned(), "<nil>".to_owned()])
    );
    assert_eq!(right_rows.Next().expect("right result exhausted"), None);
}

// 只有无附加冲突语义的普通主键插入才能省略已有行扫描。
#[test]
fn plain_primary_key_insert_skips_existing_row_scan() {
    assert!(!ConcreteSession::relational_insert_requires_existing_rows(
        false, false, false, false,
    ));
    for arguments in [
        (true, false, false, false),
        (false, true, false, false),
        (false, false, true, false),
        (false, false, false, true),
    ] {
        assert!(ConcreteSession::relational_insert_requires_existing_rows(
            arguments.0,
            arguments.1,
            arguments.2,
            arguments.3,
        ));
    }
}

#[test]
fn nested_window_and_scalar_subquery_use_window_evaluation() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database nested_window_subquery")
        .expect("create test database");
    session
        .execute("use nested_window_subquery")
        .expect("select test database");
    session
        .execute("create table t1 (c1 int primary key)")
        .expect("create scalar-subquery table");
    session
        .execute("create table t2 (c1 int, c2 text)")
        .expect("create window source table");
    session
        .execute("insert into t1 values (10)")
        .expect("seed scalar-subquery table");
    session
        .execute("insert into t2 values (1,'alpha'),(1,'beta'),(2,'gamma')")
        .expect("seed window source table");

    let mut result = session
        .execute(
            "select coalesce(count(*) over (partition by ref_1.c1), \
             (select ref_1.c2 from t1)) from t2 as ref_1 \
             order by ref_1.c1, ref_1.c2",
        )
        .expect("execute nested window and scalar subquery")
        .remove(0);
    for expected in ["2", "2", "1"] {
        assert_eq!(
            result.Next().expect("read nested window result"),
            Some(vec![expected.to_owned()])
        );
    }
    assert_eq!(result.Next().expect("nested window result exhausted"), None);
}

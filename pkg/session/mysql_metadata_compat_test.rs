// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// MySQL 元数据兼容性回归测试。
//
// 这里模拟 DataGrip 与 Connector/J 的探测查询，验证 information_schema、
// performance_schema、sys 等系统库即使没有运行时数据，也能提供稳定的列结构、
// 约束关系和 MySQL/JDBC 约定的投影语义。

use crate::runtime::{CONCRETE_NULL_VALUE, ConcreteRecordSet, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

// 元数据查询预期只返回一个结果集；失败信息同时保留原 SQL，便于定位兼容性回归。
fn execute_record_set(session: &crate::runtime::ConcreteSession, sql: &str) -> ConcreteRecordSet {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("metadata query failed: {sql}: {error}"))
        .remove(0)
}

// 将流式结果完整物化，便于按 JDBC 可见的字符串形式核对列值与顺序。
fn collect(mut result: ConcreteRecordSet) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    while let Some(row) = result.Next().expect("read metadata row") {
        rows.push(row);
    }
    rows
}

fn execute_rows(session: &crate::runtime::ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    collect(execute_record_set(session, sql))
}

// 构造同时包含主键、唯一键、外键及可空默认值的最小元数据样本。
fn create_metadata_fixture(session: &crate::runtime::ConcreteSession) {
    for sql in [
        "create database metadata_compat",
        "use metadata_compat",
        "create table parent_records (\
         id bigint auto_increment primary key,\
         code varchar(32) not null,\
         unique key uk_parent_code(code))",
        "create table child_records (\
         id bigint auto_increment primary key,\
         parent_id bigint not null,\
         note varchar(64) null default 'draft',\
         unique key uk_child_note(note))",
        "alter table child_records add constraint fk_child_parent \
         foreign key (parent_id) references parent_records(id) \
         on delete cascade on update restrict",
    ] {
        session
            .execute(sql)
            .unwrap_or_else(|error| panic!("fixture statement failed: {sql}: {error}"));
    }
}

#[test]
fn performance_schema_mysql_introspection_uses_registered_empty_tables() {
    // 客户端会直接探测这些表；即使表为空，也必须在注册表中保留权威列定义。
    const CLIENT_REQUIRED_TABLES: [&str; 9] = [
        "cond_instances",
        "events_waits_current",
        "events_waits_history",
        "events_waits_history_long",
        "accounts",
        "hosts",
        "users",
        "binary_log_transaction_compression_stats",
        "events_transactions_summary_by_user_by_event_name",
    ];

    let (_domain, session) = CreateAnalyzeSession().expect("canonical metadata session");
    let performance_schema = astersql_infoschema_perfschema::build_performance_schema()
        .expect("authoritative performance_schema registry");

    for table_name in CLIENT_REQUIRED_TABLES {
        let table = performance_schema
            .tables
            .iter()
            .find(|table| table.name == table_name)
            .unwrap_or_else(|| panic!("missing registered performance_schema table {table_name}"));
        let expected_columns = table
            .columns
            .iter()
            .map(|column| column.name.clone())
            .collect::<Vec<_>>();
        let explicit_projection = expected_columns.join(", ");

        // 直接投影、通配符和派生表三条路径必须暴露相同的列顺序，且均不伪造数据行。
        let direct = execute_record_set(
            &session,
            &format!("select {explicit_projection} from performance_schema.`{table_name}`"),
        );
        assert_eq!(
            direct.columns(),
            expected_columns,
            "explicit projection must bind registered columns for {table_name}",
        );
        assert!(
            collect(direct).is_empty(),
            "registered table {table_name} must have no runtime rows",
        );

        let wildcard = execute_record_set(
            &session,
            &format!("select * from performance_schema.`{table_name}`"),
        );
        assert_eq!(
            wildcard.columns(),
            expected_columns,
            "wildcard projection must preserve registered column order for {table_name}",
        );
        assert!(collect(wildcard).is_empty());

        let derived = execute_record_set(
            &session,
            &format!(
                "select * from (select * from performance_schema.`{table_name}`) as registered"
            ),
        );
        assert_eq!(
            derived.columns(),
            expected_columns,
            "derived wildcard must preserve registered columns for {table_name}",
        );
        assert!(collect(derived).is_empty());

        assert_eq!(
            execute_rows(
                &session,
                &format!(
                    "select count(*) from (select * from performance_schema.`{table_name}`) as registered"
                ),
            ),
            vec![vec!["0".to_owned()]],
            "derived count must observe an empty registered table for {table_name}",
        );
    }

    // “已注册但为空”不能掩盖未知表错误，错误边界仍需符合 MySQL。
    let error = match session.execute("select * from performance_schema.not_registered") {
        Ok(_) => panic!("unknown performance_schema table must fail"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(
        message.contains("performance_schema.not_registered") && message.contains("doesn't exist"),
        "unknown table must retain the MySQL missing-table boundary: {message}",
    );
}

#[test]
fn datagrip_connector_j_discovers_complete_table_metadata() {
    // 按 Connector/J 的发现顺序核对库、表、列、索引和约束之间的关联。
    let (_domain, session) = CreateAnalyzeSession().expect("canonical metadata session");
    create_metadata_fixture(&session);

    assert_eq!(
        execute_rows(
            &session,
            "select schema_name as table_cat \
             from information_schema.schemata \
             where schema_name='metadata_compat'",
        ),
        vec![vec!["metadata_compat".to_owned()]],
    );
    assert_eq!(
        execute_rows(
            &session,
            "select table_schema as table_cat, table_name, \
             case when table_type='BASE TABLE' then 'TABLE' else table_type end as table_type \
             from information_schema.tables \
             where table_schema='metadata_compat' and table_name like 'child_records' \
             order by table_schema, table_name",
        ),
        vec![vec![
            "metadata_compat".to_owned(),
            "child_records".to_owned(),
            "TABLE".to_owned(),
        ]],
    );

    // JDBC 类型编号、可空性、默认值和自增标志都来自 information_schema 的派生表达式。
    let columns = execute_rows(
        &session,
        "select table_schema as table_cat, table_name, column_name, \
         case when data_type='bigint' then -5 else 12 end as data_type, \
         upper(data_type) as type_name, ordinal_position, is_nullable, column_default, \
         case when extra like '%auto_increment%' then 'YES' else 'NO' end as is_autoincrement \
         from information_schema.columns \
         where table_schema='metadata_compat' and table_name='child_records' \
         order by table_schema, table_name, ordinal_position",
    );
    assert_eq!(
        columns,
        vec![
            vec![
                "metadata_compat".to_owned(),
                "child_records".to_owned(),
                "id".to_owned(),
                "-5".to_owned(),
                "BIGINT".to_owned(),
                "1".to_owned(),
                "NO".to_owned(),
                String::new(),
                "YES".to_owned(),
            ],
            vec![
                "metadata_compat".to_owned(),
                "child_records".to_owned(),
                "parent_id".to_owned(),
                "-5".to_owned(),
                "BIGINT".to_owned(),
                "2".to_owned(),
                "NO".to_owned(),
                String::new(),
                "NO".to_owned(),
            ],
            vec![
                "metadata_compat".to_owned(),
                "child_records".to_owned(),
                "note".to_owned(),
                "12".to_owned(),
                "VARCHAR".to_owned(),
                "3".to_owned(),
                "YES".to_owned(),
                "draft".to_owned(),
                "NO".to_owned(),
            ],
        ],
    );

    // 外键隐式索引也必须可见，同时主键不能因约束与索引两条来源而重复。
    let statistics = execute_rows(
        &session,
        "select table_name, non_unique, index_name, seq_in_index, column_name \
         from information_schema.statistics \
         where table_schema='metadata_compat' and table_name='child_records' \
         order by index_name, seq_in_index",
    );
    assert!(
        statistics.contains(&vec![
            "child_records".to_owned(),
            "0".to_owned(),
            "PRIMARY".to_owned(),
            "1".to_owned(),
            "id".to_owned(),
        ]),
        "primary key missing from statistics: {statistics:?}",
    );
    assert!(
        statistics.contains(&vec![
            "child_records".to_owned(),
            "0".to_owned(),
            "uk_child_note".to_owned(),
            "1".to_owned(),
            "note".to_owned(),
        ]),
        "unique index missing from statistics: {statistics:?}",
    );
    assert!(
        statistics.contains(&vec![
            "child_records".to_owned(),
            "1".to_owned(),
            "fk_child_parent".to_owned(),
            "1".to_owned(),
            "parent_id".to_owned(),
        ]),
        "foreign-key index missing from statistics: {statistics:?}",
    );
    assert_eq!(
        statistics.iter().filter(|row| row[2] == "PRIMARY").count(),
        1,
        "primary index must be reported once: {statistics:?}",
    );

    let constraints = execute_rows(
        &session,
        "select constraint_name, table_name, constraint_type \
         from information_schema.table_constraints \
         where constraint_schema='metadata_compat' and table_name='child_records' \
         order by constraint_name",
    );
    for expected in [
        vec![
            "PRIMARY".to_owned(),
            "child_records".to_owned(),
            "PRIMARY KEY".to_owned(),
        ],
        vec![
            "uk_child_note".to_owned(),
            "child_records".to_owned(),
            "UNIQUE".to_owned(),
        ],
        vec![
            "fk_child_parent".to_owned(),
            "child_records".to_owned(),
            "FOREIGN KEY".to_owned(),
        ],
    ] {
        assert!(
            constraints.contains(&expected),
            "constraint {expected:?} missing from {constraints:?}",
        );
    }

    let key_usage = execute_rows(
        &session,
        "select constraint_name, table_name, column_name, ordinal_position, \
         referenced_table_schema, referenced_table_name, referenced_column_name \
         from information_schema.key_column_usage \
         where constraint_schema='metadata_compat' and table_name='child_records' \
         order by constraint_name, ordinal_position",
    );
    assert!(
        key_usage.contains(&vec![
            "fk_child_parent".to_owned(),
            "child_records".to_owned(),
            "parent_id".to_owned(),
            "1".to_owned(),
            "metadata_compat".to_owned(),
            "parent_records".to_owned(),
            "id".to_owned(),
        ]),
        "foreign-key usage missing: {key_usage:?}",
    );

    assert_eq!(
        execute_rows(
            &session,
            "select constraint_name, unique_constraint_name, update_rule, delete_rule, \
             table_name, referenced_table_name \
             from information_schema.referential_constraints \
             where constraint_schema='metadata_compat' and table_name='child_records'",
        ),
        vec![vec![
            "fk_child_parent".to_owned(),
            "PRIMARY".to_owned(),
            "RESTRICT".to_owned(),
            "CASCADE".to_owned(),
            "child_records".to_owned(),
            "parent_records".to_owned(),
        ]],
    );

    assert!(
        execute_rows(&session, "show databases")
            .iter()
            .any(|row| row == &vec!["metadata_compat".to_owned()]),
    );
    assert_eq!(
        execute_rows(&session, "show tables"),
        vec![
            vec!["child_records".to_owned()],
            vec!["parent_records".to_owned()],
        ],
    );
    let full_columns = execute_rows(&session, "show full columns from child_records");
    assert_eq!(full_columns.len(), 3);
    assert_eq!(full_columns[2][0], "note");
    assert_eq!(full_columns[2][5], "draft");
    let described = execute_rows(&session, "describe child_records");
    assert_eq!(
        described
            .iter()
            .map(|row| row[0].as_str())
            .collect::<Vec<_>>(),
        vec!["id", "parent_id", "note"],
    );
    let shown_indexes = execute_rows(&session, "show index from child_records");
    assert!(
        shown_indexes
            .iter()
            .any(|row| row[2] == "uk_child_note" && row[4] == "note"),
    );

    // DDL 后的元数据查询必须立即看到新增列，避免客户端刷新仍读取旧模式。
    session
        .execute("alter table child_records add column refreshed_at datetime(3) null")
        .expect("alter metadata fixture");
    assert_eq!(
        execute_rows(
            &session,
            "select column_name, ordinal_position \
             from information_schema.columns \
             where table_schema='metadata_compat' and table_name='child_records' \
             and column_name='refreshed_at'",
        ),
        vec![vec!["refreshed_at".to_owned(), "4".to_owned()]],
    );
    assert!(
        execute_rows(&session, "describe child_records")
            .iter()
            .any(|row| row[0] == "refreshed_at"),
    );
}

#[test]
fn datagrip_full_introspection_materializes_system_metadata_in_derived_queries() {
    // DataGrip 会把系统表包在派生查询中；物化后仍需保留系统对象和注册列。
    let (_domain, session) = CreateAnalyzeSession().expect("canonical metadata session");

    for (schema, expected_object) in [
        ("mysql", "user"),
        ("performance_schema", "setup_consumers"),
        ("sys", "schema_unused_indexes"),
    ] {
        let rows = execute_rows(
            &session,
            &format!(
                "select object_name from (\
                 select table_name as object_name \
                 from information_schema.tables \
                 where table_schema='{schema}'\
                 ) as introspected_objects \
                 where object_name='{expected_object}'",
            ),
        );
        assert_eq!(
            rows,
            vec![vec![expected_object.to_owned()]],
            "full introspection must enumerate {schema}.{expected_object}",
        );
    }

    assert_eq!(
        execute_rows(
            &session,
            "select table_schema, table_name, index_name, last_access_time \
             from information_schema.cluster_tidb_index_usage",
        ),
        Vec::<Vec<String>>::new(),
    );
    assert_eq!(
        execute_rows(
            &session,
            "select object_schema, object_name, index_name from sys.schema_unused_indexes",
        ),
        Vec::<Vec<String>>::new(),
    );

    assert_eq!(
        execute_rows(
            &session,
            "select count(*) from (\
             select * from information_schema.column_privileges\
             ) as column_privileges",
        ),
        vec![vec!["0".to_owned()]],
    );

    for (table, projected_columns) in [
        ("user_privileges", "grantee, table_catalog"),
        ("schema_privileges", "grantee, table_catalog, table_schema"),
        (
            "table_privileges",
            "grantee, table_catalog, table_schema, table_name",
        ),
    ] {
        assert_eq!(
            execute_rows(
                &session,
                &format!(
                    "select count(*) from (select {projected_columns} from information_schema.{table}) as privileges"
                ),
            ),
            vec![vec!["0".to_owned()]],
            "full introspection must expose the registered columns of {table}",
        );
    }

    assert_eq!(
        execute_rows(
            &session,
            "select grantee, table_name, column_name, privilege_type, is_grantable \
             from information_schema.column_privileges \
             where table_schema = 'metadata_compat' \
             union all \
             select grantee, table_name, '' column_name, privilege_type, is_grantable \
             from information_schema.table_privileges \
             where table_schema = 'metadata_compat' \
             order by table_name, grantee, privilege_type",
        ),
        Vec::<Vec<String>>::new(),
    );

    assert_eq!(
        execute_rows(
            &session,
            "select collation_name, character_set_name, is_default, is_compiled, pad_attribute \
             from information_schema.collations where collation_name='utf8mb4_bin'",
        ),
        vec![vec![
            "utf8mb4_bin".to_owned(),
            "utf8mb4".to_owned(),
            "Yes".to_owned(),
            "Yes".to_owned(),
            "PAD SPACE".to_owned(),
        ]],
    );

    assert_eq!(
        execute_rows(
            &session,
            "select count(*) from (\
             select type, instance, `key`, value \
             from information_schema.CLUSTER_CONFIG\
             ) as cluster_config",
        ),
        vec![vec!["0".to_owned()]],
    );

    assert_eq!(
        execute_rows(
            &session,
            "select thread_id, event_id, event_name, operation \
             from performance_schema.events_waits_history",
        ),
        Vec::<Vec<String>>::new(),
    );
    assert_eq!(
        execute_rows(
            &session,
            "select count(*) from (\
             select * from performance_schema.events_waits_history_long\
             ) as events_waits_history_long",
        ),
        vec![vec!["0".to_owned()]],
    );

    assert_eq!(
        execute_rows(
            &session,
            "select user, host, current_connections, total_connections \
             from performance_schema.accounts",
        ),
        Vec::<Vec<String>>::new(),
    );
    for table in ["hosts", "users"] {
        assert_eq!(
            execute_rows(
                &session,
                &format!(
                    "select count(*) from (select * from performance_schema.{table}) as summary"
                ),
            ),
            vec![vec!["0".to_owned()]],
        );
    }

    assert_eq!(
        execute_rows(
            &session,
            "select log_type, compression_type, transaction_counter, \
             compressed_bytes_counter, uncompressed_bytes_counter, compression_percentage \
             from performance_schema.binary_log_transaction_compression_stats",
        ),
        Vec::<Vec<String>>::new(),
    );
    assert_eq!(
        execute_rows(
            &session,
            "select count(*) from (\
             select * from performance_schema.binary_log_transaction_compression_stats\
             ) as binary_log_transaction_compression_stats",
        ),
        vec![vec!["0".to_owned()]],
    );

    assert_eq!(
        execute_rows(
            &session,
            "select user, event_name, count_star, sum_timer_wait, count_read_write, \
             count_read_only \
             from performance_schema.events_transactions_summary_by_user_by_event_name",
        ),
        Vec::<Vec<String>>::new(),
    );
    assert_eq!(
        execute_rows(
            &session,
            "select count(*) from (\
             select * \
             from performance_schema.events_transactions_summary_by_user_by_event_name\
             ) as transaction_summary",
        ),
        vec![vec!["0".to_owned()]],
    );

    for sql in [
        "select table_schema from information_schema.statistics group by table_schema",
        "select table_schema from information_schema.partitions group by table_schema",
        "select table_schema from information_schema.key_column_usage group by table_schema",
        "select table_schema from information_schema.table_constraints group by table_schema",
        "select table_schema from information_schema.views group by table_schema",
        "select grantee from information_schema.user_privileges group by grantee",
    ] {
        execute_rows(&session, sql);
    }

    // 排序规则的数值字段在直接查询与派生查询中都必须保持可解析的数值表示。
    let collation_rows = execute_rows(
        &session,
        "select collation_name, character_set_name, id, is_default, is_compiled, sortlen \
         from information_schema.collations where collation_name='utf8mb4_bin'",
    );
    assert_eq!(collation_rows.len(), 1);
    assert!(collation_rows[0][2].parse::<u64>().is_ok());
    assert!(collation_rows[0][5].parse::<u64>().is_ok());
    let derived_collation_rows = execute_rows(
        &session,
        "select id, sortlen from (\
         select * from information_schema.collations\
         ) as collations where collation_name='utf8mb4_bin'",
    );
    assert_eq!(derived_collation_rows.len(), 1);
    assert!(derived_collation_rows[0][0].parse::<u64>().is_ok());
    assert!(derived_collation_rows[0][1].parse::<u64>().is_ok());

    assert_eq!(
        execute_rows(
            &session,
            "select variable, value from sys.sys_config order by variable",
        ),
        vec![
            vec!["diagnostics.allow_i_s_tables".to_owned(), "OFF".to_owned()],
            vec!["diagnostics.include_raw".to_owned(), "OFF".to_owned()],
            vec![
                "ps_thread_trx_info.max_length".to_owned(),
                "65535".to_owned(),
            ],
            vec![
                "statement_performance_analyzer.limit".to_owned(),
                "100".to_owned(),
            ],
            vec![
                "statement_performance_analyzer.view".to_owned(),
                "<nil>".to_owned(),
            ],
            vec!["statement_truncate_len".to_owned(), "64".to_owned()],
        ],
    );
    assert_eq!(
        execute_rows(
            &session,
            "select count(*) from (select * from sys.sys_config) as sys_config",
        ),
        vec![vec!["6".to_owned()]],
    );
    assert_eq!(
        execute_rows(
            &session,
            "select time, instance, value \
             from metrics_schema.node_disk_available_size",
        ),
        Vec::<Vec<String>>::new(),
    );
    assert_eq!(
        execute_rows(
            &session,
            "select count(*) from (\
             select time, instance, value \
             from metrics_schema.node_disk_available_size\
             ) as node_disk_available_size",
        ),
        vec![vec!["0".to_owned()]],
    );

    assert_eq!(
        execute_rows(
            &session,
            "select name, object_instance_begin \
             from performance_schema.cond_instances",
        ),
        Vec::<Vec<String>>::new(),
    );
    assert_eq!(
        execute_rows(
            &session,
            "select count(*) from (\
             select * from performance_schema.cond_instances\
             ) as cond_instances",
        ),
        vec![vec!["0".to_owned()]],
    );

    // 覆盖客户端携带应用注释与 LIMIT 的真实统计表探测语句。
    execute_rows(&session, "select * from mysql.stats_meta");
    execute_rows(
        &session,
        "/* ApplicationName=DataGrip 2025.1.3 */ \
         SELECT t.* FROM mysql.stats_buckets t LIMIT 501",
    );
    execute_rows(
        &session,
        "/* ApplicationName=DataGrip 2025.1.3 */ \
         SELECT t.* FROM mysql.stats_meta t LIMIT 501",
    );

    // NULL 与布尔字面量需映射为运行时约定的 JDBC 可见字符串。
    assert_eq!(
        execute_rows(&session, "select null, false, true"),
        vec![vec![
            CONCRETE_NULL_VALUE.to_owned(),
            "0".to_owned(),
            "1".to_owned(),
        ]],
    );
}

#[test]
fn datagrip_native_introspection_supports_boolean_projection_and_positional_order() {
    // 原生探测依赖布尔比较结果，并使用 ORDER BY 序号稳定排列库、表和列。
    let (_domain, session) = CreateAnalyzeSession().expect("canonical metadata session");

    let collations = execute_rows(
        &session,
        "select collation_name, character_set_name, \
         lower(is_default) = 'yes' as is_default \
         from information_schema.collations \
         where collation_name = 'utf8mb4_bin'",
    );
    assert_eq!(
        collations,
        vec![vec![
            "utf8mb4_bin".to_owned(),
            "utf8mb4".to_owned(),
            "1".to_owned(),
        ]],
        "DataGrip reads the derived is_default value as a JDBC boolean",
    );

    let schemas = execute_rows(
        &session,
        "select schema_name as major_name, default_collation_name \
         from information_schema.schemata order by 1",
    );
    assert!(schemas.windows(2).all(|rows| rows[0][0] <= rows[1][0]));

    let normalized_schemas = execute_rows(
        &session,
        "select lower(schema_name) as normalized_name \
         from information_schema.schemata order by 1",
    );
    assert!(
        normalized_schemas
            .windows(2)
            .all(|rows| rows[0][0] <= rows[1][0])
    );

    let tables = execute_rows(
        &session,
        "select table_schema as schema_name, table_name as major_name, \
         table_type, engine from information_schema.tables order by 1,2,4",
    );
    assert!(tables.windows(2).all(|rows| {
        (&rows[0][0], &rows[0][1], &rows[0][3]) <= (&rows[1][0], &rows[1][1], &rows[1][3])
    }));

    let minor_names = execute_rows(
        &session,
        "select T.table_schema as schema_name, \
                T.table_name as major_name, \
                case when T.table_type like '%TABLE' then 'T' \
                     when T.table_type like '%VIEW' then 'V' end as major_kind, \
                C.ordinal_position as position, \
                cast(null as char(1)) as direction, \
                C.column_name as minor_name \
         from information_schema.tables T, information_schema.columns C \
         where T.table_schema in ('information_schema', 'mysql', \
                                  'performance_schema', 'sys', \
                                  'metrics_schema', 'test', 'metadata_compat') \
           and T.table_schema = C.table_schema \
           and T.table_name = C.table_name \
         order by 1,2,4",
    );
    assert!(!minor_names.is_empty());
    assert!(minor_names.iter().all(|row| row[3].parse::<u64>().is_ok()));
}

#[test]
fn information_schema_table_ids_keep_pairs_when_schema_is_dropped() {
    let (domain, session) = CreateAnalyzeSession().expect("metadata session");
    for sql in [
        "create database metadata_drop",
        "create database metadata_keep",
        "create table metadata_drop.z_removed (id int)",
        "create table metadata_drop.a_removed (id int)",
        "create table metadata_keep.b_retained (id int)",
        "create table metadata_keep.a_retained (id int)",
    ] {
        session
            .execute(sql)
            .expect("create nonempty schema fixtures");
    }
    let before = execute_rows(
        &session,
        "select table_schema, table_name, tidb_table_id from information_schema.tables \
         where table_schema in ('metadata_drop', 'metadata_keep') \
         order by table_schema, table_name",
    );
    assert_eq!(before.len(), 4);
    assert_eq!(
        before
            .iter()
            .map(|row| (row[0].as_str(), row[1].as_str()))
            .collect::<Vec<_>>(),
        [
            ("metadata_drop", "a_removed"),
            ("metadata_drop", "z_removed"),
            ("metadata_keep", "a_retained"),
            ("metadata_keep", "b_retained"),
        ],
    );
    let ids = before
        .iter()
        .map(|row| row[2].as_str())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "select table_schema, table_name, tidb_table_id from information_schema.tables \
         where tidb_table_id in ({ids}) order by table_schema, table_name"
    );
    // The real consumer owns paired catalog data before DDL. Drop the schema
    // from another thread before reading any rows: no later SchemaByID lookup
    // may panic or detach a surviving table from its schema.
    let pending = execute_record_set(&session, &sql);
    let ddl_domain = domain.clone();
    std::thread::spawn(move || {
        ddl_domain
            .ddl_drop_database("metadata_drop", false)
            .expect("drop schema");
    })
    .join()
    .expect("DDL thread must not panic");
    assert!(
        !domain
            .ddl_database_names()
            .expect("read canonical schema names")
            .iter()
            .any(|name| name.eq_ignore_ascii_case("metadata_drop"))
    );
    assert_eq!(collect(pending), before);
    let retained = before
        .into_iter()
        .filter(|row| row[0] == "metadata_keep")
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), 2);
    assert_eq!(execute_rows(&session, &sql), retained);
    assert!(
        execute_rows(
            &session,
            "select table_schema, table_name from information_schema.tables \
         where table_schema = 'metadata_drop'",
        )
        .is_empty()
    );
}

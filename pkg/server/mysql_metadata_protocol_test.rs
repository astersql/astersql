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

// 通过真实 TCP 连接验证 MySQL 元数据协议的兼容性。
//
// 测试覆盖 JDBC 等客户端依赖的系统目录、权限表与 Performance Schema，
// 同时核对结果集列定义包中的来源标识、类型、标志位及 DDL 后的元数据刷新。

use crate::mysql_compat_test_support::{
    CLIENT_DEPRECATE_EOF, ColumnDefinition, MysqlCompatServer, MysqlTestClient, TextResultSet,
    TextValue, WireResponse,
};
use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, Instant};

const MYSQL_TYPE_LONGLONG: u8 = 0x08;
const MYSQL_TYPE_LONG: u8 = 0x03;
const MYSQL_TYPE_VAR_STRING: u8 = 0xfd;
const NOT_NULL_FLAG: u16 = 0x0001;
const PRI_KEY_FLAG: u16 = 0x0002;
const UNSIGNED_FLAG: u16 = 0x0020;
const AUTO_INCREMENT_FLAG: u16 = 0x0200;

// 需要执行建库、授权等准备语句时，统一要求协议层返回 OK 包。
fn expect_ok(response: WireResponse, context: &str) {
    assert!(
        matches!(response, WireResponse::Ok(_)),
        "{context} did not return OK: {response:?}",
    );
}

// 按列名取得协议返回的完整列定义，便于集中核对元数据字段。
fn column<'a>(columns: &'a [ColumnDefinition], name: &str) -> &'a ColumnDefinition {
    columns
        .iter()
        .find(|column| column.name == name)
        .unwrap_or_else(|| panic!("column {name} missing from {columns:?}"))
}

// 元数据查询必须返回结果集；OK 或 ERR 等其他包都表示协议契约不符。
fn query_result_set(client: &mut MysqlTestClient, sql: &str) -> TextResultSet {
    match client
        .query(sql)
        .unwrap_or_else(|error| panic!("query system metadata over real TCP: {sql}: {error}"))
    {
        WireResponse::ResultSet(result) => result,
        response => {
            panic!("system metadata query did not return a result set: {sql}: {response:?}")
        }
    }
}

// 将文本协议行转换为 UTF-8 字符串；这些系统目录查询不应产生 NULL 单元格。
fn text_rows(result: &TextResultSet) -> Vec<Vec<String>> {
    result
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|value| match value {
                    TextValue::Bytes(value) => String::from_utf8(value.clone())
                        .unwrap_or_else(|error| panic!("metadata cell is not UTF-8: {error}")),
                    TextValue::Null => panic!("unexpected NULL metadata cell in {row:?}"),
                })
                .collect()
        })
        .collect()
}

// 规范化对象名和类型的大小写，避免系统目录与 SHOW 命令的展示差异干扰比较。
fn object_types(result: &TextResultSet) -> BTreeMap<String, String> {
    text_rows(result)
        .into_iter()
        .map(|row| {
            assert_eq!(
                row.len(),
                2,
                "system object row must contain name and type: {row:?}",
            );
            (row[0].to_ascii_lowercase(), row[1].to_ascii_uppercase())
        })
        .collect()
}

// 即使查询没有返回数据行，结果集也必须携带完整且有序的列定义。
fn assert_column_names(result: &TextResultSet, expected: &[&str]) {
    assert_eq!(
        result
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect::<Vec<_>>(),
        expected,
    );
}

// 核对系统目录投影列在协议层保留其来源表和原始列名。
fn assert_metadata_column_identity(
    columns: &[ColumnDefinition],
    name: &str,
    table: &str,
    org_name: &str,
) {
    let metadata = column(columns, name);
    assert_eq!(metadata.catalog, "def");
    assert_eq!(metadata.schema, "information_schema");
    assert!(
        metadata.table.eq_ignore_ascii_case(table),
        "{name} table identity is {:?}, expected {table}",
        metadata.table,
    );
    assert!(
        metadata.org_name.eq_ignore_ascii_case(org_name),
        "{name} original column identity is {:?}, expected {org_name}",
        metadata.org_name,
    );
}

#[test]
fn mysql_protocol_exposes_system_schema_catalog_for_jdbc_clients() {
    let server = MysqlCompatServer::start().expect("start real MySQL listener");
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("connect root JDBC-equivalent client");

    let schemata = query_result_set(
        &mut client,
        "select schema_name from information_schema.schemata order by schema_name",
    );
    let schema_names = text_rows(&schemata)
        .into_iter()
        .map(|row| {
            assert_eq!(row.len(), 1, "SCHEMATA row must contain one name: {row:?}");
            row[0].to_ascii_lowercase()
        })
        .collect::<Vec<_>>();

    // 每类系统库选取一个代表对象，贯通验证库、表、列三级目录。
    let expected_catalogs = [
        ("information_schema", "TABLES", "BASE TABLE", "TABLE_SCHEMA"),
        ("mysql", "user", "BASE TABLE", "Host"),
        (
            "performance_schema",
            "setup_consumers",
            "BASE TABLE",
            "NAME",
        ),
        ("sys", "schema_unused_indexes", "VIEW", "object_schema"),
        ("metrics_schema", "uptime", "BASE TABLE", "time"),
    ];

    let mut table_columns = None;
    let mut columns_columns = None;
    for (schema, expected_table, expected_type, expected_column) in expected_catalogs {
        assert!(
            schema_names.iter().any(|candidate| candidate == schema),
            "system schema {schema} missing from INFORMATION_SCHEMA.SCHEMATA: {schema_names:?}",
        );

        let tables = query_result_set(
            &mut client,
            &format!(
                "select table_name, table_type from information_schema.tables \
                 where table_schema='{schema}' order by table_name",
            ),
        );
        let catalog_objects = object_types(&tables);
        assert_eq!(
            catalog_objects
                .get(&expected_table.to_ascii_lowercase())
                .map(String::as_str),
            Some(expected_type),
            "{schema}.{expected_table} missing from INFORMATION_SCHEMA.TABLES: {catalog_objects:?}",
        );

        let shown = query_result_set(&mut client, &format!("show full tables from {schema}"));
        let shown_objects = object_types(&shown);
        assert_eq!(
            shown_objects, catalog_objects,
            "{schema} differs between SHOW FULL TABLES and INFORMATION_SCHEMA.TABLES",
        );

        let columns = query_result_set(
            &mut client,
            &format!(
                "select table_schema, table_name, column_name, ordinal_position \
                 from information_schema.columns \
                 where table_schema='{schema}' and table_name='{expected_table}' \
                 order by ordinal_position",
            ),
        );
        let column_rows = text_rows(&columns);
        assert!(
            column_rows.iter().any(|row| {
                row.len() == 4
                    && row[0].eq_ignore_ascii_case(schema)
                    && row[1].eq_ignore_ascii_case(expected_table)
                    && row[2].eq_ignore_ascii_case(expected_column)
            }),
            "{schema}.{expected_table}.{expected_column} missing from \
             INFORMATION_SCHEMA.COLUMNS: {column_rows:?}",
        );

        // 各轮查询投影相同，只需保存一份列定义用于后续协议字段核对。
        table_columns.get_or_insert_with(|| tables.columns.clone());
        columns_columns.get_or_insert_with(|| columns.columns.clone());
    }

    assert_metadata_column_identity(&schemata.columns, "schema_name", "SCHEMATA", "SCHEMA_NAME");
    let table_columns = table_columns.expect("TABLES query column definitions");
    assert_metadata_column_identity(&table_columns, "table_name", "TABLES", "TABLE_NAME");
    assert_metadata_column_identity(&table_columns, "table_type", "TABLES", "TABLE_TYPE");
    let columns_columns = columns_columns.expect("COLUMNS query column definitions");
    assert_metadata_column_identity(&columns_columns, "column_name", "COLUMNS", "COLUMN_NAME");
    assert_metadata_column_identity(
        &columns_columns,
        "ordinal_position",
        "COLUMNS",
        "ORDINAL_POSITION",
    );
    let ordinal_position = column(&columns_columns, "ordinal_position");
    assert_eq!(ordinal_position.column_type, MYSQL_TYPE_LONG);
    assert_eq!(ordinal_position.flags & UNSIGNED_FLAG, UNSIGNED_FLAG);

    // sys 兼容层只注册已支持的视图，不应虚构存储过程或 MySQL 8 专属对象。
    let sys_views = query_result_set(
        &mut client,
        "select table_name, check_option, is_updatable, security_type \
         from information_schema.views where table_schema='sys' order by table_name",
    );
    assert_eq!(
        text_rows(&sys_views),
        vec![vec![
            "schema_unused_indexes".to_owned(),
            "NONE".to_owned(),
            "NO".to_owned(),
            "DEFINER".to_owned(),
        ]],
    );
    for (catalog, schema_column) in [
        ("routines", "routine_schema"),
        ("parameters", "specific_schema"),
    ] {
        let result = query_result_set(
            &mut client,
            &format!(
                "select count(*) from information_schema.{catalog} where {schema_column}='sys'"
            ),
        );
        assert_eq!(
            text_rows(&result),
            vec![vec!["0".to_owned()]],
            "unsupported sys stored programs must not receive {catalog} rows",
        );
    }
    let unsupported = client
        .query("select * from sys.innodb_buffer_stats_by_schema")
        .expect("receive unsupported sys view response");
    assert!(
        matches!(unsupported, WireResponse::Err(_)),
        "unsupported MySQL 8 sys view must return a missing-object error: {unsupported:?}",
    );

    // 用必定为空的过滤条件验证 SELECT * 的列布局，避免数据内容掩盖元数据问题。
    let schemata_all = query_result_set(
        &mut client,
        "select * from information_schema.schemata \
         where schema_name='__astersql_missing_schema__'",
    );
    assert_column_names(
        &schemata_all,
        &[
            "CATALOG_NAME",
            "SCHEMA_NAME",
            "DEFAULT_CHARACTER_SET_NAME",
            "DEFAULT_COLLATION_NAME",
            "SQL_PATH",
            "TIDB_PLACEMENT_POLICY_NAME",
        ],
    );
    let tables_all = query_result_set(
        &mut client,
        "select * from information_schema.tables \
         where table_schema='__astersql_missing_schema__'",
    );
    assert_column_names(
        &tables_all,
        &[
            "TABLE_CATALOG",
            "TABLE_SCHEMA",
            "TABLE_NAME",
            "TABLE_TYPE",
            "ENGINE",
            "VERSION",
            "ROW_FORMAT",
            "TABLE_ROWS",
            "AVG_ROW_LENGTH",
            "DATA_LENGTH",
            "MAX_DATA_LENGTH",
            "INDEX_LENGTH",
            "DATA_FREE",
            "AUTO_INCREMENT",
            "CREATE_TIME",
            "UPDATE_TIME",
            "CHECK_TIME",
            "TABLE_COLLATION",
            "CHECKSUM",
            "CREATE_OPTIONS",
            "TABLE_COMMENT",
            "TIDB_TABLE_ID",
            "TIDB_ROW_ID_SHARDING_INFO",
            "TIDB_PK_TYPE",
            "TIDB_PLACEMENT_POLICY_NAME",
            "TIDB_TABLE_MODE",
            "TIDB_AFFINITY",
            "TIDB_STORAGE_CLASS",
        ],
    );
    let columns_all = query_result_set(
        &mut client,
        "select * from information_schema.columns \
         where table_schema='__astersql_missing_schema__'",
    );
    assert_column_names(
        &columns_all,
        &[
            "TABLE_CATALOG",
            "TABLE_SCHEMA",
            "TABLE_NAME",
            "COLUMN_NAME",
            "ORDINAL_POSITION",
            "COLUMN_DEFAULT",
            "IS_NULLABLE",
            "DATA_TYPE",
            "CHARACTER_MAXIMUM_LENGTH",
            "CHARACTER_OCTET_LENGTH",
            "NUMERIC_PRECISION",
            "NUMERIC_SCALE",
            "DATETIME_PRECISION",
            "CHARACTER_SET_NAME",
            "COLLATION_NAME",
            "COLUMN_TYPE",
            "COLUMN_KEY",
            "EXTRA",
            "PRIVILEGES",
            "COLUMN_COMMENT",
            "GENERATION_EXPRESSION",
            "SRS_ID",
        ],
    );
}

#[test]
fn show_processlist_reports_integer_scale_over_mysql_protocol() {
    let server = MysqlCompatServer::start().expect("start real MySQL listener");
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("connect root processlist client");

    // JDBC 会依据类型和小数位判断整数列，数值文本也不得带浮点表示。
    let result = query_result_set(&mut client, "show processlist");
    let id = column(&result.columns, "Id");
    assert_eq!(id.column_type, MYSQL_TYPE_LONGLONG);
    assert_eq!(id.decimals, 0);
    let time = column(&result.columns, "Time");
    assert_eq!(time.column_type, MYSQL_TYPE_LONG);
    assert_eq!(time.decimals, 0);
    assert!(
        result.rows.iter().all(
            |row| matches!(&row[0], TextValue::Bytes(value) if value.iter().all(u8::is_ascii_digit))
        ),
        "SHOW PROCESSLIST Id values must remain integer text",
    );
}

#[test]
fn mysql_protocol_exposes_collations_and_persisted_users() {
    let server = MysqlCompatServer::start().expect("start real MySQL listener");
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("connect root metadata client");

    let collations = query_result_set(&mut client, "show collation like 'utf8mb4_bin'");
    assert_eq!(
        text_rows(&collations),
        vec![vec![
            "utf8mb4_bin".to_owned(),
            "utf8mb4".to_owned(),
            "46".to_owned(),
            "Yes".to_owned(),
            "Yes".to_owned(),
            "1".to_owned(),
            "PAD SPACE".to_owned(),
        ]],
    );
    let collation_id = column(&collations.columns, "Id");
    assert_eq!(collation_id.column_type, MYSQL_TYPE_LONGLONG);
    assert_eq!(collation_id.decimals, 0);
    assert_eq!(
        collation_id.flags & (UNSIGNED_FLAG | NOT_NULL_FLAG),
        UNSIGNED_FLAG | NOT_NULL_FLAG,
    );

    // 用户及多层级权限必须在 mysql.* 兼容表中随 GRANT/REVOKE 同步持久化。
    expect_ok(
        client
            .query("create user 'metadata_reader'@'localhost'")
            .expect("create metadata user"),
        "CREATE USER",
    );
    let users = query_result_set(
        &mut client,
        "select User, Host from mysql.user where User='metadata_reader'",
    );
    assert_eq!(
        text_rows(&users),
        vec![vec!["metadata_reader".to_owned(), "localhost".to_owned()]],
    );
    expect_ok(
        client
            .query("grant select, insert on test.* to 'metadata_reader'@'localhost'")
            .expect("grant database privileges"),
        "GRANT",
    );
    expect_ok(
        client
            .query("grant update on test.* to 'metadata_reader'@'localhost'")
            .expect("extend database privileges"),
        "GRANT UPDATE",
    );
    let database_privileges = query_result_set(
        &mut client,
        "select Host, DB, User, Select_priv, Insert_priv, Update_priv \
         from mysql.db where User='metadata_reader' and DB='test'",
    );
    assert_eq!(
        text_rows(&database_privileges),
        vec![vec![
            "localhost".to_owned(),
            "test".to_owned(),
            "metadata_reader".to_owned(),
            "Y".to_owned(),
            "Y".to_owned(),
            "Y".to_owned(),
        ]],
    );
    expect_ok(
        client
            .query("create table test.metadata_privileges (id int primary key, visible int)")
            .expect("create privilege target"),
        "CREATE TABLE privilege target",
    );
    for (sql, context) in [
        (
            "grant process on *.* to 'metadata_reader'@'localhost'",
            "GRANT global",
        ),
        (
            "grant delete on test.metadata_privileges to 'metadata_reader'@'localhost'",
            "GRANT table",
        ),
        (
            "grant update (visible) on test.metadata_privileges to 'metadata_reader'@'localhost'",
            "GRANT column",
        ),
    ] {
        expect_ok(client.query(sql).expect(context), context);
    }
    assert_eq!(
        text_rows(&query_result_set(
            &mut client,
            "select Process_priv from mysql.user where User='metadata_reader'",
        )),
        vec![vec!["Y".to_owned()]],
    );
    assert_eq!(
        text_rows(&query_result_set(
            &mut client,
            "select Table_priv from mysql.tables_priv where User='metadata_reader'",
        )),
        vec![vec!["Delete".to_owned()]],
    );
    assert_eq!(
        text_rows(&query_result_set(
            &mut client,
            "select Column_name, Column_priv from mysql.columns_priv where User='metadata_reader'",
        )),
        vec![vec!["visible".to_owned(), "Update".to_owned()]],
    );
    for (sql, context) in [
        (
            "revoke process on *.* from 'metadata_reader'@'localhost'",
            "REVOKE global",
        ),
        (
            "revoke delete on test.metadata_privileges from 'metadata_reader'@'localhost'",
            "REVOKE table",
        ),
        (
            "revoke update (visible) on test.metadata_privileges from 'metadata_reader'@'localhost'",
            "REVOKE column",
        ),
    ] {
        expect_ok(client.query(sql).expect(context), context);
    }
    assert!(
        text_rows(&query_result_set(
            &mut client,
            "select * from mysql.tables_priv where User='metadata_reader'",
        ))
        .is_empty()
    );
    assert!(
        text_rows(&query_result_set(
            &mut client,
            "select * from mysql.columns_priv where User='metadata_reader'",
        ))
        .is_empty()
    );
    expect_ok(
        client
            .query("revoke select, insert, update on test.* from 'metadata_reader'@'localhost'")
            .expect("revoke database privileges"),
        "REVOKE database",
    );
    assert!(
        text_rows(&query_result_set(
            &mut client,
            "select * from mysql.db where User='metadata_reader'",
        ))
        .is_empty()
    );
    expect_ok(
        client
            .query("drop user 'metadata_reader'@'localhost'")
            .expect("drop metadata user"),
        "DROP USER",
    );
    assert!(
        text_rows(&query_result_set(
            &mut client,
            "select * from mysql.user where User='metadata_reader'",
        ))
        .is_empty()
    );

    // sys_config 提供客户端探测所需的默认兼容配置。
    let sys_config = query_result_set(
        &mut client,
        "select variable, value from sys.sys_config \
         where variable='diagnostics.allow_i_s_tables'",
    );
    assert_eq!(
        text_rows(&sys_config),
        vec![vec![
            "diagnostics.allow_i_s_tables".to_owned(),
            "OFF".to_owned(),
        ]],
    );
}

#[test]
fn mysql_protocol_serves_performance_schema_compatibility_tables() {
    // 这些表由常见 MySQL 客户端直接探测，即使暂不产出数据也必须可查询。
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

    let server = MysqlCompatServer::start().expect("start real MySQL listener");
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("connect root performance_schema client");

    let shown = object_types(&query_result_set(
        &mut client,
        "show full tables from performance_schema",
    ));
    for table_name in CLIENT_REQUIRED_TABLES {
        assert_eq!(
            shown.get(table_name).map(String::as_str),
            Some("BASE TABLE"),
            "performance_schema.{table_name} missing from SHOW FULL TABLES: {shown:?}",
        );
    }

    let catalog_columns = query_result_set(
        &mut client,
        "select table_name, column_name from information_schema.columns \
         where table_schema='performance_schema' order by table_name, ordinal_position",
    );
    let mut columns_by_table = BTreeMap::<String, Vec<String>>::new();
    for row in text_rows(&catalog_columns) {
        assert_eq!(
            row.len(),
            2,
            "INFORMATION_SCHEMA.COLUMNS row must contain table and column names: {row:?}",
        );
        columns_by_table
            .entry(row[0].to_ascii_lowercase())
            .or_default()
            .push(row[1].clone());
    }

    assert_eq!(
        columns_by_table.get("cond_instances"),
        Some(&vec!["name".to_owned(), "object_instance_begin".to_owned(),]),
    );
    assert_eq!(
        columns_by_table.get("accounts"),
        Some(&vec![
            "user".to_owned(),
            "host".to_owned(),
            "current_connections".to_owned(),
            "total_connections".to_owned(),
            "max_session_controlled_memory".to_owned(),
            "max_session_total_memory".to_owned(),
        ]),
    );
    assert_eq!(
        columns_by_table.get("binary_log_transaction_compression_stats"),
        Some(&vec![
            "log_type".to_owned(),
            "compression_type".to_owned(),
            "transaction_counter".to_owned(),
            "compressed_bytes_counter".to_owned(),
            "uncompressed_bytes_counter".to_owned(),
            "compression_percentage".to_owned(),
        ]),
    );

    for table_name in CLIENT_REQUIRED_TABLES {
        // 连接汇总表返回当前会话，其余占位表保持空集但仍暴露完整列定义。
        let dynamic_connection_summary = matches!(table_name, "accounts" | "hosts" | "users");
        let expected_columns = columns_by_table
            .get(table_name)
            .unwrap_or_else(|| panic!("{table_name} missing from INFORMATION_SCHEMA.COLUMNS"));
        assert!(
            !expected_columns.is_empty(),
            "performance_schema.{table_name} must expose column metadata",
        );

        let explicit = query_result_set(
            &mut client,
            &format!(
                "select {} from performance_schema.`{table_name}`",
                expected_columns.join(", ")
            ),
        );
        assert_eq!(
            explicit
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            expected_columns
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            "explicit projection metadata differs for performance_schema.{table_name}",
        );
        assert_eq!(
            explicit.rows.is_empty(),
            !dynamic_connection_summary,
            "performance_schema.{table_name} dynamic/static row contract differs",
        );

        let wildcard = query_result_set(
            &mut client,
            &format!("select * from performance_schema.`{table_name}`"),
        );
        assert_eq!(
            wildcard
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            expected_columns
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            "wildcard metadata differs for performance_schema.{table_name}",
        );
        assert_eq!(
            wildcard.rows.is_empty(),
            !dynamic_connection_summary,
            "performance_schema.{table_name} wildcard dynamic/static row contract differs",
        );

        let count = query_result_set(
            &mut client,
            &format!(
                "select count(*) from (select * from performance_schema.`{table_name}`) \
                 as registered"
            ),
        );
        assert_eq!(
            text_rows(&count),
            vec![vec![if dynamic_connection_summary {
                "1".to_owned()
            } else {
                "0".to_owned()
            }]],
            "derived count must observe the performance_schema.{table_name} row contract",
        );
    }
}

#[test]
fn performance_schema_account_summaries_follow_connection_lifecycle() {
    let server = MysqlCompatServer::start().expect("start real MySQL listener");
    let mut observer = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("connect performance_schema observer");

    let accounts = query_result_set(
        &mut observer,
        "select user, host, current_connections, total_connections \
         from performance_schema.accounts order by user, host",
    );
    assert_eq!(
        text_rows(&accounts),
        vec![vec![
            "root".to_owned(),
            "127.0.0.1".to_owned(),
            "1".to_owned(),
            "1".to_owned(),
        ]],
        "the observer connection must appear in the account summary",
    );

    // 鉴权失败的连接未建立会话，不能计入当前或累计连接数。
    assert!(
        server
            .connect_user("missing_user", CLIENT_DEPRECATE_EOF)
            .is_err(),
        "unknown user authentication must fail",
    );
    let after_failed_auth = query_result_set(
        &mut observer,
        "select current_connections, total_connections from performance_schema.accounts",
    );
    assert_eq!(
        text_rows(&after_failed_auth),
        vec![vec!["1".to_owned(), "1".to_owned()]],
        "failed authentication must not change current or total connection counts",
    );

    let second = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("connect concurrent root session");
    for table in ["accounts", "users", "hosts"] {
        let counts = query_result_set(
            &mut observer,
            &format!(
                "select current_connections, total_connections \
                 from performance_schema.{table}"
            ),
        );
        assert_eq!(
            text_rows(&counts),
            vec![vec!["2".to_owned(), "2".to_owned()]],
            "performance_schema.{table} must count both live TCP sessions",
        );
    }

    drop(second);
    // 服务端异步感知 TCP 关闭，因此在限时轮询中等待当前连接数收敛。
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let counts = query_result_set(
            &mut observer,
            "select current_connections, total_connections \
             from performance_schema.accounts",
        );
        if text_rows(&counts) == vec![vec!["1".to_owned(), "2".to_owned()]] {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "closed TCP session did not leave the current account count: {counts:?}",
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn mysql_metadata_packets_preserve_column_identity_and_flags() {
    let server = MysqlCompatServer::start().expect("start real MySQL listener");
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("connect to real MySQL listener");
    for sql in [
        "create database metadata_wire",
        "use metadata_wire",
        "create table parent_records (\
         id bigint auto_increment primary key,\
         code varchar(32) not null unique)",
        "create table child_records (\
         id bigint auto_increment primary key,\
         parent_id bigint not null,\
         note varchar(64) null default 'draft',\
         unique key uk_child_note(note))",
        "alter table child_records add constraint fk_child_parent \
         foreign key (parent_id) references parent_records(id)",
    ] {
        expect_ok(
            client
                .query(sql)
                .unwrap_or_else(|error| panic!("send fixture statement {sql}: {error}")),
            sql,
        );
    }

    // SELECT 的列定义需同时保留来源标识、类型、长度和键/空值/自增标志。
    let WireResponse::ResultSet(selected) = client
        .query("select id, note from child_records")
        .expect("read real TCP SELECT metadata")
    else {
        panic!("table SELECT did not return a result set");
    };
    assert!(selected.rows.is_empty());
    let id = column(&selected.columns, "id");
    assert_eq!(id.catalog, "def");
    assert_eq!(id.schema, "metadata_wire");
    assert_eq!(id.table, "child_records");
    assert_eq!(id.org_table, "child_records");
    assert_eq!(id.org_name, "id");
    assert_eq!(id.column_type, MYSQL_TYPE_LONGLONG);
    assert_eq!(
        id.flags & (NOT_NULL_FLAG | PRI_KEY_FLAG | AUTO_INCREMENT_FLAG),
        NOT_NULL_FLAG | PRI_KEY_FLAG | AUTO_INCREMENT_FLAG,
    );
    assert_ne!(id.character_set, 0);
    assert_ne!(id.column_length, 0);

    let note = column(&selected.columns, "note");
    assert_eq!(note.schema, "metadata_wire");
    assert_eq!(note.table, "child_records");
    assert_eq!(note.org_table, "child_records");
    assert_eq!(note.org_name, "note");
    assert_eq!(note.column_type, MYSQL_TYPE_VAR_STRING);
    assert_ne!(note.character_set, 0);
    assert_eq!(note.column_length, 64 * 4);

    // COM_FIELD_LIST 与 SELECT 应基于同一份表元数据，并额外返回列默认值。
    let fields = client
        .field_list("child_records", "%")
        .expect("read COM_FIELD_LIST over real TCP");
    assert_eq!(
        fields
            .iter()
            .map(|field| field.name.as_str())
            .collect::<Vec<_>>(),
        vec!["id", "parent_id", "note"],
    );
    let field_note = column(&fields, "note");
    assert_eq!(field_note.schema, "metadata_wire");
    assert_eq!(field_note.table, "child_records");
    assert_eq!(field_note.org_table, "child_records");
    assert_eq!(field_note.org_name, "note");
    assert_eq!(field_note.column_type, MYSQL_TYPE_VAR_STRING);
    assert_eq!(
        field_note.default_value.as_deref(),
        Some(b"draft".as_slice())
    );

    // DDL 后再次请求两条协议路径，确认没有复用过期的列定义缓存。
    expect_ok(
        client
            .query("alter table child_records add column refreshed_at datetime(3) null")
            .expect("alter table through real TCP"),
        "ALTER TABLE ADD COLUMN",
    );
    let refreshed_fields = client
        .field_list("child_records", "%")
        .expect("refresh COM_FIELD_LIST after ALTER");
    assert!(
        refreshed_fields
            .iter()
            .any(|field| field.name == "refreshed_at"),
        "COM_FIELD_LIST did not refresh: {refreshed_fields:?}",
    );
    let WireResponse::ResultSet(refreshed) = client
        .query("select refreshed_at from child_records")
        .expect("read refreshed SELECT metadata")
    else {
        panic!("refreshed SELECT did not return a result set");
    };
    let refreshed = column(&refreshed.columns, "refreshed_at");
    assert_eq!(refreshed.schema, "metadata_wire");
    assert_eq!(refreshed.table, "child_records");
    assert_eq!(refreshed.org_name, "refreshed_at");
    assert_eq!(refreshed.decimals, 3);
}

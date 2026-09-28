// Copyright 2026 AsterSQL.

// MySQL 系统库元数据的端到端回归测试。
//
// 通过规范会话验证各系统库的对象可见性、两种元数据查询入口的一致性、
// 关键列序，以及同一领域中新建会话仍能读取完整系统元数据。

use std::collections::BTreeMap;

use crate::runtime::{ConcreteRecordSet, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

// 将分页结果集完整展开，便于后续断言同时展示原始行作为诊断信息。
fn collect_system_metadata_rows(mut result: ConcreteRecordSet) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    while let Some(row) = result.Next().expect("read system metadata row") {
        rows.push(row);
    }
    rows
}

// 系统元数据查询在本测试中都应只返回一个结果集。
fn execute_system_metadata_rows(
    session: &crate::runtime::ConcreteSession,
    sql: &str,
) -> Vec<Vec<String>> {
    let result = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("system metadata query failed: {sql}: {error}"))
        .remove(0);
    collect_system_metadata_rows(result)
}

// 对象名和类型分别归一化，以消除不同元数据入口的大小写表现差异。
fn object_types(rows: &[Vec<String>]) -> BTreeMap<String, String> {
    rows.iter()
        .map(|row| {
            assert_eq!(
                row.len(),
                2,
                "system object metadata must contain name and type: {row:?}",
            );
            (row[0].to_ascii_lowercase(), row[1].to_ascii_uppercase())
        })
        .collect()
}

// 只约束兼容性依赖的关键列及其序号，允许系统表以后追加其他列。
fn assert_stable_columns(
    session: &crate::runtime::ConcreteSession,
    schema: &str,
    table: &str,
    expected: &[(&str, &str)],
) {
    let sql = format!(
        "select column_name, ordinal_position \
         from information_schema.columns \
         where table_schema='{schema}' and table_name='{table}' \
         order by ordinal_position",
    );
    let rows = execute_system_metadata_rows(session, &sql);
    let actual = rows
        .iter()
        .map(|row| {
            assert_eq!(
                row.len(),
                2,
                "system column metadata must contain name and position: {row:?}",
            );
            (row[0].to_ascii_lowercase(), row[1].clone())
        })
        .collect::<BTreeMap<_, _>>();

    for &(column, position) in expected {
        assert_eq!(
            actual.get(&column.to_ascii_lowercase()).map(String::as_str),
            Some(position),
            "{schema}.{table}.{column} must keep ordinal position {position}; rows: {rows:?}",
        );
    }
}

#[test]
fn canonical_session_exposes_system_schema_objects_and_columns() {
    let (domain, session) = CreateAnalyzeSession().expect("canonical system metadata session");
    let databases = execute_system_metadata_rows(&session, "show databases");

    // 每个系统库选取代表性对象，覆盖基础表与视图两类元数据。
    let expected_catalogs: &[(&str, &[(&str, &str)])] = &[
        (
            "information_schema",
            &[
                ("TABLES", "BASE TABLE"),
                ("COLUMNS", "BASE TABLE"),
                ("SCHEMATA", "BASE TABLE"),
            ],
        ),
        (
            "mysql",
            &[
                ("user", "BASE TABLE"),
                ("global_variables", "BASE TABLE"),
                ("tidb", "BASE TABLE"),
            ],
        ),
        (
            "performance_schema",
            &[
                ("global_status", "BASE TABLE"),
                ("session_status", "BASE TABLE"),
                ("setup_consumers", "BASE TABLE"),
            ],
        ),
        ("sys", &[("schema_unused_indexes", "VIEW")]),
        ("metrics_schema", &[("uptime", "BASE TABLE")]),
    ];

    // 系统库必须先通过数据库枚举入口对客户端可见。
    for (schema, _) in expected_catalogs {
        assert!(
            databases
                .iter()
                .any(|row| row
                    .first()
                    .is_some_and(|database| database.eq_ignore_ascii_case(schema))),
            "system schema {schema} missing from SHOW DATABASES: {databases:?}",
        );
    }

    // SHOW 与 INFORMATION_SCHEMA 必须给出完全相同的对象名称和类型。
    for (schema, expected_objects) in expected_catalogs {
        let shown_rows =
            execute_system_metadata_rows(&session, &format!("show full tables from {schema}"));
        let shown_objects = object_types(&shown_rows);
        for &(table, table_type) in *expected_objects {
            assert_eq!(
                shown_objects
                    .get(&table.to_ascii_lowercase())
                    .map(String::as_str),
                Some(table_type),
                "{schema}.{table} must be enumerable after SHOW DATABASES exposed {schema}; \
                 SHOW DATABASES rows: {databases:?}; SHOW FULL TABLES rows: {shown_rows:?}",
            );
        }

        let catalog_rows = execute_system_metadata_rows(
            &session,
            &format!(
                "select table_name, table_type \
                 from information_schema.tables \
                 where table_schema='{schema}' \
                 order by table_name",
            ),
        );
        let catalog_objects = object_types(&catalog_rows);
        assert_eq!(
            catalog_objects, shown_objects,
            "{schema} object names and types differ between INFORMATION_SCHEMA.TABLES \
             and SHOW FULL TABLES; information_schema rows: {catalog_rows:?}; \
             SHOW rows: {shown_rows:?}",
        );
    }

    // 固定客户端常用列的位置，防止元数据协议在重构中发生不兼容漂移。
    assert_stable_columns(
        &session,
        "information_schema",
        "TABLES",
        &[
            ("TABLE_SCHEMA", "2"),
            ("TABLE_NAME", "3"),
            ("TABLE_TYPE", "4"),
        ],
    );
    assert_stable_columns(&session, "mysql", "user", &[("Host", "1"), ("User", "2")]);
    assert_stable_columns(
        &session,
        "performance_schema",
        "setup_consumers",
        &[("NAME", "1"), ("ENABLED", "2")],
    );
    assert_stable_columns(
        &session,
        "sys",
        "schema_unused_indexes",
        &[
            ("object_schema", "1"),
            ("object_name", "2"),
            ("index_name", "3"),
        ],
    );
    assert_stable_columns(
        &session,
        "metrics_schema",
        "uptime",
        &[
            ("time", "1"),
            ("instance", "2"),
            ("job", "3"),
            ("value", "4"),
        ],
    );

    // 复用同一领域创建新会话，确认系统元数据并非首个会话的临时状态。
    let reconnected = crate::runtime::ConcreteSession::new(std::sync::Arc::clone(&domain));
    let mysql = object_types(&execute_system_metadata_rows(
        &reconnected,
        "show full tables from mysql",
    ));
    let sys = object_types(&execute_system_metadata_rows(
        &reconnected,
        "show full tables from sys",
    ));
    assert_eq!(mysql.get("user").map(String::as_str), Some("BASE TABLE"));
    assert_eq!(
        sys.get("schema_unused_indexes").map(String::as_str),
        Some("VIEW"),
    );
}

// Copyright 2026 AsterSQL.

// `performance_schema` 连接账户汇总的运行时回归测试。
//
// 验证 `accounts` 保留每个用户与主机组合（包括匿名身份），而 `users` 和
// `hosts` 分别按单一身份维度汇总当前连接数与累计连接数。

use astersql_session_sessmgr::PerformanceSchemaAccountSummary;

use crate::runtime::system_query::performance_schema_connection_summary_rows;

/// 从虚拟表结果中读取列值，并将缺失列或 SQL NULL 统一表示为 `None`。
fn value<'a>(
    row: &'a std::collections::HashMap<String, Option<String>>,
    column: &str,
) -> Option<&'a str> {
    row.get(column).and_then(Option::as_deref)
}

#[test]
/// 验证账户明细、用户汇总和主机汇总遵循各自的分组及空值语义。
fn performance_schema_account_rows_group_users_hosts_and_anonymous_identities() {
    let summaries = vec![
        PerformanceSchemaAccountSummary {
            user: Some("root".to_owned()),
            host: Some("127.0.0.1".to_owned()),
            current_connections: 2,
            total_connections: 3,
        },
        PerformanceSchemaAccountSummary {
            user: Some("root".to_owned()),
            host: Some("10.0.0.8".to_owned()),
            current_connections: 1,
            total_connections: 4,
        },
        PerformanceSchemaAccountSummary {
            user: None,
            host: None,
            current_connections: 0,
            total_connections: 1,
        },
    ];

    // `accounts` 不跨用户或主机聚合，并且匿名账户仍以 NULL 身份出现。
    let accounts = performance_schema_connection_summary_rows(&summaries, "accounts");
    assert_eq!(accounts.len(), 3);
    assert!(accounts.iter().all(|row| {
        value(row, "max_session_controlled_memory") == Some("0")
            && value(row, "max_session_total_memory") == Some("0")
    }));
    assert!(accounts.iter().any(|row| {
        value(row, "user") == Some("root")
            && value(row, "host") == Some("127.0.0.1")
            && value(row, "current_connections") == Some("2")
            && value(row, "total_connections") == Some("3")
    }));
    assert!(accounts.iter().any(|row| {
        value(row, "user").is_none()
            && value(row, "host").is_none()
            && value(row, "total_connections") == Some("1")
    }));

    // `users` 将同一用户在不同主机上的连接计数相加。
    let users = performance_schema_connection_summary_rows(&summaries, "users");
    assert_eq!(users.len(), 2);
    assert!(users.iter().any(|row| {
        value(row, "user") == Some("root")
            && value(row, "current_connections") == Some("3")
            && value(row, "total_connections") == Some("7")
            && value(row, "max_session_controlled_memory") == Some("0")
            && value(row, "max_session_total_memory") == Some("0")
    }));
    assert!(users.iter().any(|row| {
        value(row, "user").is_none()
            && value(row, "current_connections") == Some("0")
            && value(row, "total_connections") == Some("1")
    }));

    // `hosts` 以主机为分组键，并保留该主机的当前及累计连接计数。
    let hosts = performance_schema_connection_summary_rows(&summaries, "hosts");
    assert_eq!(hosts.len(), 3);
    assert!(hosts.iter().any(|row| {
        value(row, "host") == Some("10.0.0.8")
            && value(row, "current_connections") == Some("1")
            && value(row, "total_connections") == Some("4")
    }));
    assert!(hosts.iter().any(|row| {
        value(row, "host").is_none()
            && value(row, "current_connections") == Some("0")
            && value(row, "total_connections") == Some("1")
    }));

    // 虚拟表名匹配与 SQL 标识符一致，不区分 ASCII 大小写；其它表不产生结果。
    assert_eq!(
        performance_schema_connection_summary_rows(&summaries, "AcCoUnTs"),
        accounts
    );
    assert!(performance_schema_connection_summary_rows(&summaries, "threads").is_empty());
}

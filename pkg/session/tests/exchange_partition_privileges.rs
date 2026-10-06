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

use std::sync::Arc;

use astersql_parser_auth::parser::auth::auth::UserIdentity;

use astersql_session::runtime::{ConcreteSession, CreateAnalyzeSession};
use astersql_session::testutil::TestRecordSet;

/// 执行无结果集的权限语句，并保留 SQL 上下文以便快速定位失败。
fn execute(session: &ConcreteSession, sql: &str) {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("privilege statement failed: {sql}: {error}"));
}

/// 执行查询并完整收集第一份结果集，用于直接断言权限系统表的持久化内容。
fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut result = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("privilege query failed: {sql}: {error}"))
        .remove(0);
    let mut rows = Vec::new();
    while let Some(row) = result
        .Next()
        .unwrap_or_else(|error| panic!("read privilege query row: {sql}: {error}"))
    {
        rows.push(row);
    }
    rows
}

/// 构造测试认证及权限校验所需的用户身份。
fn identity(user: &str, host: &str) -> UserIdentity {
    UserIdentity {
        username: user.to_owned(),
        hostname: host.to_owned(),
        ..Default::default()
    }
}

#[test]
fn exchange_partition_checks_both_tables_privileges() {
    let (domain, admin) = CreateAnalyzeSession().unwrap();
    for sql in [
        "create database exchange_src",
        "create database exchange_dst",
        "create table exchange_src.pt (a int primary key) partition by range(a) (partition p0 values less than(100), partition p1 values less than(200))",
        "create table exchange_dst.nt (a int primary key)",
        "insert into exchange_src.pt values (1),(150)",
        "insert into exchange_dst.nt values (2)",
        "create user 'exchange_low'@'%'",
    ] {
        execute(&admin, sql);
    }
    let mut low = ConcreteSession::new(Arc::clone(&domain));
    execute(&low, "use exchange_dst");
    let exchange = "alter table exchange_src.pt exchange partition p0 with table exchange_dst.nt";
    for (grant, command, table) in [
        (None, "ALTER", "pt"),
        (
            Some("grant alter on exchange_src.* to 'exchange_low'@'%'"),
            "DROP",
            "pt",
        ),
        (
            Some("grant drop on exchange_src.* to 'exchange_low'@'%'"),
            "CREATE",
            "nt",
        ),
        (
            Some("grant create on exchange_dst.* to 'exchange_low'@'%'"),
            "INSERT",
            "nt",
        ),
        (
            Some("grant insert on exchange_dst.* to 'exchange_low'@'%'"),
            "INSERT",
            "pt",
        ),
        (
            Some("grant insert on exchange_src.* to 'exchange_low'@'%'"),
            "CREATE",
            "pt",
        ),
        (
            Some("grant create on exchange_src.* to 'exchange_low'@'%'"),
            "ALTER",
            "nt",
        ),
        (
            Some("grant alter on exchange_dst.* to 'exchange_low'@'%'"),
            "DROP",
            "nt",
        ),
    ] {
        if let Some(grant) = grant {
            execute(&admin, grant);
        }
        low.AuthenticateUserForTest(&identity("exchange_low", "%"))
            .unwrap();
        let error = low
            .execute(exchange)
            .err()
            .expect("missing privilege must deny exchange")
            .to_string();
        assert!(error.contains("1142"), "{error}");
        assert!(
            error.contains(&format!("{command} command denied")),
            "{error}"
        );
        assert!(error.contains(&format!("'{table}'")), "{error}");
        assert_eq!(
            rows(&admin, "select * from exchange_src.pt order by a"),
            vec![vec!["1".to_owned()], vec!["150".to_owned()]]
        );
        assert_eq!(
            rows(&admin, "select * from exchange_dst.nt"),
            vec![vec!["2".to_owned()]]
        );
    }
    // Go's successful exchange case uses empty tables. Keep nonempty rows above
    // to prove denied statements have no side effects, then verify the allowed
    // exchange through the real physical metadata change.
    execute(&admin, "delete from exchange_src.pt");
    execute(&admin, "delete from exchange_dst.nt");
    let old_normal = domain.stats_table("exchange_dst", "nt").unwrap().1.ID;
    execute(&admin, "grant drop on exchange_dst.* to 'exchange_low'@'%'");
    low.AuthenticateUserForTest(&identity("exchange_low", "%"))
        .unwrap();
    execute(
        &low,
        "alter table exchange_src.pt exchange partition p0 with table nt",
    );
    assert_eq!(
        rows(&admin, "select * from exchange_src.pt order by a"),
        Vec::<Vec<String>>::new()
    );
    assert_eq!(
        rows(&admin, "select * from exchange_dst.nt"),
        Vec::<Vec<String>>::new()
    );
    assert_eq!(
        domain
            .stats_table("exchange_src", "pt")
            .unwrap()
            .1
            .Partition
            .as_ref()
            .unwrap()
            .Definitions[0]
            .ID,
        old_normal
    );
}

#[test]
fn exchange_partition_rejects_reserved_system_table_target() {
    let (domain, admin) = CreateAnalyzeSession().unwrap();
    execute(
        &admin,
        "create table test.exchange_pt (a int primary key) partition by range(a) (partition p0 values less than(100))",
    );
    // Use persisted metadata with the same reserved ID as the actual job table.
    domain
        .ddl_create_table(
            "mysql",
            astersql_meta_model::TableInfo {
                ID: astersql_meta_metadef::TiDBDDLJobTableID,
                Name: astersql_parser_ast::NewCIStr("exchange_reserved"),
                ..Default::default()
            },
            false,
        )
        .unwrap();
    let before_partitioned = domain.stats_table("test", "exchange_pt").unwrap().1;
    let before_target = domain.stats_table("mysql", "exchange_reserved").unwrap().1;
    let error = admin
        .execute(
            "alter table test.exchange_pt exchange partition p0 with table mysql.exchange_reserved",
        )
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("[ddl:8267]"), "{error}");
    assert!(
        error.contains("Exchange partition on system table 'mysql.exchange_reserved'"),
        "{error}"
    );
    assert_eq!(
        domain.stats_table("test", "exchange_pt").unwrap().1.ID,
        before_partitioned.ID
    );
    assert_eq!(
        domain
            .stats_table("test", "exchange_pt")
            .unwrap()
            .1
            .Partition
            .as_ref()
            .unwrap()
            .Definitions
            .iter()
            .map(|part| part.ID)
            .collect::<Vec<_>>(),
        before_partitioned
            .Partition
            .as_ref()
            .unwrap()
            .Definitions
            .iter()
            .map(|part| part.ID)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        domain
            .stats_table("mysql", "exchange_reserved")
            .unwrap()
            .1
            .ID,
        before_target.ID
    );
}

#[test]
fn alter_rename_checks_target_database_without_exchange_extras() {
    let (domain, admin) = CreateAnalyzeSession().unwrap();
    for sql in [
        "create database rename_src",
        "create database rename_dst",
        "create table rename_src.pt (a int)",
        "insert into rename_src.pt values(7)",
        "create user 'rename_low'@'%'",
        "grant alter, drop on rename_src.* to 'rename_low'@'%'",
    ] {
        execute(&admin, sql);
    }
    let mut low = ConcreteSession::new(Arc::clone(&domain));
    low.AuthenticateUserForTest(&identity("rename_low", "%"))
        .unwrap();
    execute(&low, "use rename_dst");
    let sql = "alter table rename_src.pt rename to nt";
    let error = low
        .execute(sql)
        .err()
        .expect("target CREATE missing")
        .to_string();
    assert!(error.contains("CREATE command denied"), "{error}");
    execute(
        &admin,
        "grant create, insert on rename_dst.* to 'rename_low'@'%'",
    );
    low.AuthenticateUserForTest(&identity("rename_low", "%"))
        .unwrap();
    execute(&low, sql);
    assert_eq!(
        rows(&admin, "select * from rename_dst.nt"),
        vec![vec!["7".to_owned()]]
    );
}

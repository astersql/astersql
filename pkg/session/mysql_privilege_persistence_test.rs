// Copyright 2026 AsterSQL.

// MySQL 权限系统表持久化的集成测试。
//
// 覆盖账户及全局、库、表、列四级权限在授权、撤权和删除账户后的存储状态，
// 同时验证新会话读取持久化权限，以及失败的账户 DDL/授权事务不会污染系统表与权限缓存。

use std::sync::Arc;

use astersql_parser_auth::parser::auth::auth::UserIdentity;

use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

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
// 验证权限变更会同步落入对应层级的系统表，并能被新建会话读取。
fn mysql_privilege_tables_follow_grant_revoke_lifecycle() {
    let (domain, admin) = CreateAnalyzeSession().expect("canonical privilege session");
    execute(&admin, "create database privilege_lifecycle");
    execute(&admin, "use privilege_lifecycle");
    execute(
        &admin,
        "create table protected_rows (id int primary key, visible varchar(16), secret varchar(16))",
    );
    execute(
        &admin,
        "create user 'persistent_user'@'localhost' identified by 's3cret'",
    );

    execute(
        &admin,
        "grant process on *.* to 'persistent_user'@'localhost'",
    );
    execute(
        &admin,
        "grant select on privilege_lifecycle.* to 'persistent_user'@'localhost'",
    );
    execute(
        &admin,
        "grant insert on privilege_lifecycle.protected_rows to 'persistent_user'@'localhost'",
    );
    execute(
        &admin,
        "grant update (visible) on privilege_lifecycle.protected_rows to 'persistent_user'@'localhost'",
    );
    // 重复授权必须合并到已有持久化记录，不能产生重复行。
    execute(
        &admin,
        "grant insert on privilege_lifecycle.protected_rows to 'persistent_user'@'localhost'",
    );

    assert_eq!(
        rows(
            &admin,
            "select Process_priv from mysql.user where User='persistent_user' and Host='localhost'",
        ),
        vec![vec!["Y".to_owned()]],
    );
    assert_eq!(
        rows(
            &admin,
            "select DB, Select_priv from mysql.db where User='persistent_user' and Host='localhost'",
        ),
        vec![vec!["privilege_lifecycle".to_owned(), "Y".to_owned()]],
    );
    assert_eq!(
        rows(
            &admin,
            "select DB, Table_name, Table_priv from mysql.tables_priv where User='persistent_user' and Host='localhost'",
        ),
        vec![vec![
            "privilege_lifecycle".to_owned(),
            "protected_rows".to_owned(),
            "Insert".to_owned(),
        ]],
    );
    assert_eq!(
        rows(
            &admin,
            "select DB, Table_name, Column_name, Column_priv from mysql.columns_priv where User='persistent_user' and Host='localhost'",
        ),
        vec![vec![
            "privilege_lifecycle".to_owned(),
            "protected_rows".to_owned(),
            "visible".to_owned(),
            "Update".to_owned(),
        ]],
    );

    // 复用同一 domain 创建新会话，确认认证读取的是已发布的持久化账户状态。
    let mut reconnect = ConcreteSession::new(Arc::clone(&domain));
    reconnect
        .AuthenticateUserForTest(&identity("persistent_user", "localhost"))
        .expect("persisted user authenticates in a fresh session");

    // 撤销各层级最后一项权限后，除 mysql.user 的否定标记外，其余权限行应被清理。
    execute(
        &admin,
        "revoke process on *.* from 'persistent_user'@'localhost'",
    );
    execute(
        &admin,
        "revoke select on privilege_lifecycle.* from 'persistent_user'@'localhost'",
    );
    execute(
        &admin,
        "revoke insert on privilege_lifecycle.protected_rows from 'persistent_user'@'localhost'",
    );
    execute(
        &admin,
        "revoke update (visible) on privilege_lifecycle.protected_rows from 'persistent_user'@'localhost'",
    );

    assert_eq!(
        rows(
            &admin,
            "select Process_priv from mysql.user where User='persistent_user' and Host='localhost'",
        ),
        vec![vec!["N".to_owned()]],
    );
    assert!(
        rows(
            &admin,
            "select * from mysql.db where User='persistent_user'"
        )
        .is_empty()
    );
    assert!(
        rows(
            &admin,
            "select * from mysql.tables_priv where User='persistent_user'"
        )
        .is_empty()
    );
    assert!(
        rows(
            &admin,
            "select * from mysql.columns_priv where User='persistent_user'"
        )
        .is_empty()
    );

    // 删除账户还必须同步失效权限缓存，后续新会话不能继续认证。
    execute(&admin, "drop user 'persistent_user'@'localhost'");
    let mut disconnected = ConcreteSession::new(Arc::clone(&domain));
    assert!(
        disconnected
            .AuthenticateUserForTest(&identity("persistent_user", "localhost"))
            .is_err(),
        "DROP USER must invalidate reconnect authentication",
    );
}

#[test]
// 验证账户批处理或权限提交失败时，系统表与内存权限缓存保持一致回滚。
fn mysql_privilege_failed_account_batch_rolls_back_tables_and_cache() {
    let (domain, admin) = CreateAnalyzeSession().expect("canonical privilege rollback session");
    execute(&admin, "create database privilege_rollback");
    execute(&admin, "create user 'already_there'@'localhost'");

    // 批处理中任一账户已存在时，先前尚可创建的账户也不得对外可见。
    let error = match admin
        .execute("create user 'must_rollback'@'localhost', 'already_there'@'localhost'")
    {
        Ok(_) => panic!("duplicate account must fail the whole CREATE USER batch"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("already exists"));
    assert!(
        rows(
            &admin,
            "select User from mysql.user where User='must_rollback' and Host='localhost'",
        )
        .is_empty(),
        "failed account DDL must not publish a mysql.user row",
    );

    let mut reconnect = ConcreteSession::new(Arc::clone(&domain));
    assert!(
        reconnect
            .AuthenticateUserForTest(&identity("must_rollback", "localhost"))
            .is_err(),
        "failed account DDL must not publish a cache record",
    );

    // 在持久化提交点注入失败，检查表记录与缓存增量都未发布。
    admin.InjectNextDmlCommitError("injected privilege commit failure");
    let grant_error = match admin
        .execute("grant select on privilege_rollback.* to 'already_there'@'localhost'")
    {
        Ok(_) => panic!("injected commit failure must reject GRANT"),
        Err(error) => error,
    };
    assert!(
        grant_error
            .to_string()
            .contains("injected privilege commit failure")
    );
    assert!(
        rows(
            &admin,
            "select * from mysql.db where User='already_there' and DB='privilege_rollback'",
        )
        .is_empty(),
        "failed GRANT commit must roll back mysql.db",
    );
    assert!(
        !admin.VerifyPrivilegeForTest(
            &identity("already_there", "localhost"),
            "privilege_rollback",
            "",
            "",
            astersql_privilege_privileges::SelectPriv,
        ),
        "failed GRANT commit must not publish the cache delta",
    );
}

#[test]
fn mysql_alter_user_preserves_require() {
    let (_, admin) = CreateAnalyzeSession().expect("canonical REQUIRE session");
    execute(
        &admin,
        "CREATE USER 'require_user'@'%' REQUIRE SUBJECT '/C=US/O=Example/CN=TiDB' SAN 'DNS:foo'",
    );
    let priv_sql = "SELECT Priv FROM mysql.global_priv WHERE User='require_user' AND Host='%'";
    let priv_value = r#"{"ssl_type":3,"x509_subject":"/C=US/O=Example/CN=TiDB","san":"DNS:foo"}"#;
    assert_eq!(rows(&admin, priv_sql), vec![vec![priv_value.to_owned()]]);

    execute(&admin, "ALTER USER 'require_user'@'%' ACCOUNT LOCK");
    assert_eq!(rows(&admin, priv_sql), vec![vec![priv_value.to_owned()]]);
    assert_eq!(
        rows(
            &admin,
            "SELECT Account_locked FROM mysql.user WHERE User='require_user' AND Host='%'"
        ),
        vec![vec!["Y".to_owned()]],
    );
    assert_eq!(
        rows(&admin, "SHOW CREATE USER 'require_user'@'%'") ,
        vec![vec!["CREATE USER `require_user`@`%` IDENTIFIED WITH 'mysql_native_password' AS '' REQUIRE SUBJECT '/C=US/O=Example/CN=TiDB' SAN 'DNS:foo' PASSWORD EXPIRE DEFAULT ACCOUNT LOCK PASSWORD HISTORY DEFAULT PASSWORD REUSE INTERVAL DEFAULT".to_owned()]],
    );

    for sql in [
        "ALTER USER 'require_user'@'%' ACCOUNT UNLOCK",
        "ALTER USER 'require_user'@'%' PASSWORD EXPIRE",
        "ALTER USER 'require_user'@'%' COMMENT ''",
    ] {
        execute(&admin, sql);
        assert_eq!(
            rows(&admin, priv_sql),
            vec![vec![priv_value.to_owned()]],
            "{sql}"
        );
    }
    assert_eq!(
        rows(
            &admin,
            "SELECT Account_locked, Password_expired FROM mysql.user WHERE User='require_user' AND Host='%'"
        ),
        vec![vec!["N".to_owned(), "Y".to_owned()]],
    );
    let attributes = rows(
        &admin,
        "SELECT User_attributes FROM mysql.user WHERE User='require_user' AND Host='%'",
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&attributes[0][0]).unwrap(),
        serde_json::json!({"metadata": {"comment": ""}})
    );
    execute(&admin, "ALTER USER 'require_user'@'%' REQUIRE SSL");
    assert_eq!(
        rows(&admin, priv_sql),
        vec![vec![r#"{"ssl_type":1}"#.to_owned()]]
    );
    execute(&admin, "ALTER USER 'require_user'@'%' REQUIRE NONE");
    assert_eq!(rows(&admin, priv_sql), vec![vec!["{}".to_owned()]]);

    execute(
        &admin,
        "CREATE USER 'token_only'@'%' IDENTIFIED WITH 'tidb_auth_token' REQUIRE token_issuer 'issuer-abc'",
    );
    let token_priv_sql =
        "SELECT count(*) FROM mysql.global_priv WHERE User='token_only' AND Host='%'";
    assert_eq!(rows(&admin, token_priv_sql), vec![vec!["0".to_owned()]]);
    execute(&admin, "ALTER USER 'token_only'@'%' ACCOUNT LOCK");
    assert_eq!(rows(&admin, token_priv_sql), vec![vec!["0".to_owned()]]);
    assert_eq!(
        rows(&admin, "SHOW CREATE USER 'token_only'@'%'") ,
        vec![vec!["CREATE USER `token_only`@`%` IDENTIFIED WITH 'tidb_auth_token' AS '' REQUIRE NONE token_issuer issuer-abc PASSWORD EXPIRE DEFAULT ACCOUNT LOCK PASSWORD HISTORY DEFAULT PASSWORD REUSE INTERVAL DEFAULT".to_owned()]],
    );
}

#[test]
fn mysql_alter_user_token_only_require_preserves_existing_tls() {
    let (_, admin) = CreateAnalyzeSession().expect("canonical token REQUIRE session");
    execute(
        &admin,
        "CREATE USER 'token_tls'@'%' IDENTIFIED WITH 'tidb_auth_token' REQUIRE SUBJECT '/C=US/O=Example/CN=TiDB' SAN 'DNS:foo' token_issuer 'issuer-old'",
    );
    let priv_sql = "SELECT Priv FROM mysql.global_priv WHERE User='token_tls' AND Host='%'";
    let expected = vec![vec![
        r#"{"ssl_type":3,"x509_subject":"/C=US/O=Example/CN=TiDB","san":"DNS:foo"}"#.to_owned(),
    ]];
    assert_eq!(rows(&admin, priv_sql), expected);
    execute(
        &admin,
        "ALTER USER 'token_tls'@'%' REQUIRE token_issuer 'issuer-new'",
    );
    assert_eq!(rows(&admin, priv_sql), expected);
    assert_eq!(
        rows(
            &admin,
            "SELECT Token_issuer FROM mysql.user WHERE User='token_tls' AND Host='%'"
        ),
        vec![vec!["issuer-new".to_owned()]]
    );
    assert!(
        rows(&admin, "SHOW CREATE USER 'token_tls'@'%'")[0][0].contains(
            "REQUIRE SUBJECT '/C=US/O=Example/CN=TiDB' SAN 'DNS:foo' token_issuer issuer-new"
        )
    );
}

#[test]
fn mysql_alter_user_require_mutations_are_atomic() {
    let (_, admin) = CreateAnalyzeSession().expect("canonical REQUIRE rollback session");
    execute(
        &admin,
        "CREATE USER 'require_atomic'@'%' REQUIRE SUBJECT '/C=US/O=Example/CN=TiDB' SAN 'DNS:foo'",
    );
    let priv_sql = "SELECT Priv FROM mysql.global_priv WHERE User='require_atomic' AND Host='%'";
    let original_priv = rows(&admin, priv_sql);
    let original_create = rows(&admin, "SHOW CREATE USER 'require_atomic'@'%'");
    let error = match admin
        .execute("ALTER USER 'require_atomic'@'%', 'missing_require'@'%' REQUIRE SSL ACCOUNT LOCK")
    {
        Ok(_) => panic!("missing account must roll back the whole ALTER USER batch"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("Operation ALTER USER failed for 'missing_require'@'%'"),
        "{error}"
    );
    assert_eq!(rows(&admin, priv_sql), original_priv);
    assert_eq!(
        rows(&admin, "SHOW CREATE USER 'require_atomic'@'%'"),
        original_create
    );
    assert_eq!(
        rows(
            &admin,
            "SELECT Account_locked FROM mysql.user WHERE User='require_atomic' AND Host='%'"
        ),
        vec![vec!["N".to_owned()]]
    );

    admin.InjectNextDmlCommitError("injected REQUIRE commit failure");
    let error = match admin.execute("ALTER USER 'require_atomic'@'%' REQUIRE SSL ACCOUNT LOCK") {
        Ok(_) => panic!("commit failure must not publish TLS or account cache changes"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("injected REQUIRE commit failure"),
        "{error}"
    );
    assert_eq!(rows(&admin, priv_sql), original_priv);
    assert_eq!(
        rows(&admin, "SHOW CREATE USER 'require_atomic'@'%'"),
        original_create
    );
    assert_eq!(
        rows(
            &admin,
            "SELECT Account_locked FROM mysql.user WHERE User='require_atomic' AND Host='%'"
        ),
        vec![vec!["N".to_owned()]]
    );

    execute(
        &admin,
        "ALTER USER IF EXISTS 'require_atomic'@'%', 'missing_require'@'%' REQUIRE SSL ACCOUNT LOCK",
    );
    assert_eq!(
        rows(&admin, priv_sql),
        vec![vec![r#"{"ssl_type":1}"#.to_owned()]]
    );
    assert_eq!(
        rows(
            &admin,
            "SELECT Account_locked FROM mysql.user WHERE User='require_atomic' AND Host='%'"
        ),
        vec![vec!["Y".to_owned()]]
    );
}

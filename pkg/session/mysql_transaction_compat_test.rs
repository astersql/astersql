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

// MySQL 常见事务行为的端到端兼容性测试。
//
// 测试通过共享同一 domain 的两个真实会话交叉观察数据，覆盖事务可见性、保存点、
// 语句失败原子性、DDL 隐式提交、自动提交切换，以及连接重置和断开时的回滚语义。

use std::sync::Arc;

use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

const SERVER_STATUS_IN_TRANS: u16 = 0x0001;
const SERVER_STATUS_AUTOCOMMIT: u16 = 0x0002;

// 执行不需要读取结果的语句，并在失败信息中保留 SQL 便于定位场景。
fn execute(session: &ConcreteSession, sql: &str) {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("transaction statement failed: {sql}: {error}"));
}

// 完整拉取查询结果，避免只验证首批记录而漏掉后续行。
fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut result = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("transaction query failed: {sql}: {error}"))
        .remove(0);
    let mut rows = Vec::new();
    while let Some(row) = result
        .Next()
        .unwrap_or_else(|error| panic!("read transaction query row: {sql}: {error}"))
    {
        rows.push(row);
    }
    rows
}

// 同时核对 MySQL 协议状态位，确保事务内部状态能正确暴露给客户端。
fn assert_status(session: &ConcreteSession, context: &str, in_transaction: bool, autocommit: bool) {
    let status = session.protocol_state().status;
    assert_eq!(
        status & SERVER_STATUS_IN_TRANS != 0,
        in_transaction,
        "SERVER_STATUS_IN_TRANS after {context} in {status:#06x}"
    );
    assert_eq!(
        status & SERVER_STATUS_AUTOCOMMIT != 0,
        autocommit,
        "SERVER_STATUS_AUTOCOMMIT after {context} in {status:#06x}"
    );
}

#[test]
fn common_mysql_transactions_preserve_visibility_savepoints_and_status() {
    let (domain, first) = CreateAnalyzeSession().expect("first real session");
    let second = ConcreteSession::new(Arc::clone(&domain));
    execute(&first, "create database transaction_compat");
    execute(&first, "use transaction_compat");
    execute(&second, "use transaction_compat");
    execute(
        &first,
        "create table transaction_rows (id int primary key, value varchar(16))",
    );

    // 关闭自动提交后的首条 DML 隐式开启事务：本会话可见，其他会话仅在提交后可见。
    assert_status(&first, "DDL", false, true);
    execute(&first, "set autocommit = 0");
    assert_status(&first, "SET autocommit=0", false, false);
    execute(
        &first,
        "insert into transaction_rows values (1, 'implicit')",
    );
    assert_status(&first, "implicit DML", true, false);
    assert_eq!(
        rows(&first, "select id, value from transaction_rows order by id"),
        vec![vec!["1".to_owned(), "implicit".to_owned()]]
    );
    assert!(rows(&second, "select * from transaction_rows").is_empty());
    execute(&first, "commit");
    assert_status(&first, "COMMIT", false, false);
    assert_eq!(
        rows(&second, "select id, value from transaction_rows"),
        vec![vec!["1".to_owned(), "implicit".to_owned()]]
    );

    // 回滚到保存点只撤销保存点后的写入，并保留之前的事务修改。
    execute(&first, "start transaction");
    assert_status(&first, "START TRANSACTION", true, false);
    execute(&first, "insert into transaction_rows values (2, 'before')");
    execute(&first, "savepoint keep_before");
    execute(&first, "insert into transaction_rows values (3, 'after')");
    execute(&first, "rollback to savepoint keep_before");
    execute(&first, "release savepoint keep_before");
    assert_eq!(
        rows(&first, "select id from transaction_rows order by id"),
        vec![vec!["1".to_owned()], vec!["2".to_owned()]]
    );
    execute(&first, "commit");
    assert_eq!(
        rows(&second, "select id from transaction_rows order by id"),
        vec![vec!["1".to_owned()], vec!["2".to_owned()]]
    );

    // 单条语句失败不能破坏同一事务中已成功的写入；最终回滚仍应丢弃全部未提交内容。
    execute(&first, "begin");
    execute(&first, "insert into transaction_rows values (4, 'kept')");
    let duplicate = first.execute("insert into transaction_rows values (4, 'duplicate')");
    assert!(
        duplicate.is_err(),
        "duplicate statement must fail atomically"
    );
    assert_eq!(
        rows(&first, "select value from transaction_rows where id = 4"),
        vec![vec!["kept".to_owned()]]
    );
    execute(&first, "rollback");
    assert!(
        rows(&second, "select * from transaction_rows where id = 4").is_empty(),
        "ROLLBACK must discard the successful statement before the error"
    );

    // DDL 会隐式提交此前的 DML，并在执行完成后结束当前事务。
    execute(&first, "begin");
    execute(
        &first,
        "insert into transaction_rows values (5, 'ddl-commit')",
    );
    execute(&first, "create table ddl_committed (id int primary key)");
    assert_status(&first, "implicit-commit DDL", false, false);
    assert_eq!(
        rows(&second, "select value from transaction_rows where id = 5"),
        vec![vec!["ddl-commit".to_owned()]],
        "DDL must implicitly commit the preceding transaction"
    );
    execute(&second, "insert into ddl_committed values (1)");

    // 恢复自动提交后，每条 DML 都应立即对其他会话可见。
    execute(&first, "set autocommit = 1");
    assert_status(&first, "SET autocommit=1", false, true);
    execute(
        &first,
        "insert into transaction_rows values (6, 'autocommit')",
    );
    assert_eq!(
        rows(&second, "select value from transaction_rows where id = 6"),
        vec![vec!["autocommit".to_owned()]]
    );

    // 重置连接会回滚活动事务并恢复状态位，同时保留当前数据库选择。
    execute(&first, "begin");
    execute(&first, "insert into transaction_rows values (7, 'reset')");
    first.reset_connection().expect("reset real session");
    assert_status(&first, "COM_RESET_CONNECTION", false, true);
    assert_eq!(
        first.protocol_state().current_database,
        "transaction_compat"
    );
    assert!(rows(&second, "select * from transaction_rows where id = 7").is_empty());

    // 会话析构必须回滚未提交事务，不能把临时写入发布到共享存储。
    {
        let disconnected = ConcreteSession::new(Arc::clone(&domain));
        execute(&disconnected, "use transaction_compat");
        execute(&disconnected, "begin");
        execute(
            &disconnected,
            "insert into transaction_rows values (8, 'disconnect')",
        );
    }
    assert!(
        rows(&second, "select * from transaction_rows where id = 8").is_empty(),
        "dropping a session must not publish its uncommitted transaction"
    );
}

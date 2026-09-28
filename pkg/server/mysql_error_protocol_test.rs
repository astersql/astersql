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

// MySQL 错误包协议兼容性测试。
//
// 通过真实 MySQL 监听器执行语句，校验常见服务端错误在网络协议中仍保留
// MySQL errno、SQLSTATE 和可识别的错误信息，而非只验证内部错误类型。

use crate::mysql_compat_test_support::{
    CLIENT_DEPRECATE_EOF, ErrPacket, MysqlCompatServer, WireResponse,
};

/// 断言夹具语句在协议层返回 OK 包。
fn expect_ok(response: WireResponse, context: &str) {
    assert!(
        matches!(response, WireResponse::Ok(_)),
        "{context} did not return OK: {response:?}",
    );
}

/// 断言 ERR 包的编号、SQLSTATE 和消息片段均符合 MySQL 客户端预期。
fn expect_error(response: WireResponse, code: u16, sql_state: &[u8; 5], message_fragment: &str) {
    let WireResponse::Err(ErrPacket {
        code: actual_code,
        sql_state: actual_state,
        message,
    }) = response
    else {
        panic!("expected ERR {code}/{sql_state:?}, got {response:?}");
    };
    assert_eq!(actual_code, code, "wrong errno for {message}");
    assert_eq!(
        actual_state,
        Some(*sql_state),
        "wrong SQLSTATE for {message}"
    );
    assert!(
        message
            .to_ascii_lowercase()
            .contains(&message_fragment.to_ascii_lowercase()),
        "message {message:?} does not contain {message_fragment:?}",
    );
}

#[test]
/// 覆盖一组常见数据库、对象、约束和语法错误的网络协议表示。
fn common_mysql_errors_preserve_errno_sqlstate_and_message() {
    let server = MysqlCompatServer::start().expect("start real MySQL listener");
    let mut client = server
        .connect_root(CLIENT_DEPRECATE_EOF, None)
        .expect("connect to real MySQL listener");

    // Go's mock store bootstraps root with global privileges.  The Rust
    // real-listener fixture only bypasses root authentication, so materialize
    // the equivalent grants before testing errors through the privilege layer.
    for sql in [
        "create user if not exists 'root'@'%'",
        "grant all privileges on *.* to 'root'@'%'",
    ] {
        expect_ok(
            client
                .query(sql)
                .unwrap_or_else(|error| panic!("bootstrap root privilege {sql}: {error}")),
            sql,
        );
    }

    // 先在无当前数据库的状态下验证 unknown database，再建立后续错误场景共用的夹具。
    expect_error(
        client
            .query("use mysql_error_missing")
            .expect("unknown database"),
        1049,
        b"42000",
        "unknown database",
    );
    for sql in [
        "create database mysql_error_wire",
        "use mysql_error_wire",
        "create table parent (id bigint primary key)",
        "create table child (\
         id bigint primary key,\
         required_v varchar(16) not null,\
         parent_id bigint,\
         constraint fk_child_parent foreign key (parent_id) references parent(id))",
        "insert into parent values (1)",
        "insert into child values (1, 'first', 1)",
    ] {
        expect_ok(
            client
                .query(sql)
                .unwrap_or_else(|error| panic!("send fixture statement {sql}: {error}")),
            sql,
        );
    }

    // 每条失败语句同时锁定错误编号、SQLSTATE 和稳定的消息片段，防止协议映射退化。
    for (sql, code, state, fragment) in [
        (
            "select * from missing_table",
            1146,
            b"42S02",
            "missing_table",
        ),
        (
            "select missing_column from child",
            1054,
            b"42S22",
            "unknown column",
        ),
        (
            "create table child (id int)",
            1050,
            b"42S01",
            "already exists",
        ),
        (
            "insert into child values (1, 'again', 1)",
            1062,
            b"23000",
            "duplicate entry",
        ),
        (
            "insert into child values (2, null, 1)",
            1048,
            b"23000",
            "cannot be null",
        ),
        (
            "insert into child values (2, 'orphan', 999)",
            1452,
            b"23000",
            "foreign key constraint fails",
        ),
        (
            "rollback to savepoint absent",
            1305,
            b"42000",
            "does not exist",
        ),
        ("select from", 1064, b"42000", "syntax"),
    ] {
        expect_error(
            client
                .query(sql)
                .unwrap_or_else(|error| panic!("send failing statement {sql}: {error}")),
            code,
            state,
            fragment,
        );
    }
}

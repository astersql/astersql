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

// MySQL DML 兼容性集成测试。
//
// 通过真实会话覆盖常用 INSERT、UPDATE、DELETE 及其扩展语法，除结果行外还校验
// 原子性约束与协议层的受影响行数、自增 ID 和告警计数，防止执行器与会话状态脱节。

use crate::runtime::{
    ConcreteProtocolState, ConcreteRecordSet, ConcreteSession, CreateAnalyzeSession,
};
use crate::testutil::TestRecordSet;

// 将流式结果集完整物化，便于后续按行比较兼容性结果。
fn collect(mut result: ConcreteRecordSet) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    while let Some(row) = result.Next().expect("read DML compatibility row") {
        rows.push(row);
    }
    rows
}

// 执行预期成功的语句，并取得该语句完成后的协议状态快照。
fn execute(session: &ConcreteSession, sql: &str) -> ConcreteProtocolState {
    session
        .execute(sql)
        .unwrap_or_else(|error| panic!("DML statement failed: {sql}: {error}"));
    session.protocol_state()
}

// 执行预期失败的语句并返回错误文本，意外成功时立即终止测试。
fn execute_error(session: &ConcreteSession, sql: &str) -> String {
    match session.execute(sql) {
        Ok(_) => panic!("DML statement unexpectedly succeeded: {sql}"),
        Err(error) => error.to_string(),
    }
}

// 执行验证查询并读取其首个结果集。
fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let result = session
        .execute(sql)
        .unwrap_or_else(|error| panic!("DML verification query failed: {sql}: {error}"))
        .remove(0);
    collect(result)
}

// 集中校验客户端可见的三项语句状态，避免只验证表中数据而遗漏协议语义。
fn assert_state(
    state: &ConcreteProtocolState,
    affected_rows: u64,
    last_insert_id: u64,
    warning_count: u16,
) {
    assert_eq!(state.affected_rows, affected_rows, "{state:?}");
    assert_eq!(state.last_insert_id, last_insert_id, "{state:?}");
    assert_eq!(state.warning_count, warning_count, "{state:?}");
}

#[test]
// 覆盖普通表与会话 KV 表上的 DML 数据、约束、原子性及协议状态兼容性。
fn common_mysql_dml_preserves_rows_constraints_and_statement_state() {
    let (_domain, session) = CreateAnalyzeSession().expect("canonical DML session");
    execute(&session, "create database dml_compat");
    execute(&session, "use dml_compat");
    execute(
        &session,
        "create table items (\
         id bigint not null auto_increment,\
         code varchar(32) not null,\
         quantity int not null default 7,\
         note varchar(64) null default null,\
         primary key (id),\
         unique key uk_items_code (code))",
    );
    execute(
        &session,
        "create table item_copy (\
         id bigint not null auto_increment,\
         code varchar(32) not null,\
         quantity int not null default 7,\
         note varchar(64) null default null,\
         primary key (id),\
         unique key uk_item_copy_code (code))",
    );

    // 先覆盖默认值、NULL、多行插入和自增编号的基础行为。
    let state = execute(&session, "insert into items (code) values ('alpha')");
    assert_state(&state, 1, 1, 0);
    let state = execute(
        &session,
        "insert into items (code, quantity, note) values \
         ('beta', default, null), ('gamma', 9, 'ready')",
    );
    assert_state(&state, 2, 2, 0);
    assert_eq!(
        rows(
            &session,
            "select id, code, quantity, note from items order by id",
        ),
        vec![
            vec![
                "1".to_owned(),
                "alpha".to_owned(),
                "7".to_owned(),
                "<nil>".to_owned(),
            ],
            vec![
                "2".to_owned(),
                "beta".to_owned(),
                "7".to_owned(),
                "<nil>".to_owned(),
            ],
            vec![
                "3".to_owned(),
                "gamma".to_owned(),
                "9".to_owned(),
                "ready".to_owned(),
            ],
        ],
    );

    // 显式写入自增列不应覆盖会话中最近一次由分配器生成的 ID。
    let state = execute(
        &session,
        "insert into items (id, code, quantity) values (100, 'explicit', 5)",
    );
    assert_state(&state, 1, 0, 0);
    assert_eq!(
        rows(&session, "select last_insert_id()"),
        vec![vec!["2".to_owned()]],
        "an explicit AUTO_INCREMENT value must not change LAST_INSERT_ID()",
    );

    // INSERT ... SELECT 也必须报告首个分配的自增 ID，并保持源行顺序与默认值。
    let state = execute(
        &session,
        "insert into item_copy (code, quantity, note) \
         select code, quantity, note from items where id <= 2 order by id",
    );
    assert_state(&state, 2, 1, 0);
    assert_eq!(
        rows(
            &session,
            "select id, code, quantity, note from item_copy order by id",
        ),
        vec![
            vec![
                "1".to_owned(),
                "alpha".to_owned(),
                "7".to_owned(),
                "<nil>".to_owned(),
            ],
            vec![
                "2".to_owned(),
                "beta".to_owned(),
                "7".to_owned(),
                "<nil>".to_owned(),
            ],
        ],
    );

    // 多元组语句任一元组违反唯一键或非空约束时，先前元组不得部分可见。
    let error = execute_error(
        &session,
        "insert into items (code, quantity) values ('partial', 1), ('alpha', 99)",
    );
    assert!(error.contains("Duplicate entry"), "{error}");
    assert_state(&session.protocol_state(), 0, 0, 0);
    assert!(
        rows(&session, "select code from items where code = 'partial'").is_empty(),
        "a failed multi-row INSERT must not publish its earlier tuple",
    );

    let error = execute_error(
        &session,
        "insert into items (code, quantity) values ('not_null_partial', 1), (null, 2)",
    );
    assert!(error.contains("cannot be null"), "{error}");
    assert_state(&session.protocol_state(), 0, 0, 0);
    assert!(
        rows(
            &session,
            "select code from items where code = 'not_null_partial'",
        )
        .is_empty(),
        "a NOT NULL failure must roll back all tuples in the statement",
    );

    // IGNORE 跳过冲突元组，但仍需把冲突作为可查询的告警报告给客户端。
    let state = execute(
        &session,
        "insert ignore into items (code, quantity) values ('alpha', 100), ('delta', 4)",
    );
    assert_eq!(state.affected_rows, 1, "{state:?}");
    assert_eq!(state.warning_count, 1, "{state:?}");
    let warnings = rows(&session, "show warnings");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0][2].contains("Duplicate entry"), "{warnings:?}");

    // Upsert 与 REPLACE 除最终数据外，还具有 MySQL 特定的受影响行数语义。
    let state = execute(
        &session,
        "insert into items (code, quantity) values ('alpha', 11) \
         on duplicate key update quantity = values(quantity)",
    );
    assert_state(&state, 2, 1, 0);
    assert_eq!(
        rows(&session, "select quantity from items where code = 'alpha'",),
        vec![vec!["11".to_owned()]],
    );

    let state = execute(
        &session,
        "replace into items (code, quantity, note) values ('beta', 20, 'replaced')",
    );
    assert_eq!(state.affected_rows, 2, "{state:?}");
    assert!(state.last_insert_id > 3, "{state:?}");
    assert_eq!(
        rows(
            &session,
            "select code, quantity, note from items where code = 'beta'",
        ),
        vec![vec![
            "beta".to_owned(),
            "20".to_owned(),
            "replaced".to_owned(),
        ]],
    );

    // 有序 LIMIT 更新/删除只能影响选中的一行，未实际改变值的更新计为零行。
    let state = execute(
        &session,
        "update items set quantity = quantity + 1 \
         where quantity >= 9 order by id desc limit 1",
    );
    assert_state(&state, 1, 0, 0);
    assert_eq!(
        rows(
            &session,
            "select code, quantity from items where quantity >= 9 order by id",
        ),
        vec![
            vec!["alpha".to_owned(), "11".to_owned()],
            vec!["gamma".to_owned(), "9".to_owned()],
            vec!["beta".to_owned(), "21".to_owned()],
        ],
    );
    let state = execute(
        &session,
        "update items set quantity = quantity where code = 'alpha'",
    );
    assert_state(&state, 0, 0, 0);

    let state = execute(
        &session,
        "delete from items where quantity >= 9 order by id asc limit 1",
    );
    assert_state(&state, 1, 0, 0);
    assert!(
        rows(&session, "select code from items where code = 'alpha'").is_empty(),
        "ordered DELETE must remove only the first matching row",
    );

    // 在真实会话 KV 事务上复验原子性和冲突变体，避免兼容性只在普通表路径成立。
    let state = execute(
        &session,
        "insert into aster_session_kv(k, v) values ('seed', 'one'), ('other', 'two')",
    );
    assert_state(&state, 2, 0, 0);
    let error = execute_error(
        &session,
        "insert into aster_session_kv(k, v) values ('kv_partial', 'x'), ('seed', 'duplicate')",
    );
    assert!(error.contains("duplicate session KV key"), "{error}");
    assert!(
        rows(
            &session,
            "select v from aster_session_kv where k = 'kv_partial'",
        )
        .is_empty(),
        "the real KV transaction must remain atomic",
    );
    let state = execute(
        &session,
        "insert ignore into aster_session_kv(k, v) values \
         ('seed', 'ignored'), ('kv_inserted', 'three')",
    );
    assert_state(&state, 1, 0, 1);
    assert_eq!(
        rows(
            &session,
            "select v from aster_session_kv where k = 'kv_inserted'",
        ),
        vec![vec!["three".to_owned()]],
    );
    let state = execute(
        &session,
        "insert into aster_session_kv(k, v) values ('seed', 'upserted') \
         on duplicate key update v = values(v)",
    );
    assert_state(&state, 2, 0, 0);
    let state = execute(
        &session,
        "replace into aster_session_kv(k, v) values ('other', 'replaced')",
    );
    assert_state(&state, 2, 0, 0);
    assert_eq!(
        rows(&session, "select v from aster_session_kv where k = 'seed'"),
        vec![vec!["upserted".to_owned()]],
    );
}

#[test]
// 混合显式 ID 与自动分配 ID 时，协议应返回本语句首个实际分配的编号。
fn mixed_explicit_and_allocated_auto_increment_reports_first_allocated_id() {
    let (_domain, session) = CreateAnalyzeSession().expect("canonical DML session");
    execute(&session, "create database autoid_mixed_insert");
    execute(&session, "use autoid_mixed_insert");
    execute(
        &session,
        "create table t (id bigint primary key auto_increment, n int)",
    );

    let state = execute(
        &session,
        "insert into t (id, n) values (3000, -1), (null, -2)",
    );
    assert_state(&state, 2, 3001, 0);
    assert_eq!(
        rows(&session, "select id, n from t order by id"),
        vec![
            vec!["3000".to_owned(), "-1".to_owned()],
            vec!["3001".to_owned(), "-2".to_owned()],
        ],
    );
    assert_eq!(
        rows(&session, "select last_insert_id()"),
        vec![vec!["3001".to_owned()]],
    );
}

#[test]
fn bit_column_update_arithmetic_preserves_unsigned_values_and_text_columns() {
    let (_domain, session) = CreateAnalyzeSession().expect("BIT arithmetic session");
    execute(&session, "create database bit_arithmetic");
    execute(&session, "use bit_arithmetic");
    execute(
        &session,
        "create table bits (id int primary key, bits bit(64), result bigint unsigned, text_value varchar(20))",
    );
    execute(
        &session,
        "insert into bits values (1, 1, 0, '0xFF'), (2, 9007199254740993, 0, 'text'), (3, null, 0, 'null')",
    );
    execute(&session, "update bits set result=bits+1 where id=1");
    assert_eq!(
        rows(&session, "select result, text_value from bits where id=1"),
        vec![vec!["2", "0xFF"]]
    );
    execute(&session, "update bits set bits=bits+1 where id=1");
    execute(&session, "update bits set result=(bits)+1 where id=1");
    assert_eq!(
        rows(&session, "select result from bits where id=1"),
        vec![vec!["3"]]
    );
    execute(&session, "update bits set result=bits+1 where id=2");
    assert_eq!(
        rows(&session, "select result from bits where id=2"),
        vec![vec!["9007199254740994"]]
    );
    execute(&session, "update bits set result=bits+1 where id=3");
    assert_eq!(
        rows(&session, "select id from bits where result is null"),
        vec![vec!["3"]]
    );
}

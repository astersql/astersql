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

use crate::runtime::split_statement_sql;

#[test]
fn double_dash_requires_mysql_comment_whitespace() {
    assert_eq!(
        split_statement_sql("SELECT 1--2; SELECT 3"),
        vec!["SELECT 1--2".to_owned(), "SELECT 3".to_owned()]
    );
    assert_eq!(
        split_statement_sql("SELECT 1-- comment;\n; SELECT 3"),
        vec!["SELECT 1-- comment;".to_owned(), "SELECT 3".to_owned()]
    );
}

use crate::runtime::{ConcreteSession, CreateAnalyzeSession};

fn ru_session() -> ConcreteSession {
    let (_, session) = CreateAnalyzeSession().expect("create real session");
    for sql in [
        "create database if not exists test",
        "use test",
        "create table t_unistore_act_rows(a int, b int, index(a, b))",
        "insert into t_unistore_act_rows values (1, 0), (1, 0), (2, 0), (2, 1)",
        "analyze table t_unistore_act_rows",
    ] {
        session
            .execute(sql)
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
    session
}

#[test]
fn explain_ru_analyze_preserves_operator_rows() {
    let session = ru_session();
    for (sql, expected) in [
        ("select * from t_unistore_act_rows", vec!["4", "4"]),
        (
            "select * from t_unistore_act_rows where b > 0",
            vec!["1", "1", "4"],
        ),
    ] {
        let mut result = session
            .execute(&format!("explain analyze format='ru' {sql}"))
            .unwrap_or_else(|error| panic!("{sql}: {error}"))
            .remove(0);
        assert_eq!(
            result.columns(),
            [
                "id", "task", "actRows", "selfRU", "cumRU", "cumRU%", "detail"
            ]
        );
        for field in result.result_fields() {
            let field = field.as_ref().expect("RU has a concrete string schema");
            assert_eq!(field.db_name.L, "information_schema");
            assert_eq!(
                field.column.FieldType.GetType(),
                astersql_parser_mysql::r#type::TypeString
            );
            assert_eq!(
                field.column.FieldType.GetFlen(),
                astersql_parser_mysql::r#const::MaxBlobWidth as isize
            );
            assert_eq!(
                field.column.FieldType.GetFlag(),
                astersql_parser_mysql::r#type::UnsignedFlag
            );
            assert_eq!(field.column.FieldType.GetDecimal(), 0);
            let (charset, collation) = astersql_types::field::DefaultCharsetForType(
                astersql_parser_mysql::r#type::TypeString,
            );
            assert_eq!(field.column.FieldType.GetCharset(), charset);
            assert_eq!(field.column.FieldType.GetCollate(), collation);
        }
        let mut rows = Vec::new();
        while let Some(row) = result.next_row().expect("read real explain result") {
            rows.push(row);
        }
        assert_eq!(rows.len(), expected.len(), "{sql}");
        for (row, expected) in rows.iter().zip(expected) {
            assert_eq!(row.len(), 7);
            assert!(!row[0].is_empty());
            assert!(!row[1].is_empty());
            assert_eq!(row[2], expected, "{sql}");
            assert!(row[3..].iter().all(String::is_empty));
        }
    }
}

#[test]
fn explain_ru_requires_analyze() {
    let session = ru_session();
    for format in ["'ru'", "ru", "'RU'"] {
        let error = session
            .execute(&format!(
                "explain format={format} select * from t_unistore_act_rows"
            ))
            .err()
            .expect("RU requires ANALYZE");
        assert!(error.to_string().contains("'explain format=ru' cannot work without 'analyze', please use 'explain analyze format=ru'"), "{error}");
    }
}

#[test]
fn explain_ru_context_bypasses_plan_cache() {
    let session = ru_session();
    session
        .execute("set @@session.tidb_enable_non_prepared_plan_cache = 1")
        .unwrap();
    session
        .execute("select * from t_unistore_act_rows")
        .unwrap();
    session
        .execute("explain analyze format='ru' select * from t_unistore_act_rows")
        .unwrap();
    session.WithSessionVars(|vars| {
        assert!(vars.StmtCtx.IsInExplainStmt());
        assert_eq!(vars.StmtCtx.ExplainFormatValue(), "ru");
    });
    let mut result = session
        .execute("select @@last_plan_from_cache")
        .unwrap()
        .remove(0);
    assert_eq!(result.next_row().unwrap().unwrap(), ["0"]);
}

#[test]
fn pg_core_dialect_prepared_execution() {
    use crate::runtime::{ConcretePreparedArgument, PARSED_DIALECTS};
    use astersql_parser_ast::SqlDialect;
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    mysql
        .execute("CREATE TABLE test.pg_prepared_identity_t3 (id BIGINT PRIMARY KEY)")
        .unwrap();
    mysql
        .execute("INSERT INTO test.pg_prepared_identity_t3 VALUES (1), (2)")
        .unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), SqlDialect::PostgreSQL);
    let other = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    let mut observations = Vec::new();
    for (session, dialect) in [
        (&pg, SqlDialect::PostgreSQL),
        (&mysql, SqlDialect::MySQL),
        (&other, SqlDialect::PostgreSQL),
    ] {
        PARSED_DIALECTS.with(|seen| seen.borrow_mut().clear());
        let (first, count, _) = session
            .prepare_protocol_statement("SELECT ? AS prepared_identity")
            .unwrap();
        assert_eq!(count, 1);
        let (second, _, _) = session
            .prepare_protocol_statement("SELECT ? AS prepared_other")
            .unwrap();
        observations.push((dialect, PARSED_DIALECTS.with(|seen| seen.borrow().clone())));
        for (id, value) in [(first, 17), (second, 23), (first, 31), (second, 47)] {
            PARSED_DIALECTS.with(|seen| seen.borrow_mut().clear());
            let mut sets = session
                .execute_protocol_statement(id, &[ConcretePreparedArgument::Signed(value)])
                .unwrap();
            assert_eq!(
                sets[0].next_row().unwrap().unwrap(),
                vec![value.to_string()]
            );
            assert_eq!(sets[0].next_row().unwrap(), None);
            sets[0].close().unwrap();
            observations.push((dialect, PARSED_DIALECTS.with(|seen| seen.borrow().clone())));
        }
        session.close_protocol_statement(first).unwrap();
        assert!(
            session
                .execute_protocol_statement(first, &[ConcretePreparedArgument::Signed(1)])
                .is_err()
        );
        assert!(session.execute_protocol_statement(second, &[]).is_err());
        assert!(session.prepare_protocol_statement("SELECT FROM").is_err());
        assert_eq!(
            session
                .execute_protocol_statement(second, &[ConcretePreparedArgument::Signed(59)])
                .unwrap()[0]
                .next_row()
                .unwrap()
                .unwrap(),
            vec!["59"]
        );
        session.close_protocol_statement(second).unwrap();
        // The SQL PREPARE path is a separate production consumer.
        PARSED_DIALECTS.with(|seen| seen.borrow_mut().clear());
        session
            .execute("PREPARE identity_stmt FROM 'SELECT ?'")
            .unwrap();
        observations.push((dialect, PARSED_DIALECTS.with(|seen| seen.borrow().clone())));
        for value in [61, -67] {
            session
                .execute(&format!("SET @identity_arg = {value}"))
                .unwrap();
            PARSED_DIALECTS.with(|seen| seen.borrow_mut().clear());
            let mut sets = session
                .execute("EXECUTE identity_stmt USING @identity_arg")
                .unwrap();
            assert_eq!(
                sets[0].next_row().unwrap().unwrap(),
                vec![value.to_string()]
            );
            observations.push((dialect, PARSED_DIALECTS.with(|seen| seen.borrow().clone())));
        }
        session.execute("DEALLOCATE PREPARE identity_stmt").unwrap();
        PARSED_DIALECTS.with(|seen| seen.borrow_mut().clear());
        session.execute("PREPARE identity_table_stmt FROM 'SELECT id FROM test.pg_prepared_identity_t3 WHERE id = ?'").unwrap();
        observations.push((dialect, PARSED_DIALECTS.with(|seen| seen.borrow().clone())));
        for value in [1, 2, 1] {
            session
                .execute(&format!("SET @identity_arg = {value}"))
                .unwrap();
            PARSED_DIALECTS.with(|seen| seen.borrow_mut().clear());
            let mut sets = session
                .execute("EXECUTE identity_table_stmt USING @identity_arg")
                .unwrap();
            assert_eq!(
                sets[0].next_row().unwrap().unwrap(),
                vec![value.to_string()]
            );
            assert_eq!(sets[0].next_row().unwrap(), None);
            sets[0].close().unwrap();
            observations.push((dialect, PARSED_DIALECTS.with(|seen| seen.borrow().clone())));
        }
        session
            .execute("DEALLOCATE PREPARE identity_table_stmt")
            .unwrap();
        assert_eq!(session.sql_dialect(), dialect);
    }
    let (removed, _, _) = pg.prepare_protocol_statement("SELECT ?").unwrap();
    let (survivor, _, _) = other.prepare_protocol_statement("SELECT ?").unwrap();
    pg.reset_connection().unwrap();
    assert_eq!(pg.sql_dialect(), SqlDialect::PostgreSQL);
    assert!(
        pg.execute_protocol_statement(removed, &[ConcretePreparedArgument::Signed(71)])
            .is_err()
    );
    assert_eq!(
        other
            .execute_protocol_statement(survivor, &[ConcretePreparedArgument::Signed(73)])
            .unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        vec!["73"]
    );
    other.close_protocol_statement(survivor).unwrap();
    PARSED_DIALECTS.with(|seen| seen.borrow_mut().clear());
    let (fresh, _, _) = pg.prepare_protocol_statement("SELECT ?").unwrap();
    assert_eq!(
        pg.execute_protocol_statement(fresh, &[ConcretePreparedArgument::Signed(79)])
            .unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        vec!["79"]
    );
    observations.push((
        SqlDialect::PostgreSQL,
        PARSED_DIALECTS.with(|seen| seen.borrow().clone()),
    ));
    pg.close_protocol_statement(fresh).unwrap();
    for (expected, seen) in observations {
        assert!(!seen.is_empty());
        assert!(
            seen.iter().all(|actual| *actual == expected),
            "expected {expected:?}, actual parser identities {seen:?}"
        );
    }
}

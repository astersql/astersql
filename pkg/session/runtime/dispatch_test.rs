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

#[test]
fn pg_core_integer_result_metadata_negative_parameters() {
    use crate::runtime::ConcretePreparedArgument;
    let (domain, _) = crate::runtime::CreateAnalyzeSession().unwrap();
    let session = crate::runtime::ConcreteSession::new_with_dialect(
        domain,
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let (statement, count, fields) = session
        .prepare_protocol_statement("SELECT ?::int4 AS v4, ?::int8 AS v8")
        .unwrap();
    assert_eq!(count, 2);
    assert_eq!(fields.len(), 2);
    for value in [i64::MIN, -9007199254740993, -17, 31] {
        let mut sets = session
            .execute_protocol_statement(
                statement,
                &[
                    ConcretePreparedArgument::Signed(-17),
                    ConcretePreparedArgument::Signed(value),
                ],
            )
            .unwrap();
        assert_eq!(
            sets[0].next_row().unwrap().unwrap(),
            ["-17".to_owned(), value.to_string()]
        );
        assert!(sets[0].next_row().unwrap().is_none());
        sets[0].close().unwrap();
    }
    session.close_protocol_statement(statement).unwrap();
}

#[test]
fn pg_dialect_04_returning_metadata() {
    use astersql_parser_ast::SqlDialect;
    use astersql_parser_mysql::r#type::{TypeLong, TypeVarchar};
    let (domain, session) = CreateAnalyzeSession().unwrap();
    session
        .execute("CREATE DATABASE returning_metadata")
        .unwrap();
    session.execute("USE returning_metadata").unwrap();
    session
        .execute("CREATE TABLE t(id INT, v VARCHAR(20))")
        .unwrap();
    let session = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    session.execute("USE returning_metadata").unwrap();
    let sql = "INSERT INTO t(id) VALUES (?) RETURNING id, id+1 AS next_id, t.*";
    let (id, params, fields) = session.prepare_protocol_statement(sql).unwrap();
    assert_eq!(params, 1);
    assert_eq!(
        fields
            .iter()
            .map(|f| f.column_as_name.O.as_str())
            .collect::<Vec<_>>(),
        ["id", "next_id", "id", "v"]
    );
    assert_eq!(
        fields
            .iter()
            .map(|f| f.column.FieldType.GetType())
            .collect::<Vec<_>>(),
        [TypeLong, TypeLong, TypeLong, TypeVarchar]
    );
    assert_eq!(session.protocol_integer_parameter_types(sql).unwrap(), [23]);
    assert_eq!(session.describe_result_fields(sql).unwrap().len(), 4);
    let mut returned = session
        .execute_protocol_statement(id, &[crate::runtime::ConcretePreparedArgument::Signed(7)])
        .unwrap()
        .remove(0);
    assert_eq!(
        returned.next_row().unwrap().unwrap(),
        ["7", "8", "7", "<nil>"]
    );
    assert!(returned.next_row().unwrap().is_none());
    session.close_protocol_statement(id).unwrap();
    assert_eq!(
        session.execute("SELECT id FROM t").unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        ["7"]
    );
    for sql in [
        "INSERT INTO t(id) VALUES (1) RETURNING missing+1",
        "INSERT INTO t(id) VALUES (1) RETURNING other.*",
    ] {
        assert!(session.prepare_protocol_statement(sql).is_err(), "{sql}");
    }
    session.execute("DROP DATABASE returning_metadata").unwrap();
}

#[test]
fn pg_dialect_05_update_returning_metadata() {
    use astersql_parser_ast::SqlDialect;
    use astersql_parser_mysql::r#type::{TypeLong, TypeVarchar};
    let (domain, mysql_session) = CreateAnalyzeSession().unwrap();
    mysql_session
        .execute("CREATE DATABASE update_returning_metadata")
        .unwrap();
    mysql_session
        .execute("USE update_returning_metadata")
        .unwrap();
    mysql_session
        .execute("CREATE TABLE t(id INT, v VARCHAR(20))")
        .unwrap();
    mysql_session
        .execute("INSERT INTO t VALUES (1, 'one'), (2, 'two')")
        .unwrap();
    let session = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    session.execute("USE update_returning_metadata").unwrap();

    let sql = "UPDATE t AS a SET id=? WHERE id=1 RETURNING a.id AS \"ID\", a.*";
    let (id, params, fields) = session.prepare_protocol_statement(sql).unwrap();
    assert_eq!(params, 1);
    assert_eq!(
        fields
            .iter()
            .map(|field| field.column_as_name.O.as_str())
            .collect::<Vec<_>>(),
        ["ID", "id", "v"]
    );
    assert_eq!(
        fields
            .iter()
            .map(|field| field.column.FieldType.GetType())
            .collect::<Vec<_>>(),
        [TypeLong, TypeLong, TypeVarchar]
    );
    assert_eq!(session.protocol_integer_parameter_types(sql).unwrap(), [23]);
    assert_eq!(session.describe_result_fields(sql).unwrap().len(), 3);
    session.close_protocol_statement(id).unwrap();

    let zero_rows = "UPDATE t AS a SET id=$1 WHERE FALSE RETURNING a.id";
    assert_eq!(session.describe_result_fields(zero_rows).unwrap().len(), 1);
    let mut rows = session.execute("SELECT id FROM t ORDER BY id").unwrap();
    let mut rows = rows.remove(0);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["1".to_owned()]);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["2".to_owned()]);
    assert!(rows.next_row().unwrap().is_none());
    rows.close().unwrap();
    session
        .execute("DROP DATABASE update_returning_metadata")
        .unwrap();
}

#[test]
fn pg10_01_default_arithmetic() {
    use astersql_parser_ast::SqlDialect;
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    mysql.execute("CREATE DATABASE pg10_01_default").unwrap();
    mysql.execute("USE pg10_01_default").unwrap();
    assert!(
        mysql
            .execute("CREATE TABLE mysql_gate(id INT DEFAULT (40 + 2))")
            .is_err()
    );
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    pg.execute("USE pg10_01_default").unwrap();
    pg.execute("CREATE TABLE t(id integer PRIMARY KEY DEFAULT (40 + 2), v integer, note text)")
        .unwrap();
    pg.execute("INSERT INTO t(v,note) VALUES (7,NULL)").unwrap();
    let mut rows = pg.execute("SELECT id,v,note FROM t").unwrap().remove(0);
    assert_eq!(rows.next_row().unwrap().unwrap()[..2], ["42", "7"]);
    pg.execute("DELETE FROM t").unwrap();
    pg.execute("INSERT INTO t(v,note) VALUES (8,NULL)").unwrap();
    assert_eq!(
        pg.execute("SELECT id FROM t").unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        ["42"]
    );
    pg.execute("INSERT INTO t VALUES (43,9,NULL)").unwrap();
    assert!(pg.execute("INSERT INTO t VALUES (NULL,10,NULL)").is_err());
    pg.execute("CREATE TABLE ops(id integer PRIMARY KEY DEFAULT ((40 + 2) * 2 - 1),v integer)")
        .unwrap();
    pg.execute("INSERT INTO ops(v) VALUES (7)").unwrap();
    assert_eq!(
        pg.execute("SELECT id FROM ops").unwrap()[0]
            .next_row()
            .unwrap()
            .unwrap(),
        ["83"]
    );
    pg.execute("CREATE TABLE bad(id integer PRIMARY KEY DEFAULT (2147483647 + 1),v integer)")
        .unwrap();
    assert!(pg.execute("INSERT INTO bad(v) VALUES (7)").is_err());
    assert!(
        pg.execute("SELECT * FROM bad").unwrap()[0]
            .next_row()
            .unwrap()
            .is_none()
    );
    pg.execute("DROP DATABASE pg10_01_default").unwrap();
}

#[test]
fn pg10_01_returning_rows_and_atomicity() {
    use astersql_parser_ast::SqlDialect;
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    mysql.execute("CREATE DATABASE pg10_01_shared").unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    pg.execute("USE pg10_01_shared").unwrap();
    pg.execute("CREATE TABLE t(id integer PRIMARY KEY DEFAULT (40 + 2),v integer,note text)")
        .unwrap();
    let mut result = pg
        .execute("INSERT INTO t(v,note) VALUES (7,NULL) RETURNING id AS generated_id,v,note")
        .unwrap()
        .remove(0);
    assert_eq!(result.columns(), ["generated_id", "v", "note"]);
    let row = result.next_row().unwrap().unwrap();
    assert_eq!(&row[..2], ["42", "7"]);
    assert_eq!(row[2], "<nil>");
    assert!(result.next_row().unwrap().is_none());
    for sql in [
        "INSERT INTO t VALUES (1,10,NULL),(42,20,'duplicate') RETURNING id",
        "INSERT INTO t VALUES (2,11,NULL) RETURNING 10 / (v - 11) AS boom",
        "INSERT INTO t VALUES (3,12,NULL) RETURNING missing",
        "INSERT INTO t VALUES (4,13,NULL) ON CONFLICT (v) DO NOTHING RETURNING id",
    ] {
        assert!(pg.execute(sql).is_err(), "{sql}");
    }
    let mut result = pg
        .execute("SELECT id,v FROM t ORDER BY id")
        .unwrap()
        .remove(0);
    assert_eq!(result.next_row().unwrap().unwrap(), ["42", "7"]);
    assert!(result.next_row().unwrap().is_none());
    pg.execute("DROP DATABASE pg10_01_shared").unwrap();
}

#[test]
fn pg10_01_execution_boundaries() {
    use astersql_parser_ast::SqlDialect;
    let (domain, setup) = CreateAnalyzeSession().unwrap();
    setup.execute("CREATE DATABASE pg10_01_boundaries").unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    pg.execute("USE pg10_01_boundaries").unwrap();
    pg.execute("CREATE TABLE t(id integer PRIMARY KEY,v bigint)")
        .unwrap();
    for sql in [
        "INSERT INTO t VALUES (1,7) RETURNING id % 0",
        "INSERT INTO t VALUES (2,7) RETURNING v / 2",
        "INSERT IGNORE INTO t VALUES (3,7) RETURNING id",
    ] {
        assert!(
            pg.execute(sql).is_err(),
            "unproven execution shape must stay gated: {sql}"
        );
    }
    assert!(
        pg.execute("SELECT * FROM t").unwrap()[0]
            .next_row()
            .unwrap()
            .is_none()
    );
    pg.execute("DROP DATABASE pg10_01_boundaries").unwrap();
}

#[test]
fn pg10_02_update_rows_and_atomicity() {
    let (domain, setup) = CreateAnalyzeSession().unwrap();
    setup.execute("CREATE DATABASE pg10_02_shared").unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("USE pg10_02_shared").unwrap();
    pg.execute("CREATE TABLE t(id integer PRIMARY KEY,v integer,note text)")
        .unwrap();
    pg.execute("INSERT INTO t VALUES (1,10,NULL),(2,20,'b')")
        .unwrap();
    let mut rows = pg
        .execute("UPDATE t AS a SET v=v+1 WHERE id=1 RETURNING a.id,v,note")
        .unwrap()
        .remove(0);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["1", "11", "<nil>"]);
    assert!(rows.next_row().unwrap().is_none());
    for sql in [
        "UPDATE t SET v=v+1 RETURNING 10/(v-12)",
        "UPDATE t SET v=99 RETURNING missing",
        "UPDATE t SET v=99 FROM t AS other RETURNING t.id",
    ] {
        assert!(pg.execute(sql).is_err(), "{sql}");
    }
    let mut rows = pg
        .execute("SELECT id,v FROM t ORDER BY id")
        .unwrap()
        .remove(0);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["1", "11"]);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["2", "20"]);
    assert!(rows.next_row().unwrap().is_none());
    pg.execute("DROP DATABASE pg10_02_shared").unwrap();
}

#[test]
fn pg10_03_delete_rows_and_atomicity() {
    let (domain, setup) = CreateAnalyzeSession().unwrap();
    setup.execute("CREATE DATABASE pg10_03_shared").unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("USE pg10_03_shared").unwrap();
    pg.execute("CREATE TABLE t(id integer PRIMARY KEY,v integer,note text)")
        .unwrap();
    pg.execute("INSERT INTO t VALUES (1,10,NULL),(2,20,'b')")
        .unwrap();
    for sql in [
        "DELETE FROM t RETURNING 10/(v-20)",
        "DELETE FROM t RETURNING missing",
        "DELETE FROM t USING t AS other WHERE t.id=other.id RETURNING t.id",
    ] {
        assert!(pg.execute(sql).is_err(), "{sql}");
    }
    let mut rows = pg
        .execute("SELECT id,v,note FROM t ORDER BY id")
        .unwrap()
        .remove(0);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["1", "10", "<nil>"]);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["2", "20", "b"]);
    assert!(rows.next_row().unwrap().is_none());
    let mut rows = pg
        .execute("DELETE FROM t AS a WHERE id=1 RETURNING a.id,v,note")
        .unwrap()
        .remove(0);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["1", "10", "<nil>"]);
    assert!(rows.next_row().unwrap().is_none());
    let mut rows = pg
        .execute("SELECT id,v,note FROM t ORDER BY id")
        .unwrap()
        .remove(0);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["2", "20", "b"]);
    assert!(rows.next_row().unwrap().is_none());
    pg.execute("DROP DATABASE pg10_03_shared").unwrap();
}

#[test]
fn pg10_06_text_unique_prerequisite() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    assert!(
        mysql
            .execute("CREATE TABLE mysql_text_unique(v text UNIQUE)")
            .is_err()
    );
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE conflict_doc(id integer PRIMARY KEY,email text UNIQUE,v integer NOT NULL CHECK(v>0))").unwrap();
    pg.execute("INSERT INTO conflict_doc VALUES (1,'a',10),(2,'b',20)")
        .unwrap();
    assert!(
        pg.execute("INSERT INTO conflict_doc VALUES (3,'b',30)")
            .is_err()
    );

    pg.execute("INSERT INTO conflict_doc VALUES (3,'b',30) ON CONFLICT(email) DO NOTHING")
        .unwrap();
    // Preserve complete values even when they share a long prefix.
    let prefix = "x".repeat(800);
    pg.execute(&format!(
        "INSERT INTO conflict_doc VALUES (3,'{prefix}a',30),(4,'{prefix}b',40)"
    ))
    .unwrap();
    assert!(
        pg.execute(&format!(
            "INSERT INTO conflict_doc VALUES (5,'{prefix}a',50)"
        ))
        .is_err()
    );
    assert!(
        pg.execute(&format!(
            "INSERT INTO conflict_doc VALUES (6,'{}',60)",
            "x".repeat(4000)
        ))
        .is_err()
    );
    pg.execute(
        "INSERT INTO conflict_doc VALUES (1,'changed',99),(7,'c',70) ON CONFLICT(id) DO NOTHING",
    )
    .unwrap();
    assert!(
        pg.execute("INSERT INTO conflict_doc VALUES (8,'b',80) ON CONFLICT(id) DO NOTHING")
            .is_err()
    );
    assert!(
        pg.execute("INSERT INTO conflict_doc VALUES (8,'d',80) ON CONFLICT(v) DO NOTHING")
            .is_err()
    );
    let rows = pg
        .execute("SELECT email,v FROM conflict_doc WHERE id=1")
        .unwrap()[0]
        .next_row()
        .unwrap()
        .unwrap();
    assert_eq!(rows, ["a", "10"]);
    pg.execute("DROP TABLE conflict_doc").unwrap();
}

#[test]
fn pg10_07_all_unique_arbiters() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE conflict_doc(id integer PRIMARY KEY,email text UNIQUE,v integer NOT NULL CHECK(v>0))").unwrap();
    pg.execute("INSERT INTO conflict_doc VALUES (1,'a',10),(2,'b',20)")
        .unwrap();
    for values in ["(3,'b',30)", "(1,'changed',99)"] {
        pg.execute(&format!(
            "INSERT INTO conflict_doc VALUES {values} ON CONFLICT DO NOTHING"
        ))
        .unwrap();
    }
    pg.execute("INSERT INTO conflict_doc VALUES (1,'changed',99),(3,'c',30),(3,'changed',99),(4,'c',40) ON CONFLICT DO NOTHING").unwrap();
    pg.execute("INSERT INTO conflict_doc VALUES (4,NULL,40),(5,NULL,50) ON CONFLICT DO NOTHING")
        .unwrap();
    for values in ["(6,'d',NULL)", "(6,'d',0)", "(6,'d',1/0)"] {
        assert!(
            pg.execute(&format!(
                "INSERT INTO conflict_doc VALUES {values} ON CONFLICT DO NOTHING"
            ))
            .is_err()
        );
    }
    let mut results = pg
        .execute("SELECT id,email,v FROM conflict_doc ORDER BY id")
        .unwrap();
    let rows = &mut results[0];
    for expected in [
        vec!["1", "a", "10"],
        vec!["2", "b", "20"],
        vec!["3", "c", "30"],
        vec!["4", "<nil>", "40"],
        vec!["5", "<nil>", "50"],
    ] {
        assert_eq!(rows.next_row().unwrap().unwrap(), expected);
    }
    assert!(rows.next_row().unwrap().is_none());
    pg.execute("DROP TABLE conflict_doc").unwrap();
}

#[test]
fn pg10_07_check_prerequisite() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE conflict_doc(id integer PRIMARY KEY,email text UNIQUE,v integer NOT NULL CHECK(v>0))").unwrap();
    let result = pg.execute("INSERT INTO conflict_doc VALUES (3,'c',0)");
    pg.execute("DROP TABLE conflict_doc").unwrap();
    assert!(
        result.is_err(),
        "plain INSERT must enforce CHECK(v>0), without any conflict clause"
    );
}

#[test]
fn pg10_07_check_candidate_semantics() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(
        domain.clone(),
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    pg.execute("CREATE TABLE pg_check_local(id integer PRIMARY KEY,v integer, CHECK(v>0))")
        .unwrap();
    let table = domain.table_by_name("test", "pg_check_local").unwrap();
    assert_eq!(table.Constraints.len(), 1);
    assert!(table.Constraints[0].Enforced);
    assert_eq!(table.Constraints[0].State, astersql_meta_model::StatePublic);
    assert!(!table.Constraints[0].ExprString.is_empty());
    pg.execute("INSERT INTO pg_check_local VALUES (1,NULL),(2,1) ON CONFLICT DO NOTHING")
        .unwrap();
    for sql in [
        "INSERT INTO pg_check_local VALUES (1,0) ON CONFLICT DO NOTHING",
        "INSERT INTO pg_check_local VALUES (3,3),(4,0) ON CONFLICT DO NOTHING",
    ] {
        assert!(pg.execute(sql).is_err(), "{sql}");
    }
    let mut results = pg
        .execute("SELECT id,v FROM pg_check_local ORDER BY id")
        .unwrap();
    assert_eq!(results[0].next_row().unwrap().unwrap(), ["1", "<nil>"]);
    assert_eq!(results[0].next_row().unwrap().unwrap(), ["2", "1"]);
    assert!(results[0].next_row().unwrap().is_none());
    pg.execute("DROP TABLE pg_check_local").unwrap();
    // The local PG wiring must not enable a new MySQL constraint policy.
    mysql
        .execute("CREATE TABLE mysql_check_local(id int PRIMARY KEY,v int CHECK(v>0))")
        .unwrap();
    mysql
        .execute("INSERT INTO mysql_check_local VALUES (1,0)")
        .unwrap();
    mysql.execute("DROP TABLE mysql_check_local").unwrap();
}

#[test]
fn pg10_08_real_constraint_identity() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    use astersql_meta_model::{DecodeTableInfo, EncodeTableInfo};
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(
        domain.clone(),
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    mysql.execute("CREATE TABLE mysql_named(id INT PRIMARY KEY,email VARCHAR(30),CONSTRAINT mysql_email UNIQUE(email))").unwrap();
    mysql
        .execute("INSERT INTO mysql_named VALUES(1,'before')")
        .unwrap();
    let (statement, count, _) = mysql
        .prepare_protocol_statement("SELECT email FROM mysql_named WHERE id=?")
        .unwrap();
    assert_eq!(count, 1);
    mysql.execute("BEGIN").unwrap();
    mysql
        .execute("UPDATE mysql_named SET email='pending' WHERE id=1")
        .unwrap();
    pg.execute("CREATE TABLE conflict_doc(id integer CONSTRAINT conflict_doc_pkey PRIMARY KEY,email text CONSTRAINT conflict_doc_email_key UNIQUE,v integer NOT NULL CHECK(v>0))").unwrap();
    pg.execute("CREATE UNIQUE INDEX ordinary_email_idx ON conflict_doc(email)")
        .unwrap();
    pg.execute("CREATE TABLE conflict_other(email text CONSTRAINT other_email_key UNIQUE)")
        .unwrap();
    domain.force_full_reload_for_test().unwrap();
    let table = domain.table_by_name("test", "conflict_doc").unwrap();
    let decoded = DecodeTableInfo(&EncodeTableInfo(&table).unwrap()).unwrap();
    assert_eq!(
        decoded.PostgreSQLPrimaryConstraint.as_deref(),
        Some("conflict_doc_pkey")
    );
    let support = decoded
        .Indices
        .iter()
        .find(|index| index.PostgreSQLConstraintName.as_deref() == Some("conflict_doc_email_key"))
        .unwrap();
    assert!(support.ID > 0);
    assert!(support.Unique);
    assert_eq!(support.Columns[0].Name.L, "email");
    let ordinary = decoded
        .Indices
        .iter()
        .find(|index| index.Name.L == "ordinary_email_idx")
        .unwrap();
    assert!(ordinary.Unique);
    assert!(ordinary.PostgreSQLConstraintName.is_none());
    // Old metadata never acquires declaration provenance by name guessing.
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&EncodeTableInfo(&table).unwrap()).unwrap();
    legacy
        .as_object_mut()
        .unwrap()
        .remove("postgresql_primary_constraint");
    for index in legacy["index_info"].as_array_mut().unwrap() {
        index
            .as_object_mut()
            .unwrap()
            .remove("postgresql_constraint_name");
    }
    let legacy = DecodeTableInfo(&serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert!(legacy.PostgreSQLPrimaryConstraint.is_none());
    assert!(
        legacy
            .Indices
            .iter()
            .all(|index| index.PostgreSQLConstraintName.is_none())
    );
    pg.execute("INSERT INTO conflict_doc VALUES(1,'a',10),(2,'b',20)")
        .unwrap();
    pg.execute("INSERT INTO conflict_doc VALUES(3,'b',30) ON CONFLICT ON CONSTRAINT conflict_doc_email_key DO NOTHING").unwrap();
    for name in [
        "missing_constraint",
        "other_email_key",
        "ordinary_email_idx",
    ] {
        assert!(pg.execute(&format!("INSERT INTO conflict_doc VALUES(3,'c',30) ON CONFLICT ON CONSTRAINT {name} DO NOTHING")).is_err(), "{name}");
    }
    pg.execute("INSERT INTO conflict_doc VALUES(1,'c',30) ON CONFLICT ON CONSTRAINT conflict_doc_pkey DO NOTHING").unwrap();
    let mut result = pg
        .execute("SELECT id,email,v FROM conflict_doc ORDER BY id")
        .unwrap();
    assert_eq!(result[0].next_row().unwrap().unwrap(), vec!["1", "a", "10"]);
    assert_eq!(result[0].next_row().unwrap().unwrap(), vec!["2", "b", "20"]);
    assert!(result[0].next_row().unwrap().is_none());
    // The association travels with the real supporting index, not its name.
    pg.execute("ALTER TABLE conflict_doc RENAME INDEX conflict_doc_email_key TO renamed_support")
        .unwrap();
    pg.execute("INSERT INTO conflict_doc VALUES(3,'b',30) ON CONFLICT ON CONSTRAINT conflict_doc_email_key DO NOTHING").unwrap();
    pg.execute("ALTER TABLE conflict_doc DROP INDEX renamed_support")
        .unwrap();
    assert!(pg.execute("INSERT INTO conflict_doc VALUES(3,'c',30) ON CONFLICT ON CONSTRAINT conflict_doc_email_key DO NOTHING").is_err());
    for ddl in [
        "CREATE TABLE bad_names(a integer CONSTRAINT duplicate UNIQUE,b integer CONSTRAINT duplicate UNIQUE)",
        "CREATE TABLE bad_names(a integer CONSTRAINT duplicate UNIQUE,b integer,CONSTRAINT duplicate UNIQUE(b))",
    ] {
        assert!(pg.execute(ddl).is_err(), "{ddl}");
        assert!(domain.table_by_name("test", "bad_names").is_err());
    }
    pg.execute("CREATE TABLE quoted_constraint(email text CONSTRAINT \"EmailKey\" UNIQUE)")
        .unwrap();
    pg.execute("INSERT INTO quoted_constraint VALUES('a')")
        .unwrap();
    pg.execute("INSERT INTO quoted_constraint VALUES('a') ON CONFLICT ON CONSTRAINT \"EmailKey\" DO NOTHING").unwrap();
    assert!(pg.execute("INSERT INTO quoted_constraint VALUES('b') ON CONFLICT ON CONSTRAINT emailkey DO NOTHING").is_err());
    pg.execute("DROP TABLE quoted_constraint").unwrap();
    let mut rows = mysql
        .execute_protocol_statement(
            statement,
            &[crate::runtime::ConcretePreparedArgument::Signed(1)],
        )
        .unwrap();
    assert_eq!(rows[0].next_row().unwrap().unwrap(), vec!["pending"]);
    let mut observer = pg
        .execute("SELECT email FROM mysql_named WHERE id=1")
        .unwrap();
    assert_eq!(observer[0].next_row().unwrap().unwrap(), vec!["before"]);
    mysql.execute("ROLLBACK").unwrap();
    let mut rows = mysql
        .execute_protocol_statement(
            statement,
            &[crate::runtime::ConcretePreparedArgument::Signed(1)],
        )
        .unwrap();
    assert_eq!(rows[0].next_row().unwrap().unwrap(), vec!["before"]);
    mysql.close_protocol_statement(statement).unwrap();
    assert!(
        domain
            .table_by_name("test", "mysql_named")
            .unwrap()
            .Indices
            .iter()
            .all(|index| index.PostgreSQLConstraintName.is_none())
    );
    mysql.execute("DROP TABLE mysql_named").unwrap();
    mysql
        .execute("CREATE TABLE copied_named LIKE conflict_other")
        .unwrap();
    let copied = domain.table_by_name("test", "copied_named").unwrap();
    assert!(copied.PostgreSQLPrimaryConstraint.is_none());
    assert!(
        copied
            .Indices
            .iter()
            .all(|index| index.PostgreSQLConstraintName.is_none())
    );
    assert!(pg.execute("INSERT INTO copied_named VALUES('new') ON CONFLICT ON CONSTRAINT other_email_key DO NOTHING").is_err());
    mysql.execute("DROP TABLE copied_named").unwrap();
    pg.execute("DROP TABLE conflict_other").unwrap();
    pg.execute("DROP TABLE conflict_doc").unwrap();
}

#[test]
fn pg10_09_storage_failure_is_not_a_conflict() {
    let (domain, observer) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE conflict_storage(id integer PRIMARY KEY,email text UNIQUE,v integer NOT NULL)")
        .unwrap();
    pg.execute("INSERT INTO conflict_storage VALUES (1,'a',10),(2,'b',20)")
        .unwrap();
    for returning in [false, true] {
        pg.InjectNextDmlCommitError("pg10_09 storage commit failure");
        let sql = format!(
            "INSERT INTO conflict_storage VALUES (3,'c',30),(4,'d',40) ON CONFLICT DO NOTHING{}",
            if returning {
                " RETURNING id,email,v"
            } else {
                ""
            }
        );
        let error = match pg.execute(&sql) {
            Err(error) => error,
            Ok(_) => panic!("storage failure must not become DO NOTHING success"),
        };
        assert!(
            error.to_string().contains("pg10_09 storage commit failure"),
            "{error}"
        );
        for session in [&pg, &observer] {
            let mut results = session
                .execute("SELECT id,email,v FROM conflict_storage ORDER BY id")
                .unwrap();
            assert_eq!(results[0].next_row().unwrap().unwrap(), ["1", "a", "10"]);
            assert_eq!(results[0].next_row().unwrap().unwrap(), ["2", "b", "20"]);
            assert!(results[0].next_row().unwrap().is_none());
        }
        pg.execute("INSERT INTO conflict_storage VALUES (3,'c',30) ON CONFLICT DO NOTHING")
            .unwrap();
        pg.execute("DELETE FROM conflict_storage WHERE id=3")
            .unwrap();
    }
    pg.execute("DROP TABLE conflict_storage").unwrap();
}

#[test]
fn pg10_10_expression_gate() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(
        domain.clone(),
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let observer =
        ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE expression_gate(id integer PRIMARY KEY,email text UNIQUE,v integer)")
        .unwrap();
    pg.execute("INSERT INTO expression_gate VALUES(1,'a',10),(2,'b',20)")
        .unwrap();
    fn rows(session: &ConcreteSession) -> Vec<Vec<String>> {
        let mut result = session
            .execute("SELECT id,email,v FROM expression_gate ORDER BY id")
            .unwrap();
        let mut rows = Vec::new();
        while let Some(row) = result[0].next_row().unwrap() {
            rows.push(row);
        }
        rows
    }
    let initial = rows(&pg);
    for target in [
        "((id+1))",
        "(lower(email))",
        "((lower(email)))",
        "(id,(v+1))",
        "((id))",
        "((v+1)) WHERE v>0",
    ] {
        for transaction in [false, true] {
            if transaction {
                pg.execute("BEGIN").unwrap();
                pg.execute("INSERT INTO expression_gate VALUES(9,'pending',90)")
                    .unwrap();
            }
            let sql = format!(
                "INSERT INTO expression_gate VALUES(3,'c',30) ON CONFLICT {target} DO NOTHING"
            );
            let error = pg
                .execute(&sql)
                .err()
                .expect("expression arbitration must be rejected");
            assert!(
                error
                    .to_string()
                    .contains("unsupported PostgreSQL conflict"),
                "{sql}: {error}"
            );
            assert_eq!(rows(&observer), initial);
            if transaction {
                pg.execute("ROLLBACK").unwrap();
            }
            assert_eq!(rows(&pg), initial);
        }
    }
    pg.execute("INSERT INTO expression_gate VALUES(1,'c',30) ON CONFLICT (id) DO NOTHING")
        .unwrap();
    assert_eq!(rows(&pg), initial);
    pg.execute("DROP TABLE expression_gate").unwrap();
}

#[test]
fn pg10_10_mysql_query_prepared_coexistence() {
    use crate::runtime::{ConcretePreparedArgument as Arg, ConcreteSession, CreateAnalyzeSession};
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    let observer = ConcreteSession::new(domain.clone());
    let pg = ConcreteSession::new_with_dialect(
        domain.clone(),
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
        let mut sets = session.execute(sql).unwrap();
        let mut rows = Vec::new();
        while let Some(row) = sets[0].next_row().unwrap() {
            rows.push(row);
        }
        rows
    }
    mysql.execute("SET autocommit=1").unwrap();
    for session in [&mysql, &observer] {
        eprintln!(
            "mysql settings: {:?}",
            rows(session, "SELECT @@sql_mode,@@transaction_isolation")
        );
    }
    pg.execute(
        "CREATE TABLE conflict_gate_coexist(id integer PRIMARY KEY,email text UNIQUE,v integer)",
    )
    .unwrap();
    pg.execute("INSERT INTO conflict_gate_coexist VALUES(1,'a',10),(2,'b',20)")
        .unwrap();
    for prepared in [false, true] {
        for commit in [false, true] {
            mysql.execute("CREATE TABLE conflict_mysql_doc(id INT PRIMARY KEY,email VARCHAR(30) UNIQUE,v INT NOT NULL)").unwrap();
            mysql
                .execute("INSERT INTO conflict_mysql_doc VALUES(1,'a',10),(2,'b',20)")
                .unwrap();
            let scan = "SELECT id,email,v FROM conflict_mysql_doc ORDER BY id";
            let initial = rows(&mysql, scan);
            let (select, count, fields) = mysql
                .prepare_protocol_statement("SELECT v FROM conflict_mysql_doc WHERE id=?")
                .unwrap();
            assert_eq!(count, 1);
            assert_eq!(fields.len(), 1);
            assert_eq!(fields[0].column_as_name.O, "v");
            assert_eq!(
                fields[0].column.FieldType.GetType(),
                astersql_parser_mysql::r#type::TypeLong
            );

            let (insert, count, _) = mysql
                .prepare_protocol_statement(
                    "INSERT IGNORE INTO conflict_mysql_doc VALUES(?,?,?),(?,?,?)",
                )
                .unwrap();
            assert_eq!(count, 6);
            mysql.execute("BEGIN").unwrap();
            if prepared {
                mysql
                    .execute_protocol_statement(
                        insert,
                        &[
                            Arg::Signed(1),
                            Arg::Text("changed".into()),
                            Arg::Signed(99),
                            Arg::Signed(3),
                            Arg::Text("c".into()),
                            Arg::Signed(30),
                        ],
                    )
                    .unwrap();
            } else {
                mysql
                    .execute(
                        "INSERT IGNORE INTO conflict_mysql_doc VALUES(1,'changed',99),(3,'c',30)",
                    )
                    .unwrap();
            }
            let state = mysql.protocol_state();
            assert_eq!(state.affected_rows, 1);
            assert_eq!(state.warning_count, 1);
            eprintln!(
                "mysql prepared={prepared} commit={commit}: affected={} warnings={:?}",
                state.affected_rows,
                rows(&mysql, "SHOW WARNINGS")
            );
            let stored = rows(&mysql, scan);
            assert_eq!(
                stored,
                vec![
                    vec!["1", "a", "10"],
                    vec!["2", "b", "20"],
                    vec!["3", "c", "30"]
                ]
            );
            for target in ["(id) WHERE NULL", "(lower(email))", "(id,(v+1))"] {
                let mut sets = mysql
                    .execute_protocol_statement(select, &[Arg::Signed(1)])
                    .unwrap();
                assert_eq!(sets[0].next_row().unwrap().unwrap(), vec!["10"]);
                assert_eq!(rows(&observer, scan), initial);
                assert!(pg.execute(&format!("INSERT INTO conflict_gate_coexist VALUES(3,'c',30) ON CONFLICT {target} DO NOTHING")).is_err());
                assert_eq!(rows(&mysql, scan), stored);
                let mut sets = mysql
                    .execute_protocol_statement(select, &[Arg::Signed(1)])
                    .unwrap();
                assert_eq!(sets[0].next_row().unwrap().unwrap(), vec!["10"]);
                assert_eq!(
                    rows(
                        &pg,
                        "SELECT id,email,v FROM conflict_gate_coexist ORDER BY id"
                    ),
                    initial
                );
            }
            mysql
                .execute(if commit { "COMMIT" } else { "ROLLBACK" })
                .unwrap();
            for session in [&mysql, &observer] {
                assert_eq!(
                    rows(session, scan),
                    if commit {
                        stored.clone()
                    } else {
                        initial.clone()
                    }
                );
            }
            mysql.close_protocol_statement(select).unwrap();
            mysql.close_protocol_statement(insert).unwrap();
            mysql.execute("DROP TABLE conflict_mysql_doc").unwrap();
        }
    }
    pg.execute("DROP TABLE conflict_gate_coexist").unwrap();
}

#[test]
fn pg10_10_complex_setup_has_no_catalog_residue() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(
        domain.clone(),
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    for sql in [
        "CREATE TABLE conflict_deferred(id integer,CONSTRAINT conflict_deferred_u UNIQUE(id) DEFERRABLE INITIALLY DEFERRED)",
        "CREATE TABLE conflict_exclusion(id integer,CONSTRAINT conflict_exclusion_e EXCLUDE USING btree(id WITH =))",
    ] {
        let error = pg.execute(sql).err().expect(sql);
        assert!(error.to_string().contains("not supported"), "{error}");
        assert!(!error.to_string().contains("syntax"), "{error}");
    }
    assert!(domain.table_by_name("test", "conflict_deferred").is_err());
    assert!(domain.table_by_name("test", "conflict_exclusion").is_err());
    pg.execute("CREATE TABLE conflict_partial(id integer PRIMARY KEY,k integer,active boolean)")
        .unwrap();
    let error = pg
        .execute("CREATE UNIQUE INDEX conflict_partial_active ON conflict_partial(k) WHERE active")
        .err()
        .expect("unsupported setup");
    assert!(error.to_string().contains("not supported"), "{error}");
    assert!(
        domain
            .table_by_name("test", "conflict_partial")
            .unwrap()
            .Indices
            .iter()
            .all(|index| index.Name.L != "conflict_partial_active")
    );
    pg.execute("DROP TABLE conflict_partial").unwrap();
}

#[test]
fn pg10_10_missing_index_is_a_binding_error() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(
        domain.clone(),
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let observer =
        ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE missing_arbiter(id integer PRIMARY KEY,email text UNIQUE,v integer)")
        .unwrap();
    pg.execute("INSERT INTO missing_arbiter VALUES(1,'a',10),(2,'b',20)")
        .unwrap();
    for transaction in [false, true] {
        if transaction {
            pg.execute("BEGIN").unwrap();
            pg.execute("INSERT INTO missing_arbiter VALUES(9,'pending',90)")
                .unwrap();
        }
        let error=pg.execute("INSERT INTO missing_arbiter VALUES(3,'c',30) ON CONFLICT(v) DO NOTHING RETURNING id,email,v").err().expect("missing arbiter");
        assert_eq!(error.pg_array_error().unwrap().state, "42P10");
        if transaction {
            pg.execute("ROLLBACK").unwrap();
        }
        for session in [&pg, &observer] {
            let mut sets = session
                .execute("SELECT id,email,v FROM missing_arbiter ORDER BY id")
                .unwrap();
            assert_eq!(sets[0].next_row().unwrap().unwrap(), vec!["1", "a", "10"]);
            assert_eq!(sets[0].next_row().unwrap().unwrap(), vec!["2", "b", "20"]);
            assert_eq!(sets[0].next_row().unwrap(), None);
        }
    }
    pg.execute("DROP TABLE missing_arbiter").unwrap();
}

#[test]
fn pg10_11_candidate_binding() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(
        domain.clone(),
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let o = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
        let mut sets = session.execute(sql).unwrap();
        let mut result = vec![];
        while let Some(row) = sets[0].next_row().unwrap() {
            result.push(row);
        }
        result
    }
    pg.execute("CREATE TABLE candidate_binding(id integer PRIMARY KEY,email text UNIQUE,v integer NOT NULL DEFAULT 7 CHECK(v>0))").unwrap();
    pg.execute("INSERT INTO candidate_binding VALUES(1,'a',10),(2,'b',20)")
        .unwrap();
    for commit in [false, true] {
        pg.execute("BEGIN").unwrap();
        assert_eq!(
            rows(
                &pg,
                "INSERT INTO candidate_binding AS dst(id,email) VALUES(1,'candidate') ON CONFLICT(id) DO UPDATE SET v=dst.v+excluded.v RETURNING id,email,v"
            ),
            vec![vec!["1", "a", "17"]]
        );
        assert_eq!(
            rows(&o, "SELECT v FROM candidate_binding WHERE id=1"),
            vec![vec!["10"]]
        );
        pg.execute(if commit { "COMMIT" } else { "ROLLBACK" })
            .unwrap();
        assert_eq!(
            rows(&o, "SELECT v FROM candidate_binding WHERE id=1"),
            vec![vec![if commit { "17" } else { "10" }]]
        );
    }
    // Binding errors must be found even when no update is needed.
    for sql in [
        "INSERT INTO candidate_binding AS dst VALUES(3,'c',3) ON CONFLICT(id) DO UPDATE SET v=candidate_binding.v",
        "INSERT INTO candidate_binding AS dst VALUES(3,'c',3) ON CONFLICT(id) DO UPDATE SET dst.v=excluded.v",
        "INSERT INTO candidate_binding AS dst VALUES(3,'c',3) ON CONFLICT(id) DO UPDATE SET v=excluded.missing",
    ] {
        assert!(pg.execute(sql).is_err(), "{sql}");
        assert!(rows(&pg, "SELECT id FROM candidate_binding WHERE id=3").is_empty());
    }
    pg.execute("DROP TABLE candidate_binding").unwrap();
}

#[test]
fn pg10_12_action_condition() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(
        domain.clone(),
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let observer =
        ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
        let mut sets = session.execute(sql).unwrap();
        let mut result = vec![];
        while let Some(row) = sets[0].next_row().unwrap() {
            result.push(row);
        }
        result
    }
    for commit in [false, true] {
        pg.execute("CREATE TABLE action_condition(id integer PRIMARY KEY,email text UNIQUE,v integer NOT NULL DEFAULT 7 CHECK(v>0))").unwrap();
        pg.execute("INSERT INTO action_condition VALUES(1,'a',10),(2,'b',20)")
            .unwrap();
        // Target WHERE remains an independent write-before-effect gate.
        for sql in [
            "INSERT INTO action_condition AS dst VALUES(3,'c',30) ON CONFLICT(id) WHERE id>0 DO UPDATE SET v=excluded.v WHERE dst.v<excluded.v",
            "INSERT INTO action_condition AS dst VALUES(3,'c',30) ON CONFLICT(id) DO UPDATE SET v=excluded.v WHERE 7",
        ] {
            assert!(pg.execute(sql).is_err(), "{sql}");
            assert!(rows(&pg, "SELECT id FROM action_condition WHERE id=3").is_empty());
        }
        pg.execute("BEGIN").unwrap();
        for condition in ["dst.v<excluded.v", "NULL::boolean"] {
            assert!(rows(&pg, &format!("INSERT INTO action_condition AS dst VALUES(1,'candidate',3) ON CONFLICT(id) DO UPDATE SET v=1/0 WHERE {condition} RETURNING id,email,v")).is_empty());
            assert_eq!(pg.LastDmlReport().unwrap().AffectedRows, 0);
        }
        assert_eq!(
            rows(
                &pg,
                "INSERT INTO action_condition AS dst VALUES(1,'candidate',30),(3,'c',3) ON CONFLICT(id) DO UPDATE SET v=excluded.v WHERE dst.v<excluded.v RETURNING id,email,v"
            ),
            vec![vec!["1", "a", "30"], vec!["3", "c", "3"]]
        );
        assert_eq!(pg.LastDmlReport().unwrap().AffectedRows, 2);
        let scan = "SELECT id,email,v FROM action_condition ORDER BY id";
        let initial = vec![vec!["1", "a", "10"], vec!["2", "b", "20"]];
        let updated = vec![
            vec!["1", "a", "30"],
            vec!["2", "b", "20"],
            vec!["3", "c", "3"],
        ];
        assert_eq!(rows(&pg, scan), updated);
        assert_eq!(rows(&observer, scan), initial);
        pg.execute(if commit { "COMMIT" } else { "ROLLBACK" })
            .unwrap();
        for session in [&pg, &observer] {
            assert_eq!(
                rows(session, scan),
                if commit { &updated } else { &initial }.clone()
            );
        }
        pg.execute("DROP TABLE action_condition").unwrap();
    }
}

#[test]
fn pg10_13_secondary_errors_are_atomic() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(
        domain.clone(),
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let observer =
        ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    fn rows(session: &ConcreteSession) -> Vec<Vec<String>> {
        let mut sets = session
            .execute("SELECT id,email,v FROM secondary_errors ORDER BY id")
            .unwrap();
        let mut rows = vec![];
        while let Some(row) = sets[0].next_row().unwrap() {
            rows.push(row);
        }
        rows
    }
    let initial = vec![vec!["1", "a", "10"], vec!["2", "b", "20"]];
    for transaction in [false, true] {
        for assignment in [
            "email=excluded.email",
            "v=NULL",
            "v=0",
            "v=-excluded.v",
            "v=dst.v/0",
        ] {
            pg.execute("CREATE TABLE secondary_errors(id integer PRIMARY KEY,email text UNIQUE,v integer NOT NULL DEFAULT 7 CHECK(v>0))").unwrap();
            pg.execute("INSERT INTO secondary_errors VALUES(1,'a',10),(2,'b',20)")
                .unwrap();
            if transaction {
                pg.execute("BEGIN").unwrap();
                pg.execute("INSERT INTO secondary_errors VALUES(9,'pending',90)")
                    .unwrap();
            }
            let sql = format!(
                "INSERT INTO secondary_errors AS dst VALUES(3,'c',30),(1,'b',3) ON CONFLICT(id) DO UPDATE SET {assignment}"
            );
            let error = pg
                .execute(&sql)
                .err()
                .expect("secondary error must abort the statement");
            if assignment == "v=0" || assignment == "v=-excluded.v" {
                assert_eq!(error.pg_array_error().unwrap().state, "23514");
            }
            assert_eq!(rows(&observer), initial);
            if transaction {
                pg.execute("ROLLBACK").unwrap();
            }
            assert_eq!(rows(&pg), initial);
            // Both unique indexes remain usable after the failed mixed statement.
            pg.execute("BEGIN").unwrap();
            pg.execute("INSERT INTO secondary_errors VALUES(3,'c',30)")
                .unwrap();
            pg.execute("ROLLBACK").unwrap();
            assert_eq!(rows(&pg), initial);
            pg.execute("DROP TABLE secondary_errors").unwrap();
        }
    }
}

#[test]
fn pg10_14_repeated_rows_and_skipped_actions() {
    use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(
        domain.clone(),
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let observer =
        ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    fn rows(session: &ConcreteSession) -> Vec<Vec<String>> {
        let mut sets = session
            .execute("SELECT id,email,v FROM repeated_rows ORDER BY id")
            .unwrap();
        let mut rows = vec![];
        while let Some(row) = sets[0].next_row().unwrap() {
            rows.push(row);
        }
        rows
    }
    let initial = vec![vec!["1", "a", "10"], vec!["2", "b", "20"]];
    for transaction in [false, true] {
        for (input, action, fails, updated) in [
            (
                "(1,'candidate',3),(1,'again',4)",
                "v=dst.v+excluded.v",
                true,
                false,
            ),
            ("(3,'c',3),(3,'d',4)", "v=dst.v+excluded.v", true, false),
            (
                "(3,'c',30),(1,'candidate',3),(1,'again',4)",
                "v=dst.v+excluded.v",
                true,
                false,
            ),
            (
                "(1,'candidate',30),(1,'again',4)",
                "v=excluded.v WHERE dst.v<excluded.v",
                true,
                false,
            ),
            (
                "(3,'c',30),(3,'d',4)",
                "v=excluded.v WHERE false",
                true,
                false,
            ),
            ("(1,'candidate',3),(1,'again',4)", "v=dst.v", true, false),
            (
                "(1,'candidate',3),(1,'again',30)",
                "v=excluded.v WHERE dst.v<excluded.v",
                false,
                true,
            ),
            (
                "(1,'candidate',3),(1,'again',4)",
                "v=1/0 WHERE false",
                false,
                false,
            ),
            (
                "(1,'candidate',3),(1,'again',4)",
                "v=1/0 WHERE NULL::boolean",
                false,
                false,
            ),
        ] {
            pg.execute("CREATE TABLE repeated_rows(id integer PRIMARY KEY,email text UNIQUE,v integer NOT NULL CHECK(v>0))").unwrap();
            pg.execute("INSERT INTO repeated_rows VALUES(1,'a',10),(2,'b',20)")
                .unwrap();
            if transaction {
                pg.execute("BEGIN").unwrap();
            }
            let sql = format!(
                "INSERT INTO repeated_rows AS dst VALUES{input} ON CONFLICT(id) DO UPDATE SET {action}"
            );
            let result = pg.execute(&sql);
            if fails {
                assert_eq!(
                    result
                        .err()
                        .expect("repeat must fail")
                        .pg_array_error()
                        .unwrap()
                        .state,
                    "21000"
                );
            } else {
                result.unwrap();
            }
            if !transaction || fails {
                let expected = if updated {
                    vec![vec!["1", "a", "30"], vec!["2", "b", "20"]]
                } else {
                    initial.clone()
                };
                assert_eq!(rows(&observer), expected);
            } else {
                assert_eq!(rows(&observer), initial);
            }
            if transaction {
                pg.execute("ROLLBACK").unwrap();
            }
            if transaction || fails {
                assert_eq!(rows(&pg), initial);
            }
            pg.execute("BEGIN").unwrap();
            pg.execute("INSERT INTO repeated_rows VALUES(3,'c',30)")
                .unwrap();
            pg.execute("ROLLBACK").unwrap();
            pg.execute("DROP TABLE repeated_rows").unwrap();
        }
    }
}

#[test]
fn pg10_15_refresh_keeps_prior_candidates() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let session =
        ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    session
        .execute("CREATE DATABASE IF NOT EXISTS test")
        .unwrap();
    session.execute("USE test").unwrap();
    session
        .execute(
            "CREATE TABLE conflict_refresh (id integer PRIMARY KEY,email text UNIQUE,v integer)",
        )
        .unwrap();
    session
        .execute("SET tidb_txn_mode = 'pessimistic'")
        .unwrap();
    for end in ["COMMIT", "ROLLBACK"] {
        session.execute("BEGIN").unwrap();
        session
            .execute("INSERT INTO conflict_refresh VALUES (1,'earlier',10)")
            .unwrap();
        let mut result = session.execute("INSERT INTO conflict_refresh VALUES (2,'batch',20),(3,'batch',30),(4,'earlier',40),(5,NULL,50),(6,NULL,60) ON CONFLICT (email) DO NOTHING RETURNING id,v").unwrap();
        let mut returned = vec![];
        while let Some(row) = result[0].next_row().unwrap() {
            returned.push(row);
        }
        assert_eq!(
            returned,
            vec![vec!["2", "20"], vec!["5", "50"], vec!["6", "60"]]
        );
        session.execute(end).unwrap();
        let mut result = session
            .execute("SELECT id,email,v FROM conflict_refresh ORDER BY id")
            .unwrap();
        let mut rows = vec![];
        while let Some(row) = result[0].next_row().unwrap() {
            rows.push(row);
        }
        if end == "COMMIT" {
            assert_eq!(rows.len(), 4);
            assert_eq!(rows[0], vec!["1", "earlier", "10"]);
        } else {
            assert!(rows.is_empty());
        }
        session.execute("DELETE FROM conflict_refresh").unwrap();
    }
}

#[test]
fn pg10_16_refresh_keeps_updated_candidates() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE DATABASE IF NOT EXISTS test").unwrap();
    pg.execute("USE test").unwrap();
    pg.execute(
        "CREATE TABLE conflict_update_refresh (id integer PRIMARY KEY,email text UNIQUE,v integer)",
    )
    .unwrap();
    pg.execute("SET tidb_txn_mode = 'pessimistic'").unwrap();
    for end in ["COMMIT", "ROLLBACK"] {
        pg.execute("INSERT INTO conflict_update_refresh VALUES (1,'old',10)")
            .unwrap();
        pg.execute("BEGIN").unwrap();
        // The first candidate moves the primary key; the second reuses the freed key.
        // Refreshing arbitration must retain both the deletion and the new row.
        let mut result = pg.execute("INSERT INTO conflict_update_refresh VALUES (2,'old',20),(1,'new',30) ON CONFLICT (email) DO UPDATE SET id=excluded.id,v=excluded.v RETURNING id,v").unwrap();
        assert_eq!(result[0].next_row().unwrap().unwrap(), vec!["2", "20"]);
        assert_eq!(result[0].next_row().unwrap().unwrap(), vec!["1", "30"]);
        assert!(result[0].next_row().unwrap().is_none());
        pg.execute(end).unwrap();
        let mut result = pg
            .execute("SELECT id,email,v FROM conflict_update_refresh ORDER BY id")
            .unwrap();
        if end == "COMMIT" {
            assert_eq!(
                result[0].next_row().unwrap().unwrap(),
                vec!["1", "new", "30"]
            );
            assert_eq!(
                result[0].next_row().unwrap().unwrap(),
                vec!["2", "old", "20"]
            );
        } else {
            assert_eq!(
                result[0].next_row().unwrap().unwrap(),
                vec!["1", "old", "10"]
            );
        }
        assert!(result[0].next_row().unwrap().is_none());
        pg.execute("DELETE FROM conflict_update_refresh").unwrap();
    }
}

#[test]
fn pg10_18_partial_cte_aliases() {
    use astersql_parser_ast::SqlDialect;
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    let sql = "WITH a(x) AS (SELECT 1 AS id,2 AS v) SELECT x,v FROM a";
    let mut result = pg.execute(sql).unwrap();
    assert_eq!(result[0].next_row().unwrap().unwrap(), vec!["1", "2"]);
    assert!(result[0].next_row().unwrap().is_none());
    assert!(mysql.execute(sql).is_err());
    let error = pg
        .execute("WITH a(x,y,z) AS (SELECT 1 AS id,2 AS v) SELECT x FROM a")
        .err()
        .expect("excess aliases must fail");
    assert_eq!(error.pg_array_error().unwrap().state, "42P10");
    let mut result = mysql
        .execute("WITH a(x,v) AS (SELECT 1 AS id,2 AS v) SELECT x,v FROM a")
        .unwrap();
    assert_eq!(result[0].next_row().unwrap().unwrap(), vec!["1", "2"]);
}

#[test]
fn pg10_19_repeated_cte_error_identity_and_cleanup() {
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    let sql = "WITH a AS (SELECT 10::int4 AS v UNION ALL SELECT 10::int4 AS v) SELECT 10::int4/(l.v-10::int4)::int4 AS bad FROM a l CROSS JOIN a r";
    let error = pg
        .execute(sql)
        .err()
        .expect("consumer must fail at runtime");
    assert_eq!(
        error.pg_int4_error(),
        Some(astersql_expression::PgInt4Error::DivisionByZero)
    );
    let mut rows = pg
        .execute("WITH a AS (SELECT 7::int4 AS v) SELECT l.v,r.v FROM a l CROSS JOIN a r")
        .unwrap();
    assert_eq!(rows[0].next_row().unwrap().unwrap(), vec!["7", "7"]);
    assert!(rows[0].next_row().unwrap().is_none());
    assert!(pg.execute("SELECT v FROM a").is_err());
}

#[test]
fn pg10_20_unused_cte_evaluation() {
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    for sql in [
        "WITH a AS (SELECT 1::int4/0::int4 AS v) SELECT 7::int4 AS v",
        "WITH a AS (SELECT 1::int4/0::int4 AS v),b AS (SELECT v FROM a) SELECT 7::int4 AS v",
        "WITH a AS (SELECT 1::int4/0::int4 AS v) SELECT v FROM (WITH a AS (SELECT 7::int4 AS v) SELECT v FROM a) x",
        "WITH a AS (SELECT 7::int4 AS v),b AS (WITH a AS (SELECT v FROM a) SELECT v FROM a) SELECT v FROM b",
    ] {
        let mut result = pg
            .execute(sql)
            .unwrap_or_else(|error| panic!("{sql}: {error}"));
        assert_eq!(result[0].next_row().unwrap().unwrap(), vec!["7"], "{sql}");
        assert!(result[0].next_row().unwrap().is_none());
    }
    let error = pg
        .execute("WITH a AS (SELECT 1::int4/0::int4 AS v) SELECT v FROM a")
        .err()
        .expect("a consumed producer must execute");
    assert_eq!(
        error.pg_int4_error(),
        Some(astersql_expression::PgInt4Error::DivisionByZero)
    );
    assert!(pg.execute("SELECT v FROM a").is_err());
}

thread_local! {
    static PG10_21_MOVE_EVENTS: std::cell::RefCell<Option<Vec<(&'static str,u64)>>> = const { std::cell::RefCell::new(None) };
}

pub(crate) fn pg10_21_record_move_event(kind: &'static str, start_ts: u64) {
    PG10_21_MOVE_EVENTS.with_borrow_mut(|events| {
        if let Some(events) = events {
            events.push((kind, start_ts));
        }
    });
}

#[test]
fn pg10_21_move_atomic_rows() {
    use astersql_parser_ast::SqlDialect;
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    let mysql_observer = ConcreteSession::new(domain.clone());
    let pg = ConcreteSession::new_with_dialect(domain.clone(), SqlDialect::PostgreSQL);
    let observer = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    let rows = |session: &ConcreteSession, sql: &str| {
        let mut sets = session.execute(sql).unwrap();
        let mut rows = Vec::new();
        while let Some(row) = sets[0].next_row().unwrap() {
            rows.push(row);
        }
        rows.sort();
        rows
    };
    mysql
        .execute("CREATE TABLE pg10_21_mysql(id INT PRIMARY KEY,v INT)")
        .unwrap();
    mysql
        .execute("INSERT INTO pg10_21_mysql VALUES(1,10),(2,NULL)")
        .unwrap();
    let mysql_initial = rows(&mysql, "SELECT id,v FROM pg10_21_mysql ORDER BY id");
    let (prepared, _, _) = mysql
        .prepare_protocol_statement("SELECT id,v FROM pg10_21_mysql WHERE id=?")
        .unwrap();
    let prepared_row = mysql
        .execute_protocol_statement(
            prepared,
            &[crate::runtime::ConcretePreparedArgument::Signed(2)],
        )
        .unwrap()[0]
        .next_row()
        .unwrap()
        .unwrap();
    assert_eq!(prepared_row, mysql_initial[1]);
    mysql.execute("BEGIN").unwrap();
    mysql
        .execute("UPDATE pg10_21_mysql SET v=11 WHERE id=1")
        .unwrap();
    assert_eq!(
        rows(
            &mysql_observer,
            "SELECT id,v FROM pg10_21_mysql ORDER BY id"
        ),
        mysql_initial
    );
    for sql in [
        "CREATE TABLE pg10_21_src(id INT PRIMARY KEY,v INT)",
        "CREATE TABLE pg10_21_dst(id INT PRIMARY KEY,v INT)",
        "INSERT INTO pg10_21_src VALUES (1,10),(2,NULL),(3,30)",
    ] {
        pg.execute(sql).unwrap();
    }
    let initial = rows(&pg, "SELECT id,v FROM pg10_21_src");
    pg.execute("BEGIN").unwrap();
    let start = rows(&pg, "SELECT @@tidb_current_ts")[0][0]
        .parse::<u64>()
        .unwrap();
    PG10_21_MOVE_EVENTS.with_borrow_mut(|events| *events = Some(vec![]));
    let returned = rows(
        &pg,
        "WITH moved AS (DELETE FROM pg10_21_src WHERE id<=2 RETURNING id,v) INSERT INTO pg10_21_dst SELECT id,v FROM moved RETURNING id,v",
    );
    let events = PG10_21_MOVE_EVENTS.with_borrow_mut(Option::take).unwrap();
    eprintln!("pg10_21 real producer/consumer transaction events: {events:?}");
    assert_eq!(
        events,
        vec![("delete", start), ("delete-complete", 2), ("insert", start)]
    );
    assert_ne!(start, 0);
    assert_eq!(returned, initial[..2]);
    assert_eq!(
        rows(&pg, "SELECT @@tidb_current_ts")[0][0]
            .parse::<u64>()
            .unwrap(),
        start
    );
    assert_eq!(rows(&pg, "SELECT id,v FROM pg10_21_src"), initial[2..]);
    assert_eq!(rows(&pg, "SELECT id,v FROM pg10_21_dst"), returned);
    assert_eq!(rows(&observer, "SELECT id,v FROM pg10_21_src"), initial);
    assert!(rows(&observer, "SELECT id,v FROM pg10_21_dst").is_empty());
    pg.execute("ROLLBACK").unwrap();
    assert_eq!(rows(&observer, "SELECT id,v FROM pg10_21_src"), initial);
    pg.execute("INSERT INTO pg10_21_dst VALUES(1,99)").unwrap();
    assert!(pg.execute("WITH moved AS (DELETE FROM pg10_21_src WHERE id<=2 RETURNING id,v) INSERT INTO pg10_21_dst SELECT id,v FROM moved RETURNING id,v").is_err());
    assert_eq!(rows(&observer, "SELECT id,v FROM pg10_21_src"), initial);
    // Other readers and sibling writes require independent snapshot evidence.
    assert!(pg.execute("WITH moved AS (DELETE FROM pg10_21_src WHERE id<=2 RETURNING id,v) SELECT id,v FROM moved").is_err());
    assert_eq!(rows(&observer, "SELECT id,v FROM pg10_21_src"), initial);
    assert_eq!(
        rows(
            &mysql_observer,
            "SELECT id,v FROM pg10_21_mysql ORDER BY id"
        ),
        mysql_initial
    );
    mysql.execute("COMMIT").unwrap();
    assert_eq!(
        rows(&mysql_observer, "SELECT v FROM pg10_21_mysql WHERE id=1"),
        vec![vec!["11".to_owned()]]
    );
    mysql.execute("BEGIN").unwrap();
    mysql
        .execute("UPDATE pg10_21_mysql SET v=12 WHERE id=1")
        .unwrap();
    mysql.execute("ROLLBACK").unwrap();
    assert_eq!(
        rows(&mysql_observer, "SELECT v FROM pg10_21_mysql WHERE id=1"),
        vec![vec!["11".to_owned()]]
    );
    let next = mysql
        .execute_protocol_statement(
            prepared,
            &[crate::runtime::ConcretePreparedArgument::Signed(2)],
        )
        .unwrap()[0]
        .next_row()
        .unwrap()
        .unwrap();
    assert_eq!(next, prepared_row);
    mysql.close_protocol_statement(prepared).unwrap();
    mysql.execute("DROP TABLE pg10_21_mysql").unwrap();
    for sql in ["DROP TABLE pg10_21_src", "DROP TABLE pg10_21_dst"] {
        pg.execute(sql).unwrap();
    }
}

thread_local! {
    static PG10_22_VIEWS: std::cell::RefCell<Vec<(&'static str,usize)>> = const { std::cell::RefCell::new(Vec::new()) };
}
pub(crate) fn pg10_22_record_view(branch: &'static str, id: usize) {
    PG10_22_VIEWS.with_borrow_mut(|events| events.push((branch, id)));
}
#[test]
fn pg10_22_shared_statement_view() {
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    let rows = |sql: &str| {
        let mut sets = pg.execute(sql).unwrap();
        let mut rows = Vec::new();
        while let Some(row) = sets[0].next_row().unwrap() {
            rows.push(row);
        }
        rows.sort();
        rows
    };
    for sql in [
        "CREATE TABLE pg10_22_src(id INT PRIMARY KEY,v INT)",
        "CREATE TABLE pg10_22_dst(id INT PRIMARY KEY,v INT)",
        "INSERT INTO pg10_22_src VALUES(1,10),(2,NULL),(3,30)",
    ] {
        pg.execute(sql).unwrap();
    }
    let initial = rows("SELECT id,v FROM pg10_22_src");
    assert!(pg.execute("WITH moved AS (DELETE FROM pg10_22_src WHERE id<=2 RETURNING id,v),seen AS (SELECT id,v FROM pg10_22_dst WHERE id<=2) INSERT INTO pg10_22_dst SELECT seen.id,seen.v FROM seen JOIN moved ON seen.id=moved.id RETURNING id,v").is_err());
    assert_eq!(rows("SELECT id,v FROM pg10_22_src"), initial);
    assert!(rows("SELECT id,v FROM pg10_22_dst").is_empty());
    for ending in ["ROLLBACK", "COMMIT"] {
        pg.execute("BEGIN").unwrap();
        // This earlier transaction write must be in the statement view.
        pg.execute("UPDATE pg10_22_src SET v=11 WHERE id=1")
            .unwrap();
        PG10_22_VIEWS.with_borrow_mut(Vec::clear);
        let returned = rows(
            "WITH moved AS (DELETE FROM pg10_22_src WHERE id<=2 RETURNING id,v),seen AS (SELECT id,v FROM pg10_22_src WHERE id<=2) INSERT INTO pg10_22_dst SELECT seen.id,seen.v FROM seen JOIN moved ON seen.id=moved.id RETURNING id,v",
        );
        let events = PG10_22_VIEWS.with_borrow_mut(std::mem::take);
        eprintln!("pg10_22 immutable source view identities: {events:?}");
        assert!(events.iter().any(|e| e.0 == "delete"));
        assert!(events.iter().any(|e| e.0 == "select"));
        assert!(events.iter().all(|e| e.1 == events[0].1 && e.1 != 0));
        assert_eq!(returned[0], vec!["1".to_owned(), "11".to_owned()]);
        assert_eq!(returned.len(), 2);
        assert_eq!(
            rows("SELECT id,v FROM pg10_22_src"),
            vec![vec!["3".to_owned(), "30".to_owned()]]
        );
        pg.execute(ending).unwrap();
    }
    for sql in ["DROP TABLE pg10_22_src", "DROP TABLE pg10_22_dst"] {
        pg.execute(sql).unwrap();
    }
}

#[test]
fn pg10_23_single_complete_producer() {
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    let rows = |sql: &str| {
        let mut sets = pg.execute(sql).unwrap();
        let mut rows = Vec::new();
        while let Some(row) = sets[0].next_row().unwrap() {
            rows.push(row);
        }
        rows.sort();
        rows
    };
    for sql in [
        "CREATE TABLE pg10_23_src(id INT PRIMARY KEY,v INT)",
        "CREATE TABLE pg10_23_dst(id INT PRIMARY KEY,v INT)",
    ] {
        pg.execute(sql).unwrap();
    }
    for ending in ["ROLLBACK", "COMMIT"] {
        for bound in [0, 2] {
            for sql in [
                "DELETE FROM pg10_23_dst",
                "DELETE FROM pg10_23_src",
                "INSERT INTO pg10_23_src VALUES(1,10),(2,NULL),(3,30)",
            ] {
                pg.execute(sql).unwrap();
            }
            let initial = rows("SELECT id,v FROM pg10_23_src");
            pg.execute("BEGIN").unwrap();
            PG10_21_MOVE_EVENTS.with_borrow_mut(|events| *events = Some(vec![]));
            let returned = rows(&format!(
                "WITH moved AS (DELETE FROM pg10_23_src WHERE id<={bound} RETURNING id,v) INSERT INTO pg10_23_dst SELECT a.id,a.v FROM moved AS a JOIN moved AS b ON a.id=b.id RETURNING id,v"
            ));
            let events = PG10_21_MOVE_EVENTS.with_borrow_mut(Option::take).unwrap();
            eprintln!("pg10_23 producer events {ending}/{bound}: {events:?}");
            let starts = events
                .iter()
                .filter(|e| e.0 == "delete")
                .collect::<Vec<_>>();
            assert_eq!(starts.len(), 1);
            assert_ne!(starts[0].1, 0);
            let completed = events
                .iter()
                .filter(|e| e.0 == "delete-complete")
                .collect::<Vec<_>>();
            assert_eq!(completed.len(), 1);
            assert_eq!(completed[0].1, bound);
            let expected = if bound == 2 {
                initial[..2].to_vec()
            } else {
                vec![]
            };
            assert_eq!(returned, expected);
            assert_eq!(rows("SELECT id,v FROM pg10_23_dst"), expected);
            assert_eq!(
                rows("SELECT id,v FROM pg10_23_src"),
                if bound == 2 {
                    initial[2..].to_vec()
                } else {
                    initial.clone()
                }
            );
            pg.execute(ending).unwrap();
            assert_eq!(
                rows("SELECT id,v FROM pg10_23_dst"),
                if ending == "COMMIT" { expected } else { vec![] }
            );
            assert_eq!(
                rows("SELECT id,v FROM pg10_23_src"),
                if ending == "COMMIT" && bound == 2 {
                    initial[2..].to_vec()
                } else {
                    initial
                }
            );
        }
    }
    for sql in ["DROP TABLE pg10_23_src", "DROP TABLE pg10_23_dst"] {
        pg.execute(sql).unwrap();
    }
}

#[test]
fn pg10_25_cancel_move_boundaries() {
    use astersql_parser_ast::SqlDialect;
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), SqlDialect::PostgreSQL);
    let observer = ConcreteSession::new_with_dialect(domain, SqlDialect::PostgreSQL);
    let rows = |session: &ConcreteSession, sql: &str| {
        let mut sets = session.execute(sql).unwrap();
        let mut result = Vec::new();
        while let Some(row) = sets[0].next_row().unwrap() {
            result.push(row);
        }
        result.sort();
        result
    };
    for sql in [
        "CREATE TABLE pg10_25_src(id INT PRIMARY KEY,v INT)",
        "CREATE TABLE pg10_25_dst(id INT PRIMARY KEY,v INT)",
    ] {
        pg.execute(sql).unwrap();
    }
    for boundary in ["produce", "consume"] {
        for explicit in [false, true] {
            for sql in [
                "DELETE FROM pg10_25_dst",
                "DELETE FROM pg10_25_src",
                "INSERT INTO pg10_25_src VALUES(1,10),(2,NULL),(3,30)",
            ] {
                pg.execute(sql).unwrap();
            }
            let scan_src = "SELECT id,v FROM pg10_25_src ORDER BY id";
            let scan_dst = "SELECT id,v FROM pg10_25_dst ORDER BY id";
            let initial = rows(&pg, scan_src);
            if explicit {
                pg.execute("BEGIN").unwrap();
                pg.execute("INSERT INTO pg10_25_src VALUES(9,90)").unwrap();
            }
            let killer = pg.cancellation_handle();
            let cancellation = astersql_testkit_testfailpoint::enable_call(
                &format!("astersql/session/pg_move_before_{boundary}"),
                move || killer.SendKillSignal(astersql_util_memory::sqlkiller::QueryInterrupted),
            );
            let result = pg.execute("WITH moved AS (DELETE FROM pg10_25_src WHERE id<=2 RETURNING id,v) INSERT INTO pg10_25_dst SELECT id,v FROM moved RETURNING id,v");
            drop(cancellation);
            let error = result.err().expect("cancelled move must fail");
            assert!(
                error.to_string().contains("interrupted"),
                "{boundary}: {error}"
            );
            assert_eq!(rows(&observer, scan_src), initial);
            assert!(rows(&observer, scan_dst).is_empty());
            if explicit {
                pg.execute("ROLLBACK").unwrap();
            }
            for session in [&pg, &observer] {
                assert_eq!(rows(session, scan_src), initial);
                assert!(rows(session, scan_dst).is_empty());
                session.execute("SELECT 1").unwrap();
            }
            // The next real move proves the CTE scope/snapshot and cancellation
            // signal were cleaned up rather than merely hidden by readback.
            let moved = rows(
                &pg,
                "WITH moved AS (DELETE FROM pg10_25_src WHERE id<=2 RETURNING id,v) INSERT INTO pg10_25_dst SELECT id,v FROM moved RETURNING id,v",
            );
            assert_eq!(moved, initial[..2]);
            assert_eq!(rows(&observer, scan_src), initial[2..]);
            assert_eq!(rows(&observer, scan_dst), moved);
        }
    }
    for sql in ["DROP TABLE pg10_25_src", "DROP TABLE pg10_25_dst"] {
        pg.execute(sql).unwrap();
    }
}

#[test]
fn pg10_28_typed_returning_description_does_not_write() {
    use crate::runtime::ConcretePreparedArgument;
    let (domain, setup) = CreateAnalyzeSession().unwrap();
    setup.execute("CREATE DATABASE pg10_28_shared").unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("USE pg10_28_shared").unwrap();
    pg.execute("CREATE TABLE t(id integer PRIMARY KEY,v integer,note text)")
        .unwrap();
    pg.execute("INSERT INTO t VALUES(1,10,'before')").unwrap();
    let (id, count, fields) = pg.prepare_protocol_statement_with_parameter_types(
        "UPDATE t AS r SET v=v+?,note=? WHERE id=1 RETURNING r.id,r.v AS \"After\",r.note,r.v+? AS projected",
        &[23,25,23],
    ).unwrap();
    assert_eq!(count, 3);
    assert_eq!(
        fields
            .iter()
            .map(|f| f.column.FieldType.GetType())
            .collect::<Vec<_>>(),
        [3, 3, 252, 3]
    );
    let mut rows = pg.execute("SELECT id,v,note FROM t").unwrap().remove(0);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["1", "10", "before"]);
    pg.execute("BEGIN").unwrap();
    let mut sets = pg
        .execute_protocol_statement(
            id,
            &[
                ConcretePreparedArgument::Signed(1),
                ConcretePreparedArgument::Null,
                ConcretePreparedArgument::Signed(100),
            ],
        )
        .unwrap();
    assert_eq!(
        sets[0].next_row().unwrap().unwrap(),
        ["1", "11", "<nil>", "111"]
    );
    pg.execute("ROLLBACK").unwrap();
    let mut rows = pg.execute("SELECT id,v,note FROM t").unwrap().remove(0);
    assert_eq!(rows.next_row().unwrap().unwrap(), ["1", "10", "before"]);
    pg.close_protocol_statement(id).unwrap();
    pg.execute("DROP DATABASE pg10_28_shared").unwrap();
}

#[test]
fn pg10_29_not_null_conflict_does_not_swallow_or_write() {
    let (domain, _) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain, astersql_parser_ast::SqlDialect::PostgreSQL);
    pg.execute("CREATE TABLE pg10_29_nn(id integer PRIMARY KEY,v integer NOT NULL)")
        .unwrap();
    pg.execute("INSERT INTO pg10_29_nn VALUES(1,10)").unwrap();
    for target in ["", "(id)"] {
        let error = pg
            .execute(&format!(
                "INSERT INTO pg10_29_nn VALUES(2,20),(1,NULL) ON CONFLICT {target} DO NOTHING"
            ))
            .err()
            .expect("NOT NULL precedes conflict skip");
        assert_eq!(error.pg_array_error().unwrap().state, "23502");
        let mut result = pg
            .execute("SELECT id,v FROM pg10_29_nn ORDER BY id")
            .unwrap()
            .remove(0);
        assert_eq!(result.next_row().unwrap().unwrap(), ["1", "10"]);
        assert!(result.next_row().unwrap().is_none());
    }
    pg.execute("DROP TABLE pg10_29_nn").unwrap();
}

#[test]
fn pg11_08_exact_column_identity() {
    use crate::runtime::ConcretePreparedArgument;
    use astersql_meta_model::{DecodeTableInfo, EncodeTableInfo};
    use astersql_parser_ast::SqlDialect;
    fn row(session: &ConcreteSession, sql: &str) -> Vec<String> {
        let mut result = session
            .execute(sql)
            .unwrap_or_else(|e| panic!("{sql}: {e}"));
        let value = result[0].next_row().unwrap().unwrap();
        assert!(result[0].next_row().unwrap().is_none());
        result[0].close().unwrap();
        value
    }
    let (domain, mysql) = CreateAnalyzeSession().unwrap();
    let pg = ConcreteSession::new_with_dialect(domain.clone(), SqlDialect::PostgreSQL);
    let mysql_observer = ConcreteSession::new(domain.clone());
    mysql
        .execute("CREATE TABLE mysql_pg1108 (id INT PRIMARY KEY,c INT)")
        .unwrap();
    mysql
        .execute("INSERT INTO mysql_pg1108 VALUES (1,30)")
        .unwrap();
    let saved_mode = row(&mysql, "SELECT @@SESSION.sql_mode")[0].clone();
    let (mysql_statement, count, _) = mysql
        .prepare_protocol_statement("SELECT C FROM mysql_pg1108 WHERE id=?")
        .unwrap();
    assert_eq!(count, 1);
    assert!(
        mysql
            .execute("CREATE TABLE mysql_pg1108_duplicate (c INT,`C` INT)")
            .is_err()
    );
    mysql
        .execute("CREATE TABLE mysql_pg1108_legacy (id INT PRIMARY KEY,C INT)")
        .unwrap();
    mysql
        .execute("INSERT INTO mysql_pg1108_legacy VALUES (1,42)")
        .unwrap();
    assert_eq!(row(&pg, r#"SELECT "C" FROM mysql_pg1108_legacy"#), ["42"]);
    assert!(pg.execute("SELECT c FROM mysql_pg1108_legacy").is_err());
    let legacy = domain.table_by_name("test", "mysql_pg1108_legacy").unwrap();
    assert_eq!(
        (
            legacy.Columns[1].Name.O.as_str(),
            legacy.Columns[1].Name.L.as_str()
        ),
        ("C", "c")
    );
    assert_eq!(
        row(&mysql, "SELECT c,C FROM mysql_pg1108_legacy"),
        ["42", "42"]
    );

    mysql.execute("BEGIN").unwrap();
    mysql
        .execute("UPDATE mysql_pg1108 SET C=31 WHERE id=1")
        .unwrap();
    pg.execute(r#"CREATE TABLE pg1108_bound (id integer PRIMARY KEY,c integer,"C" integer)"#)
        .unwrap();
    pg.execute(r#"INSERT INTO pg1108_bound (id,"C",c) VALUES (1,11,10)"#)
        .unwrap();
    domain.force_full_reload_for_test().unwrap();
    let table = domain.table_by_name("test", "pg1108_bound").unwrap();
    let table = DecodeTableInfo(&EncodeTableInfo(&table).unwrap()).unwrap();
    assert!(table.ExactName);
    assert_eq!(
        table
            .Columns
            .iter()
            .map(|c| (c.ID, c.Offset, c.Name.O.as_str(), c.Name.L.as_str()))
            .collect::<Vec<_>>(),
        [(1, 0, "id", "id"), (2, 1, "c", "c"), (3, 2, "C", "C")]
    );
    assert_eq!(astersql_parser_ast::NewCIStr("C").L, "c");
    let (statement, count, fields) = pg
        .prepare_protocol_statement(r#"SELECT c,"C" FROM pg1108_bound WHERE id=?"#)
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        fields
            .iter()
            .map(|f| (
                f.column_as_name.O.as_str(),
                f.column_origin.unwrap().table_id,
                f.column_origin.unwrap().attribute_number
            ))
            .collect::<Vec<_>>(),
        [("c", table.ID, 2), ("C", table.ID, 3)]
    );
    pg.execute(r#"UPDATE pg1108_bound SET "C"=21 WHERE id=1"#)
        .unwrap();
    let mut result = pg
        .execute_protocol_statement(statement, &[ConcretePreparedArgument::Signed(1)])
        .unwrap();
    assert_eq!(result[0].next_row().unwrap().unwrap(), ["10", "21"]);
    assert!(result[0].next_row().unwrap().is_none());
    result[0].close().unwrap();
    for bad in [
        r#"UPDATE pg1108_bound SET "cC"=99 WHERE id=99"#,
        r#"INSERT INTO pg1108_bound (id,"cC") VALUES (2,99)"#,
    ] {
        assert!(pg.execute(bad).is_err(), "{bad}");
        assert_eq!(row(&pg, r#"SELECT c,"C" FROM pg1108_bound"#), ["10", "21"]);
    }
    for mode in ["", "ANSI_QUOTES"] {
        mysql
            .execute(&format!("SET SESSION sql_mode='{mode}'"))
            .unwrap();
        assert_eq!(row(&mysql, "SELECT c,C FROM mysql_pg1108"), ["31", "31"]);
        assert_eq!(
            row(&mysql_observer, "SELECT c,C FROM mysql_pg1108"),
            ["30", "30"]
        );
        assert_eq!(
            row(&mysql, r#"SELECT "c" FROM mysql_pg1108"#),
            [if mode.is_empty() { "c" } else { "31" }]
        );
        assert_eq!(row(&pg, r#"SELECT c,"C" FROM pg1108_bound"#), ["10", "21"]);
        let mut result = mysql
            .execute_protocol_statement(mysql_statement, &[ConcretePreparedArgument::Signed(1)])
            .unwrap();
        assert_eq!(result[0].next_row().unwrap().unwrap(), ["31"]);
        result[0].close().unwrap();
    }
    mysql
        .execute(&format!(
            "SET SESSION sql_mode='{}'",
            saved_mode.replace('\'', "''")
        ))
        .unwrap();
    assert_eq!(row(&mysql, "SELECT @@SESSION.sql_mode"), [saved_mode]);
    mysql.execute("ROLLBACK").unwrap();
    assert_eq!(row(&mysql, "SELECT C FROM mysql_pg1108"), ["30"]);
    pg.close_protocol_statement(statement).unwrap();
    mysql.close_protocol_statement(mysql_statement).unwrap();
    pg.execute("DROP TABLE pg1108_bound").unwrap();
    mysql.execute("DROP TABLE mysql_pg1108").unwrap();
    mysql.execute("DROP TABLE mysql_pg1108_legacy").unwrap();
}

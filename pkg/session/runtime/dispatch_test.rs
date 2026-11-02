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

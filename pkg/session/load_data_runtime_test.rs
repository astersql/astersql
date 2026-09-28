// Copyright 2026 AsterSQL.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::runtime::{ConcreteSession, CreateAnalyzeSession};
use crate::testutil::TestRecordSet;

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempFile(PathBuf);

impl TempFile {
    fn new(contents: &[u8]) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "astersql_load_data_runtime_{}_{}.csv",
            std::process::id(),
            sequence
        ));
        std::fs::write(&path, contents).expect("write LOAD DATA fixture");
        Self(path)
    }

    fn sql_path(&self) -> String {
        self.0
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('\'', "\\'")
    }

    fn unused(stem: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "astersql_{stem}_{}_{}.csv",
            std::process::id(),
            sequence
        ));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn session() -> ConcreteSession {
    CreateAnalyzeSession().expect("create LOAD DATA session").1
}

fn execute(session: &ConcreteSession, sql: &str) {
    session.execute(sql).expect(sql);
}

fn rows(session: &ConcreteSession, sql: &str) -> Vec<Vec<String>> {
    let mut result_sets = session.execute(sql).expect(sql);
    let mut result = result_sets.pop().expect("query record set");
    let mut rows = Vec::new();
    while let Some(row) = result.Next().expect("query row") {
        rows.push(row);
    }
    rows
}

fn execute_error(session: &ConcreteSession, sql: &str) -> String {
    match session.execute(sql) {
        Ok(_) => panic!("expected statement to fail: {sql}"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn local_infile_reads_client_file_but_server_infile_is_rejected() {
    let fixture = TempFile::new(b"1\tone\n2\ttwo\n");
    let session = session();
    execute(&session, "use test");
    execute(
        &session,
        "create table t (id int primary key, value varchar(20))",
    );

    execute(
        &session,
        &format!(
            "load data local infile '{}' into table t",
            fixture.sql_path()
        ),
    );
    assert_eq!(
        rows(&session, "select id, value from t order by id"),
        vec![
            vec![String::from("1"), String::from("one")],
            vec![String::from("2"), String::from("two")]
        ]
    );

    let error = execute_error(
        &session,
        &format!("load data infile '{}' into table t", fixture.sql_path()),
    );
    assert!(
        error
            .to_string()
            .contains("Don't support load data from tidb-server's disk."),
        "unexpected server-file error: {error}"
    );
}

#[test]
fn missing_local_infile_reports_the_path_error() {
    let missing = std::env::temp_dir().join(format!(
        "astersql_missing_load_data_{}_{}.csv",
        std::process::id(),
        TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&missing);
    let fixture = TempFile(missing);
    let session = session();
    execute(&session, "use test");
    execute(&session, "create table t (id int)");

    let error = execute_error(
        &session,
        &format!(
            "load data local infile '{}' into table t",
            fixture.sql_path()
        ),
    );
    assert!(
        error.contains("No such file or directory") || error.contains("os error 2"),
        "unexpected missing-file error: {error}"
    );
}

#[test]
fn local_infile_set_assignments_resolve_case_insensitive_user_variables() {
    let fixture = TempFile::new(b"1,2,3\n4,5,6\n");
    let session = session();
    execute(&session, "use test");
    execute(&session, "create table t (c1 int, c2 int, c3 int)");

    for (first, second) in [("@val1", "@val2"), ("@VAL1", "@VAL2")] {
        execute(
            &session,
            &format!(
                "load data local infile '{}' into table t fields terminated by ',' (c1, {first}, {second}) set c3 = {second} * 100, c2 = cast({first} as unsigned)",
                fixture.sql_path()
            ),
        );
        assert_eq!(
            rows(&session, "select * from t order by c1"),
            vec![
                vec![String::from("1"), String::from("2"), String::from("300")],
                vec![String::from("4"), String::from("5"), String::from("600")]
            ]
        );
        execute(&session, "delete from t");
    }
}

#[test]
fn local_infile_rejects_views_and_sequences_as_non_updatable_targets() {
    let fixture = TempFile::new(b"1\n");
    let session = session();
    execute(&session, "use test");
    execute(&session, "create view v1 as select 1");
    execute(&session, "create sequence s1");

    for target in ["v1", "s1"] {
        let error = execute_error(
            &session,
            &format!(
                "load data local infile '{}' into table {target}",
                fixture.sql_path()
            ),
        );
        assert!(
            error.to_string().contains(&format!(
                "target table {target} of the LOAD is not updatable"
            )),
            "unexpected {target} error: {error}"
        );
    }
}

#[test]
fn local_infile_integer_prefixes_participate_in_duplicate_key_checks() {
    let fixture = TempFile::new(b"\n1abc\n");
    let session = session();
    execute(&session, "use test");
    execute(
        &session,
        "create table t (id int not null auto_increment primary key)",
    );

    execute(
        &session,
        &format!(
            "load data local infile '{}' into table t",
            fixture.sql_path()
        ),
    );
    let state = session.protocol_state();
    assert_eq!(state.affected_rows, 1);
    assert_eq!(state.last_insert_id, 1);
    assert_eq!(
        rows(&session, "select * from t"),
        vec![vec![String::from("1")]]
    );
}

#[test]
fn local_infile_ignore_lines_skips_raw_header_before_csv_quote_parsing() {
    let fixture = TempFile::new(b"\"a,b,c\n\"1\",2,\"3\"\n");
    let session = session();
    execute(&session, "use test");
    execute(
        &session,
        "create table t (id int primary key, b int, c text)",
    );

    let sql = format!(
        "load data local infile '{}' into table t fields terminated by ',' optionally enclosed by '\"' ignore 1 lines",
        fixture.sql_path()
    );
    let statement = astersql_parser::Parser::default()
        .ParseOneStmt(&sql, "", "")
        .expect("parse LOAD DATA statement");
    let load = statement
        .as_any()
        .downcast_ref::<astersql_parser_ast::LoadDataStmt>()
        .expect("LOAD DATA AST");
    assert_eq!(
        load.FieldsInfo
            .as_ref()
            .and_then(|fields| fields.Enclosed.as_deref()),
        Some("\"")
    );
    assert_eq!(load.IgnoreLines, Some(1));
    execute(&session, &sql);
    let warnings = rows(&session, "show warnings");
    assert_eq!(
        rows(&session, "select * from t"),
        vec![vec![
            String::from("1"),
            String::from("2"),
            String::from("3")
        ]],
        "warnings: {warnings:?}"
    );
}

#[test]
fn select_into_outfile_uses_load_data_compatible_defaults() {
    let outfile = TempFile::unused("select_into_outfile");
    let session = session();
    execute(&session, "use test");
    execute(&session, "create table t (id int, value varchar(20))");
    execute(
        &session,
        "insert into t values (1, 'one'), (2, 'two'), (null, null)",
    );

    execute(
        &session,
        &format!("select * from t into outfile '{}'", outfile.sql_path()),
    );
    assert_eq!(
        std::fs::read(&outfile.0).expect("read SELECT INTO OUTFILE result"),
        b"1\tone\n2\ttwo\n\\N\t\\N\n"
    );

    execute(&session, "create table t1 (id int, value varchar(20))");
    execute(
        &session,
        &format!(
            "load data local infile '{}' into table t1",
            outfile.sql_path()
        ),
    );
    assert_eq!(
        rows(&session, "select * from t1 order by id"),
        vec![
            vec![String::from("<nil>"), String::from("<nil>")],
            vec![String::from("1"), String::from("one")],
            vec![String::from("2"), String::from("two")]
        ]
    );
}

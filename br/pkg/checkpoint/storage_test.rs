// Copyright 2026 AsterSQL.

use std::sync::{Arc, Mutex};

use crate::storage::{dropCheckpointTables, initCheckpointTable};
use crate::stubs::{
    CIStr, Context, Domain, InfoSchema, MemStorage, RestrictedSQLExecutor, Result, Session, SqlRow,
    SqlValue, Storage, TableInfoName,
};

#[derive(Default)]
struct RecordingSession {
    calls: Arc<Mutex<Vec<(String, Vec<SqlValue>)>>>,
}

impl Session for RecordingSession {
    fn Close(&mut self) {}

    fn ExecuteInternal(&mut self, _ctx: &Context, sql: &str, args: &[SqlValue]) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((sql.to_string(), args.to_vec()));
        Ok(())
    }

    fn GetRestrictedSQLExecutor(&self) -> Arc<dyn RestrictedSQLExecutor> {
        Arc::new(EmptyRestricted)
    }
}

struct EmptyRestricted;

impl RestrictedSQLExecutor for EmptyRestricted {
    fn ExecRestrictedSQL(
        &self,
        _ctx: &Context,
        _sql: &str,
        _args: &[SqlValue],
    ) -> Result<Vec<SqlRow>> {
        Ok(vec![])
    }
}

struct EmptyInfoSchema;

impl InfoSchema for EmptyInfoSchema {
    fn TableExists(&self, _db: &CIStr, _table: &CIStr) -> bool {
        false
    }

    fn SchemaTableInfos(&self, _ctx: &Context, _db: &CIStr) -> Result<Vec<TableInfoName>> {
        Ok(vec![])
    }
}

struct EmptyDomain;

impl Domain for EmptyDomain {
    fn Store(&self) -> Arc<dyn Storage> {
        Arc::new(MemStorage::new())
    }

    fn InfoSchema(&self) -> Arc<dyn InfoSchema> {
        Arc::new(EmptyInfoSchema)
    }
}

fn assert_identifier_args(call: &(String, Vec<SqlValue>), sql: &str, expected: &[&str]) {
    assert_eq!(call.0, sql);
    let actual: Vec<&str> = call
        .1
        .iter()
        .map(|arg| match arg {
            SqlValue::Str(value) => value.as_str(),
            _ => panic!("identifier must be passed as a string argument"),
        })
        .collect();
    assert_eq!(actual, expected);
}

#[test]
fn ddl_identifiers_use_go_percent_n_binding() {
    let ctx = Context::Background();
    let mut session = RecordingSession::default();
    let calls = session.calls.clone();

    initCheckpointTable(&ctx, &mut session, "db-name", &["data-table"]).unwrap();
    dropCheckpointTables(&ctx, &EmptyDomain, &mut session, "db-name", &["data-table"]).unwrap();

    let calls = calls.lock().unwrap();
    assert_identifier_args(&calls[0], "CREATE DATABASE IF NOT EXISTS %n;", &["db-name"]);
    assert_identifier_args(
        &calls[2],
        "DROP TABLE IF EXISTS %n.%n;",
        &["db-name", "data-table"],
    );
    assert_identifier_args(&calls[3], "DROP DATABASE %n;", &["db-name"]);
}

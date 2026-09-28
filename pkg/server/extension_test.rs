// Copyright 2026 AsterSQL.

use crate::extension::{
    ClientConn, ConnEventInfo, ConnEventTp, ConnectionInfo, ExtensionListeners, PreparedMeta,
    SessionVars, Statement, StmtEventTp, TableEntry, stmtEventInfo,
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq)]
struct EventSnapshot {
    digest: (String, Option<String>),
    related_tables: Vec<TableEntry>,
}

#[derive(Default)]
struct Listener {
    events: Mutex<Vec<EventSnapshot>>,
}

impl ExtensionListeners for Listener {
    fn has_stmt_event_listeners(&self) -> bool {
        true
    }

    fn on_connection_event(&self, _: ConnEventTp, _: ConnEventInfo) {}

    fn on_stmt_event(&self, _: StmtEventTp, info: stmtEventInfo) {
        self.events.lock().unwrap().push(EventSnapshot {
            digest: info.SQLDigest(),
            related_tables: info.RelatedTables(),
        });
    }
}

fn client(listener: Arc<Listener>, vars: SessionVars) -> ClientConn {
    ClientConn {
        connection_info: ConnectionInfo::default(),
        session_vars: Some(vars),
        extensions: Some(listener),
    }
}

#[test]
fn prepared_execute_uses_cached_normalized_sql_and_digest() {
    let listener = Arc::new(Listener::default());
    let mut vars = SessionVars::default();
    vars.prepared.insert(
        7,
        PreparedMeta {
            statement: Statement::Sql {
                text: "select * from t where id = 42".into(),
                related_tables: vec![],
            },
            normalized_sql: "select * from `t` where `id` = ?".into(),
            digest: Some("prepared-digest".into()),
        },
    );

    crate::extension::onExtensionStmtEnd(
        &client(listener.clone(), vars),
        Statement::Execute { id: 7 },
        true,
        None,
        vec![],
    );

    assert_eq!(
        listener.events.lock().unwrap()[0].digest,
        (
            "select * from `t` where `id` = ?".into(),
            Some("prepared-digest".into())
        )
    );
}

#[test]
fn successful_statement_uses_stmt_context_tables_even_when_empty() {
    let listener = Arc::new(Listener::default());
    let vars = SessionVars {
        current_db: "current".into(),
        ..SessionVars::default()
    };

    crate::extension::onExtensionStmtEnd(
        &client(listener.clone(), vars),
        Statement::Sql {
            text: "select * from fallback".into(),
            related_tables: vec![TableEntry {
                db: String::new(),
                table: "fallback".into(),
            }],
        },
        true,
        None,
        vec![],
    );

    assert!(listener.events.lock().unwrap()[0].related_tables.is_empty());
}

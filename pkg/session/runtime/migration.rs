// Copyright 2026 AsterSQL.

//! The read-timestamp/system-variable slice of Go session-state migration.
//! Storage, transaction consumption, SQL parsing and JSON are existing implementations.
use super::session::RuntimeTimeZone;
use super::*;
use chrono::TimeZone;

impl ConcreteSession {
    pub(super) fn parse_session_read_datetime(&self, value: &str) -> SessionResult<i64> {
        let naive = chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.f")
            .map_err(|error| session_error("parse read timestamp", error))?;
        let local = match *self.time_zone.borrow() {
            RuntimeTimeZone::Named(zone) => zone
                .from_local_datetime(&naive)
                .map(|time| time.fixed_offset()),
            RuntimeTimeZone::Fixed(zone) => zone.from_local_datetime(&naive),
        };
        // Go selects the first occurrence of a repeated wall clock. Export checks
        // exact round-trip equality so the second occurrence cannot silently move.
        local
            .earliest()
            .map(|time| time.timestamp_micros())
            .ok_or_else(|| SessionError::new("invalid timestamp in session time zone"))
    }

    pub(super) fn show_migration_system_vars(&self) -> SessionResult<ConcreteRecordSet> {
        let (snapshot, pending) = {
            let state = self.state.borrow();
            (
                state.snapshot_read_ts.unwrap_or_default(),
                state.pending_stale_read_ts.unwrap_or_default(),
            )
        };
        let zone = self
            .session_vars
            .GetHintSystemVar("time_zone")
            .map_err(|error| session_error("read migration time zone", error))?;
        let mut values = self
            .session_vars
            .GetEffectiveSessionStateSystemVars(snapshot, pending, &zone)
            .map_err(|error| {
                let error = astersql_sessionctx_sessionstates::ErrCannotMigrateSession
                    .GenWithStackByArgs(&[error.to_string().into()]);
                SessionError::with_source(error.to_string(), error)
            })?;
        // These values live in the concrete runtime rather than its hint cache.
        // Read them through the same production SELECT path used by clients.
        for name in [
            "character_set_connection",
            "collation_connection",
            "character_set_database",
            "collation_database",
            "character_set_server",
            "collation_server",
            "tidb_skip_isolation_level_check",
            "transaction_isolation",
        ] {
            values.insert(name.into(), self.select_variable(name, true)?);
        }
        let states = astersql_sessionctx_sessionstates::SessionStates {
            SystemVars: values,
            ..Default::default()
        };
        let json = serde_json::to_string(&states)
            .map_err(|error| session_error("encode session states", error))?;
        Ok(ConcreteRecordSet::new(
            vec!["Session_states".into()],
            vec![vec![json]],
        ))
    }

    pub(super) fn restore_migration_system_vars(&self, json: &str) -> SessionResult<()> {
        let states: astersql_sessionctx_sessionstates::SessionStates =
            serde_json::from_str(json)
                .map_err(|error| session_error("decode session states", error))?;
        // The existing SET executor owns validation, charset reset side effects,
        // snapshot catalog acquisition and one-shot timestamp consumption.
        for name in astersql_sessionctx_variable::OrderByDependency(&states.SystemVars) {
            let value = &states.SystemVars[&name];
            let sql = format!(
                "SET SESSION `{}`='{}'",
                name.replace('`', "``"),
                value.replace('\\', "\\\\").replace('\'', "''")
            );
            let mut parser = Parser::with_dialect(ast::SqlDialect::MySQL);
            let (nodes, _) = parser
                .ParseSQL(&sql, &[])
                .map_err(|error| session_error("parse restored system variable", error))?;
            let set = nodes
                .first()
                .and_then(|node| node.as_any().downcast_ref::<ast::SetStmt>())
                .ok_or_else(|| SessionError::new("invalid session-state variable assignment"))?;
            self.execute_set(set)?;
        }
        Ok(())
    }
}

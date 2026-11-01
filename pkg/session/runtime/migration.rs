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

    pub(super) fn encode_import_session_states(
        &self,
    ) -> SessionResult<astersql_sessionctx_sessionstates::SessionStates> {
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
        let mut states = astersql_sessionctx_sessionstates::SessionStates {
            SystemVars: values,
            ..Default::default()
        };
        let state = self.state.borrow();
        states.CurrentDB = state.current_database.clone();
        states.Status = astersql_executor_importer::QuerySessionStates::Status(self);
        for (name, value) in &state.user_variables {
            let datum = if state.string_user_variables.contains(name) {
                astersql_types::datum::NewStringDatum(value.clone())
            } else if let Ok(value) = value.parse::<i64>() {
                astersql_types::datum::NewIntDatum(value)
            } else if let Ok(value) = value.parse::<u64>() {
                astersql_types::datum::NewUintDatum(value)
            } else if let Ok(value) = value.parse::<f64>() {
                astersql_types::datum::NewFloat64Datum(value)
            } else {
                astersql_types::datum::NewStringDatum(value.clone())
            };
            let tp = match datum.Kind() {
                astersql_types::datum::KindInt64 | astersql_types::datum::KindUint64 => 8,
                astersql_types::datum::KindFloat64 => 5,
                _ => 253,
            };
            let mut field_type = astersql_parser_types::NewFieldType(tp);
            if datum.Kind() == astersql_types::datum::KindUint64 {
                field_type.AddFlag(astersql_parser_mysql::r#type::UnsignedFlag);
            }
            states
                .UserVarTypes
                .insert(name.clone(), Box::new(field_type));
            states.UserVars.insert(name.clone(), Box::new(datum));
        }
        for warning in &state.last_warnings {
            states.Warnings.push(astersql_util_context::SQLWarn {
                Level: warning.level.to_owned(),
                Err: Some(astersql_errors::New(warning.message.clone())),
            });
        }
        Ok(states)
    }

    pub(super) fn show_migration_system_vars(&self) -> SessionResult<ConcreteRecordSet> {
        self.CheckCanMigrateStates()?;
        let states = self.encode_import_session_states()?;
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

// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

//! TTL worker boundary over the executable system SQL session.

use astersql_ttl_ttlworker::session::{
    Datum, ExpirationPredicate, PhysicalTable, Row, SessionError, SessionState, WorkerSession,
};
use chrono::Offset;

use super::{ConcreteSession, quote_argument};

/// Owns one executable session for a TTL scan or delete worker. The concrete
/// session is thread confined, so each worker must construct its own adapter.
pub struct TtlWorkerSqlSession {
    session: ConcreteSession,
    state: SessionState,
    reusable: bool,
    expiration: Option<(i64, u64, ExpirationPredicate)>,
}

impl TtlWorkerSqlSession {
    pub fn new(session: ConcreteSession) -> Self {
        Self {
            session,
            state: SessionState::default(),
            reusable: true,
            expiration: None,
        }
    }

    /// Scan and delete must carry the same captured frontier even if GLOBAL changes.
    pub(super) fn use_expiration(
        &mut self,
        table: &PhysicalTable,
        unix: u64,
        predicate: ExpirationPredicate,
    ) {
        self.expiration = Some((table.physical_id, unix, predicate));
    }

    pub fn reusable(&self) -> bool {
        self.reusable
    }
}

impl WorkerSession for TtlWorkerSqlSession {
    fn state(&self) -> &SessionState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut SessionState {
        &mut self.state
    }

    fn execute(&mut self, sql: &str, args: &[Datum]) -> Result<Vec<Row>, SessionError> {
        let sql = bind_ttl_parameters(sql, args)?;
        let result_sets = self
            .session
            .execute(&sql)
            .map_err(|error| SessionError::Execute(error.to_string()))?;
        let mut rows = Vec::new();
        for mut result_set in result_sets {
            while let Some(row) = result_set
                .next_row()
                .map_err(|error| SessionError::Execute(error.to_string()))?
            {
                rows.push(row.into_iter().map(Datum::Text).collect());
            }
        }
        Ok(rows)
    }

    fn refresh_state(&mut self) -> Result<(), SessionError> {
        for name in [
            "tidb_retry_limit",
            "tidb_enable_1pc",
            "tidb_enable_async_commit",
            "time_zone",
            "tidb_isolation_read_engines",
            "tidb_distsql_scan_concurrency",
            "tidb_enable_paging",
        ] {
            let rows = self.execute(&format!("SELECT @@{name}"), &[])?;
            let Some(Datum::Text(value)) = rows.first().and_then(|row| row.first()) else {
                return Err(SessionError::Execute(format!(
                    "failed to get {name} variable"
                )));
            };
            self.state.variables.insert(name.into(), value.clone());
        }
        let on = |v: &str| v.eq_ignore_ascii_case("ON") || v == "1";
        for name in ["tidb_enable_1pc", "tidb_enable_async_commit"] {
            let value = if on(&self.state.variables[name]) {
                "ON"
            } else {
                "OFF"
            };
            self.state.variables.insert(name.into(), value.into());
        }
        self.state.distsql_scan_concurrency = self.state.variables["tidb_distsql_scan_concurrency"]
            .parse()
            .map_err(|_| SessionError::Execute("invalid scan concurrency".into()))?;
        self.state.enable_paging = on(&self.state.variables["tidb_enable_paging"]);
        self.state.in_transaction = self.session.inner.state.borrow().transaction.is_some();
        Ok(())
    }

    fn expiration_predicate(
        &mut self,
        table: &PhysicalTable,
        unix: u64,
    ) -> Result<ExpirationPredicate, SessionError> {
        if let Some((id, frontier, predicate)) = &self.expiration {
            if *id == table.physical_id && *frontier == unix {
                return Ok(predicate.clone());
            }
        }
        // Fetch through SQL so global-variable lookup errors remain observable.
        let rows = self.execute("SELECT @@global.time_zone", &[])?;
        let Some(Datum::Text(zone)) = rows.first().and_then(|row| row.first()) else {
            return Err(SessionError::Execute(
                "get global time zone for TTL expiration condition".into(),
            ));
        };
        let zone = super::session::RuntimeTimeZone::parse(zone)
            .ok_or_else(|| SessionError::Execute("invalid global TTL time zone".into()))?;
        let (_, model) = self
            .session
            .inner
            .domain
            .stats_table(&table.schema, &table.table)
            .ok_or(SessionError::TableChanged)?;
        let column = model
            .Columns
            .iter()
            .find(|col| col.Name.O.eq_ignore_ascii_case(&table.ttl_column))
            .ok_or(SessionError::TableChanged)?;
        let instant = chrono::DateTime::from_timestamp(
            i64::try_from(unix).map_err(|_| {
                SessionError::Execute("TTL expiry is outside Unix time range".into())
            })?,
            0,
        )
        .ok_or_else(|| SessionError::Execute("TTL expiry is outside Unix time range".into()))?;
        let offset = match zone {
            super::session::RuntimeTimeZone::Named(zone) => {
                instant.with_timezone(&zone).offset().fix()
            }
            super::session::RuntimeTimeZone::Fixed(offset) => offset,
        };
        let predicate = if column.GetType() == astersql_parser_mysql::r#type::TypeTimestamp {
            ExpirationPredicate {
                expression: "FROM_UNIXTIME(%?)".into(),
                argument: Datum::Unsigned(unix),
            }
        } else {
            ExpirationPredicate {
                expression: "CAST(%? AS DATETIME)".into(),
                argument: Datum::Text({
                    let wall_clock = instant.with_timezone(&offset);
                    let value = wall_clock.format("%Y-%m-%d %H:%M:%S").to_string();
                    // DATE's midnight frontier can use a date-only CAST input:
                    // CAST still yields the identical DATETIME midnight, while
                    // the text-based runtime preserves the strict equality bound.
                    // Non-midnight frontiers retain the time so today's DATE can expire.
                    if column.GetType() == astersql_parser_mysql::r#type::TypeDate
                        && value.ends_with(" 00:00:00")
                    {
                        wall_clock.format("%Y-%m-%d").to_string()
                    } else {
                        value
                    }
                }),
            }
        };
        self.use_expiration(table, unix, predicate.clone());
        Ok(predicate)
    }

    fn execute_in_transaction(
        &mut self,
        sql: &str,
        args: &[Datum],
    ) -> Result<Vec<Row>, SessionError> {
        self.execute("BEGIN OPTIMISTIC", &[])?;
        match self.execute(sql, args) {
            Ok(rows) => {
                if let Err(error) = self.execute("COMMIT", &[]) {
                    self.avoid_reuse();
                    return Err(error);
                }
                Ok(rows)
            }
            Err(error) => {
                if self.execute("ROLLBACK", &[]).is_err() {
                    self.avoid_reuse();
                }
                Err(error)
            }
        }
    }

    fn avoid_reuse(&mut self) {
        self.reusable = false;
    }
}

fn bind_ttl_parameters(sql: &str, args: &[Datum]) -> Result<String, SessionError> {
    let mut result = String::with_capacity(sql.len() + args.len() * 8);
    let mut remaining = sql;
    for argument in args {
        let Some(position) = remaining.find("%?") else {
            return Err(SessionError::Execute("too many TTL SQL arguments".into()));
        };
        result.push_str(&remaining[..position]);
        let suffix = &remaining[position + 2..];
        // TTL SQL uses UTC sessions. Convert its Unix-second predicate here
        // until the general SQL runtime can evaluate FROM_UNIXTIME itself.
        if result.to_ascii_uppercase().ends_with("FROM_UNIXTIME(") && suffix.starts_with(')') {
            let seconds = match argument {
                Datum::Unsigned(value) => i64::try_from(*value).map_err(|_| {
                    SessionError::Execute("TTL expiry is outside Unix time range".into())
                })?,
                Datum::Integer(value) => *value,
                _ => {
                    return Err(SessionError::Execute(
                        "TTL expiry must be Unix seconds".into(),
                    ));
                }
            };
            let timestamp = chrono::DateTime::from_timestamp(seconds, 0).ok_or_else(|| {
                SessionError::Execute("TTL expiry is outside Unix time range".into())
            })?;
            result.truncate(result.len() - "FROM_UNIXTIME(".len());
            result.push_str(&quote_argument(
                &timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
            remaining = &suffix[1..];
            continue;
        }
        match argument {
            Datum::Null => result.push_str("NULL"),
            Datum::Integer(value) => result.push_str(&value.to_string()),
            Datum::Unsigned(value) => result.push_str(&value.to_string()),
            Datum::Text(value) => result.push_str(&quote_argument(value)),
            Datum::Bytes(value) => {
                result.push_str("X'");
                for byte in value {
                    use std::fmt::Write;
                    write!(result, "{byte:02X}").expect("writing to String cannot fail");
                }
                result.push('\'');
            }
        }
        remaining = suffix;
    }
    if remaining.contains("%?") {
        return Err(SessionError::Execute("too few TTL SQL arguments".into()));
    }
    result.push_str(remaining);
    Ok(result)
}

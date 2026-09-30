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

use astersql_ttl_ttlworker::session::{Datum, Row, SessionError, SessionState, WorkerSession};

use super::{ConcreteSession, quote_argument};

/// Owns one executable session for a TTL scan or delete worker. The concrete
/// session is thread confined, so each worker must construct its own adapter.
pub struct TtlWorkerSqlSession {
    session: ConcreteSession,
    state: SessionState,
    reusable: bool,
}

impl TtlWorkerSqlSession {
    pub fn new(session: ConcreteSession) -> Self {
        Self {
            session,
            state: SessionState::default(),
            reusable: true,
        }
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

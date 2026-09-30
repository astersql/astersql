// Copyright 2026 AsterSQL.

//! Thread-confined SQL sessions for the generic TTL table timer store.

use std::any::Any;
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};

use astersql_domain::Domain;
use astersql_session_syssession as syssession;
use astersql_session_syssession::SessionContext;
use astersql_timer_tablestore::{SqlArg, SqlCell, SqlResult, SqlRow};

use super::{ConcreteSession, quote_argument};

const BINARY_PREFIX: &str = "__astersql_binary_hex__:";

enum Command {
    Execute(String, mpsc::SyncSender<Result<Vec<Vec<String>>, String>>),
    Stop,
}

struct TimerSessionContext {
    sender: mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
    transaction_open: bool,
}

impl TimerSessionContext {
    fn new(domain: Arc<Domain>) -> syssession::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("ttl-timer-sql-session".into())
            .spawn(move || {
                let session = ConcreteSession::new(domain);
                while let Ok(command) = receiver.recv() {
                    match command {
                        Command::Execute(sql, reply) => {
                            let result = (|| {
                                let mut rows = Vec::new();
                                for mut record_set in
                                    session.execute(&sql).map_err(|error| error.to_string())?
                                {
                                    while let Some(row) =
                                        record_set.next_row().map_err(|error| error.to_string())?
                                    {
                                        rows.push(row);
                                    }
                                }
                                Ok(rows)
                            })();
                            let _ = reply.send(result);
                        }
                        Command::Stop => break,
                    }
                }
            })
            .map_err(|error| syssession::SessionError::new(error.to_string()))?;
        Ok(Self {
            sender,
            thread: Some(thread),
            transaction_open: false,
        })
    }

    fn query(&mut self, sql: &str) -> syssession::Result<Vec<SqlRow>> {
        let (reply, result) = mpsc::sync_channel(1);
        self.sender
            .send(Command::Execute(sql.to_owned(), reply))
            .map_err(|_| syssession::SessionError::new("TTL timer SQL worker closed"))?;
        let rows = result
            .recv()
            .map_err(|_| syssession::SessionError::new("TTL timer SQL worker dropped response"))?
            .map_err(syssession::SessionError::new)?;
        let upper = sql.trim_start().to_ascii_uppercase();
        if upper.starts_with("BEGIN") {
            self.transaction_open = true;
        } else if upper.starts_with("COMMIT") || upper.starts_with("ROLLBACK") {
            self.transaction_open = false;
        }
        rows.into_iter().map(|row| decode_row(sql, row)).collect()
    }
}

impl syssession::SessionContext for TimerSessionContext {
    fn close(&mut self) {
        let _ = self.sender.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
    fn on_became_owner(&mut self) -> syssession::Result<()> {
        Ok(())
    }
    fn on_resign_owner(&mut self) -> syssession::Result<()> {
        Ok(())
    }
    fn has_pending_transaction(&self) -> bool {
        self.transaction_open
    }
    fn rollback_transaction(&mut self) -> syssession::Result<()> {
        self.query("ROLLBACK")?;
        Ok(())
    }
    fn reset_state(&mut self) -> syssession::Result<()> {
        Ok(())
    }
    fn register_internal_session(&mut self) -> bool {
        true
    }
    fn unregister_internal_session(&mut self) {}
    fn execute(&mut self, sql: &str) -> syssession::Result<Vec<syssession::RecordSet>> {
        Ok(vec![Box::new(SqlResult {
            rows: self.query(sql)?,
        })])
    }
    fn execute_internal(
        &mut self,
        sql: &str,
        args: &[syssession::SqlValue],
    ) -> syssession::Result<syssession::RecordSet> {
        let sql = bind_parameters(sql, args)?;
        Ok(Box::new(SqlResult {
            rows: self.query(&sql)?,
        }))
    }
    fn execute_statement(
        &mut self,
        _statement: &dyn Any,
    ) -> syssession::Result<syssession::RecordSet> {
        Err(syssession::SessionError::new(
            "TTL timer session does not execute parsed statements",
        ))
    }
    fn parse_with_params(
        &mut self,
        _sql: &str,
        _args: &[syssession::SqlValue],
    ) -> syssession::Result<syssession::Statement> {
        Err(syssession::SessionError::new(
            "TTL timer session does not parse statements",
        ))
    }
    fn exec_restricted_statement(
        &mut self,
        _statement: &dyn Any,
    ) -> syssession::Result<Vec<syssession::Row>> {
        Err(syssession::SessionError::new(
            "TTL timer session does not execute restricted statements",
        ))
    }
    fn exec_restricted_sql(
        &mut self,
        _sql: &str,
        _args: &[syssession::SqlValue],
    ) -> syssession::Result<Vec<syssession::Row>> {
        Err(syssession::SessionError::new(
            "TTL timer session does not execute restricted SQL",
        ))
    }
}

impl Drop for TimerSessionContext {
    fn drop(&mut self) {
        self.close();
    }
}

/// Each pooled session owns a dedicated worker thread; calls on one lease stay
/// on that thread through BEGIN, reads, writes and COMMIT.
pub fn new_ttl_timer_session_pool(
    domain: Arc<Domain>,
    capacity: isize,
) -> Arc<syssession::AdvancedSessionPool> {
    Arc::new(syssession::NewAdvancedSessionPool(capacity, move || {
        Ok(Box::new(TimerSessionContext::new(Arc::clone(&domain))?))
    }))
}

fn bind_parameters(sql: &str, args: &[syssession::SqlValue]) -> syssession::Result<String> {
    let mut result = String::new();
    let mut remaining = sql;
    for arg in args {
        let position = remaining
            .find("%?")
            .ok_or_else(|| syssession::SessionError::new("too many TTL timer SQL arguments"))?;
        result.push_str(&remaining[..position]);
        let value = arg.downcast_ref::<SqlArg>().ok_or_else(|| {
            syssession::SessionError::new("unsupported TTL timer SQL argument type")
        })?;
        let suffix = &remaining[position + 2..];
        if result.to_ascii_uppercase().ends_with("FROM_UNIXTIME(") && suffix.starts_with(')') {
            let seconds = match value {
                SqlArg::I64(value) => *value,
                SqlArg::U64(value) => i64::try_from(*value).map_err(|_| {
                    syssession::SessionError::new("TTL timer timestamp outside Unix range")
                })?,
                _ => {
                    return Err(syssession::SessionError::new(
                        "TTL timer timestamp must be Unix seconds",
                    ));
                }
            };
            let timestamp = chrono::DateTime::from_timestamp(seconds, 0).ok_or_else(|| {
                syssession::SessionError::new("TTL timer timestamp outside Unix range")
            })?;
            result.truncate(result.len() - "FROM_UNIXTIME(".len());
            result.push_str(&quote_argument(
                &timestamp.format("%Y-%m-%d %H:%M:%S").to_string(),
            ));
            remaining = &suffix[1..];
            continue;
        }
        result.push_str(&match value {
            SqlArg::Null => "NULL".into(),
            SqlArg::String(value) | SqlArg::Json(value) => quote_argument(value),
            SqlArg::Bytes(value) => format!(
                "X'{}'",
                value
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<String>()
            ),
            SqlArg::Bool(value) => i32::from(*value).to_string(),
            SqlArg::I64(value) => value.to_string(),
            SqlArg::U64(value) => value.to_string(),
        });
        remaining = suffix;
    }
    if remaining.contains("%?") {
        return Err(syssession::SessionError::new(
            "too few TTL timer SQL arguments",
        ));
    }
    result.push_str(remaining);
    Ok(result)
}

fn decode_row(sql: &str, row: Vec<String>) -> syssession::Result<SqlRow> {
    let upper = sql.trim_start().to_ascii_uppercase();
    let full_timer_row = upper.starts_with("SELECT ID, NAMESPACE, TIMER_KEY, TIMER_DATA");
    let update_check = upper.starts_with("SELECT EVENT_ID, VERSION,");
    let scalar_number =
        upper.starts_with("SELECT @@LAST_INSERT_ID") || upper.starts_with("SELECT ROW_COUNT()");
    row.into_iter()
        .enumerate()
        .map(|(index, value)| {
            if value == "<nil>" {
                return Ok(SqlCell::Null);
            }
            if let Some(hex) = value.strip_prefix(BINARY_PREFIX) {
                let bytes = hex
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|pair| {
                        let hex = std::str::from_utf8(pair)
                            .map_err(|error| syssession::SessionError::new(error.to_string()))?;
                        u8::from_str_radix(hex, 16)
                            .map_err(|error| syssession::SessionError::new(error.to_string()))
                    })
                    .collect::<syssession::Result<Vec<_>>>()?;
                return Ok(SqlCell::Bytes(bytes));
            }
            if full_timer_row {
                if matches!(index, 0 | 18) {
                    return value
                        .parse::<u64>()
                        .map(SqlCell::U64)
                        .map_err(|error| syssession::SessionError::new(error.to_string()));
                }
                if index == 9 {
                    return value
                        .parse::<i64>()
                        .map(SqlCell::I64)
                        .map_err(|error| syssession::SessionError::new(error.to_string()));
                }
                if matches!(index, 8 | 14 | 16 | 17) {
                    let time = chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S")
                        .map_err(|error| {
                        syssession::SessionError::new(format!(
                            "decode TTL timer timestamp {value}: {error}"
                        ))
                    })?;
                    return Ok(SqlCell::Timestamp(time.and_utc().fixed_offset()));
                }
                if index == 10 {
                    return Ok(SqlCell::Json(value));
                }
                if matches!(index, 3 | 13 | 15) {
                    return Ok(SqlCell::Bytes(value.into_bytes()));
                }
            }
            if scalar_number || (update_check && index == 1) {
                return value
                    .parse::<u64>()
                    .map(SqlCell::U64)
                    .map_err(|error| syssession::SessionError::new(error.to_string()));
            }
            Ok(SqlCell::String(value))
        })
        .collect::<syssession::Result<Vec<_>>>()
        .map(SqlRow)
}

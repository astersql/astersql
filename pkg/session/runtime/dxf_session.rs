// Copyright 2026 AsterSQL.

//! Concrete SQL sessions for the DXF TaskManager. Each pool lease owns a worker
//! and one canonical session, so BEGIN/COMMIT/ROLLBACK span the same transaction.
use super::*;
use astersql_dxf_framework_storage as dxf;
use std::sync::mpsc;

struct Command {
    sql: String,
    alter_table_mode: Option<(i64, i64, astersql_meta_model::TableMode)>,
    read_txn_start_ts: bool,
    reply: mpsc::Sender<Result<dxf::SQLResult, dxf::Error>>,
}
struct Backend {
    sender: Mutex<Option<mpsc::Sender<Command>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}
fn sql_error(error: impl std::fmt::Display) -> dxf::Error {
    dxf::Error::new(error.to_string())
}

fn literal(value: dxf::Value) -> String {
    fn quoted(value: &str) -> String {
        format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
    }
    match value {
        dxf::Value::Null => "NULL".into(),
        dxf::Value::Int(value) | dxf::Value::Decimal(value) => value.to_string(),
        dxf::Value::U64(value) => value.to_string(),
        dxf::Value::String(value) | dxf::Value::Json(value) => quoted(&value),
        dxf::Value::Bytes(value) => format!(
            "x'{}'",
            value.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ),
        dxf::Value::Time(value) => {
            let time: chrono::DateTime<chrono::Utc> = value.into();
            quoted(&time.format("%Y-%m-%d %H:%M:%S%.6f").to_string())
        }
    }
}
fn bind(sql: &str, args: Vec<dxf::Value>) -> Result<String, dxf::Error> {
    let mut parts = sql.split("%?");
    let mut output = parts.next().unwrap_or_default().to_owned();
    let mut args = args.into_iter();
    for part in parts {
        output.push_str(&literal(
            args.next()
                .ok_or_else(|| sql_error("missing DXF SQL argument"))?,
        ));
        output.push_str(part);
    }
    if args.next().is_some() {
        return Err(sql_error("extra DXF SQL arguments"));
    }
    Ok(output)
}
fn cell(column: &str, value: String) -> dxf::Value {
    if value == SHOW_NULL_CELL {
        return dxf::Value::Null;
    }
    if let Some(bytes) = super::row_codec::binary_runtime_bytes(&value) {
        return dxf::Value::Bytes(bytes);
    }
    if matches!(
        column.rsplit('.').next().unwrap_or(column),
        "create_time" | "start_time" | "state_update_time" | "end_time"
    ) {
        if let Ok(time) = chrono::NaiveDateTime::parse_from_str(&value, "%Y-%m-%d %H:%M:%S%.f") {
            return dxf::Value::Time(time.and_utc().into());
        }
    }
    dxf::Value::String(value)
}
impl Backend {
    fn start(domain: Arc<Domain>) -> Arc<Self> {
        let (sender, receiver) = mpsc::channel::<Command>();
        let worker = std::thread::spawn(move || {
            let session = ConcreteSession::new(domain);
            while let Ok(command) = receiver.recv() {
                let result = (|| {
                    if command.read_txn_start_ts {
                        let timestamp = session
                            .state
                            .borrow()
                            .transaction
                            .as_ref()
                            .map(|transaction| transaction.StartTS())
                            .ok_or_else(|| sql_error("DXF transaction is not active"))?;
                        return Ok(dxf::SQLResult {
                            rows: vec![dxf::chunk::Row::new(vec![dxf::Value::U64(timestamp)])],
                            affected_rows: 0,
                        });
                    }
                    if let Some((schema_id, table_id, mode)) = command.alter_table_mode {
                        let schema = session.domain.info_schema().SchemaByID(schema_id);
                        let table = session.domain.info_schema().TableByID(table_id);
                        let changed = match (&schema, &table) {
                            (Some(schema), Some(table)) => session
                                .domain
                                .stats_table(&schema.name.lower, &table.Meta().name.lower)
                                .is_none_or(|(_, table)| table.Mode != mode),
                            _ => true,
                        };
                        let job_guard = if changed {
                            Some(RuntimeDdlJobGuard::new(begin_runtime_ddl_job(
                                &session.domain,
                                &schema
                                    .as_ref()
                                    .map_or(String::new(), |s| s.name.lower.clone()),
                                &table
                                    .as_ref()
                                    .map_or(String::new(), |t| t.Meta().name.lower.clone()),
                                "alter table mode",
                            )))
                        } else {
                            None
                        };
                        let ddl_result = session
                            .domain
                            .ddl_set_table_mode_by_ids(schema_id, table_id, mode);
                        if let Some(guard) = job_guard {
                            let result = ddl_result
                                .as_ref()
                                .map(|_| ())
                                .map_err(|error| session_error("alter table mode", error));
                            guard.finish(&result);
                        }
                        ddl_result.map_err(sql_error)?;
                        return Ok(dxf::SQLResult {
                            rows: Vec::new(),
                            affected_rows: 0,
                        });
                    }
                    let sets = session.execute(&command.sql).map_err(|error| {
                        sql_error(format!(
                            "DXF {}: {error}",
                            command
                                .sql
                                .split_whitespace()
                                .take(3)
                                .collect::<Vec<_>>()
                                .join(" ")
                        ))
                    })?;
                    let mut rows = Vec::new();
                    for set in sets {
                        rows.extend(set.rows.into_iter().map(|values| {
                            dxf::chunk::Row::new(
                                values
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, value)| cell(&set.columns[i], value))
                                    .collect(),
                            )
                        }));
                    }
                    let affected_rows = session
                        .state
                        .borrow()
                        .last_dml_report
                        .as_ref()
                        .map_or(0, |report| report.AffectedRows);
                    Ok(dxf::SQLResult {
                        rows,
                        affected_rows,
                    })
                })();
                let _ = command.reply.send(result);
            }
            let _ = session.execute("rollback");
        });
        Arc::new(Self {
            sender: Mutex::new(Some(sender)),
            worker: Mutex::new(Some(worker)),
        })
    }
}
impl dxf::SQLBackend for Backend {
    fn execute(&self, sql: &str, args: Vec<dxf::Value>) -> Result<dxf::SQLResult, dxf::Error> {
        let sql = bind(sql, args)?;
        let (reply, result) = mpsc::channel();
        self.sender
            .lock()
            .map_err(sql_error)?
            .as_ref()
            .ok_or_else(|| sql_error("DXF session closed"))?
            .send(Command {
                sql,
                alter_table_mode: None,
                read_txn_start_ts: false,
                reply,
            })
            .map_err(sql_error)?;
        result.recv().map_err(sql_error)?
    }

    fn alter_table_mode_for_import(&self, schema_id: i64, table_id: i64) -> Result<(), dxf::Error> {
        let (reply, result) = mpsc::channel();
        self.sender
            .lock()
            .map_err(sql_error)?
            .as_ref()
            .ok_or_else(|| sql_error("DXF session closed"))?
            .send(Command {
                sql: String::new(),
                alter_table_mode: Some((
                    schema_id,
                    table_id,
                    astersql_meta_model::TableMode::TableModeImport,
                )),
                read_txn_start_ts: false,
                reply,
            })
            .map_err(sql_error)?;
        result.recv().map_err(sql_error)??;
        Ok(())
    }
    fn alter_table_mode_for_normal(&self, schema_id: i64, table_id: i64) -> Result<(), dxf::Error> {
        let (reply, result) = mpsc::channel();
        self.sender
            .lock()
            .map_err(sql_error)?
            .as_ref()
            .ok_or_else(|| sql_error("DXF session closed"))?
            .send(Command {
                sql: String::new(),
                alter_table_mode: Some((
                    schema_id,
                    table_id,
                    astersql_meta_model::TableMode::TableModeNormal,
                )),
                read_txn_start_ts: false,
                reply,
            })
            .map_err(sql_error)?;
        result.recv().map_err(sql_error)??;
        Ok(())
    }
    fn txn_start_ts(&self) -> Result<u64, dxf::Error> {
        let (reply, result) = mpsc::channel();
        self.sender
            .lock()
            .map_err(sql_error)?
            .as_ref()
            .ok_or_else(|| sql_error("DXF session closed"))?
            .send(Command {
                sql: String::new(),
                alter_table_mode: None,
                read_txn_start_ts: true,
                reply,
            })
            .map_err(sql_error)?;
        let rows = result.recv().map_err(sql_error)??.rows;
        rows.first()
            .map(|row| row.GetUint64(0))
            .ok_or_else(|| sql_error("DXF transaction timestamp missing"))
    }
}
impl Drop for Backend {
    fn drop(&mut self) {
        self.sender.get_mut().unwrap().take();
        if let Some(worker) = self.worker.get_mut().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl ConcreteSession {
    /// Open the production TaskManager on this Domain's canonical KV storage.
    /// Fresh calls deliberately create fresh sessions; history survives their
    /// destruction because it is stored in the mysql task tables.
    pub fn ImportTaskManager(&self) -> SessionResult<dxf::TaskManager> {
        static SCHEMA: Mutex<()> = Mutex::new(());
        let _schema = SCHEMA.lock().unwrap();
        let setup = ConcreteSession::new(self.domain.clone());
        for sql in [
            astersql_meta_metadef::CreateTiDBGlobalTaskTable,
            astersql_meta_metadef::CreateTiDBGlobalTaskHistoryTable,
            astersql_meta_metadef::CreateDistFrameworkMetaTable,
            astersql_meta_metadef::CreateTiDBImportJobsTable,
            astersql_meta_metadef::CreateTiDBBackgroundSubtaskTable,
            astersql_meta_metadef::CreateTiDBBackgroundSubtaskHistoryTable,
        ] {
            let sql = if sql.starts_with("create table mysql.") {
                sql.replacen("create table ", "create table if not exists ", 1)
            } else {
                sql.to_owned()
            };
            setup.execute(&sql)?;
        }
        let domain = self.domain.clone();
        Ok(dxf::NewTaskManager(dxf::util::SessionPool::with_factory(
            move || {
                Ok(dxf::sessionctx::Context::with_backend(Backend::start(
                    domain.clone(),
                )))
            },
        )))
    }
}

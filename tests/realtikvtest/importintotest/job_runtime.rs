// Copyright 2026 AsterSQL.
//! File-import integration adapter. Execute the production SQL executor against
//! the suite's object/table storage, with a joined background worker and shared
//! job records shared by IMPORT, SHOW and CANCEL.

use super::*;
use std::sync::{Condvar, mpsc};

// This executor is runtime-generic and has no crate dependencies. Compile the
// production source directly, so this integration target exercises its actual
// submit/wait/cancel implementation without the unrelated executor crate graph.
#[path = "../../../pkg/executor/import_into.rs"]
mod executor;
use executor::{ImportExpression, ImportIntoRuntime};

#[derive(Debug)]
struct Error(String);
impl From<executor::UnsupportedImportFunction> for Error {
    fn from(value: executor::UnsupportedImportFunction) -> Self {
        Self(value.to_string())
    }
}
type Outcome = Arc<(Mutex<Option<Result<(), String>>>, Condvar)>;
struct Work {
    tk: testkit::TestKit,
    sql: String,
    id: i64,
    result: Outcome,
    cancel: Arc<Mutex<Option<bool>>>,
}
struct Worker {
    sender: mpsc::Sender<Work>,
    stop: Arc<AtomicBool>,
    handle: std::thread::JoinHandle<()>,
}
static WORKER: Mutex<Option<Worker>> = Mutex::new(None);

pub(super) fn start() {
    let mut slot = WORKER.lock().unwrap();
    if slot.is_some() {
        return;
    }
    let (sender, receiver) = mpsc::channel::<Work>();
    let stop = Arc::new(AtomicBool::new(false));
    let stopping = stop.clone();
    let handle = std::thread::spawn(move || {
        while !stopping.load(Ordering::SeqCst) {
            fire_call(
                "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/beforeGetSchedulableTasks",
                FailCtx::None,
            );
            let work = match receiver.recv_timeout(Duration::from_millis(10)) {
                Ok(work) => work,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            };
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                work.tk
                    .execute_import_job(&work.sql, work.id, &work.cancel)
                    .map(|_| ())
            }))
            .unwrap_or_else(|payload| {
                let msg = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "import worker panicked".into());
                Err(msg)
            });
            if let Err(error) = &outcome
                && error != "context canceled"
            {
                let mut e = eng();
                if let Some(j) = e.import_jobs.iter_mut().find(|j| j.id == work.id) {
                    if j.status != "cancelled" {
                        j.status = "failed".into();
                        j.error_message = error.clone();
                        j.summary_json.clear();
                    }
                }
                if let Some(task) = e.tasks.get_mut(&importinto::TaskKey(work.id)) {
                    task.State = proto::TaskStateReverted.into();
                }
            }
            *work.result.0.lock().unwrap() = Some(outcome);
            work.result.1.notify_all();
        }
    });
    *slot = Some(Worker {
        sender,
        stop,
        handle,
    });
}
pub(super) fn stop() {
    let worker = WORKER.lock().unwrap().take();
    if let Some(worker) = worker {
        worker.stop.store(true, Ordering::SeqCst);
        drop(worker.sender);
        worker.handle.join().expect("import scheduler panicked");
    }
}
pub(super) fn cancelled(id: i64) -> bool {
    eng()
        .import_jobs
        .iter()
        .any(|j| j.id == id && j.status == "cancelled")
}
pub(super) fn hook(path: &str, ctx: FailCtx) {
    fire_call(path, ctx);
    if let Some(term) = failpoint::term(path) {
        if let Some(ms) = term
            .strip_prefix("sleep(")
            .and_then(|v| v.strip_suffix(')'))
            .and_then(|v| v.parse::<u64>().ok())
        {
            std::thread::sleep(Duration::from_millis(ms));
        }
    }
}
pub(super) fn redact(uri: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(uri) else {
        return uri.into();
    };
    let pairs: Vec<_> = parsed
        .query_pairs()
        .map(|(k, v)| {
            let value = if matches!(k.as_ref(), "access-key" | "secret-access-key") {
                "xxxxxx".into()
            } else {
                v.into_owned()
            };
            (k.into_owned(), value)
        })
        .collect();
    if !pairs.is_empty() {
        parsed.query_pairs_mut().clear().extend_pairs(pairs);
    }
    parsed.into()
}

struct Expression;
impl ImportExpression for Expression {
    fn scalar_function_name(&self) -> Option<&str> {
        None
    }
    fn required_optional_properties(&self) -> u64 {
        0
    }
    fn children(&self) -> &[Self] {
        &[]
    }
}
struct Controller {
    uri: String,
    detached: bool,
    asynchronous: bool,
    size: i64,
}
struct Runtime {
    uri: String,
    tk: testkit::TestKit,
    work: Option<Work>,
    outcome: Outcome,
    cancel: Arc<Mutex<Option<bool>>>,
}
impl Runtime {
    fn dispatch(&mut self) {
        if let Some(work) = self.work.take() {
            start();
            WORKER
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .sender
                .send(work)
                .expect("scheduler stopped");
        }
    }
    fn submit(&mut self, controller: &Controller, sql: &str) -> Result<(i64, i64), Error> {
        let lower = sql.to_lowercase();
        let after = sql[lower
            .find(" into ")
            .ok_or_else(|| Error("missing INTO".into()))?
            + 6..]
            .trim();
        let name = after
            .split(|c: char| c.is_whitespace() || c == '(')
            .next()
            .unwrap();
        let (db, table) = name
            .split_once('.')
            .map(|(d, t)| (qident(d), qident(t)))
            .unwrap_or_else(|| (self.tk.cur_db(), qident(name)));
        let session = self.tk.Session();
        let keyspace = self.tk.keyspace_name();
        let mut e = eng();
        let table_id = e
            .tables
            .get(&self.tk.store.path)
            .and_then(|m| m.get(&table_key(&db, &table)))
            .ok_or_else(|| Error(format!("unknown table {db}.{table}")))?
            .id;
        let id = e.next_job_id;
        e.next_job_id += 1;
        let task_id = e.next_task_id;
        e.next_task_id += 1;
        let task_key = importinto::TaskKey(id);
        e.import_jobs.push(ImportJob {
            id,
            table_id,
            table_schema: db,
            table_name: table,
            summary_json: String::new(),
            keyspace,
            status: "pending".into(),
            step: String::new(),
            created_by: session.created_by(),
            file_location: redact(&controller.uri),
            format: importer::DataFormatCSV.into(),
            source_file_size: controller.size,
            error_message: String::new(),
            group_key: String::new(),
            columns_and_vars: String::new(),
            set_clause: String::new(),
        });
        e.tasks.insert(
            task_key.clone(),
            proto::Task {
                ID: task_id,
                Key: task_key,
                Type: proto::ImportInto.into(),
                State: proto::TaskStatePending.into(),
                Step: -1,
                RequiredSlots: 1,
                Meta: Vec::new(),
                MaxNodeCount: 1,
            },
        );
        if failpoint::is_enabled("github.com/pingcap/tidb/pkg/executor/importer/setLastImportJobID")
        {
            importer::TestLastImportJobID.store(id, Ordering::SeqCst);
        }
        if failpoint::is_enabled(
            "github.com/pingcap/tidb/pkg/dxf/framework/storage/testSetLastTaskID",
        ) {
            storage::TestLastTaskID.store(task_id, Ordering::SeqCst);
        }
        drop(e);
        self.work = Some(Work {
            tk: self.tk.clone(),
            sql: sql.into(),
            id,
            result: self.outcome.clone(),
            cancel: self.cancel.clone(),
        });
        Ok((id, id))
    }
}
impl ImportIntoRuntime for Runtime {
    type Context = ();
    type Request = Vec<Vec<String>>;
    type Plan = String;
    type Table = ();
    type SelectExecutor = ();
    type ImportPlan = String;
    type Assignment = ();
    type Expression = Expression;
    type Controller = Controller;
    type Task = i64;
    type JobInfo = ImportJob;
    type Error = Error;
    fn grow_and_reset_request(&self, r: &mut Self::Request) {
        r.clear();
    }
    fn create_import_plan(&mut self, _: &mut (), p: &String, _: &()) -> Result<String, Error> {
        Ok(p.clone())
    }
    fn column_assignments(&self, _: &String) -> Vec<()> {
        Vec::new()
    }
    fn encoding_optional_properties(&mut self, _: &String) -> Result<u64, Error> {
        Ok(0)
    }
    fn build_assignment_expression(&mut self, _: &String, _: &()) -> Result<Expression, Error> {
        Ok(Expression)
    }
    fn create_controller(&mut self, sql: String, _: &(), _: &String) -> Result<Controller, Error> {
        let lower = sql.to_lowercase();
        let pos = lower
            .find(" from ")
            .ok_or_else(|| Error("missing FROM".into()))?;
        let uri = extract_quoted(&sql[pos + 6..]).ok_or_else(|| Error("missing URI".into()))?;
        let asynchronous = importinto::ShouldUseAsyncPrepare(&importer::Plan {
            CloudStorageURI: extract_with_cloud_uri(&sql).unwrap_or_default(),
            ..Default::default()
        });
        self.uri = uri.clone();
        Ok(Controller {
            uri,
            detached: lower.contains("detached"),
            asynchronous,
            size: -1,
        })
    }
    fn should_use_async_prepare(&self, c: &Controller) -> bool {
        c.asynchronous
    }
    fn initialize_data_files(&mut self, _: &mut (), c: &mut Controller) -> Result<(), Error> {
        let (sizes, _) = self
            .tk
            .load_sources(&c.uri)
            .map_err(|e| Error(format!("{}: {e}", exeerrors::ErrLoadDataPreCheckFailed)))?;
        c.size = sizes.iter().sum::<usize>() as i64;
        if c.size == 0 {
            return Err(Error(format!(
                "{}: the file is empty",
                exeerrors::ErrLoadDataPreCheckFailed
            )));
        }
        Ok(())
    }
    fn next_generation_kernel(&self) -> bool {
        kerneltype::IsNextGen()
    }
    fn calculate_resource_parameters(
        &mut self,
        _: &mut (),
        _: &mut Controller,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn check_requirements_in_new_session(
        &mut self,
        _: &mut (),
        _: &mut Controller,
        _: bool,
    ) -> Result<(), Error> {
        Ok(())
    }
    fn initialize_tikv_configs(&mut self, _: &mut (), _: &mut Controller) -> Result<(), Error> {
        Ok(())
    }
    fn controller_is_detached(&self, c: &Controller) -> bool {
        c.detached
    }
    fn controller_path(&self, _: &Controller) -> &str {
        &self.uri
    }
    fn path_is_local(&self, p: &str) -> Result<bool, Error> {
        Ok(!p.contains("://"))
    }
    fn distributed_tasks_enabled(&self) -> bool {
        true
    }
    fn populate_chunks(&mut self, _: &mut (), _: &mut Controller) -> Result<(), Error> {
        Ok(())
    }
    fn submit_standalone_task(
        &mut self,
        _: &mut (),
        c: &Controller,
        s: &str,
        _: bool,
    ) -> Result<(i64, i64), Error> {
        self.submit(c, s)
    }
    fn submit_distributed_task(
        &mut self,
        _: &mut (),
        c: &Controller,
        s: &str,
    ) -> Result<(i64, i64), Error> {
        self.submit(c, s)
    }
    fn wait_task_done_or_paused(&mut self, _: &mut (), _: &i64) -> Result<(), Error> {
        self.dispatch();
        let mut result = self.outcome.0.lock().unwrap();
        loop {
            if self.cancel.lock().unwrap().unwrap_or(false) {
                return Err(Error("context canceled".into()));
            }
            if let Some(result) = result.as_ref() {
                return result.clone().map_err(Error);
            }
            result = self
                .outcome
                .1
                .wait_timeout(result, Duration::from_millis(10))
                .unwrap()
                .0;
        }
    }
    fn error_is_context_cancelled(&self, e: &Error) -> bool {
        e.0 == "context canceled"
    }
    fn cancel_and_wait_import_job_background(&mut self, id: i64) -> Result<(), Error> {
        self.tk
            .cancel_import_job("", &format!("cancel import job {id}"))
            .map_err(Error)?;
        let mut result = self.outcome.0.lock().unwrap();
        while result.is_none() {
            result = self.outcome.1.wait(result).unwrap();
        }
        Err(Error("context canceled".into()))
    }
    fn get_job_with_system_session(&mut self, _: &mut (), id: i64) -> Result<ImportJob, Error> {
        let job = eng()
            .import_jobs
            .iter()
            .find(|j| j.id == id)
            .cloned()
            .ok_or_else(|| Error(exeerrors::ErrLoadDataJobNotFound.into()))?;
        self.dispatch();
        Ok(job)
    }
    fn fill_one_job_info(&self, r: &mut Vec<Vec<String>>, j: &ImportJob) {
        r.push(j.to_result_row());
    }
    fn import_from_select_pipeline(
        &mut self,
        _: &mut (),
        _: &mut Controller,
        _: &mut (),
    ) -> Result<(), Error> {
        Err(Error("SELECT uses the suite SELECT executor".into()))
    }
    fn close_controller(&mut self, _: &mut Controller) {}
    fn close_base_executor(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn has_super_privilege(&self) -> bool {
        self.tk.Session().is_super
    }
    fn get_job_for_action(
        &mut self,
        _: &mut (),
        id: i64,
        superuser: bool,
    ) -> Result<ImportJob, Error> {
        let job = eng()
            .import_jobs
            .iter()
            .find(|j| j.id == id)
            .cloned()
            .ok_or_else(|| Error(exeerrors::ErrLoadDataJobNotFound.into()))?;
        if !superuser && job.created_by != self.tk.Session().created_by() {
            return Err(Error(plannererrors::ErrSpecificAccessDenied.into()));
        }
        Ok(job)
    }
    fn job_can_cancel(&self, j: &ImportJob) -> bool {
        matches!(j.status.as_str(), "pending" | "running")
    }
    fn invalid_cancel_operation(&self) -> Error {
        Error(exeerrors::ErrLoadDataInvalidOperation.into())
    }
    fn cancel_and_wait_import_job(&mut self, _: &mut (), id: i64) -> Result<(), Error> {
        self.tk
            .cancel_import_job("", &format!("cancel import job {id}"))
            .map_err(Error)
    }
}
pub(super) fn execute(tk: &testkit::TestKit, sql: &str) -> Result<testkit::ResultSet, String> {
    let cancel = Arc::new(Mutex::new(None));
    fire_call(
        "github.com/pingcap/tidb/pkg/executor/cancellableCtx",
        FailCtx::CancelFlag(cancel.clone()),
    );
    let runtime = Runtime {
        uri: String::new(),
        tk: tk.snapshot_session(),
        work: None,
        outcome: Arc::new((Mutex::new(None), Condvar::new())),
        cancel,
    };
    let mut exec = executor::newImportIntoExec(runtime, None, sql.into(), sql.into(), ());
    let mut rows = Vec::new();
    let result = exec.Next(&mut (), &mut rows).map_err(|e| e.0);
    let close = exec.Close().map_err(|e| e.0);
    result.and(close)?;
    Ok(testkit::ResultSet { rows })
}

pub(super) fn cancel(tk: &testkit::TestKit, job_id: i64) -> Result<(), String> {
    let runtime = Runtime {
        uri: String::new(),
        tk: tk.snapshot_session(),
        work: None,
        outcome: Arc::new((Mutex::new(None), Condvar::new())),
        cancel: Arc::new(Mutex::new(None)),
    };
    executor::ImportIntoActionExec {
        runtime,
        action: executor::ImportIntoAction::Cancel,
        job_id,
    }
    .Next(&mut (), &mut ())
    .map_err(|error| error.0)
}

/// Keep import mode and task registration alive through all execution phases;
/// restore both on success, error, cancellation and unwinding.
pub(super) struct Resources {
    task_id: String,
    import_mode: bool,
}
impl Resources {
    pub(super) fn acquire(task_id: i64, import_mode: bool) -> Self {
        if import_mode {
            mode_switcher::ToImportMode();
        }
        let task_id = task_id.to_string();
        task_register::RegisterTaskOnce(&task_id);
        Self {
            task_id,
            import_mode,
        }
    }
}
impl Drop for Resources {
    fn drop(&mut self) {
        if self.import_mode {
            mode_switcher::ToNormalMode();
        }
        task_register::Close(&self.task_id);
        fire_call(
            "github.com/pingcap/tidb/pkg/dxf/framework/scheduler/WaitCleanUpFinished",
            FailCtx::None,
        );
    }
}

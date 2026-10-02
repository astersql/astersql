// Copyright 2026 AsterSQL.

//! Native DXF scan/encode and physical-ingest operators. Bounded channels carry
//! owned batches; sessions stay on their worker thread and Close releases them.
use super::{ConcreteSession, Domain, system_session};
use astersql_meta_model::group_3::Job;
use astersql_resourcemanager_pool_workerpool as pool;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

thread_local! {
    static SESSIONS:RefCell<HashMap<Uuid,ConcreteSession>>=RefCell::new(HashMap::new());
}
fn with_session<T>(
    id: Uuid,
    domain: &Arc<Domain>,
    action: impl FnOnce(&mut ConcreteSession) -> Result<T, String>,
) -> Result<T, String> {
    SESSIONS.with(|sessions| {
        let mut sessions = sessions.borrow_mut();
        let session = sessions.entry(id).or_insert_with(|| {
            let mut session = ConcreteSession::new(domain.clone());
            session.SetInRestrictedSQL(true);
            session
        });
        action(session)
    })
}
fn close_session(id: Uuid) {
    SESSIONS.with(|sessions| {
        sessions.borrow_mut().remove(&id);
    });
}
fn failure(error: impl std::fmt::Display) -> pool::Error {
    pool::Error::new(error.to_string())
}

pub(super) struct ReadTask {
    pub physical: i64,
    pub start: Vec<u8>,
    pub end: Vec<u8>,
}
impl pool::TaskMayPanic for ReadTask {
    fn RecoverArgs(&self) -> (String, String, Option<pool::Error>) {
        ("ddl_backfill".into(), "tableScanWorker".into(), None)
    }
}
struct WriteTask {
    start: Vec<u8>,
    records: system_session::IndexBackfillRecords,
}
impl pool::TaskMayPanic for WriteTask {
    fn RecoverArgs(&self) -> (String, String, Option<pool::Error>) {
        ("ddl_backfill".into(), "indexIngestWorker".into(), None)
    }
}
pub(super) struct Ack {
    pub start: Vec<u8>,
    pub context: astersql_ddl::backfilling::BackfillTaskContext,
}
struct Reader {
    id: Uuid,
    domain: Arc<Domain>,
    job: Arc<Job>,
    indexes: Vec<i64>,
    context: pool::Context,
    output: pool::Channel<WriteTask>,
    closed: Arc<AtomicU32>,
}
impl pool::Worker<ReadTask, pool::None> for Reader {
    fn HandleTask(
        &mut self,
        task: ReadTask,
        _: &mut dyn FnMut(pool::None),
    ) -> Result<(), pool::Error> {
        let mut start = task.start;
        while start < task.end {
            if self.context.IsCancelled() {
                return Err(failure("distributed index scan cancelled"));
            }
            #[cfg(test)]
            astersql_testkit_testfailpoint::inject_value(
                "github.com/pingcap/tidb/pkg/ddl/scanRecordExec",
                &serde_json::to_string(
                    &serde_json::json!({"id":self.job.id,"reorg_meta":&self.job.reorg_meta}),
                )
                .map_err(failure)?,
            );

            let meta = self
                .job
                .reorg_meta
                .as_ref()
                .ok_or_else(|| failure("DXF reorg metadata missing"))?;
            let request = astersql_ddl::backfilling::IndexBackfillBatch {
                schema_id: self.job.schema_id,
                table_id: self.job.table_id,
                index_ids: self.indexes.clone(),
                task: astersql_ddl::backfilling::ReorgBackfillTask {
                    physical_table_id: task.physical,
                    start_key: start.clone(),
                    end_key: task.end.clone(),
                    ..Default::default()
                },
                batch_size: meta.GetBatchSize().max(1) as usize,
                resource_group: meta.ResourceGroupName.clone(),
                sql_mode: meta.SQLMode as i64,
            };
            let records = with_session(self.id, &self.domain, |session| {
                session
                    .execute("BEGIN")
                    .map_err(|error| error.to_string())?;
                let result =
                    system_session::generate_index_backfill_records(session, request, true);
                let rollback = session.execute("ROLLBACK");
                match result {
                    Ok(result) => {
                        rollback.map_err(|error| error.to_string())?;
                        Ok(result)
                    }
                    Err(error) => {
                        if let Err(rollback) = rollback {
                            eprintln!("DDL scan rollback failed: {rollback}");
                        }
                        Err(error)
                    }
                }
            })
            .map_err(failure)?;
            let next = records.context.next_key.clone();
            if next <= start {
                return Err(failure("distributed index checkpoint did not advance"));
            }
            if !self.output.send(WriteTask { start, records }) {
                return Err(failure("distributed index writer closed"));
            }
            start = next;
        }
        Ok(())
    }
    fn Close(&mut self) -> Result<(), pool::Error> {
        close_session(self.id);
        self.closed.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}
#[derive(Clone)]
pub(super) struct CloudWriteConfig {
    pub store: Arc<super::modify_column_cloud_store::CloudStore>,
    pub prefix: String,
    pub indexes: Vec<i64>,
    pub memory_per_index: u64,
    pub key_prefix: Vec<u8>,
    pub summaries: Arc<Mutex<Vec<super::modify_column_cloud_meta::SortedMeta>>>,
}
struct Writer {
    id: Uuid,
    domain: Arc<Domain>,
    job_id: i64,
    options: astersql_kv::SSTImportOptions,
    context: pool::Context,
    import_lock: Arc<Mutex<()>>,
    cloud: Option<CloudWriteConfig>,
    external_writers: Option<Vec<astersql_ingestor_simplesst::writer::Writer>>,
    closed: Arc<AtomicU32>,
}
impl pool::Worker<WriteTask, Ack> for Writer {
    fn HandleTask(
        &mut self,
        task: WriteTask,
        send: &mut dyn FnMut(Ack),
    ) -> Result<(), pool::Error> {
        if self.context.IsCancelled() {
            return Err(failure("distributed index ingest cancelled"));
        }
        if let Some(cloud) = &self.cloud {
            if self.external_writers.is_none() {
                let mut writers = Vec::with_capacity(cloud.indexes.len());
                for group in 0..cloud.indexes.len() {
                    let mut builder = astersql_ingestor_simplesst::writer::WriterBuilder::new();
                    builder
                        .set_memory_size_limit(cloud.memory_per_index)
                        .set_group_offset(group as i32)
                        .set_key_prefix(cloud.key_prefix.clone());
                    writers.push(builder.build_with_sink(
                        cloud.store.clone(),
                        &cloud.prefix,
                        &Uuid::new_v4().to_string(),
                    ));
                }
                self.external_writers = Some(writers);
            }
            let mut context = task.records.context;
            for (_, index, entries) in task.records.records {
                let group = cloud
                    .indexes
                    .iter()
                    .position(|id| *id == index.ID)
                    .ok_or_else(|| failure("cloud writer index group missing"))?;
                for (key, value, _) in entries {
                    self.external_writers.as_mut().unwrap()[group]
                        .write_row(&key.0, &value)
                        .map_err(failure)?;
                }
                context.added_count += 1;
            }
            send(Ack {
                start: task.start,
                context,
            });
            return Ok(());
        }
        // The import critical section also makes the live unique-key check and
        // physical import indivisible across chunks. Encoded batches remain
        // bounded; sharing the old in-memory engine would retain every key.
        let import = self.import_lock.lock().map_err(failure)?;
        let context = with_session(self.id, &self.domain, |session| {
            session
                .execute("BEGIN")
                .map_err(|error| error.to_string())?;
            match system_session::write_index_backfill_records(
                session,
                task.records,
                Some(self.job_id),
                self.options.clone(),
            ) {
                Ok(result) => {
                    session
                        .execute("COMMIT")
                        .map_err(|error| error.to_string())?;
                    Ok(result)
                }
                Err(error) => {
                    if let Err(rollback) = session.execute("ROLLBACK") {
                        eprintln!("DDL ingest rollback failed: {rollback}");
                    }
                    Err(error)
                }
            }
        })
        .map_err(failure)?;
        drop(import);
        send(Ack {
            start: task.start,
            context,
        });
        Ok(())
    }
    fn Close(&mut self) -> Result<(), pool::Error> {
        let result = (|| {
            if let Some(writers) = self.external_writers.take() {
                let cloud = self
                    .cloud
                    .as_ref()
                    .ok_or_else(|| failure("cloud writer config missing during close"))?;
                for (group, mut writer) in writers.into_iter().enumerate() {
                    let summary = writer.close().map_err(failure)?;
                    cloud.summaries.lock().map_err(failure)?[group]
                        .merge(&super::modify_column_cloud_meta::SortedMeta::from_simple(
                            &summary,
                        ))
                        .map_err(failure)?;
                }
            }
            Ok(())
        })();
        close_session(self.id);
        self.closed.fetch_add(1, Ordering::AcqRel);
        result
    }
}

pub(super) struct Pipeline {
    readers: Mutex<pool::WorkerPool<ReadTask, pool::None>>,
    writers: Mutex<pool::WorkerPool<WriteTask, Ack>>,
    input: pool::Channel<ReadTask>,
    encoded: pool::Channel<WriteTask>,
    pub context: pool::Context,
    pub results: pool::Channel<Ack>,
    options: astersql_kv::SSTImportOptions,
    stopped: AtomicBool,
    closed_readers: Arc<AtomicU32>,
    closed_writers: Arc<AtomicU32>,
    average_row_size: usize,
    global_sort: bool,
}
impl Pipeline {
    pub(super) fn start(
        domain: Arc<Domain>,
        job: Arc<Job>,
        indexes: Vec<i64>,
        cpu: i32,
        average_row_size: usize,
        options: astersql_kv::SSTImportOptions,
        cloud: Option<CloudWriteConfig>,
    ) -> Arc<Self> {
        let global_sort = cloud.is_some();
        let (reads, writes) = astersql_ddl::backfilling_txn_executor::expected_ingest_worker_count(
            cpu.max(1) as usize,
            average_row_size,
            global_sort,
        );
        let context = pool::Context::background();
        let input = pool::Channel::bounded(1);
        let encoded = pool::Channel::bounded(1);
        let results = pool::Channel::bounded(1);
        let closed_readers = Arc::new(AtomicU32::new(0));
        let closed_writers = Arc::new(AtomicU32::new(0));
        let read_domain = domain.clone();
        let read_job = job.clone();
        let read_context = context.clone();
        let output = encoded.clone();
        let read_closed = closed_readers.clone();
        let mut readers =
            pool::WorkerPool::NewWorkerPool("ddl-table-scan", (), reads as i32, move || Reader {
                id: Uuid::new_v4(),
                domain: read_domain.clone(),
                job: read_job.clone(),
                indexes: indexes.clone(),
                context: read_context.clone(),
                output: output.clone(),
                closed: read_closed.clone(),
            });
        readers.SetTaskReceiver(input.clone());
        readers.Start(context.clone());
        let write_context = context.clone();
        let write_options = options.clone();
        let write_closed = closed_writers.clone();
        let import_lock = Arc::new(Mutex::new(()));
        let mut writers =
            pool::WorkerPool::NewWorkerPool("ddl-index-ingest", (), writes as i32, move || {
                Writer {
                    id: Uuid::new_v4(),
                    domain: domain.clone(),
                    job_id: job.id,
                    options: write_options.clone(),
                    context: write_context.clone(),
                    import_lock: import_lock.clone(),
                    cloud: cloud.clone(),
                    external_writers: None,
                    closed: write_closed.clone(),
                }
            });
        writers.SetTaskReceiver(encoded.clone());
        writers.SetResultSender(results.clone());
        writers.Start(context.clone());
        Arc::new(Self {
            readers: Mutex::new(readers),
            writers: Mutex::new(writers),
            input,
            encoded,
            context,
            results,
            options,
            stopped: AtomicBool::new(false),
            closed_readers,
            closed_writers,
            average_row_size,
            global_sort,
        })
    }
    pub(super) fn feed(self: &Arc<Self>, ranges: Vec<ReadTask>) -> std::thread::JoinHandle<()> {
        let input = self.input.clone();
        // Keep inputs open until the subtask closes the pipeline. Idle workers
        // must remain available for Tune while the other stage still runs.
        std::thread::spawn(move || {
            for range in ranges {
                if !input.send(range) {
                    break;
                }
            }
        })
    }
    pub(super) fn tune(&self, cpu: i32) {
        let (reads, writes) = astersql_ddl::backfilling_txn_executor::expected_ingest_worker_count(
            cpu.max(1) as usize,
            self.average_row_size,
            self.global_sort,
        );
        self.readers.lock().unwrap().Tune(reads as i32, true);
        self.writers.lock().unwrap().Tune(writes as i32, true);
    }
    #[cfg(test)]
    pub(super) fn closed_workers(&self) -> (u32, u32) {
        (
            self.closed_readers.load(Ordering::Acquire),
            self.closed_writers.load(Ordering::Acquire),
        )
    }
    pub(super) fn finish(&self) -> Result<(), pool::Error> {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return self.context.OperatorErr().map_or(Ok(()), Err);
        }
        // All acknowledged ranges have reached the durable frontier. Closing
        // stage inputs lets worker Close publish the final cloud writer stats
        // before cancellation; each pool cancels its own child context only.
        self.input.close();
        self.readers.lock().map_err(failure)?.Release();
        self.encoded.close();
        self.writers.lock().map_err(failure)?.Release();
        self.context.Cancel();
        self.context.OperatorErr().map_or(Ok(()), Err)
    }
    pub(super) fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        // Wake physical writes before joining: the limiter bridge observes this
        // cancellation even when the framework has not cancelled its context.
        self.options.context.cancel();
        self.context.Cancel();
        self.input.close();
        self.encoded.close();
        self.results.close();
        self.readers.lock().unwrap().Release();
        self.writers.lock().unwrap().Release();
    }
}
impl Drop for Pipeline {
    fn drop(&mut self) {
        self.shutdown();
    }
}

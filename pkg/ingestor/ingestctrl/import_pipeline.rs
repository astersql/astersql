// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! The local import pipeline owns every generated job until completion. Worker
//! cancellation and worker release are separate: no result is published as final
//! until Release has joined all workers, including their data-reference cleanup.

use crate::job_worker::{RegionJob, RegionJobStage, RegionJobWorker};
use crate::region_job::{regionJobRetryer, storeBalancer};
use crate::{CancellationToken, Error, Result};
use astersql_ingestor_engineapi::{self as api, IngestData};
use astersql_resourcemanager_pool_workerpool::{self as pool, TaskMayPanic};
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicI64, Ordering},
};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(1);
const MAX_RETRIES: usize = 30;

fn engine_error(error: api::EngineError) -> Error {
    match error.downcast::<Error>() {
        Ok(error) => *error,
        Err(error) => Error::InvalidData(error.to_string()),
    }
}

#[derive(Default)]
struct Group {
    pending: Mutex<usize>,
    changed: Condvar,
    error: Mutex<Option<Error>>,
    bytes: AtomicI64,
    count: AtomicI64,
}

impl Group {
    fn fail(&self, error: Error, context: &pool::Context) {
        let mut first = self.error.lock().unwrap();
        if first.is_none() {
            *first = Some(error.clone());
        }
        drop(first);
        context.OnError(pool::Error::new(error.to_string()));
        self.changed.notify_all();
    }
}

/// Data cleanup must precede decrementing the outstanding job count.
/// This ordering also makes blocking DecRef part of the worker's lifetime.
pub(crate) struct JobResources {
    pub data: Arc<dyn IngestData>,
    group: Arc<Group>,
}

impl std::fmt::Debug for JobResources {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobResources").finish_non_exhaustive()
    }
}

impl JobResources {
    pub fn reference(&self) {
        *self.group.pending.lock().unwrap() += 1;
        self.data.IncRef();
    }
    pub fn done(&self) {
        self.data.DecRef();
        let mut pending = self.group.pending.lock().unwrap();
        *pending -= 1;
        self.group.changed.notify_all();
    }
    pub fn finish(&self, bytes: i64, count: i64) {
        self.data.Finish(bytes, count);
        self.group.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.group.count.fetch_add(count, Ordering::Relaxed);
    }
}

// Channel sends can lose a race with cancellation. Carry cleanup ownership in
// the value so an unsent task or result still releases its reference exactly once.
pub(crate) struct OwnedJob(Option<RegionJob>);
impl OwnedJob {
    fn take(&mut self) -> RegionJob {
        self.0.take().unwrap()
    }
}
impl Drop for OwnedJob {
    fn drop(&mut self) {
        if let Some(job) = &self.0 {
            job.done();
        }
    }
}
impl TaskMayPanic for OwnedJob {
    fn RecoverArgs(&self) -> (String, String, Option<pool::Error>) {
        self.0.as_ref().unwrap().RecoverArgs()
    }
}

struct Worker {
    inner: Box<dyn RegionJobWorker>,
    context: pool::Context,
    group: Arc<Group>,
    balancer: Option<Arc<storeBalancer>>,
}
struct StoreLoadGuard {
    balancer: Option<Arc<storeBalancer>>,
    peers: Vec<u64>,
}
impl Drop for StoreLoadGuard {
    fn drop(&mut self) {
        if let Some(balancer) = &self.balancer {
            if let Err(error) = balancer.releaseStoreLoad(&self.peers) {
                log::error!("failed to release region job store load: {error}");
            }
        }
    }
}
impl pool::Worker<OwnedJob, OwnedJob> for Worker {
    fn HandleTask(
        &mut self,
        mut owned: OwnedJob,
        send: &mut dyn FnMut(OwnedJob),
    ) -> std::result::Result<(), pool::Error> {
        let job = owned.take();
        let mut cleanup = OwnedJob(Some(job.clone()));
        let load = StoreLoadGuard {
            balancer: self.balancer.clone(),
            peers: job.region.peer_store_ids.clone(),
        };
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.inner.HandleTask(job)))
                .unwrap_or_else(|payload| {
                    let (label, info, _) = cleanup.RecoverArgs();
                    log::error!(
                        "{label}: {info}: region job worker panic: {payload:?}; stack={}",
                        std::backtrace::Backtrace::force_capture()
                    );
                    astersql_metrics::metrics::PanicCounter
                        .with_label_values(&[&label])
                        .inc();
                    Err(Error::InvalidData("region job worker panic".into()))
                });
        drop(load);
        match result {
            Ok(jobs) => {
                if !jobs.is_empty() {
                    cleanup.0.take();
                }
                for job in jobs {
                    send(OwnedJob(Some(job)));
                }
                Ok(())
            }
            Err(error) => {
                self.group.fail(error.clone(), &self.context);
                Err(pool::Error::new(error.to_string()))
            }
        }
    }
    fn Close(&mut self) -> std::result::Result<(), pool::Error> {
        self.inner.Close().map_err(|error| {
            self.group.fail(error.clone(), &self.context);
            pool::Error::new(error.to_string())
        })
    }
}

/// Region generation is an external PD boundary. The pipeline preserves all
/// generated jobs and their shared data; callers provide the existing generator.
pub type JobGenerator = Arc<
    dyn Fn(&CancellationToken, Arc<dyn IngestData>, &[api::Range]) -> Result<Vec<RegionJob>>
        + Send
        + Sync,
>;
pub type WorkerFactory =
    Arc<dyn Fn(CancellationToken) -> Result<Box<dyn RegionJobWorker>> + Send + Sync>;

/// Hooks correspond to Go's test failpoint boundaries, without global state.
#[derive(Clone, Default)]
pub struct ImportOptions {
    /// Local engines use store balancing and parallel generation. External
    /// engines retain one generator to bound resident ingest data.
    pub local_engine: bool,
    pub before_release: Option<Arc<dyn Fn() + Send + Sync>>,
    pub before_wait_outcome: Option<Arc<dyn Fn() + Send + Sync>>,
    pub before_receive_result: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Wait for a successful pool outcome after result-channel closure. Closure
/// alone is not success: Release closes the channel during error cleanup too.
pub(crate) fn wait_pool_outcome(token: &CancellationToken, succeeded: &AtomicBool) -> Result<()> {
    loop {
        token.check()?;
        if succeeded.load(Ordering::Acquire) {
            return Ok(());
        }
        std::thread::sleep(POLL);
    }
}

/// Run the same dispatcher loop used by do_import, including its closed-result
/// outcome wait. Tests exercise this boundary directly so Group errors cannot
/// mask an incorrectly successful dispatcher return.
pub(crate) fn dispatch_results(
    token: &CancellationToken,
    results: &pool::Channel<OwnedJob>,
    retryer: &regionJobRetryer,
    succeeded: &AtomicBool,
    before_wait_outcome: &Option<Arc<dyn Fn() + Send + Sync>>,
    before_receive: &Option<Arc<dyn Fn() + Send + Sync>>,
) -> Result<()> {
    token.check()?;
    loop {
        token.check()?;
        if let Some(hook) = before_receive {
            hook();
        }
        let mut owned = match results.recv_timeout(POLL) {
            Ok(Some(owned)) => owned,
            Ok(None) | Err(pool::RecvTimeoutError::Disconnected) => break,
            Err(pool::RecvTimeoutError::Timeout) => continue,
        };
        token.check()?;
        let mut job = owned.take();
        match job.stage {
            RegionJobStage::Ingested => job.done(),
            RegionJobStage::RegionScanned | RegionJobStage::Wrote => {
                job.retry_count += 1;
                if job.retry_count > MAX_RETRIES {
                    job.done();
                    return Err(job.last_retryable_cause.clone().unwrap_or_else(|| {
                        Error::Retryable(
                            job.last_retryable_error
                                .clone()
                                .unwrap_or_else(|| "retry limit exceeded".into()),
                        )
                    }));
                }
                let delay = Duration::from_secs((1u64 << job.retry_count.min(31)).min(30));
                let mut cleanup = OwnedJob(Some(job.clone()));
                if retryer.push(job, Instant::now() + delay) {
                    cleanup.0.take();
                }
            }
            RegionJobStage::NeedRescan => {
                job.done();
                return Err(Error::InvalidData("should not reach here".into()));
            }
        }
    }
    if let Some(hook) = before_wait_outcome {
        hook();
    }
    wait_pool_outcome(token, succeeded)?;
    retryer.close();
    Ok(())
}

pub fn do_import(
    parent: &CancellationToken,
    engine: Arc<dyn api::Engine>,
    concurrency: usize,
    generate: JobGenerator,
    factory: WorkerFactory,
    options: ImportOptions,
) -> Result<(i64, i64)> {
    parent.check()?;
    let group = Arc::new(Group::default());
    let worker_ctx = pool::Context::background();
    // The same wctx is supplied to factories and Start. OnError must cancel
    // callbacks already running before Release joins their threads.
    let wctx = pool::NewContext(worker_ctx.clone());
    let token = parent.for_workers(wctx.clone());
    let parent_token = parent.for_workers(worker_ctx.clone());
    let tasks = pool::Channel::bounded(0);
    let api_ctx = api::Context::background();
    let succeeded = Arc::new(AtomicBool::new(false));
    let finished = AtomicBool::new(false);
    let retryer = Arc::new(regionJobRetryer::default());
    let balancer = options
        .local_engine
        .then(|| Arc::new(storeBalancer::default()));
    let producer_done = AtomicBool::new(false);
    let retry_done = AtomicBool::new(false);
    let mut workers = pool::WorkerPool::NewWorkerPoolWithFallibleFactory(
        "region-job",
        (),
        concurrency.max(1) as i32,
        {
            let group = group.clone();
            let wctx = wctx.clone();
            let token = token.clone();
            let balancer = balancer.clone();
            move || match factory(token.clone()) {
                Ok(inner) => Some(Worker {
                    inner,
                    context: wctx.clone(),
                    group: group.clone(),
                    balancer: balancer.clone(),
                }),
                Err(error) => {
                    group.fail(error, &wctx);
                    None
                }
            }
        },
    );
    workers.SetTaskReceiver(tasks.clone());
    workers.Start(wctx.clone());
    let results = workers.GetResultChan().unwrap();

    let submit = |job: RegionJob| {
        let mut owned = OwnedJob(Some(job.clone()));
        if token.is_cancelled() {
            return;
        }
        if let Some(balancer) = &balancer {
            match balancer.push(job) {
                Ok(()) => {
                    owned.0.take();
                }
                Err(error) => group.fail(error, &wctx),
            }
        } else {
            owned.0.take();
            tasks.send(OwnedJob(Some(job)));
        }
    };

    std::thread::scope(|scope| {
        let monitor = scope.spawn(|| {
            while !finished.load(Ordering::Acquire) {
                if parent.is_cancelled() && !worker_ctx.IsCancelled() {
                    group.fail(Error::Cancelled, &wctx);
                    worker_ctx.Cancel();
                }
                if wctx.IsCancelled() {
                    api_ctx.cancel();
                    tasks.close();
                    group.changed.notify_all();
                }
                std::thread::sleep(POLL);
            }
        });
        let balancing = scope.spawn(|| {
            if let Some(balancer) = &balancer {
                while !token.is_cancelled() {
                    match balancer.pickJob() {
                        Ok(Some(job)) => {
                            let mut load = StoreLoadGuard {
                                balancer: Some(balancer.clone()),
                                peers: job.region.peer_store_ids.clone(),
                            };
                            // Ownership of store load transfers to the worker on send.
                            if tasks.send(OwnedJob(Some(job))) {
                                load.balancer.take();
                            }
                        }
                        Ok(None) => std::thread::sleep(POLL),
                        Err(error) => {
                            group.fail(error, &wctx);
                            break;
                        }
                    }
                }
                // Producers may be finishing a cancelled generator. Drain only
                // after both senders have stopped adding new queue entries.
                while !producer_done.load(Ordering::Acquire) || !retry_done.load(Ordering::Acquire)
                {
                    std::thread::sleep(POLL);
                }
                while let Ok(Some(job)) = balancer.pickJob() {
                    if let Err(error) = balancer.releaseStoreLoad(&job.region.peer_store_ids) {
                        group.fail(error, &wctx);
                    }
                    job.done();
                }
            }
        });
        let dispatcher = scope.spawn(|| -> Result<()> {
            let result = dispatch_results(
                &parent_token,
                &results,
                &retryer,
                &succeeded,
                &options.before_wait_outcome,
                &options.before_receive_result,
            );
            if let Err(error) = &result {
                group.fail(error.clone(), &wctx);
                worker_ctx.Cancel();
            }
            result
        });
        let retry = scope.spawn(|| {
            while !token.is_cancelled() && !succeeded.load(Ordering::Acquire) {
                match retryer.popReady(&token) {
                    Ok(Some(job)) => {
                        submit(job);
                    }
                    Ok(None) => std::thread::sleep(POLL),
                    Err(Error::Cancelled) => break,
                    Err(error) => {
                        group.fail(error, &wctx);
                        break;
                    }
                }
            }
            for job in retryer.cleanupUnprocessedJobs() {
                job.done();
            }
            retry_done.store(true, Ordering::Release);
        });
        let producer = scope.spawn(|| {
            let (tx, rx) = std::sync::mpsc::sync_channel(0);
            let rx = Mutex::new(rx);
            std::thread::scope(|generation| {
                let loader = generation.spawn(|| {
                    if let Err(error) = engine.LoadIngestData(&api_ctx, &tx) {
                        if !token.is_cancelled() {
                            group.fail(engine_error(error), &wctx);
                        }
                    }
                    drop(tx);
                });
                let mut generators = Vec::new();
                for _ in 0..if options.local_engine {
                    concurrency.max(1)
                } else {
                    1
                } {
                    let rx = &rx;
                    let token = &token;
                    let group = &group;
                    let wctx = &wctx;
                    let generate = &generate;
                    let submit = &submit;
                    generators.push(generation.spawn(move || {
                        loop {
                            let batch = match rx.lock().unwrap().recv_timeout(POLL) {
                                Ok(batch) => batch,
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                            };
                            if token.is_cancelled() {
                                continue;
                            }
                            let data: Arc<dyn IngestData> = Arc::from(batch.Data);
                            let generated =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    generate(&token, data.clone(), &batch.SortedRanges)
                                }))
                                .unwrap_or_else(|_| {
                                    Err(Error::InvalidData("region job generator panic".into()))
                                });
                            match generated {
                                Ok(mut jobs) => {
                                    let resources = Arc::new(JobResources {
                                        data,
                                        group: group.clone(),
                                    });
                                    // Ref the complete batch before handing any job to workers.
                                    for job in &mut jobs {
                                        job.resources = Some(resources.clone());
                                        job.r#ref();
                                    }
                                    for job in jobs {
                                        submit(job);
                                    }
                                }
                                Err(error) => group.fail(error, &wctx),
                            }
                        }
                    }));
                }
                for generator in generators {
                    generator.join().unwrap();
                }
                if loader.join().is_err() {
                    group.fail(Error::InvalidData("engine loader panic".into()), &wctx);
                }
            });
            let mut pending = group.pending.lock().unwrap();
            while *pending != 0 && !token.is_cancelled() {
                pending = group.changed.wait_timeout(pending, POLL).unwrap().0;
            }
            wctx.Cancel();
            producer_done.store(true, Ordering::Release);
        });
        while !wctx.IsCancelled() {
            std::thread::sleep(POLL);
        }
        if let Some(before_release) = options.before_release {
            before_release();
        }
        workers.Release();
        // Operator errors may be set after the outstanding-job wait unblocks.
        if let Some(error) = wctx.OperatorErr() {
            let mut first = group.error.lock().unwrap();
            if first.is_none() {
                *first = Some(Error::InvalidData(error.to_string()));
            }
            worker_ctx.Cancel();
        } else if !parent.is_cancelled() && !worker_ctx.IsCancelled() {
            succeeded.store(true, Ordering::Release);
        } else {
            worker_ctx.Cancel();
        }
        producer.join().unwrap();
        let _ = dispatcher.join().unwrap();
        retry.join().unwrap();
        balancing.join().unwrap();
        finished.store(true, Ordering::Release);
        monitor.join().unwrap();
    });
    let error = group.error.lock().unwrap().clone();
    match error {
        Some(error) => Err(error),
        None => {
            parent.check()?;
            Ok((
                group.bytes.load(Ordering::Relaxed),
                group.count.load(Ordering::Relaxed),
            ))
        }
    }
}

/// Adapter for the existing whole-range TiKV client boundary. The client owns
/// Region discovery and Write+Ingest within each supplied range.
pub(crate) struct ClientWorker {
    pub client: Arc<dyn crate::local::ImportClient>,
    pub engine: Arc<crate::engine::Engine>,
    pub token: CancellationToken,
}
impl RegionJobWorker for ClientWorker {
    fn HandleTask(&self, mut job: RegionJob) -> Result<Vec<RegionJob>> {
        let (bytes, count) =
            self.client
                .WriteAndIngest(&self.token, &self.engine, &[job.key_range.clone()])?;
        self.token.check()?;
        job.write_result = Some(crate::job_worker::TikvWriteResult {
            total_bytes: bytes,
            count,
            ..Default::default()
        });
        job.convertStageTo(RegionJobStage::Ingested);
        Ok(vec![job])
    }
    fn Close(&self) -> Result<()> {
        Ok(())
    }
}

pub(crate) struct LocalEngineSource {
    engine: Arc<crate::engine::Engine>,
    ranges: Vec<crate::KeyRange>,
}
impl LocalEngineSource {
    pub fn new(engine: Arc<crate::engine::Engine>, ranges: Vec<crate::KeyRange>) -> Self {
        Self { engine, ranges }
    }
}
impl api::Engine for LocalEngineSource {
    fn ID(&self) -> String {
        self.engine.ID()
    }
    fn LoadIngestData(
        &self,
        ctx: &api::Context,
        out: &std::sync::mpsc::SyncSender<api::DataAndRanges>,
    ) -> std::result::Result<(), api::EngineError> {
        if ctx.is_cancelled() {
            return Err(Box::new(Error::Cancelled));
        }
        out.send(api::DataAndRanges {
            Data: Box::new(LocalData {
                pairs: self.engine.snapshot()?,
                ts: self.engine.engine_meta.ts.load(Ordering::Acquire),
                refs: Default::default(),
            }),
            SortedRanges: self
                .ranges
                .iter()
                .map(|r| api::Range {
                    Start: r.start.clone(),
                    End: r.end.clone(),
                })
                .collect(),
        })
        .map_err(|e| Box::new(Error::InvalidData(e.to_string())) as api::EngineError)
    }
    fn KVStatistics(&self) -> (i64, i64) {
        self.engine.KVStatistics()
    }
    fn ImportedStatistics(&self) -> (i64, i64) {
        self.engine.ImportedStatistics()
    }
    fn ConflictInfo(&self) -> api::ConflictInfo {
        api::ConflictInfo {
            Count: self.engine.ConflictInfo().count as u64,
            Files: vec![],
        }
    }
    fn GetKeyRange(&self) -> std::result::Result<(Vec<u8>, Vec<u8>), api::EngineError> {
        let range = self.engine.GetKeyRange()?;
        Ok((range.start, range.end))
    }
    fn GetRegionSplitKeys(&self) -> std::result::Result<Vec<Vec<u8>>, api::EngineError> {
        Ok(self.engine.GetRegionSplitKeys()?)
    }
    fn Close(&mut self) -> std::result::Result<(), api::EngineError> {
        Ok(())
    }
}

pub(crate) struct LocalData {
    pub pairs: Vec<crate::KvPair>,
    pub ts: u64,
    pub refs: std::sync::atomic::AtomicUsize,
}
impl IngestData for LocalData {
    fn GetFirstAndLastKey(
        &self,
        lower: &[u8],
        upper: &[u8],
    ) -> std::result::Result<(Option<Vec<u8>>, Option<Vec<u8>>), api::EngineError> {
        let pairs: Vec<_> = self
            .pairs
            .iter()
            .filter(|p| {
                (lower.is_empty() || p.key.as_slice() >= lower)
                    && (upper.is_empty() || p.key.as_slice() < upper)
            })
            .collect();
        Ok((
            pairs.first().map(|p| p.key.clone()),
            pairs.last().map(|p| p.key.clone()),
        ))
    }
    fn NewIter(
        &self,
        ctx: &api::Context,
        lower: &[u8],
        upper: &[u8],
        _: &mut astersql_lightning_membuf::Pool,
    ) -> Box<dyn api::ForwardIter> {
        Box::new(DataIter {
            pairs: self
                .pairs
                .iter()
                .filter(|p| {
                    (lower.is_empty() || p.key.as_slice() >= lower)
                        && (upper.is_empty() || p.key.as_slice() < upper)
                })
                .cloned()
                .collect(),
            index: None,
            ctx: ctx.clone(),
        })
    }
    fn GetTS(&self) -> u64 {
        self.ts
    }
    fn IncRef(&self) {
        self.refs.fetch_add(1, Ordering::AcqRel);
    }
    fn DecRef(&self) {
        self.refs.fetch_sub(1, Ordering::AcqRel);
    }
    fn Finish(&self, _: i64, _: i64) {}
}
struct DataIter {
    pairs: Vec<crate::KvPair>,
    index: Option<usize>,
    ctx: api::Context,
}
impl api::ForwardIter for DataIter {
    fn First(&mut self) -> bool {
        self.index = Some(0);
        self.Valid()
    }
    fn Valid(&self) -> bool {
        !self.ctx.is_cancelled() && self.index.is_some_and(|i| i < self.pairs.len())
    }
    fn Next(&mut self) -> bool {
        if let Some(index) = &mut self.index {
            *index += 1;
        }
        self.Valid()
    }
    fn Key(&self) -> &[u8] {
        &self.pairs[self.index.unwrap()].key
    }
    fn Value(&self) -> &[u8] {
        &self.pairs[self.index.unwrap()].value
    }
    fn Close(&mut self) -> std::result::Result<(), api::EngineError> {
        self.index = None;
        Ok(())
    }
    fn Error(&self) -> Option<&api::EngineError> {
        None
    }
    fn ReleaseBuf(&mut self) {}
}

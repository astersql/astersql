// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
// http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Persistent DXF adapters for MODIFY COLUMN index backfills.
use super::{ConcreteSession, Domain};
use astersql_dxf_framework_scheduler as scheduler;
use astersql_dxf_framework_storage as storage;
use astersql_dxf_framework_taskexecutor as executor;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread::JoinHandle;
use std::time::Duration;

#[path = "modify_column_cloud_executor.rs"]
mod cloud;
#[path = "modify_column_cloud_planner.rs"]
mod cloud_planner;

fn error(value: impl std::fmt::Display) -> executor::ExecutorError {
    executor::ExecutorError(value.to_string())
}
fn task(value: storage::proto::Task) -> executor::Task {
    executor::Task {
        TaskBase: executor::TaskBase {
            ID: value.ID,
            Key: value.Key.clone(),
            Type: value.Type.to_owned(),
            State: match value.State {
                storage::proto::TaskStatePending => executor::TaskState::Pending,
                storage::proto::TaskStateRunning => executor::TaskState::Running,
                storage::proto::TaskStateSucceed => executor::TaskState::Succeed,
                storage::proto::TaskStatePausing => executor::TaskState::Pausing,
                storage::proto::TaskStateReverting => executor::TaskState::Reverting,
                storage::proto::TaskStateReverted => executor::TaskState::Reverted,
                storage::proto::TaskStateFailed => executor::TaskState::Failed,
                _ => executor::TaskState::Modifying,
            },
            Step: value.Step,
            Priority: value.Priority,
            CreateTime: value.CreateTime,
            RequiredSlots: value.RequiredSlots,
            Keyspace: value.Keyspace.clone(),
            ..Default::default()
        },
        Meta: value.Meta,
    }
}
fn state(value: executor::SubtaskState) -> storage::proto::SubtaskState {
    match value {
        executor::SubtaskState::Pending => storage::proto::SubtaskStatePending,
        executor::SubtaskState::Running => storage::proto::SubtaskStateRunning,
        executor::SubtaskState::Succeed => storage::proto::SubtaskStateSucceed,
        executor::SubtaskState::Failed => storage::proto::SubtaskStateFailed,
        executor::SubtaskState::Canceled => storage::proto::SubtaskStateCanceled,
        executor::SubtaskState::Paused => storage::proto::SubtaskStatePaused,
    }
}
fn subtask(value: storage::proto::Subtask) -> executor::Subtask {
    executor::Subtask {
        SubtaskBase: executor::SubtaskBase {
            ID: value.ID,
            TaskID: value.TaskID,
            Step: value.Step,
            ExecID: value.ExecID.clone(),
            State: match value.State {
                storage::proto::SubtaskStatePending => executor::SubtaskState::Pending,
                storage::proto::SubtaskStateRunning => executor::SubtaskState::Running,
                storage::proto::SubtaskStateSucceed => executor::SubtaskState::Succeed,
                storage::proto::SubtaskStateFailed => executor::SubtaskState::Failed,
                storage::proto::SubtaskStateCanceled => executor::SubtaskState::Canceled,
                _ => executor::SubtaskState::Paused,
            },
        },
        Meta: value.Meta,
    }
}
struct TaskTable(storage::TaskManager, Weak<Domain>);
impl executor::TaskTable for TaskTable {
    fn AcquireTaskRuntime(
        &self,
        _: &executor::Context,
        task: &executor::Task,
    ) -> executor::Result<Option<Arc<dyn executor::TaskRuntime>>> {
        let domain = self
            .1
            .upgrade()
            .ok_or_else(|| error("DXF executor Domain closed"))?;
        let runtime = TaskRuntimeBinding::acquire(domain, task).map_err(error)?;
        Ok(Some(Arc::new(runtime)))
    }

    fn GetTaskExecInfoByExecID(
        &self,
        _: &executor::Context,
        node: &str,
    ) -> executor::Result<Vec<executor::TaskExecInfo>> {
        self.0
            .GetTaskExecInfoByExecID((), node.to_owned())
            .map_err(error)?
            .into_iter()
            .map(|info| {
                self.0
                    .GetTaskByID((), info.TaskBase.ID)
                    .map(|value| executor::TaskExecInfo {
                        TaskBase: task(value).TaskBase,
                    })
                    .map_err(error)
            })
            .collect()
    }
    fn GetTasksInStates(
        &self,
        _: &executor::Context,
        states: &[executor::TaskState],
    ) -> executor::Result<Vec<executor::Task>> {
        let states = states
            .iter()
            .map(|value| {
                match value {
                    executor::TaskState::Pending => "pending",
                    executor::TaskState::Running => "running",
                    executor::TaskState::Modifying => "modifying",
                    executor::TaskState::Pausing => "pausing",
                    executor::TaskState::Reverting => "reverting",
                    executor::TaskState::Succeed => "succeed",
                    executor::TaskState::Reverted => "reverted",
                    executor::TaskState::Failed => "failed",
                }
                .into()
            })
            .collect();
        self.0
            .GetTasksInStates((), states)
            .map(|values| values.into_iter().map(task).collect())
            .map_err(error)
    }
    fn GetTaskByID(&self, _: &executor::Context, id: i64) -> executor::Result<executor::Task> {
        self.0.GetTaskByID((), id).map(task).map_err(error)
    }
    fn GetSubtasksByExecIDAndStepAndStates(
        &self,
        _: &executor::Context,
        id: &str,
        task_id: i64,
        step: i64,
        states: &[executor::SubtaskState],
    ) -> executor::Result<Vec<executor::Subtask>> {
        self.0
            .GetSubtasksByExecIDAndStepAndStates(
                (),
                id.to_owned(),
                task_id,
                step,
                states.iter().copied().map(state).collect(),
            )
            .map(|values| values.into_iter().map(subtask).collect())
            .map_err(error)
    }
    fn GetFirstSubtaskInStates(
        &self,
        _: &executor::Context,
        id: &str,
        task_id: i64,
        step: i64,
        states: &[executor::SubtaskState],
    ) -> executor::Result<Option<executor::Subtask>> {
        self.0
            .GetFirstSubtaskInStates(
                (),
                id.to_owned(),
                task_id,
                step,
                states.iter().copied().map(state).collect(),
            )
            .map(|value| value.map(subtask))
            .map_err(error)
    }
    fn StartSubtask(&self, _: &executor::Context, id: i64, node: &str) -> executor::Result<()> {
        self.0.StartSubtask((), id, node.to_owned()).map_err(error)
    }
    fn FinishSubtask(
        &self,
        _: &executor::Context,
        node: &str,
        id: i64,
        meta: &[u8],
    ) -> executor::Result<()> {
        self.0
            .FinishSubtask((), node.to_owned(), id, meta.to_vec())
            .map_err(error)
    }
    fn FailSubtask(
        &self,
        _: &executor::Context,
        node: &str,
        id: i64,
        failure: &executor::ExecutorError,
    ) -> executor::Result<()> {
        self.0
            .FailSubtask(
                (),
                node.to_owned(),
                id,
                Some(storage::Error::new(failure.to_string())),
            )
            .map_err(error)
    }
    fn UpdateSubtaskStateAndError(
        &self,
        _: &executor::Context,
        node: &str,
        id: i64,
        status: executor::SubtaskState,
        failure: Option<&executor::ExecutorError>,
    ) -> executor::Result<()> {
        self.0
            .UpdateSubtaskStateAndError(
                (),
                node.to_owned(),
                id,
                state(status),
                failure.map(|failure| storage::Error::new(failure.to_string())),
            )
            .map_err(error)
    }
    fn InitMeta(&self, _: &executor::Context, node: &str, role: &str) -> executor::Result<()> {
        self.0
            .InitMeta(
                (),
                node.to_owned(),
                if role.is_empty() {
                    astersql_sessionctx_vardef::ServiceScope.Load()
                } else {
                    role.to_owned()
                },
            )
            .map_err(error)
    }
    fn RecoverMeta(&self, _: &executor::Context, node: &str, role: &str) -> executor::Result<()> {
        self.0
            .RecoverMeta(
                (),
                node.to_owned(),
                if role.is_empty() {
                    astersql_sessionctx_vardef::ServiceScope.Load()
                } else {
                    role.to_owned()
                },
            )
            .map_err(error)
    }
    fn CancelSubtask(&self, _: &executor::Context, node: &str, id: i64) -> executor::Result<()> {
        self.0.CancelSubtask((), node.to_owned(), id).map_err(error)
    }
    fn PauseSubtasks(&self, _: &executor::Context, node: &str, id: i64) -> executor::Result<()> {
        self.0.PauseSubtasks((), node.to_owned(), id).map_err(error)
    }
    fn RunningSubtasksBack2Pending(
        &self,
        _: &executor::Context,
        subtasks: &[executor::SubtaskBase],
    ) -> executor::Result<()> {
        self.0
            .RunningSubtasksBack2Pending(
                (),
                subtasks
                    .iter()
                    .map(|subtask| storage::proto::SubtaskBase {
                        ID: subtask.ID,
                        TaskID: subtask.TaskID,
                        Step: subtask.Step,
                        ExecID: subtask.ExecID.clone(),
                        State: state(subtask.State),
                        ..Default::default()
                    })
                    .collect(),
            )
            .map_err(error)
    }
    fn UpdateSubtaskCheckpoint(
        &self,
        _: &executor::Context,
        id: i64,
        value: &dyn std::any::Any,
    ) -> executor::Result<()> {
        let value = value
            .downcast_ref::<serde_json::Value>()
            .ok_or_else(|| error("invalid backfill checkpoint type"))?;
        self.0
            .ExecuteSQLWithNewSession(
                (),
                "UPDATE mysql.tidb_background_subtask SET checkpoint = %? WHERE id = %?",
                vec![value.to_string().into(), id.into()],
            )
            .map_err(error)?;
        Ok(())
    }
    fn GetSubtaskCheckpoint(&self, _: &executor::Context, id: i64) -> executor::Result<String> {
        self.0.GetSubtaskCheckpoint((), id).map_err(error)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct TaskSummary {
    index_kv_size: u64,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct TaskMeta {
    job: serde_json::Value,
    ele_ids: Vec<i64>,
    ele_type_key: String,
    cloud_storage_uri: String,
    estimate_row_size: i64,
    merge_temp_index: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    summary: Option<TaskSummary>,
    version: i32,
}
#[derive(serde::Serialize, serde::Deserialize)]
struct SubtaskMeta {
    physical_table_id: i64,
    #[serde(default)]
    row_start: Option<String>,
    #[serde(default)]
    row_end: Option<String>,
    #[serde(default, rename = "start-key")]
    start_key: Option<String>,
    #[serde(default, rename = "end-key")]
    end_key: Option<String>,
    #[serde(default)]
    ts: u64,
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Checkpoint {
    next_key: String,
    row_count: i64,
}
struct Extension {
    domain: Weak<Domain>,
    manager: storage::TaskManager,
    node_resource: executor::NodeResource,
}
impl executor::Extension for Extension {
    fn IsIdempotent(&self, _: &executor::Subtask) -> bool {
        true
    }
    fn GetStepExecutor(
        &self,
        task: &executor::Task,
    ) -> executor::Result<Arc<dyn executor::StepExecutor>> {
        if !matches!(
            task.TaskBase.Step,
            astersql_dxf_framework_proto::BackfillStepReadIndex
                | astersql_dxf_framework_proto::BackfillStepMergeSort
                | astersql_dxf_framework_proto::BackfillStepWriteAndIngest
                | astersql_dxf_framework_proto::BackfillStepMergeTempIndex
        ) {
            return Err(error(format!(
                "unknown backfill step {}",
                task.TaskBase.Step
            )));
        }
        let meta: TaskMeta = serde_json::from_slice(&task.Meta).map_err(error)?;
        let job = astersql_meta_model::group_3::Job::decode(
            &serde_json::to_vec(&meta.job).map_err(error)?,
        )
        .map_err(error)?;
        astersql_testkit_testfailpoint::inject_value(
            "github.com/pingcap/tidb/pkg/ddl/beforeGetUserTableForBackfillStep",
            &serde_json::to_string(&meta.job).map_err(error)?,
        );
        let step = ReadIndex::new(
            self.domain
                .upgrade()
                .ok_or_else(|| error("DXF Domain is closed"))?,
            self.manager.clone(),
            job,
            meta.ele_ids,
            meta.merge_temp_index,
            meta.cloud_storage_uri,
        )
        .with_resource(
            self.node_resource.GetStepResource(&task.TaskBase),
            meta.estimate_row_size.max(0) as usize,
        );
        if matches!(
            task.TaskBase.Step,
            astersql_dxf_framework_proto::BackfillStepMergeSort
                | astersql_dxf_framework_proto::BackfillStepWriteAndIngest
        ) {
            return Ok(Arc::new(cloud::CloudStep::new(step, task.TaskBase.Step)?));
        }
        let step = Arc::new(step);
        #[cfg(test)]
        if !step.merge {
            read_steps_for_test()
                .lock()
                .unwrap()
                .insert(step.job.id, Arc::downgrade(&step));
        }
        Ok(step)
    }
    fn IsRetryableError(&self, failure: &executor::ExecutorError) -> bool {
        failure.0.contains("write conflict")
            || failure.0.contains("region")
            || failure.0.contains("not leader")
    }
}
pub(super) struct ReadIndex {
    domain: Arc<Domain>,
    manager: storage::TaskManager,
    job: Arc<astersql_meta_model::group_3::Job>,
    pipeline: Mutex<Option<Arc<super::modify_column_pipeline::Pipeline>>>,
    resource: Mutex<executor::StepResource>,
    average_row_size: usize,
    index_ids: Vec<i64>,
    merge: bool,
    cloud_storage_uri: String,
    write_limiter: Arc<dyn astersql_ingestor_ingestctrl::localhelper::StoreWriteLimiter>,
}
// The framework context and KV transport have different cancellation types.
// Keep both tokens tied to the real subtask for all peer writes, and join the
// bridge before releasing the subtask's import controls.
pub(super) struct ImportControl {
    pub(super) options: astersql_kv::SSTImportOptions,
    cloud_cancelled: Arc<AtomicBool>,
    local: astersql_ingestor_ingestctrl::CancellationToken,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl Drop for ImportControl {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.options.context.cancel();
        self.local.cancel();
        self.cloud_cancelled.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct NativeWriteLimiter {
    inner: Arc<dyn astersql_ingestor_ingestctrl::localhelper::StoreWriteLimiter>,
    local: astersql_ingestor_ingestctrl::CancellationToken,
}
impl astersql_kv::SSTWriteLimiter for NativeWriteLimiter {
    fn WaitN(
        &self,
        context: &astersql_kv::Context,
        store: u64,
        bytes: usize,
    ) -> Result<(), astersql_kv::errors::SharedError> {
        if context.is_cancelled() {
            return Err(astersql_kv::errors::SharedError::new(
                astersql_ingestor_ingestctrl::Error::Cancelled,
            ));
        }
        self.inner
            .WaitN(
                &self.local,
                store,
                bytes
                    .try_into()
                    .map_err(|_| astersql_kv::errors::New("SST write bytes overflow"))?,
            )
            // Preserve the typed cancellation error. Capturing/resolving a new
            // stack here delays shutdown of large debug binaries after the
            // limiter has already stopped, and discards the original cause.
            .map_err(astersql_kv::errors::SharedError::new)
    }
}

impl ReadIndex {
    pub(super) fn new(
        domain: Arc<Domain>,
        manager: storage::TaskManager,
        job: astersql_meta_model::group_3::Job,
        index_ids: Vec<i64>,
        merge: bool,
        cloud_storage_uri: String,
    ) -> Self {
        let speed = job
            .reorg_meta
            .as_ref()
            .map_or(0, |meta| meta.GetMaxWriteSpeed());
        let cpu = job
            .reorg_meta
            .as_ref()
            .map_or(1, |meta| meta.GetConcurrency())
            .max(1);
        Self {
            domain,
            manager,
            job: Arc::new(job),
            pipeline: Mutex::new(None),
            resource: Mutex::new(executor::StepResource {
                CPU: cpu,
                Memory: 0,
            }),
            average_row_size: 0,
            index_ids,
            merge,
            cloud_storage_uri,
            write_limiter: Arc::new(
                astersql_ingestor_ingestctrl::localhelper::newStoreWriteLimiter(speed as isize),
            ),
        }
    }
    fn with_resource(mut self, resource: executor::StepResource, average_row_size: usize) -> Self {
        self.resource = Mutex::new(resource);
        self.average_row_size = average_row_size;
        self
    }
    pub(super) fn import_control(&self, context: &executor::Context) -> ImportControl {
        let local = astersql_ingestor_ingestctrl::CancellationToken::default();
        let options = astersql_kv::SSTImportOptions {
            context: astersql_kv::Context::new(),
            write_limiter: Some(Arc::new(NativeWriteLimiter {
                inner: self.write_limiter.clone(),
                local: local.clone(),
            })),
        };
        let cloud_cancelled = Arc::new(AtomicBool::new(false));
        let cloud = cloud_cancelled.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let ctx = context.clone();
        let kv = options.context.clone();
        let token = local.clone();
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                if ctx.Done() || kv.is_cancelled() {
                    token.cancel();
                    kv.cancel();
                    cloud.store(true, Ordering::Release);
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        ImportControl {
            options,
            cloud_cancelled,
            local,
            stop,
            worker: Some(worker),
        }
    }
}
struct PipelineGuard<'a> {
    step: &'a ReadIndex,
    pipeline: Arc<super::modify_column_pipeline::Pipeline>,
    producer: Option<JoinHandle<()>>,
}
impl Drop for PipelineGuard<'_> {
    fn drop(&mut self) {
        self.pipeline.shutdown();
        if let Some(producer) = self.producer.take() {
            let _ = producer.join();
        }
        if let Ok(mut current) = self.step.pipeline.lock() {
            if current
                .as_ref()
                .is_some_and(|value| Arc::ptr_eq(value, &self.pipeline))
            {
                *current = None;
            }
        }
    }
}
impl ReadIndex {
    fn run_pipeline(
        &self,
        ctx: &executor::Context,
        subtask: &mut executor::Subtask,
        physical: i64,
        mut start: Vec<u8>,
        end: Vec<u8>,
        mut checkpoint: Checkpoint,
    ) -> executor::Result<()> {
        use super::modify_column_pipeline::{Pipeline, ReadTask};
        use astersql_resourcemanager_pool_workerpool::RecvTimeoutError;
        use base64::Engine;
        if start >= end {
            return Ok(());
        }
        let mut ranges = self
            .domain
            .storage_handle()
            .with_storage(|store| store.DDLRegionRanges(&start, &end))
            .map_err(error)?
            .unwrap_or_else(|| vec![(start.clone(), end.clone())]);
        ranges.sort_by(|left, right| left.0.cmp(&right.0));
        let mut frontier = start.clone();
        let mut tasks = Vec::with_capacity(ranges.len());
        for (lower, upper) in ranges {
            let lower = lower.max(start.clone());
            let upper = if upper.is_empty() {
                end.clone()
            } else {
                upper.min(end.clone())
            };
            if lower >= upper {
                continue;
            }
            if lower != frontier {
                return Err(error(
                    "DDL region ranges do not cover the subtask continuously",
                ));
            }
            frontier = upper.clone();
            tasks.push(ReadTask {
                physical,
                start: lower,
                end: upper,
            });
        }
        if frontier != end {
            return Err(error("DDL region ranges omit the subtask tail"));
        }
        let subtask_id = subtask.SubtaskBase.ID;
        let import = self.import_control(ctx);
        let resource = self.resource.lock().map_err(error)?.clone();
        let summaries = Arc::new(Mutex::new(
            vec![super::modify_column_cloud_meta::SortedMeta::default(); self.index_ids.len()],
        ));
        let cloud_store = if self.cloud_storage_uri.is_empty() {
            None
        } else {
            Some(
                super::modify_column_cloud_store::CloudStore::open(
                    &self.cloud_storage_uri,
                    import.cloud_cancelled.clone(),
                )
                .map_err(error)?,
            )
        };
        let cloud = if let Some(store) = &cloud_store {
            let keyspace = self
                .domain
                .storage_handle()
                .with_storage(|store| store.DDLKeyspaceID())
                .map_err(error)?;
            let codec = if keyspace == u32::MAX {
                astersql_store_copr::network_backend::KeyCodec::v1()
            } else {
                astersql_store_copr::network_backend::KeyCodec::v2(String::new(), keyspace)
                    .map_err(error)?
            };
            let writers = resource.CPU.max(1) as i64;
            let divisor = writers
                .checked_mul(2)
                .and_then(|count| count.checked_mul(self.index_ids.len() as i64))
                .ok_or_else(|| error("cloud writer memory divisor overflow"))?;
            if divisor == 0 {
                return Err(error("cloud read has no index groups"));
            }
            Some(super::modify_column_pipeline::CloudWriteConfig {
                store: store.clone(),
                prefix: format!("{}/{}", subtask.SubtaskBase.TaskID, subtask_id),
                indexes: self.index_ids.clone(),
                memory_per_index: resource.Memory.max(0) as u64 / divisor as u64,
                key_prefix: codec.encode_key(&[]),
                summaries: summaries.clone(),
            })
        } else {
            None
        };
        let pipeline = Pipeline::start(
            self.domain.clone(),
            self.job.clone(),
            self.index_ids.clone(),
            resource.CPU,
            self.average_row_size,
            import.options.clone(),
            cloud,
        );
        *self.pipeline.lock().map_err(error)? = Some(pipeline.clone());
        let _guard = PipelineGuard {
            step: self,
            pipeline: pipeline.clone(),
            producer: Some(pipeline.feed(tasks)),
        };
        let mut completed = std::collections::BTreeMap::new();
        while start < end {
            if ctx.Done() {
                return Err(error("distributed index backfill cancelled"));
            }
            if let Some(failure) = pipeline.context.OperatorErr() {
                return Err(error(failure));
            }
            let ack = match pipeline.results.recv_timeout(Duration::from_millis(20)) {
                Ok(Some(ack)) => ack,
                Err(RecvTimeoutError::Timeout) => continue,
                Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(error(pipeline.context.OperatorErr().map_or_else(
                        || "distributed index workers stopped before completion".into(),
                        |failure| failure.to_string(),
                    )));
                }
            };
            if ack.start < start || completed.insert(ack.start.clone(), ack).is_some() {
                return Err(error("duplicate distributed index batch acknowledgement"));
            }
            let mut advanced = false;
            while let Some(ack) = completed.remove(&start) {
                if ack.context.next_key <= start || ack.context.next_key > end {
                    return Err(error("distributed index checkpoint outside its range"));
                }
                start = ack.context.next_key;
                checkpoint.row_count += ack.context.scan_count;
                advanced = true;
            }
            if advanced {
                checkpoint.next_key = base64::engine::general_purpose::STANDARD.encode(&start);
                self.manager
                    .ExecuteSQLWithNewSession(
                        (),
                        "UPDATE mysql.tidb_background_subtask SET checkpoint = %? WHERE id = %?",
                        vec![
                            serde_json::to_string(&checkpoint).map_err(error)?.into(),
                            subtask_id.into(),
                        ],
                    )
                    .map_err(error)?;
                self.manager
                    .UpdateSubtaskSummary(
                        (),
                        subtask_id,
                        storage::execute::SubtaskSummary {
                            RowCount: checkpoint.row_count,
                        },
                    )
                    .map_err(error)?;
            }
        }
        pipeline.finish().map_err(error)?;
        if let Some(store) = cloud_store {
            let fields = super::modify_column_cloud_meta::ExternalFields {
                meta_groups: summaries.lock().map_err(error)?.clone(),
                ele_ids: self.index_ids.clone(),
                ..Default::default()
            };
            let mut internal = serde_json::from_slice(&subtask.Meta).map_err(error)?;
            subtask.Meta = super::modify_column_cloud_meta::write(
                store.as_ref(),
                &mut internal,
                &fields,
                format!("{}/{}/meta.json", subtask.SubtaskBase.TaskID, subtask_id),
            )
            .map_err(error)?;
        }
        Ok(())
    }
}
impl executor::StepExecutor for ReadIndex {
    fn TaskMetaModified(&self, _: &executor::Context, bytes: &[u8]) -> executor::Result<()> {
        if self.merge {
            return Ok(());
        } // Go mergeTempIndexExecutor currently ignores modifications.
        let meta: TaskMeta = serde_json::from_slice(bytes).map_err(error)?;
        let next = astersql_meta_model::group_3::Job::decode(
            &serde_json::to_vec(&meta.job).map_err(error)?,
        )
        .map_err(error)?;
        let next = next
            .reorg_meta
            .as_ref()
            .ok_or_else(|| error("modified DXF reorg metadata missing"))?;
        let current = self
            .job
            .reorg_meta
            .as_ref()
            .ok_or_else(|| error("DXF reorg metadata missing"))?;
        current.SetBatchSize(next.GetBatchSize());
        // Go chooses local/cloud behavior from the URI held by the initialized
        // executor, rather than allowing incoming task metadata to switch it.
        if self.cloud_storage_uri.is_empty() {
            current.SetMaxWriteSpeed(next.GetMaxWriteSpeed());
            self.write_limiter
                .UpdateLimit(next.GetMaxWriteSpeed() as isize);
        }
        Ok(())
    }
    fn ResourceModified(
        &self,
        _: &executor::Context,
        resource: &executor::StepResource,
    ) -> executor::Result<()> {
        if self.merge {
            return Ok(());
        }
        let pipeline = self
            .pipeline
            .lock()
            .map_err(error)?
            .clone()
            .ok_or_else(|| error("no subtask running"))?;
        pipeline.tune(resource.CPU);
        *self.resource.lock().map_err(error)? = resource.clone();
        Ok(())
    }
    fn RunSubtask(
        &self,
        ctx: &executor::Context,
        subtask: &mut executor::Subtask,
    ) -> executor::Result<()> {
        use base64::Engine;
        let codec = base64::engine::general_purpose::STANDARD;
        let meta: SubtaskMeta = serde_json::from_slice(&subtask.Meta).map_err(error)?;
        let end = codec
            .decode(
                meta.row_end
                    .as_ref()
                    .or(meta.end_key.as_ref())
                    .ok_or_else(|| error("DXF subtask end key missing"))?,
            )
            .map_err(error)?;
        let stored = self
            .manager
            .GetSubtaskCheckpoint((), subtask.SubtaskBase.ID)
            .map_err(error)?;
        let mut checkpoint: Checkpoint = if stored.is_empty() || stored.trim() == "{}" {
            Checkpoint {
                next_key: meta
                    .row_start
                    .clone()
                    .or(meta.start_key.clone())
                    .ok_or_else(|| error("DXF subtask start key missing"))?,
                row_count: 0,
            }
        } else {
            serde_json::from_str(&stored).map_err(error)?
        };
        if !self.cloud_storage_uri.is_empty() && !self.merge {
            checkpoint = Checkpoint {
                next_key: meta
                    .row_start
                    .clone()
                    .or(meta.start_key.clone())
                    .ok_or_else(|| error("cloud read range start missing"))?,
                row_count: 0,
            };
        }
        let mut start = codec.decode(&checkpoint.next_key).map_err(error)?;
        let reorg = self
            .job
            .reorg_meta
            .as_ref()
            .ok_or_else(|| error("distributed MODIFY reorg metadata missing"))?;
        if !self.merge {
            return self.run_pipeline(ctx, subtask, meta.physical_table_id, start, end, checkpoint);
        }
        let import = self.import_control(ctx);
        let mut session = ConcreteSession::new(self.domain.clone());
        session.SetInRestrictedSQL(true);
        while start < end {
            if ctx.Done() {
                return Err(error("distributed index backfill cancelled"));
            }
            session.execute("BEGIN").map_err(error)?;
            #[cfg(test)]
            astersql_testkit_testfailpoint::inject_value(
                "github.com/pingcap/tidb/pkg/ddl/scanRecordExec",
                &serde_json::to_string(
                    &serde_json::json!({"id":self.job.id,"reorg_meta":&self.job.reorg_meta}),
                )
                .map_err(error)?,
            );

            let request = astersql_ddl::backfilling::IndexBackfillBatch {
                schema_id: self.job.schema_id,
                table_id: self.job.table_id,
                index_ids: self.index_ids.clone(),
                task: astersql_ddl::backfilling::ReorgBackfillTask {
                    physical_table_id: meta.physical_table_id,
                    start_key: start.clone(),
                    end_key: end.clone(),
                    ..Default::default()
                },
                batch_size: reorg.GetBatchSize().max(1) as usize,
                resource_group: reorg.ResourceGroupName.clone(),
                sql_mode: reorg.SQLMode as i64,
            };
            let result = match if self.merge {
                super::modify_column_backfill::merge(&mut session, request)
            } else {
                super::system_session::backfill_index_batch_with_ingest_options(
                    &mut session,
                    request,
                    Some(self.job.id),
                    import.options.clone(),
                )
            } {
                Ok(result) => result,
                Err(failure) => {
                    let _ = session.execute("ROLLBACK");
                    return Err(error(failure));
                }
            };
            session.execute("COMMIT").map_err(error)?;
            if result.next_key <= start {
                return Err(error("distributed index checkpoint did not advance"));
            }
            start = result.next_key;
            checkpoint.next_key = codec.encode(&start);
            checkpoint.row_count += result.scan_count;
            let checkpoint_json = serde_json::to_string(&checkpoint).map_err(error)?;
            self.manager
                .ExecuteSQLWithNewSession(
                    (),
                    "UPDATE mysql.tidb_background_subtask SET checkpoint = %? WHERE id = %?",
                    vec![checkpoint_json.into(), subtask.SubtaskBase.ID.into()],
                )
                .map_err(error)?;
            self.manager
                .UpdateSubtaskSummary(
                    (),
                    subtask.SubtaskBase.ID,
                    storage::execute::SubtaskSummary {
                        RowCount: checkpoint.row_count,
                    },
                )
                .map_err(error)?;
        }
        Ok(())
    }
}

/// The owner dispatches the durable range and waits for actual executor completion.
pub(super) fn run(
    session: &mut ConcreteSession,
    request: astersql_ddl::backfilling::IndexBackfillBatch,
    job: &mut astersql_meta_model::group_3::Job,
    merging: bool,
    worker: Arc<NodeService>,
    cloud_storage_uri: String,
) -> Result<astersql_ddl::backfilling::BackfillTaskContext, String> {
    use base64::Engine;
    let codec = base64::engine::general_purpose::STANDARD;
    let manager = if astersql_config_kerneltype::IsNextGen() {
        storage::GetDXFSvcTaskMgr().map_err(|e| e.to_string())?
    } else {
        session.ImportTaskManager().map_err(|e| e.to_string())?
    };
    let encoded_job = job.encode(false).map_err(|e| e.to_string())?;
    let reorg = job
        .reorg_meta
        .as_ref()
        .ok_or("distributed MODIFY reorg metadata missing")?;
    let node = worker.node.clone();
    let mut key = astersql_ddl::index::TaskKeyBuilder::new()
        .set_multi_schema(job.multi_schema_info.as_ref().map(|info| info.seq as i64))
        .set_merge_temporary_index(merging)
        .build(job.id);
    let keyspace = session
        .domain
        .storage_handle()
        .with_storage(|store| store.GetKeyspace());
    if astersql_config_kerneltype::IsNextGen() {
        key = format!("{keyspace}/{key}");
    }
    let step = if merging {
        astersql_dxf_framework_proto::BackfillStepMergeTempIndex
    } else {
        astersql_dxf_framework_proto::BackfillStepReadIndex
    };
    if !merging && reorg.UseCloudStorage && cloud_storage_uri.is_empty() {
        return Err("cloud backfill storage URI is empty".into());
    }
    let estimate_row_size = session
        .execute(&format!(
            "select AVG_ROW_LENGTH from information_schema.tables where TIDB_TABLE_ID = {}",
            request.table_id
        ))
        .ok()
        .and_then(|rows| {
            rows.first()
                .and_then(|set| set.rows.front())
                .and_then(|row| row.first())
                .and_then(|v| v.parse::<i64>().ok())
        })
        .unwrap_or(0);
    let existing = manager.GetTaskByKeyWithHistory((), key.clone());
    let task_id = match existing {
        Ok(task) => task.ID,
        Err(failure) if failure.to_string().contains("not found") => {
            let meta = TaskMeta {
                job: serde_json::from_slice(&encoded_job).map_err(|e| e.to_string())?,
                ele_ids: request.index_ids.clone(),
                ele_type_key: codec.encode(b"_idx_"),
                cloud_storage_uri: cloud_storage_uri.clone(),
                estimate_row_size,
                merge_temp_index: merging,
                summary: None,
                version: 1,
            };
            manager
                .CreateTask(
                    (),
                    key,
                    storage::proto::Backfill,
                    keyspace.clone(),
                    astersql_ddl::index::adjust_concurrency(
                        reorg.GetConcurrency().max(1) as usize,
                        manager
                            .GetCPUCountOfNodeByRole((), reorg.TargetScope.clone())
                            .map_err(|e| e.to_string())?
                            .max(1) as usize,
                    ) as i32,
                    reorg.TargetScope.clone(),
                    reorg.MaxNodeCount,
                    storage::proto::ExtraParams {
                        PauseOnKVDiskFull: true,
                        ..Default::default()
                    },
                    serde_json::to_vec(&meta).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?
        }
        Err(failure) => return Err(failure.to_string()),
    };
    let task_manager: Arc<dyn scheduler::TaskManager> =
        Arc::new(scheduler::StorageTaskManagerAdapter::new(manager.clone()));
    let node_manager = Arc::new(scheduler::NodeManager::new());
    let slot_manager = Arc::new(scheduler::SlotManager::new());
    slot_manager.set_next_gen(astersql_config_kerneltype::IsNextGen());
    node_manager
        .refresh_nodes(task_manager.as_ref(), &slot_manager)
        .map_err(|e| e.to_string())?;
    slot_manager
        .update(&node_manager, task_manager.as_ref())
        .map_err(|e| e.to_string())?;
    let task = task_manager
        .task_by_id(task_id)
        .map_err(|e| e.to_string())?;
    let (reserve_node, allocated) = slot_manager.can_reserve(&task.base);
    if !allocated {
        return Err("no DXF slots available for distributed MODIFY".into());
    }
    slot_manager.reserve(&task.base, &reserve_node);
    let param = scheduler::Param {
        task_manager: task_manager.clone(),
        node_manager: node_manager.clone(),
        slot_manager: slot_manager.clone(),
        server_id: node,
        allocated_slots: allocated,
        node_resource: None,
    };
    let mut txn = session
        .domain
        .storage_handle()
        .with_storage(|store| store.Begin(&[]))
        .map_err(|e| e.to_string())?;
    let table = astersql_meta::TransactionMutator::new(txn.as_mut())
        .get_table(request.schema_id, request.table_id)?
        .ok_or("distributed MODIFY table missing")?;
    txn.Rollback().map_err(|e| e.to_string())?;
    let physical_ids = table
        .GetPartitionInfo()
        .map(|partition| {
            partition
                .Definitions
                .iter()
                .map(|definition| definition.ID)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![table.ID]);
    let planner = Arc::new(scheduler::BaseScheduler::new(
        task,
        param.clone(),
        Arc::new(Planner {
            domain: session.domain.clone(),
            physical_ids,
            index_ids: request.index_ids.clone(),
            step,
            cloud_storage_uri: cloud_storage_uri.clone(),
            node_manager: node_manager.clone(),
            mock_node: worker.node.starts_with("mock-"),
        }),
    ));
    use scheduler::Scheduler;
    planner.init().map_err(|e| e.to_string())?;
    let mut balancer = scheduler::Balancer::new(param);
    loop {
        if worker.stopped.load(Ordering::Acquire) {
            planner.close();
            slot_manager.unreserve(&planner.task().base, &reserve_node);
            return Err("distributed MODIFY node stopped".into());
        }
        node_manager
            .refresh_nodes(task_manager.as_ref(), &slot_manager)
            .map_err(|e| e.to_string())?;
        planner.schedule_once().map_err(|e| e.to_string())?;
        let latest = planner.task();
        if latest.base.is_done() {
            planner.close();
            slot_manager.unreserve(&latest.base, &reserve_node);
            if latest.base.state != scheduler::TASK_STATE_SUCCEED {
                return Err(latest
                    .error
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| format!("distributed MODIFY task {}", latest.base.state)));
            }
            break;
        }
        balancer
            .balance(&[planner.clone()])
            .map_err(|e| e.to_string())?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let subtasks = manager
        .GetSubtasksWithHistory((), task_id, step)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    for stored in subtasks {
        if stored.State != storage::proto::SubtaskStateSucceed {
            return Err("distributed index subtask did not succeed".into());
        }
        let checkpoint = manager
            .GetSubtaskCheckpoint((), stored.ID)
            .map_err(|e| e.to_string())?;
        let _: Checkpoint = serde_json::from_str(&checkpoint).map_err(|e| e.to_string())?;
    }
    // Go updateDistTaskRowCount replaces reorg progress with the read-index
    // summaries; it does not add the earlier column or merge stages again.
    job.set_row_count(
        manager
            .GetSubtaskRowCount(
                (),
                task_id,
                astersql_dxf_framework_proto::BackfillStepReadIndex,
            )
            .map_err(|e| e.to_string())?,
    );
    Ok(astersql_ddl::backfilling::BackfillTaskContext {
        next_key: request.task.end_key,
        scan_count: 0,
        done: true,
        ..Default::default()
    })
}

struct Planner {
    domain: Arc<Domain>,
    physical_ids: Vec<i64>,
    index_ids: Vec<i64>,
    step: i64,
    cloud_storage_uri: String,
    node_manager: Arc<scheduler::NodeManager>,
    mock_node: bool,
}
impl Planner {
    fn ranges(&self, node_count: usize) -> Result<Vec<SubtaskMeta>, String> {
        use astersql_ddl::backfilling_dist_scheduler as planning;
        use base64::Engine;
        let codec = base64::engine::general_purpose::STANDARD;
        let merging = self.step == astersql_dxf_framework_proto::BackfillStepMergeTempIndex;
        let mut result = Vec::new();
        for &physical in &self.physical_ids {
            let mut bounds = Vec::new();
            if merging {
                for &index in &self.index_ids {
                    bounds.push((
                        index,
                        astersql_ddl::reorg::encode_temporary_index_range(physical, index, index),
                    ));
                }
            } else {
                let prefix = super::kv::Key(astersql_tablecodec::GenTableRecordPrefix(physical).0);
                let upper = prefix.PrefixNext();
                let range =
                    self.domain
                        .storage_handle()
                        .with_storage(|store| -> Result<_, String> {
                            let version = store
                                .CurrentVersion(super::kv::GlobalTxnScope)
                                .map_err(|e| e.to_string())?;
                            if version.Ver == 0 {
                                return Err("invalid storage current version 0".into());
                            }
                            let snapshot = store.GetSnapshot(version);
                            let mut first = snapshot
                                .Iter(prefix.clone(), Some(upper.clone()))
                                .map_err(|e| e.to_string())?;
                            let start = first.Valid().then(|| first.Key().0);
                            first.Close();
                            let Some(start) = start else { return Ok(None) };
                            let mut last = snapshot
                                .IterReverse(Some(upper), Some(prefix))
                                .map_err(|e| e.to_string())?;
                            let end = if last.Valid() {
                                last.Key().Next().0
                            } else {
                                super::kv::Key(start.clone()).Next().0
                            };
                            last.Close();
                            Ok(Some((start, end)))
                        })?;
                if let Some(range) = range {
                    bounds.push((0, range));
                }
            }
            for (index, (start, end)) in bounds {
                let planned = planning::retry_region_plan(
                    || {
                        self.domain
                            .storage_handle()
                            .with_storage(|store| store.DDLRegionRanges(&start, &end))
                            .map_err(|e| planning::PlanError::RegionScan(e.to_string()))
                            .map(|ranges| {
                                ranges
                                    .unwrap_or_else(|| vec![(start.clone(), end.clone())])
                                    .into_iter()
                                    .map(|(start_key, end_key)| planning::RegionMeta {
                                        start_key,
                                        end_key,
                                    })
                                    .collect()
                            })
                    },
                    |regions| {
                        if merging {
                            planning::generate_temporary_index_plan(
                                physical,
                                index,
                                start.clone(),
                                end.clone(),
                                regions,
                                node_count,
                            )
                        } else {
                            planning::try_generate_plan_for_physical_table(
                                physical,
                                &start,
                                &end,
                                regions,
                                node_count,
                                !self.cloud_storage_uri.is_empty(),
                                || {
                                    let version = self
                                        .domain
                                        .storage_handle()
                                        .with_storage(|store| {
                                            store.CurrentVersion(super::kv::GlobalTxnScope)
                                        })
                                        .map_err(|e| {
                                            planning::PlanError::TimestampAllocation(e.to_string())
                                        })?;
                                    if version.Ver == 0 {
                                        return Err(planning::PlanError::TimestampAllocation(
                                            "invalid storage current version 0".into(),
                                        ));
                                    }
                                    Ok(version.Ver)
                                },
                            )
                        }
                    },
                    |delay| {
                        std::thread::sleep(delay);
                        Ok(())
                    },
                )
                .map_err(|error| format!("DXF region planning: {error:?}"))?;
                for meta in planned {
                    result.push(SubtaskMeta {
                        physical_table_id: physical,
                        row_start: (!merging).then(|| codec.encode(meta.row_start)),
                        row_end: (!merging).then(|| codec.encode(meta.row_end)),
                        start_key: merging
                            .then(|| codec.encode(meta.legacy_sorted_kv_meta.start_key)),
                        end_key: merging.then(|| codec.encode(meta.legacy_sorted_kv_meta.end_key)),
                        ts: meta.ts,
                    });
                }
            }
        }
        Ok(result)
    }
}
impl scheduler::Extension for Planner {
    fn on_next_subtasks_batch(
        &self,
        handle: &dyn scheduler::TaskHandle,
        task: &mut scheduler::Task,
        nodes: &[String],
        next_step: i64,
    ) -> scheduler::Result<Vec<Vec<u8>>> {
        let node_count = if astersql_config_kerneltype::IsNextGen() {
            task.base.max_node_count.max(1) as usize
        } else {
            nodes.len()
        };
        if matches!(
            next_step,
            astersql_dxf_framework_proto::BackfillStepMergeSort
                | astersql_dxf_framework_proto::BackfillStepWriteAndIngest
        ) {
            return self
                .cloud_plans(handle, task, node_count, next_step)
                .map_err(scheduler::SchedulerError::new);
        }
        self.ranges(node_count)
            .map_err(scheduler::SchedulerError::new)?
            .iter()
            .map(|meta| {
                serde_json::to_vec(meta).map_err(|e| scheduler::SchedulerError::new(e.to_string()))
            })
            .collect()
    }
    fn on_done(
        &self,
        _: &dyn scheduler::TaskHandle,
        _: &mut scheduler::Task,
    ) -> scheduler::Result<()> {
        Ok(())
    }
    fn eligible_instances(&self, _: &scheduler::Task) -> scheduler::Result<Vec<String>> {
        Ok(Vec::new())
    }
    fn is_retryable_error(&self, failure: &scheduler::SchedulerError) -> bool {
        failure.0.contains("write conflict")
            || failure.0.contains("region")
            || failure.0.contains("not leader")
    }
    fn next_step(&self, task: &scheduler::TaskBase) -> i64 {
        match task.step {
            scheduler::STEP_INIT => self.step,
            astersql_dxf_framework_proto::BackfillStepReadIndex
                if !self.cloud_storage_uri.is_empty() =>
            {
                astersql_dxf_framework_proto::BackfillStepMergeSort
            }
            astersql_dxf_framework_proto::BackfillStepMergeSort => {
                astersql_dxf_framework_proto::BackfillStepWriteAndIngest
            }
            _ => scheduler::STEP_DONE,
        }
    }
    fn on_prepare(
        &self,
        _: &dyn scheduler::TaskHandle,
        _: &mut scheduler::Task,
    ) -> scheduler::Result<()> {
        Ok(())
    }
    fn modify_meta(
        &self,
        old_meta: &[u8],
        modifications: &[scheduler::Modification],
    ) -> scheduler::Result<Vec<u8>> {
        let convert = |e: Box<dyn std::error::Error>| scheduler::SchedulerError::new(e.to_string());
        let mut meta: TaskMeta =
            serde_json::from_slice(old_meta).map_err(|e| convert(Box::new(e)))?;
        let mut job = astersql_meta_model::group_3::Job::decode(
            &serde_json::to_vec(&meta.job).map_err(|e| convert(Box::new(e)))?,
        )
        .map_err(|e| scheduler::SchedulerError::new(e.to_string()))?;
        let reorg = job.reorg_meta.as_ref().ok_or_else(|| {
            scheduler::SchedulerError::new("distributed MODIFY reorg metadata missing")
        })?;
        for change in modifications {
            match change.kind.as_str() {
                astersql_dxf_framework_proto::ModifyBatchSize => {
                    reorg.SetBatchSize(change.to as i32)
                }
                astersql_dxf_framework_proto::ModifyMaxWriteSpeed => {
                    reorg.SetMaxWriteSpeed(change.to)
                }
                _ => {}
            }
        }
        meta.job = serde_json::from_slice(
            &job.encode(false)
                .map_err(|e| scheduler::SchedulerError::new(e.to_string()))?,
        )
        .map_err(|e| convert(Box::new(e)))?;
        serde_json::to_vec(&meta).map_err(|e| convert(Box::new(e)))
    }
}

// Target runtimes remain submit-only; DXF borrows their Store/Domain through
// the same lifetime handle that prevents cross-keyspace idle eviction.
type TargetBinding = (Weak<dyn astersql_domain_crossks::Store>, Weak<Domain>);
fn target_domains() -> &'static Mutex<std::collections::HashMap<usize, TargetBinding>> {
    static TARGETS: OnceLock<Mutex<std::collections::HashMap<usize, TargetBinding>>> =
        OnceLock::new();
    TARGETS.get_or_init(Default::default)
}
pub(super) fn register_target_runtime(
    store: &Arc<dyn astersql_domain_crossks::Store>,
    domain: &Arc<Domain>,
) {
    let id = Arc::as_ptr(store) as *const () as usize;
    let mut targets = target_domains().lock().unwrap();
    targets.retain(|_, (store, domain)| store.strong_count() > 0 && domain.strong_count() > 0);
    targets.insert(id, (Arc::downgrade(store), Arc::downgrade(domain)));
}
pub(super) struct TaskRuntimeBinding {
    domain: Arc<Domain>,
    handle: Mutex<Option<astersql_domain_crossks::RuntimeHandle>>,
    released: AtomicBool,
}
impl TaskRuntimeBinding {
    pub(super) fn acquire(domain: Arc<Domain>, task: &executor::Task) -> Result<Self, String> {
        let target = &task.TaskBase.Keyspace;
        let current = domain
            .storage_handle()
            .with_storage(|store| store.GetKeyspace());
        if current == *target {
            return Ok(Self {
                domain,
                handle: Mutex::new(None),
                released: AtomicBool::new(false),
            });
        }
        let manager = domain
            .cross_ks_manager()
            .ok_or("cross-keyspace task runtime manager unavailable")?;
        let handle = manager
            .acquire(target, &format!("DXF/executor/{}", task.TaskBase.ID))
            .map_err(|e| e.0)?;
        let store = handle.store();
        let id = Arc::as_ptr(&store) as *const () as usize;
        let target_domain = target_domains()
            .lock()
            .map_err(|e| e.to_string())?
            .get(&id)
            .and_then(|(_, domain)| domain.upgrade())
            .ok_or("cross-keyspace task Domain unavailable")?;
        let binding = Self {
            domain: target_domain,
            handle: Mutex::new(Some(handle)),
            released: AtomicBool::new(false),
        };
        executor::TaskRuntime::CheckTaskKeyspace(&binding, target).map_err(|e| e.to_string())?;
        Ok(binding)
    }
}
impl executor::TaskRuntime for TaskRuntimeBinding {
    fn AsAny(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    fn Release(&self) {
        self.released.store(true, Ordering::Release);
        self.handle.lock().unwrap().take();
    }

    fn CheckTaskKeyspace(&self, keyspace: &str) -> executor::Result<()> {
        if self.released.load(Ordering::Acquire) || self.domain.is_closed() {
            return Err(error("task runtime Domain closed"));
        }
        let actual = self
            .domain
            .storage_handle()
            .with_storage(|store| store.GetKeyspace());
        if actual != keyspace
            || self
                .handle
                .lock()
                .map_err(error)?
                .as_ref()
                .is_some_and(|handle| handle.store().keyspace() != keyspace)
        {
            return Err(error(format!(
                "task keyspace {keyspace} differs from runtime Store {actual}"
            )));
        }
        Ok(())
    }
}
struct UnavailableRuntime(String);
impl executor::TaskRuntime for UnavailableRuntime {
    fn CheckTaskKeyspace(&self, _: &str) -> executor::Result<()> {
        Err(error(&self.0))
    }
}

/// A Domain owns one background DXF executor manager; factory bindings use weak
/// references so node shutdown does not keep the Domain or its sessions alive.
pub(super) struct NodeService {
    manager: Option<Arc<executor::Manager>>,
    node: String,
    task_manager: storage::TaskManager,
    stopped: AtomicBool,
}
fn nodes() -> &'static Mutex<std::collections::HashMap<String, Weak<NodeService>>> {
    static NODES: OnceLock<Mutex<std::collections::HashMap<String, Weak<NodeService>>>> =
        OnceLock::new();
    NODES.get_or_init(Default::default)
}
impl NodeService {
    pub(super) fn start(session: &mut ConcreteSession) -> Result<Arc<Self>, String> {
        let node = match astersql_domain_infosync::GetServerInfo() {
            Ok(info) => {
                if info.IP.contains(':') {
                    format!("[{}]:{}", info.IP, info.Port)
                } else {
                    format!("{}:{}", info.IP, info.Port)
                }
            }
            Err(_) => format!(
                "mock-{:p}-{}",
                Arc::as_ptr(&session.domain),
                std::process::id()
            ),
        };
        let keyspace = session
            .domain
            .storage_handle()
            .with_storage(|store| store.GetKeyspace());
        let local_manager = session.ImportTaskManager().map_err(|e| e.to_string())?;
        storage::SetTaskManager(local_manager.clone());
        if astersql_config_kerneltype::IsNextGen()
            && keyspace != astersql_domain_crossks::SYSTEM_KEYSPACE
        {
            let manager = session
                .domain
                .cross_ks_manager()
                .ok_or("SYSTEM task runtime manager unavailable")?;
            let runtime = manager
                .get_or_create(astersql_domain_crossks::SYSTEM_KEYSPACE)
                .map_err(|e| e.0)?;
            let store = runtime.store();
            let id = Arc::as_ptr(&store) as *const () as usize;
            let target = target_domains()
                .lock()
                .map_err(|e| e.to_string())?
                .get(&id)
                .and_then(|(_, domain)| domain.upgrade())
                .ok_or("SYSTEM task Domain unavailable")?;
            let task_manager = ConcreteSession::new(target)
                .ImportTaskManager()
                .map_err(|e| e.to_string())?;
            storage::SetDXFSvcTaskMgr(task_manager.clone());
            // Go InitDistTaskLoop initializes SYSTEM access, then returns before
            // constructing an executor manager in a user keyspace.
            return Ok(Arc::new(Self {
                manager: None,
                node,
                task_manager,
                stopped: AtomicBool::new(false),
            }));
        }
        let mut bindings = nodes().lock().map_err(|e| e.to_string())?;
        if let Some(worker) = bindings.get(&node).and_then(Weak::upgrade) {
            if !worker.stopped.load(Ordering::Acquire) {
                return Ok(worker);
            }
        }
        executor::RegisterTaskType(
            storage::proto::Backfill.into(),
            Arc::new(|ctx, task, mut param| {
                let binding = nodes()
                    .lock()
                    .unwrap()
                    .get(&param.execID)
                    .and_then(Weak::upgrade);
                match param
                    .TaskRuntime
                    .as_ref()
                    .and_then(|runtime| runtime.AsAny())
                    .and_then(|runtime| runtime.downcast_ref::<TaskRuntimeBinding>())
                {
                    Some(runtime) if binding.is_some() => {
                        param.Extension = Arc::new(Extension {
                            domain: Arc::downgrade(&runtime.domain),
                            manager: binding.as_ref().unwrap().task_manager.clone(),
                            node_resource: param.nodeRc.clone(),
                        });
                    }
                    _ => {
                        param.Extension = Arc::new(UnavailableExtension);
                        param.TaskRuntime = Some(Arc::new(UnavailableRuntime(
                            "DXF task runtime binding unavailable".into(),
                        )));
                    }
                }
                executor::NewBaseTaskExecutor(executor::Context::Child(&ctx), task, param)
            }),
        );
        let task_manager = local_manager;
        let resources = storage::GetNodeResource().ok_or("DXF node resources unavailable")?;
        let manager = executor::NewManager(
            executor::Context::Background(),
            node.clone(),
            Arc::new(TaskTable(
                task_manager.clone(),
                Arc::downgrade(&session.domain),
            )),
            executor::NodeResource {
                TotalCPU: resources.TotalCPU,
                TotalMem: resources.TotalMem as i64,
                TotalDisk: resources.TotalDisk as u64,
            },
        )
        .map_err(|e| e.to_string())?;
        let worker = Arc::new(Self {
            manager: Some(manager),
            node,
            task_manager,
            stopped: AtomicBool::new(false),
        });
        bindings.insert(worker.node.clone(), Arc::downgrade(&worker));
        drop(bindings);
        worker
            .manager
            .as_ref()
            .unwrap()
            .InitMeta()
            .map_err(|e| e.to_string())?;
        worker
            .manager
            .as_ref()
            .unwrap()
            .Start()
            .map_err(|e| e.to_string())?;
        Ok(worker)
    }
    pub(super) fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(manager) = &self.manager {
            manager.Stop();
        }
    }
}
impl Drop for NodeService {
    fn drop(&mut self) {
        self.stop();
    }
}

struct UnavailableExtension;
impl executor::Extension for UnavailableExtension {
    fn IsIdempotent(&self, _: &executor::Subtask) -> bool {
        true
    }
    fn GetStepExecutor(
        &self,
        _: &executor::Task,
    ) -> executor::Result<Arc<dyn executor::StepExecutor>> {
        Err(error("DXF executor node binding unavailable"))
    }
    fn IsRetryableError(&self, _: &executor::ExecutorError) -> bool {
        false
    }
}

#[cfg(test)]
fn read_steps_for_test() -> &'static Mutex<std::collections::HashMap<i64, Weak<ReadIndex>>> {
    static STEPS: OnceLock<Mutex<std::collections::HashMap<i64, Weak<ReadIndex>>>> =
        OnceLock::new();
    STEPS.get_or_init(|| Mutex::new(Default::default()))
}
#[cfg(test)]
pub(super) fn read_index_for_test(job_id: i64) -> Option<Arc<ReadIndex>> {
    read_steps_for_test()
        .lock()
        .unwrap()
        .get(&job_id)
        .and_then(Weak::upgrade)
}
#[cfg(test)]
impl ReadIndex {
    pub(super) fn closed_pipeline_workers_for_test(&self) -> (u32, u32) {
        self.pipeline
            .lock()
            .unwrap()
            .as_ref()
            .map_or((0, 0), |pipeline| pipeline.closed_workers())
    }
}

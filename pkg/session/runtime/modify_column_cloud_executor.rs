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

//! Cloud merge/import executors for durable MODIFY index subtasks.
use super::super::{modify_column_cloud_meta as wire, modify_column_cloud_store::CloudStore};
use super::*;
use astersql_ingestor_globalsort as sort;
use astersql_ingestor_ingestctrl::local::engineapi::Engine as _;
use std::sync::atomic::AtomicU64;

#[cfg(test)]
#[path = "modify_column_cloud_executor_test.rs"]
mod tests;

#[derive(Clone)]
enum Active {
    Merge(Arc<sort::merge::MergeOperator>),
    Import(
        Arc<sort::engine::ExternalEngineAdapter>,
        sort::reader::CancellationToken,
    ),
}
pub(super) struct CloudStep {
    base: ReadIndex,
    step: i64,
    backend: Mutex<Option<Arc<super::super::import_sst::Backend>>>,
    active: Mutex<Option<Active>>,
    rows: AtomicU64,
}
impl CloudStep {
    pub(super) fn new(base: ReadIndex, step: i64) -> executor::Result<Self> {
        if base.cloud_storage_uri.is_empty() {
            return Err(error("cloud backfill storage URI is empty"));
        }
        Ok(Self {
            base,
            step,
            backend: Mutex::new(None),
            active: Mutex::new(None),
            rows: AtomicU64::new(0),
        })
    }
    fn backend(&self) -> executor::Result<Arc<super::super::import_sst::Backend>> {
        self.backend
            .lock()
            .map_err(error)?
            .clone()
            .ok_or_else(|| error("local backend not found"))
    }
    fn run_merge(
        &self,
        subtask: &mut executor::Subtask,
        store: Arc<CloudStore>,
        token: sort::reader::CancellationToken,
    ) -> executor::Result<()> {
        let mut internal = wire::read(store.as_ref(), &subtask.Meta).map_err(error)?;
        let mut fields: wire::ExternalFields =
            serde_json::from_value(internal.clone()).map_err(error)?;
        let sorted = Arc::new(Mutex::new(wire::SortedMeta::default()));
        let summary = sorted.clone();
        let resource = self.base.resource.lock().map_err(error)?.clone();
        let op = Arc::new(
            sort::merge::NewMergeOperator(
                token,
                store.clone(),
                resource.Memory / i64::from(resource.CPU.max(1)),
                format!("{}/{}", subtask.SubtaskBase.TaskID, subtask.SubtaskBase.ID),
                astersql_ingestor_simplesst::writer::DefaultBlockSize,
                Some(Arc::new(move |next| {
                    // Writer summaries contain valid raw byte keys, so converting
                    // them to wire metadata cannot produce invalid base64.
                    summary
                        .lock()
                        .unwrap()
                        .merge(&wire::SortedMeta::from_merge(next))
                        .unwrap();
                })),
                None,
                resource.CPU.max(1) as usize,
                true,
                sort::OnDuplicateKey::Error,
            )
            .map_err(error)?,
        );
        *self.active.lock().map_err(error)? = Some(Active::Merge(op.clone()));
        sort::merge::MergeOverlappingFiles(&fields.data_files, &op).map_err(error)?;
        fields.meta_groups = vec![sorted.lock().map_err(error)?.clone()];
        subtask.Meta = wire::write(
            store.as_ref(),
            &mut internal,
            &fields,
            format!(
                "{}/{}/meta.json",
                subtask.SubtaskBase.TaskID, subtask.SubtaskBase.ID
            ),
        )
        .map_err(error)?;
        Ok(())
    }
    fn run_import(
        &self,
        subtask: &mut executor::Subtask,
        store: Arc<CloudStore>,
        token: sort::reader::CancellationToken,
        control: &ImportControl,
    ) -> executor::Result<()> {
        let internal = wire::read(store.as_ref(), &subtask.Meta).map_err(error)?;
        let fields: wire::ExternalFields =
            serde_json::from_value(internal.clone()).map_err(error)?;
        let mut all = wire::SortedMeta::default();
        for group in &fields.meta_groups {
            all.merge(group).map_err(error)?;
        }
        // Older tasks carried the single sorted group inline.
        if fields.meta_groups.is_empty() {
            all.merge(&fields.legacy).map_err(error)?;
        }
        let ts = internal
            .get("ts")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| error("cloud subtask timestamp missing"))?;
        let keys = if fields.range_job_keys.is_empty() {
            &fields.range_split_keys
        } else {
            &fields.range_job_keys
        };
        let job_keys = keys
            .iter()
            .map(|key| wire::decode(key).map_err(error))
            .collect::<executor::Result<Vec<_>>>()?;
        let split_keys = fields
            .range_split_keys
            .iter()
            .map(|key| wire::decode(key).map_err(error))
            .collect::<executor::Result<Vec<_>>>()?;
        let snapshot = self
            .base
            .domain
            .storage_handle()
            .with_storage(|store| store.GetSnapshot(astersql_kv::MaxVersion));
        let table = astersql_meta::SnapshotReader::new(snapshot)
            .get_table(self.base.job.schema_id, self.base.job.table_id)
            .map_err(error)?
            .ok_or_else(|| error("DDL cloud backfill table missing"))?;
        let indexes = self
            .base
            .index_ids
            .iter()
            .map(|id| {
                table
                    .Indices
                    .iter()
                    .find(|index| index.ID == *id)
                    .ok_or_else(|| error("DDL cloud backfill index missing"))
            })
            .collect::<executor::Result<Vec<_>>>()?;
        let index_id = match fields.ele_ids.as_slice() {
            [id] => indexes
                .iter()
                .find(|index| index.ID == *id)
                .map_or(0, |index| index.ID),
            [] => {
                indexes
                    .first()
                    .ok_or_else(|| error("DDL cloud backfill indexes missing"))?
                    .ID
            }
            ids => return Err(error(format!("unexpected EleIDs count {}", ids.len()))),
        };
        let (_, id) = astersql_lightning_backend::MakeUUID(&table.Name.L, index_id);
        let resource = self.base.resource.lock().map_err(error)?.clone();
        let engine = sort::engine::NewExternalEngine(
            store,
            fields.data_files,
            fields.stat_files,
            wire::decode(all.start.as_deref().unwrap_or("")).map_err(error)?,
            wire::decode(all.end.as_deref().unwrap_or("")).map_err(error)?,
            job_keys,
            split_keys,
            resource.CPU.max(1),
            ts,
            all.size as i64,
            0,
            true,
            resource.Memory,
            sort::OnDuplicateKey::Error,
            format!("{}/{}", subtask.SubtaskBase.TaskID, subtask.SubtaskBase.ID),
        )
        .map_err(error)?;
        let backend = self.backend()?;
        backend
            .set_import_options(control.options.clone())
            .map_err(error)?;
        let source = backend
            .register_external(id, engine, token.clone())
            .map_err(error)?;
        *self.active.lock().map_err(error)? = Some(Active::Import(source.clone(), token));
        let result = backend
            .import_external_native(&control.local, id)
            .map_err(error);
        self.rows
            .store(source.ImportedStatistics().1 as u64, Ordering::Release);
        let cleanup = backend.cleanup_external(id).map_err(error);
        result.and(cleanup)
    }
}
struct ActiveGuard<'a>(&'a CloudStep);
impl Drop for ActiveGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.0.active.lock() {
            *active = None;
        }
    }
}
impl executor::StepExecutor for CloudStep {
    fn Init(&self, _: &executor::Context) -> executor::Result<()> {
        if self.step == astersql_dxf_framework_proto::BackfillStepWriteAndIngest {
            let resource = self.base.resource.lock().map_err(error)?.clone();
            let keyspace = self
                .base
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
            *self.backend.lock().map_err(error)? = Some(
                super::super::import_sst::Backend::new_with_key_prefix(
                    self.base.domain.clone(),
                    self.base.job.id,
                    Default::default(),
                    codec.encode_key(&[]),
                    resource.CPU.max(1) as usize,
                )
                .map_err(error)?,
            );
        }
        Ok(())
    }
    fn RunSubtask(
        &self,
        ctx: &executor::Context,
        subtask: &mut executor::Subtask,
    ) -> executor::Result<()> {
        if ctx.Done() {
            return Err(error("distributed cloud backfill cancelled"));
        }
        let control = self.base.import_control(ctx);
        let store = CloudStore::open(
            &self.base.cloud_storage_uri,
            control.cloud_cancelled.clone(),
        )
        .map_err(error)?;
        let token = sort::reader::CancellationToken::from_cancellation_flag(
            control.cloud_cancelled.clone(),
        );
        let _active = ActiveGuard(self);
        if self.step == astersql_dxf_framework_proto::BackfillStepMergeSort {
            self.run_merge(subtask, store, token)
        } else {
            self.run_import(subtask, store, token, &control)
        }
    }
    fn Cleanup(&self, _: &executor::Context) -> executor::Result<()> {
        self.backend.lock().map_err(error)?.take();
        Ok(())
    }
    fn RealtimeSummary(&self) -> Option<executor::SubtaskSummary> {
        let active = self.active.lock().ok()?.clone();
        let rows = if let Some(Active::Import(engine, _)) = active {
            engine.ImportedStatistics().1 as u64
        } else {
            self.rows.load(Ordering::Acquire)
        };
        Some(executor::SubtaskSummary { RowCount: rows })
    }
    fn ResetSummary(&self) {
        self.rows.store(0, Ordering::Release);
    }
    fn TaskMetaModified(&self, _: &executor::Context, bytes: &[u8]) -> executor::Result<()> {
        if self.step != astersql_dxf_framework_proto::BackfillStepWriteAndIngest {
            return Ok(());
        }
        let meta: TaskMeta = serde_json::from_slice(bytes).map_err(error)?;
        let next = astersql_meta_model::group_3::Job::decode(
            &serde_json::to_vec(&meta.job).map_err(error)?,
        )
        .map_err(error)?;
        let speed = next
            .reorg_meta
            .as_ref()
            .ok_or_else(|| error("modified DXF reorg metadata missing"))?
            .GetMaxWriteSpeed();
        self.base
            .job
            .reorg_meta
            .as_ref()
            .ok_or_else(|| error("DXF reorg metadata missing"))?
            .SetMaxWriteSpeed(speed);
        self.base.write_limiter.UpdateLimit(speed as isize);
        Ok(())
    }
    fn ResourceModified(
        &self,
        _: &executor::Context,
        resource: &executor::StepResource,
    ) -> executor::Result<()> {
        // Import's memory-only update is intentionally ignored by Go when the
        // actual backend concurrency already matches, including idle subtasks.
        if self.step == astersql_dxf_framework_proto::BackfillStepWriteAndIngest
            && self.base.resource.lock().map_err(error)?.CPU == resource.CPU
        {
            return Ok(());
        }
        let active = self
            .active
            .lock()
            .map_err(error)?
            .clone()
            .ok_or_else(|| error("no subtask running"))?;
        match active {
            Active::Merge(op) => op.Tune(resource.CPU.max(1) as usize).map_err(error)?,
            Active::Import(engine, token) => {
                engine
                    .ResourceHandle()
                    .UpdateResourceWith(&token, resource.CPU, resource.Memory)
                    .map_err(error)?;
                self.backend()?
                    .set_worker_concurrency(resource.CPU.max(1) as usize);
            }
        }
        *self.base.resource.lock().map_err(error)? = resource.clone();
        Ok(())
    }
}

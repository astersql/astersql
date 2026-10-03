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

// Import Into 最小子任务（minimal task）执行与后处理 checksum 汇总。
//
// 本模块驱动 importer 管线处理单个输入分片（chunk）：local sort（本地排序）
// 从本地 engine 打开 writer；global sort（全局排序）使用 encode-and-sort 算子提供的 writer。
// 另提供从后处理元数据构建最终校验和（checksum）的确定性逻辑。

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use astersql_dxf_framework_taskexecutor_execute::Collector;
use astersql_errors as errors;
use astersql_executor_importer as importer;
use astersql_lightning_backend::EngineWriter;
use astersql_lightning_backend_encode::Context;
use astersql_lightning_verification as verification;

use crate::proto::{PostProcessStepMeta, importStepMinimalTask};

use astersql_dxf_framework_proto::subtask::{StepResource, Subtask};
use astersql_dxf_framework_taskexecutor_execute as execute;
use astersql_meta_autoid::AllocatorType;

/// Remote checksum manager supplied by the NextGen host. The executor owns
/// its lifetime and always calls Close, including on checksum failure.
pub trait PostProcessChecksumManager: Send + Sync {
    fn Checksum(&self, ctx: &execute::Context) -> Result<importer::RemoteChecksum, String>;
    fn Close(&self);
}

/// Runtime boundary for storage/session operations in Go postProcess.
pub trait PostProcessHost: Send + Sync {
    fn RebaseAllocatorBases(
        &self,
        ctx: &execute::Context,
        max_ids: &HashMap<AllocatorType, i64>,
        plan: &importer::Plan,
    ) -> Result<(), String>;
    fn RemoteChecksumClassic(
        &self,
        ctx: &execute::Context,
        plan: &importer::Plan,
    ) -> Result<importer::RemoteChecksum, String>;
    fn NewNextGenChecksumManager(
        &self,
        ctx: &execute::Context,
        task_id: i64,
        plan: &importer::Plan,
    ) -> Result<Box<dyn PostProcessChecksumManager>, String>;
}

#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct PostProcessWire {
    Checksum: HashMap<i64, crate::proto::Checksum>,
    DeletedRowsChecksum: crate::proto::Checksum,
    #[serde(
        rename = "too-many-conflicts-from-index",
        alias = "TooManyConflictsFromIndex"
    )]
    TooManyConflictsFromIndex: bool,
    MaxIDs: HashMap<String, i64>,
}

fn decodePostProcessMeta(bytes: &[u8]) -> Result<PostProcessStepMeta, String> {
    let wire: PostProcessWire = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let mut max_ids = HashMap::new();
    for (kind, value) in wire.MaxIDs {
        let allocator = match kind.as_str() {
            "0" | "_tidb_rowid" => AllocatorType::RowId,
            "1" | "auto_increment" => AllocatorType::AutoIncrement,
            "2" | "auto_random" => AllocatorType::AutoRandom,
            "3" | "sequence" => AllocatorType::Sequence,
            _ => return Err(format!("unknown allocator type {kind}")),
        };
        max_ids.insert(allocator, value);
    }
    Ok(PostProcessStepMeta {
        Checksum: wire.Checksum,
        DeletedRowsChecksum: wire.DeletedRowsChecksum,
        TooManyConflictsFromIndex: wire.TooManyConflictsFromIndex,
        MaxIDs: max_ids,
    })
}

/// Go's postProcessStepExecutor, with host-supplied storage and SQL/TiKV IO.
pub struct PostProcessStepExecutor {
    task_id: i64,
    plan: importer::Plan,
    host: Arc<dyn PostProcessHost>,
    framework: Option<execute::FrameworkInfo>,
}

pub fn NewPostProcessStepExecutor(
    task_id: i64,
    plan: importer::Plan,
    host: Arc<dyn PostProcessHost>,
) -> PostProcessStepExecutor {
    PostProcessStepExecutor {
        task_id,
        plan,
        host,
        framework: None,
    }
}

/// Parse the persisted task and dispatch the Go post-process step to its
/// executable host boundary. Task-level registration is owned by the import
/// task executor.
pub fn GetPostProcessStepExecutor(
    task: &astersql_dxf_framework_proto::task::Task,
    host: Arc<dyn PostProcessHost>,
) -> Result<Box<dyn execute::StepExecutor>, errors::SharedError> {
    let meta = crate::proto::TaskMeta::Unmarshal(&task.Meta)?;
    if task.Step != astersql_dxf_framework_proto::step::ImportStepPostProcess {
        return Err(errors::New(format!(
            "unknown step {} for import task {}",
            task.Step, task.ID
        )));
    }
    Ok(Box::new(NewPostProcessStepExecutor(
        task.ID, meta.Plan, host,
    )))
}

impl PostProcessStepExecutor {
    pub fn RunMeta(&self, bytes: &[u8]) -> Result<(), String> {
        self.RunMetaWithContext(&execute::Context::default(), bytes)
    }

    pub fn RunMetaWithContext(&self, ctx: &execute::Context, bytes: &[u8]) -> Result<(), String> {
        let meta = decodePostProcessMeta(bytes)?;
        runPostProcessWith(
            &meta,
            |meta| {
                self.host
                    .RebaseAllocatorBases(ctx, &meta.MaxIDs, &self.plan)
            },
            |checksum| {
                if astersql_config_kerneltype::IsNextGen() {
                    let manager =
                        self.host
                            .NewNextGenChecksumManager(ctx, self.task_id, &self.plan)?;
                    struct CloseGuard(Box<dyn PostProcessChecksumManager>);
                    impl Drop for CloseGuard {
                        fn drop(&mut self) {
                            self.0.Close();
                        }
                    }
                    let manager = CloseGuard(manager);
                    importer::VerifyChecksum(&self.plan, checksum, || manager.0.Checksum(ctx))
                } else {
                    importer::VerifyChecksum(&self.plan, checksum, || {
                        self.host.RemoteChecksumClassic(ctx, &self.plan)
                    })
                }
            },
        )
    }
}

impl execute::StepExecFrameworkInfo for PostProcessStepExecutor {
    fn restricted(&self) {}
    fn GetStep(&self) -> i64 {
        self.framework
            .as_ref()
            .map_or(0, execute::StepExecFrameworkInfo::GetStep)
    }
    fn GetResource(&self) -> Option<Arc<StepResource>> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetResource)
    }
    fn SetResource(&self, resource: Arc<StepResource>) {
        if let Some(framework) = &self.framework {
            framework.SetResource(resource);
        }
    }
    fn GetMeterRecorder(&self) -> Option<Arc<astersql_dxf_framework_metering::Recorder>> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetMeterRecorder)
    }
    fn GetCheckpointUpdateFunc(&self) -> Option<execute::CheckpointUpdateFunc> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetCheckpointUpdateFunc)
    }
    fn GetCheckpointFunc(&self) -> Option<execute::CheckpointGetFunc> {
        self.framework
            .as_ref()
            .and_then(execute::StepExecFrameworkInfo::GetCheckpointFunc)
    }
}

impl execute::StepExecutor for PostProcessStepExecutor {
    fn Init(&mut self, _: execute::Context) -> anyhow::Result<()> {
        Ok(())
    }
    fn RunSubtask(&mut self, ctx: execute::Context, subtask: &mut Subtask) -> anyhow::Result<()> {
        self.RunMetaWithContext(&ctx, &subtask.Meta)
            .map_err(|error| anyhow::anyhow!(error))
    }
    fn RealtimeSummary(&mut self) -> Option<&execute::SubtaskSummary> {
        None
    }
    fn ResetSummary(&mut self) {}
    fn Cleanup(&mut self, _: execute::Context) -> anyhow::Result<()> {
        Ok(())
    }
    fn TaskMetaModified(&mut self, _: execute::Context, _: Vec<u8>) -> anyhow::Result<()> {
        Ok(())
    }
    fn ResourceModified(&mut self, _: execute::Context, _: &StepResource) -> anyhow::Result<()> {
        Ok(())
    }
    fn SetFrameworkInfo(&mut self, info: execute::FrameworkInfo) {
        self.framework = Some(info);
    }
}

/// The Go executor copies its chunk and sets FileMeta.Loc immediately before
/// ProcessChunk. Carry that selected plan location across the importer host
/// parser boundary without changing the persisted chunk wire format.
pub(crate) struct LocatedImportChunk<'a> {
    pub(crate) chunk: &'a importer::Chunk,
    pub(crate) location: &'a str,
}

impl importer::ImportChunk for LocatedImportChunk<'_> {
    fn Key(&self) -> String {
        self.chunk.GetKey()
    }
    fn Path(&self) -> &str {
        &self.chunk.Path
    }
    fn FileSize(&self) -> i64 {
        self.chunk.FileSize
    }
    fn Offset(&self) -> i64 {
        self.chunk.Offset
    }
    fn EndOffset(&self) -> i64 {
        self.chunk.EndOffset
    }
    fn PrevRowIDMax(&self) -> i64 {
        self.chunk.PrevRowIDMax
    }
    fn RowIDMax(&self) -> i64 {
        self.chunk.RowIDMax
    }
    fn SourceType(&self) -> astersql_lightning_mydump::SourceType {
        self.chunk.Type
    }
    fn Compression(&self) -> astersql_lightning_mydump::Compression {
        self.chunk.Compression
    }
    fn Timestamp(&self) -> i64 {
        self.chunk.Timestamp
    }
    fn ParquetLocation(&self) -> Option<&str> {
        Some(self.location)
    }
}

/// 用真实 importer 管线执行一个输入分片。
///
/// local sort 从两个本地 engine 打开 writer；global sort 消费 encode-and-sort 算子构建的 writer。
/// Execute one input chunk with the real importer pipeline. Local sort opens
/// writers from the two local engines; global sort consumes writers built by
/// the encode-and-sort operator.
pub fn runImportMinimalTask(
    ctx: &Context,
    task: &mut importStepMinimalTask,
    data_writer: Option<Box<dyn EngineWriter>>,
    index_writer: Option<Box<dyn EngineWriter>>,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
) -> Result<(), errors::SharedError> {
    let chunk = LocatedImportChunk {
        chunk: &task.Chunk,
        location: task
            .SharedVars
            .TableImporter
            .LoadDataController
            .ParquetLocation(),
    };
    // 按 keyspace 初始化分组校验和，供本分片编码过程累加。
    let checksum = Arc::new(Mutex::new(verification::NewKVGroupChecksumWithKeyspace(
        &task.SharedVars.TableImporter.GetKeySpace(),
    )));

    if task
        .SharedVars
        .TableImporter
        .LoadDataController
        .Plan
        .IsLocalSort()
    {
        // local sort：必须已初始化 data/index 本地 engine，再走 ProcessChunk。
        let data_engine = task
            .SharedVars
            .DataEngine
            .as_ref()
            .ok_or_else(|| errors::New("local sort data engine is not initialized"))?;
        let index_engine = task
            .SharedVars
            .IndexEngine
            .as_ref()
            .ok_or_else(|| errors::New("local sort index engine is not initialized"))?;
        importer::ProcessChunkAndLogger(
            ctx,
            &chunk,
            &task.SharedVars.TableImporter,
            data_engine,
            index_engine,
            Some(Arc::clone(&checksum)),
            collector,
            &task.logger,
        )
        .map_err(errors::New)?;
    } else {
        // global sort：使用调用方注入的 data/index writer。
        let data_writer =
            data_writer.ok_or_else(|| errors::New("global sort data writer is not initialized"))?;
        let index_writer = index_writer
            .ok_or_else(|| errors::New("global sort index writer is not initialized"))?;
        importer::ProcessChunkWithWriterAndLogger(
            ctx,
            &chunk,
            &task.SharedVars.TableImporter,
            data_writer,
            index_writer,
            Some(Arc::clone(&checksum)),
            collector,
            &task.logger,
        )
        .map_err(errors::New)?;
    }

    // 将本分片校验和并入共享变量，供后续步骤汇总。
    let checksum = checksum
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    task.SharedVars.Checksum.Add(&checksum);
    Ok(())
}

/// 合并编码阶段各 KV 组 checksum，并减去冲突解决删除的行。
///
/// 这是 Go 后处理路径中确定性的一半；分配器基址调整与远程 checksum IO 仍由运行时集成。
/// Merge encode-step checksums and subtract rows removed by conflict
/// resolution. This is the deterministic half of Go's post-processing path;
/// allocator rebasing and remote checksum IO remain runtime integrations.
pub fn buildFinalChecksum(meta: &PostProcessStepMeta) -> verification::KVChecksum {
    let mut grouped = verification::NewKVGroupChecksumForAdd();
    // 按 group_id 还原原始 Size/KVs/Sum，再合并为单一 KVChecksum。
    for (group_id, checksum) in &meta.Checksum {
        grouped.AddRawGroup(*group_id, checksum.Size, checksum.KVs, checksum.Sum);
    }
    let mut result = grouped.MergedChecksum();
    // 冲突解决删除的行需从最终校验和中扣除。
    result.Sub(&meta.DeletedRowsChecksum.ToKVChecksum());
    result
}

/// Execute the Go post-process ordering around the deterministic checksum.
/// The runtime supplies allocator rebasing and the selected remote checksum
/// implementation (SQL for Classic or TiKV manager for NextGen).
pub fn runPostProcessWith(
    meta: &PostProcessStepMeta,
    rebase: impl FnOnce(&PostProcessStepMeta) -> Result<(), String>,
    verify: impl FnOnce(&verification::KVChecksum) -> Result<(), String>,
) -> Result<(), String> {
    rebase(meta)?;
    let final_checksum = buildFinalChecksum(meta);
    if meta.TooManyConflictsFromIndex {
        return Ok(());
    }
    verify(&final_checksum)
}

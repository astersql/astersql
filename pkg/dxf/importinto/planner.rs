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

// IMPORT INTO 规划器：逻辑计划、物理计划与各步骤 PipelineSpec 生成。
//
// 将 LogicalPlan（作业配置与 chunk 映射）按当前步骤生成 PhysicalPlan，
// 并序列化为子任务 meta；覆盖 encode/merge/ingest/冲突收集与解决/后处理。
// 关键约束是 step 边界与 Go 保持一致，避免子任务 meta 在不同步骤间漂移。

use std::any::Any;
use std::collections::HashMap;

use astersql_config_kerneltype as kerneltype;
use astersql_dxf_framework_proto as dxfproto;
use astersql_dxf_framework_storage as dxfstorage;
use astersql_errors as errors;
use astersql_executor_importer as importer;
use astersql_ingestor_globalsort as globalsort;
use astersql_ingestor_simplesst as simplesst;
use astersql_kv as kv;
use astersql_lightning_log::Logger;
use astersql_meta_autoid as autoid;
use astersql_objstore as objstore;
use astersql_parser_mysql as mysql;
use astersql_table as table;
use globalsort::Storage as _;
use std::ops::Deref;
use std::sync::Arc;

use crate::proto::*;

/// Planning inputs that are independent of the unfinished framework planner
/// crate. Previous typed metas keep the same step boundaries as Go and avoid
/// losing external-sort state while the framework's wire adapters are ported.
/// 规划上下文：下一步骤、并发/节点数，以及前序步骤已反序列化的 typed meta，
/// 在框架线格式适配未完成时保留外部排序状态边界。
#[derive(Clone, Default)]
pub struct PlanCtx {
    pub TaskID: i64,
    pub ThreadCnt: i32,
    pub GlobalSort: bool,
    pub NextTaskStep: dxfproto::Step,
    pub ExecuteNodesCnt: i32,
    pub PreviousSubtaskMetas: HashMap<dxfproto::Step, Vec<Vec<u8>>>,
    pub PreviousImportMetas: Vec<ImportStepMeta>,
    pub PreviousMergeMetas: Vec<MergeSortStepMeta>,
    pub PreviousWriteMetas: Vec<WriteIngestStepMeta>,
    pub PreviousCollectMetas: Vec<CollectConflictsStepMeta>,
    pub StorageContext: objstore::storage::Context,
    pub ObjectStore: Option<objstore::storage::StorageRef>,
    pub KVStore: Option<Arc<dyn kv::Storage + Send + Sync>>,
    pub CommitTS: Option<u64>,
    pub RegionSplitSize: i64,
    pub RegionSplitKeys: i64,
    pub NodeMemPerCore: i64,
    pub ControllerServices:
        Option<Arc<dyn Fn() -> importer::LoadDataControllerServices + Send + Sync>>,
    pub ImporterService: Option<Arc<dyn importer::TableImporterService>>,
    pub ForceMergeGroup: Option<String>,
    pub Table: Option<Arc<dyn table::Table>>,
}

/// 管线规格：可序列化为子任务 meta，并支持 downcast。
pub trait PipelineSpec: Send + Sync {
    fn ToSubtaskMeta(&self, plan_ctx: &PlanCtx) -> Result<Vec<u8>, errors::SharedError>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// 物理计划中处理器之间的输出链接。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LinkSpec {
    pub ProcessorID: usize,
}

/// 处理器输入描述（列类型与上游链接）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InputSpec {
    pub ColumnTypes: Vec<u8>,
    pub Links: Vec<LinkSpec>,
}

/// 处理器输出描述（下游链接列表）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct OutputSpec {
    pub Links: Vec<LinkSpec>,
}

/// 单个物理处理器：绑定一步 PipelineSpec 与步骤号。
pub struct ProcessorSpec {
    pub ID: usize,
    pub Input: InputSpec,
    pub Pipeline: Box<dyn PipelineSpec>,
    pub Output: OutputSpec,
    pub Step: dxfproto::Step,
}

/// 物理计划：一组有序处理器。
#[derive(Default)]
pub struct PhysicalPlan {
    pub Processors: Vec<ProcessorSpec>,
}

impl PhysicalPlan {
    /// 追加一个处理器。
    pub fn AddProcessor(&mut self, processor: ProcessorSpec) {
        self.Processors.push(processor);
    }

    /// 将指定步骤的所有处理器序列化为子任务 meta 字节列表。
    pub fn ToSubtaskMetas(
        &self,
        plan_ctx: &PlanCtx,
        step: dxfproto::Step,
    ) -> Result<Vec<Vec<u8>>, errors::SharedError> {
        self.Processors
            .iter()
            .filter(|processor| processor.Step == step)
            .map(|processor| processor.Pipeline.ToSubtaskMeta(plan_ctx))
            .collect()
    }
}

/// 逻辑计划：作业配置、可调度实例、chunk 映射与 prepare 外部路径。
pub struct LogicalPlan {
    pub JobID: i64,
    pub Plan: importer::Plan,
    pub Stmt: String,
    pub EligibleInstances: Vec<ServerInfo>,
    pub ChunkMap: HashMap<i32, Vec<importer::Chunk>>,
    pub PrepareMode: dxfproto::PrepareMode,
    pub PreparedChunkMapExternalPath: String,
    pub Logger: Logger,
    pub summary: importer::StepSummary,
}

struct SortStoreLease {
    store: objstore::storage::StorageRef,
    owned: bool,
}

impl Deref for SortStoreLease {
    type Target = objstore::storage::StorageRef;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl Drop for SortStoreLease {
    fn drop(&mut self) {
        if self.owned {
            self.store.Close();
        }
    }
}

impl Default for LogicalPlan {
    fn default() -> Self {
        Self {
            JobID: 0,
            Plan: importer::Plan::default(),
            Stmt: String::new(),
            EligibleInstances: Vec::new(),
            ChunkMap: HashMap::new(),
            PrepareMode: dxfproto::PrepareModeDisabled,
            PreparedChunkMapExternalPath: String::new(),
            Logger: astersql_lightning_log::L(),
            summary: importer::StepSummary::default(),
        }
    }
}

impl LogicalPlan {
    fn sort_store(&self, ctx: &PlanCtx) -> Result<SortStoreLease, errors::SharedError> {
        if let Some(store) = &ctx.ObjectStore {
            return Ok(SortStoreLease {
                store: store.clone(),
                owned: false,
            });
        }
        let store = objstore::storage::NewFromURL(&ctx.StorageContext, &self.Plan.CloudStorageURI)
            .map_err(|error| errors::New(error.to_string()))?;
        Ok(SortStoreLease { store, owned: true })
    }

    fn write_external_plan_meta(
        &self,
        ctx: &PlanCtx,
        specs: &mut [Box<dyn PipelineSpec>],
    ) -> Result<(), errors::SharedError> {
        if !ctx.GlobalSort {
            return Ok(());
        }
        let store = self.sort_store(ctx)?;
        for (index, spec) in specs.iter_mut().enumerate() {
            let path = globalsort::PlanMetaPath(
                ctx.TaskID,
                &dxfproto::Step2Str(dxfproto::ImportInto, ctx.NextTaskStep),
                index + 1,
            );
            let meta = spec.ToSubtaskMeta(ctx)?;
            let mut value: serde_json::Value =
                serde_json::from_slice(&meta).map_err(|error| errors::New(error.to_string()))?;
            let external_keys: &[&str] = if spec.as_any().is::<ImportSpec>() {
                &["Chunks", "SortedDataMeta", "SortedIndexMetas"]
            } else if spec.as_any().is::<MergeSortSpec>() {
                &[
                    "data-files",
                    "start-key",
                    "end-key",
                    "total-kv-size",
                    "total-kv-cnt",
                    "multiple-files-stats",
                    "conflict-info",
                ]
            } else if spec.as_any().is::<WriteIngestSpec>() {
                &[
                    "sorted-kv-meta",
                    "data-files",
                    "stat-files",
                    "range-job-keys",
                    "range-split-keys",
                ]
            } else if spec.as_any().is::<CollectConflictsSpec>()
                || spec.as_any().is::<ConflictResolutionSpec>()
            {
                &["infos"]
            } else {
                &[]
            };
            let mut external = serde_json::Map::new();
            for key in external_keys {
                if let Some(field) = value.as_object_mut().and_then(|value| value.remove(*key)) {
                    external.insert((*key).to_owned(), field);
                }
            }
            let bytes =
                serde_json::to_vec(&external).map_err(|error| errors::New(error.to_string()))?;
            store
                .WriteFile(&ctx.StorageContext, &path, &bytes)
                .map_err(|error| errors::New(error.to_string()))?;
            set_spec_external_path(spec.as_any_mut(), path);
        }
        Ok(())
    }
    /// 从 Plan 提取任务级 ExtraParams（手动恢复、prepare 模式等）。
    pub fn GetTaskExtraParams(&self) -> dxfproto::ExtraParams {
        dxfproto::ExtraParams {
            ManualRecovery: self.Plan.ManualRecovery,
            PrepareMode: self.PrepareMode,
            ..Default::default()
        }
    }

    /// 序列化为 TaskMeta 字节，供分布式任务持久化。
    pub fn ToTaskMeta(&self) -> Result<Vec<u8>, errors::SharedError> {
        TaskMeta {
            JobID: self.JobID,
            Plan: self.Plan.clone(),
            Stmt: self.Stmt.clone(),
            Summary: importer::Summary::default(),
            EligibleInstances: self.EligibleInstances.clone(),
            ChunkMap: self.ChunkMap.clone(),
            PreparedMetaExternalPath: self.PreparedChunkMapExternalPath.clone(),
        }
        .Marshal()
    }

    /// 从 TaskMeta 字节恢复逻辑计划字段。
    pub fn FromTaskMeta(&mut self, bytes: &[u8]) -> Result<(), errors::SharedError> {
        let task_meta = TaskMeta::Unmarshal(bytes)?;
        self.JobID = task_meta.JobID;
        self.Plan = task_meta.Plan;
        self.Stmt = task_meta.Stmt;
        self.EligibleInstances = task_meta.EligibleInstances;
        self.ChunkMap = task_meta.ChunkMap;
        self.PreparedChunkMapExternalPath = task_meta.PreparedMetaExternalPath;
        Ok(())
    }

    /// 按 PlanCtx.NextTaskStep 生成对应步骤的物理计划。
    pub fn ToPhysicalPlan(
        &mut self,
        plan_ctx: PlanCtx,
    ) -> Result<PhysicalPlan, errors::SharedError> {
        // 按步骤分派生成 PipelineSpec 列表。
        let mut specs: Vec<Box<dyn PipelineSpec>> = match plan_ctx.NextTaskStep {
            dxfproto::ImportStepImport | dxfproto::ImportStepEncodeAndSort => {
                generateImportSpecs(&plan_ctx, self)?
            }
            dxfproto::ImportStepMergeSort => generateMergeSortSpecs(&plan_ctx, self)?,
            dxfproto::ImportStepWriteAndIngest => generateWriteIngestSpecs(&plan_ctx, self)?,
            dxfproto::ImportStepCollectConflicts => generateCollectConflictsSpecs(&plan_ctx, self)?,
            dxfproto::ImportStepConflictResolution => {
                generateConflictResolutionSpecs(&plan_ctx, self)?
            }
            dxfproto::ImportStepPostProcess => vec![Box::new(PostProcessSpec {
                Schema: self.Plan.DBName.clone(),
                Table: self
                    .Plan
                    .TableInfo
                    .as_ref()
                    .map(|table| table.Name.L.clone())
                    .unwrap_or_default(),
            })],
            _ => Vec::new(),
        };
        if plan_ctx.NextTaskStep != dxfproto::ImportStepPostProcess {
            self.write_external_plan_meta(&plan_ctx, &mut specs)?;
        }
        let mut physical_plan = PhysicalPlan::default();
        let processor_count = specs.len();
        // 每个 spec 成为一个 processor，输出统一链接到占位 sink（processor_count）。
        for (id, spec) in specs.into_iter().enumerate() {
            let post_process = plan_ctx.NextTaskStep == dxfproto::ImportStepPostProcess;
            physical_plan.AddProcessor(ProcessorSpec {
                ID: id,
                Input: if post_process {
                    InputSpec {
                        ColumnTypes: vec![mysql::r#type::TypeLonglong; 5]
                            .into_iter()
                            .chain([mysql::r#type::TypeJSON])
                            .collect(),
                        Links: Vec::new(),
                    }
                } else {
                    InputSpec::default()
                },
                Pipeline: spec,
                Output: if post_process {
                    OutputSpec::default()
                } else {
                    OutputSpec {
                        Links: vec![LinkSpec {
                            ProcessorID: processor_count,
                        }],
                    }
                },
                Step: plan_ctx.NextTaskStep,
            });
        }
        Ok(physical_plan)
    }
}

/// encode/import 步骤的管线规格：携带 ImportStepMeta 与 Plan 副本。
pub struct ImportSpec {
    pub ImportStepMeta: ImportStepMeta,
    pub Plan: importer::Plan,
}

impl PipelineSpec for ImportSpec {
    fn ToSubtaskMeta(&self, _plan_ctx: &PlanCtx) -> Result<Vec<u8>, errors::SharedError> {
        self.ImportStepMeta.Marshal()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

macro_rules! impl_meta_spec {
    ($name:ident, $field:ident) => {
        pub struct $name {
            pub $field: $field,
        }
        impl PipelineSpec for $name {
            fn ToSubtaskMeta(&self, _plan_ctx: &PlanCtx) -> Result<Vec<u8>, errors::SharedError> {
                self.$field.Marshal()
            }
            fn as_any(&self) -> &dyn Any {
                self
            }
            fn as_any_mut(&mut self) -> &mut dyn Any {
                self
            }
        }
    };
}

impl_meta_spec!(MergeSortSpec, MergeSortStepMeta);
impl_meta_spec!(WriteIngestSpec, WriteIngestStepMeta);
impl_meta_spec!(CollectConflictsSpec, CollectConflictsStepMeta);
impl_meta_spec!(ConflictResolutionSpec, ConflictResolutionStepMeta);

/// 后处理步骤规格：聚合前序 checksum、删除行 checksum 与 MaxIDs。
pub struct PostProcessSpec {
    pub Schema: String,
    pub Table: String,
}

impl PipelineSpec for PostProcessSpec {
    fn ToSubtaskMeta(&self, plan_ctx: &PlanCtx) -> Result<Vec<u8>, errors::SharedError> {
        let encode_step = if plan_ctx.GlobalSort {
            dxfproto::ImportStepEncodeAndSort
        } else {
            dxfproto::ImportStepImport
        };
        let import_metas = if let Some(bytes) = plan_ctx.PreviousSubtaskMetas.get(&encode_step) {
            bytes
                .iter()
                .map(|bytes| ImportStepMeta::Unmarshal(bytes))
                .collect::<Result<Vec<_>, _>>()?
        } else {
            plan_ctx.PreviousImportMetas.clone()
        };
        let collect_metas = if let Some(bytes) = plan_ctx
            .PreviousSubtaskMetas
            .get(&dxfproto::ImportStepCollectConflicts)
        {
            bytes
                .iter()
                .map(|bytes| {
                    let value: serde_json::Value = serde_json::from_slice(bytes)
                        .map_err(|error| errors::New(error.to_string()))?;
                    let checksum = serde_json::from_value(
                        value
                            .get("checksum")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|error| errors::New(error.to_string()))?;
                    Ok(CollectConflictsStepMeta {
                        Checksum: checksum,
                        TooManyConflictsFromIndex: value
                            .get("too-many-conflicts-from-index")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        ..Default::default()
                    })
                })
                .collect::<Result<Vec<_>, errors::SharedError>>()?
        } else {
            plan_ctx.PreviousCollectMetas.clone()
        };
        // 合并所有 import 步骤的 checksum 与各 allocator 的最大 ID。
        let mut checksum = astersql_lightning_verification::NewKVGroupChecksumForAdd();
        let mut max_ids: HashMap<autoid::AllocatorType, i64> = HashMap::new();
        for meta in &import_metas {
            for (id, value) in &meta.Checksum {
                checksum.AddRawGroup(*id, value.Size, value.KVs, value.Sum);
            }
            for (kind, value) in &meta.MaxIDs {
                if *value <= 0 {
                    continue;
                }
                max_ids
                    .entry(*kind)
                    .and_modify(|current| *current = (*current).max(*value))
                    .or_insert(*value);
            }
        }
        // 汇总冲突收集阶段删除行的 checksum，以及索引冲突过多标记。
        let mut deleted = astersql_lightning_verification::NewKVChecksum();
        let mut too_many = false;
        for meta in &collect_metas {
            if let Some(value) = &meta.Checksum {
                deleted.Add(&value.ToKVChecksum());
            }
            too_many |= meta.TooManyConflictsFromIndex;
        }
        let meta = PostProcessStepMeta {
            Checksum: checksum
                .GetInnerChecksums()
                .into_iter()
                .map(|(id, value)| (id, newFromKVChecksum(&value)))
                .collect(),
            DeletedRowsChecksum: newFromKVChecksum(&deleted),
            TooManyConflictsFromIndex: too_many,
            MaxIDs: max_ids,
        };
        serde_json::to_vec(&post_process_value(&meta))
            .map_err(|error| errors::New(error.to_string()))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

fn set_spec_external_path(spec: &mut dyn Any, path: String) {
    if let Some(spec) = spec.downcast_mut::<ImportSpec>() {
        spec.ImportStepMeta.BaseExternalMeta.ExternalPath = path;
    } else if let Some(spec) = spec.downcast_mut::<MergeSortSpec>() {
        spec.MergeSortStepMeta.BaseExternalMeta.ExternalPath = path;
    } else if let Some(spec) = spec.downcast_mut::<WriteIngestSpec>() {
        spec.WriteIngestStepMeta.BaseExternalMeta.ExternalPath = path;
    } else if let Some(spec) = spec.downcast_mut::<CollectConflictsSpec>() {
        spec.CollectConflictsStepMeta.BaseExternalMeta.ExternalPath = path;
    } else if let Some(spec) = spec.downcast_mut::<ConflictResolutionSpec>() {
        spec.ConflictResolutionStepMeta
            .BaseExternalMeta
            .ExternalPath = path;
    }
}

/// 将 PostProcessStepMeta 转为与 Go JSON 字段名兼容的 Value。
fn post_process_value(meta: &PostProcessStepMeta) -> serde_json::Value {
    let max_ids: HashMap<String, i64> = meta
        .MaxIDs
        .iter()
        .map(|(kind, value)| (kind.as_str().to_owned(), *value))
        .collect();
    let mut value = serde_json::json!({
        "Checksum": meta.Checksum,
        "DeletedRowsChecksum": meta.DeletedRowsChecksum,
        "MaxIDs": max_ids,
    });
    if meta.TooManyConflictsFromIndex {
        value["too-many-conflicts-from-index"] = true.into();
    }
    value
}

/// 按 ChunkMap 为每个 engine（跳过索引引擎）生成 ImportSpec。
pub fn generateImportSpecs(
    plan_ctx: &PlanCtx,
    plan: &mut LogicalPlan,
) -> Result<Vec<Box<dyn PipelineSpec>>, errors::SharedError> {
    let chunk_map = if !plan.PreparedChunkMapExternalPath.is_empty() {
        let store = plan.sort_store(plan_ctx)?;
        let bytes = store
            .ReadFile(&plan_ctx.StorageContext, &plan.PreparedChunkMapExternalPath)
            .map_err(|error| errors::New(error.to_string()))?;
        PreparedMeta::Unmarshal(&bytes)?.ChunkMap
    } else if !plan.ChunkMap.is_empty() {
        plan.ChunkMap.clone()
    } else {
        let table_info = plan
            .Plan
            .TableInfo
            .as_ref()
            .ok_or_else(|| errors::New("import task plan has no table info"))?;
        let table: Arc<dyn table::Table> = if let Some(table) = &plan_ctx.Table {
            table.clone()
        } else {
            Arc::from(
                table::BuildTableFromMeta(table_info)
                    .map_err(|error| errors::New(error.to_string()))?
                    .ok_or_else(|| errors::New("table metadata factory is not installed"))?,
            )
        };
        let args = importer::ASTArgsFromStmt(&plan.Stmt).map_err(errors::New)?;
        let services = plan_ctx
            .ControllerServices
            .as_ref()
            .ok_or_else(|| errors::New("import controller services are unavailable"))?;
        let service = plan_ctx
            .ImporterService
            .as_ref()
            .ok_or_else(|| errors::New("table importer service is unavailable"))?;
        let mut controller =
            importer::NewLoadDataController(plan.Plan.clone(), table, args, services(), Vec::new())
                .map_err(errors::New)?;
        let result = (|| {
            controller
                .InitDataFiles(&astersql_objstore_storeapi::Context::default())
                .map_err(errors::New)?;
            controller.SetExecuteNodeCnt(plan_ctx.ExecuteNodesCnt.max(1) as usize);
            controller
                .PopulateChunks(service.as_ref())
                .map_err(errors::New)
        })();
        controller.Close();
        result?
    };
    let mut ids: Vec<i32> = chunk_map.keys().copied().collect();
    ids.sort_unstable();
    let mut specs: Vec<Box<dyn PipelineSpec>> = Vec::with_capacity(ids.len());
    for id in ids {
        if id == importer::IndexEngineID {
            continue;
        }
        let chunks = chunk_map.get(&id).cloned().unwrap_or_default();
        // 用各 chunk 的 RowIDMax 更新步骤摘要中的行数上界。
        for chunk in &chunks {
            plan.summary.RowCnt = plan.summary.RowCnt.max(chunk.RowIDMax);
        }
        plan.summary.Bytes = plan.Plan.TotalFileSize;
        specs.push(Box::new(ImportSpec {
            ImportStepMeta: ImportStepMeta {
                ID: id,
                Chunks: chunks,
                ..Default::default()
            },
            Plan: plan.Plan.clone(),
        }));
    }
    Ok(specs)
}

/// 从前序 import meta 合并出按 kv group 分组的 SortedKVMeta。
fn previous_value(
    ctx: &PlanCtx,
    plan: &LogicalPlan,
    bytes: &[u8],
) -> Result<serde_json::Value, errors::SharedError> {
    let mut value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| errors::New(error.to_string()))?;
    if let Some(path) = value
        .get("ExternalPath")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.is_empty())
    {
        let store = plan.sort_store(ctx)?;
        let external = store
            .ReadFile(&ctx.StorageContext, path)
            .map_err(|error| errors::New(error.to_string()))?;
        let external: serde_json::Value =
            serde_json::from_slice(&external).map_err(|error| errors::New(error.to_string()))?;
        if let (Some(internal), Some(external)) = (value.as_object_mut(), external.as_object()) {
            internal.extend(
                external
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone())),
            );
        }
    }
    Ok(value)
}

fn previous_import_metas(
    ctx: &PlanCtx,
    plan: &LogicalPlan,
) -> Result<Vec<ImportStepMeta>, errors::SharedError> {
    if let Some(bytes) = ctx
        .PreviousSubtaskMetas
        .get(&dxfproto::ImportStepEncodeAndSort)
    {
        bytes
            .iter()
            .map(|bytes| {
                let value = previous_value(ctx, plan, bytes)?;
                ImportStepMeta::Unmarshal(
                    &serde_json::to_vec(&value).map_err(|error| errors::New(error.to_string()))?,
                )
            })
            .collect()
    } else {
        Ok(ctx.PreviousImportMetas.clone())
    }
}

fn previous_merge_metas(
    ctx: &PlanCtx,
    plan: &LogicalPlan,
) -> Result<Vec<MergeSortStepMeta>, errors::SharedError> {
    if let Some(bytes) = ctx.PreviousSubtaskMetas.get(&dxfproto::ImportStepMergeSort) {
        bytes
            .iter()
            .map(|bytes| {
                let value = previous_value(ctx, plan, bytes)?;
                Ok(MergeSortStepMeta {
                    KVGroup: value
                        .get("kv-group")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    RecordedConflictKVCount: value
                        .get("recorded-conflict-kv-count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    SortedKVMeta: serde_json::from_value(
                        value.get("SortedKVMeta").cloned().unwrap_or(value),
                    )
                    .map_err(|error| errors::New(error.to_string()))?,
                    ..Default::default()
                })
            })
            .collect()
    } else {
        Ok(ctx.PreviousMergeMetas.clone())
    }
}

fn previous_write_metas(
    ctx: &PlanCtx,
    plan: &LogicalPlan,
) -> Result<Vec<WriteIngestStepMeta>, errors::SharedError> {
    if let Some(bytes) = ctx
        .PreviousSubtaskMetas
        .get(&dxfproto::ImportStepWriteAndIngest)
    {
        bytes
            .iter()
            .map(|bytes| {
                let value = previous_value(ctx, plan, bytes)?;
                Ok(WriteIngestStepMeta {
                    KVGroup: value
                        .get("kv-group")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    RecordedConflictKVCount: value
                        .get("recorded-conflict-kv-count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                    SortedKVMeta: serde_json::from_value(
                        value
                            .get("sorted-kv-meta")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    )
                    .map_err(|error| errors::New(error.to_string()))?,
                    ..Default::default()
                })
            })
            .collect()
    } else {
        Ok(ctx.PreviousWriteMetas.clone())
    }
}

fn encoded_kv_metas(
    plan_ctx: &PlanCtx,
    plan: &LogicalPlan,
) -> Result<HashMap<String, SortedKVMeta>, errors::SharedError> {
    let mut result = HashMap::new();
    result.insert(DATA_KV_GROUP.to_owned(), SortedKVMeta::default());
    for meta in &previous_import_metas(plan_ctx, plan)? {
        if let Some(data) = &meta.SortedDataMeta {
            result
                .entry(DATA_KV_GROUP.to_owned())
                .or_insert_with(SortedKVMeta::default)
                .Merge(data);
        }
        for (index_id, index_meta) in &meta.SortedIndexMetas {
            result
                .entry(index_id_to_kv_group(*index_id))
                .or_insert_with(SortedKVMeta::default)
                .Merge(index_meta);
        }
    }
    Ok(result)
}

/// 为需要多文件归并的 kv group 生成 MergeSortSpec；单文件且未强制合并则跳过。
pub fn generateMergeSortSpecs(
    plan_ctx: &PlanCtx,
    plan: &mut LogicalPlan,
) -> Result<Vec<Box<dyn PipelineSpec>>, errors::SharedError> {
    let _store = plan.sort_store(plan_ctx)?;
    let mut specs: Vec<Box<dyn PipelineSpec>> = Vec::new();
    for (group, meta) in encoded_kv_metas(plan_ctx, plan)? {
        if meta.MultipleFilesStats.is_empty() {
            continue;
        }
        let force_group = plan_ctx
            .ForceMergeGroup
            .as_deref()
            .is_some_and(|forced| forced == group || forced == "*");
        if !plan.Plan.ForceMergeStep
            && !force_group
            && skip_merge_sort(&meta.MultipleFilesStats, plan_ctx.ThreadCnt)
        {
            continue;
        }
        plan.summary.Bytes = plan.summary.Bytes.saturating_add(meta.TotalKVSize as i64);
        if group == DATA_KV_GROUP {
            plan.summary.RowCnt = plan.summary.RowCnt.saturating_add(meta.TotalKVCnt as i64);
        }
        // 按节点数×并发度切分文件组，生成多个 merge 子任务。
        for files in globalsort::DivideMergeSortDataFiles(
            &meta.GetDataFiles(),
            plan_ctx.ExecuteNodesCnt.max(1) as usize,
            plan_ctx.ThreadCnt.max(1) as usize,
        )
        .map_err(|error| errors::New(error.to_string()))?
        {
            specs.push(Box::new(MergeSortSpec {
                MergeSortStepMeta: MergeSortStepMeta {
                    KVGroup: group.clone(),
                    DataFiles: files,
                    ..Default::default()
                },
            }));
        }
    }
    Ok(specs)
}

fn skip_merge_sort(stats: &[MultipleFilesStat], concurrency: i32) -> bool {
    let mut points = Vec::with_capacity(stats.len() * 2);
    for stat in stats {
        points.push((&stat.MinKey, false, stat.MaxOverlappingNum));
        points.push((&stat.MaxKey, true, stat.MaxOverlappingNum));
    }
    points.sort_by(|a, b| a.0.cmp(b.0).then(a.1.cmp(&b.1)));
    let mut overlap = 0_i64;
    let mut maximum = 0_i64;
    for (_, is_end, weight) in points {
        if is_end {
            overlap -= weight;
        } else {
            overlap += weight;
            maximum = maximum.max(overlap);
        }
    }
    maximum <= simplesst::writer::GetAdjustedMergeSortOverlapThreshold(concurrency)
}

/// 合并 merge 步骤结果，并并入未强制合并的单文件 encode 组，供 ingest 使用。
fn ingest_kv_metas(
    plan_ctx: &PlanCtx,
    plan: &LogicalPlan,
    force_merge: bool,
) -> Result<HashMap<String, SortedKVMeta>, errors::SharedError> {
    let mut result: HashMap<String, SortedKVMeta> = HashMap::new();
    for meta in &previous_merge_metas(plan_ctx, plan)? {
        result
            .entry(meta.KVGroup.clone())
            .or_default()
            .Merge(&meta.SortedKVMeta);
    }
    for (group, meta) in encoded_kv_metas(plan_ctx, plan)? {
        let force_group = plan_ctx
            .ForceMergeGroup
            .as_deref()
            .is_some_and(|forced| forced == group || forced == "*");
        if !force_merge
            && !force_group
            && skip_merge_sort(&meta.MultipleFilesStats, plan_ctx.ThreadCnt)
        {
            if result.contains_key(&group) {
                return Err(errors::New(
                    "kv group of encode step conflict with merge sort step",
                ));
            }
            result.insert(group, meta);
        }
    }
    Ok(result)
}

/// 为每个 kv group 生成 WriteIngestSpec（含 range job/split keys）。
pub fn generateWriteIngestSpecs(
    plan_ctx: &PlanCtx,
    plan: &mut LogicalPlan,
) -> Result<Vec<Box<dyn PipelineSpec>>, errors::SharedError> {
    let store = plan.sort_store(plan_ctx)?;
    let ts = if let Some(ts) = plan_ctx.CommitTS {
        ts
    } else {
        plan_ctx
            .KVStore
            .as_ref()
            .ok_or_else(|| errors::New("KV store is required for write-ingest planning"))?
            .CurrentVersion(kv::GlobalTxnScope)?
            .Ver
    };
    let mut specs: Vec<Box<dyn PipelineSpec>> = Vec::new();
    for (group, meta) in ingest_kv_metas(plan_ctx, plan, plan.Plan.ForceMergeStep)? {
        if meta.MultipleFilesStats.is_empty() {
            continue;
        }
        plan.summary.Bytes = plan.summary.Bytes.saturating_add(meta.TotalKVSize as i64);
        if group == DATA_KV_GROUP {
            plan.summary.RowCnt = plan.summary.RowCnt.saturating_add(meta.TotalKVCnt as i64);
        }
        specs.extend(split_for_one_subtask(
            plan_ctx,
            store.store.clone(),
            group,
            meta,
            ts,
        )?);
    }
    Ok(specs)
}

struct GlobalSortStoreAdapter {
    ctx: objstore::storage::Context,
    store: objstore::storage::StorageRef,
}

impl globalsort::Storage for GlobalSortStoreAdapter {
    fn read(&self, path: &str) -> globalsort::Result<Vec<u8>> {
        self.store
            .ReadFile(&self.ctx, path)
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))
    }
    fn write(&self, path: &str, data: Vec<u8>) -> globalsort::Result<()> {
        self.store
            .WriteFile(&self.ctx, path, &data)
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))
    }
    fn delete_files(&self, paths: &[String]) -> globalsort::Result<()> {
        self.store
            .DeleteFiles(&self.ctx, paths)
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))
    }
    fn list_prefix(&self, prefix: &str) -> globalsort::Result<Vec<String>> {
        let mut paths = Vec::new();
        self.store
            .WalkDir(&self.ctx, None, &mut |path, _| {
                if path.starts_with(prefix) {
                    paths.push(path.to_owned());
                }
                Ok(())
            })
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))?;
        Ok(paths)
    }
}

pub(crate) fn split_for_one_subtask(
    plan_ctx: &PlanCtx,
    store: objstore::storage::StorageRef,
    group: String,
    meta: SortedKVMeta,
    ts: u64,
) -> Result<Vec<Box<dyn PipelineSpec>>, errors::SharedError> {
    let adapter = GlobalSortStoreAdapter {
        ctx: plan_ctx.StorageContext.clone(),
        store,
    };
    let files: Vec<globalsort::MultipleFilesStat> = meta
        .MultipleFilesStats
        .iter()
        .map(|stat| {
            let filenames = stat
                .Filenames
                .iter()
                .map(|pair| {
                    let encoded = adapter
                        .read(&pair[1])
                        .map_err(|error| errors::New(error.to_string()))?;
                    let props = simplesst::codec::decode_multi_props(&encoded)
                        .map_err(|error| errors::New(error.to_string()))?;
                    Ok(globalsort::FilePair {
                        data_file: pair[0].clone(),
                        stat_file: pair[1].clone(),
                        properties: props
                            .into_iter()
                            .map(|prop| globalsort::RangeProperty {
                                first_key: prop.FirstKey,
                                last_key: prop.LastKey,
                                size: prop.Size,
                                keys: prop.Keys,
                            })
                            .collect(),
                    })
                })
                .collect::<Result<Vec<_>, errors::SharedError>>()?;
            Ok(globalsort::MultipleFilesStat { filenames })
        })
        .collect::<Result<Vec<_>, errors::SharedError>>()?;
    let (default_region_size, default_region_keys) = if kerneltype::IsNextGen() {
        (1024 * 1024 * 1024, 102_400_000)
    } else {
        (96 * 1024 * 1024, 960_000)
    };
    let (configured_size, configured_keys) = plan_ctx
        .ImporterService
        .as_ref()
        .and_then(|service| service.RegionSplitSizeKeys().ok())
        .unwrap_or((plan_ctx.RegionSplitSize, plan_ctx.RegionSplitKeys));
    let region_size = configured_size.max(default_region_size);
    let region_keys = configured_keys.max(default_region_keys);
    let node_mem_per_core = if plan_ctx.NodeMemPerCore > 0 {
        plan_ctx.NodeMemPerCore
    } else {
        let resource = dxfstorage::GetNodeResource()
            .ok_or_else(|| errors::New("node resource is unavailable"))?;
        if resource.TotalCPU <= 0 {
            return Err(errors::New("node CPU capacity must be positive"));
        }
        resource.TotalMem / i64::from(resource.TotalCPU)
    };
    let (range_size, range_keys) =
        globalsort::split::CalRangeSize(node_mem_per_core, region_size, region_keys);
    let mut splitter = globalsort::split::NewRangeSplitter(
        &files,
        &adapter,
        100 * 1024 * 1024 * 1024,
        i64::MAX,
        range_size,
        range_keys,
        region_size,
        region_keys,
    )
    .map_err(|error| errors::New(error.to_string()))?;
    let result = (|| -> Result<Vec<Box<dyn PipelineSpec>>, errors::SharedError> {
        let mut start = meta.StartKey.clone();
        let mut specs: Vec<Box<dyn PipelineSpec>> = Vec::new();
        loop {
            let result = splitter
                .SplitOneRangesGroup()
                .map_err(|error| errors::New(error.to_string()))?;
            let final_group = result.end_key_of_group.is_empty();
            let end = if final_group {
                meta.EndKey.clone()
            } else {
                result.end_key_of_group
            };
            if start >= end {
                return Err(errors::New(format!(
                    "invalid kv range, startKey: {}, endKey: {}",
                    globalsort::hex(&start),
                    globalsort::hex(&end)
                )));
            }
            let mut job_keys = vec![start.clone()];
            job_keys.extend(result.interior_range_job_keys);
            job_keys.push(end.clone());
            let mut split_keys = vec![start.clone()];
            split_keys.extend(result.interior_region_split_keys);
            split_keys.push(end.clone());
            specs.push(Box::new(WriteIngestSpec {
                WriteIngestStepMeta: WriteIngestStepMeta {
                    KVGroup: group.clone(),
                    SortedKVMeta: SortedKVMeta {
                        StartKey: start.clone(),
                        EndKey: end.clone(),
                        TotalKVSize: 100 * 1024 * 1024 * 1024,
                        ..Default::default()
                    },
                    DataFiles: result.data_files,
                    StatFiles: result.stat_files,
                    RangeJobKeys: job_keys,
                    RangeSplitKeys: split_keys,
                    TS: ts,
                    ..Default::default()
                },
            }));
            start = end;
            if final_group {
                break;
            }
        }
        Ok(specs)
    })();
    let _ = splitter.Close();
    result
}

/// 从前序 import/merge/write meta 汇总各 kv group 的冲突信息。
pub fn collectConflictInfos(
    plan_ctx: &PlanCtx,
    plan: &LogicalPlan,
) -> Result<KVGroupConflictInfos, errors::SharedError> {
    let mut result = KVGroupConflictInfos::default();
    let import_metas = if let Some(bytes) = plan_ctx
        .PreviousSubtaskMetas
        .get(&dxfproto::ImportStepEncodeAndSort)
    {
        bytes
            .iter()
            .map(|bytes| {
                let envelope = ImportStepMeta::Unmarshal(bytes)?;
                if envelope.RecordedConflictKVCount == 0 {
                    return Ok(None);
                }
                let full = previous_value(plan_ctx, plan, bytes)?;
                ImportStepMeta::Unmarshal(
                    &serde_json::to_vec(&full).map_err(|error| errors::New(error.to_string()))?,
                )
                .map(Some)
            })
            .collect::<Result<Vec<_>, errors::SharedError>>()?
            .into_iter()
            .flatten()
            .collect()
    } else {
        plan_ctx.PreviousImportMetas.clone()
    };
    for meta in &import_metas {
        if meta.RecordedConflictKVCount == 0 {
            continue;
        }
        if let Some(data) = &meta.SortedDataMeta {
            result.addDataConflictInfo(&data.ConflictInfo);
        }
        for (index_id, index) in &meta.SortedIndexMetas {
            result.addIndexConflictInfo(*index_id, &index.ConflictInfo);
        }
    }
    let merge_metas = if let Some(bytes) = plan_ctx
        .PreviousSubtaskMetas
        .get(&dxfproto::ImportStepMergeSort)
    {
        bytes
            .iter()
            .map(|bytes| {
                let envelope: serde_json::Value = serde_json::from_slice(bytes)
                    .map_err(|error| errors::New(error.to_string()))?;
                if envelope
                    .get("recorded-conflict-kv-count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
                    == 0
                {
                    return Ok(None);
                }
                let mut one = plan_ctx.clone();
                one.PreviousSubtaskMetas
                    .insert(dxfproto::ImportStepMergeSort, vec![bytes.clone()]);
                previous_merge_metas(&one, plan).map(|mut metas| metas.pop())
            })
            .collect::<Result<Vec<_>, errors::SharedError>>()?
            .into_iter()
            .flatten()
            .collect()
    } else {
        plan_ctx.PreviousMergeMetas.clone()
    };
    for meta in &merge_metas {
        if meta.RecordedConflictKVCount > 0 {
            result.addConflictInfo(&meta.KVGroup, &meta.SortedKVMeta.ConflictInfo);
        }
    }
    let write_metas = if let Some(bytes) = plan_ctx
        .PreviousSubtaskMetas
        .get(&dxfproto::ImportStepWriteAndIngest)
    {
        bytes
            .iter()
            .map(|bytes| {
                let envelope: serde_json::Value = serde_json::from_slice(bytes)
                    .map_err(|error| errors::New(error.to_string()))?;
                if envelope
                    .get("recorded-conflict-kv-count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0)
                    == 0
                {
                    return Ok(None);
                }
                let mut one = plan_ctx.clone();
                one.PreviousSubtaskMetas
                    .insert(dxfproto::ImportStepWriteAndIngest, vec![bytes.clone()]);
                previous_write_metas(&one, plan).map(|mut metas| metas.pop())
            })
            .collect::<Result<Vec<_>, errors::SharedError>>()?
            .into_iter()
            .flatten()
            .collect()
    } else {
        plan_ctx.PreviousWriteMetas.clone()
    };
    for meta in &write_metas {
        if meta.RecordedConflictKVCount > 0 {
            result.addConflictInfo(&meta.KVGroup, &meta.SortedKVMeta.ConflictInfo);
        }
    }
    Ok(result)
}

/// 有冲突时生成单个 CollectConflictsSpec，并更新摘要行数。
pub fn generateCollectConflictsSpecs(
    plan_ctx: &PlanCtx,
    plan: &mut LogicalPlan,
) -> Result<Vec<Box<dyn PipelineSpec>>, errors::SharedError> {
    let _store = plan.sort_store(plan_ctx)?;
    let infos = collectConflictInfos(plan_ctx, plan)?;
    plan.summary.RowCnt = totalConflicts(&infos);
    if infos.ConflictInfos.is_empty() {
        return Ok(Vec::new());
    }
    let recorded_data = infos
        .ConflictInfos
        .get(DATA_KV_GROUP)
        .map(|info| i64::try_from(info.Count).unwrap_or(i64::MAX))
        .unwrap_or(0);
    Ok(vec![Box::new(CollectConflictsSpec {
        CollectConflictsStepMeta: CollectConflictsStepMeta {
            Infos: infos,
            RecordedDataKVConflicts: recorded_data,
            ..Default::default()
        },
    })])
}

/// 有冲突时生成单个 ConflictResolutionSpec。
pub fn generateConflictResolutionSpecs(
    plan_ctx: &PlanCtx,
    plan: &mut LogicalPlan,
) -> Result<Vec<Box<dyn PipelineSpec>>, errors::SharedError> {
    let _store = plan.sort_store(plan_ctx)?;
    let infos = collectConflictInfos(plan_ctx, plan)?;
    plan.summary.RowCnt = totalConflicts(&infos);
    if infos.ConflictInfos.is_empty() {
        return Ok(Vec::new());
    }
    Ok(vec![Box::new(ConflictResolutionSpec {
        ConflictResolutionStepMeta: ConflictResolutionStepMeta {
            Infos: infos,
            ..Default::default()
        },
    })])
}

/// 对各 kv group 的冲突计数求和（饱和加，溢出钳制到 i64::MAX）。
pub fn totalConflicts(infos: &KVGroupConflictInfos) -> i64 {
    infos.ConflictInfos.values().fold(0_i64, |total, info| {
        total.saturating_add(i64::try_from(info.Count).unwrap_or(i64::MAX))
    })
}

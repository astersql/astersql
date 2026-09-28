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

// IMPORT INTO 任务与各步骤元数据协议定义及 JSON 序列化。
//
// 定义 TaskMeta、各 StepMeta、SortedKVMeta、冲突信息与 SharedVars 等结构，
// 保证与 Go 侧 wire 字段名兼容，供调度器与执行器在步骤间传递状态。

use std::collections::HashMap;
use std::sync::atomic::AtomicI64;
use std::sync::{Arc, Mutex};

use astersql_dxf_framework_proto as framework_proto;
use astersql_errors as errors;
use astersql_executor_importer as importer;
use astersql_ingestor_engineapi as engineapi;
use astersql_lightning_backend as backend;
use astersql_lightning_log::Logger;
use astersql_lightning_mydump as mydump;
use astersql_lightning_verification as verification;
use astersql_meta_autoid as autoid;
use astersql_meta_model as model;
use astersql_objstore_storeapi as storeapi;
use astersql_parser_ast as ast;

/// Server identity persisted in task metadata. It contains the fields used by
/// the distributed-task executor and remains wire-compatible with Go's
/// `serverinfo.ServerInfo` representation.
/// 持久化在任务元数据中的服务器身份，字段与 Go `serverinfo.ServerInfo` 线兼容。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ServerInfo {
    #[serde(default, rename = "id", alias = "ID")]
    pub id: String,
    #[serde(default, rename = "ip", alias = "IP")]
    pub ip: String,
    #[serde(default, rename = "listening_port", alias = "ListeningPort")]
    pub listening_port: u32,
}

/// A pair of external data/stat files emitted by one SST writer.
/// 单个 SST writer 产出的外部 data/stat 文件对列表。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct MultipleFilesStat {
    #[serde(
        serialize_with = "serialize_go_bytes",
        deserialize_with = "deserialize_go_bytes"
    )]
    pub MinKey: Vec<u8>,
    #[serde(
        serialize_with = "serialize_go_bytes",
        deserialize_with = "deserialize_go_bytes"
    )]
    pub MaxKey: Vec<u8>,
    pub Filenames: Vec<[String; 2]>,
    pub MaxOverlappingNum: i64,
}

/// Writer result consumed by the import-step aggregators.
/// import 步骤聚合器用的 writer 结果摘要（键范围、大小、冲突等）。
#[derive(Clone, Debug, Default)]
pub struct WriterSummary {
    pub Min: Option<Vec<u8>>,
    pub Max: Option<Vec<u8>>,
    pub TotalSize: u64,
    pub TotalCnt: u64,
    pub MultipleFilesStats: Vec<MultipleFilesStat>,
    pub ConflictInfo: engineapi::ConflictInfo,
}

/// External-sort metadata kept local until the globalsort crate exposes its
/// already migrated production modules.
/// 外部排序元数据（键范围、KV 规模、多文件统计与冲突）；globalsort crate 就绪前本地保留。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SortedKVMeta {
    #[serde(
        rename = "start-key",
        alias = "StartKey",
        serialize_with = "serialize_go_bytes",
        deserialize_with = "deserialize_go_bytes"
    )]
    pub StartKey: Vec<u8>,
    #[serde(
        rename = "end-key",
        alias = "EndKey",
        serialize_with = "serialize_go_bytes",
        deserialize_with = "deserialize_go_bytes"
    )]
    pub EndKey: Vec<u8>,
    #[serde(rename = "total-kv-size", alias = "TotalKVSize")]
    pub TotalKVSize: u64,
    #[serde(rename = "total-kv-cnt", alias = "TotalKVCnt")]
    pub TotalKVCnt: u64,
    #[serde(rename = "multiple-files-stats", alias = "MultipleFilesStats")]
    pub MultipleFilesStats: Vec<MultipleFilesStat>,
    #[serde(rename = "conflict-info", with = "go_conflict_info")]
    pub ConflictInfo: engineapi::ConflictInfo,
}

fn serialize_go_bytes<S: serde::Serializer>(
    value: &Vec<u8>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use base64::Engine;
    if value.is_empty() {
        serializer.serialize_none()
    } else {
        serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(value))
    }
}

fn deserialize_go_bytes<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<u8>, D::Error> {
    use base64::Engine;
    use serde::Deserialize;
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .map(|value| {
            base64::engine::general_purpose::STANDARD
                .decode(value)
                .map_err(serde::de::Error::custom)
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}

fn go_byte_slices_value(values: &[Vec<u8>]) -> serde_json::Value {
    use base64::Engine;
    serde_json::Value::Array(
        values
            .iter()
            .map(|value| {
                if value.is_empty() {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(
                        base64::engine::general_purpose::STANDARD.encode(value),
                    )
                }
            })
            .collect(),
    )
}

mod go_conflict_info {
    use astersql_ingestor_engineapi::ConflictInfo;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    #[derive(Serialize, Deserialize, Default)]
    struct Wire {
        #[serde(default, alias = "count")]
        Count: u64,
        #[serde(default, alias = "files")]
        Files: Vec<String>,
    }
    pub fn serialize<S: Serializer>(
        value: &ConflictInfo,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        Wire {
            Count: value.Count,
            Files: value.Files.clone(),
        }
        .serialize(serializer)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ConflictInfo, D::Error> {
        let wire = Wire::deserialize(deserializer)?;
        Ok(ConflictInfo {
            Count: wire.Count,
            Files: wire.Files,
        })
    }
}

impl SortedKVMeta {
    /// 合并另一份 SortedKVMeta：扩展键范围、累加计数并合并冲突信息。
    pub fn Merge(&mut self, other: &Self) {
        if other.StartKey.is_empty() && other.EndKey.is_empty() {
            return;
        }
        if self.StartKey.is_empty() && self.EndKey.is_empty() {
            *self = other.clone();
            return;
        }
        if other.StartKey < self.StartKey {
            self.StartKey.clone_from(&other.StartKey);
        }
        if other.EndKey > self.EndKey {
            self.EndKey.clone_from(&other.EndKey);
        }
        self.TotalKVSize = self.TotalKVSize.wrapping_add(other.TotalKVSize);
        self.TotalKVCnt = self.TotalKVCnt.wrapping_add(other.TotalKVCnt);
        self.MultipleFilesStats
            .extend(other.MultipleFilesStats.iter().cloned());
        self.ConflictInfo.Merge(&other.ConflictInfo);
    }

    /// 将 WriterSummary 转为 SortedKVMeta 后合并。
    pub fn MergeSummary(&mut self, summary: &WriterSummary) {
        self.Merge(&new_sorted_kv_meta(summary));
    }

    /// 取出所有 data 文件路径（Filenames 对的第一项）。
    pub fn GetDataFiles(&self) -> Vec<String> {
        self.MultipleFilesStats
            .iter()
            .flat_map(|stat| stat.Filenames.iter().map(|pair| pair[0].clone()))
            .collect()
    }

    /// 取出所有 stat 文件路径（Filenames 对的第二项）。
    pub fn GetStatFiles(&self) -> Vec<String> {
        self.MultipleFilesStats
            .iter()
            .flat_map(|stat| stat.Filenames.iter().map(|pair| pair[1].clone()))
            .collect()
    }
}

/// 由 WriterSummary 构造 SortedKVMeta；EndKey 在 Max 非空时追加 0 字节作为开区间上界。
pub fn new_sorted_kv_meta(summary: &WriterSummary) -> SortedKVMeta {
    if summary.Min.as_ref().is_none_or(Vec::is_empty)
        && summary.Max.as_ref().is_none_or(Vec::is_empty)
    {
        return SortedKVMeta::default();
    }
    let start_key = summary.Min.clone().unwrap_or_default();
    let mut end_key = summary.Max.clone().unwrap_or_default();
    if !end_key.is_empty() {
        end_key.push(0);
    }
    SortedKVMeta {
        StartKey: start_key,
        EndKey: end_key,
        TotalKVSize: summary.TotalSize,
        TotalKVCnt: summary.TotalCnt,
        MultipleFilesStats: summary.MultipleFilesStats.clone(),
        ConflictInfo: summary.ConflictInfo.clone(),
    }
}

/// 数据 KV 组的固定组名。
pub const DATA_KV_GROUP: &str = "data";
/// 将索引 ID 转为 kv group 名（十进制字符串）。
pub fn index_id_to_kv_group(index_id: i64) -> String {
    index_id.to_string()
}

/// 外部元数据基类：仅含 ExternalPath，非空时表示大对象已外置。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct BaseExternalMeta {
    #[serde(default, rename = "ExternalPath")]
    pub ExternalPath: String,
}

impl BaseExternalMeta {
    /// 将 JSON Value 序列化为字节。
    fn marshal_value(&self, value: &serde_json::Value) -> Result<Vec<u8>, errors::SharedError> {
        serde_json::to_vec(value).map_err(|error| errors::New(error.to_string()))
    }
}

/// Chunk 的 JSON 视图（与 Go importer.Chunk 字段对应）。
fn chunk_value(chunk: &importer::Chunk) -> serde_json::Value {
    serde_json::json!({
        "Path": chunk.Path,
        "FileSize": chunk.FileSize,
        "Offset": chunk.Offset,
        "EndOffset": chunk.EndOffset,
        "PrevRowIDMax": chunk.PrevRowIDMax,
        "RowIDMax": chunk.RowIDMax,
        "Type": chunk.Type as i32,
        "Compression": chunk.Compression as i32,
        "Timestamp": chunk.Timestamp,
    })
}

/// Chunk 列表的 JSON 数组。
fn chunks_value(chunks: &[importer::Chunk]) -> serde_json::Value {
    serde_json::Value::Array(chunks.iter().map(chunk_value).collect())
}

/// engine_id → chunks 映射的 JSON 对象（键为字符串化的 engine id）。
fn chunk_map_value(chunks: &HashMap<i32, Vec<importer::Chunk>>) -> serde_json::Value {
    serde_json::Value::Object(
        chunks
            .iter()
            .map(|(engine_id, chunks)| (engine_id.to_string(), chunks_value(chunks)))
            .collect(),
    )
}

/// 冲突信息 map 的 JSON 对象。
fn conflict_infos_value(infos: &KVGroupConflictInfos) -> serde_json::Value {
    serde_json::Value::Object(
        infos
            .ConflictInfos
            .iter()
            .map(|(group, info)| {
                (
                    group.clone(),
                    serde_json::json!({"Count": info.Count, "Files": info.Files}),
                )
            })
            .collect(),
    )
}

// TaskMeta 对应 Go 的 IMPORT INTO 顶层任务元数据；所有字段在 Go 中都要求可序列化。
#[derive(Clone, Default)]
pub struct TaskMeta {
    // IMPORT INTO job id，对应 mysql.tidb_import_jobs。
    pub JobID: i64,
    pub Plan: importer::Plan,
    pub Stmt: String,
    // Summary 汇总整个导入任务的进度和结果。
    pub Summary: importer::Summary,
    // EligibleInstances 为空时表示所有实例都可执行；非分布式导入时用于绑定发起实例。
    pub EligibleInstances: Vec<ServerInfo>,
    // ChunkMap 以 engine ID 为键保存待导入文件块，CSV split_file 会预先拆到不同 engine。
    pub ChunkMap: std::collections::HashMap<i32, Vec<importer::Chunk>>,
    // PreparedMetaExternalPath 指向 prepare 阶段写入外部存储的 chunk 元数据。
    pub PreparedMetaExternalPath: String,
}

/// Chunk 线格式中间结构（snake_case 字段，供 serde 使用）。
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct ChunkWire {
    #[serde(alias = "Path")]
    path: String,
    #[serde(alias = "FileSize")]
    file_size: i64,
    #[serde(alias = "Offset")]
    offset: i64,
    #[serde(alias = "EndOffset")]
    end_offset: i64,
    #[serde(alias = "PrevRowIDMax")]
    prev_row_id_max: i64,
    #[serde(alias = "RowIDMax")]
    row_id_max: i64,
    #[serde(alias = "Type")]
    source_type: i32,
    #[serde(alias = "Compression")]
    compression: i32,
    #[serde(alias = "Timestamp")]
    timestamp: i64,
}

impl From<&importer::Chunk> for ChunkWire {
    fn from(chunk: &importer::Chunk) -> Self {
        Self {
            path: chunk.Path.clone(),
            file_size: chunk.FileSize,
            offset: chunk.Offset,
            end_offset: chunk.EndOffset,
            prev_row_id_max: chunk.PrevRowIDMax,
            row_id_max: chunk.RowIDMax,
            source_type: chunk.Type as i32,
            compression: chunk.Compression as i32,
            timestamp: chunk.Timestamp,
        }
    }
}

impl TryFrom<ChunkWire> for importer::Chunk {
    type Error = errors::SharedError;

    fn try_from(chunk: ChunkWire) -> Result<Self, Self::Error> {
        let source_type = match chunk.source_type {
            0 => mydump::SourceType::Ignore,
            1 => mydump::SourceType::SchemaSchema,
            2 => mydump::SourceType::TableSchema,
            3 => mydump::SourceType::Sql,
            4 => mydump::SourceType::Csv,
            5 => mydump::SourceType::Parquet,
            6 => mydump::SourceType::ViewSchema,
            value => return Err(errors::New(format!("invalid source type {value}"))),
        };
        let compression = match chunk.compression {
            0 => mydump::Compression::None,
            1 => mydump::Compression::Gz,
            2 => mydump::Compression::Lz4,
            3 => mydump::Compression::Zstd,
            4 => mydump::Compression::Xz,
            5 => mydump::Compression::Lzo,
            6 => mydump::Compression::Snappy,
            value => return Err(errors::New(format!("invalid compression {value}"))),
        };
        Ok(importer::Chunk {
            Path: chunk.path,
            FileSize: chunk.file_size,
            Offset: chunk.offset,
            EndOffset: chunk.end_offset,
            PrevRowIDMax: chunk.prev_row_id_max,
            RowIDMax: chunk.row_id_max,
            Type: source_type,
            Compression: compression,
            Timestamp: chunk.timestamp,
        })
    }
}

/// 步骤摘要线格式（字节数与行数）。
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct StepSummaryWire {
    #[serde(alias = "input-bytes")]
    bytes: i64,
    #[serde(alias = "input-rows")]
    rows: i64,
}

impl From<&importer::StepSummary> for StepSummaryWire {
    fn from(summary: &importer::StepSummary) -> Self {
        Self {
            bytes: summary.Bytes,
            rows: summary.RowCnt,
        }
    }
}

impl From<StepSummaryWire> for importer::StepSummary {
    fn from(summary: StepSummaryWire) -> Self {
        Self {
            Bytes: summary.bytes,
            RowCnt: summary.rows,
        }
    }
}

/// 整任务 Summary 的线格式，覆盖 encode/merge/ingest/冲突各阶段摘要。
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct SummaryWire {
    #[serde(alias = "encode-summary")]
    encode: StepSummaryWire,
    #[serde(alias = "merge-summary")]
    merge: StepSummaryWire,
    #[serde(alias = "ingest-summary")]
    ingest: StepSummaryWire,
    #[serde(alias = "collect-conflicts-summary")]
    collect_conflicts: StepSummaryWire,
    #[serde(alias = "resolve-conflicts-summary")]
    resolve_conflicts: StepSummaryWire,
    #[serde(alias = "row-count")]
    imported_rows: i64,
    #[serde(alias = "conflict-row-count")]
    conflict_rows: u64,
    #[serde(alias = "too-many-conflicts")]
    too_many_conflicts: bool,
}

impl From<&importer::Summary> for SummaryWire {
    fn from(summary: &importer::Summary) -> Self {
        Self {
            encode: (&summary.EncodeSummary).into(),
            merge: (&summary.MergeSummary).into(),
            ingest: (&summary.IngestSummary).into(),
            collect_conflicts: (&summary.CollectConflictsSummary).into(),
            resolve_conflicts: (&summary.ResolveConflictsSummary).into(),
            imported_rows: summary.ImportedRows,
            conflict_rows: summary.ConflictRowCnt,
            too_many_conflicts: summary.TooManyConflicts,
        }
    }
}

impl From<SummaryWire> for importer::Summary {
    fn from(summary: SummaryWire) -> Self {
        Self {
            EncodeSummary: summary.encode.into(),
            MergeSummary: summary.merge.into(),
            IngestSummary: summary.ingest.into(),
            CollectConflictsSummary: summary.collect_conflicts.into(),
            ResolveConflictsSummary: summary.resolve_conflicts.into(),
            ImportedRows: summary.imported_rows,
            ConflictRowCnt: summary.conflict_rows,
            TooManyConflicts: summary.too_many_conflicts,
        }
    }
}

/// importer::Plan 的可序列化子集，用于 TaskMeta wire。
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct PlanWire {
    #[serde(alias = "DBName")]
    db_name: String,
    #[serde(alias = "DBID")]
    db_id: i64,
    #[serde(default, alias = "TableInfo")]
    table_info: Option<model::TableInfo>,
    #[serde(default, alias = "DesiredTableInfo")]
    desired_table_info: Option<model::TableInfo>,
    table_id: Option<i64>,
    table_name: Option<String>,
    #[serde(alias = "Path")]
    path: String,
    #[serde(alias = "Format")]
    format: String,
    #[serde(alias = "Restrictive")]
    restrictive: bool,
    #[serde(alias = "LocationID")]
    location_id: String,
    #[serde(alias = "SQLMode")]
    sql_mode: i64,
    #[serde(alias = "ImportantSysVars")]
    important_sys_vars: HashMap<String, String>,
    #[serde(alias = "FieldNullDef")]
    field_null_def: Vec<String>,
    #[serde(alias = "NullValueOptEnclosed")]
    null_value_opt_enclosed: bool,
    #[serde(alias = "DiskQuota")]
    disk_quota: i64,
    #[serde(alias = "Checksum")]
    checksum: String,
    #[serde(alias = "MaxWriteSpeed")]
    max_write_speed: i64,
    #[serde(alias = "MaxEngineSize")]
    max_engine_size: i64,
    #[serde(alias = "OnDupKey")]
    on_dup_key: String,
    #[serde(alias = "InImportInto")]
    in_import_into: bool,
    #[serde(alias = "DataSourceType")]
    data_source_type: String,
    #[serde(alias = "IsRaftKV2")]
    is_raft_kv2: bool,
    #[serde(alias = "SpecifiedOptionNames")]
    specified_option_names: HashMap<String, serde_json::Value>,
    #[serde(alias = "FieldsTerminatedBy")]
    fields_terminated_by: String,
    #[serde(alias = "FieldsEnclosedBy")]
    fields_enclosed_by: String,
    #[serde(alias = "FieldsEscapedBy")]
    fields_escaped_by: String,
    #[serde(alias = "FieldsOptEnclosed")]
    fields_opt_enclosed: bool,
    #[serde(alias = "LinesStartingBy")]
    lines_starting_by: String,
    #[serde(alias = "LinesTerminatedBy")]
    lines_terminated_by: String,
    #[serde(alias = "Charset")]
    charset: Option<String>,
    #[serde(alias = "IgnoreLines")]
    ignore_lines: u64,
    #[serde(alias = "ThreadCnt")]
    thread_count: usize,
    #[serde(alias = "MaxNodeCnt")]
    max_node_count: i32,
    #[serde(alias = "SplitFile")]
    split_file: bool,
    #[serde(alias = "MaxRecordedErrors")]
    max_recorded_errors: i64,
    #[serde(alias = "Detached")]
    detached: bool,
    #[serde(alias = "DisableTiKVImportMode")]
    disable_tikv_import_mode: bool,
    #[serde(alias = "CloudStorageURI")]
    cloud_storage_uri: String,
    #[serde(alias = "DisablePrecheck")]
    disable_precheck: bool,
    #[serde(alias = "GroupKey")]
    group_key: String,
    #[serde(alias = "DistSQLScanConcurrency")]
    distsql_scan_concurrency: usize,
    #[serde(alias = "User")]
    user: String,
    #[serde(alias = "TotalFileSize")]
    total_file_size: i64,
    #[serde(alias = "ForceMergeStep")]
    force_merge_step: bool,
    #[serde(alias = "ManualRecovery")]
    manual_recovery: bool,
    #[serde(alias = "Keyspace")]
    keyspace: String,
    #[serde(alias = "UseNewCollate")]
    use_new_collate: Option<bool>,
}

impl From<&importer::Plan> for PlanWire {
    fn from(plan: &importer::Plan) -> Self {
        Self {
            db_name: plan.DBName.clone(),
            db_id: plan.DBID,
            table_info: plan.TableInfo.as_deref().cloned(),
            desired_table_info: plan.DesiredTableInfo.as_deref().cloned(),
            table_id: plan.TableInfo.as_ref().map(|table| table.ID),
            table_name: plan.TableInfo.as_ref().map(|table| table.Name.O.clone()),
            path: plan.Path.clone(),
            format: plan.Format.clone(),
            restrictive: plan.Restrictive,
            location_id: plan.LocationID.clone(),
            sql_mode: plan.SQLMode.0,
            important_sys_vars: plan.ImportantSysVars.clone(),
            field_null_def: plan.FieldNullDef.clone(),
            null_value_opt_enclosed: plan.NullValueOptEnclosed,
            disk_quota: plan.DiskQuota.0,
            checksum: match plan.Checksum {
                importer::PostOpLevel::Off => "off",
                importer::PostOpLevel::Optional => "optional",
                importer::PostOpLevel::Required => "required",
            }
            .to_owned(),
            max_write_speed: plan.MaxWriteSpeed.0,
            max_engine_size: plan.MaxEngineSize.0,
            on_dup_key: if plan.OnDupKey == importer::OnDupKeyModeCapture {
                "capture"
            } else {
                "error"
            }
            .to_owned(),
            in_import_into: plan.InImportInto,
            data_source_type: if plan.DataSourceType == importer::DataSourceTypeQuery {
                "query"
            } else {
                "file"
            }
            .to_owned(),
            is_raft_kv2: plan.IsRaftKV2,
            specified_option_names: plan
                .SpecifiedOptionNames
                .iter()
                .map(|name| (name.clone(), serde_json::Value::Null))
                .collect(),
            fields_terminated_by: plan.LineFieldsInfo.FieldsTerminatedBy.clone(),
            fields_enclosed_by: plan.LineFieldsInfo.FieldsEnclosedBy.clone(),
            fields_escaped_by: plan.LineFieldsInfo.FieldsEscapedBy.clone(),
            fields_opt_enclosed: plan.LineFieldsInfo.FieldsOptEnclosed,
            lines_starting_by: plan.LineFieldsInfo.LinesStartingBy.clone(),
            lines_terminated_by: plan.LineFieldsInfo.LinesTerminatedBy.clone(),
            charset: plan.Charset.clone(),
            ignore_lines: plan.IgnoreLines,
            thread_count: plan.ThreadCnt,
            max_node_count: plan.MaxNodeCnt,
            split_file: plan.SplitFile,
            max_recorded_errors: plan.MaxRecordedErrors,
            detached: plan.Detached,
            disable_tikv_import_mode: plan.DisableTiKVImportMode,
            cloud_storage_uri: plan.CloudStorageURI.clone(),
            disable_precheck: plan.DisablePrecheck,
            group_key: plan.GroupKey.clone(),
            distsql_scan_concurrency: plan.DistSQLScanConcurrency,
            user: plan.User.clone(),
            total_file_size: plan.TotalFileSize,
            force_merge_step: plan.ForceMergeStep,
            manual_recovery: plan.ManualRecovery,
            keyspace: plan.Keyspace.clone(),
            use_new_collate: plan.UseNewCollate,
        }
    }
}

impl From<PlanWire> for importer::Plan {
    fn from(plan: PlanWire) -> Self {
        let table_info = plan.table_info.or_else(|| {
            plan.table_id.map(|id| model::TableInfo {
                ID: id,
                Name: ast::NewCIStr(plan.table_name.as_deref().unwrap_or_default()),
                ..Default::default()
            })
        });
        importer::Plan {
            DBName: plan.db_name,
            DBID: plan.db_id,
            TableInfo: table_info.map(Arc::new),
            DesiredTableInfo: plan.desired_table_info.map(Arc::new),
            Path: plan.path,
            Format: plan.format,
            Restrictive: plan.restrictive,
            LocationID: plan.location_id,
            SQLMode: astersql_parser_mysql::r#const::SQLMode(plan.sql_mode),
            ImportantSysVars: plan.important_sys_vars,
            FieldNullDef: plan.field_null_def,
            NullValueOptEnclosed: plan.null_value_opt_enclosed,
            DiskQuota: importer::ByteSize(plan.disk_quota),
            Checksum: match plan.checksum.as_str() {
                "off" => importer::PostOpLevel::Off,
                "optional" => importer::PostOpLevel::Optional,
                _ => importer::PostOpLevel::Required,
            },
            MaxWriteSpeed: importer::ByteSize(plan.max_write_speed),
            MaxEngineSize: importer::ByteSize(plan.max_engine_size),
            OnDupKey: if plan.on_dup_key == "capture" {
                importer::OnDupKeyModeCapture
            } else {
                importer::OnDupKeyModeError
            },
            InImportInto: plan.in_import_into,
            DataSourceType: if plan.data_source_type == "query" {
                importer::DataSourceTypeQuery
            } else {
                importer::DataSourceTypeFile
            },
            IsRaftKV2: plan.is_raft_kv2,
            SpecifiedOptionNames: plan.specified_option_names.into_keys().collect(),
            LineFieldsInfo: importer::LineFieldsInfo {
                FieldsTerminatedBy: plan.fields_terminated_by,
                FieldsEnclosedBy: plan.fields_enclosed_by,
                FieldsEscapedBy: plan.fields_escaped_by,
                FieldsOptEnclosed: plan.fields_opt_enclosed,
                LinesStartingBy: plan.lines_starting_by,
                LinesTerminatedBy: plan.lines_terminated_by,
            },
            Charset: plan.charset,
            IgnoreLines: plan.ignore_lines,
            ThreadCnt: plan.thread_count,
            MaxNodeCnt: plan.max_node_count,
            SplitFile: plan.split_file,
            MaxRecordedErrors: plan.max_recorded_errors,
            Detached: plan.detached,
            DisableTiKVImportMode: plan.disable_tikv_import_mode,
            CloudStorageURI: plan.cloud_storage_uri,
            DisablePrecheck: plan.disable_precheck,
            GroupKey: plan.group_key,
            DistSQLScanConcurrency: plan.distsql_scan_concurrency,
            User: plan.user,
            TotalFileSize: plan.total_file_size,
            ForceMergeStep: plan.force_merge_step,
            ManualRecovery: plan.manual_recovery,
            Keyspace: plan.keyspace,
            UseNewCollate: plan.use_new_collate,
            ..Default::default()
        }
    }
}

/// TaskMeta 的 serde 线格式结构。
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct TaskMetaWire {
    #[serde(alias = "JobID")]
    job_id: i64,
    #[serde(alias = "Plan")]
    plan: PlanWire,
    #[serde(alias = "Stmt")]
    stmt: String,
    #[serde(alias = "Summary")]
    summary: SummaryWire,
    #[serde(alias = "EligibleInstances")]
    eligible_instances: Vec<ServerInfo>,
    #[serde(alias = "ChunkMap")]
    chunk_map: HashMap<i32, Vec<ChunkWire>>,
    #[serde(alias = "PreparedMetaExternalPath")]
    prepared_meta_external_path: String,
}

impl TaskMeta {
    /// 序列化为 JSON 字节。
    pub fn Marshal(&self) -> Result<Vec<u8>, errors::SharedError> {
        let wire = TaskMetaWire {
            job_id: self.JobID,
            plan: (&self.Plan).into(),
            stmt: self.Stmt.clone(),
            summary: (&self.Summary).into(),
            eligible_instances: self.EligibleInstances.clone(),
            chunk_map: self
                .ChunkMap
                .iter()
                .map(|(id, chunks)| (*id, chunks.iter().map(ChunkWire::from).collect()))
                .collect(),
            prepared_meta_external_path: self.PreparedMetaExternalPath.clone(),
        };
        serde_json::to_vec(&wire).map_err(|error| errors::New(error.to_string()))
    }

    /// 从 JSON 字节反序列化；校验 source_type/compression 枚举合法性。
    pub fn Unmarshal(bytes: &[u8]) -> Result<Self, errors::SharedError> {
        let wire: TaskMetaWire =
            serde_json::from_slice(bytes).map_err(|error| errors::New(error.to_string()))?;
        let chunk_map = wire
            .chunk_map
            .into_iter()
            .map(|(id, chunks)| {
                chunks
                    .into_iter()
                    .map(importer::Chunk::try_from)
                    .collect::<Result<Vec<_>, _>>()
                    .map(|chunks| (id, chunks))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(Self {
            JobID: wire.job_id,
            Plan: wire.plan.into(),
            Stmt: wire.stmt,
            Summary: wire.summary.into(),
            EligibleInstances: wire.eligible_instances,
            ChunkMap: chunk_map,
            PreparedMetaExternalPath: wire.prepared_meta_external_path,
        })
    }
}

// PreparedMeta 对应 Go 的 prepare 阶段外部元数据。
#[derive(Clone, Default)]
pub struct PreparedMeta {
    pub BaseExternalMeta: BaseExternalMeta,
    // Go tag `external:"true"` 表示大对象写入外部存储；这里保留字段形状。
    pub ChunkMap: std::collections::HashMap<i32, Vec<importer::Chunk>>,
}

impl PreparedMeta {
    pub fn Unmarshal(bytes: &[u8]) -> Result<Self, errors::SharedError> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|error| errors::New(error.to_string()))?;
        let chunks = value
            .get("chunk_map")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let wire: HashMap<i32, Vec<ChunkWire>> = if chunks.is_null() {
            HashMap::new()
        } else {
            serde_json::from_value(chunks).map_err(|error| errors::New(error.to_string()))?
        };
        let ChunkMap = wire
            .into_iter()
            .map(|(id, chunks)| {
                chunks
                    .into_iter()
                    .map(importer::Chunk::try_from)
                    .collect::<Result<Vec<_>, _>>()
                    .map(|chunks| (id, chunks))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(Self {
            ChunkMap,
            ..Default::default()
        })
    }
    // Marshal 对应 Go 的 BaseExternalMeta.Marshal(m)，只保留委托序列化语义。
    pub fn Marshal(&self) -> Result<Vec<u8>, errors::SharedError> {
        let mut value = serde_json::json!({"ExternalPath": self.BaseExternalMeta.ExternalPath});
        if self.BaseExternalMeta.ExternalPath.is_empty() {
            value["chunk_map"] = chunk_map_value(&self.ChunkMap);
        }
        self.BaseExternalMeta.marshal_value(&value)
    }
}

// ImportStepMeta 是 import/encode-and-sort 步骤的 subtask meta。
#[derive(Clone, Default)]
pub struct ImportStepMeta {
    pub BaseExternalMeta: BaseExternalMeta,
    // ID 是 engine ID，不是 tidb_background_subtask 表中的 subtask id。
    pub ID: i32,
    pub Chunks: Vec<importer::Chunk>,
    // Checksum 的 key 语义见 KVGroupChecksum；Go 中是 map[int64]Checksum。
    pub Checksum: std::collections::HashMap<i64, Checksum>,
    // MaxIDs 记录各 allocator type 在编码过程中使用到的最大值。
    pub MaxIDs: std::collections::HashMap<autoid::AllocatorType, i64>,
    pub SortedDataMeta: Option<SortedKVMeta>,
    // SortedIndexMetas 是 index id 到 sorted kv meta 的映射。
    pub SortedIndexMetas: std::collections::HashMap<i64, SortedKVMeta>,
    // RecordedConflictKVCount 汇总所有 SortedKVMeta 中的冲突 KV 数，避免无冲突时读取外部 meta。
    pub RecordedConflictKVCount: u64,
}

impl ImportStepMeta {
    /// Decode the persisted subtask envelope before loading optional external details.
    pub fn Unmarshal(bytes: &[u8]) -> Result<Self, errors::SharedError> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|error| errors::New(error.to_string()))?;
        let field = |name: &str| value.get(name).cloned().unwrap_or(serde_json::Value::Null);
        let id_value = field("ID");
        let id = if id_value.is_null() {
            0
        } else {
            let number = id_value
                .as_i64()
                .ok_or_else(|| errors::New("import step ID must be an integer"))?;
            i32::try_from(number).map_err(|_| errors::New("import step ID exceeds int32 range"))?
        };
        let chunks = match field("Chunks") {
            serde_json::Value::Null => Vec::new(),
            chunks => serde_json::from_value::<Vec<ChunkWire>>(chunks)
                .map_err(|error| errors::New(error.to_string()))?
                .into_iter()
                .map(importer::Chunk::try_from)
                .collect::<Result<Vec<_>, _>>()?,
        };
        let checksum = match field("Checksum") {
            serde_json::Value::Null => HashMap::new(),
            checksums => serde_json::from_value::<HashMap<String, Checksum>>(checksums)
                .map_err(|error| errors::New(error.to_string()))?
                .into_iter()
                .map(|(id, checksum)| {
                    id.parse::<i64>()
                        .map(|id| (id, checksum))
                        .map_err(|error| errors::New(error.to_string()))
                })
                .collect::<Result<HashMap<_, _>, _>>()?,
        };
        let max_ids = match field("MaxIDs") {
            serde_json::Value::Null => HashMap::new(),
            maximums => serde_json::from_value::<HashMap<String, i64>>(maximums)
                .map_err(|error| errors::New(error.to_string()))?
                .into_iter()
                .map(|(name, maximum)| {
                    let kind = match name.as_str() {
                        "_tidb_rowid" => autoid::AllocatorType::RowId,
                        "auto_increment" => autoid::AllocatorType::AutoIncrement,
                        "auto_random" => autoid::AllocatorType::AutoRandom,
                        "sequence" => autoid::AllocatorType::Sequence,
                        _ => return Err(errors::New(format!("unknown allocator type {name}"))),
                    };
                    Ok((kind, maximum))
                })
                .collect::<Result<HashMap<_, _>, _>>()?,
        };
        let sorted_data_meta = serde_json::from_value(field("SortedDataMeta"))
            .map_err(|error| errors::New(error.to_string()))?;
        let sorted_index_metas = match field("SortedIndexMetas") {
            serde_json::Value::Null => HashMap::new(),
            metas => {
                serde_json::from_value(metas).map_err(|error| errors::New(error.to_string()))?
            }
        };
        let path_value = field("ExternalPath");
        let external_path = if path_value.is_null() {
            String::new()
        } else {
            path_value
                .as_str()
                .ok_or_else(|| errors::New("external path must be a string"))?
                .to_owned()
        };
        let conflict_value = field("RecordedConflictKVCount");
        let recorded_conflict_count = if conflict_value.is_null() {
            0
        } else {
            conflict_value
                .as_u64()
                .ok_or_else(|| errors::New("recorded conflict count must be an unsigned integer"))?
        };
        Ok(Self {
            BaseExternalMeta: BaseExternalMeta {
                ExternalPath: external_path,
            },
            ID: id,
            Chunks: chunks,
            Checksum: checksum,
            MaxIDs: max_ids,
            SortedDataMeta: sorted_data_meta,
            SortedIndexMetas: sorted_index_metas,
            RecordedConflictKVCount: recorded_conflict_count,
        })
    }

    // Marshal 对应 Go 的 import step meta JSON 序列化。
    pub fn Marshal(&self) -> Result<Vec<u8>, errors::SharedError> {
        let checksums: serde_json::Map<String, serde_json::Value> = self
            .Checksum
            .iter()
            .map(|(id, checksum)| {
                (
                    id.to_string(),
                    serde_json::to_value(checksum).expect("checksum serialization is infallible"),
                )
            })
            .collect();
        let max_ids: serde_json::Map<String, serde_json::Value> = self
            .MaxIDs
            .iter()
            .map(|(kind, value)| (kind.as_str().to_owned(), (*value).into()))
            .collect();
        let mut value = serde_json::json!({
            "ExternalPath": self.BaseExternalMeta.ExternalPath,
            "ID": self.ID,
            "Checksum": if checksums.is_empty() { serde_json::Value::Null } else { serde_json::Value::Object(checksums) },
            "MaxIDs": if max_ids.is_empty() { serde_json::Value::Null } else { serde_json::Value::Object(max_ids) },
        });
        if self.RecordedConflictKVCount != 0 {
            value["RecordedConflictKVCount"] = self.RecordedConflictKVCount.into();
        }
        if self.BaseExternalMeta.ExternalPath.is_empty() {
            value["Chunks"] = if self.Chunks.is_empty() {
                serde_json::Value::Null
            } else {
                chunks_value(&self.Chunks)
            };
            value["SortedDataMeta"] = serde_json::to_value(&self.SortedDataMeta)
                .map_err(|error| errors::New(error.to_string()))?;
            value["SortedIndexMetas"] = if self.SortedIndexMetas.is_empty() {
                serde_json::Value::Null
            } else {
                serde_json::to_value(&self.SortedIndexMetas)
                    .map_err(|error| errors::New(error.to_string()))?
            };
        }
        self.BaseExternalMeta.marshal_value(&value)
    }
}

// MergeSortStepMeta 是 merge-sort 步骤的 meta。
#[derive(Clone, Default)]
pub struct MergeSortStepMeta {
    pub BaseExternalMeta: BaseExternalMeta,
    // KVGroup 是 sorted kv 的组名，可能是 dataKVGroup 或 index-id。
    pub KVGroup: String,
    pub DataFiles: Vec<String>,
    pub SortedKVMeta: SortedKVMeta,
    pub RecordedConflictKVCount: u64,
}

impl MergeSortStepMeta {
    // Marshal 对应 Go 的 merge sort step meta JSON 序列化。
    pub fn Marshal(&self) -> Result<Vec<u8>, errors::SharedError> {
        let mut value = serde_json::json!({
            "ExternalPath": self.BaseExternalMeta.ExternalPath,
            "kv-group": self.KVGroup,
        });
        if self.RecordedConflictKVCount != 0 {
            value["recorded-conflict-kv-count"] = self.RecordedConflictKVCount.into();
        }
        if self.BaseExternalMeta.ExternalPath.is_empty() {
            value["data-files"] = serde_json::to_value(&self.DataFiles)
                .map_err(|error| errors::New(error.to_string()))?;
            let sorted = serde_json::to_value(&self.SortedKVMeta)
                .map_err(|error| errors::New(error.to_string()))?;
            if let (Some(target), Some(sorted)) = (value.as_object_mut(), sorted.as_object()) {
                target.extend(
                    sorted
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
            }
        }
        self.BaseExternalMeta.marshal_value(&value)
    }
}

// WriteIngestStepMeta 是 write-and-ingest 步骤的 meta，仅在 global sort 路径使用。
#[derive(Clone, Default)]
pub struct WriteIngestStepMeta {
    pub BaseExternalMeta: BaseExternalMeta,
    pub KVGroup: String,
    pub SortedKVMeta: SortedKVMeta,
    pub RecordedConflictKVCount: u64,
    pub DataFiles: Vec<String>,
    pub StatFiles: Vec<String>,
    pub RangeJobKeys: Vec<Vec<u8>>,
    pub RangeSplitKeys: Vec<Vec<u8>>,
    pub TS: u64,
}

impl WriteIngestStepMeta {
    // Marshal 对应 Go 的 write ingest step meta JSON 序列化。
    pub fn Marshal(&self) -> Result<Vec<u8>, errors::SharedError> {
        let mut value = serde_json::json!({
            "ExternalPath": self.BaseExternalMeta.ExternalPath,
            "kv-group": self.KVGroup,
            "ts": self.TS,
        });
        if self.RecordedConflictKVCount != 0 {
            value["recorded-conflict-kv-count"] = self.RecordedConflictKVCount.into();
        }
        if self.BaseExternalMeta.ExternalPath.is_empty() {
            value["sorted-kv-meta"] = serde_json::to_value(&self.SortedKVMeta)
                .map_err(|error| errors::New(error.to_string()))?;
            value["data-files"] = serde_json::to_value(&self.DataFiles)
                .map_err(|error| errors::New(error.to_string()))?;
            value["stat-files"] = serde_json::to_value(&self.StatFiles)
                .map_err(|error| errors::New(error.to_string()))?;
            value["range-job-keys"] = go_byte_slices_value(&self.RangeJobKeys);
            value["range-split-keys"] = go_byte_slices_value(&self.RangeSplitKeys);
        }
        self.BaseExternalMeta.marshal_value(&value)
    }
}

// KVGroupConflictInfos 保存每个 kv group 的冲突信息。
#[derive(Clone, Default)]
pub struct KVGroupConflictInfos {
    pub ConflictInfos: std::collections::HashMap<String, engineapi::ConflictInfo>,
}

impl KVGroupConflictInfos {
    // addDataConflictInfo 对应 Go 的 data kv 冲突聚合入口。
    pub fn addDataConflictInfo(&mut self, other: &engineapi::ConflictInfo) {
        self.addConflictInfo(DATA_KV_GROUP, other);
    }

    // addIndexConflictInfo 先把 index id 转成 kv group，再复用通用合并逻辑。
    pub fn addIndexConflictInfo(&mut self, indexID: i64, other: &engineapi::ConflictInfo) {
        let kvGroup = index_id_to_kv_group(indexID);
        self.addConflictInfo(&kvGroup, other);
    }

    // addConflictInfo 对应 Go 的 lazy map 初始化和 ConflictInfo.Merge。
    pub fn addConflictInfo(&mut self, kvGroup: &str, other: &engineapi::ConflictInfo) {
        if other.Count == 0 {
            return;
        }
        let ci = self
            .ConflictInfos
            .entry(kvGroup.to_string())
            .or_insert_with(engineapi::ConflictInfo::default);
        ci.Merge(other);
    }
}

// CollectConflictsStepMeta 是 collect-conflicts 步骤的 meta。
#[derive(Clone, Default)]
pub struct CollectConflictsStepMeta {
    pub BaseExternalMeta: BaseExternalMeta,
    pub Infos: KVGroupConflictInfos,
    pub RecordedDataKVConflicts: i64,
    // Checksum 是所有冲突行的 checksum。
    pub Checksum: Option<Checksum>,
    // ConflictedRowCount 是所有冲突行数量。
    pub ConflictedRowCount: i64,
    // ConflictedRowFilenames 是记录给用户手动处理的冲突行文件名。
    pub ConflictedRowFilenames: Vec<String>,
    // 该标记表示由于 maxTotalConflictRowFileSize 达上限而停止记录冲突行文件。
    pub ConflictedRowRecordingCapped: bool,
    // TooManyConflictsFromIndex 为 true 时会跳过 checksum，因为索引冲突过多。
    pub TooManyConflictsFromIndex: bool,
}

impl CollectConflictsStepMeta {
    // Marshal 对应 Go 的 collect conflicts step meta JSON 序列化。
    pub fn Marshal(&self) -> Result<Vec<u8>, errors::SharedError> {
        let mut value = serde_json::json!({ "ExternalPath": self.BaseExternalMeta.ExternalPath });
        if self.RecordedDataKVConflicts != 0 {
            value["recorded-data-kv-conflicts"] = self.RecordedDataKVConflicts.into();
        }
        if let Some(checksum) = &self.Checksum {
            value["checksum"] =
                serde_json::to_value(checksum).map_err(|error| errors::New(error.to_string()))?;
        }
        if self.ConflictedRowCount != 0 {
            value["conflicted-row-count"] = self.ConflictedRowCount.into();
        }
        if !self.ConflictedRowFilenames.is_empty() {
            value["conflicted-row-filenames"] = serde_json::to_value(&self.ConflictedRowFilenames)
                .map_err(|error| errors::New(error.to_string()))?;
        }
        if self.ConflictedRowRecordingCapped {
            value["conflicted-row-recording-capped"] = true.into();
        }
        if self.TooManyConflictsFromIndex {
            value["too-many-conflicts-from-index"] = true.into();
        }
        if self.BaseExternalMeta.ExternalPath.is_empty() {
            value["infos"] = serde_json::json!({
                "conflict-infos": conflict_infos_value(&self.Infos)
            });
        }
        self.BaseExternalMeta.marshal_value(&value)
    }
}

// ConflictResolutionStepMeta 是 conflict-resolution 步骤的 meta。
#[derive(Clone, Default)]
pub struct ConflictResolutionStepMeta {
    pub BaseExternalMeta: BaseExternalMeta,
    pub Infos: KVGroupConflictInfos,
}

impl ConflictResolutionStepMeta {
    // Marshal 对应 Go 的 conflict resolution step meta JSON 序列化。
    pub fn Marshal(&self) -> Result<Vec<u8>, errors::SharedError> {
        let mut value = serde_json::json!({"ExternalPath": self.BaseExternalMeta.ExternalPath});
        if self.BaseExternalMeta.ExternalPath.is_empty() {
            value["infos"] = serde_json::json!({
                "conflict-infos": conflict_infos_value(&self.Infos)
            });
        }
        self.BaseExternalMeta.marshal_value(&value)
    }
}

// PostProcessStepMeta 是 post-process 步骤的 meta。
#[derive(Clone, Default)]
pub struct PostProcessStepMeta {
    // Checksum 累积 encode 步骤所有 subtask 的 checksum。
    pub Checksum: std::collections::HashMap<i64, Checksum>,
    // DeletedRowsChecksum 是冲突处理删除行的 checksum。
    pub DeletedRowsChecksum: Checksum,
    // 索引冲突过多时 deleted rows checksum 可能不准确，Go 会跳过校验。
    pub TooManyConflictsFromIndex: bool,
    // MaxIDs 保存所有 import subtask 的最大 allocator base。
    pub MaxIDs: std::collections::HashMap<autoid::AllocatorType, i64>,
}

// SharedVars 是一个 subtask 内所有 minimal task 共享的并发状态。
pub struct SharedVars {
    pub TableImporter: importer::TableImporter,
    pub DataEngine: Option<Arc<backend::OpenedEngine>>,
    pub IndexEngine: Option<Arc<backend::OpenedEngine>>,
    // Go 使用 sync.Mutex 保护 Checksum、SortedDataMeta 和 SortedIndexMetas 的聚合。
    pub mu: Mutex<()>,
    pub Checksum: verification::KVGroupChecksum,
    pub SortedDataMeta: SortedKVMeta,
    pub SortedIndexMetas: std::collections::HashMap<i64, SortedKVMeta>,
    pub RecordedConflictKVCount: u64,
    pub ShareMu: Mutex<()>,
    pub globalSortStore: Option<Arc<dyn storeapi::Storage>>,
    pub dataKVFileCount: AtomicI64,
    pub indexKVFileCount: AtomicI64,
}

impl SharedVars {
    // mergeDataSummary 对应 Go 的数据 KV writer summary 合并，锁内更新 sorted meta 和冲突计数。
    pub fn mergeDataSummary(&mut self, summary: &WriterSummary) {
        let _guard = self
            .mu
            .lock()
            .expect("shared import summary mutex poisoned");
        self.SortedDataMeta.MergeSummary(summary);
        self.RecordedConflictKVCount += summary.ConflictInfo.Count;
    }

    // mergeIndexSummary 对应 Go 的索引 KV writer summary 合并；缺失 index meta 时先创建。
    pub fn mergeIndexSummary(&mut self, indexID: i64, summary: &WriterSummary) {
        let _guard = self
            .mu
            .lock()
            .expect("shared import summary mutex poisoned");
        self.RecordedConflictKVCount += summary.ConflictInfo.Count;
        if let Some(meta) = self.SortedIndexMetas.get_mut(&indexID) {
            meta.MergeSummary(summary);
            return;
        }
        self.SortedIndexMetas
            .insert(indexID, new_sorted_kv_meta(summary));
    }
}

// importStepMinimalTask 是 IMPORT INTO 的 minimal task，Go 中由 TaskExecutor 把 Chunks 拆成 Chunk。
pub struct importStepMinimalTask {
    pub Plan: importer::Plan,
    pub Chunk: importer::Chunk,
    pub SharedVars: SharedVars,
    pub logger: Logger,
}

impl importStepMinimalTask {
    // RecoverArgs 对应 workerpool.TaskMayPanic 接口，返回 panic 指标标签和错误提示。
    pub fn RecoverArgs(&self) -> (String, String, errors::SharedError) {
        (
            framework_proto::ImportInto.to_owned(),
            "importStepMininalTask".to_string(),
            errors::New("panic occurred during import, please check log"),
        )
    }

    // String 对应 Go 的 fmt.Sprintf("chunk:%s:%d", Path, Offset)。
    pub fn String(&self) -> String {
        format!("chunk:{}:{}", self.Chunk.Path, self.Chunk.Offset)
    }
}

// Checksum records the checksum information.
/// KV checksum 三元组：异或和、KV 条数、总字节数。
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Checksum {
    pub Sum: u64,
    pub KVs: u64,
    pub Size: u64,
}

// newFromKVChecksum 从 verification.KVChecksum 抽取三个字段。
pub fn newFromKVChecksum(sum: &verification::KVChecksum) -> Checksum {
    Checksum {
        Sum: sum.Sum(),
        KVs: sum.SumKVS(),
        Size: sum.SumSize(),
    }
}

impl Checksum {
    // ToKVChecksum converts the Checksum to verification.KVChecksum.
    pub fn ToKVChecksum(&self) -> verification::KVChecksum {
        verification::MakeKVChecksum(self.Size, self.KVs, self.Sum)
    }
}

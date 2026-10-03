// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// Import Into 的 conflict-resolution（冲突解决）子任务执行器。
//
// 在 collect-conflicts 之后，按 KV 组串行解析冲突：创建查重专用编码器，
// 由调用方回调完成从对象存储读取冲突 KV 并删除集群中相关行的实际逻辑。
// 另提供 `createEncoders`，在 worker 启动前预创建全部编码器以避免数据竞争。

#![allow(non_camel_case_types, non_snake_case)]

use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicI64};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use astersql_dxf_framework_metering as metering;
use astersql_dxf_framework_proto::subtask::{StepResource, Subtask};
use astersql_dxf_framework_taskexecutor_execute as execute;
use astersql_dxf_framework_taskexecutor_execute::{
    Collector, StepExecFrameworkInfo, SubtaskSummary,
};
use astersql_dxf_importinto_conflictedkv::{
    ConflictContext, ConflictKVPair, ConflictRowCodec, ConflictStore, NewDeleter, TrafficRecorder,
};
use astersql_errors as errors;
use astersql_executor_importer::{
    ImportDatumConverter, NewTableDefinitionFromMeta, NewTableKVEncoderFromMeta, TableImporter,
    TableKVEncoder,
};
use astersql_ingestor_engineapi::ConflictInfo;
use astersql_ingestor_globalsort::{self as globalsort, Storage};
use astersql_kv::{Handle, IntHandle, Key};
use astersql_lightning_backend_encode::{EncodingConfig, SessionOptions};
use astersql_lightning_backend_kv::{self as backend_kv, Pairs};
use astersql_meta_model::TableInfo;
use astersql_objstore::storage as object_storage;
use astersql_objstore_recording as recording;
use astersql_tablecodec as tablecodec;
use astersql_types::datum::Datum;

use crate::proto::{ConflictResolutionStepMeta, KVGroupConflictInfos, TaskMeta};

/// Decode Go's inline wire shape and, when configured, load the external meta
/// once from the same object store used for conflicted KV files.
#[allow(non_snake_case)]
pub fn ReadConflictResolutionMeta(
    bytes: &[u8],
    object_store: &dyn Storage,
) -> Result<ConflictResolutionStepMeta, errors::SharedError> {
    let inline: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|error| errors::New(error.to_string()))?;
    let external_path = inline
        .get("ExternalPath")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let value = if external_path.is_empty() {
        inline
    } else {
        let external = object_store
            .read(&external_path)
            .map_err(|error| errors::New(error.to_string()))?;
        serde_json::from_slice(&external).map_err(|error| errors::New(error.to_string()))?
    };
    let mut meta = ConflictResolutionStepMeta::default();
    meta.BaseExternalMeta.ExternalPath = external_path;
    let groups = value
        .get("infos")
        .and_then(|infos| infos.get("conflict-infos"))
        .and_then(serde_json::Value::as_object);
    let mut infos = KVGroupConflictInfos::default();
    if let Some(groups) = groups {
        for (group, info) in groups {
            let count = match info.get("Count") {
                Some(value) => value
                    .as_u64()
                    .ok_or_else(|| errors::New(format!("invalid Count for group {group}")))?,
                None => 0,
            };
            let files = match info.get("Files") {
                Some(value) => value
                    .as_array()
                    .ok_or_else(|| errors::New(format!("invalid Files for group {group}")))?
                    .iter()
                    .map(|file| {
                        file.as_str().map(str::to_owned).ok_or_else(|| {
                            errors::New(format!("invalid filename for group {group}"))
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
                None => Vec::new(),
            };
            infos.ConflictInfos.insert(
                group.clone(),
                ConflictInfo {
                    Count: count,
                    Files: files,
                },
            );
        }
    }
    meta.Infos = infos;
    Ok(meta)
}

/// Conflict-resolution state independent of cluster-store adapters.
/// 与集群存储适配器解耦的冲突解决步骤状态。
pub struct conflictResolutionStepExecutor {
    pub tableImporter: TableImporter,
    pub summary: SubtaskSummary,
}

impl conflictResolutionStepExecutor {
    /// 构造执行器，持有查重用的 TableImporter 与进度摘要。
    pub fn new(table_importer: TableImporter) -> Self {
        Self {
            tableImporter: table_importer,
            summary: SubtaskSummary::default(),
        }
    }

    /// Resolve groups serially, as Go currently does. The callback owns the
    /// object-store reader and `ConflictStore` adapter; this method owns group
    /// ordering, concurrency selection and error propagation.
    /// 与 Go 一致：按组串行解析。回调负责对象存储读取与 ConflictStore 适配；
    /// 本方法负责组顺序、并发度选择与错误传播。
    pub fn RunGroups<F>(
        &mut self,
        meta: &ConflictResolutionStepMeta,
        concurrency: i32,
        mut resolve: F,
    ) -> Result<(), errors::SharedError>
    where
        F: FnMut(&str, &ConflictInfo, Vec<TableKVEncoder>) -> Result<(), errors::SharedError>,
    {
        let concurrency = concurrency.max(1);
        // 每组先预创建编码器，再交给回调做实际删除。
        for (kv_group, conflict_info) in &meta.Infos.ConflictInfos {
            let encoders = createEncoders(concurrency, &self.tableImporter)?;
            resolve(kv_group, conflict_info, encoders)?;
        }
        Ok(())
    }

    /// 关闭 TableImporter。
    pub fn Cleanup(&mut self) {
        self.tableImporter.Close();
    }

    /// 刷新并返回实时进度摘要。
    pub fn RealtimeSummary(&mut self) -> &SubtaskSummary {
        self.summary.Update();
        &self.summary
    }

    /// 重置进度计数。
    pub fn ResetSummary(&mut self) {
        self.summary.Reset();
    }
}

impl Collector for conflictResolutionStepExecutor {
    fn Accepted(&self, _accepted: i64) {}

    /// 累加已处理的冲突 KV 条数。
    fn Processed(&self, processed_conflict_kvs: i64, _bytes: i64) {
        self.summary
            .Processed
            .fetch_add(processed_conflict_kvs, Ordering::Relaxed);
    }
}

/// Read one group's SST files and run the same data/index deleter used by the
/// Go executor. Codecs are built before readers or workers start, preserving
/// generated-column initialization ordering. The caller supplies the cluster
/// and object-store adapters while this function owns the read/delete flow.
#[allow(non_snake_case)]
pub fn ResolveConflictGroup(
    context: &ConflictContext,
    object_store: Arc<dyn Storage>,
    cluster_store: Arc<dyn ConflictStore>,
    target_table: Arc<TableInfo>,
    kv_group: &str,
    info: &ConflictInfo,
    codecs: Vec<Box<dyn ConflictRowCodec + Send>>,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
    traffic_recorder: Option<Arc<dyn TrafficRecorder>>,
) -> Result<(), errors::SharedError> {
    let target_index = crate::collect_conflicts::getKVGroupIndexInfo(&target_table, kv_group)
        .map_err(errors::New)?;
    let cancellation = globalsort::reader::CancellationToken::default();
    let mut reader = globalsort::reader::ReadKVFilesAsync(
        cancellation.clone(),
        object_store,
        info.Files.clone(),
    );
    std::thread::scope(|scope| {
        let mut senders = Vec::with_capacity(codecs.len());
        let mut workers = Vec::with_capacity(codecs.len());
        for mut codec in codecs {
            codec.ConfigureKeyspace(cluster_store.Keyspace());
            let (sender, receiver) = mpsc::sync_channel::<ConflictKVPair>(
                astersql_dxf_importinto_conflictedkv::BufferedHandleLimit
                    .load(std::sync::atomic::Ordering::Acquire)
                    .max(1),
            );
            senders.push(sender);
            let table = target_table.clone();
            let store = cluster_store.clone();
            let traffic = traffic_recorder.clone();
            let progress = collector.clone();
            workers.push(scope.spawn(move || {
                let progress = progress.map(|collector| -> Arc<dyn Collector> { collector });
                let mut deleter = NewDeleter(table, store, kv_group, codec, progress, traffic);
                deleter.Run(context, &receiver)
            }));
        }

        let mut read_error = None;
        let mut index = 0usize;
        for pair in &mut reader {
            if context.IsCancelled() {
                read_error = Some("conflict resolution cancelled".to_owned());
                cancellation.cancel();
                break;
            }
            match pair {
                Ok(pair) => {
                    if !senders.is_empty() {
                        let input = ConflictKVPair {
                            Key: Key(pair.key.clone()),
                            Value: pair.value.clone(),
                        };
                        let worker = match crate::collect_conflicts::conflictWorkerForPair(
                            &input,
                            target_index.as_ref(),
                            senders.len(),
                            index,
                            &cluster_store.Keyspace(),
                        ) {
                            Ok(worker) => worker,
                            Err(error) => {
                                read_error = Some(error);
                                cancellation.cancel();
                                break;
                            }
                        };
                        if crate::collect_conflicts::sendConflictPair(
                            context,
                            &senders[worker],
                            input,
                        )
                        .is_err()
                        {
                            read_error = Some("conflict deleter worker stopped".to_owned());
                            cancellation.cancel();
                            break;
                        }
                        index += 1;
                    }
                }
                Err(error) => {
                    read_error = Some(error.to_string());
                    cancellation.cancel();
                    break;
                }
            }
        }
        drop(senders);
        let mut worker_error = None;
        for worker in workers {
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    worker_error.get_or_insert(error);
                }
                Err(_) => {
                    worker_error.get_or_insert("conflict deleter worker panicked".to_owned());
                }
            }
        }
        read_error
            .or(worker_error)
            .map_or(Ok(()), |error| Err(errors::New(error)))
    })
}

/// Construct each non-Send importer encoder inside its worker and wait for all
/// encoders to initialize before reading SST files. This preserves Go's race
/// avoidance without moving shared AST/session state between threads.
#[allow(non_snake_case, clippy::too_many_arguments)]
pub fn ResolveConflictGroupFromMeta(
    context: &ConflictContext,
    object_store: Arc<dyn Storage>,
    cluster_store: Arc<dyn ConflictStore>,
    target_table: Arc<TableInfo>,
    kv_group: &str,
    info: &ConflictInfo,
    concurrency: usize,
    session_options: SessionOptions,
    datum_converter: Arc<dyn ImportDatumConverter>,
    collector: Option<Arc<dyn Collector + Send + Sync>>,
    traffic_recorder: Option<Arc<dyn TrafficRecorder>>,
) -> Result<(), errors::SharedError> {
    let target_index = crate::collect_conflicts::getKVGroupIndexInfo(&target_table, kv_group)
        .map_err(errors::New)?;
    std::thread::scope(|scope| {
        let (ready_sender, ready_receiver) = mpsc::channel::<Result<(), String>>();
        let mut senders = Vec::with_capacity(concurrency);
        let mut workers = Vec::with_capacity(concurrency);
        for _ in 0..concurrency {
            let (sender, receiver) = mpsc::sync_channel::<ConflictKVPair>(
                astersql_dxf_importinto_conflictedkv::BufferedHandleLimit
                    .load(std::sync::atomic::Ordering::Acquire)
                    .max(1),
            );
            senders.push(sender);
            let ready = ready_sender.clone();
            let table = target_table.clone();
            let store = cluster_store.clone();
            let options = session_options.clone();
            let converter = datum_converter.clone();
            let progress = collector.clone();
            let traffic = traffic_recorder.clone();
            workers.push(scope.spawn(move || {
                let codec = (|| -> Result<ImporterConflictCodec, String> {
                    let definition = NewTableDefinitionFromMeta(&table)?;
                    let config = EncodingConfig {
                        Table: Some(Arc::new(definition)),
                        UseIdentityAutoRowID: true,
                        SessionOptions: options.clone(),
                        ..EncodingConfig::default()
                    };
                    let encoder = NewTableKVEncoderFromMeta(&config, &table, converter)?;
                    NewImporterConflictCodecWithOptions(encoder, &table, &options)
                        .map_err(|error| error.to_string())
                })();
                match codec {
                    Ok(mut codec) => {
                        codec.ConfigureKeyspace(store.Keyspace());
                        let _ = ready.send(Ok(()));
                        let progress =
                            progress.map(|collector| -> Arc<dyn Collector> { collector });
                        let mut deleter =
                            NewDeleter(table, store, kv_group, Box::new(codec), progress, traffic);
                        deleter.Run(context, &receiver)
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error.clone()));
                        Err(error)
                    }
                }
            }));
        }
        drop(ready_sender);
        let mut init_error = None;
        for _ in 0..concurrency {
            match ready_receiver.recv() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    init_error.get_or_insert(error);
                }
                Err(_) => {
                    init_error
                        .get_or_insert("conflict worker exited before initialization".to_owned());
                }
            }
        }

        let cancellation = globalsort::reader::CancellationToken::default();
        let mut read_error = None;
        if init_error.is_none() {
            let mut reader = globalsort::reader::ReadKVFilesAsync(
                cancellation.clone(),
                object_store,
                info.Files.clone(),
            );
            let mut index = 0usize;
            for pair in &mut reader {
                if context.IsCancelled() {
                    read_error = Some("conflict resolution cancelled".to_owned());
                    cancellation.cancel();
                    break;
                }
                match pair {
                    Ok(pair) => {
                        if !senders.is_empty() {
                            let input = ConflictKVPair {
                                Key: Key(pair.key.clone()),
                                Value: pair.value.clone(),
                            };
                            let worker = match crate::collect_conflicts::conflictWorkerForPair(
                                &input,
                                target_index.as_ref(),
                                senders.len(),
                                index,
                                &cluster_store.Keyspace(),
                            ) {
                                Ok(worker) => worker,
                                Err(error) => {
                                    read_error = Some(error);
                                    cancellation.cancel();
                                    break;
                                }
                            };
                            if crate::collect_conflicts::sendConflictPair(
                                context,
                                &senders[worker],
                                input,
                            )
                            .is_err()
                            {
                                read_error = Some("conflict deleter worker stopped".to_owned());
                                cancellation.cancel();
                                break;
                            }
                            index += 1;
                        }
                    }
                    Err(error) => {
                        read_error = Some(error.to_string());
                        cancellation.cancel();
                        break;
                    }
                }
            }
        }
        drop(senders);
        let mut worker_error = None;
        for worker in workers {
            match worker.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    worker_error.get_or_insert(error);
                }
                Err(_) => {
                    worker_error.get_or_insert("conflict deleter worker panicked".to_owned());
                }
            }
        }
        init_error
            .or(read_error)
            .or(worker_error)
            .map_or(Ok(()), |error| Err(errors::New(error)))
    })
}

/// Bridge the production object-store API to the global-sort SST reader.
#[derive(Clone)]
pub struct ConflictObjectStorage {
    pub store: object_storage::StorageRef,
    pub context: object_storage::Context,
    pub access: Option<Arc<recording::AccessStats>>,
}

impl Storage for ConflictObjectStorage {
    fn read(&self, path: &str) -> globalsort::Result<Vec<u8>> {
        let bytes = self
            .store
            .ReadFile(&self.context, path)
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))?;
        if let Some(access) = &self.access {
            access.requests.get.fetch_add(1, Ordering::Relaxed);
            access
                .traffic
                .read
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        }
        Ok(bytes)
    }

    fn write(&self, path: &str, value: Vec<u8>) -> globalsort::Result<()> {
        self.store
            .WriteFile(&self.context, path, &value)
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))?;
        if let Some(access) = &self.access {
            access.requests.put.fetch_add(1, Ordering::Relaxed);
            access
                .traffic
                .write
                .fetch_add(value.len() as u64, Ordering::Relaxed);
        }
        Ok(())
    }

    fn delete_files(&self, paths: &[String]) -> globalsort::Result<()> {
        self.store
            .DeleteFiles(&self.context, paths)
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))
    }

    fn list_prefix(&self, prefix: &str) -> globalsort::Result<Vec<String>> {
        let mut paths = Vec::new();
        self.store
            .WalkDir(
                &self.context,
                Some(&object_storage::WalkOption {
                    obj_prefix: prefix.to_owned(),
                    ..Default::default()
                }),
                &mut |path, _size| {
                    paths.push(path.to_owned());
                    Ok(())
                },
            )
            .map_err(|error| globalsort::Error::InvalidData(error.to_string()))?;
        Ok(paths)
    }
}

/// Production bridge from an eagerly created importer encoder to the
/// conflicted-KV handler's codec contract.
pub struct ImporterConflictCodec {
    keyspace: Vec<u8>,
    encoder: TableKVEncoder,
    decoder: backend_kv::TableKVDecoder,
}

#[allow(non_snake_case)]
pub fn NewImporterConflictCodec(
    encoder: TableKVEncoder,
    table: &TableInfo,
) -> Result<ImporterConflictCodec, errors::SharedError> {
    NewImporterConflictCodecWithOptions(encoder, table, &SessionOptions::default())
}

#[allow(non_snake_case)]
pub fn NewImporterConflictCodecWithOptions(
    mut encoder: TableKVEncoder,
    table: &TableInfo,
    options: &SessionOptions,
) -> Result<ImporterConflictCodec, errors::SharedError> {
    let definition = match NewTableDefinitionFromMeta(table) {
        Ok(definition) => definition,
        Err(error) => {
            let _ = encoder.Close();
            return Err(errors::New(error));
        }
    };
    let decoder = match backend_kv::NewTableKVDecoder(definition, &table.Name.O, options) {
        Ok(decoder) => decoder,
        Err(error) => {
            let _ = encoder.Close();
            return Err(errors::New(error));
        }
    };
    Ok(ImporterConflictCodec {
        encoder,
        decoder,
        keyspace: Vec::new(),
    })
}

fn importerHandle(handle: &dyn tablecodec::kv::Handle) -> Result<Box<dyn Handle>, String> {
    if let Some(partition) = handle
        .as_any()
        .downcast_ref::<tablecodec::kv::PartitionHandle>()
    {
        return Ok(Box::new(astersql_kv::NewPartitionHandle(
            partition.PartitionID,
            importerHandle(partition.Handle.as_ref())?,
        )));
    }
    if handle.IsInt() {
        Ok(Box::new(IntHandle(handle.IntValue())))
    } else {
        astersql_kv::NewCommonHandle(handle.Encoded())
            .map(|handle| Box::new(handle) as Box<dyn Handle>)
            .map_err(|error| error.to_string())
    }
}

impl ConflictRowCodec for ImporterConflictCodec {
    fn ConfigureKeyspace(&mut self, keyspace: Vec<u8>) {
        self.keyspace = keyspace;
    }
    fn StripKeyspacePrefix(&self, key: &Key) -> Result<Key, String> {
        crate::collect_conflicts::decodeConflictKey(key, &self.keyspace)
    }

    fn DecodeRowKey(&self, key: &Key) -> Result<Box<dyn Handle>, String> {
        let handle = tablecodec::DecodeRowKey(tablecodec::kv::Key(key.0.clone()))
            .map_err(|error| error.to_string())?;
        importerHandle(handle.as_ref())
    }

    fn DecodeRow(&self, handle: &dyn Handle, value: &[u8]) -> Result<Vec<Datum>, String> {
        let decoded_handle = if handle.IsInt() {
            backend_kv::Handle::Int(handle.IntValue())
        } else {
            let keys = handle
                .Data()
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(
                    |datum| match backend_kv::fromCanonicalDatum(&datum, None)? {
                        astersql_lightning_backend_encode::Datum::Int(value) => {
                            Ok(backend_kv::DatumKey::Int(value))
                        }
                        astersql_lightning_backend_encode::Datum::UInt(value) => {
                            Ok(backend_kv::DatumKey::UInt(value))
                        }
                        astersql_lightning_backend_encode::Datum::String(value) => {
                            Ok(backend_kv::DatumKey::String(value))
                        }
                        astersql_lightning_backend_encode::Datum::Bytes(value) => {
                            Ok(backend_kv::DatumKey::Bytes(value))
                        }
                        other => Err(format!("unsupported common-handle datum {other:?}")),
                    },
                )
                .collect::<Result<Vec<_>, String>>()?;
            backend_kv::Handle::Common(keys)
        };
        let (row, _) = self.decoder.DecodeRawRowData(&decoded_handle, value)?;
        row.iter().map(backend_kv::toCanonicalDatum).collect()
    }

    fn DecodeTableID(&self, key: &Key) -> i64 {
        tablecodec::DecodeTableID(tablecodec::kv::Key(key.0.clone()))
    }

    fn DecodeIndexHandle(
        &self,
        key: &Key,
        value: &[u8],
        index_column_count: usize,
    ) -> Result<Box<dyn Handle>, String> {
        if key.0.len() < tablecodec::prefixLen + tablecodec::idLen {
            return Err("invalid index key".to_owned());
        }
        let handle =
            tablecodec::DecodeIndexHandle(key.0.clone(), value.to_vec(), index_column_count)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "index value does not contain a handle".to_owned())?;
        importerHandle(handle.as_ref())
    }

    fn EncodeRowKey(&self, table_id: i64, handle: &dyn Handle) -> Key {
        let physical_id = handle
            .as_any()
            .downcast_ref::<astersql_kv::PartitionHandle>()
            .map_or(table_id, |partition| partition.PartitionID);
        Key(tablecodec::EncodeRowKey(physical_id, &handle.Encoded()).0)
    }

    fn EncodeRow(
        &mut self,
        _handle: &dyn Handle,
        row: &[Datum],
        auto_row_id: i64,
    ) -> Result<Pairs, String> {
        let row = row
            .iter()
            .map(|datum| backend_kv::fromCanonicalDatum(datum, None))
            .collect::<Result<Vec<_>, _>>()?;
        self.encoder.Encode(&row, auto_row_id)
    }

    fn Close(&mut self) -> Result<(), String> {
        self.encoder.Close()
    }
}

/// The part of TableImporter consumed by conflict resolution. A production
/// importer uses the same Plan/table/converter and closes its resources here.
pub trait ConflictResolutionImporter {
    fn Plan(&self) -> &astersql_executor_importer::Plan;
    fn TableInfo(&self) -> Arc<TableInfo>;
    fn DatumConverter(&self) -> Arc<dyn ImportDatumConverter>;
    fn Close(&mut self);
}

impl ConflictResolutionImporter for TableImporter {
    fn Plan(&self) -> &astersql_executor_importer::Plan {
        &self.LoadDataController.Plan
    }
    fn TableInfo(&self) -> Arc<TableInfo> {
        Arc::new(self.LoadDataController.Table.Meta().clone())
    }
    fn DatumConverter(&self) -> Arc<dyn ImportDatumConverter> {
        self.LoadDataController.DatumConverter.clone()
    }
    fn Close(&mut self) {
        TableImporter::Close(self);
    }
}

/// External integrations required to construct an importer and open its
/// object store. The executor keeps Go's lifecycle and conflict pipeline;
/// applications provide their SQL session/storage implementation here.
pub trait ConflictResolutionRuntime: Send + Sync {
    fn BuildImporter(
        &self,
        task_id: i64,
        meta: &TaskMeta,
    ) -> Result<Box<dyn ConflictResolutionImporter>, String>;
    fn OpenObjectStore(
        &self,
        context: &execute::Context,
        uri: &str,
    ) -> Result<object_storage::StorageRef, String>;
    fn ClusterStore(&self) -> Arc<dyn ConflictStore>;
}

/// Production bridge from persisted task metadata to importer and object storage.
/// The host supplies its importer services; table construction, statement parsing,
/// controller initialization and TableImporter creation follow Go's getTableImporter.
pub struct ConfiguredConflictResolutionRuntime {
    pub ControllerServices:
        Arc<dyn Fn() -> astersql_executor_importer::LoadDataControllerServices + Send + Sync>,
    pub ImporterService: Arc<dyn astersql_executor_importer::TableImporterService>,
    pub Cluster: Arc<dyn ConflictStore>,
}

impl ConflictResolutionRuntime for ConfiguredConflictResolutionRuntime {
    fn BuildImporter(
        &self,
        task_id: i64,
        meta: &TaskMeta,
    ) -> Result<Box<dyn ConflictResolutionImporter>, String> {
        let table_info = meta
            .Plan
            .TableInfo
            .as_ref()
            .ok_or_else(|| "import task plan has no table info".to_owned())?;
        let table = astersql_table::BuildTableFromMeta(table_info)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "table metadata factory is not installed".to_owned())?;
        let args = astersql_executor_importer::ASTArgsFromStmt(&meta.Stmt)?;
        let mut controller = astersql_executor_importer::NewLoadDataController(
            meta.Plan.clone(),
            Arc::from(table),
            args,
            (self.ControllerServices)(),
            Vec::new(),
        )?;
        controller.InitDataStore(&astersql_objstore_storeapi::Context::default())?;
        let importer = astersql_executor_importer::NewTableImporter(
            controller,
            task_id.to_string(),
            self.ImporterService.clone(),
        )?;
        Ok(Box::new(importer))
    }

    fn OpenObjectStore(
        &self,
        context: &execute::Context,
        uri: &str,
    ) -> Result<object_storage::StorageRef, String> {
        if context.is_cancelled() {
            return Err("conflict resolution cancelled".to_owned());
        }
        object_storage::NewFromURL(&object_storage::Context::default(), uri)
            .map_err(|error| error.to_string())
    }

    fn ClusterStore(&self) -> Arc<dyn ConflictStore> {
        self.Cluster.clone()
    }
}

struct ResolutionProgress(Arc<AtomicI64>);
impl Collector for ResolutionProgress {
    fn Accepted(&self, _accepted: i64) {}
    fn Processed(&self, processed: i64, _bytes: i64) {
        self.0.fetch_add(processed, Ordering::Relaxed);
    }
}

struct ResolutionMeterTraffic(Arc<metering::Recorder>);
impl TrafficRecorder for ResolutionMeterTraffic {
    fn IncClusterReadBytes(&self, bytes: u64) {
        self.0.IncClusterReadBytes(bytes);
    }
    fn IncClusterWriteBytes(&self, bytes: u64) {
        self.0.IncClusterWriteBytes(bytes);
    }
}

/// Framework-facing conflict-resolution executor. Unlike the lower-level
/// state helper, this owns Init/RunSubtask/Cleanup and the object-store guard.
pub struct ConflictResolutionStepExecutor {
    task_id: i64,
    task_meta: TaskMeta,
    runtime: Arc<dyn ConflictResolutionRuntime>,
    importer: Option<Box<dyn ConflictResolutionImporter>>,
    summary: SubtaskSummary,
    framework: Option<execute::FrameworkInfo>,
}

#[allow(non_snake_case)]
pub fn NewConflictResolutionStepExecutor(
    task_id: i64,
    task_meta: TaskMeta,
    runtime: Arc<dyn ConflictResolutionRuntime>,
) -> ConflictResolutionStepExecutor {
    ConflictResolutionStepExecutor {
        task_id,
        task_meta,
        runtime,
        importer: None,
        summary: SubtaskSummary::default(),
        framework: None,
    }
}

impl execute::StepExecFrameworkInfo for ConflictResolutionStepExecutor {
    fn restricted(&self) {}
    fn GetStep(&self) -> astersql_dxf_framework_proto::step::Step {
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
    fn GetMeterRecorder(&self) -> Option<Arc<metering::Recorder>> {
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

impl execute::StepExecutor for ConflictResolutionStepExecutor {
    fn Init(&mut self, context: execute::Context) -> anyhow::Result<()> {
        if context.is_cancelled() {
            return Err(anyhow::anyhow!("conflict resolution cancelled"));
        }
        self.importer = Some(
            self.runtime
                .BuildImporter(self.task_id, &self.task_meta)
                .map_err(|error| anyhow::anyhow!(error))?,
        );
        Ok(())
    }

    fn RunSubtask(
        &mut self,
        context: execute::Context,
        subtask: &mut Subtask,
    ) -> anyhow::Result<()> {
        let importer = self
            .importer
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("conflict resolution is not initialized"))?;
        let plan = importer.Plan();
        let table = importer.TableInfo();
        let converter = importer.DatumConverter();
        let options = SessionOptions {
            SQLMode: plan.SQLMode.0 as u64,
            SysVars: plan.ImportantSysVars.clone(),
            ..SessionOptions::default()
        };
        let concurrency = self
            .GetResource()
            .map(|resource| resource.CPU.Capacity().max(0) as usize)
            .unwrap_or(0);
        let raw_store = self
            .runtime
            .OpenObjectStore(&context, &plan.CloudStorageURI)
            .map_err(|error| anyhow::anyhow!(error))?;
        let access = Arc::new(recording::AccessStats::default());
        let object_context = object_storage::Context::default();
        let object_store: Arc<dyn Storage> = Arc::new(ConflictObjectStorage {
            store: raw_store.clone(),
            context: object_context.clone(),
            access: Some(access.clone()),
        });
        let conflict_context = ConflictContext::default();
        let watcher_done = Arc::new(AtomicBool::new(false));
        let watcher = {
            let token = context.clone();
            let conflict = conflict_context.clone();
            let objects = object_context.clone();
            let done = watcher_done.clone();
            std::thread::spawn(move || {
                while !done.load(Ordering::Acquire) {
                    if token.is_cancelled() {
                        conflict.Cancel();
                        objects.cancel();
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            })
        };
        let meter = self.GetMeterRecorder();
        let traffic: Option<Arc<dyn TrafficRecorder>> = meter
            .clone()
            .map(|meter| Arc::new(ResolutionMeterTraffic(meter)) as Arc<dyn TrafficRecorder>);
        let result = (|| -> anyhow::Result<()> {
            let meta = ReadConflictResolutionMeta(&subtask.Meta, object_store.as_ref())
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
            for (group, info) in &meta.Infos.ConflictInfos {
                let processed = Arc::new(AtomicI64::new(0));
                let collector: Arc<dyn Collector + Send + Sync> =
                    Arc::new(ResolutionProgress(processed.clone()));
                let group_result = ResolveConflictGroupFromMeta(
                    &conflict_context,
                    object_store.clone(),
                    self.runtime.ClusterStore(),
                    table.clone(),
                    group,
                    info,
                    concurrency,
                    options.clone(),
                    converter.clone(),
                    Some(collector),
                    traffic.clone(),
                );
                self.summary
                    .Processed
                    .fetch_add(processed.load(Ordering::Relaxed), Ordering::Relaxed);
                group_result.map_err(|error| anyhow::anyhow!(error.to_string()))?;
            }
            Ok(())
        })();
        watcher_done.store(true, Ordering::Release);
        let _ = watcher.join();
        raw_store.Close();
        self.summary.MergeObjStoreRequests(&access.requests);
        if let Some(meter) = meter {
            meter.MergeObjStoreAccess(&access);
        }
        result
    }

    fn RealtimeSummary(&mut self) -> Option<&SubtaskSummary> {
        self.summary.Update();
        Some(&self.summary)
    }
    fn ResetSummary(&mut self) {
        self.summary.Reset();
    }
    fn Cleanup(&mut self, _context: execute::Context) -> anyhow::Result<()> {
        if let Some(mut importer) = self.importer.take() {
            importer.Close();
        }
        Ok(())
    }
    fn TaskMetaModified(
        &mut self,
        _context: execute::Context,
        _meta: Vec<u8>,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("not implemented"))
    }
    fn ResourceModified(
        &mut self,
        _context: execute::Context,
        _resource: &StepResource,
    ) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("not implemented"))
    }
    fn SetFrameworkInfo(&mut self, info: execute::FrameworkInfo) {
        self.framework = Some(info);
    }
}

/// Eagerly initialize every encoder before workers start. Generated-column
/// expression construction mutates AST nodes, so lazy worker initialization
/// would reproduce the data race documented in the Go implementation.
/// 在 worker 启动前急切初始化全部编码器。生成列表达式构造会修改 AST 节点，
/// 若在 worker 内惰性初始化会复现 Go 侧记录的数据竞争。
pub fn createEncoders(
    concurrency: i32,
    table_importer: &TableImporter,
) -> Result<Vec<TableKVEncoder>, errors::SharedError> {
    let mut encoders = Vec::with_capacity(concurrency.max(0) as usize);
    for _ in 0..concurrency.max(0) {
        match table_importer.GetKVEncoderForDupResolve() {
            Ok(encoder) => encoders.push(encoder),
            Err(error) => {
                // 创建失败时关闭已成功打开的编码器，避免资源泄漏。
                for encoder in &mut encoders {
                    let _ = encoder.Close();
                }
                return Err(errors::New(error));
            }
        }
    }
    Ok(encoders)
}

#[cfg(test)]
pub(crate) fn importerHandleForTest(
    handle: &dyn tablecodec::kv::Handle,
) -> Result<Box<dyn Handle>, String> {
    importerHandle(handle)
}

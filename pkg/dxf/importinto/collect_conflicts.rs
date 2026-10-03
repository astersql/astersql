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

// Import Into 的 collect-conflicts 子任务执行器。
//
// 在冲突解决（conflict resolution）流程的前半段，按 KV 组串行消费冲突元信息：
// 对每组冲突 KV 创建编码器、收集冲突行与 checksum（校验和），
// 并在子任务结束时把结果写回步骤元数据，供后续 resolve 与远程校验使用。

#![allow(non_camel_case_types, non_snake_case)]

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use astersql_dxf_framework_taskexecutor_execute::{Collector, SubtaskSummary};
use astersql_dxf_importinto_conflictedkv::{
    BoundedKeySet, CollectResult, NewBoundedKeySet, NewCollectResult,
};
use astersql_errors as errors;
use astersql_executor_importer::{TableImporter, TableKVEncoder};
use astersql_ingestor_engineapi::ConflictInfo;

use crate::conflict_resolution::createEncoders;
use crate::proto::{CollectConflictsStepMeta, newFromKVChecksum};

/// Per-subtask aggregation state for conflict collection. Object-store and
/// cluster-store adapters are supplied by the caller, while this type retains
/// the same memory cap and checksum de-duplication state as Go.
/// 每个子任务的冲突收集聚合状态。对象存储与集群存储适配器由调用方注入；
/// 本类型保留与 Go 一致的内存上限，以及基于完整行键的 checksum 去重状态。
pub struct collectConflictsStepExecutor {
    pub taskID: i64,
    pub tableImporter: TableImporter,
    pub currSubtaskID: i64,
    /// 索引侧已收集的行键占用字节数（跨组共享）。
    pub sizeOfRowKeysFromIndex: Arc<AtomicI64>,
    /// 行键集合的内存上限（通常为子任务内存容量的一半）。
    pub sizeLimitOfRowKeysFromIndex: i64,
    /// 已写出冲突行文件的总大小（跨 worker 共享）。
    pub sizeOfConflictRowFiles: Arc<AtomicI64>,
    pub result: CollectResult,
    /// 有界行键集合：用于唯一索引冲突行的 checksum 去重。
    pub sharedRowKeySet: BoundedKeySet,
    pub summary: SubtaskSummary,
}

impl collectConflictsStepExecutor {
    /// 构造执行器；初始行键上限为 0，需在子任务开始时 `resetForNewSubtask`。
    pub fn new(task_id: i64, table_importer: TableImporter) -> Self {
        let handle_size = Arc::new(AtomicI64::new(0));
        let result = NewCollectResult(&table_importer.GetKeySpace());
        Self {
            taskID: task_id,
            tableImporter: table_importer,
            currSubtaskID: 0,
            sizeOfRowKeysFromIndex: Arc::clone(&handle_size),
            sizeLimitOfRowKeysFromIndex: 0,
            sizeOfConflictRowFiles: Arc::new(AtomicI64::new(0)),
            result,
            sharedRowKeySet: NewBoundedKeySet(handle_size, 0),
            summary: SubtaskSummary::default(),
        }
    }

    /// 为新子任务重置计数器、结果与有界行键集合。
    /// `memory_capacity` 的一半用作索引行键去重的内存上限。
    pub fn resetForNewSubtask(&mut self, subtask_id: i64, memory_capacity: i64) {
        self.currSubtaskID = subtask_id;
        self.sizeOfRowKeysFromIndex.store(0, Ordering::Release);
        self.sizeOfConflictRowFiles.store(0, Ordering::Release);
        self.sizeLimitOfRowKeysFromIndex = memory_capacity / 2;
        self.result = NewCollectResult(&self.tableImporter.GetKeySpace());
        self.sharedRowKeySet = NewBoundedKeySet(
            Arc::clone(&self.sizeOfRowKeysFromIndex),
            self.sizeLimitOfRowKeysFromIndex,
        );
    }

    /// Collect groups serially because one row can conflict in several unique
    /// index groups. The callback may fan each group out to `concurrency`
    /// workers after all encoders have been initialized.
    /// 串行处理各 KV 组：同一行可能在多个唯一索引组中冲突。
    /// 回调可在编码器全部初始化后，将该组扇出到 `concurrency` 个 worker。
    pub fn RunGroups<F>(
        &mut self,
        meta: &CollectConflictsStepMeta,
        concurrency: i32,
        mut collect: F,
    ) -> Result<(), errors::SharedError>
    where
        F: FnMut(
            &str,
            &ConflictInfo,
            Vec<TableKVEncoder>,
            &mut BoundedKeySet,
        ) -> Result<CollectResult, errors::SharedError>,
    {
        let concurrency = concurrency.max(1);
        // 按组串行：先创建编码器，再交给调用方完成实际收集并合并结果。
        for (kv_group, conflict_info) in &meta.Infos.ConflictInfos {
            getKVGroupIndexInfo(
                &self.tableImporter.LoadDataController.Table.Meta(),
                kv_group,
            )
            .map_err(errors::New)?;
            let encoders = createEncoders(concurrency, &self.tableImporter)?;
            let result = collect(kv_group, conflict_info, encoders, &mut self.sharedRowKeySet)?;
            self.result.Merge(Some(&result));
        }
        Ok(())
    }

    /// 子任务结束时把 checksum、冲突行数/文件名与截断标记写回步骤元数据。
    pub fn onFinished(&self, meta: &mut CollectConflictsStepMeta) {
        applyCollectResult(meta, &self.result, self.sharedRowKeySet.BoundExceeded());
    }

    /// 关闭底层 TableImporter，释放导入相关资源。
    pub fn Cleanup(&mut self) {
        self.tableImporter.Close();
    }

    /// 刷新并返回实时子任务进度摘要。
    pub fn RealtimeSummary(&mut self) -> &SubtaskSummary {
        self.summary.Update();
        &self.summary
    }

    /// 重置进度计数，供下一子任务复用。
    pub fn ResetSummary(&mut self) {
        self.summary.Reset();
    }
}

/// Apply the aggregate result to subtask metadata exactly as Go's
/// `onFinished` does. Keeping this transformation independent from the live
/// importer makes every persisted field directly testable without replacing
/// the object-store or cluster-store boundary with a weaker mock.
pub(crate) fn applyCollectResult(
    meta: &mut CollectConflictsStepMeta,
    result: &CollectResult,
    too_many_conflicts_from_index: bool,
) {
    meta.Checksum = Some(newFromKVChecksum(&result.Checksum));
    meta.ConflictedRowCount = result.RowCount;
    meta.ConflictedRowFilenames.clone_from(&result.Filenames);
    meta.ConflictedRowRecordingCapped = result.RowRecordingCapped;
    meta.TooManyConflictsFromIndex = too_many_conflicts_from_index;
}

impl Collector for collectConflictsStepExecutor {
    fn Accepted(&self, _accepted: i64) {}

    /// 累加已处理的冲突 KV 条数到子任务进度。
    fn Processed(&self, processed_conflict_kvs: i64, _bytes: i64) {
        self.summary
            .Processed
            .fetch_add(processed_conflict_kvs, Ordering::Relaxed);
    }
}

/// Conflict row files deliberately live outside `<task-id>/`, because cleanup
/// removes that directory while users must be able to inspect these files.
/// 冲突行文件刻意放在 `<task-id>/` 之外：清理会删除任务目录，
/// 但用户仍需能事后检查这些冲突行文件。
pub fn getConflictRowFilenamePrefix(taskID: i64, subtaskID: i64, uuid: &str) -> String {
    Path::new("conflicted-rows")
        .join(taskID.to_string())
        .join(format!("{subtaskID}-{uuid}"))
        .to_string_lossy()
        .into_owned()
}

/// Validate index metadata before starting encoders/readers.
pub fn getKVGroupIndexInfo(
    table: &astersql_meta_model::TableInfo,
    group: &str,
) -> Result<Option<astersql_meta_model::IndexInfo>, String> {
    if group == astersql_dxf_importinto_conflictedkv::DataKVGroup {
        return Ok(None);
    }
    let id = group.parse::<i64>().map_err(|error| error.to_string())?;
    table
        .Indices
        .iter()
        .find(|index| index.ID == id)
        .cloned()
        .map(Some)
        .ok_or_else(|| {
            format!(
                "index {id} from KV group {group:?} not found in table {}",
                table.Name.O
            )
        })
}

/// Same CRC32 IEEE routing as Go: all MVI entries for one encoded handle stay together.
pub fn conflictWorkerForPair(
    pair: &astersql_dxf_importinto_conflictedkv::ConflictKVPair,
    index: Option<&astersql_meta_model::IndexInfo>,
    workers: usize,
    ordinal: usize,
    keyspace: &[u8],
) -> Result<usize, String> {
    if workers == 0 {
        return Err("conflict group requires a worker".to_owned());
    }
    let Some(index) = index.filter(|index| index.MVIndex && workers > 1) else {
        return Ok(ordinal % workers);
    };
    mvIndexWorker(pair, index, workers, keyspace)
}

fn mvIndexWorker(
    pair: &astersql_dxf_importinto_conflictedkv::ConflictKVPair,
    index: &astersql_meta_model::IndexInfo,
    workers: usize,
    keyspace: &[u8],
) -> Result<usize, String> {
    if workers == 0 {
        return Err("conflict group requires a worker".to_owned());
    }
    let key = decodeConflictKey(&pair.Key, keyspace)?;
    let handle =
        astersql_tablecodec::DecodeIndexHandle(key.0, pair.Value.clone(), index.Columns.len())
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "index value does not contain a handle".to_owned())?;
    let mut crc = u32::MAX;
    for byte in handle.Encoded() {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    Ok((!crc % workers as u32) as usize)
}

/// Decode with the store's actual keyspace, rejecting mismatched or truncated prefixes.
pub fn decodeConflictKey(
    key: &astersql_kv::Key,
    keyspace: &[u8],
) -> Result<astersql_kv::Key, String> {
    if keyspace.is_empty() {
        return Ok(key.clone());
    }
    key.0
        .strip_prefix(keyspace)
        .map(|bytes| astersql_kv::Key(bytes.to_vec()))
        .ok_or_else(|| "conflict key does not belong to store keyspace".to_owned())
}

/// Dispatch bounded MVI batches with cancellation while receiving and sending.
/// Owning the senders ensures all output channels close on success or error.
pub fn dispatchMVIndexKVPairs(
    context: &astersql_dxf_importinto_conflictedkv::ConflictContext,
    input: &std::sync::mpsc::Receiver<astersql_dxf_importinto_conflictedkv::ConflictKVPair>,
    outputs: Vec<std::sync::mpsc::SyncSender<astersql_dxf_importinto_conflictedkv::ConflictKVPair>>,
    index: &astersql_meta_model::IndexInfo,
    keyspace: &[u8],
) -> Result<(), String> {
    use std::sync::mpsc::{RecvTimeoutError, TrySendError};
    use std::time::Duration;
    loop {
        if context.IsCancelled() {
            return Err("conflict dispatch cancelled".to_owned());
        }
        let mut pair = match input.recv_timeout(Duration::from_millis(10)) {
            Ok(pair) => pair,
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => continue,
        };
        let worker = mvIndexWorker(&pair, index, outputs.len(), keyspace)?;
        loop {
            if context.IsCancelled() {
                return Err("conflict dispatch cancelled".to_owned());
            }
            match outputs[worker].try_send(pair) {
                Ok(()) => break,
                Err(TrySendError::Full(unsent)) => {
                    pair = unsent;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(TrySendError::Disconnected(_)) => {
                    return Err("conflict handler stopped".to_owned());
                }
            }
        }
    }
}

/// Collect one real SST group through the existing collector/codec boundary.
/// All worker codecs finish initialization before the SST reader starts.
#[allow(clippy::too_many_arguments)]
pub fn CollectConflictGroup<F>(
    context: &astersql_dxf_importinto_conflictedkv::ConflictContext,
    input_store: Arc<dyn astersql_ingestor_globalsort::Storage>,
    output_store: Arc<dyn astersql_objstore_storeapi::Storage>,
    cluster: Arc<dyn astersql_dxf_importinto_conflictedkv::ConflictStore>,
    table: Arc<astersql_meta_model::TableInfo>,
    group: &str,
    info: &ConflictInfo,
    concurrency: usize,
    codec_factory: &F,
    filename_prefix: &str,
    global_set: Arc<BoundedKeySet>,
    shared_size: Arc<AtomicI64>,
    size_limit: i64,
    shared_file_size: Arc<AtomicI64>,
    progress_collector: Option<Arc<dyn Collector + Send + Sync>>,
    traffic_recorder: Option<Arc<dyn astersql_dxf_importinto_conflictedkv::TrafficRecorder>>,
) -> Result<(CollectResult, BoundedKeySet), String>
where
    F: Fn() -> Result<Box<dyn astersql_dxf_importinto_conflictedkv::ConflictRowCodec>, String>
        + Sync,
{
    use astersql_dxf_importinto_conflictedkv::{ConflictKVPair, NewCollector};
    use astersql_ingestor_globalsort::reader::{CancellationToken, ReadKVFilesAsync};
    let target_index = getKVGroupIndexInfo(&table, group)?;
    if concurrency == 0 {
        return Err("conflict group requires a worker".to_owned());
    }
    let cancellation = CancellationToken::default();
    std::thread::scope(|scope| {
        let mut outputs = Vec::new();
        let mut workers = Vec::new();
        let (ready_sender, ready_receiver) = std::sync::mpsc::channel();
        for _ in 0..concurrency {
            let (sender, receiver) = std::sync::mpsc::sync_channel(
                astersql_dxf_importinto_conflictedkv::BufferedHandleLimit
                    .load(Ordering::Acquire)
                    .max(1),
            );
            outputs.push(sender);
            let table = table.clone();
            let output_store = output_store.clone();
            let cluster = cluster.clone();
            let global = global_set.clone();
            let shared_size = shared_size.clone();
            let shared_files = shared_file_size.clone();
            let prefix = format!("{filename_prefix}/{}", uuid::Uuid::new_v4());
            let progress = progress_collector.clone();
            let traffic = traffic_recorder.clone();
            let ready = ready_sender.clone();
            workers.push(scope.spawn(move || {
                let codec = match codec_factory() {
                    Ok(mut codec) => {
                        codec.ConfigureKeyspace(cluster.Keyspace());
                        let _ = ready.send(Ok(()));
                        codec
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error.clone()));
                        return Err(error);
                    }
                };
                drop(ready);
                let local = NewBoundedKeySet(shared_size.clone(), size_limit);
                let mut collector = NewCollector(
                    table,
                    output_store,
                    cluster,
                    prefix,
                    group,
                    codec,
                    global,
                    local,
                    Some(shared_files),
                    progress.map(|collector| -> Arc<dyn Collector> { collector }),
                    traffic,
                );
                let result = collector.Run(context, &receiver);
                let closed = collector.Close(context);
                result.and(closed)?;
                let mut local = NewBoundedKeySet(shared_size, size_limit);
                collector.MergeRowKeysInto(&mut local);
                Ok::<_, String>((collector.GetCollectResult().clone(), local))
            }));
        }
        drop(ready_sender);
        let mut failure = None;
        for _ in 0..concurrency {
            match ready_receiver.recv() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    failure.get_or_insert("conflict collector initialization stopped".to_owned());
                }
            }
        }
        if failure.is_none() {
            for (ordinal, pair) in
                ReadKVFilesAsync(cancellation.clone(), input_store, info.Files.clone()).enumerate()
            {
                if context.IsCancelled() {
                    failure = Some("conflict collection cancelled".to_owned());
                    break;
                }
                let pair = match pair {
                    Ok(pair) => ConflictKVPair {
                        Key: astersql_kv::Key(pair.key),
                        Value: pair.value,
                    },
                    Err(error) => {
                        failure = Some(error.to_string());
                        break;
                    }
                };
                let worker = match conflictWorkerForPair(
                    &pair,
                    target_index.as_ref(),
                    outputs.len(),
                    ordinal,
                    &cluster.Keyspace(),
                ) {
                    Ok(worker) => worker,
                    Err(error) => {
                        failure = Some(error);
                        break;
                    }
                };
                if let Err(error) = sendConflictPair(context, &outputs[worker], pair) {
                    failure = Some(error);
                    break;
                }
            }
        }
        if failure.is_some() {
            cancellation.cancel();
        }
        drop(outputs);
        let mut result = NewCollectResult(&cluster.Keyspace());
        let mut local = NewBoundedKeySet(shared_size, size_limit);
        for worker in workers {
            match worker.join() {
                Ok(Ok((collected, keys))) => {
                    result.Merge(Some(&collected));
                    local.Merge(Some(&keys));
                }
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    failure.get_or_insert("conflict collector panicked".to_owned());
                }
            }
        }
        failure.map_or(Ok((result, local)), Err)
    })
}

pub(crate) fn sendConflictPair(
    context: &astersql_dxf_importinto_conflictedkv::ConflictContext,
    output: &std::sync::mpsc::SyncSender<astersql_dxf_importinto_conflictedkv::ConflictKVPair>,
    mut pair: astersql_dxf_importinto_conflictedkv::ConflictKVPair,
) -> Result<(), String> {
    loop {
        if context.IsCancelled() {
            return Err("conflict dispatch cancelled".to_owned());
        }
        match output.try_send(pair) {
            Ok(()) => return Ok(()),
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                return Err("conflict handler stopped".to_owned());
            }
            Err(std::sync::mpsc::TrySendError::Full(unsent)) => {
                pair = unsent;
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
        }
    }
}

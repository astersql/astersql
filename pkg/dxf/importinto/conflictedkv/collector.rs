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

// 冲突行收集器：把冲突编码行写入对象存储，并汇总 checksum。
//
// 在 collect-conflicts 步骤中，按 data / 唯一索引 KV 组选择 DataKVHandler 或
// IndexKVHandler；索引 Handler 在成功处理后把完整行键记入有界集合以便去重。写出冲突行时受单文件
// 大小与跨 collector 共享的总大小上限约束，超限则标记 `RowRecordingCapped`。
// 共享总大小先记账后判断，保持多 worker 并发下与 Go 一致的截断语义。

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use astersql_dxf_framework_taskexecutor_execute::Collector as ProgressCollector;
use astersql_kv::Key;
use astersql_lightning_backend_kv::Pairs;
use astersql_lightning_verification::{KVChecksum, NewKVChecksumWithKeyspace};
use astersql_meta_model::TableInfo;
use astersql_objstore_objectio::Writer;
use astersql_objstore_storeapi::{Storage, WriterOption};
use astersql_types::datum::{Datum, DatumsToString};

use crate::{
    BoundedKeySet, ConflictContext, ConflictKVPair, ConflictRowCodec, ConflictStore, DataKVGroup,
    EncodedRowHandler, Handler, NewBaseHandler, NewDataKVHandler, NewIndexKVHandler, NewKeyFilter,
    NewLazyRefreshedSnapshot, TrafficRecorder,
};

/// 单个冲突行文件的最大字节数（默认 8GiB）。
pub static MaxConflictRowFileSize: AtomicI64 = AtomicI64::new(8_i64 << 30);
/// 所有冲突行文件合计大小上限（默认 1GiB）；跨多个 collector 共享计数。
static maxTotalConflictRowFileSize: AtomicI64 = AtomicI64::new(1_i64 << 30);
/// 对象存储分片上传的最小 part 大小（5MiB）。
const minUploadPartSize: i64 = 5 * 1024 * 1024;

/// 一次收集运行的汇总结果：行数、文件大小、截断标记、checksum 与文件名列表。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CollectResult {
    pub RowCount: i64,
    pub TotalFileSize: i64,
    /// 因总大小超限而停止继续写冲突行文件。
    pub RowRecordingCapped: bool,
    pub Checksum: KVChecksum,
    pub Filenames: Vec<String>,
}

/// 按 keyspace（键空间）初始化空的收集结果与 checksum。
pub fn NewCollectResult(keyspace: &[u8]) -> CollectResult {
    CollectResult {
        RowCount: 0,
        TotalFileSize: 0,
        RowRecordingCapped: false,
        Checksum: NewKVChecksumWithKeyspace(keyspace),
        Filenames: Vec::with_capacity(1),
    }
}

impl CollectResult {
    /// 合并另一份结果；`None` 视为空操作。截断标记按或运算保留。
    pub fn Merge(&mut self, other: Option<&CollectResult>) {
        let Some(other) = other else {
            return;
        };
        self.RowCount += other.RowCount;
        self.TotalFileSize += other.TotalFileSize;
        self.RowRecordingCapped |= other.RowRecordingCapped;
        self.Checksum.Add(&other.Checksum);
        self.Filenames.extend(other.Filenames.iter().cloned());
    }
}

/// 冲突行收集器：驱动 Handler 解码冲突 KV，写出文本行并更新 CollectResult。
pub struct ConflictCollector {
    store: Arc<dyn Storage>,
    filename_prefix: String,
    handler: Option<Box<dyn Handler>>,
    result: CollectResult,
    handle_set: Arc<Mutex<BoundedKeySet>>,
    shared_total_file_size: Arc<AtomicI64>,
    file_sequence: usize,
    current_file_size: i64,
    writer: Option<Box<dyn Writer>>,
    /// 总大小超限后为 true，后续行仍计数但不写文件。
    stop_recording: bool,
}

/// 构造收集器：按 `kv_group` 选择 data 或 index Handler；索引组挂全局行键过滤。
#[allow(clippy::too_many_arguments)]
pub fn NewCollector(
    target_table: Arc<TableInfo>,
    object_store: Arc<dyn Storage>,
    cluster_store: Arc<dyn ConflictStore>,
    filename_prefix: impl Into<String>,
    kv_group: impl Into<String>,
    codec: Box<dyn ConflictRowCodec>,
    global_set: Arc<BoundedKeySet>,
    local_set: BoundedKeySet,
    shared_total_file_size: Option<Arc<AtomicI64>>,
    progress_collector: Option<Arc<dyn ProgressCollector>>,
    traffic_recorder: Option<Arc<dyn TrafficRecorder>>,
) -> ConflictCollector {
    let kv_group = kv_group.into();
    let shared_total_file_size =
        shared_total_file_size.unwrap_or_else(|| Arc::new(AtomicI64::new(0)));
    let local_set = Arc::new(Mutex::new(local_set));
    let base = NewBaseHandler(target_table, kv_group.clone(), codec, progress_collector);
    // data 组直接解码行；index 组需快照回查行，并用全局行键集合跳过已处理行。
    let handler: Box<dyn Handler> = if kv_group == DataKVGroup {
        Box::new(NewDataKVHandler(base))
    } else {
        Box::new(NewIndexKVHandler(
            base,
            NewLazyRefreshedSnapshot(cluster_store.clone(), traffic_recorder),
            Some(NewKeyFilter(global_set, local_set.clone())),
        ))
    };
    ConflictCollector {
        store: object_store,
        filename_prefix: filename_prefix.into(),
        handler: Some(handler),
        result: NewCollectResult(&cluster_store.Keyspace()),
        handle_set: local_set,
        shared_total_file_size,
        file_sequence: 0,
        current_file_size: 0,
        writer: None,
        stop_recording: false,
    }
}

impl ConflictCollector {
    /// 取出 Handler 跑完整条冲突 KV 通道，结束后归还 Handler。
    pub fn Run(
        &mut self,
        context: &ConflictContext,
        pairs: &mpsc::Receiver<ConflictKVPair>,
    ) -> Result<(), String> {
        let mut handler = self
            .handler
            .take()
            .ok_or_else(|| "collector handler is already running".to_owned())?;
        let result = handler
            .PreRun()
            .and_then(|_| handler.Run(context, pairs, self));
        self.handler = Some(handler);
        result
    }

    /// 将一行 Datum 序列化为文本并追加到当前冲突行文件；超限则截断记录。
    fn recordRowToFile(&mut self, context: &ConflictContext, row: &[Datum]) -> Result<(), String> {
        if self.stop_recording {
            return Ok(());
        }
        let row = DatumsToString(row, true).map_err(|error| error.to_string())?;
        let content_size = row.len() as i64 + 1;
        // 先原子累加共享总大小，再与上限比较，保证多 collector 并发时一致。
        let total = self
            .shared_total_file_size
            .fetch_add(content_size, Ordering::AcqRel)
            + content_size;
        let limit = maxTotalConflictRowFileSize.load(Ordering::Acquire);
        if total > limit {
            return self.onTotalSizeLimitExceeded(context);
        }
        // 无 writer 或当前文件已达单文件上限时切换新文件。
        if self.writer.is_none()
            || self.current_file_size >= MaxConflictRowFileSize.load(Ordering::Acquire)
        {
            self.switchFile(context)?;
            self.current_file_size = 0;
        }
        let content = format!("{row}\n");
        self.writer
            .as_mut()
            .expect("writer initialized")
            .Write(&context.ObjectIO, content.as_bytes())
            .map_err(|error| error.to_string())?;
        self.result.TotalFileSize += content_size;
        self.current_file_size += content_size;
        Ok(())
    }

    /// 关闭当前 writer（若有），打开下一个序号的冲突行文件。
    fn switchFile(&mut self, context: &ConflictContext) -> Result<(), String> {
        if let Some(mut writer) = self.writer.take() {
            writer
                .Close(&context.ObjectIO)
                .map_err(|error| error.to_string())?;
        }
        self.file_sequence += 1;
        let filename = getRowFileName(&self.filename_prefix, self.file_sequence);
        let option = WriterOption {
            Concurrency: 20,
            PartSize: minUploadPartSize,
        };
        let writer = self
            .store
            .Create(&context.ObjectIO, &filename, Some(&option))
            .map_err(|error| error.to_string())?;
        self.result.Filenames.push(filename);
        self.writer = Some(writer);
        Ok(())
    }

    /// 总大小超限：停止后续写文件，关闭当前 writer，并标记结果已截断。
    fn onTotalSizeLimitExceeded(&mut self, context: &ConflictContext) -> Result<(), String> {
        if self.stop_recording {
            return Ok(());
        }
        self.stop_recording = true;
        self.result.RowRecordingCapped = true;
        if let Some(mut writer) = self.writer.take() {
            self.current_file_size = 0;
            writer
                .Close(&context.ObjectIO)
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    /// 关闭 Handler 与未关闭的 writer；两者错误用 `and` 合并返回。
    pub fn Close(&mut self, context: &ConflictContext) -> Result<(), String> {
        let handler_result = if let Some(mut handler) = self.handler.take() {
            let result = handler.Close(context, self);
            self.handler = Some(handler);
            result
        } else {
            Ok(())
        };
        let writer_result = if let Some(mut writer) = self.writer.take() {
            writer
                .Close(&context.ObjectIO)
                .map_err(|error| error.to_string())
        } else {
            Ok(())
        };
        handler_result.and(writer_result)
    }

    /// 返回当前收集结果的不可变引用。
    pub fn GetCollectResult(&self) -> &CollectResult {
        &self.result
    }
}

impl EncodedRowHandler for ConflictCollector {
    /// 写出冲突行并更新行数与 checksum；过滤登记由 Handler 在成功后完成。
    fn HandleEncodedRow(
        &mut self,
        context: &ConflictContext,
        _row_key: &Key,
        row: &[Datum],
        pairs: &Pairs,
    ) -> Result<(), String> {
        self.recordRowToFile(context, row)?;
        self.result.RowCount += 1;
        self.result.Checksum.Update(&pairs.Pairs);
        Ok(())
    }
}

/// 按前缀与序号生成冲突行文件名，形如 `{prefix}/data-0001.txt`。
pub fn getRowFileName(prefix: &str, sequence: usize) -> String {
    let absolute = prefix.starts_with('/');
    let filename = format!("data-{sequence:04}.txt");
    let mut components: Vec<&str> = Vec::new();
    for component in prefix.split('/').chain(std::iter::once(filename.as_str())) {
        match component {
            "" | "." => {}
            ".." if components.last().is_some_and(|last| *last != "..") => {
                components.pop();
            }
            ".." if absolute => {}
            _ => components.push(component),
        }
    }
    let path = components.join("/");
    if absolute { format!("/{path}") } else { path }
}

/// 测试用：覆盖全局总文件大小上限。
#[cfg(test)]
pub fn SetMaxTotalConflictRowFileSizeForTest(limit: i64) {
    maxTotalConflictRowFileSize.store(limit, Ordering::Release);
}

impl ConflictCollector {
    /// Number of successfully processed index row keys in this worker.
    #[cfg(test)]
    pub fn RowKeySetLenForTest(&self) -> usize {
        self.handle_set.lock().unwrap().Len()
    }
}

impl ConflictCollector {
    /// Merge successful index row keys after every worker finishes the KV group.
    pub fn MergeRowKeysInto(&self, set: &mut BoundedKeySet) {
        set.Merge(Some(&self.handle_set.lock().unwrap()));
    }
}

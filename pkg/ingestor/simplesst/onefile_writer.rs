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

// 单文件 SST（Sorted String Table）写入器。
//
// `OneFileWriter` 将有序 KV 写入一对数据/统计文件；重复键可按 Ignore/Remove/Record/Error
// 策略处理，Record 模式下多余冲突行写入 `_dup` 文件。

/// 单个 writer 的默认内存上限；Go 用它减少逐 KV 重复分配。
// pub const DefaultOneWriterMemSizeLimit: u64 = 128 * 1024 * 1024;
/// 当前实现把内存上限同时作为默认块大小。
// pub const DefaultOneWriterBlockSize: usize = DefaultOneWriterMemSizeLimit as usize;
/// 计算上传分片时使用较保守的除数，确保总分片数远低于 S3 的 10000 限制。
// pub const MaxUploadPartCount: usize = 5000;
// const logPartNumInterval: u64 = 999;
//
/// OneFileWriter 对应 Go 同名结构：数据和统计各写一个文件，重复键可另写冲突文件。
// pub struct OneFileWriter {
// 对象存储和编码缓冲区由调用方注入，不自行建立外部连接。
//     store: Storage,
//     kvStore: Option<KeyValueStore>,
//     kvBuffer: Buffer,
//
// 每个 writer 的累计统计。
//     totalSize: u64,
//     totalCnt: u64,
//     rc: RangePropertiesCollector,
//
// 文件路径和底层 writer 延迟初始化，避免 remove 模式产生空文件。
//     writerID: String,
//     filenamePrefix: String,
//     rnd: Rand,
//     dataFile: String,
//     statFile: String,
//     dataWriter: Option<ObjectWriter>,
//     statWriter: Option<ObjectWriter>,
//
//     onClose: OnWriterCloseFunc,
//     closed: bool,
//
// 重复键检测以当前 pivot 为一组；输入必须保持按 key 排序。
//     onDup: OnDuplicateKey,
//     pivotKey: Option<Vec<u8>>,
//     pivotValue: Option<Vec<u8>>,
//     currDupCnt: usize,
//     recordedDupCnt: usize,
//     dupFile: String,
//     dupWriter: Option<ObjectWriter>,
//     dupKVStore: Option<KeyValueStore>,
//
//     minKey: Option<Vec<u8>>,
//     maxKey: Option<Vec<u8>>,
//     logger: Logger,
//     partSize: i64,
//     writtenBytes: i64,
//     lastLogWriteSize: u64,
// }
//
// impl OneFileWriter {
/// lazyInitWriter 对应 Go 的数据/统计 writer 延迟初始化。
//     fn lazyInitWriter(&mut self, ctx: &Context) -> Result<(), Error> {
//         if self.dataWriter.is_some() {
//             return Ok(());
//         }
//
//         let prefix = self.getPartitionedPrefix();
//         let dataFile = joinPath(&prefix, "one-file");
//         let mut dataWriter = self.store.Create(
//             ctx,
//             &dataFile,
//             WriterOption { Concurrency: maxUploadWorkersPerThread, PartSize: self.partSize },
//         )?;
//
//         let statFile = joinPath(&(prefix + statSuffix), "one-file");
//         let statWriter = match self.store.Create(
//             ctx,
//             &statFile,
//             WriterOption { Concurrency: maxUploadWorkersPerThread, PartSize: MinUploadPartSize },
//         ) {
//             Ok(writer) => writer,
//             Err(err) => {
// Go 在统计 writer 创建失败时主动关闭已创建的数据 writer，防止泄漏半成品上传。
//                 self.logger.Info("create stat writer failed", &err);
//                 let _ = dataWriter.Close(ctx);
//                 return Err(err);
//             }
//         };
//
//         self.logger.InfoFiles("one file writer", &dataFile, &statFile, self.onDup);
//         self.dataFile = dataFile;
//         self.statFile = statFile;
//         self.dataWriter = Some(dataWriter);
//         self.statWriter = Some(statWriter);
//         self.kvStore = Some(NewKeyValueStore(ctx, self.dataWriter.as_mut().unwrap(), Some(&mut self.rc)));
//         Ok(())
//     }
//
/// lazyInitDupFile 对应 Go 冲突文件初始化，仅 record 模式出现第三个同键值时调用。
//     fn lazyInitDupFile(&mut self, ctx: &Context) -> Result<(), Error> {
//         if self.dupWriter.is_some() {
//             return Ok(());
//         }
//
//         let dupFile = joinPath(&(self.getPartitionedPrefix() + dupSuffix), "one-file");
// 重复键通常很少，Go 固定并发度为 1，以控制冲突处理阶段的内存和分片数量。
//         let dupWriter = self.store.Create(
//             ctx,
//             &dupFile,
//             WriterOption { Concurrency: 1, PartSize: self.partSize },
//         ).map_err(|err| {
//             self.logger.Info("create dup writer failed", &err);
//             err
//         })?;
//         self.dupFile = dupFile;
//         self.dupWriter = Some(dupWriter);
//         self.dupKVStore = Some(NewKeyValueStore(ctx, self.dupWriter.as_mut().unwrap(), None));
//         Ok(())
//     }
//
/// InitPartSizeAndLogger 对应 Go 初始化方法，从 context 派生日志器并记录上传分片大小。
//     pub fn InitPartSizeAndLogger(&mut self, ctx: &Context, partSize: i64) {
//         self.logger = Logger::from_context(ctx);
//         self.partSize = partSize;
//     }
//
/// WriteRow 实现 ingest.Writer：先按策略处理重复键，再把有效 KV 编码写入数据流。
//     pub fn WriteRow(&mut self, ctx: &Context, idxKey: &[u8], idxVal: &[u8]) -> Result<(), Error> {
//         let result = if self.onDup != OnDuplicateKey::Ignore {
//             self.handleDupAndWrite(ctx, idxKey, idxVal)
//         } else {
//             self.doWriteRow(ctx, idxKey, idxVal)
//         };
//
// 对应 Go defer：无论写入成功与否，跨过 999 个估算分片时都报告一次进度。
//         if self.partSize > 0
//             && (self.totalSize - self.lastLogWriteSize) / self.partSize as u64 >= logPartNumInterval
//         {
//             self.logger.Progress(&self.writerID, self.partSize, self.totalSize);
//             self.lastLogWriteSize = self.totalSize;
//         }
//         result
//     }
//
/// handleDupAndWrite 按连续相同 key 的 pivot 计数执行 record、error 或 remove 策略。
//     fn handleDupAndWrite(&mut self, ctx: &Context, idxKey: &[u8], idxVal: &[u8]) -> Result<(), Error> {
//         if self.currDupCnt == 0 || self.pivotKey.as_deref() != Some(idxKey) {
//             return self.onNextPivot(ctx, Some(idxKey), Some(idxVal));
//         }
//
//         self.currDupCnt += 1;
//         match self.onDup {
//             OnDuplicateKey::Record => {
//                 if self.currDupCnt == 2 {
// 每组前两个重复项写入主数据文件，供后续阶段定位冲突。
//                     let key = self.pivotKey.clone().unwrap();
//                     let value = self.pivotValue.clone().unwrap();
//                     self.doWriteRow(ctx, &key, &value)?;
//                     self.doWriteRow(ctx, idxKey, idxVal)?;
//                 } else {
// 第三个及后续项进入单独 dup 文件，并累计 ConflictInfo.Count。
//                     self.lazyInitDupFile(ctx)?;
//                     self.dupKVStore.as_mut().unwrap().AddRawKV(idxKey, idxVal)?;
//                     self.recordedDupCnt += 1;
//                 }
//             }
//             OnDuplicateKey::Error => return Err(Error::found_duplicate_keys(idxKey, idxVal)),
// Remove 是 Go switch 的 default：整组重复键都不写主文件。
//             OnDuplicateKey::Remove | OnDuplicateKey::Ignore => {}
//         }
//         Ok(())
//     }
//
/// onNextPivot 结束上一组 key 并缓存新 pivot；None 表示 close 阶段刷新最后一组。
//     fn onNextPivot(
//         &mut self,
//         ctx: &Context,
//         idxKey: Option<&[u8]>,
//         idxVal: Option<&[u8]>,
//     ) -> Result<(), Error> {
//         if self.currDupCnt == 1 {
// 上一 pivot 没有重复，延迟到此处才真正写入。
//             let key = self.pivotKey.clone().unwrap();
//             let value = self.pivotValue.clone().unwrap();
//             self.doWriteRow(ctx, &key, &value)?;
//         }
//         if let Some(key) = idxKey {
//             self.pivotKey = Some(key.to_vec());
//             self.pivotValue = Some(idxVal.unwrap_or_default().to_vec());
//             self.currDupCnt = 1;
//         } else {
//             self.pivotKey = None;
//             self.pivotValue = None;
//             self.currDupCnt = 0;
//         }
//         Ok(())
//     }
//
/// handlePivotOnClose 对应 Go 的关闭前哨调用，用空 pivot 刷新最后一个唯一键。
//     fn handlePivotOnClose(&mut self, ctx: &Context) -> Result<(), Error> {
//         self.onNextPivot(ctx, None, None)
//     }
//
/// doWriteRow 编码一条 KV，并在内存块耗尽时落盘当前范围统计。
//     fn doWriteRow(&mut self, ctx: &Context, idxKey: &[u8], idxVal: &[u8]) -> Result<(), Error> {
//         if self.minKey.is_none() {
//             self.minKey = Some(idxKey.to_vec());
//         }
//         self.lazyInitWriter(ctx)?;
//
//         let keyLen = idxKey.len();
//         let length = keyLen + idxVal.len() + LengthBytes * 2;
//         let mut buf = self.kvBuffer.AllocBytesWithSliceLocation(length);
//         if buf.is_none() {
//             self.kvBuffer.Reset();
//             buf = self.kvBuffer.AllocBytesWithSliceLocation(length);
//             if buf.is_none() {
// 与 Go 一样不支持大于 blockSize 的单条 KV，重置后仍失败则立即返回。
//                 return Err(Error::allocation_failed(length));
//             }
//
// 一个 kvBuffer 消耗完后结束当前块，写统计并让下一属性的 offset 对齐 kvStore。
//             let kvStore = self.kvStore.as_mut().unwrap();
//             kvStore.Finish();
//             let encodedStat = self.rc.Encode();
//             self.statWriter.as_mut().unwrap().Write(ctx, &encodedStat)?;
//             self.rc.Reset();
//             self.rc.currProp.Offset = kvStore.offset;
//         }
//
//         let buf = buf.unwrap();
//         encodeToBuf(buf, idxKey, idxVal);
//         self.maxKey = Some(buf[LengthBytes * 2..LengthBytes * 2 + keyLen].to_vec());
//         self.kvStore.as_mut().unwrap().addEncodedData(&buf[..length])?;
//         self.totalCnt += 1;
//         self.totalSize += (keyLen + idxVal.len()) as u64;
//         self.writtenBytes += length as i64;
//         if self.writtenBytes >= 16 * 1024 * 1024 {
// Go 每累计 16 MiB 上报一次 MergeSortWriteBytes，随后清零局部计数。
//             MergeSortWriteBytes::Add(self.writtenBytes as f64);
//             self.writtenBytes = 0;
//         }
//         Ok(())
//     }
//
/// Close 对应 Go 公共关闭入口，生成汇总并保证回调只执行一次。
//     pub fn Close(&mut self, ctx: &Context) -> Result<(), Error> {
//         if self.closed {
//             return Err(Error::writer_closed(&self.writerID));
//         }
//         self.closeImpl(ctx)?;
//         self.logger.CloseSummary(&self.writerID, self.totalCnt, self.totalSize, self.recordedDupCnt);
//
//         let mut minKey = None;
//         let mut maxKey = None;
//         let mut multipleFilesStats = Vec::with_capacity(1);
//         if self.totalCnt > 0 {
// 全部 KV 都因重复而移除时 totalCnt 为零，此时不会发布空文件范围。
//             minKey = self.minKey.clone();
//             maxKey = self.maxKey.clone();
//             let mut stat = MultipleFilesStat::default();
//             stat.Filenames.push([self.dataFile.clone(), self.statFile.clone()]);
//             stat.Build(&[minKey.clone().unwrap()], &[maxKey.clone().unwrap()]);
//             multipleFilesStats.push(stat);
//         }
//
//         let conflictInfo = if self.recordedDupCnt > 0 {
//             ConflictInfo { Count: self.recordedDupCnt as u64, Files: vec![self.dupFile.clone()] }
//         } else {
//             ConflictInfo::default()
//         };
//         (self.onClose)(WriterSummary {
//             WriterID: self.writerID.clone(), Seq: 0, Min: minKey, Max: maxKey,
//             TotalSize: self.totalSize, TotalCnt: self.totalCnt,
//             KVFileCount: 1, MultipleFilesStats: multipleFilesStats, ConflictInfo: conflictInfo,
//         });
//         self.totalCnt = 0;
//         self.totalSize = 0;
//         self.closed = true;
//         Ok(())
//     }
//
/// closeImpl 严格保留 Go 的关闭顺序：pivot、剩余统计、数据、统计、冲突文件。
//     fn closeImpl(&mut self, ctx: &Context) -> Result<(), Error> {
//         self.handlePivotOnClose(ctx)?;
//         if self.dataWriter.is_some() {
//             self.kvStore.as_mut().unwrap().Finish();
//             let encodedStat = self.rc.Encode();
//             self.statWriter.as_mut().unwrap().Write(ctx, &encodedStat)?;
//             self.rc.Reset();
//
// 任一步关闭失败都记录并立即返回，避免掩盖最先出现的 IO 错误。
//             self.dataWriter.as_mut().unwrap().Close(ctx).map_err(|err| {
//                 self.logger.Error("Close data writer failed", &err);
//                 err
//             })?;
//             self.statWriter.as_mut().unwrap().Close(ctx).map_err(|err| {
//                 self.logger.Error("Close stat writer failed", &err);
//                 err
//             })?;
//         }
//         if let Some(writer) = self.dupWriter.as_mut() {
//             self.dupKVStore.as_mut().unwrap().Finish();
//             writer.Close(ctx).map_err(|err| {
//                 self.logger.Error("Close dup writer failed", &err);
//                 err
//             })?;
//         }
//         Ok(())
//     }
//
/// getPartitionedPrefix 对应 Go 方法，使用 writer 自有随机源分散对象前缀。
//     fn getPartitionedPrefix(&mut self) -> String {
//         randPartitionedPrefix(&self.filenamePrefix, &mut self.rnd)
//     }
// }
//
/// encodeToBuf 按两个大端 uint64 长度头及 key/value 正文编码一条记录。
// fn encodeToBuf(buf: &mut [u8], key: &[u8], value: &[u8]) {
//     assert_eq!(buf.len(), LengthBytes * 2 + key.len() + value.len());
//     buf[..LengthBytes].copy_from_slice(&(key.len() as u64).to_be_bytes());
//     buf[LengthBytes..LengthBytes * 2].copy_from_slice(&(value.len() as u64).to_be_bytes());
//     buf[LengthBytes * 2..LengthBytes * 2 + key.len()].copy_from_slice(key);
//     buf[LengthBytes * 2 + key.len()..].copy_from_slice(value);
// }
// */
use crate::file::{DUP_SUFFIX, KeyValueStore, STAT_SUFFIX};
use crate::writer::{
    CloseCallback, ConflictInfo, DuplicateMode, MinUploadPartSize, MultipleFilesStat,
    RangePropertiesCollector, WriterBuilder, WriterSummary, join_path, rand_partitioned_prefix,
};
use crate::{Error, MemoryStorage, Result};

/// 单个 writer 的默认内存上限（128 MiB），对应 Go DefaultOneWriterMemSizeLimit。
pub const DefaultOneWriterMemSizeLimit: u64 = 128 * 1024 * 1024;
/// 当前实现把内存上限同时作为默认块大小。
pub const DefaultOneWriterBlockSize: usize = DefaultOneWriterMemSizeLimit as usize;
/// 计算上传分片时使用较保守的除数，确保总分片数远低于 S3 的 10000 限制。
pub const MaxUploadPartCount: usize = 5000;

/// 单文件写入器：数据与统计各写一个对象，重复键可另写冲突文件。
pub struct OneFileWriter {
    storage: MemoryStorage,
    writer_id: String,
    filename_prefix: String,
    random_state: u64,
    memory_limit: u64,
    property_size: u64,
    property_keys: u64,
    on_duplicate: DuplicateMode,
    on_close: CloseCallback,
    group_offset: i32,
    /// 当前连续同键组；输入必须按 key 有序。
    pivot: Option<Vec<(Vec<u8>, Vec<u8>)>>,
    data_rows: Vec<(Vec<u8>, Vec<u8>)>,
    duplicate_rows: Vec<(Vec<u8>, Vec<u8>)>,
    buffered_bytes: u64,
    part_size: i64,
    closed: bool,
}
impl OneFileWriter {
    /// 由 WriterBuilder 注入配置；随机种子来自文件名前缀哈希以保证分区可复现。
    pub(crate) fn new(
        storage: MemoryStorage,
        prefix: &str,
        writer_id: &str,
        builder: &WriterBuilder,
    ) -> Self {
        let (memory_limit, _, property_size, property_keys, on_duplicate, on_close, group_offset) =
            builder.configuration();
        let filename_prefix = join_path(prefix, writer_id);
        let random_state = crate::writer::get_hash(&filename_prefix);
        Self {
            storage,
            writer_id: writer_id.into(),
            filename_prefix,
            random_state,
            memory_limit,
            property_size,
            property_keys,
            on_duplicate,
            on_close,
            group_offset,
            pivot: None,
            data_rows: Vec::new(),
            duplicate_rows: Vec::new(),
            buffered_bytes: 0,
            part_size: MinUploadPartSize,
            closed: false,
        }
    }
    /// 设置对象存储上传分片大小；必须为正。
    pub fn init_part_size(&mut self, part_size: i64) -> Result<()> {
        if part_size <= 0 {
            return Err(Error::InvalidData("part size must be positive".into()));
        }
        self.part_size = part_size;
        Ok(())
    }
    /// 将一条 KV 记入主数据缓冲；超过 memory_limit 则拒绝。
    fn emit(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        let length = 16u64 + key.len() as u64 + value.len() as u64;
        if length > self.memory_limit {
            return Err(Error::InvalidData(format!(
                "key/value pair exceeds writer memory limit: {length}"
            )));
        }
        self.buffered_bytes = self.buffered_bytes.saturating_add(length);
        self.data_rows.push((key, value));
        Ok(())
    }
    /// 结束当前 pivot 组，按 DuplicateMode 决定写入主文件或冲突缓冲。
    fn finish_pivot(&mut self) -> Result<()> {
        let Some(group) = self.pivot.take() else {
            return Ok(());
        };
        match self.on_duplicate {
            DuplicateMode::Ignore => {
                for (key, value) in group {
                    self.emit(key, value)?;
                }
            }
            DuplicateMode::Remove => {
                // 仅唯一键写入；整组重复则全部丢弃。
                if group.len() == 1 {
                    let (key, value) = group.into_iter().next().unwrap();
                    self.emit(key, value)?;
                }
            }
            DuplicateMode::Record => {
                if group.len() == 1 {
                    let (key, value) = group.into_iter().next().unwrap();
                    self.emit(key, value)?;
                } else {
                    // 前两条写主文件供定位冲突，其余进入 dup 缓冲。
                    for (key, value) in group.iter().take(2) {
                        self.emit(key.clone(), value.clone())?;
                    }
                    self.duplicate_rows.extend(group.into_iter().skip(2));
                }
            }
            DuplicateMode::Error => {
                let mut values = group.into_iter();
                let (key, value) = values.next().unwrap();
                if values.next().is_some() {
                    // `write_row` already reported the duplicate when the second row arrived.
                    // Go retains only the duplicate count in this state, so advancing to the
                    // next pivot or closing drops the whole group without returning it again.
                    return Ok(());
                }
                self.emit(key, value)?;
            }
        }
        Ok(())
    }
    /// 写入一行；非 Ignore 时按连续同键聚合到 pivot。
    pub fn write_row(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        if self.on_duplicate == DuplicateMode::Ignore {
            return self.emit(key.to_vec(), value.to_vec());
        }
        match self.pivot.as_mut() {
            None => self.pivot = Some(vec![(key.to_vec(), value.to_vec())]),
            Some(group) if group[0].0.as_slice() == key => {
                group.push((key.to_vec(), value.to_vec()));
                if self.on_duplicate == DuplicateMode::Error {
                    return Err(Error::DuplicateKey {
                        key: key.to_vec(),
                        value: value.to_vec(),
                    });
                }
            }
            Some(_) => {
                self.finish_pivot()?;
                self.pivot = Some(vec![(key.to_vec(), value.to_vec())]);
            }
        }
        Ok(())
    }
    /// 刷新 pivot、落盘数据/统计/冲突文件，并触发关闭回调。
    pub fn close(&mut self) -> Result<WriterSummary> {
        if self.closed {
            return Err(Error::Closed);
        }
        self.finish_pivot()?;
        let mut summary = WriterSummary {
            WriterID: self.writer_id.clone(),
            GroupOffset: self.group_offset,
            ..WriterSummary::default()
        };
        if !self.data_rows.is_empty() {
            // 延迟创建对象：有有效 KV 才写入数据文件与统计文件。
            let data_path = join_path(
                &rand_partitioned_prefix(&self.filename_prefix, &mut self.random_state),
                "one-file",
            );
            let stat_path = join_path(
                &(rand_partitioned_prefix(&self.filename_prefix, &mut self.random_state)
                    + STAT_SUFFIX),
                "one-file",
            );
            let mut store = KeyValueStore::new(Some(RangePropertiesCollector::new(
                self.property_size,
                self.property_keys,
            )));
            for (k, v) in &self.data_rows {
                store.add_raw_kv(k, v)?;
            }
            let (data, collector) = store.into_parts();
            self.storage.write(data_path.clone(), data)?;
            self.storage
                .write(stat_path.clone(), collector.unwrap().encode()?)?;
            summary.Min = self.data_rows.first().unwrap().0.clone();
            summary.Max = self.data_rows.last().unwrap().0.clone();
            summary.TotalCnt = self.data_rows.len() as u64;
            summary.TotalSize = self
                .data_rows
                .iter()
                .map(|(k, v)| (k.len() + v.len()) as u64)
                .sum();
            summary.KVFileCount = 1;
            let mut stat = MultipleFilesStat {
                Filenames: vec![[data_path, stat_path]],
                ..MultipleFilesStat::default()
            };
            stat.build(&[summary.Min.clone()], &[summary.Max.clone()])?;
            summary.MultipleFilesStats.push(stat);
        }
        if !self.duplicate_rows.is_empty() {
            // Record 模式冲突行写入独立 `_dup` 对象。
            let prefix =
                rand_partitioned_prefix(&self.filename_prefix, &mut self.random_state) + DUP_SUFFIX;
            let path = join_path(&prefix, "one-file");
            let mut store = KeyValueStore::new(None);
            for (k, v) in &self.duplicate_rows {
                store.add_raw_kv(k, v)?;
            }
            let (data, _) = store.into_parts();
            self.storage.write(path.clone(), data)?;
            summary.ConflictInfo = ConflictInfo {
                Count: self.duplicate_rows.len() as u64,
                Files: vec![path],
            };
        }
        (self.on_close)(&summary);
        self.closed = true;
        Ok(summary)
    }
    /// Go 风格别名：`init_part_size`。
    pub fn InitPartSizeAndLogger(&mut self, part_size: i64) -> Result<()> {
        self.init_part_size(part_size)
    }
    /// Go 风格别名：`write_row`。
    pub fn WriteRow(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.write_row(key, value)
    }
    /// Go 风格别名：`close`。
    pub fn Close(&mut self) -> Result<WriterSummary> {
        self.close()
    }
}

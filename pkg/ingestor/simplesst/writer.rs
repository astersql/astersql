// Copyright 2026 AsterSQL.
/*
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


pub const MultiFileStatNum: usize = 500;
pub const DefaultPropSizeDist: u64 = 1024 * 1024;
pub const DefaultPropKeysDist: u64 = 8 * 1024;
const maxUploadWorkersPerThread: usize = 8;
const partitionHeader: &str = "p";
const partitionHeaderChar: u8 = b'p';
const maxMergeSortOverlapThreshold: i64 = 4000;
pub const MaxMergeSortFileCountStep: usize = 4000;
pub const MergeSortMaxSubtaskTargetFiles: usize = 16;
pub const MinUploadPartSize: i64 = 5 * 1024 * 1024;
pub const DefaultMemSizeLimit: u64 = 256 * 1024 * 1024;
pub const DefaultBlockSize: usize = 16 * 1024 * 1024;
const flushKVsRetryTimes: usize = 3;
const LengthBytes: usize = 8;

/// commonGetAdjustCount 根据并发度计算 merge-sort 阈值，并用全局上限截断。
fn commonGetAdjustCount(isOverlapThreshold: bool, mut concurrency: i32) -> i64 {
    debug_assert!(concurrency > 0, "concurrency must be greater than 0");
    if concurrency <= 0 {
        // Go 的 intest.Assert 在生产构建可能不生效，因此仍保留容错和错误日志。
        logInvalidConcurrency(concurrency);
        concurrency = 1;
    }
    let count = 250 * concurrency as i64;
    if isOverlapThreshold {
        count.min(maxMergeSortOverlapThreshold)
    } else {
        count.min(MaxMergeSortFileCountStep as i64)
    }
}

/// GetAdjustedMergeSortOverlapThreshold 在低并发、低可用内存机器上降低重叠统计阈值。
pub fn GetAdjustedMergeSortOverlapThreshold(concurrency: i32) -> i64 {
    commonGetAdjustCount(true, concurrency)
}

/// GetAdjustedMergeSortFileCountStep 返回按并发度调整后的文件数步长。
pub fn GetAdjustedMergeSortFileCountStep(concurrency: i32) -> usize {
    commonGetAdjustCount(false, concurrency) as usize
}

/// GetAdjustedBlockSize 对齐缓冲大小；对齐浪费超过 10% 时直接使用每 writer 可用大小，避免 OOM。
pub fn GetAdjustedBlockSize(totalBufSize: u64, defBlockSize: usize) -> usize {
    let alignedSize = GetAlignedSize(totalBufSize, defBlockSize as u64);
    if alignedSize as f64 / totalBufSize as f64 > 1.1 {
        totalBufSize as usize
    } else {
        defBlockSize
    }
}

/// RangeProperty 是统计文件中的范围属性；FirstKey/LastKey 均为闭区间端点。
#[derive(Clone, Default)]
pub struct RangeProperty {
    pub FirstKey: Vec<u8>,
    pub LastKey: Vec<u8>,
    pub Offset: u64,
    pub Keys: u64,
    pub Size: u64,
}

/// RangePropertiesCollector 按字节数或 key 数切分范围属性；构造后才可使用。
pub struct RangePropertiesCollector {
    props: Vec<RangeProperty>,
    pub currProp: RangeProperty,
    propSizeDist: u64,
    propKeysDist: u64,
}

impl RangePropertiesCollector {
    /// NewRangePropertiesCollector 对应 Go 构造函数，预留约 1024 条属性空间。
    pub fn NewRangePropertiesCollector(propSizeDist: u64, propKeysDist: u64) -> Self {
        Self {
            props: Vec::with_capacity(1024),
            currProp: RangeProperty::default(),
            propSizeDist,
            propKeysDist,
        }
    }

    /// CurrProp 暴露当前正在累计的属性，OneFileWriter 用它对齐新块 offset。
    pub fn CurrProp(&mut self) -> &mut RangeProperty {
        &mut self.currProp
    }

    /// onNextEncodedData 接收已编码 KV 和写入后的文件大小，更新当前范围。
    fn onNextEncodedData(&mut self, data: &[u8], size: u64) {
        let keyLen = u64::from_be_bytes(data[..LengthBytes].try_into().unwrap()) as usize;
        let key = &data[2 * LengthBytes..2 * LengthBytes + keyLen];
        if self.currProp.FirstKey.is_empty() {
            self.currProp.FirstKey = key.to_vec();
        }
        self.currProp.LastKey = key.to_vec();
        // 属性 Size 不包含两个长度头，与 Go 的数据大小口径一致。
        self.currProp.Size += (data.len() - 2 * LengthBytes) as u64;
        self.currProp.Keys += 1;

        if self.currProp.Size >= self.propSizeDist || self.currProp.Keys >= self.propKeysDist {
            self.props.push(self.currProp.clone());
            // 新属性从当前文件尾开始；LastKey 会在下一条 KV 到来时被覆盖。
            self.currProp.FirstKey.clear();
            self.currProp.Offset = size;
            self.currProp.Keys = 0;
            self.currProp.Size = 0;
        }
    }

    /// onFileEnd 把未达到阈值的最后一段属性加入结果。
    fn onFileEnd(&mut self) {
        if self.currProp.Keys > 0 {
            self.props.push(self.currProp.clone());
        }
    }

    /// Reset 清空已完成属性和当前累计状态，以便复用 collector。
    pub fn Reset(&mut self) {
        self.props.clear();
        self.currProp = RangeProperty::default();
    }

    /// Encode 对应 Go 的 encodeMultiProps，生成长度前缀的属性字节流。
    pub fn Encode(&self) -> Vec<u8> {
        encodeMultiProps(Vec::with_capacity(1024), &self.props)
    }
}

/// WriterSummary 是关闭回调看到的不可变汇总；重复策略会影响 TotalSize/TotalCnt。
pub struct WriterSummary {
    pub WriterID: String,
    pub GroupOffset: i32,
    pub Seq: usize,
    pub Min: Option<Key>,
    pub Max: Option<Key>,
    pub TotalSize: u64,
    pub TotalCnt: u64,
    pub KVFileCount: usize,
    pub MultipleFilesStats: Vec<MultipleFilesStat>,
    pub ConflictInfo: ConflictInfo,
}

pub type OnWriterCloseFunc = Box<dyn FnMut(WriterSummary)>;

/// dummyOnWriterCloseFunc 对应 Go nil 回调的安全替代，不执行任何业务动作。
fn dummyOnWriterCloseFunc(_: WriterSummary) {}

/// WriterBuilder 保存两种 writer 共用的缓冲、属性、codec、冲突策略配置。
pub struct WriterBuilder {
    groupOffset: i32,
    memSizeLimit: u64,
    blockSize: usize,
    propSizeDist: u64,
    propKeysDist: u64,
    onClose: OnWriterCloseFunc,
    tikvCodec: Option<Codec>,
    onDup: OnDuplicateKey,
}

impl WriterBuilder {
    /// NewWriterBuilder 建立与 Go 完全相同的默认配置。
    pub fn NewWriterBuilder() -> Self {
        Self {
            groupOffset: 0,
            memSizeLimit: DefaultMemSizeLimit,
            blockSize: DefaultBlockSize,
            propSizeDist: DefaultPropSizeDist,
            propKeysDist: DefaultPropKeysDist,
            onClose: Box::new(dummyOnWriterCloseFunc),
            tikvCodec: None,
            onDup: OnDuplicateKey::Ignore,
        }
    }

    pub fn SetMemorySizeLimit(&mut self, size: u64) -> &mut Self {
        self.memSizeLimit = size;
        self
    }

    pub fn SetPropSizeDistance(&mut self, dist: u64) -> &mut Self {
        self.propSizeDist = dist;
        self
    }

    pub fn SetPropKeysDistance(&mut self, dist: u64) -> &mut Self {
        self.propKeysDist = dist;
        self
    }

    /// SetOnCloseFunc 对应 Go setter；调用方未提供回调时继续使用 dummy。
    pub fn SetOnCloseFunc(&mut self, onClose: Option<OnWriterCloseFunc>) -> &mut Self {
        self.onClose = onClose.unwrap_or_else(|| Box::new(dummyOnWriterCloseFunc));
        self
    }

    pub fn SetBlockSize(&mut self, blockSize: usize) -> &mut Self {
        self.blockSize = blockSize;
        self
    }

    /// SetGroupOffset 用于多 schema change 时区分不同索引 writer 的汇总。
    pub fn SetGroupOffset(&mut self, offset: i32) -> &mut Self {
        self.groupOffset = offset;
        self
    }

    pub fn SetTiKVCodec(&mut self, codec: Option<Codec>) -> &mut Self {
        self.tikvCodec = codec;
        self
    }

    pub fn SetOnDup(&mut self, onDup: OnDuplicateKey) -> &mut Self {
        self.onDup = onDup;
        self
    }

    /// Build 创建多文件 Writer；文件位于 `{prefix}/{writerID}` 的随机分区前缀下。
    pub fn Build(&mut self, store: Storage, prefix: &str, writerID: &str) -> Writer {
        let filenamePrefix = joinPath(prefix, writerID);
        let pool = BufferPool::new(self.blockSize);
        Writer {
            store,
            writerID: writerID.to_owned(),
            groupOffset: self.groupOffset,
            currentSeq: 0,
            rnd: Rand::from_seed(getHash(&filenamePrefix)),
            filenamePrefix,
            rc: RangePropertiesCollector::NewRangePropertiesCollector(self.propSizeDist, self.propKeysDist),
            kvBuffer: pool.NewBuffer(self.memSizeLimit),
            kvLocations: Vec::new(), kvSize: 0, batchSize: 0,
            onClose: std::mem::replace(&mut self.onClose, Box::new(dummyOnWriterCloseFunc)),
            onDup: self.onDup, closed: false, multiFileStats: Vec::new(),
            fileMinKeys: Vec::with_capacity(MultiFileStatNum),
            fileMaxKeys: Vec::with_capacity(MultiFileStatNum),
            minKey: None, maxKey: None, totalSize: 0, totalCnt: 0, kvFileCount: 0,
            tikvCodec: self.tikvCodec.clone(), conflictInfo: ConflictInfo::default(),
        }
    }

    /// BuildOneFile 创建只含一个数据文件和一个统计文件的 writer。
    pub fn BuildOneFile(&mut self, store: Storage, prefix: &str, writerID: &str) -> OneFileWriter {
        let filenamePrefix = joinPath(prefix, writerID);
        let pool = BufferPool::new(self.blockSize);
        OneFileWriter::from_builder(
            store, filenamePrefix, writerID.to_owned(),
            pool.NewBuffer(self.memSizeLimit),
            RangePropertiesCollector::NewRangePropertiesCollector(self.propSizeDist, self.propKeysDist),
            std::mem::replace(&mut self.onClose, Box::new(dummyOnWriterCloseFunc)), self.onDup,
        )
    }
}

/// MultipleFilesStat 聚合最多 500 个文件，避免逐文件统计全部常驻内存。
#[derive(Default)]
pub struct MultipleFilesStat {
    pub MinKey: Option<Key>,
    pub MaxKey: Option<Key>,
    /// Filenames 按数据文件首 key 排序，每项为 `[dataFile, statFile]`。
    pub Filenames: Vec<[String; 2]>,
    pub MaxOverlappingNum: i64,
}

/// startKeysAndFiles 对应 Go 的 sort.Interface 适配器，确保 key 与文件名同步换位。
/// Build 的 Rust 实现直接 zip 排序；保留该类型和三个方法，便于人工逐声明对照来源。
struct startKeysAndFiles {
    startKeys: Vec<Key>,
    files: Vec<[String; 2]>,
}

impl startKeysAndFiles {
    fn Len(&self) -> usize {
        self.startKeys.len()
    }

    fn Less(&self, i: usize, j: usize) -> bool {
        self.startKeys[i] < self.startKeys[j]
    }

    fn Swap(&mut self, i: usize, j: usize) {
        self.startKeys.swap(i, j);
        self.files.swap(i, j);
    }
}

impl MultipleFilesStat {
    /// Build 计算聚合范围、按首 key 重排文件，并计算最大重叠度。
    pub fn Build(&mut self, startKeys: &[Key], endKeys: &[Key]) {
        if startKeys.is_empty() {
            return;
        }
        self.MinKey = startKeys.iter().min().cloned();
        self.MaxKey = endKeys.iter().max().cloned();

        // Go 的 startKeysAndFiles 同步交换两个切片；这里用 zip 后按 key 排序实现相同关系。
        let mut keyedFiles: Vec<_> = startKeys.iter().cloned().zip(self.Filenames.drain(..)).collect();
        keyedFiles.sort_by(|a, b| a.0.cmp(&b.0));
        self.Filenames = keyedFiles.into_iter().map(|(_, file)| file).collect();

        let mut points = Vec::with_capacity(startKeys.len() * 2);
        points.extend(startKeys.iter().map(|k| Endpoint { Key: k.0.clone(), Tp: EndpointTp::InclusiveStart, Weight: 1 }));
        points.extend(endKeys.iter().map(|k| Endpoint { Key: k.0.clone(), Tp: EndpointTp::InclusiveEnd, Weight: 1 }));
        self.MaxOverlappingNum = GetMaxOverlapping(&mut points);
    }
}

/// GetMaxOverlappingTotal 按每组的最坏重叠权重估算所有聚合统计的重叠层级。
pub fn GetMaxOverlappingTotal(stats: &[MultipleFilesStat]) -> i64 {
    let mut points = Vec::with_capacity(stats.len() * 2);
    for stat in stats {
        points.push(Endpoint { Key: stat.MinKey.as_ref().unwrap().0.clone(), Tp: EndpointTp::InclusiveStart, Weight: stat.MaxOverlappingNum });
        points.push(Endpoint { Key: stat.MaxKey.as_ref().unwrap().0.clone(), Tp: EndpointTp::InclusiveEnd, Weight: stat.MaxOverlappingNum });
    }
    GetMaxOverlapping(&mut points)
}

/// Writer 把内存中的 KV 批次排序后写入成对的数据/统计文件，并维护 500 文件聚合统计。
pub struct Writer {
    store: Storage,
    writerID: String,
    groupOffset: i32,
    currentSeq: usize,
    filenamePrefix: String,
    rnd: Rand,
    rc: RangePropertiesCollector,
    kvBuffer: Buffer,
    kvLocations: Vec<SliceLocation>,
    kvSize: i64,
    onClose: OnWriterCloseFunc,
    onDup: OnDuplicateKey,
    closed: bool,
    batchSize: u64,
    multiFileStats: Vec<MultipleFilesStat>,
    fileMinKeys: Vec<Key>,
    fileMaxKeys: Vec<Key>,
    minKey: Option<Key>,
    maxKey: Option<Key>,
    totalSize: u64,
    totalCnt: u64,
    kvFileCount: usize,
    tikvCodec: Option<Codec>,
    conflictInfo: ConflictInfo,
}

impl Writer {
    /// WriteRow 实现 ingest.Writer，把 KV 编码进预分配块；空间不足时先刷新已有批次。
    pub fn WriteRow(&mut self, ctx: &Context, key: &[u8], val: &[u8], _handle: Option<Handle>) -> Result<(), Error> {
        let encodedKey;
        let key = if let Some(codec) = &self.tikvCodec {
            encodedKey = codec.EncodeKey(key);
            &encodedKey
        } else {
            key
        };
        let length = key.len() + val.len() + LengthBytes * 2;
        let mut allocation = self.kvBuffer.AllocBytesWithSliceLocation(length);
        if allocation.is_none() {
            self.flushKVs(ctx, false)?;
            allocation = self.kvBuffer.AllocBytesWithSliceLocation(length);
            if allocation.is_none() {
                return Err(Error::allocation_failed(length));
            }
        }
        let (dataBuf, loc) = allocation.unwrap();
        encodeToBuf(dataBuf, key, val);
        self.kvLocations.push(loc);
        self.kvSize += (key.len() + val.len()) as i64;
        self.batchSize += length as u64;
        Ok(())
    }

    /// LockForWrite 在 Go 中是 noop，因为外部存储 writer 的 flushKVs 已保证线程安全。
    pub fn LockForWrite(&self) -> impl FnOnce() {
        || {}
    }

    pub fn WrittenBytes(&self) -> i64 {
        self.totalSize as i64
    }

    /// Close 只允许调用一次；最终刷新后销毁大缓冲并发布 WriterSummary。
    pub fn Close(&mut self, ctx: &Context) -> Result<(), Error> {
        if self.closed {
            return Err(Error::writer_closed(&self.writerID));
        }
        self.closed = true;
        let result = self.flushKVs(ctx, true);
        // 对应 Go defer：即使最终 flush 失败也要归还预分配内存。
        self.kvBuffer.Destroy();
        result?;
        logWriterSummary(ctx, self);
        self.kvLocations.clear();
        (self.onClose)(WriterSummary {
            WriterID: self.writerID.clone(), GroupOffset: self.groupOffset, Seq: self.currentSeq,
            Min: self.minKey.clone(), Max: self.maxKey.clone(), TotalSize: self.totalSize,
            TotalCnt: self.totalCnt, KVFileCount: self.kvFileCount,
            MultipleFilesStats: std::mem::take(&mut self.multiFileStats),
            ConflictInfo: std::mem::take(&mut self.conflictInfo),
        });
        Ok(())
    }

    /// recordMinMax 更新整个 writer 的闭区间范围，并复制 key 脱离可复用缓冲区。
    fn recordMinMax(&mut self, newMin: &[u8], newMax: &[u8]) {
        if self.minKey.as_ref().map_or(true, |k| newMin < k.as_slice()) {
            self.minKey = Some(Key(newMin.to_vec()));
        }
        if self.maxKey.as_ref().map_or(true, |k| newMax > k.as_slice()) {
            self.maxKey = Some(Key(newMax.to_vec()));
        }
    }

    /// flushKVs 对当前批次排序、处理重复键、重试上传，并更新文件级与冲突统计。
    fn flushKVs(&mut self, ctx: &Context, fromClose: bool) -> Result<(), Error> {
        if self.kvLocations.is_empty() {
            return Ok(());
        }
        let sortStart = Instant::now();
        let mut dupLoc = None;
        self.kvLocations.sort_by(|i, j| {
            let result = self.getKeyByLoc(i).cmp(self.getKeyByLoc(j));
            if result.is_eq() && dupLoc.is_none() {
                dupLoc = Some(i.clone());
            }
            result
        });
        recordSortMetrics(self.batchSize, sortStart.elapsed());

        let batchKVCnt = self.kvLocations.len();
        let mut dupLocs = Vec::new();
        let mut dupCnt = 0;
        if let Some(firstDup) = dupLoc {
            match self.onDup {
                OnDuplicateKey::Ignore => {}
                OnDuplicateKey::Record => {
                    // 缺乏全局视图时保留每组前两个，其余写冲突文件供后续阶段合并识别。
                    (self.kvLocations, dupLocs, dupCnt) = removeDuplicatesMoreThanTwo(
                        &mut self.kvLocations, |loc| self.getKeyByLoc(loc));
                    self.kvSize = self.reCalculateKVSize();
                }
                OnDuplicateKey::Remove => {
                    (self.kvLocations, _, dupCnt) = RemoveDuplicates(
                        &mut self.kvLocations, |loc| self.getKeyByLoc(loc), false);
                    self.kvSize = self.reCalculateKVSize();
                }
                OnDuplicateKey::Error => {
                    // 错误参数必须复制，避免返回后缓冲重置使 key/value 失效。
                    let key = self.getKeyByLoc(&firstDup).to_vec();
                    let val = self.getValueByLoc(&firstDup).to_vec();
                    return Err(Error::found_duplicate_keys(&key, &val));
                }
            }
        }

        let writeStart = Instant::now();
        let mut files = (String::new(), String::new(), String::new());
        if !self.kvLocations.is_empty() {
            let mut lastError = None;
            for retry in 0..flushKVsRetryTimes {
                match self.flushSortedKVs(ctx, &dupLocs) {
                    Ok(result) => { files = result; lastError = None; break; }
                    Err(err) => {
                        lastError = Some(err);
                        if ctx.is_cancelled() { break; }
                        // Go 最多尝试三次，每次失败记录序号；文件名包含相同 seq，底层负责覆盖/重建。
                        logFlushRetry(&self.writerID, self.currentSeq, retry);
                    }
                }
            }
            if let Some(err) = lastError {
                return Err(err);
            }
        }
        recordFlushMetrics(self.batchSize, batchKVCnt, dupCnt, dupLocs.len(), sortStart, writeStart);

        if !self.kvLocations.is_empty() {
            self.totalCnt += self.kvLocations.len() as u64;
            self.totalSize += self.kvSize as u64;
            self.kvFileCount += 1;
            let minKey = self.getKeyByLoc(self.kvLocations.first().unwrap()).to_vec();
            let maxKey = self.getKeyByLoc(self.kvLocations.last().unwrap()).to_vec();
            self.recordMinMax(&minKey, &maxKey);
            self.addNewKVFile2MultiFileStats(files.0, files.1, &minKey, &maxKey);
        }
        if fromClose && !self.multiFileStats.is_empty() {
            self.multiFileStats.last_mut().unwrap().Build(&self.fileMinKeys, &self.fileMaxKeys);
        }
        if !dupLocs.is_empty() {
            self.conflictInfo.Merge(ConflictInfo { Count: dupLocs.len() as u64, Files: vec![files.2] });
        }

        // 只有上传及统计全部成功后才重置批次，失败时保留缓冲以允许调用方诊断或重试。
        self.kvLocations.clear();
        self.kvSize = 0;
        self.kvBuffer.Reset();
        self.batchSize = 0;
        self.currentSeq += 1;
        Ok(())
    }

    /// addNewKVFile2MultiFileStats 每 500 个文件结束一组统计并开始下一组。
    fn addNewKVFile2MultiFileStats(&mut self, dataFile: String, statFile: String, minKey: &[u8], maxKey: &[u8]) {
        let needNew = self.multiFileStats.last().map_or(true, |s| s.Filenames.len() == MultiFileStatNum);
        if needNew {
            if let Some(last) = self.multiFileStats.last_mut() {
                last.Build(&self.fileMinKeys, &self.fileMaxKeys);
            }
            self.multiFileStats.push(MultipleFilesStat { Filenames: Vec::with_capacity(MultiFileStatNum), ..Default::default() });
            self.fileMinKeys.clear();
            self.fileMaxKeys.clear();
        }
        self.multiFileStats.last_mut().unwrap().Filenames.push([dataFile, statFile]);
        self.fileMinKeys.push(Key(minKey.to_vec()));
        self.fileMaxKeys.push(Key(maxKey.to_vec()));
    }

    /// flushSortedKVs 把排序后位置逐项写入数据文件，再写统计文件和可选冲突文件。
    fn flushSortedKVs(&mut self, ctx: &Context, dupLocs: &[SliceLocation]) -> Result<(String, String, String), Error> {
        let writeStart = Instant::now();
        let (dataFile, statFile, mut dataWriter, mut statWriter) = self.createStorageWriter(ctx)?;
        self.rc.Reset();
        let mut kvStore = NewKeyValueStore(ctx, &mut dataWriter, Some(&mut self.rc));
        for loc in &self.kvLocations {
            kvStore.addEncodedData(self.kvBuffer.GetSlice(loc))?;
        }
        kvStore.Finish();
        let encodedStat = self.rc.Encode();
        let statSize = encodedStat.len();
        statWriter.Write(ctx, &encodedStat)?;

        // 正常路径按 data、stat 顺序关闭；任一错误都由 RAII guard 尽力关闭仍打开的 writer。
        dataWriter.Close(ctx)?;
        statWriter.Close(ctx)?;
        let dupPath = if dupLocs.is_empty() { String::new() } else { self.writeDupKVs(ctx, dupLocs)? };
        recordSortedWriteMetrics(self.batchSize, statSize, writeStart.elapsed());
        Ok((dataFile, statFile, dupPath))
    }

    /// writeDupKVs 将 record 策略多出的重复项保持原编码写入独立文件。
    fn writeDupKVs(&mut self, ctx: &Context, kvLocs: &[SliceLocation]) -> Result<String, Error> {
        let (dupPath, mut dupWriter) = self.createDupWriter(ctx)?;
        let mut dupStore = NewKeyValueStore(ctx, &mut dupWriter, None);
        for loc in kvLocs {
            dupStore.addEncodedData(self.kvBuffer.GetSlice(loc))?;
        }
        dupStore.Finish();
        dupWriter.Close(ctx)?;
        Ok(dupPath)
    }

    /// getKeyByLoc 从大端长度头计算 key 切片，不复制底层块。
    fn getKeyByLoc<'a>(&'a self, loc: &SliceLocation) -> &'a [u8] {
        let block = self.kvBuffer.GetSlice(loc);
        let keyLen = u64::from_be_bytes(block[..LengthBytes].try_into().unwrap()) as usize;
        &block[2 * LengthBytes..2 * LengthBytes + keyLen]
    }

    /// getValueByLoc 跳过两个长度头和 key，返回剩余 value。
    fn getValueByLoc<'a>(&'a self, loc: &SliceLocation) -> &'a [u8] {
        let block = self.kvBuffer.GetSlice(loc);
        let keyLen = u64::from_be_bytes(block[..LengthBytes].try_into().unwrap()) as usize;
        &block[2 * LengthBytes + keyLen..]
    }

    fn reCalculateKVSize(&self) -> i64 {
        self.kvLocations.iter().map(|loc| loc.Length as i64 - (2 * LengthBytes) as i64).sum()
    }

    /// createStorageWriter 为当前 seq 分别创建数据和统计 writer；统计创建失败会关闭数据 writer。
    fn createStorageWriter(&mut self, ctx: &Context) -> Result<(String, String, ObjectWriter, ObjectWriter), Error> {
        let dataPath = joinPath(&self.getPartitionedPrefix(), &self.currentSeq.to_string());
        let mut dataWriter = self.store.Create(ctx, &dataPath, WriterOption { Concurrency: 20, PartSize: MinUploadPartSize })?;
        let statPath = joinPath(&(self.getPartitionedPrefix() + statSuffix), &self.currentSeq.to_string());
        let statsWriter = match self.store.Create(ctx, &statPath, WriterOption { Concurrency: 20, PartSize: MinUploadPartSize }) {
            Ok(writer) => writer,
            Err(err) => { let _ = dataWriter.Close(ctx); return Err(err); }
        };
        Ok((dataPath, statPath, dataWriter, statsWriter))
    }

    fn createDupWriter(&mut self, ctx: &Context) -> Result<(String, ObjectWriter), Error> {
        let path = joinPath(&(self.getPartitionedPrefix() + dupSuffix), &self.currentSeq.to_string());
        let writer = self.store.Create(ctx, &path, WriterOption { Concurrency: 20, PartSize: MinUploadPartSize })?;
        Ok((path, writer))
    }

    fn getPartitionedPrefix(&mut self) -> String {
        randPartitionedPrefix(&self.filenamePrefix, &mut self.rnd)
    }
}

/// randPartitionedPrefix 生成 p 加 8 位二进制随机数，使云对象 key 可提前按前缀分区降低限流。
pub fn randPartitionedPrefix(prefix: &str, rnd: &mut Rand) -> String {
    let partitionPrefix = format!("{}{:08b}", partitionHeader, rnd.Intn(256));
    joinPath(&partitionPrefix, prefix)
}

/// IsValidPartition 验证前缀必须恰为 `p[01]{8}`。
pub fn IsValidPartition(input: &[u8]) -> bool {
    input.len() == 9 && input[0] == partitionHeaderChar && input[1..].iter().all(|c| *c == b'0' || *c == b'1')
}

/// EngineWriter 把 lightning backend.EngineWriter 的 Rows 接口适配到排序 Writer。
pub struct EngineWriter {
    w: Writer,
}

impl EngineWriter {
    pub fn NewEngineWriter(w: Writer) -> Self {
        Self { w }
    }

    /// AppendRows 把 encode.Rows 展开成 KV；空输入直接成功，不触发缓冲或 IO。
    pub fn AppendRows(&mut self, ctx: &Context, _columnNames: &[String], rows: Rows) -> Result<(), Error> {
        let kvs = Rows2KvPairs(rows);
        for item in kvs {
            self.w.WriteRow(ctx, &item.Key, &item.Val, None)?;
        }
        Ok(())
    }

    /// IsSynced 只在保存 checkpoint 时使用；Go 实现始终返回 true。
    pub fn IsSynced(&self) -> bool {
        true
    }

    /// Close 关闭底层 writer；ChunkFlushStatus 在 Go 中返回 nil。
    pub fn Close(&mut self, ctx: &Context) -> Result<Option<ChunkFlushStatus>, Error> {
        self.w.Close(ctx)?;
        Ok(None)
    }
}

/// getSpeed 格式化吞吐；零时长返回横线，避免除零和无意义的无穷大。
fn getSpeed(n: u64, dur: f64, isBytes: bool) -> String {
    if dur == 0.0 {
        return "-".to_owned();
    }
    if isBytes {
        formatBytes(n as f64 / dur)
    } else {
        format!("{:.4}", n as f64 / dur)
    }
}

/// getHash 对文件名前缀计算稳定 FNV-1a 64 位种子，使随机分区序列可复现。
fn getHash(value: &str) -> i64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in value.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash as i64
}
*/

// 多文件排序 SST Writer：缓冲、去重、写出数据/统计/冲突文件。
//
// 对应 Go `writer.go`：按内存上限批次排序后写入对象存储；支持 Ignore/Record/
// Remove/Error 重复键策略，并收集 `MultipleFilesStat` 供后续 merge-sort 决策。
// SST：Sorted String Table；ingest：跳过常规写路径直接导入存储引擎。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

use crate::codec::{RangeProperty, encode_multi_props};
use crate::file::{DUP_SUFFIX, KeyValueStore, STAT_SUFFIX, encode_kv};
use crate::util::{Endpoint, EndpointTp, get_max_overlapping};
use crate::{Error, MemoryStorage, Result};

/// Sink for real object stores. The writer only needs whole-file writes;
/// callers retain ownership of the store and its close lifecycle.
pub trait WriterSink: Send + Sync {
    fn write_file(&self, path: &str, data: &[u8]) -> std::result::Result<(), String>;
}

enum WriterStorage {
    Memory(MemoryStorage),
    External(Arc<dyn WriterSink>),
}

impl WriterStorage {
    fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        match self {
            Self::Memory(storage) => storage.write(path, data.to_vec()),
            Self::External(sink) => sink
                .write_file(path, data)
                .map_err(|error| Error::Io(std::io::ErrorKind::Other, error)),
        }
    }
}

/// 每组多文件统计最多收录的文件对数。
pub static MultiFileStatNum: AtomicUsize = AtomicUsize::new(500);
/// 按累计字节数切分范围属性的默认阈值（1 MiB）。
pub const DefaultPropSizeDist: u64 = 1024 * 1024;
/// 按 key 数切分范围属性的默认阈值。
pub const DefaultPropKeysDist: u64 = 8 * 1024;
/// merge-sort 文件数步长上限。
pub const MaxMergeSortFileCountStep: usize = 4000;
/// 单个 merge 子任务目标文件数上限。
pub const MergeSortMaxSubtaskTargetFiles: usize = 16;
/// 上传分片最小大小（5 MiB，对齐常见对象存储限制）。
pub const MinUploadPartSize: i64 = 5 * 1024 * 1024;
/// Writer 默认内存缓冲上限（256 MiB）。
pub const DefaultMemSizeLimit: u64 = 256 * 1024 * 1024;
/// 默认块大小（16 MiB）。
pub const DefaultBlockSize: usize = 16 * 1024 * 1024;
/// merge-sort 区间重叠阈值全局上限。
pub const MAX_MERGE_SORT_OVERLAP_THRESHOLD: i64 = 4000;
/// 单个对象写入失败时的最大尝试次数。
const FLUSH_KVS_RETRY_TIMES: usize = 3;

/// 按并发度计算调整值：`250 * concurrency`，再截断到对应上限。
pub fn common_get_adjust_count(overlap: bool, concurrency: i32) -> i64 {
    let concurrency = concurrency.max(1) as i64;
    let maximum = if overlap {
        MAX_MERGE_SORT_OVERLAP_THRESHOLD
    } else {
        MaxMergeSortFileCountStep as i64
    };
    (250 * concurrency).min(maximum)
}
/// 低并发机器上降低 merge-sort 重叠阈值。
pub fn GetAdjustedMergeSortOverlapThreshold(concurrency: i32) -> i64 {
    common_get_adjust_count(true, concurrency)
}
/// 按并发度调整后的文件数步长。
pub fn GetAdjustedMergeSortFileCountStep(concurrency: i32) -> usize {
    common_get_adjust_count(false, concurrency) as usize
}
/// 对齐缓冲大小；对齐浪费超过 10% 时直接用可用总量，避免 OOM。
pub fn GetAdjustedBlockSize(total: u64, default_size: usize) -> usize {
    if total == 0 {
        return default_size;
    }
    let block = default_size as u64;
    let aligned = total.div_ceil(block) * block;
    if aligned as f64 / total as f64 > 1.1 {
        total as usize
    } else {
        default_size
    }
}

#[derive(Clone, Debug)]
/// 按字节数或 key 数切分并收集 `RangeProperty`。
pub struct RangePropertiesCollector {
    props: Vec<RangeProperty>,
    curr_prop: RangeProperty,
    prop_size_dist: u64,
    prop_keys_dist: u64,
}

impl RangePropertiesCollector {
    /// 构造空收集器，使用给定的大小/键数切分距离。
    pub fn new(size_distance: u64, keys_distance: u64) -> Self {
        Self {
            props: Vec::new(),
            curr_prop: RangeProperty::default(),
            prop_size_dist: size_distance,
            prop_keys_dist: keys_distance,
        }
    }
    /// 只读当前正在累计的属性。
    pub fn curr_prop(&self) -> &RangeProperty {
        &self.curr_prop
    }
    /// 可变借用当前属性（如对齐新块 offset）。
    pub fn curr_prop_mut(&mut self) -> &mut RangeProperty {
        &mut self.curr_prop
    }
    /// 已完成的范围属性列表。
    pub fn properties(&self) -> &[RangeProperty] {
        &self.props
    }
    /// 接收已编码 KV 与写入后文件大小，更新当前范围；达阈值则封存并开新段。
    pub fn on_next_encoded_data(&mut self, data: &[u8], file_size: u64) -> Result<()> {
        if data.len() < 16 {
            return Err(Error::InvalidData("truncated encoded key/value".into()));
        }
        // 编码布局：`<key-len:8><val-len:8><key><value>`，大端长度。
        let key_len = u64::from_be_bytes(data[..8].try_into().unwrap()) as usize;
        if 16usize
            .checked_add(key_len)
            .is_none_or(|end| end > data.len())
        {
            return Err(Error::InvalidData("truncated key".into()));
        }
        let key = &data[16..16 + key_len];
        if self.curr_prop.FirstKey.is_empty() {
            self.curr_prop.FirstKey = key.to_vec();
        }
        self.curr_prop.LastKey = key.to_vec();
        // Size 不含两个长度头，与 Go 数据口径一致。
        self.curr_prop.Size = self
            .curr_prop
            .Size
            .checked_add((data.len() - 16) as u64)
            .ok_or_else(|| Error::InvalidData("property size overflow".into()))?;
        self.curr_prop.Keys += 1;
        if self.curr_prop.Size >= self.prop_size_dist || self.curr_prop.Keys >= self.prop_keys_dist
        {
            self.props.push(self.curr_prop.clone());
            // 新属性从当前文件尾开始。
            self.curr_prop = RangeProperty {
                Offset: file_size,
                ..RangeProperty::default()
            };
        }
        Ok(())
    }
    /// 文件结束时把未达阈值的最后一段属性并入结果。
    pub fn on_file_end(&mut self) {
        if self.curr_prop.Keys > 0 {
            self.props.push(self.curr_prop.clone());
            self.curr_prop.Keys = 0;
        }
    }
    /// 清空已完成属性与当前累计状态，以便复用。
    pub fn reset(&mut self) {
        self.props.clear();
        self.curr_prop = RangeProperty::default();
    }
    /// 编码为长度前缀的多属性字节流。
    pub fn encode(&self) -> Result<Vec<u8>> {
        encode_multi_props(&self.props)
    }
    /// Go 风格别名。
    pub fn CurrProp(&self) -> &RangeProperty {
        self.curr_prop()
    }
    /// Go 风格别名。
    pub fn Reset(&mut self) {
        self.reset()
    }
    /// Go 风格别名。
    pub fn Encode(&self) -> Result<Vec<u8>> {
        self.encode()
    }
}
/// Go 风格构造入口。
pub fn NewRangePropertiesCollector(
    size_distance: u64,
    keys_distance: u64,
) -> RangePropertiesCollector {
    RangePropertiesCollector::new(size_distance, keys_distance)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 重复键处理策略。
pub enum DuplicateMode {
    #[default]
    /// 保留全部重复键（不主动去重）。
    Ignore,
    /// 保留每组前两条，其余写入冲突文件。
    Record,
    /// 丢弃整组重复键。
    Remove,
    /// 发现重复即返回错误。
    Error,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 冲突（重复键）汇总：条数与冲突文件路径。
pub struct ConflictInfo {
    pub Count: u64,
    pub Files: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 一组数据/统计文件对的范围与重叠统计。
pub struct MultipleFilesStat {
    pub MinKey: Vec<u8>,
    pub MaxKey: Vec<u8>,
    /// 每项为 `[data_path, stat_path]`。
    pub Filenames: Vec<[String; 2]>,
    /// 本组内文件键范围的最大重叠数。
    pub MaxOverlappingNum: i64,
}
impl MultipleFilesStat {
    /// 按 start 排序文件名，并计算组内最大重叠。
    pub fn build(&mut self, start_keys: &[Vec<u8>], end_keys: &[Vec<u8>]) -> Result<()> {
        if start_keys.len() != end_keys.len() || start_keys.len() != self.Filenames.len() {
            return Err(Error::InvalidData("file statistics length mismatch".into()));
        }
        if start_keys.is_empty() {
            return Ok(());
        }
        self.MinKey = start_keys.iter().min().unwrap().clone();
        self.MaxKey = end_keys.iter().max().unwrap().clone();
        // 按起始键排序文件对，便于后续 merge 决策。
        let mut order: Vec<usize> = (0..start_keys.len()).collect();
        order.sort_by(|a, b| start_keys[*a].cmp(&start_keys[*b]));
        self.Filenames = order
            .into_iter()
            .map(|index| self.Filenames[index].clone())
            .collect();
        let mut points = Vec::with_capacity(start_keys.len() * 2);
        for key in start_keys {
            points.push(Endpoint {
                Key: key.clone(),
                Tp: EndpointTp::InclusiveStart,
                Weight: 1,
            });
        }
        for key in end_keys {
            points.push(Endpoint {
                Key: key.clone(),
                Tp: EndpointTp::InclusiveEnd,
                Weight: 1,
            });
        }
        self.MaxOverlappingNum = get_max_overlapping(&mut points);
        Ok(())
    }
    /// Go 风格别名。
    pub fn Build(&mut self, start_keys: &[Vec<u8>], end_keys: &[Vec<u8>]) -> Result<()> {
        self.build(start_keys, end_keys)
    }
}
/// 跨多组统计文件的加权最大重叠（权重为组内 MaxOverlappingNum）。
pub fn GetMaxOverlappingTotal(stats: &[MultipleFilesStat]) -> i64 {
    let mut points = Vec::with_capacity(stats.len() * 2);
    for stat in stats {
        points.push(Endpoint {
            Key: stat.MinKey.clone(),
            Tp: EndpointTp::InclusiveStart,
            Weight: stat.MaxOverlappingNum,
        });
    }
    for stat in stats {
        points.push(Endpoint {
            Key: stat.MaxKey.clone(),
            Tp: EndpointTp::InclusiveEnd,
            Weight: stat.MaxOverlappingNum,
        });
    }
    get_max_overlapping(&mut points)
}

#[derive(Clone, Debug, Default)]
/// Writer 关闭后的不可变汇总；重复策略会影响 TotalSize/TotalCnt。
pub struct WriterSummary {
    pub WriterID: String,
    pub GroupOffset: i32,
    pub Seq: usize,
    pub Min: Vec<u8>,
    pub Max: Vec<u8>,
    pub TotalSize: u64,
    pub TotalCnt: u64,
    pub KVFileCount: usize,
    pub MultipleFilesStats: Vec<MultipleFilesStat>,
    pub ConflictInfo: ConflictInfo,
}

/// 关闭回调类型：接收最终汇总。
pub(crate) type CloseCallback = Arc<dyn Fn(&WriterSummary) + Send + Sync>;

#[derive(Clone)]
/// 构建 Writer / OneFileWriter 的共享配置。
pub struct WriterBuilder {
    group_offset: i32,
    memory_size_limit: u64,
    block_size: usize,
    prop_size_distance: u64,
    prop_keys_distance: u64,
    on_close: CloseCallback,
    on_duplicate: DuplicateMode,
    key_prefix: Vec<u8>,
}
impl Default for WriterBuilder {
    /// 与 Go NewWriterBuilder 对齐的默认配置。
    fn default() -> Self {
        Self {
            group_offset: 0,
            memory_size_limit: DefaultMemSizeLimit,
            block_size: DefaultBlockSize,
            prop_size_distance: DefaultPropSizeDist,
            prop_keys_distance: DefaultPropKeysDist,
            on_close: Arc::new(|_| {}),
            on_duplicate: DuplicateMode::Ignore,
            key_prefix: Vec::new(),
        }
    }
}
impl WriterBuilder {
    /// 使用默认配置构造 builder。
    pub fn new() -> Self {
        Self::default()
    }
    /// 设置内存缓冲上限。
    pub fn set_memory_size_limit(&mut self, size: u64) -> &mut Self {
        self.memory_size_limit = size;
        self
    }
    /// 设置按字节切分属性的距离。
    pub fn set_prop_size_distance(&mut self, distance: u64) -> &mut Self {
        self.prop_size_distance = distance;
        self
    }
    /// 设置按 key 数切分属性的距离。
    pub fn set_prop_keys_distance(&mut self, distance: u64) -> &mut Self {
        self.prop_keys_distance = distance;
        self
    }
    /// 设置块大小。
    pub fn set_block_size(&mut self, size: usize) -> &mut Self {
        self.block_size = size;
        self
    }
    /// 设置分组偏移，写入汇总供上层识别。
    pub fn set_group_offset(&mut self, offset: i32) -> &mut Self {
        self.group_offset = offset;
        self
    }
    /// 设置重复键策略。
    pub fn set_on_duplicate(&mut self, mode: DuplicateMode) -> &mut Self {
        self.on_duplicate = mode;
        self
    }
    /// EncodeKey of TiKV API V1 is identity; API V2 prepends its keyspace bytes.
    pub fn set_key_prefix(&mut self, prefix: Vec<u8>) -> &mut Self {
        self.key_prefix = prefix;
        self
    }
    /// 设置关闭回调。
    pub fn set_on_close<F>(&mut self, callback: F) -> &mut Self
    where
        F: Fn(&WriterSummary) + Send + Sync + 'static,
    {
        self.on_close = Arc::new(callback);
        self
    }
    /// 构建多文件排序 Writer。
    pub fn build(&self, storage: MemoryStorage, prefix: &str, writer_id: &str) -> Writer {
        Writer::new(WriterStorage::Memory(storage), prefix, writer_id, self)
    }
    /// Build a writer against a production object-store sink.
    pub fn build_with_sink(
        &self,
        sink: Arc<dyn WriterSink>,
        prefix: &str,
        writer_id: &str,
    ) -> Writer {
        Writer::new(WriterStorage::External(sink), prefix, writer_id, self)
    }
    /// 构建单文件 Writer（数据与统计各一个文件）。
    pub fn build_one_file(
        &self,
        storage: MemoryStorage,
        prefix: &str,
        writer_id: &str,
    ) -> crate::onefile_writer::OneFileWriter {
        crate::onefile_writer::OneFileWriter::new(storage, prefix, writer_id, self)
    }
    /// 导出配置元组供 Writer / OneFileWriter 构造使用。
    pub(crate) fn configuration(
        &self,
    ) -> (u64, usize, u64, u64, DuplicateMode, CloseCallback, i32) {
        (
            self.memory_size_limit,
            self.block_size,
            self.prop_size_distance,
            self.prop_keys_distance,
            self.on_duplicate,
            Arc::clone(&self.on_close),
            self.group_offset,
        )
    }
    /// Go 风格别名。
    pub fn SetMemorySizeLimit(&mut self, v: u64) -> &mut Self {
        self.set_memory_size_limit(v)
    }
    /// Go 风格别名。
    pub fn SetPropSizeDistance(&mut self, v: u64) -> &mut Self {
        self.set_prop_size_distance(v)
    }
    /// Go 风格别名。
    pub fn SetPropKeysDistance(&mut self, v: u64) -> &mut Self {
        self.set_prop_keys_distance(v)
    }
    /// Go 风格别名。
    pub fn SetBlockSize(&mut self, v: usize) -> &mut Self {
        self.set_block_size(v)
    }
    /// Go 风格别名。
    pub fn SetGroupOffset(&mut self, v: i32) -> &mut Self {
        self.set_group_offset(v)
    }
    /// Go 风格别名。
    pub fn SetOnDup(&mut self, v: DuplicateMode) -> &mut Self {
        self.set_on_duplicate(v)
    }
}
/// Go 风格构造入口。
pub fn NewWriterBuilder() -> WriterBuilder {
    WriterBuilder::new()
}

/// 多文件排序写入器：内存缓冲，超限或关闭时排序写出。
pub struct Writer {
    storage: WriterStorage,
    writer_id: String,
    group_offset: i32,
    filename_prefix: String,
    random_state: u64,
    current_sequence: usize,
    memory_limit: u64,
    rows: Vec<(Vec<u8>, Vec<u8>)>,
    buffered_size: u64,
    property_size: u64,
    property_keys: u64,
    on_duplicate: DuplicateMode,
    on_close: CloseCallback,
    closed: bool,
    summary: WriterSummary,
    pending_min: Vec<Vec<u8>>,
    pending_max: Vec<Vec<u8>>,
    key_prefix: Vec<u8>,
}
impl Writer {
    /// 从 builder 配置与存储路径前缀构造 Writer。
    fn new(storage: WriterStorage, prefix: &str, writer_id: &str, builder: &WriterBuilder) -> Self {
        let (memory_limit, _, property_size, property_keys, on_duplicate, on_close, group_offset) =
            builder.configuration();
        let filename_prefix = join_path(prefix, writer_id);
        Self {
            storage,
            writer_id: writer_id.into(),
            group_offset,
            random_state: get_hash(&filename_prefix),
            filename_prefix,
            current_sequence: 0,
            memory_limit,
            rows: Vec::new(),
            buffered_size: 0,
            property_size,
            property_keys,
            on_duplicate,
            on_close,
            closed: false,
            summary: WriterSummary::default(),
            pending_min: Vec::new(),
            pending_max: Vec::new(),
            key_prefix: builder.key_prefix.clone(),
        }
    }
    /// 写入一行 KV；缓冲将超限时先 flush。
    pub fn write_row(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        let key = if self.key_prefix.is_empty() {
            key.to_vec()
        } else {
            let mut encoded = Vec::with_capacity(self.key_prefix.len() + key.len());
            encoded.extend_from_slice(&self.key_prefix);
            encoded.extend_from_slice(key);
            encoded
        };
        let encoded = 16usize
            .checked_add(key.len())
            .and_then(|n| n.checked_add(value.len()))
            .ok_or_else(|| Error::InvalidData("key/value length overflow".into()))?
            as u64;
        if !self.rows.is_empty() && self.buffered_size.saturating_add(encoded) > self.memory_limit {
            self.flush(false)?;
        }
        if encoded > self.memory_limit {
            return Err(Error::InvalidData(format!(
                "key/value pair exceeds writer memory limit: {encoded}"
            )));
        }
        self.rows.push((key, value.to_vec()));
        self.buffered_size += encoded;
        Ok(())
    }
    /// 排序当前批次、按策略处理重复键，并写出数据/统计/冲突文件。
    fn flush(&mut self, closing: bool) -> Result<()> {
        if self.rows.is_empty() {
            return Ok(());
        }
        // 按 key 排序后扫描相邻重复组。
        self.rows.sort_by(|left, right| left.0.cmp(&right.0));
        let original = std::mem::take(&mut self.rows);
        let mut kept = Vec::new();
        let mut conflicts = Vec::new();
        let mut cursor = 0;
        while cursor < original.len() {
            let mut end = cursor + 1;
            while end < original.len() && original[end].0 == original[cursor].0 {
                end += 1;
            }
            let group = &original[cursor..end];
            // 单键或 Ignore：整组保留。
            if group.len() == 1 || self.on_duplicate == DuplicateMode::Ignore {
                kept.extend(group.iter().cloned());
            } else {
                match self.on_duplicate {
                    // Record：保留前两条，其余进冲突文件。
                    DuplicateMode::Record => {
                        kept.extend(group.iter().take(2).cloned());
                        conflicts.extend(group.iter().skip(2).cloned());
                    }
                    DuplicateMode::Remove => {}
                    DuplicateMode::Error => {
                        return Err(Error::DuplicateKey {
                            key: group[0].0.clone(),
                            value: group[0].1.clone(),
                        });
                    }
                    DuplicateMode::Ignore => unreachable!(),
                }
            }
            cursor = end;
        }
        if !kept.is_empty() {
            let prefix = rand_partitioned_prefix(&self.filename_prefix, &mut self.random_state);
            // 随机分区前缀分散对象 key，降低云存储限流。
            let data_path = join_path(&prefix, &self.current_sequence.to_string());
            let stat_path = join_path(&(prefix + STAT_SUFFIX), &self.current_sequence.to_string());
            let mut store = KeyValueStore::new(Some(RangePropertiesCollector::new(
                self.property_size,
                self.property_keys,
            )));
            for (key, value) in &kept {
                store.add_raw_kv(key, value)?;
            }
            let (data, collector) = store.into_parts();
            write_object_with_retry(&self.storage, &data_path, &data)?;
            let stat_data = collector.unwrap().encode()?;
            write_object_with_retry(&self.storage, &stat_path, &stat_data)?;
            let min = kept.first().unwrap().0.clone();
            let max = kept.last().unwrap().0.clone();
            self.summary.Min = if self.summary.Min.is_empty() || min < self.summary.Min {
                min.clone()
            } else {
                self.summary.Min.clone()
            };
            self.summary.Max = if self.summary.Max.is_empty() || max > self.summary.Max {
                max.clone()
            } else {
                self.summary.Max.clone()
            };
            self.summary.TotalCnt += kept.len() as u64;
            self.summary.TotalSize += kept
                .iter()
                .map(|(k, v)| (k.len() + v.len()) as u64)
                .sum::<u64>();
            self.summary.KVFileCount += 1;
            if self
                .summary
                .MultipleFilesStats
                .last()
                .is_none_or(|s| s.Filenames.len() == MultiFileStatNum.load(AtomicOrdering::Acquire))
            {
                if let Some(last) = self.summary.MultipleFilesStats.last_mut() {
                    last.build(&self.pending_min, &self.pending_max)?;
                }
                self.pending_min.clear();
                self.pending_max.clear();
                self.summary
                    .MultipleFilesStats
                    .push(MultipleFilesStat::default());
            }
            self.summary
                .MultipleFilesStats
                .last_mut()
                .unwrap()
                .Filenames
                .push([data_path, stat_path]);
            self.pending_min.push(min);
            self.pending_max.push(max);
        }
        // 冲突文件使用 DUP_SUFFIX 路径后缀。
        if !conflicts.is_empty() {
            let prefix =
                rand_partitioned_prefix(&self.filename_prefix, &mut self.random_state) + DUP_SUFFIX;
            let path = join_path(&prefix, &self.current_sequence.to_string());
            let mut data = Vec::new();
            for (k, v) in &conflicts {
                data.extend_from_slice(&encode_kv(k, v)?);
            }
            write_object_with_retry(&self.storage, &path, &data)?;
            self.summary.ConflictInfo.Count += conflicts.len() as u64;
            self.summary.ConflictInfo.Files.push(path);
        }
        self.buffered_size = 0;
        self.current_sequence += 1;
        // 关闭路径封存最后一组 MultipleFilesStat。
        if closing {
            if let Some(last) = self.summary.MultipleFilesStats.last_mut() {
                last.build(&self.pending_min, &self.pending_max)?;
            }
            self.pending_min.clear();
            self.pending_max.clear();
        }
        Ok(())
    }
    /// 关闭底层 Writer 并返回汇总。
    /// 对本 Writer：最终 flush、触发关闭回调并返回汇总。
    pub fn close(&mut self) -> Result<WriterSummary> {
        if self.closed {
            return Err(Error::Closed);
        }
        // Go sets this before the final flush, so a failed Close still consumes
        // the writer and every later Close reports the closed error.
        self.closed = true;
        self.flush(true)?;
        self.summary.WriterID = self.writer_id.clone();
        self.summary.GroupOffset = self.group_offset;
        self.summary.Seq = self.current_sequence;
        (self.on_close)(&self.summary);
        Ok(self.summary.clone())
    }
    /// 已成功写入的数据总字节数。
    pub fn written_bytes(&self) -> i64 {
        self.summary.TotalSize as i64
    }
    /// Go 风格别名。
    pub fn WriteRow(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.write_row(key, value)
    }
    /// Go 风格别名。
    pub fn WrittenBytes(&self) -> i64 {
        self.written_bytes()
    }
    /// Go 风格别名。
    pub fn Close(&mut self) -> Result<WriterSummary> {
        self.close()
    }
}

/// 对单个对象写入执行与 Go `flushKVs` 相同的三次尝试；最终错误原样返回。
fn write_object_with_retry(storage: &WriterStorage, path: &str, data: &[u8]) -> Result<()> {
    let mut last_error = None;
    for _ in 0..FLUSH_KVS_RETRY_TIMES {
        match storage.write(path, data) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("retry loop executes at least once"))
}

/// 将 Lightning EngineWriter 适配到排序 Writer。
pub struct EngineWriter {
    writer: Writer,
}
impl EngineWriter {
    /// 包装已有 Writer。
    pub fn new(writer: Writer) -> Self {
        Self { writer }
    }
    /// 逐行写入；空切片不触发 IO。
    pub fn append_rows(&mut self, rows: &[(Vec<u8>, Vec<u8>)]) -> Result<()> {
        for (k, v) in rows {
            self.writer.write_row(k, v)?;
        }
        Ok(())
    }
    /// checkpoint 场景使用；实现恒为 true。
    pub fn is_synced(&self) -> bool {
        true
    }
    /// 关闭底层 Writer 并返回汇总。
    pub fn close(&mut self) -> Result<WriterSummary> {
        self.writer.close()
    }
}
/// Go 风格构造入口。
pub fn NewEngineWriter(writer: Writer) -> EngineWriter {
    EngineWriter::new(writer)
}

/// 拼接对象路径，去掉多余斜线。
pub(crate) fn join_path(left: &str, right: &str) -> String {
    format!(
        "{}/{}",
        left.trim_end_matches('/'),
        right.trim_start_matches('/')
    )
}
/// 生成 `p` + 8 位二进制随机分区前缀并拼到路径前。
pub fn rand_partitioned_prefix(prefix: &str, state: &mut u64) -> String {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    join_path(&format!("p{:08b}", (*state & 255) as u8), prefix)
}
/// 验证前缀必须恰为 `p[01]{8}`。
pub fn IsValidPartition(input: &[u8]) -> bool {
    input.len() == 9 && input[0] == b'p' && input[1..].iter().all(|b| *b == b'0' || *b == b'1')
}
/// 格式化吞吐；零时长返回横线，避免除零。
pub fn get_speed(n: u64, duration: f64, is_bytes: bool) -> String {
    if duration == 0.0 {
        "-".into()
    } else if is_bytes {
        format!("{:.2} B/s", n as f64 / duration)
    } else {
        format!("{:.4}", n as f64 / duration)
    }
}
/// 对文件名前缀计算 FNV-1a 64 位种子，使随机分区可复现。
pub fn get_hash(value: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

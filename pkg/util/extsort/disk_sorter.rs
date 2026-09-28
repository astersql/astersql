// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 基于磁盘的外排实现（DiskSorter）。
//
// 多 Writer 将 KV 刷成 SST 分片，`sort` 合并去重后落盘并写 `sorted` 标记；
// 之后通过 `MergingIter` 多路归并有序扫描。另含压缩候选挑选/切分算法
//（对应 Go `pkg/util/extsort` disk sorter）。

use crate::external_sorter::{Error, ExternalSorter, Iterator, Result, Writer, join_errors};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::sync::CancellationToken;

/// SST 数据文件后缀。
pub const SST_FILE_SUFFIX: &str = ".sst";
/// 原子写入用的临时文件后缀。
pub const TMP_FILE_SUFFIX: &str = ".tmp";
/// KV 直方图桶默认字节阈值（约 1MiB）。
pub const DEFAULT_KV_STATS_BUCKET_SIZE: usize = 1 << 20;
/// 用户属性中存放 KvStats JSON 的键名。
pub const KV_STATS_PROP_KEY: &str = "extsort.kvstats";
/// 目录内表示“已排序完成”的标记文件名。
pub const DISK_SORTER_SORTED_FILE: &str = "sorted";
/// 状态：仍可写入。
const DISK_SORTER_STATE_WRITING: i32 = 0;
/// 状态：正在排序。
const DISK_SORTER_STATE_SORTING: i32 = 1;
/// 状态：已排序，可迭代。
const DISK_SORTER_STATE_SORTED: i32 = 2;
/// SST 文件魔数，用于校验格式。
const FILE_MAGIC: &[u8; 8] = b"EXTSORT1";

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
/// 直方图单个桶：累计字节与上界 key。
pub struct KvStatsBucket {
    pub size: usize,
    pub upper_bound: Vec<u8>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
/// 文件内 KV 分布直方图，供压缩切分估算体积。
pub struct KvStats {
    pub histogram: Vec<KvStatsBucket>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 单个 SST 文件元数据（编号、键范围、统计）。
pub struct FileMetadata {
    pub file_num: u64,
    pub start_key: Vec<u8>,
    pub end_key: Vec<u8>,
    pub last_key: Vec<u8>,
    pub kv_stats: KvStats,
}

#[derive(Clone, Debug)]
/// DiskSorter 调参：并发、写缓冲与压缩阈值等。
pub struct DiskSorterOptions {
    pub concurrency: usize,
    pub writer_buffer_size: usize,
    pub compaction_threshold: usize,
    pub max_compaction_depth: usize,
    pub max_compaction_size: usize,
}

impl Default for DiskSorterOptions {
    fn default() -> Self {
        Self {
            concurrency: std::thread::available_parallelism().map_or(1, usize::from),
            writer_buffer_size: 128 << 20,
            compaction_threshold: 16,
            max_compaction_depth: 64,
            max_compaction_size: 512 << 20,
        }
    }
}

impl DiskSorterOptions {
    /// 将为 0/非法的字段回填为默认值。
    fn ensure_defaults(mut self) -> Self {
        let defaults = Self::default();
        if self.concurrency == 0 {
            self.concurrency = defaults.concurrency;
        }
        if self.writer_buffer_size == 0 {
            self.writer_buffer_size = defaults.writer_buffer_size;
        }
        if self.compaction_threshold == 0 {
            self.compaction_threshold = defaults.compaction_threshold;
        }
        if self.max_compaction_depth < 2 {
            self.max_compaction_depth = defaults.max_compaction_depth;
        }
        if self.max_compaction_size == 0 {
            self.max_compaction_size = defaults.max_compaction_size;
        }
        self
    }
}

#[derive(Clone, Debug)]
/// 内存中的键值对。
struct KeyValue {
    key: Vec<u8>,
    value: Vec<u8>,
}

/// 边写边累计直方图桶的收集器。
pub(crate) struct KvStatsCollector {
    bucket_size: usize,
    buckets: Vec<KvStatsBucket>,
    current_size: usize,
    last_key: Vec<u8>,
}

impl KvStatsCollector {
    /// 指定桶字节阈值构造收集器。
    pub(crate) fn new(bucket_size: usize) -> Self {
        Self {
            bucket_size,
            buckets: Vec::new(),
            current_size: 0,
            last_key: Vec::new(),
        }
    }

    /// 累加一对 KV；达到阈值则封桶。
    pub(crate) fn add(&mut self, key: &[u8], value: &[u8]) {
        self.current_size += key.len() + value.len();
        self.last_key.clear();
        self.last_key.extend_from_slice(key);
        if self.current_size >= self.bucket_size {
            self.add_bucket();
        }
    }

    /// 以当前 last_key 为上界封一个桶。
    fn add_bucket(&mut self) {
        self.buckets.push(KvStatsBucket {
            size: self.current_size,
            upper_bound: self.last_key.clone(),
        });
        self.current_size = 0;
    }

    /// 封尾桶并把 JSON 写入用户属性表。
    pub(crate) fn finish(mut self, user_properties: &mut HashMap<String, String>) -> Result<()> {
        if self.current_size > 0 {
            self.add_bucket();
        }
        let stats = KvStats {
            histogram: self.buckets,
        };
        user_properties.insert(KV_STATS_PROP_KEY.to_owned(), serde_json::to_string(&stats)?);
        Ok(())
    }
}

/// DiskSorter 共享状态：目录、编号分配、状态机与文件列表。
struct DiskSorterInner {
    opts: DiskSorterOptions,
    dirname: PathBuf,
    id_alloc: AtomicU64,
    state: AtomicI32,
    pending_files: Mutex<Vec<FileMetadata>>,
    ordered_files: RwLock<Vec<FileMetadata>>,
}

#[derive(Clone)]
/// 磁盘外排器；可 Clone 共享同一 `inner`。
pub struct DiskSorter {
    inner: Arc<DiskSorterInner>,
}

/// 按六位编号生成 SST 路径（超出六位则更长）。
pub fn make_filename(dirname: &Path, file_num: u64) -> PathBuf {
    dirname.join(format!("{file_num:06}{SST_FILE_SUFFIX}"))
}

/// 从 `*.sst` 文件名解析编号；非标准名返回 None。
pub fn parse_filename(filename: &Path) -> Option<u64> {
    let name = filename.file_name()?.to_str()?;
    name.strip_suffix(SST_FILE_SUFFIX)?.parse().ok()
}

/// 将消息包装为 io::Error::other 再装箱。
fn other_error(message: impl Into<String>) -> Error {
    io::Error::other(message.into()).into()
}

/// 在系统临时目录下生成唯一工作目录。
fn ephemeral_directory() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("tidb-extsort-{}-{nonce}", std::process::id()))
}

/// 打开或创建 DiskSorter；空路径则用临时目录。恢复已有 SST 与 sorted 标记。
pub fn open_disk_sorter(dirname: impl AsRef<Path>, opts: DiskSorterOptions) -> Result<DiskSorter> {
    let supplied = dirname.as_ref();
    // 空路径 → 临时目录；否则使用调用方目录。
    let dirname = if supplied.as_os_str().is_empty() {
        ephemeral_directory()
    } else {
        supplied.to_path_buf()
    };
    fs::create_dir_all(&dirname)?;

    // 清理残留 .tmp，扫描 .sst；有 sorted 标记则进入已排序态。
    let mut files = Vec::new();
    let mut max_file_num = 0;
    for entry in fs::read_dir(&dirname)? {
        let path = entry?.path();
        if path.extension().is_some_and(|ext| ext == "tmp") {
            let _ = fs::remove_file(path);
            continue;
        }
        let Some(file_num) = parse_filename(&path) else {
            continue;
        };
        max_file_num = max_file_num.max(file_num);
        files.push(read_file_metadata(&path, file_num)?);
    }
    let sorted = dirname.join(DISK_SORTER_SORTED_FILE).is_file();
    files.sort_by(|a, b| a.start_key.cmp(&b.start_key));
    let (pending_files, ordered_files) = if sorted {
        (Vec::new(), files)
    } else {
        (files, Vec::new())
    };

    Ok(DiskSorter {
        inner: Arc::new(DiskSorterInner {
            opts: opts.ensure_defaults(),
            dirname,
            id_alloc: AtomicU64::new(max_file_num),
            state: AtomicI32::new(if sorted {
                DISK_SORTER_STATE_SORTED
            } else {
                DISK_SORTER_STATE_WRITING
            }),
            pending_files: Mutex::new(pending_files),
            ordered_files: RwLock::new(ordered_files),
        }),
    })
}

/// 读小端 u64；EOF 返回 None。
fn read_u64(reader: &mut impl Read) -> io::Result<Option<u64>> {
    let mut bytes = [0; 8];
    let read = reader.read(&mut bytes[..1])?;
    if read == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut bytes[1..])?;
    Ok(Some(u64::from_le_bytes(bytes)))
}

/// 校验魔数并顺序解码全部 KV。
fn read_file(path: &Path) -> Result<Vec<KeyValue>> {
    let mut reader = BufReader::new(File::open(path)?);
    let mut magic = [0; 8];
    reader.read_exact(&mut magic)?;
    if &magic != FILE_MAGIC {
        return Err(other_error(format!(
            "invalid external-sort file: {}",
            path.display()
        )));
    }
    let mut kvs = Vec::new();
    while let Some(key_len) = read_u64(&mut reader)? {
        let value_len = read_u64(&mut reader)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "missing value length"))?;
        let key_len = usize::try_from(key_len)?;
        let value_len = usize::try_from(value_len)?;
        let mut key = vec![0; key_len];
        let mut value = vec![0; value_len];
        reader.read_exact(&mut key)?;
        reader.read_exact(&mut value)?;
        kvs.push(KeyValue { key, value });
    }
    Ok(kvs)
}

/// 从内存 KV 列表构建 KvStats。
fn collect_stats(kvs: &[KeyValue], bucket_size: usize) -> KvStats {
    let mut collector = KvStatsCollector::new(bucket_size);
    for kv in kvs {
        collector.add(&kv.key, &kv.value);
    }
    let mut properties = HashMap::new();
    collector
        .finish(&mut properties)
        .expect("serializing KV stats");
    serde_json::from_str(&properties[KV_STATS_PROP_KEY]).expect("deserializing KV stats")
}

/// 由有序 KV 推导文件元数据；`end_key` 为 last_key 追加 0 字节（半开区间）。
fn metadata(file_num: u64, kvs: &[KeyValue], bucket_size: usize) -> FileMetadata {
    let start_key = kvs.first().map_or_else(Vec::new, |kv| kv.key.clone());
    let last_key = kvs.last().map_or_else(Vec::new, |kv| kv.key.clone());
    let mut end_key = last_key.clone();
    end_key.push(0);
    FileMetadata {
        file_num,
        start_key,
        end_key,
        last_key,
        kv_stats: collect_stats(kvs, bucket_size),
    }
}

/// 严格递增写入 SST；成功后原子 rename，并回调元数据。
pub(crate) struct SstWriter {
    dirname: PathBuf,
    file_num: u64,
    bucket_size: usize,
    kvs: Vec<KeyValue>,
    failed: Option<String>,
    closed: bool,
    on_success: Option<Box<dyn FnMut(FileMetadata)>>,
}

impl SstWriter {
    /// 创建临时文件并准备写入。
    pub(crate) fn new(
        dirname: impl AsRef<Path>,
        file_num: u64,
        bucket_size: usize,
        on_success: Option<Box<dyn FnMut(FileMetadata)>>,
    ) -> Result<Self> {
        let dirname = dirname.as_ref().to_path_buf();
        fs::create_dir_all(&dirname)?;
        File::create(PathBuf::from(format!(
            "{}{}",
            make_filename(&dirname, file_num).display(),
            TMP_FILE_SUFFIX
        )))?;
        Ok(Self {
            dirname,
            file_num,
            bucket_size,
            kvs: Vec::new(),
            failed: None,
            closed: false,
            on_success,
        })
    }

    /// 追加严格大于上一 key 的记录。
    pub(crate) fn set(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        if self.closed {
            return Err(other_error("SST writer is closed"));
        }
        if self.failed.is_some() {
            return Err(other_error("SST writer is in an error state"));
        }
        if self
            .kvs
            .last()
            .is_some_and(|previous| previous.key.as_slice() >= key)
        {
            let message = "SST keys must be added in strictly increasing order".to_owned();
            self.failed = Some(message.clone());
            return Err(other_error(message));
        }
        self.kvs.push(KeyValue {
            key: key.to_vec(),
            value: value.to_vec(),
        });
        Ok(())
    }

    /// 落盘并 rename；失败则清理临时文件。
    pub(crate) fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let destination = make_filename(&self.dirname, self.file_num);
        let temporary = PathBuf::from(format!("{}{}", destination.display(), TMP_FILE_SUFFIX));
        if let Some(message) = self.failed.take() {
            let _ = fs::remove_file(temporary);
            return Err(other_error(message));
        }
        if let Err(err) = write_file_contents(&temporary, &self.kvs)
            .and_then(|()| fs::rename(&temporary, &destination).map_err(Into::into))
        {
            let _ = fs::remove_file(temporary);
            return Err(err);
        }
        if let Some(on_success) = self.on_success.as_mut() {
            on_success(metadata(self.file_num, &self.kvs, self.bucket_size));
        }
        Ok(())
    }
}

/// 写魔数 + 长度前缀 KV 并 sync。
fn write_file_contents(path: &Path, kvs: &[KeyValue]) -> Result<()> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(FILE_MAGIC)?;
    for kv in kvs {
        writer.write_all(&(kv.key.len() as u64).to_le_bytes())?;
        writer.write_all(&(kv.value.len() as u64).to_le_bytes())?;
        writer.write_all(&kv.key)?;
        writer.write_all(&kv.value)?;
    }
    writer.into_inner()?.sync_all()?;
    Ok(())
}

/// 经 SstWriter 原子写出并返回元数据。
fn write_file_atomic(dirname: &Path, file_num: u64, kvs: &[KeyValue]) -> Result<FileMetadata> {
    let captured = Arc::new(Mutex::new(None));
    let callback_slot = Arc::clone(&captured);
    let mut writer = SstWriter::new(
        dirname,
        file_num,
        DEFAULT_KV_STATS_BUCKET_SIZE,
        Some(Box::new(move |metadata| {
            *callback_slot.lock().unwrap() = Some(metadata);
        })),
    )?;
    for kv in kvs {
        writer.set(&kv.key, &kv.value)?;
    }
    writer.close()?;
    let metadata = captured
        .lock()
        .unwrap()
        .take()
        .expect("SST writer callback must supply metadata");
    Ok(metadata)
}

/// 读取文件内容并重建元数据。
fn read_file_metadata(path: &Path, file_num: u64) -> Result<FileMetadata> {
    let kvs = read_file(path)?;
    Ok(metadata(file_num, &kvs, DEFAULT_KV_STATS_BUCKET_SIZE))
}

/// 单个 SST 的只读句柄；关闭后禁止再开迭代器。
pub(crate) struct SstReader {
    path: PathBuf,
    closed: AtomicBool,
}

impl SstReader {
    /// 将文件加载为内存 VecIterator。
    pub(crate) fn new_iter(&self) -> Result<Box<dyn Iterator>> {
        if self.closed.load(AtomicOrdering::Acquire) {
            return Err(other_error("SST reader is closed"));
        }
        let kvs = read_file(&self.path)?
            .into_iter()
            .map(|kv| (kv.key, kv.value))
            .collect();
        Ok(Box::new(VecIterator::new(kvs)))
    }

    /// 标记读者已关闭。
    fn close(&self) {
        self.closed.store(true, AtomicOrdering::Release);
    }
}

/// 池中条目：读者与引用计数。
struct ReaderEntry {
    reader: Arc<SstReader>,
    refs: usize,
}

/// 按 file_num 复用 SstReader 的引用计数池。
pub(crate) struct SstReaderPool {
    dirname: PathBuf,
    readers: Mutex<HashMap<u64, ReaderEntry>>,
}

impl SstReaderPool {
    /// 绑定数据目录。
    pub(crate) fn new(dirname: impl AsRef<Path>) -> Self {
        Self {
            dirname: dirname.as_ref().to_path_buf(),
            readers: Mutex::new(HashMap::new()),
        }
    }

    /// 获取或打开读者并增加引用。
    pub(crate) fn get(&self, file_num: u64) -> Result<Arc<SstReader>> {
        let mut readers = self.readers.lock().unwrap();
        if let Some(entry) = readers.get_mut(&file_num) {
            entry.refs += 1;
            return Ok(Arc::clone(&entry.reader));
        }
        let path = make_filename(&self.dirname, file_num);
        let _ = read_file(&path)?;
        let reader = Arc::new(SstReader {
            path,
            closed: AtomicBool::new(false),
        });
        readers.insert(
            file_num,
            ReaderEntry {
                reader: Arc::clone(&reader),
                refs: 1,
            },
        );
        Ok(reader)
    }

    /// 减少引用；归零则关闭并移出池。
    pub(crate) fn unref(&self, file_num: u64) -> Result<()> {
        let mut readers = self.readers.lock().unwrap();
        let entry = readers.get_mut(&file_num).unwrap_or_else(|| {
            panic!("SstReaderPool: unref a reader that does not exist: {file_num}")
        });
        entry.refs -= 1;
        if entry.refs == 0 {
            let entry = readers.remove(&file_num).unwrap();
            entry.reader.close();
        }
        Ok(())
    }

    /// 当前池中打开的读者数。
    pub(crate) fn reader_count(&self) -> usize {
        self.readers.lock().unwrap().len()
    }
}

impl ExternalSorter for DiskSorter {
    /// 仅在 WRITING 状态允许新建 Writer。
    fn new_writer(&self, ctx: &CancellationToken) -> Result<Box<dyn Writer>> {
        if ctx.is_cancelled() {
            return Err(other_error("external sorter writer creation cancelled"));
        }
        if self.inner.state.load(AtomicOrdering::Acquire) != DISK_SORTER_STATE_WRITING {
            return Err(other_error(
                "diskSorter started sorting, cannot write more data",
            ));
        }
        Ok(Box::new(DiskSorterWriter {
            inner: Arc::clone(&self.inner),
            kvs: Vec::new(),
            buffered_bytes: 0,
            closed: false,
        }))
    }

    /// CAS 进入 SORTING；失败回退 WRITING 以便重试。
    fn sort(&self, ctx: &CancellationToken) -> Result<()> {
        if self.is_sorted() {
            return Ok(());
        }
        self.inner
            .state
            .compare_exchange(
                DISK_SORTER_STATE_WRITING,
                DISK_SORTER_STATE_SORTING,
                AtomicOrdering::AcqRel,
                AtomicOrdering::Acquire,
            )
            .map_err(|_| other_error("diskSorter is already sorting"))?;
        let result = self.do_sort(ctx);
        if result.is_err() {
            self.inner
                .state
                .store(DISK_SORTER_STATE_WRITING, AtomicOrdering::Release);
        }
        result
    }

    fn is_sorted(&self) -> bool {
        self.inner.state.load(AtomicOrdering::Acquire) == DISK_SORTER_STATE_SORTED
    }

    /// 已排序后打开 MergingIter；通过 ReaderPool 管理 SST 生命周期。
    fn new_iterator(&self, ctx: &CancellationToken) -> Result<Box<dyn Iterator>> {
        if ctx.is_cancelled() {
            return Err(other_error("external sorter iterator creation cancelled"));
        }
        if !self.is_sorted() {
            return Err(other_error("diskSorter is not sorted"));
        }
        let files = self.inner.ordered_files.read().unwrap().clone();
        let pool = Arc::new(SstReaderPool::new(&self.inner.dirname));
        let open_pool = Arc::clone(&pool);
        let open_iter: OpenIter = Box::new(move |file| {
            let reader = open_pool.get(file.file_num)?;
            let raw_iter = match reader.new_iter() {
                Ok(iter) => iter,
                Err(err) => {
                    open_pool.unref(file.file_num)?;
                    return Err(err);
                }
            };
            let close_pool = Arc::clone(&open_pool);
            let file_num = file.file_num;
            Ok(Box::new(SstIter::new(
                raw_iter,
                Some(Box::new(move || close_pool.unref(file_num))),
            )))
        });
        Ok(Box::new(MergingIter::new(files, open_iter)))
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }

    /// 删除整个工作目录。
    fn close_and_cleanup(&self) -> Result<()> {
        match fs::remove_dir_all(&self.inner.dirname) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}

impl DiskSorter {
    /// 对 pending SST 按 Go 的重叠阈值执行必要压缩，并落 sorted 标记。
    fn do_sort(&self, ctx: &CancellationToken) -> Result<()> {
        if ctx.is_cancelled() {
            return Err(other_error("external sort cancelled"));
        }
        let mut pending = self.inner.pending_files.lock().unwrap();
        let mut ordered = pending.clone();
        ordered.sort_by(|a, b| a.start_key.cmp(&b.start_key));
        loop {
            let files = pick_compaction_files(&ordered, self.inner.opts.compaction_threshold);
            if files.is_empty() {
                break;
            }
            self.compact_files(ctx, &mut ordered, files)?;
        }

        // 原子写入 sorted 标记，便于 reopen 恢复。
        let marker_tmp = self.inner.dirname.join("sorted.tmp");
        let marker = self.inner.dirname.join(DISK_SORTER_SORTED_FILE);
        File::create(&marker_tmp)?.sync_all()?;
        fs::rename(marker_tmp, marker)?;
        *self.inner.ordered_files.write().unwrap() = ordered;
        pending.clear();
        self.inner
            .state
            .store(DISK_SORTER_STATE_SORTED, AtomicOrdering::Release);
        Ok(())
    }

    /// 将达到重叠阈值的文件按深度和估算大小切分后压缩。
    fn compact_files(
        &self,
        ctx: &CancellationToken,
        ordered: &mut Vec<FileMetadata>,
        files: Vec<FileMetadata>,
    ) -> Result<()> {
        let compactions: Vec<_> =
            split_compaction_files(files.clone(), self.inner.opts.max_compaction_depth)
                .into_iter()
                .flat_map(|group| build_compactions(&group, self.inner.opts.max_compaction_size))
                .collect();

        let mut references: HashMap<u64, usize> =
            files.iter().map(|file| (file.file_num, 0)).collect();
        for compaction in &compactions {
            for file in &compaction.overlap_files {
                *references
                    .get_mut(&file.file_num)
                    .expect("compaction references only selected files") += 1;
            }
        }

        let mut outputs = Vec::with_capacity(compactions.len());
        let mut removed = Vec::new();
        for batch in compactions.chunks(self.inner.opts.concurrency.max(1)) {
            if ctx.is_cancelled() {
                return Err(other_error("external sort cancelled"));
            }
            let batch_outputs = std::thread::scope(|scope| {
                let handles: Vec<_> = batch
                    .iter()
                    .map(|compaction| scope.spawn(move || self.run_compaction(ctx, compaction)))
                    .collect();
                handles
                    .into_iter()
                    .map(|handle| {
                        handle
                            .join()
                            .map_err(|_| other_error("external sort compaction worker panicked"))?
                    })
                    .collect::<Result<Vec<_>>>()
            })?;
            outputs.extend(batch_outputs);
            for compaction in batch {
                for file in &compaction.overlap_files {
                    let refs = references
                        .get_mut(&file.file_num)
                        .expect("compaction references only selected files");
                    *refs -= 1;
                    if *refs == 0 {
                        fs::remove_file(make_filename(&self.inner.dirname, file.file_num))?;
                        removed.push(file.file_num);
                    }
                }
            }
        }

        ordered.retain(|file| !removed.contains(&file.file_num));
        ordered.extend(outputs);
        ordered.sort_by(|a, b| a.start_key.cmp(&b.start_key));
        Ok(())
    }

    /// 归并一个半开键区间，跨文件去重后写为新 SST。
    fn run_compaction(
        &self,
        ctx: &CancellationToken,
        compaction: &Compaction,
    ) -> Result<FileMetadata> {
        let pool = Arc::new(SstReaderPool::new(&self.inner.dirname));
        let open_pool = Arc::clone(&pool);
        let open_iter: OpenIter = Box::new(move |file| {
            let reader = open_pool.get(file.file_num)?;
            let raw_iter = match reader.new_iter() {
                Ok(iter) => iter,
                Err(err) => {
                    open_pool.unref(file.file_num)?;
                    return Err(err);
                }
            };
            let close_pool = Arc::clone(&open_pool);
            let file_num = file.file_num;
            Ok(Box::new(SstIter::new(
                raw_iter,
                Some(Box::new(move || close_pool.unref(file_num))),
            )))
        });
        let mut iter = MergingIter::new(compaction.overlap_files.clone(), open_iter);
        let mut records = Vec::new();
        if iter.seek(&compaction.start_key) {
            while iter.valid() && iter.unsafe_key() < compaction.end_key.as_slice() {
                if records.len() % 1000 == 0 && ctx.is_cancelled() {
                    iter.close()?;
                    return Err(other_error("external sort cancelled"));
                }
                records.push(KeyValue {
                    key: iter.unsafe_key().to_vec(),
                    value: iter.unsafe_value().to_vec(),
                });
                iter.next();
            }
        }
        if let Some(err) = iter.take_error() {
            let _ = iter.close();
            return Err(err);
        }
        iter.close()?;
        let file_num = self.inner.id_alloc.fetch_add(1, AtomicOrdering::AcqRel) + 1;
        write_file_atomic(&self.inner.dirname, file_num, &records)
    }
}

/// ExternalSorter::Writer：缓冲排序后刷成 pending SST。
struct DiskSorterWriter {
    inner: Arc<DiskSorterInner>,
    kvs: Vec<KeyValue>,
    buffered_bytes: usize,
    closed: bool,
}

impl DiskSorterWriter {
    /// 排序去重当前缓冲并追加到 pending_files。
    fn flush_inner(&mut self) -> Result<()> {
        if self.kvs.is_empty() {
            return Ok(());
        }
        self.kvs.sort_by(|a, b| a.key.cmp(&b.key));
        self.kvs.dedup_by(|later, earlier| later.key == earlier.key);
        let file_num = self.inner.id_alloc.fetch_add(1, AtomicOrdering::AcqRel) + 1;
        let meta = write_file_atomic(&self.inner.dirname, file_num, &self.kvs)?;
        self.inner.pending_files.lock().unwrap().push(meta);
        self.kvs.clear();
        self.buffered_bytes = 0;
        Ok(())
    }
}

impl Writer for DiskSorterWriter {
    /// 缓冲写入；超过 writer_buffer_size 先 flush。
    fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        if self.closed {
            return Err(other_error("writer is closed"));
        }
        if self.inner.state.load(AtomicOrdering::Acquire) != DISK_SORTER_STATE_WRITING {
            return Err(other_error(
                "diskSorter started sorting, cannot write more data",
            ));
        }
        let incoming = key.len() + value.len();
        if !self.kvs.is_empty()
            && self.buffered_bytes + incoming > self.inner.opts.writer_buffer_size
        {
            self.flush_inner()?;
        }
        self.kvs.push(KeyValue {
            key: key.to_vec(),
            value: value.to_vec(),
        });
        self.buffered_bytes += incoming;
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        if self.closed {
            return Err(other_error("writer is closed"));
        }
        self.flush_inner()
    }

    /// flush 后标记 closed。
    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.flush_inner()?;
        self.closed = true;
        Ok(())
    }
}

/// 基于内存向量的有序迭代器。
struct VecIterator {
    kvs: Vec<(Vec<u8>, Vec<u8>)>,
    position: Option<usize>,
    closed: bool,
}

impl VecIterator {
    /// 构造未定位的迭代器。
    fn new(kvs: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
        Self {
            kvs,
            position: None,
            closed: false,
        }
    }

    /// 设置位置；越界则无效。
    fn set_position(&mut self, position: Option<usize>) -> bool {
        self.position = position.filter(|index| *index < self.kvs.len());
        self.position.is_some()
    }
}

impl Iterator for VecIterator {
    fn seek(&mut self, key: &[u8]) -> bool {
        if self.closed {
            return false;
        }
        let position = self
            .kvs
            .partition_point(|(candidate, _)| candidate.as_slice() < key);
        self.set_position(Some(position))
    }

    fn first(&mut self) -> bool {
        if self.closed {
            return false;
        }
        self.set_position(Some(0))
    }

    fn next(&mut self) -> bool {
        if self.closed {
            return false;
        }
        self.set_position(self.position.map(|position| position + 1))
    }

    fn last(&mut self) -> bool {
        if self.closed || self.kvs.is_empty() {
            self.position = None;
            return false;
        }
        self.set_position(Some(self.kvs.len() - 1))
    }

    fn valid(&self) -> bool {
        !self.closed
            && self
                .position
                .is_some_and(|position| position < self.kvs.len())
    }

    fn error(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        None
    }

    fn take_error(&mut self) -> Option<Error> {
        None
    }

    fn unsafe_key(&self) -> &[u8] {
        &self.kvs[self.position.expect("iterator is not valid")].0
    }

    fn unsafe_value(&self) -> &[u8] {
        &self.kvs[self.position.expect("iterator is not valid")].1
    }

    fn close(&mut self) -> Result<()> {
        self.closed = true;
        self.position = None;
        Ok(())
    }
}

/// 包装底层 Iterator，关闭时执行池 unref 回调。
pub(crate) struct SstIter {
    iter: Box<dyn Iterator>,
    on_close: Option<Box<dyn FnMut() -> Result<()> + Send>>,
    closed: bool,
    close_error: Option<Error>,
}

impl SstIter {
    /// 绑定底层迭代器与可选关闭回调。
    pub(crate) fn new(
        iter: Box<dyn Iterator>,
        on_close: Option<Box<dyn FnMut() -> Result<()> + Send>>,
    ) -> Self {
        Self {
            iter,
            on_close,
            closed: false,
            close_error: None,
        }
    }
}

impl Iterator for SstIter {
    fn seek(&mut self, key: &[u8]) -> bool {
        !self.closed && self.iter.seek(key)
    }

    fn first(&mut self) -> bool {
        !self.closed && self.iter.first()
    }

    fn next(&mut self) -> bool {
        !self.closed && self.iter.next()
    }

    fn last(&mut self) -> bool {
        !self.closed && self.iter.last()
    }

    fn valid(&self) -> bool {
        !self.closed && self.iter.valid()
    }

    fn error(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.close_error.as_deref().or_else(|| self.iter.error())
    }

    fn take_error(&mut self) -> Option<Error> {
        self.close_error.take().or_else(|| self.iter.take_error())
    }

    fn unsafe_key(&self) -> &[u8] {
        self.iter.unsafe_key()
    }

    fn unsafe_value(&self) -> &[u8] {
        self.iter.unsafe_value()
    }

    fn close(&mut self) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let iter_result = self.iter.close();
        let callback_result = self.on_close.as_mut().map_or(Ok(()), |callback| callback());
        match (iter_result, callback_result) {
            (Err(err), _) | (Ok(()), Err(err)) => Err(err),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

/// 归并堆中一项：打开的迭代器及其文件下标。
struct MergingIterItem {
    iter: Box<dyn Iterator>,
    index: usize,
}

/// 按文件元数据打开迭代器的回调类型。
type OpenIter = Box<dyn Fn(&FileMetadata) -> Result<Box<dyn Iterator>> + Send + Sync + 'static>;

/// 多文件多路归并迭代器（按 start_key 有序的半开区间文件集）。
pub(crate) struct MergingIter {
    ordered_files: Vec<FileMetadata>,
    open_iter: OpenIter,
    opened: Vec<MergingIterItem>,
    next_file_index: usize,
    error: Option<Error>,
}

impl MergingIter {
    /// 要求 `ordered_files` 已按 start_key 非降序。
    pub(crate) fn new(ordered_files: Vec<FileMetadata>, open_iter: OpenIter) -> Self {
        assert!(
            ordered_files
                .windows(2)
                .all(|files| files[0].start_key <= files[1].start_key),
            "MergingIter: ordered_files are not ordered by start key"
        );
        Self {
            ordered_files,
            open_iter,
            opened: Vec::new(),
            next_file_index: 0,
            error: None,
        }
    }

    /// 若子迭代器有错则记录到 self.error。
    fn remember_iterator_error(&mut self, index: usize) {
        if let Some(err) = self.opened[index].iter.take_error() {
            self.error = Some(err);
        }
    }

    /// 关闭所有已打开子迭代器。
    fn close_all(&mut self) {
        for mut item in self.opened.drain(..) {
            if let Err(err) = item.iter.close() {
                if self.error.is_none() {
                    self.error = Some(err);
                }
            }
        }
    }

    /// 在有效子迭代器中选 key 最小（并列取较小 index）的项。
    fn current_index(&self) -> Option<usize> {
        self.opened
            .iter()
            .enumerate()
            .filter(|(_, item)| item.iter.valid())
            .min_by(|(_, left), (_, right)| {
                left.iter
                    .unsafe_key()
                    .cmp(right.iter.unsafe_key())
                    .then(left.index.cmp(&right.index))
            })
            .map(|(index, _)| index)
    }

    /// 只打开起点可能不大于当前最小 key 的后续文件。
    fn maybe_open_next_files(&mut self) -> bool {
        while self.next_file_index < self.ordered_files.len() {
            let file = &self.ordered_files[self.next_file_index];
            if let Some(current) = self.current_index()
                && self.opened[current].iter.unsafe_key() < file.start_key.as_slice()
            {
                break;
            }
            let mut iter = match (self.open_iter)(file) {
                Ok(iter) => iter,
                Err(err) => {
                    self.error = Some(err);
                    return false;
                }
            };
            let index = self.next_file_index;
            self.next_file_index += 1;
            if iter.first() {
                self.opened.push(MergingIterItem { iter, index });
            } else {
                self.error = join_errors(iter.take_error(), iter.close());
                if self.error.is_some() {
                    return false;
                }
            }
        }
        self.current_index().is_some()
    }
}

impl Iterator for MergingIter {
    fn seek(&mut self, key: &[u8]) -> bool {
        self.error = None;
        self.close_all();
        if self.error.is_some() {
            return false;
        }
        self.next_file_index = self.ordered_files.len();
        for (index, file) in self.ordered_files.iter().enumerate() {
            if file.start_key.as_slice() > key {
                self.next_file_index = index;
                break;
            }
            if file.end_key.as_slice() <= key {
                continue;
            }
            let mut iter = match (self.open_iter)(file) {
                Ok(iter) => iter,
                Err(err) => {
                    self.error = Some(err);
                    return false;
                }
            };
            if iter.seek(key) {
                self.opened.push(MergingIterItem { iter, index });
            } else {
                self.error = join_errors(iter.take_error(), iter.close());
                if self.error.is_some() {
                    return false;
                }
            }
        }
        self.maybe_open_next_files()
    }

    fn first(&mut self) -> bool {
        self.seek(&[])
    }

    fn next(&mut self) -> bool {
        self.error = None;
        let Some(current) = self.current_index() else {
            return false;
        };
        // 推进所有位于当前 key 的子迭代器，实现跨文件去重。
        let key = self.opened[current].iter.unsafe_key().to_vec();
        let mut index = 0;
        while index < self.opened.len() {
            if self.opened[index].iter.valid()
                && self.opened[index].iter.unsafe_key() == key.as_slice()
                && !self.opened[index].iter.next()
            {
                self.remember_iterator_error(index);
                if self.error.is_some() {
                    return false;
                }
                let mut item = self.opened.remove(index);
                if let Err(err) = item.iter.close() {
                    self.error = Some(err);
                    return false;
                }
                continue;
            }
            index += 1;
        }
        self.error.is_none() && self.maybe_open_next_files()
    }

    fn last(&mut self) -> bool {
        self.error = None;
        self.close_all();
        if self.error.is_some() {
            return false;
        }
        self.next_file_index = self.ordered_files.len();
        let mut files: Vec<_> = self.ordered_files.iter().enumerate().collect();
        files.sort_by(|(_, left), (_, right)| right.last_key.cmp(&left.last_key));
        for (index, file) in files {
            let mut iter = match (self.open_iter)(file) {
                Ok(iter) => iter,
                Err(err) => {
                    self.error = Some(err);
                    return false;
                }
            };
            if iter.last() {
                self.opened.push(MergingIterItem { iter, index });
                break;
            } else {
                self.error = join_errors(iter.take_error(), iter.close());
                if self.error.is_some() {
                    return false;
                }
            }
        }
        self.error.is_none() && self.current_index().is_some()
    }

    fn valid(&self) -> bool {
        self.error.is_none() && self.current_index().is_some()
    }

    fn error(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.error.as_deref()
    }

    fn take_error(&mut self) -> Option<Error> {
        self.error.take()
    }

    fn unsafe_key(&self) -> &[u8] {
        self.opened[self.current_index().expect("iterator is not valid")]
            .iter
            .unsafe_key()
    }

    fn unsafe_value(&self) -> &[u8] {
        self.opened[self.current_index().expect("iterator is not valid")]
            .iter
            .unsafe_value()
    }

    fn close(&mut self) -> Result<()> {
        self.error = None;
        self.close_all();
        self.error.take().map_or(Ok(()), Err)
    }
}

/// 扫描线统计重叠深度，返回深度达到 threshold 的文件集合。
/// Sweep-line selection from the Go implementation. File ranges are half-open.
pub fn pick_compaction_files(files: &[FileMetadata], threshold: usize) -> Vec<FileMetadata> {
    if files.is_empty() {
        return Vec::new();
    }
    // 半开区间 [start,end)：起点 +1、终点 -1，同 key 先减后加以处理相邻相接。
    let mut boundaries: Vec<(Vec<u8>, i64)> = Vec::with_capacity(files.len() * 2);
    for file in files {
        boundaries.push((file.start_key.clone(), 1));
        boundaries.push((file.end_key.clone(), -1));
    }
    boundaries.sort_by(|a, b| match a.0.cmp(&b.0) {
        Ordering::Equal => a.1.cmp(&b.1),
        order => order,
    });
    // 压缩为每个边界点上的深度序列。
    let mut intervals: Vec<(Vec<u8>, i64)> = Vec::new();
    let mut depth = 0;
    let mut i = 0;
    while i < boundaries.len() {
        let key = boundaries[i].0.clone();
        while i < boundaries.len() && boundaries[i].0 == key {
            depth += boundaries[i].1;
            i += 1;
        }
        intervals.push((key, depth));
    }
    if intervals.iter().map(|(_, depth)| *depth).max().unwrap_or(0) < threshold as i64 {
        return Vec::new();
    }
    files
        .iter()
        .filter(|file| {
            intervals.iter().any(|(key, depth)| {
                *depth >= threshold as i64 && *key >= file.start_key && *key < file.end_key
            })
        })
        .cloned()
        .collect()
}

/// 按重叠连通分量切分，再按 max_compaction_depth 限深分组。
pub fn split_compaction_files(
    mut files: Vec<FileMetadata>,
    max_compaction_depth: usize,
) -> Vec<Vec<FileMetadata>> {
    if files.is_empty() {
        return Vec::new();
    }
    files.sort_by(|a, b| a.start_key.cmp(&b.start_key));
    let mut overlap_groups = Vec::new();
    let mut current = vec![files[0].clone()];
    let mut max_end = files[0].end_key.clone();
    for file in files.into_iter().skip(1) {
        if file.start_key >= max_end {
            overlap_groups.push(current);
            current = vec![file.clone()];
        } else {
            current.push(file.clone());
        }
        max_end = max_end.max(file.end_key);
    }
    overlap_groups.push(current);

    let mut result = Vec::new();
    for group in overlap_groups {
        let group_count = group.len().div_ceil(max_compaction_depth);
        let group_size = group.len().div_ceil(group_count);
        result.extend(group.chunks(group_size).map(<[FileMetadata]>::to_vec));
    }
    result
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 一次压缩任务：键范围及重叠文件。
pub struct Compaction {
    pub start_key: Vec<u8>,
    pub end_key: Vec<u8>,
    pub overlap_files: Vec<FileMetadata>,
}

/// 按 KvStats 直方图体积切分压缩区间，并收集重叠文件。
pub fn build_compactions(files: &[FileMetadata], max_compaction_size: usize) -> Vec<Compaction> {
    if files.is_empty() {
        return Vec::new();
    }
    let start = files
        .iter()
        .map(|file| &file.start_key)
        .min()
        .unwrap()
        .clone();
    let end = files
        .iter()
        .map(|file| &file.end_key)
        .max()
        .unwrap()
        .clone();
    let mut buckets: Vec<KvStatsBucket> = files
        .iter()
        .flat_map(|file| file.kv_stats.histogram.iter().cloned())
        .collect();
    if buckets.is_empty() {
        let mut overlap_files = files.to_vec();
        overlap_files.sort_by(|a, b| a.start_key.cmp(&b.start_key));
        return vec![Compaction {
            start_key: start,
            end_key: end,
            overlap_files,
        }];
    }
    buckets.sort_by(|a, b| a.upper_bound.cmp(&b.upper_bound));
    let mut merged: Vec<KvStatsBucket> = Vec::new();
    for bucket in buckets {
        if let Some(last) = merged
            .last_mut()
            .filter(|last| last.upper_bound == bucket.upper_bound)
        {
            last.size += bucket.size;
        } else {
            merged.push(bucket);
        }
    }

    let mut ranges = Vec::new();
    let mut range_start = start;
    let mut size = 0;
    for (index, bucket) in merged.iter().enumerate() {
        if index + 1 == merged.len() {
            ranges.push((range_start.clone(), end.clone()));
            break;
        }
        size += bucket.size;
        if size >= max_compaction_size {
            ranges.push((range_start.clone(), bucket.upper_bound.clone()));
            range_start = bucket.upper_bound.clone();
            size = 0;
        }
    }
    ranges
        .into_iter()
        .map(|(start_key, end_key)| {
            let mut overlap_files: Vec<_> = files
                .iter()
                .filter(|file| file.end_key > start_key && file.start_key < end_key)
                .cloned()
                .collect();
            overlap_files.sort_by(|a, b| a.start_key.cmp(&b.start_key));
            Compaction {
                start_key,
                end_key,
                overlap_files,
            }
        })
        .collect()
}

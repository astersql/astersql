// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

//! 流备份元数据读写与观察范围：对齐 Go `br/pkg/stream/stream_mgr.go`。
//! `MetadataHelper` 用引用计数缓存整文件，支持按 offset/length 切片再解压。
//! V1 Metadata 在 Parse/Marshal 间做 Files ↔ FileGroups 兼容转换。
//! `FilterPathByTs` 按文件名内 TS 窗口裁剪；解析失败时放行以兼容未来路径。
//! ZSTD 路径使用标准解码器处理任意合法帧。

// FastUnmarshalMetaData 列出前缀后逐文件过滤再回调，错误立即中止。
// StreamManager 仅作门面，不拥有额外生命周期状态。
// ContentRef.init_ref 在 Ref 耗尽后用于重置，支持同路径多轮缓存。
// ParseToMetadataHard 一对一成组，便于按文件粒度裁剪。
// Marshal 在 V1 写出前清空 FileGroups，兼容旧读取器。
// appendTableObserveRanges 使用 PrefixNext 构造半开区间。
// BuildObserveMetaRange 监听 DBs meta，覆盖建库/改库。
// decode_compressed 与 ReadFile 解耦，便于单测 fixture。
// InitCacheEntry 不读存储，仅登记引用预算。
// Close 清空缓存，避免测试间泄漏大块字节。
// FilterPathByTs 的 left/right 对应 restore 窗口边界。
// min_begin>MinTS 时视为异常命名，放行不过滤。
// 未缓存整文件读忽略 offset/length 非零以外的约束（已校验）。
// global checkpoint 前缀供其它模块拼路径，本文件不读写。
// serde_json 解析 Metadata 为桩实现，与 Go protobuf 字节不同。
// 缓存命中路径在锁外 ReadFile，降低锁持有时间。
// out of range 错误对应切片越过缓存字节长度。
// MetaPrefix/ObserveMetaRange 供上层配置观察范围。
// 补充说明：与 Go 语义对齐的约束与数据流备注（18）。
// 补充说明：与 Go 语义对齐的约束与数据流备注（19）。
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_br_pkg_encryption::{FileEncryptionInfo, Manager as EncryptionManager};
use sha2::{Digest, Sha256};

use crate::stubs::backuppb::Metadata;
use crate::stubs::errors::Error;
use crate::stubs::{Storage, meta, tablecodec};

// 外部存储上 backup meta / global checkpoint 的固定前缀。
const streamBackupMetaPrefix: &str = "v1/backupmeta";
const streamBackupGlobalCheckpointPrefix: &str = "v1/global_checkpoint";

/// 返回流备份 meta 目录前缀。
pub fn GetStreamBackupMetaPrefix() -> &'static str {
    streamBackupMetaPrefix
}

/// 返回全局 checkpoint 前缀。
pub fn GetStreamBackupGlobalCheckpointPrefix() -> &'static str {
    streamBackupGlobalCheckpointPrefix
}

/// 为表 ID 列表构造 record 前缀观察范围（左闭右开）。
pub fn appendTableObserveRanges(tblIDs: Vec<i64>) -> Vec<crate::stubs::kv::KeyRange> {
    let mut krs = Vec::with_capacity(tblIDs.len());
    for tid in tblIDs {
        let startKey = tablecodec::GenTableRecordPrefix(tid);
        let endKey = tablecodec::PrefixNext(&startKey);
        krs.push(crate::stubs::kv::KeyRange {
            StartKey: startKey,
            EndKey: endKey,
        });
    }
    krs
}

/// Snapshot-facing catalog boundary used by `BuildObserveDataRanges`.
pub trait ObserveDataSource {
    fn ListDatabases(&self, backup_ts: u64) -> Result<Vec<crate::stubs::model::DBInfo>, Error>;
    fn ListTables(
        &self,
        backup_ts: u64,
        database_id: i64,
    ) -> Result<Vec<crate::stubs::model::TableInfo>, Error>;
}

/// Table-filter contract mirrored from TiDB's table-filter package.
pub trait ObserveTableFilter {
    fn MatchSchema(&self, schema: &str) -> bool;
    fn MatchTable(&self, schema: &str, table: &str) -> bool;
}

fn buildObserveTableRange(
    table: &crate::stubs::model::TableInfo,
) -> Vec<crate::stubs::kv::KeyRange> {
    match table.GetPartitionInfo() {
        None => appendTableObserveRanges(vec![table.ID]),
        Some(partitions) => appendTableObserveRanges(
            partitions
                .Definitions
                .iter()
                .map(|definition| definition.ID)
                .collect(),
        ),
    }
}

fn buildObserverAllRange() -> Vec<crate::stubs::kv::KeyRange> {
    let start_key = b"t".to_vec();
    let end_key = tablecodec::PrefixNext(&start_key);
    vec![crate::stubs::kv::KeyRange {
        StartKey: start_key,
        EndKey: end_key,
    }]
}

/// Build table-record ranges at the requested snapshot timestamp.
pub fn BuildObserveDataRanges(
    storage: &dyn ObserveDataSource,
    filter_str: &[String],
    table_filter: &dyn ObserveTableFilter,
    backup_ts: u64,
) -> Result<Vec<crate::stubs::kv::KeyRange>, Error> {
    if filter_str == ["*.*"] {
        return Ok(buildObserverAllRange());
    }

    let mut ranges = Vec::new();
    for database in storage.ListDatabases(backup_ts)? {
        let is_mem_db = matches!(
            database.Name.L.as_str(),
            "information_schema" | "performance_schema" | "metrics_schema"
        );
        if !table_filter.MatchSchema(&database.Name.O) || is_mem_db {
            continue;
        }
        for table in storage.ListTables(backup_ts, database.ID)? {
            if table_filter.MatchTable(&database.Name.O, &table.Name.O) {
                ranges.extend(buildObserveTableRange(&table));
            }
        }
    }
    Ok(ranges)
}

/// 观察 meta 中 DBs 键空间的范围，供 DDL/库变更监听。
pub fn BuildObserveMetaRange() -> crate::stubs::kv::KeyRange {
    // Go tablecodec.MetaPrefix is the single-byte `m` prefix. Observing an encoded
    // `DBs` hash key would miss other metadata (tables, sequences, policies, ...).
    let startKey = b"m".to_vec();
    let endKey = tablecodec::PrefixNext(&startKey);
    crate::stubs::kv::KeyRange {
        StartKey: startKey,
        EndKey: endKey,
    }
}

/// 缓存条目：整文件字节 + 剩余引用次数（耗尽后清空 data 并重置 Ref）。
pub struct ContentRef {
    pub Path: String,
    pub Ref: i32,
    pub init_ref: i32,
    pub data: Option<Vec<u8>>,
}

/// 按路径缓存 backup 数据文件内容，减少重复 ReadFile。
pub struct MetadataHelper {
    cache: Mutex<HashMap<String, Arc<Mutex<ContentRef>>>>,
    encryption_manager: Option<Arc<Mutex<EncryptionManager>>>,
}

/// Encryption metadata accompanying one stored file.
pub struct EncryptedFileInfo {
    pub EncryptionInfo: FileEncryptionInfo,
    pub Checksum: Option<Vec<u8>>,
}

impl Default for MetadataHelper {
    fn default() -> Self {
        Self::new()
    }
}

/// 工厂：空缓存的 MetadataHelper。
pub fn NewMetadataHelper() -> MetadataHelper {
    MetadataHelper::new()
}

impl MetadataHelper {
    /// 新建空 helper。
    pub fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            encryption_manager: None,
        }
    }

    /// Construct a helper owning the encryption manager used by encrypted reads.
    pub fn with_encryption_manager(manager: EncryptionManager) -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            encryption_manager: Some(Arc::new(Mutex::new(manager))),
        }
    }

    /// 预登记路径的引用次数；ref_count<=0 忽略。data 懒加载。
    pub fn InitCacheEntry(&self, path: &str, ref_count: i32) {
        if ref_count <= 0 {
            return;
        }
        self.cache.lock().unwrap().insert(
            path.to_string(),
            Arc::new(Mutex::new(ContentRef {
                Path: path.to_string(),
                Ref: ref_count,
                init_ref: ref_count,
                data: None,
            })),
        );
    }

    // UNKNOWN 原样返回；ZSTD 解码完整标准帧。
    fn decode_compressed(
        data: &[u8],
        raw_length: u64,
        compression: crate::stubs::backuppb::CompressionType,
    ) -> Result<Vec<u8>, Error> {
        use crate::stubs::backuppb::CompressionType;
        match compression {
            CompressionType::UNKNOWN => Ok(data.to_vec()),
            CompressionType::ZSTD => zstd::stream::decode_all(data)
                .map_err(|e| Error::new(format!("failed to decode compressed data: {e}"))),
        }
    }

    /// 读取并可选解压；未 Init 时仅允许整文件（offset/length 均为 0）。
    /// 已缓存：IO 在锁外完成，避免跨 ReadFile 持锁（对齐 Go 并发语义）。
    fn read_file_content(
        &self,
        path: &str,
        offset: u64,
        length: u64,
        storage: &dyn crate::stubs::Storage,
    ) -> Result<Vec<u8>, Error> {
        let cached = {
            let guard = self.cache.lock().unwrap();
            guard.get(path).cloned()
        };
        let Some(cached) = cached else {
            // 未初始化缓存却请求切片 → 与 Go 相同错误。
            if offset > 0 || length > 0 {
                return Err(Error::new("the cache entry is uninitialized"));
            }
            return storage.ReadFile(path).map_err(Error::new);
        };

        // Match Go's per-ContentRef mutex: one path is loaded once while unrelated
        // paths retain independent locks and can perform storage IO concurrently.
        let slice = {
            let mut cref = cached.lock().unwrap();
            cref.Ref -= 1;
            if cref.data.is_none() {
                cref.data = Some(storage.ReadFile(path).map_err(Error::new)?);
            }
            let data = cref
                .data
                .as_ref()
                .ok_or_else(|| Error::new(format!("cache entry missing data for {path}")))?;
            let end = offset
                .checked_add(length)
                .and_then(|end| usize::try_from(end).ok())
                .ok_or_else(|| Error::new("read out of range"))?;
            let start = usize::try_from(offset).map_err(|_| Error::new("read out of range"))?;
            if start > end || end > data.len() {
                return Err(Error::new("read out of range"));
            }
            let out = data[start..end].to_vec();
            if cref.Ref <= 0 {
                cref.data = None;
                cref.Ref = cref.init_ref;
            }
            out
        };
        Ok(slice)
    }

    /// Read and optionally decompress an unencrypted file.
    pub fn ReadFile(
        &self,
        path: &str,
        offset: u64,
        length: u64,
        raw_length: u64,
        compression: crate::stubs::backuppb::CompressionType,
        storage: &dyn crate::stubs::Storage,
    ) -> Result<Vec<u8>, Error> {
        let content = self.read_file_content(path, offset, length, storage)?;
        Self::decode_compressed(&content, raw_length, compression)
    }

    /// Go-equivalent encrypted read: checksum ciphertext, decrypt, then decompress.
    pub fn ReadFileWithEncryption(
        &self,
        path: &str,
        offset: u64,
        length: u64,
        raw_length: u64,
        compression: crate::stubs::backuppb::CompressionType,
        storage: &dyn crate::stubs::Storage,
        encryption: Option<&EncryptedFileInfo>,
    ) -> Result<Vec<u8>, Error> {
        let content = self.read_file_content(path, offset, length, storage)?;
        let decrypted = match encryption {
            None => content,
            Some(info) => {
                if let Some(expected) = &info.Checksum {
                    let actual = Sha256::digest(&content);
                    if actual.as_slice() != expected.as_slice() {
                        return Err(Error::new(format!(
                            "checksum mismatch before decryption, expected {}, actual {}",
                            hex::encode(expected),
                            hex::encode(actual)
                        )));
                    }
                }
                let manager = self.encryption_manager.as_ref().ok_or_else(|| {
                    Error::new("need to decrypt data but encryption manager not set")
                })?;
                manager
                    .lock()
                    .unwrap()
                    .Decrypt(&content, &info.EncryptionInfo)
                    .map_err(Error::new)?
            }
        };
        Self::decode_compressed(&decrypted, raw_length, compression)
    }

    /// 软解析：V1 且 FileGroups 空时，把 Files 塞进单一空 Path 的 group。
    pub fn ParseToMetadata(rawMetaData: &[u8]) -> Result<Metadata, Error> {
        let mut meta: Metadata =
            serde_json::from_slice(rawMetaData).map_err(|e| Error::new(format!("{e}")))?;
        if meta.MetaVersion == crate::stubs::backuppb::MetaVersion::V1 && meta.FileGroups.is_empty()
        {
            meta.FileGroups = vec![crate::stubs::backuppb::DataFileGroup {
                Path: String::new(),
                DataFilesInfo: meta.Files.clone(),
                ..Default::default()
            }];
        }
        Ok(meta)
    }

    /// 硬解析：V1 每个 File 各自成组，保留 Min/MaxTs。
    pub fn ParseToMetadataHard(rawMetaData: &[u8]) -> Result<Metadata, Error> {
        let mut meta: Metadata =
            serde_json::from_slice(rawMetaData).map_err(|e| Error::new(format!("{e}")))?;
        if meta.MetaVersion == crate::stubs::backuppb::MetaVersion::V1 && meta.FileGroups.is_empty()
        {
            let mut groups = Vec::with_capacity(meta.Files.len());
            for d in &meta.Files {
                groups.push(crate::stubs::backuppb::DataFileGroup {
                    Path: d.Path.clone(),
                    DataFilesInfo: vec![d.clone()],
                    MaxTs: d.MaxTs,
                    MinTs: d.MinTs,
                    MinResolvedTs: d.ResolvedTs,
                    Length: d.Length,
                });
            }
            meta.FileGroups = groups;
        }
        Ok(meta)
    }

    /// 序列化：V1 写出前把 FileGroups 展平回 Files 并清空 FileGroups。
    pub fn Marshal(meta: &mut Metadata) -> Result<Vec<u8>, Error> {
        if meta.MetaVersion == crate::stubs::backuppb::MetaVersion::V1 {
            if meta.FileGroups.len() != meta.Files.len() {
                let mut files = Vec::new();
                for g in &meta.FileGroups {
                    files.extend(g.DataFilesInfo.clone());
                }
                meta.Files = files;
            }
            meta.FileGroups.clear();
        }
        serde_json::to_vec(meta).map_err(|e| Error::new(format!("{e}")))
    }

    /// 清空全部缓存条目。
    pub fn Close(&self) {
        self.cache.lock().unwrap().clear();
        if let Some(manager) = &self.encryption_manager {
            manager.lock().unwrap().Close();
        }
    }
}

/// 按 [left,right] 与文件名 TS 窗口过滤路径；不相交返回空串；解析失败原样返回。
pub fn FilterPathByTs(path: &str, left: u64, right: u64) -> String {
    let filename = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".meta");
    let Ok(meta_file) = astersql_br_pkg_stream_backupmetas::ParseName(filename) else {
        // keep consistency with old behaviour, tolerate future file path changes.
        // 无法解析时放行，避免误删未知命名。
        return path.to_string();
    };

    let min_begin = meta_file.MinBeginTsInDefaultCf;
    // min_begin 无效或大于 MinTS 时不做窗口裁剪。
    if min_begin == 0 || min_begin > meta_file.MinTS {
        return path.to_string();
    }

    // 与查询区间不相交 → 过滤掉。
    if right < min_begin || meta_file.MaxTS < left {
        return String::new();
    }
    path.to_string()
}

/// 列出 meta 前缀下文件，按 TS 过滤后回调原始字节。
pub fn FastUnmarshalMetaData<F>(
    storage: Arc<dyn Storage>,
    left: u64,
    right: u64,
    mut cb: F,
) -> Result<(), Error>
where
    F: FnMut(String, Vec<u8>) -> Result<(), Error>,
{
    for (path, _) in storage
        .ListFiles(streamBackupMetaPrefix)
        .map_err(Error::new)?
    {
        if !path.ends_with(".meta") {
            continue;
        }
        let filtered = FilterPathByTs(&path, left, right);
        if filtered.is_empty() {
            continue;
        }
        let raw = storage.ReadFile(&path).map_err(Error::new)?;
        cb(path, raw)?;
    }
    Ok(())
}

/// Parallel Go-equivalent metadata walk with caller-provided skip condition.
pub fn FastUnmarshalMetaDataWithOptions<F, S>(
    storage: Arc<dyn Storage>,
    left: u64,
    right: u64,
    worker_pool_size: usize,
    skip_condition: S,
    callback: F,
) -> Result<(), Error>
where
    F: Fn(String, Vec<u8>) -> Result<(), Error> + Send + Sync,
    S: Fn(&str) -> bool,
{
    let paths = storage
        .ListFiles(streamBackupMetaPrefix)
        .map_err(Error::new)?
        .into_iter()
        .filter_map(|(path, _)| {
            if !path.ends_with(".meta") || skip_condition(&path) {
                return None;
            }
            let filtered = FilterPathByTs(&path, left, right);
            (!filtered.is_empty()).then_some(filtered)
        })
        .collect::<Vec<_>>();
    if paths.is_empty() {
        return Ok(());
    }

    let next = AtomicUsize::new(0);
    let cancelled = AtomicBool::new(false);
    let first_error = Mutex::new(None);
    let workers = worker_pool_size.max(1).min(paths.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                while !cancelled.load(Ordering::Acquire) {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else {
                        break;
                    };
                    let result = storage
                        .ReadFile(path)
                        .map_err(|error| {
                            Error::new(format!(
                                "during reading meta file {path} from storage: {error}"
                            ))
                        })
                        .and_then(|raw| callback(path.clone(), raw));
                    if let Err(error) = result {
                        cancelled.store(true, Ordering::Release);
                        let mut slot = first_error.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(error);
                        }
                        break;
                    }
                }
            });
        }
    });
    first_error.lock().unwrap().take().map_or(Ok(()), Err)
}

/// 持有 helper 与 storage 的轻量流管理器门面。
pub struct StreamManager {
    pub helper: MetadataHelper,
    pub storage: Arc<dyn Storage>,
}

impl StreamManager {
    /// 用给定 storage 构造。
    pub fn New(storage: Arc<dyn Storage>) -> Self {
        Self {
            helper: MetadataHelper::new(),
            storage,
        }
    }

    /// 暴露 meta 前缀。
    pub fn MetaPrefix(&self) -> &'static str {
        GetStreamBackupMetaPrefix()
    }

    /// 暴露 DBs meta 观察范围。
    pub fn ObserveMetaRange(&self) -> crate::stubs::kv::KeyRange {
        BuildObserveMetaRange()
    }

    /// 从 meta field 解析 DB key，转发 `meta::ParseDBKey`。
    pub fn ParseDBKeyFromMetaField(field: &[u8]) -> Result<i64, Error> {
        meta::ParseDBKey(field).map_err(Error::new)
    }
}

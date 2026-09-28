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

// 本地 ingest Engine：内存 KV 存储、范围属性与写入批。
//
// Engine 是 Lightning/ingestor 本地排序写入单元，持有按 key 有序的 KV、
// Region（TiKV 数据分片）切分键、导入互斥状态与重复检测附属数据。
// RangeProperties 用于按大小/键数采样切分点；Writer 提供批量 Append/Flush。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use crate::iterator::{IngestLocalEngineIter, PebbleIter};
use crate::{ConflictInfo, EngineFileSize, EngineId, Error, KeyRange, KvPair, Result};

/// 引擎元数据在 KV 空间中的保留键（不可作为用户数据 key）。
pub static ENGINE_META_KEY: &[u8] = &[0, b'm', b'e', b't', b'a'];
/// 普通迭代器起始边界（跳过 meta 前缀）。
pub static NORMAL_ITER_START_KEY: &[u8] = &[1];

/// 引擎操作互斥状态位掩码类型。
pub type ImportMutexState = u32;
/// 正在执行导入。
pub const IMPORT_MUTEX_STATE_IMPORT: ImportMutexState = 1;
/// 引擎已关闭。
pub const IMPORT_MUTEX_STATE_CLOSE: ImportMutexState = 2;
/// 读锁占用计数步进值。
pub const IMPORT_MUTEX_STATE_READ_LOCK: ImportMutexState = 4;
/// 正在打开。
pub const IMPORT_MUTEX_STATE_OPEN: ImportMutexState = 8;
/// 重复检测目录后缀。
pub const DUP_DETECT_DIR_SUFFIX: &str = ".dupdetect";
/// 重复检测结果目录后缀。
pub const DUP_RESULT_DIR_SUFFIX: &str = ".dupresult";

/// 引擎级元数据：时间戳、键数量与总字节。
#[derive(Default)]
pub struct EngineMeta {
    pub ts: AtomicU64,
    pub length: AtomicI64,
    pub total_size: AtomicI64,
}

/// 某切分点相对累计的 Size/Keys 偏移。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RangeOffsets {
    pub Size: u64,
    pub Keys: u64,
}

/// 单个范围属性采样点：键与累计偏移。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RangeProperty {
    pub Key: Vec<u8>,
    pub offsets: RangeOffsets,
}

/// 范围属性采样点列表。
pub type RangeProperties = Vec<RangeProperty>;

/// 将范围属性编码为二进制：每项为 key_len + key + size + keys（大端）。
pub fn encodeRangeProperties(properties: &[RangeProperty]) -> Vec<u8> {
    let mut encoded = Vec::new();
    for property in properties {
        encoded.extend_from_slice(&(property.Key.len() as u32).to_be_bytes());
        encoded.extend_from_slice(&property.Key);
        encoded.extend_from_slice(&property.offsets.Size.to_be_bytes());
        encoded.extend_from_slice(&property.offsets.Keys.to_be_bytes());
    }
    encoded
}

/// 解码范围属性；遇到 `ENGINE_META_KEY` 时跳过该项。
pub fn decodeRangeProperties(data: &[u8]) -> Result<RangeProperties> {
    let mut cursor = 0;
    let mut output = Vec::new();
    while cursor < data.len() {
        if data.len() - cursor < 4 {
            return Err(Error::InvalidData(
                "truncated range-property key length".into(),
            ));
        }
        let key_length = u32::from_be_bytes(data[cursor..cursor + 4].try_into().unwrap()) as usize;
        cursor += 4;
        let end = cursor
            .checked_add(key_length)
            .and_then(|value| value.checked_add(16))
            .ok_or_else(|| Error::InvalidData("range-property length overflow".into()))?;
        if end > data.len() {
            return Err(Error::InvalidData(
                "truncated range-property payload".into(),
            ));
        }
        let key = data[cursor..cursor + key_length].to_vec();
        cursor += key_length;
        let size = u64::from_be_bytes(data[cursor..cursor + 8].try_into().unwrap());
        let keys = u64::from_be_bytes(data[cursor + 8..cursor + 16].try_into().unwrap());
        cursor += 16;
        // meta 键不进入对外属性列表
        if key != ENGINE_META_KEY {
            output.push(RangeProperty {
                Key: key,
                offsets: RangeOffsets {
                    Size: size,
                    Keys: keys,
                },
            });
        }
    }
    Ok(output)
}

/// 按字节/键数距离采集范围属性切分点。
pub struct RangePropertiesCollector {
    properties: RangeProperties,
    last_offsets: RangeOffsets,
    last_key: Vec<u8>,
    current_offsets: RangeOffsets,
    property_size_distance: u64,
    property_keys_distance: u64,
}

impl RangePropertiesCollector {
    /// 构造采集器；达到 size 或 keys 距离阈值时插入新切分点。
    pub fn new(property_size_distance: u64, property_keys_distance: u64) -> Self {
        Self {
            properties: Vec::with_capacity(1024),
            last_offsets: RangeOffsets::default(),
            last_key: Vec::new(),
            current_offsets: RangeOffsets::default(),
            property_size_distance,
            property_keys_distance,
        }
    }

    /// 距上一采样点的字节增量。
    fn size_in_last_range(&self) -> u64 {
        self.current_offsets.Size - self.last_offsets.Size
    }

    /// 距上一采样点的键数增量。
    fn keys_in_last_range(&self) -> u64 {
        self.current_offsets.Keys - self.last_offsets.Keys
    }

    /// 以当前累计偏移在 `key` 处插入采样点。
    fn insert_new_point(&mut self, key: &[u8]) {
        self.last_offsets = self.current_offsets.clone();
        self.properties.push(RangeProperty {
            Key: key.to_vec(),
            offsets: self.current_offsets.clone(),
        });
    }

    /// 累加一对 KV；跳过 meta 键，必要时插入新采样点。
    pub fn Add(&mut self, key: &[u8], value: &[u8]) {
        if key == ENGINE_META_KEY {
            return;
        }
        self.current_offsets.Size += (key.len() + value.len()) as u64;
        self.current_offsets.Keys += 1;
        if self.last_key.is_empty()
            || self.size_in_last_range() >= self.property_size_distance
            || self.keys_in_last_range() >= self.property_keys_distance
        {
            self.insert_new_point(key);
        }
        self.last_key = key.to_vec();
    }

    /// 收尾：若末段仍有增量则再插一点，并返回全部采样。
    pub fn Finish(mut self) -> RangeProperties {
        if self.size_in_last_range() > 0 || self.keys_in_last_range() > 0 {
            let last_key = self.last_key.clone();
            self.insert_new_point(&last_key);
        }
        self.properties
    }
}

/// 按索引 handle 聚合的大小属性（相邻采样点差分累加）。
#[derive(Clone, Debug, Default)]
pub struct SizeProperties {
    pub total_size: u64,
    pub index_handles: BTreeMap<Vec<u8>, RangeOffsets>,
}

impl SizeProperties {
    /// 将一组 RangeProperty 差分合并进 index_handles，并更新 total_size。
    pub fn add_all(&mut self, properties: &[RangeProperty]) {
        let mut previous = RangeOffsets::default();
        for property in properties {
            let delta = RangeOffsets {
                Size: property.offsets.Size.saturating_sub(previous.Size),
                Keys: property.offsets.Keys.saturating_sub(previous.Keys),
            };
            let entry = self.index_handles.entry(property.Key.clone()).or_default();
            entry.Size = entry.Size.saturating_add(delta.Size);
            entry.Keys = entry.Keys.saturating_add(delta.Keys);
            previous = property.offsets.clone();
        }
        if let Some(last) = properties.last() {
            self.total_size = self.total_size.saturating_add(last.offsets.Size);
        }
    }
}

/// 本地 ingest 引擎：有序 KV、导入状态、Region 切分与冲突信息。
pub struct Engine {
    pub engine_meta: EngineMeta,
    closed: AtomicBool,
    pub UUID: EngineId,
    data: RwLock<BTreeMap<Vec<u8>, Vec<u8>>>,
    duplicate_data: Arc<Mutex<Vec<KvPair>>>,
    region_split_size: AtomicI64,
    region_split_key_count: AtomicI64,
    region_split_keys_cache: Mutex<Vec<Vec<u8>>>,
    state: AtomicU32,
    operation_lock: Mutex<()>,
    imported_kv_size: AtomicI64,
    imported_kv_count: AtomicI64,
    pending_file_size: AtomicI64,
    memory_size: AtomicI64,
    first_error: Mutex<Option<Error>>,
}

impl Engine {
    /// 创建空引擎并设置 Region 切分阈值。
    pub fn new(id: EngineId, region_split_size: i64, region_split_key_count: i64) -> Self {
        Self {
            engine_meta: EngineMeta::default(),
            closed: AtomicBool::new(false),
            UUID: id,
            data: RwLock::new(BTreeMap::new()),
            duplicate_data: Arc::new(Mutex::new(Vec::new())),
            region_split_size: AtomicI64::new(region_split_size),
            region_split_key_count: AtomicI64::new(region_split_key_count),
            region_split_keys_cache: Mutex::new(Vec::new()),
            state: AtomicU32::new(0),
            operation_lock: Mutex::new(()),
            imported_kv_size: AtomicI64::new(0),
            imported_kv_count: AtomicI64::new(0),
            pending_file_size: AtomicI64::new(0),
            memory_size: AtomicI64::new(0),
            first_error: Mutex::new(None),
        }
    }

    /// 仅记录首个错误，后续错误忽略。
    pub fn setError(&self, error: Error) {
        if let Ok(mut first) = self.first_error.lock() {
            if first.is_none() {
                *first = Some(error);
            }
        }
    }

    /// 写入或覆盖一对 KV；拒绝空键与 meta 保留键。
    pub fn Put(&self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        if self.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        if key.is_empty() || key == ENGINE_META_KEY {
            return Err(Error::InvalidArgument(
                "reserved or empty engine key".into(),
            ));
        }
        let _operation = self.operation_lock.lock().map_err(|_| Error::Poisoned)?;
        let mut data = self.data.write().map_err(|_| Error::Poisoned)?;
        let old = data.insert(key.clone(), value.clone());
        if old.is_none() {
            self.engine_meta.length.fetch_add(1, Ordering::Relaxed);
        }
        // 按新旧 value 差量更新 total_size 与 memory_size
        let old_size = old.map_or(0, |old| key.len() + old.len());
        let delta = (key.len() + value.len()) as i64 - old_size as i64;
        self.engine_meta
            .total_size
            .fetch_add(delta, Ordering::Relaxed);
        self.memory_size.fetch_add(delta, Ordering::Relaxed);
        Ok(())
    }

    /// 标记写入结束：置 closed，若有 first_error 则返回之。
    pub fn finishWrite(&self) -> Result<()> {
        if self.closed.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        self.state
            .store(IMPORT_MUTEX_STATE_CLOSE, Ordering::Release);
        if let Some(error) = self
            .first_error
            .lock()
            .map_err(|_| Error::Poisoned)?
            .clone()
        {
            return Err(error);
        }
        Ok(())
    }

    /// 强制关闭并进入 CLOSE 状态。
    pub fn Close(&self) -> Result<()> {
        self.closed.store(true, Ordering::Release);
        self.state
            .store(IMPORT_MUTEX_STATE_CLOSE, Ordering::Release);
        Ok(())
    }

    /// 删除引擎数据目录及重复检测相关附属目录。
    pub fn Cleanup(&self, data_dir: &Path) -> Result<()> {
        let id = self.UUID.to_string();
        for path in [
            data_dir.join(&id),
            data_dir.join(format!("{id}{DUP_DETECT_DIR_SUFFIX}")),
            data_dir.join(format!("{id}{DUP_RESULT_DIR_SUFFIX}")),
        ] {
            if path.exists() {
                fs::remove_dir_all(path)?;
            }
        }
        Ok(())
    }

    /// 检查引擎目录是否存在于 `data_dir`。
    pub fn Exist(&self, data_dir: &Path) -> Result<()> {
        let path = data_dir.join(self.UUID.to_string());
        if path.exists() {
            Ok(())
        } else {
            Err(Error::NotFound(format!(
                "engine directory not found: {}",
                path.display()
            )))
        }
    }

    /// 尝试增加读锁计数；若已处于 CLOSE/IMPORT 锁定则失败。
    pub fn tryRLock(&self) -> bool {
        let current = self.state.load(Ordering::Acquire);
        if isStateLocked(current) {
            return false;
        }
        self.state
            .compare_exchange(
                current,
                current.saturating_add(IMPORT_MUTEX_STATE_READ_LOCK),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// 释放一次读锁计数。
    pub fn rUnlock(&self) {
        self.state
            .fetch_sub(IMPORT_MUTEX_STATE_READ_LOCK, Ordering::AcqRel);
    }

    /// 在忽略掩码允许时，将空闲状态 CAS 为 `new_state`。
    pub fn lockUnless(&self, new_state: ImportMutexState, ignore_mask: ImportMutexState) -> bool {
        // Go 版只在进入时检查 ignore mask，随后通过 RWMutex.Lock
        // 等待已有读/写锁释放。这里用 CAS 表达同一状态机：
        // 暂时繁忙必须等待，不能被误报为“命中忽略状态”。
        let current = self.state.load(Ordering::Acquire);
        if current & ignore_mask != 0 {
            return false;
        }
        loop {
            match self
                .state
                .compare_exchange(0, new_state, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return true,
                Err(_) => std::thread::yield_now(),
            }
        }
    }

    /// 清除互斥状态。
    pub fn unlock(&self) {
        self.state.store(0, Ordering::Release);
    }

    /// 是否处于 CLOSE 或 IMPORT 锁定。
    pub fn isLocked(&self) -> bool {
        isStateLocked(self.state.load(Ordering::Acquire))
    }

    /// 当前内存占用估计。
    pub fn TotalMemorySize(&self) -> i64 {
        self.memory_size.load(Ordering::Relaxed)
    }

    /// 返回 (总字节, 键数量)。
    pub fn KVStatistics(&self) -> (i64, i64) {
        (
            self.engine_meta.total_size.load(Ordering::Relaxed),
            self.engine_meta.length.load(Ordering::Relaxed),
        )
    }

    /// 已导入到远端的字节与键计数。
    pub fn ImportedStatistics(&self) -> (i64, i64) {
        (
            self.imported_kv_size.load(Ordering::Relaxed),
            self.imported_kv_count.load(Ordering::Relaxed),
        )
    }

    /// 累加一次导入完成的统计。
    pub fn FinishImport(&self, bytes: i64, count: i64) {
        self.imported_kv_size.fetch_add(bytes, Ordering::Relaxed);
        self.imported_kv_count.fetch_add(count, Ordering::Relaxed);
    }

    /// 汇总本地重复检测缓存中的冲突条数与字节。
    pub fn ConflictInfo(&self) -> ConflictInfo {
        let duplicates = self.duplicate_data.lock().ok();
        ConflictInfo {
            count: duplicates.as_ref().map_or(0, |items| items.len() as u64),
            size: duplicates
                .as_ref()
                .map_or(0, |items| items.iter().map(|pair| pair.size() as u64).sum()),
        }
    }

    /// 引擎 ID 字符串形式。
    pub fn ID(&self) -> String {
        self.UUID.to_string()
    }

    /// 在 [lower, upper) 内取首尾 key；空区间返回空向量对。
    pub fn GetFirstAndLastKey(&self, lower: &[u8], upper: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
        let data = self.data.read().map_err(|_| Error::Poisoned)?;
        let mut range = data.range(lower.to_vec()..);
        let first = range
            .find(|(key, _)| upper.is_empty() || key.as_slice() < upper)
            .map(|(key, _)| key.clone());
        let last = data
            .range(lower.to_vec()..)
            .take_while(|(key, _)| upper.is_empty() || key.as_slice() < upper)
            .last()
            .map(|(key, _)| key.clone());
        Ok(match (first, last) {
            (Some(first), Some(last)) => (first, last),
            _ => (Vec::new(), Vec::new()),
        })
    }

    /// 全表键范围：[first, nextKey(last))。
    pub fn GetKeyRange(&self) -> Result<KeyRange> {
        let (start, last) = self.GetFirstAndLastKey(&[], &[])?;
        Ok(KeyRange {
            start,
            end: nextKey(&last),
        })
    }

    /// 按字节/键阈值生成 Region 切分键，并缓存结果。
    pub fn GetRegionSplitKeys(&self) -> Result<Vec<Vec<u8>>> {
        let size_limit = self.region_split_size.load(Ordering::Relaxed).max(1) as u64;
        let key_limit = self.region_split_key_count.load(Ordering::Relaxed).max(1) as u64;
        let data = self.data.read().map_err(|_| Error::Poisoned)?;
        let mut keys = Vec::new();
        let mut size = 0u64;
        let mut count = 0u64;
        for (key, value) in data.iter() {
            if keys.is_empty() {
                keys.push(key.clone());
            }
            size += (key.len() + value.len()) as u64;
            count += 1;
            if size >= size_limit || count >= key_limit {
                // nextKey 作为半开区间上界，供 PD scatter/split 使用
                keys.push(nextKey(key));
                size = 0;
                count = 0;
            }
        }
        if let Some(last) = data.keys().next_back() {
            let end = nextKey(last);
            if keys.last() != Some(&end) {
                keys.push(end);
            }
        }
        *self
            .region_split_keys_cache
            .lock()
            .map_err(|_| Error::Poisoned)? = keys.clone();
        Ok(keys)
    }

    /// 构造覆盖 [lower, upper) 的本地引擎迭代器。
    pub fn newKVIter(&self, lower: &[u8], upper: &[u8]) -> Result<Box<dyn IngestLocalEngineIter>> {
        let pairs = self
            .data
            .read()
            .map_err(|_| Error::Poisoned)?
            .iter()
            .map(|(key, value)| KvPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect();
        Ok(Box::new(PebbleIter::new(pairs, lower, upper)))
    }

    /// 导出当前全部 KV 快照。
    pub fn snapshot(&self) -> Result<Vec<KvPair>> {
        Ok(self
            .data
            .read()
            .map_err(|_| Error::Poisoned)?
            .iter()
            .map(|(key, value)| KvPair {
                key: key.clone(),
                value: value.clone(),
            })
            .collect())
    }

    /// 组装磁盘配额检查用的文件大小视图。
    pub fn getEngineFileSize(&self) -> EngineFileSize {
        EngineFileSize {
            UUID: self.UUID,
            DiskSize: self.engine_meta.total_size.load(Ordering::Relaxed)
                + self.pending_file_size.load(Ordering::Relaxed),
            MemSize: self.TotalMemorySize(),
            IsImporting: self.isLocked(),
        }
    }

    /// 共享重复检测用的本地 KV 缓冲。
    pub fn duplicate_store(&self) -> Arc<Mutex<Vec<KvPair>>> {
        Arc::clone(&self.duplicate_data)
    }
}

/// 面向 Engine 的批量写入器：积攒到 batch_size 后 Flush。
pub struct Writer {
    engine: Arc<Engine>,
    batch: Vec<KvPair>,
    batch_size: usize,
    closed: bool,
}

impl Writer {
    /// 创建写入器；`batch_size` 至少为 1。
    pub fn new(engine: Arc<Engine>, batch_size: usize) -> Self {
        Self {
            engine,
            batch: Vec::new(),
            batch_size: batch_size.max(1),
            closed: false,
        }
    }

    /// 追加一对 KV，满批则自动 Flush。
    pub fn Append(&mut self, key: Vec<u8>, value: Vec<u8>) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        self.batch.push(KvPair { key, value });
        if self.batch.len() >= self.batch_size {
            self.Flush()?;
        }
        Ok(())
    }

    /// 将当前批写入引擎。
    pub fn Flush(&mut self) -> Result<()> {
        for pair in self.batch.drain(..) {
            self.engine.Put(pair.key, pair.value)?;
        }
        Ok(())
    }

    /// Flush 后标记关闭，禁止再 Append。
    pub fn Close(&mut self) -> Result<()> {
        self.Flush()?;
        self.closed = true;
        Ok(())
    }

    /// 当前未刷盘批次的估计字节数。
    pub fn EstimatedSize(&self) -> usize {
        self.batch.iter().map(KvPair::size).sum()
    }
}

/// CLOSE 或 IMPORT 位被置位时视为锁定。
pub fn isStateLocked(state: ImportMutexState) -> bool {
    state & (IMPORT_MUTEX_STATE_CLOSE | IMPORT_MUTEX_STATE_IMPORT) != 0
}

/// 生成半开区间上界；整数行 key 使用下一个可编码 handle。
pub fn nextKey(key: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return Vec::new();
    }

    // TiKV 4.x 及更早版本可能截断行 key。与 Go 实现一致，固定长度的
    // `t{tableID}_r{intHandle}` 需要推进到下一个有效行 key。
    if key.len() == 19 && key[0] == b't' && key[9] == b'_' && key[10] == b'r' {
        let table_id = i64::from_be_bytes(key[1..9].try_into().unwrap()) ^ i64::MIN;
        let handle = i64::from_be_bytes(key[11..19].try_into().unwrap()) ^ i64::MIN;
        if handle == i64::MAX {
            let next_table_id = table_id.wrapping_add(1);
            let mut result = Vec::with_capacity(9);
            result.push(b't');
            result.extend_from_slice(&(next_table_id as u64 ^ 0x8000_0000_0000_0000).to_be_bytes());
            return result;
        }

        let mut result = key[..11].to_vec();
        result.extend_from_slice(&((handle + 1) as u64 ^ 0x8000_0000_0000_0000).to_be_bytes());
        return result;
    }

    let mut result = key.to_vec();
    result.push(0);
    result
}

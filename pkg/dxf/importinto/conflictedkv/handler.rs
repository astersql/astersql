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

// 冲突 KV 处理核心：上下文、存储/编解码抽象，以及 data / 索引 Handler。
//
// DataKVHandler 直接解码行并重编码后交给 EncodedRowHandler；
// IndexKVHandler 从唯一索引 KV 得到 handle，批量用惰性刷新快照回查行再处理，
// 可选 KeyFilter 跳过已处理行。LazyRefreshedSnapshot 按固定间隔刷新 MVCC 快照。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use astersql_dxf_framework_taskexecutor_execute::{Collector, NoopCollector};
use astersql_kv::{Handle, Key, Version};
use astersql_lightning_backend_kv::Pairs;
use astersql_meta_model::{IndexInfo, TableInfo};
use astersql_objstore_objectio as objectio;
use astersql_types::datum::Datum;

use crate::KeyFilter;

/// 惰性快照最短刷新间隔。
const snapshotRefreshInterval: Duration = Duration::from_secs(15);
/// data KV 组名常量。
pub const DataKVGroup: &str = "data";
/// IndexKVHandler 缓冲 handle 达到此上限时触发批量回查。
pub static BufferedHandleLimit: AtomicUsize = AtomicUsize::new(256);

/// 冲突处理上下文：KV 与对象 IO 上下文，以及取消标志。
#[derive(Clone, Default)]
pub struct ConflictContext {
    pub KV: astersql_kv::Context,
    pub ObjectIO: objectio::Context,
    cancelled: Arc<AtomicBool>,
}

impl ConflictContext {
    /// 标记取消，并取消对象 IO。
    pub fn Cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.ObjectIO.cancel();
    }

    /// 是否已取消（本标志或 ObjectIO 取消）。
    pub fn IsCancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire) || self.ObjectIO.is_cancelled()
    }
}

/// 一条冲突 KV 对。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ConflictKVPair {
    pub Key: Key,
    pub Value: Vec<u8>,
}

/// 记录与集群交互的读/写流量。
pub trait TrafficRecorder: Send + Sync {
    fn IncClusterReadBytes(&self, bytes: u64);
    fn IncClusterWriteBytes(&self, bytes: u64);
}

/// 集群只读快照：按键批量获取。
pub trait ConflictSnapshot {
    fn BatchGet(
        &self,
        context: &ConflictContext,
        keys: &[Key],
    ) -> Result<HashMap<Vec<u8>, astersql_kv::tikvstore::ValueEntry>, String>;
}

/// 冲突删除用事务：Delete / Commit / Rollback。
pub trait ConflictTransaction: Send {
    fn Delete(&mut self, key: &Key) -> Result<(), String>;
    fn Commit(self: Box<Self>, context: &ConflictContext) -> Result<(), String>;
    fn Rollback(self: Box<Self>) -> Result<(), String>;
}

/// 冲突处理所需的集群存储抽象（keyspace、版本、快照、事务、可重试错误判断）。
pub trait ConflictStore: Send + Sync {
    fn Keyspace(&self) -> Vec<u8>;
    fn CurrentVersion(&self) -> Result<Version, String>;
    fn GetSnapshot(&self, version: Version) -> Box<dyn ConflictSnapshot>;
    fn Begin(&self) -> Result<Box<dyn ConflictTransaction>, String>;
    fn IsRetryableError(&self, error: &str) -> bool;
    /// The string boundary retains TiDB's transaction retry marker.
    fn IsTxnRetryableError(&self, error: &str) -> bool {
        error.contains(astersql_kv::TxnRetryableMark)
    }
}

/// 行/索引键的编解码与重编码，供 Handler 在冲突路径上复用。
pub trait ConflictRowCodec {
    fn ConfigureKeyspace(&mut self, _keyspace: Vec<u8>) {}
    fn StripKeyspacePrefix(&self, key: &Key) -> Result<Key, String>;
    fn DecodeRowKey(&self, key: &Key) -> Result<Box<dyn Handle>, String>;
    fn DecodeRow(&self, handle: &dyn Handle, value: &[u8]) -> Result<Vec<Datum>, String>;
    fn DecodeTableID(&self, key: &Key) -> i64;
    fn DecodeIndexHandle(
        &self,
        key: &Key,
        value: &[u8],
        index_column_count: usize,
    ) -> Result<Box<dyn Handle>, String>;
    fn EncodeRowKey(&self, table_id: i64, handle: &dyn Handle) -> Key;
    fn EncodeRow(
        &mut self,
        handle: &dyn Handle,
        row: &[Datum],
        auto_row_id: i64,
    ) -> Result<Pairs, String>;
    fn Close(&mut self) -> Result<(), String>;
}

/// 冲突 KV 流处理器：准备、消费通道中的 KV、收尾。
pub trait Handler {
    fn PreRun(&mut self) -> Result<(), String>;
    fn Run(
        &mut self,
        context: &ConflictContext,
        pairs: &mpsc::Receiver<ConflictKVPair>,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String>;
    fn Close(
        &mut self,
        context: &ConflictContext,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String>;
}

/// 单条冲突 KV 的处理接口。
pub trait KVHandler {
    fn Handle(
        &mut self,
        context: &ConflictContext,
        pair: ConflictKVPair,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String>;
}

/// 行已重编码后的回调（收集写文件或排队删除）。
pub trait EncodedRowHandler {
    fn HandleEncodedRow(
        &mut self,
        context: &ConflictContext,
        row_key: &Key,
        row: &[Datum],
        pairs: &Pairs,
    ) -> Result<(), String>;
}

/// Handler 公共状态：目标表、KV 组、编解码器与进度 Collector。
pub struct BaseHandler {
    target_table: Arc<TableInfo>,
    kv_group: String,
    codec: Box<dyn ConflictRowCodec>,
    collector: Arc<dyn Collector>,
}

/// 构造 BaseHandler；未提供 Collector 时使用 NoopCollector。
pub fn NewBaseHandler(
    target_table: Arc<TableInfo>,
    kv_group: impl Into<String>,
    codec: Box<dyn ConflictRowCodec>,
    collector: Option<Arc<dyn Collector>>,
) -> BaseHandler {
    BaseHandler {
        target_table,
        kv_group: kv_group.into(),
        codec,
        collector: collector.unwrap_or_else(|| Arc::new(NoopCollector)),
    }
}

impl BaseHandler {
    /// 重编码一行并交给 EncodedRowHandler；非聚簇索引表用 handle 整数值作 auto_row_id。
    fn encodeAndHandleRow(
        &mut self,
        context: &ConflictContext,
        row_key: &Key,
        handle: Box<dyn Handle>,
        row: Vec<Datum>,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String> {
        let auto_row_id = if self.target_table.HasClusteredIndex() {
            0
        } else {
            handle.IntValue()
        };
        let pairs = self.codec.EncodeRow(handle.as_ref(), &row, auto_row_id)?;
        row_handler.HandleEncodedRow(context, row_key, &row, &pairs)
    }

    /// 关闭底层编解码器。
    fn Close(&mut self) -> Result<(), String> {
        self.codec.Close()
    }
}

/// 处理 data KV 组冲突：解码行键与行内容后重编码。
pub struct DataKVHandler {
    base: BaseHandler,
}

/// 用给定 BaseHandler 构造 DataKVHandler。
pub fn NewDataKVHandler(base: BaseHandler) -> DataKVHandler {
    DataKVHandler { base }
}

impl Handler for DataKVHandler {
    fn PreRun(&mut self) -> Result<(), String> {
        Ok(())
    }

    fn Run(
        &mut self,
        context: &ConflictContext,
        pairs: &mpsc::Receiver<ConflictKVPair>,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String> {
        while let Ok(pair) = pairs.recv() {
            if context.IsCancelled() {
                return Err("conflict handling cancelled".into());
            }
            self.Handle(context, pair, row_handler)?;
            self.base.collector.Processed(1, 0);
        }
        Ok(())
    }

    fn Close(
        &mut self,
        _context: &ConflictContext,
        _row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String> {
        self.base.Close()
    }
}

impl KVHandler for DataKVHandler {
    fn Handle(
        &mut self,
        context: &ConflictContext,
        pair: ConflictKVPair,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String> {
        let key = self.base.codec.StripKeyspacePrefix(&pair.Key)?;
        let handle = self.base.codec.DecodeRowKey(&key)?;
        let row = self.base.codec.DecodeRow(handle.as_ref(), &pair.Value)?;
        self.base
            .encodeAndHandleRow(context, &key, handle, row, row_handler)
    }
}

/// 缓冲的「表 ID + 行 handle」，供索引路径批量回查。
struct HandleOfTable {
    row_key: Key,
    handle: Box<dyn Handle>,
}

/// 处理唯一索引 KV 组：解码索引 handle，过滤后缓冲，再批量快照取行。
pub struct IndexKVHandler {
    base: BaseHandler,
    snapshot: LazyRefreshedSnapshot,
    handle_filter: Option<KeyFilter>,
    target_index: Option<IndexInfo>,
    buffered_handles: Vec<HandleOfTable>,
}

/// 构造 IndexKVHandler；`filter` 用于跳过已在 data 路径处理过的行。
pub fn NewIndexKVHandler(
    base: BaseHandler,
    snapshot: LazyRefreshedSnapshot,
    filter: Option<KeyFilter>,
) -> IndexKVHandler {
    IndexKVHandler {
        base,
        snapshot,
        handle_filter: filter,
        target_index: None,
        buffered_handles: Vec::new(),
    }
}

impl IndexKVHandler {
    /// 处理单条索引 KV：解码 handle，可选跳过，缓冲后达上限则批量回查。
    fn HandleOne(
        &mut self,
        context: &ConflictContext,
        pair: ConflictKVPair,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String> {
        let key = self.base.codec.StripKeyspacePrefix(&pair.Key)?;
        let table_id = self.base.codec.DecodeTableID(&key);
        if table_id == 0 {
            return Err(format!("invalid table ID in key {:02x?}", pair.Key.0));
        }
        let columns = self
            .target_index
            .as_ref()
            .ok_or_else(|| "index handler was not prepared".to_owned())?
            .Columns
            .len();
        let handle = self
            .base
            .codec
            .DecodeIndexHandle(&key, &pair.Value, columns)?;
        let row_key = self.base.codec.EncodeRowKey(table_id, handle.as_ref());
        if self
            .handle_filter
            .as_ref()
            .is_some_and(|filter| filter.isHandledGlobally(&row_key))
        {
            return Ok(());
        }
        self.buffered_handles
            .push(HandleOfTable { row_key, handle });
        if self.buffered_handles.len() >= BufferedHandleLimit.load(Ordering::Acquire).max(1) {
            self.handleBufferedHandles(context, row_handler)?;
        }
        Ok(())
    }

    /// 批量编码行键、快照 BatchGet，再解码行并交给 EncodedRowHandler。
    fn handleBufferedHandles(
        &mut self,
        context: &ConflictContext,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String> {
        if self.buffered_handles.is_empty() {
            return Ok(());
        }
        let mut row_keys = Vec::with_capacity(self.buffered_handles.len());
        let mut key_to_handle = HashMap::with_capacity(self.buffered_handles.len());
        for item in &self.buffered_handles {
            let row_key = item.row_key.clone();
            key_to_handle.insert(row_key.0.clone(), item.handle.Copy());
            row_keys.push(row_key);
        }
        let rows = self.snapshot.BatchGet(context, &row_keys)?;
        for (row_key, value) in rows {
            let key = Key(row_key.clone());
            if self
                .handle_filter
                .as_ref()
                .is_some_and(|filter| filter.isHandledLocally(&key))
            {
                continue;
            }
            let handle = key_to_handle
                .remove(&row_key)
                .ok_or_else(|| "snapshot returned an unrequested row key".to_owned())?;
            let row = self.base.codec.DecodeRow(handle.as_ref(), &value.Value)?;
            self.base
                .encodeAndHandleRow(context, &key, handle, row, row_handler)?;
            if let Some(filter) = &self.handle_filter {
                filter.addLocal(&key);
            }
        }
        self.buffered_handles.clear();
        Ok(())
    }
}

impl KVHandler for IndexKVHandler {
    fn Handle(
        &mut self,
        context: &ConflictContext,
        pair: ConflictKVPair,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String> {
        self.HandleOne(context, pair, row_handler)
    }
}

impl Handler for IndexKVHandler {
    /// 从 kv_group 解析索引 ID，并在表元信息中定位目标索引。
    fn PreRun(&mut self) -> Result<(), String> {
        let index_id = self
            .base
            .kv_group
            .parse::<i64>()
            .map_err(|error| error.to_string())?;
        self.target_index = self
            .base
            .target_table
            .Indices
            .iter()
            .find(|index| index.ID == index_id)
            .cloned();
        if self.target_index.is_none() {
            return Err(format!(
                "index {} in table {}",
                index_id, self.base.target_table.Name.O
            ));
        }
        Ok(())
    }

    fn Run(
        &mut self,
        context: &ConflictContext,
        pairs: &mpsc::Receiver<ConflictKVPair>,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String> {
        while let Ok(pair) = pairs.recv() {
            if context.IsCancelled() {
                return Err("conflict handling cancelled".into());
            }
            self.HandleOne(context, pair, row_handler)?;
            self.base.collector.Processed(1, 0);
        }
        Ok(())
    }

    /// 冲刷剩余缓冲 handle，再关闭编解码器。
    fn Close(
        &mut self,
        context: &ConflictContext,
        row_handler: &mut dyn EncodedRowHandler,
    ) -> Result<(), String> {
        let buffered_result = self.handleBufferedHandles(context, row_handler);
        let close_result = self.base.Close();
        buffered_result.and(close_result)
    }
}

/// 按时间间隔惰性刷新的集群快照包装。
pub struct LazyRefreshedSnapshot {
    snapshot: Option<Box<dyn ConflictSnapshot>>,
    store: Arc<dyn ConflictStore>,
    last_refresh_time: Option<Instant>,
    traffic_recorder: Option<Arc<dyn TrafficRecorder>>,
}

/// 构造尚未拉取快照的 LazyRefreshedSnapshot。
pub fn NewLazyRefreshedSnapshot(
    store: Arc<dyn ConflictStore>,
    recorder: Option<Arc<dyn TrafficRecorder>>,
) -> LazyRefreshedSnapshot {
    LazyRefreshedSnapshot {
        snapshot: None,
        store,
        last_refresh_time: None,
        traffic_recorder: recorder,
    }
}

impl LazyRefreshedSnapshot {
    /// 若尚无快照或已超过刷新间隔，则按当前版本重新 GetSnapshot。
    fn refreshAsNeeded(&mut self) -> Result<(), String> {
        if self.snapshot.is_some()
            && self
                .last_refresh_time
                .is_some_and(|instant| instant.elapsed() < snapshotRefreshInterval)
        {
            return Ok(());
        }
        let version = self.store.CurrentVersion()?;
        self.snapshot = Some(self.store.GetSnapshot(version));
        self.last_refresh_time = Some(Instant::now());
        Ok(())
    }

    /// 刷新后批量取键；可选记录读流量（键+值字节）。
    pub fn BatchGet(
        &mut self,
        context: &ConflictContext,
        keys: &[Key],
    ) -> Result<HashMap<Vec<u8>, astersql_kv::tikvstore::ValueEntry>, String> {
        self.refreshAsNeeded()?;
        let result = self
            .snapshot
            .as_ref()
            .expect("snapshot initialized")
            .BatchGet(context, keys)?;
        if let Some(recorder) = &self.traffic_recorder {
            let bytes = result
                .iter()
                .map(|(key, value)| key.len() + value.Value.len())
                .sum::<usize>() as u64;
            recorder.IncClusterReadBytes(bytes);
        }
        Ok(result)
    }
}

/// 去掉键的 keyspace 前缀的便捷包装。
pub fn stripKeyspacePrefix(codec: &dyn ConflictRowCodec, key: &Key) -> Result<Key, String> {
    codec.StripKeyspacePrefix(key)
}

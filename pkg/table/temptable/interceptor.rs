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

// 临时表 Snapshot 拦截器（对应 Go TemporaryTableSnapshotInterceptor）。
//
// 在读路径上识别访问的 table_id：临时表键走会话 MemBuffer，
// 普通表走底层 Snapshot；范围扫描通过 UnionIter 合并脏数据与快照。

use crate::infoschema::{
    InfoSchema, SessionVarsProvider, TableInfo, TempTableError, TempTableType,
};
use std::any::Any;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

/// 字节键别名。
pub type Key = Vec<u8>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 值条目：字节载荷与可选提交时间戳 CommitTS。
pub struct ValueEntry {
    pub value: Vec<u8>,
    pub commit_ts: u64,
}

impl ValueEntry {
    /// 构造值条目。
    pub fn new(value: impl Into<Vec<u8>>, commit_ts: u64) -> Self {
        Self {
            value: value.into(),
            commit_ts,
        }
    }

    /// 空值表示 mem-buffer 删除占位（非 KeyNotExist）。
    pub fn is_value_empty(&self) -> bool {
        self.value.is_empty()
    }
}

/// 键值迭代器：valid/key/value/next/close。
pub trait KvIterator: Send {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn valid(&self) -> bool;
    fn key(&self) -> &[u8];
    fn value(&self) -> &ValueEntry;
    fn next(&mut self) -> Result<(), TempTableError>;
    fn close(&mut self);
    fn closed(&self) -> bool {
        false
    }
}

/// 点查与正/反向范围迭代抽象。
pub trait Retriever: Send + Sync {
    fn get(&self, key: &[u8]) -> Result<ValueEntry, TempTableError>;
    fn iter(&self, start: &[u8], end: &[u8]) -> Result<Box<dyn KvIterator>, TempTableError>;
    fn iter_reverse(&self, end: &[u8], start: &[u8])
    -> Result<Box<dyn KvIterator>, TempTableError>;
}

/// Snapshot：在 Retriever 基础上提供 BatchGet。
pub trait Snapshot: Retriever {
    fn batch_get(&self, keys: &[Key]) -> Result<HashMap<Key, ValueEntry>, TempTableError>;
}

#[derive(Default)]
/// 会话内存缓冲：BTreeMap 有序存储临时表脏写。
pub struct MemBuffer {
    values: RwLock<BTreeMap<Key, ValueEntry>>,
}

impl MemBuffer {
    /// 空缓冲。
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入或覆盖键。
    pub fn set(&self, key: Key, value: ValueEntry) {
        self.values.write().unwrap().insert(key, value);
    }

    /// Corresponds to Go `TemporaryTableData.SetTableKey`.
    pub fn set_table_key(
        &self,
        _table_id: i64,
        key: Key,
        value: impl Into<Vec<u8>>,
    ) -> Result<(), TempTableError> {
        self.set(key, ValueEntry::new(value, 0));
        Ok(())
    }

    /// 校验 table_id 后写入空值，模拟 TiKV mem-buffer 删除语义。
    pub fn delete_table_key(&self, table_id: i64, key: &[u8]) -> Result<(), TempTableError> {
        if get_key_accessed_table_id(key) != Some(table_id) {
            return Err(TempTableError::Store(format!(
                "key does not belong to table {table_id}"
            )));
        }
        // Match TiKV mem-buffer delete semantics used by TemporaryTableData:
        // deleted keys remain readable as an empty value without ErrNotExist.
        self.values
            .write()
            .unwrap()
            .insert(key.to_vec(), ValueEntry::new(Vec::new(), 0));
        Ok(())
    }

    /// 收集 `[start, end)` 内条目，可选反向。
    fn entries(&self, start: &[u8], end: &[u8], reverse: bool) -> Vec<(Key, ValueEntry)> {
        let values = self.values.read().unwrap();
        let mut entries = values
            .iter()
            .filter(|(key, _)| key.as_slice() >= start && (end.is_empty() || key.as_slice() < end))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        if reverse {
            entries.reverse();
        }
        entries
    }
}

impl Retriever for MemBuffer {
    fn get(&self, key: &[u8]) -> Result<ValueEntry, TempTableError> {
        self.values
            .read()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(TempTableError::KeyNotExist)
    }

    fn iter(&self, start: &[u8], end: &[u8]) -> Result<Box<dyn KvIterator>, TempTableError> {
        Ok(Box::new(VecIterator::new(self.entries(start, end, false))))
    }

    fn iter_reverse(
        &self,
        end: &[u8],
        start: &[u8],
    ) -> Result<Box<dyn KvIterator>, TempTableError> {
        Ok(Box::new(VecIterator::new(self.entries(start, end, true))))
    }
}

/// 基于预物化条目向量的迭代器。
pub struct VecIterator {
    entries: Vec<(Key, ValueEntry)>,
    position: usize,
    closed: bool,
}

impl VecIterator {
    /// 由条目列表构造。
    pub fn new(entries: Vec<(Key, ValueEntry)>) -> Self {
        Self {
            entries,
            position: 0,
            closed: false,
        }
    }

    /// 空迭代器。
    pub fn empty() -> Self {
        Self::new(Vec::new())
    }
}

impl KvIterator for VecIterator {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn valid(&self) -> bool {
        !self.closed && self.position < self.entries.len()
    }

    fn key(&self) -> &[u8] {
        &self.entries[self.position].0
    }

    fn value(&self) -> &ValueEntry {
        &self.entries[self.position].1
    }

    fn next(&mut self) -> Result<(), TempTableError> {
        if self.valid() {
            self.position += 1;
        }
        Ok(())
    }

    fn close(&mut self) {
        self.closed = true;
    }

    fn closed(&self) -> bool {
        self.closed
    }
}

/// Merge session (dirty) and snapshot iterators like Go `txn.UnionIter`.
pub struct UnionIter {
    dirty_it: Option<Box<dyn KvIterator>>,
    snapshot_it: Option<Box<dyn KvIterator>>,
    dirty_valid: bool,
    snapshot_valid: bool,
    cur_is_dirty: bool,
    is_valid: bool,
    reverse: bool,
    current_value: ValueEntry,
}

impl UnionIter {
    /// 合并会话脏迭代器与快照迭代器；初始化时调用 `update_cur`。
    pub fn new(
        dirty_it: Box<dyn KvIterator>,
        snapshot_it: Box<dyn KvIterator>,
        reverse: bool,
    ) -> Result<Self, TempTableError> {
        let dirty_valid = dirty_it.valid();
        let snapshot_valid = snapshot_it.valid();
        let mut iterator = Self {
            dirty_it: Some(dirty_it),
            snapshot_it: Some(snapshot_it),
            dirty_valid,
            snapshot_valid,
            cur_is_dirty: false,
            is_valid: false,
            reverse,
            current_value: ValueEntry::default(),
        };
        if let Err(error) = iterator.update_cur() {
            if let Some(mut dirty) = iterator.dirty_it.take() {
                dirty.close();
            }
            if let Some(mut snap) = iterator.snapshot_it.take() {
                snap.close();
            }
            return Err(error);
        }
        Ok(iterator)
    }

    fn dirty_key(&self) -> &[u8] {
        self.dirty_it.as_ref().unwrap().key()
    }

    fn dirty_value(&self) -> &ValueEntry {
        self.dirty_it.as_ref().unwrap().value()
    }

    fn snapshot_key(&self) -> &[u8] {
        self.snapshot_it.as_ref().unwrap().key()
    }

    fn dirty_next(&mut self) -> Result<(), TempTableError> {
        let iterator = self.dirty_it.as_mut().expect("dirty iterator is open");
        let result = iterator.next();
        self.dirty_valid = iterator.valid();
        result
    }

    fn snapshot_next(&mut self) -> Result<(), TempTableError> {
        let iterator = self
            .snapshot_it
            .as_mut()
            .expect("snapshot iterator is open");
        let result = iterator.next();
        self.snapshot_valid = iterator.valid();
        result
    }

    /// 推进到下一个可见键：跳过脏侧空值删除，同键时脏数据优先。
    fn update_cur(&mut self) -> Result<(), TempTableError> {
        self.is_valid = true;
        loop {
            if !self.dirty_valid && !self.snapshot_valid {
                self.is_valid = false;
                return Ok(());
            }
            if !self.dirty_valid {
                self.cur_is_dirty = false;
                self.current_value = self.snapshot_it.as_ref().unwrap().value().clone();
                return Ok(());
            }
            if !self.snapshot_valid {
                self.cur_is_dirty = true;
                if self.dirty_value().is_value_empty() {
                    self.dirty_next()?;
                    continue;
                }
                self.current_value = self.dirty_value().clone();
                return Ok(());
            }

            // 反向扫描时翻转比较结果以选择“更靠前”的一侧。
            let mut ordering = self.dirty_key().cmp(self.snapshot_key());
            if self.reverse {
                ordering = ordering.reverse();
            }
            match ordering {
                Ordering::Equal => {
                    if self.dirty_value().is_value_empty() {
                        self.dirty_next()?;
                        self.snapshot_next()?;
                        continue;
                    }
                    self.snapshot_next()?;
                    self.cur_is_dirty = true;
                    self.current_value = self.dirty_value().clone();
                    return Ok(());
                }
                Ordering::Greater => {
                    self.cur_is_dirty = false;
                    self.current_value = self.snapshot_it.as_ref().unwrap().value().clone();
                    return Ok(());
                }
                Ordering::Less => {
                    if self.dirty_value().is_value_empty() {
                        self.dirty_next()?;
                        continue;
                    }
                    self.cur_is_dirty = true;
                    self.current_value = self.dirty_value().clone();
                    return Ok(());
                }
            }
        }
    }
}

impl KvIterator for UnionIter {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn valid(&self) -> bool {
        self.is_valid && self.dirty_it.is_some() && self.snapshot_it.is_some()
    }

    fn key(&self) -> &[u8] {
        if self.cur_is_dirty {
            self.dirty_it.as_ref().unwrap().key()
        } else {
            self.snapshot_it.as_ref().unwrap().key()
        }
    }

    fn value(&self) -> &ValueEntry {
        &self.current_value
    }

    fn next(&mut self) -> Result<(), TempTableError> {
        if !self.valid() {
            return Ok(());
        }
        if self.cur_is_dirty {
            self.dirty_next()?;
        } else {
            self.snapshot_next()?;
        }
        self.update_cur()
    }

    fn close(&mut self) {
        if let Some(mut iterator) = self.dirty_it.take() {
            iterator.close();
        }
        if let Some(mut iterator) = self.snapshot_it.take() {
            iterator.close();
        }
        self.is_valid = false;
    }

    fn closed(&self) -> bool {
        self.dirty_it.is_none() && self.snapshot_it.is_none()
    }
}

/// 永远 invalid 的空迭代器（全局临时表或无会话数据时使用）。
pub struct EmptyIterator {
    closed: bool,
}

impl EmptyIterator {
    pub fn new() -> Self {
        Self { closed: false }
    }
}

impl Default for EmptyIterator {
    fn default() -> Self {
        Self::new()
    }
}

impl KvIterator for EmptyIterator {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn valid(&self) -> bool {
        false
    }

    fn key(&self) -> &[u8] {
        &[]
    }

    fn value(&self) -> &ValueEntry {
        static EMPTY: std::sync::OnceLock<ValueEntry> = std::sync::OnceLock::new();
        EMPTY.get_or_init(ValueEntry::default)
    }

    fn next(&mut self) -> Result<(), TempTableError> {
        Ok(())
    }

    fn close(&mut self) {
        self.closed = true;
    }

    fn closed(&self) -> bool {
        self.closed
    }
}

/// Snapshot 读拦截钩子：Get/BatchGet/Iter/IterReverse。
pub trait SnapshotInterceptor: Send + Sync {
    fn on_get(&self, snapshot: &dyn Snapshot, key: &[u8]) -> Result<ValueEntry, TempTableError>;
    fn on_batch_get(
        &self,
        snapshot: &dyn Snapshot,
        keys: &[Key],
    ) -> Result<HashMap<Key, ValueEntry>, TempTableError>;
    fn on_iter(
        &self,
        snapshot: &dyn Snapshot,
        start: &[u8],
        upper_bound: &[u8],
    ) -> Result<Box<dyn KvIterator>, TempTableError>;
    fn on_iter_reverse(
        &self,
        snapshot: &dyn Snapshot,
        end: &[u8],
        lower_bound: &[u8],
    ) -> Result<Box<dyn KvIterator>, TempTableError>;
}

/// 根据 InfoSchema 将临时表访问重定向到会话数据。
pub struct TemporaryTableSnapshotInterceptor {
    info_schema: Arc<dyn InfoSchema>,
    session_data: Option<Arc<dyn Retriever>>,
}

impl TemporaryTableSnapshotInterceptor {
    /// 绑定 InfoSchema 与可选会话 Retriever。
    pub fn new(info_schema: Arc<dyn InfoSchema>, session_data: Option<Arc<dyn Retriever>>) -> Self {
        Self {
            info_schema,
            session_data,
        }
    }

    /// 若 ID 对应临时表则返回其元数据。
    pub fn temporary_table_info_by_id(&self, table_id: i64) -> Option<Arc<TableInfo>> {
        let metadata = self.info_schema.table_by_id(table_id)?.metadata();
        (metadata.temp_table_type != TempTableType::None).then_some(metadata)
    }

    /// 拆分键集：临时表键从会话读取，其余留给 Snapshot BatchGet。
    pub fn batch_get_temporary_table_keys(
        &self,
        keys: &[Key],
    ) -> Result<(Vec<Key>, Option<HashMap<Key, ValueEntry>>), TempTableError> {
        let mut snapshot_keys = Vec::new();
        let mut result: Option<HashMap<Key, ValueEntry>> = None;
        for key in keys {
            let Some(table_id) = get_key_accessed_table_id(key) else {
                snapshot_keys.push(key.clone());
                continue;
            };
            let Some(table_info) = self.temporary_table_info_by_id(table_id) else {
                snapshot_keys.push(key.clone());
                continue;
            };
            match get_session_key(&table_info, self.session_data.as_deref(), key) {
                Ok(value) => {
                    result
                        .get_or_insert_with(HashMap::new)
                        .insert(key.clone(), value);
                }
                Err(TempTableError::KeyNotExist) => {}
                Err(error) => return Err(error),
            }
        }
        Ok((snapshot_keys, result))
    }

    /// 单表范围迭代：非临时表走 Snapshot；Global/无会话返回 Empty；Local 合并会话。
    pub fn iter_table(
        &self,
        table_id: i64,
        snapshot: &dyn Snapshot,
        start: &[u8],
        upper_bound: &[u8],
    ) -> Result<Box<dyn KvIterator>, TempTableError> {
        let Some(table_info) = self.temporary_table_info_by_id(table_id) else {
            return snapshot.iter(start, upper_bound);
        };
        if table_info.temp_table_type == TempTableType::Global || self.session_data.is_none() {
            return Ok(Box::new(EmptyIterator::new()));
        }
        create_union_iter(
            self.session_data.as_deref(),
            None,
            start,
            upper_bound,
            false,
        )
    }
}

// 拦截实现：点查优先会话；批量合并结果；范围扫描按表或跨表 union。
impl SnapshotInterceptor for TemporaryTableSnapshotInterceptor {
    fn on_get(&self, snapshot: &dyn Snapshot, key: &[u8]) -> Result<ValueEntry, TempTableError> {
        if let Some(table_id) = get_key_accessed_table_id(key)
            && let Some(table_info) = self.temporary_table_info_by_id(table_id)
        {
            return get_session_key(&table_info, self.session_data.as_deref(), key);
        }
        snapshot.get(key)
    }

    fn on_batch_get(
        &self,
        snapshot: &dyn Snapshot,
        keys: &[Key],
    ) -> Result<HashMap<Key, ValueEntry>, TempTableError> {
        let (snapshot_keys, temporary_result) = self.batch_get_temporary_table_keys(keys)?;
        let result = temporary_result.unwrap_or_default();
        if snapshot_keys.is_empty() {
            return Ok(result);
        }
        let mut snapshot_result = snapshot.batch_get(&snapshot_keys)?;
        // Temporary table values win if a malformed snapshot happens to return
        // a duplicate key, matching maps.Copy(snapResult, result) in Go.
        snapshot_result.extend(result);
        Ok(snapshot_result)
    }

    fn on_iter(
        &self,
        snapshot: &dyn Snapshot,
        start: &[u8],
        upper_bound: &[u8],
    ) -> Result<Box<dyn KvIterator>, TempTableError> {
        if not_table_range(start, upper_bound) {
            return snapshot.iter(start, upper_bound);
        }
        if let Some(table_id) = get_range_accessed_table_id(start, upper_bound) {
            return self.iter_table(table_id, snapshot, start, upper_bound);
        }
        create_union_iter(
            self.session_data.as_deref(),
            Some(snapshot),
            start,
            upper_bound,
            false,
        )
    }

    fn on_iter_reverse(
        &self,
        snapshot: &dyn Snapshot,
        end: &[u8],
        lower_bound: &[u8],
    ) -> Result<Box<dyn KvIterator>, TempTableError> {
        if not_table_range(&[], end) {
            return snapshot.iter_reverse(end, lower_bound);
        }
        create_union_iter(
            self.session_data.as_deref(),
            Some(snapshot),
            lower_bound,
            end,
            true,
        )
    }
}

/// 若 InfoSchema 含临时表则构造拦截器，并挂上会话 temporary_table_data。
pub fn session_snapshot_interceptor(
    context: &dyn SessionVarsProvider,
    info_schema: Arc<dyn InfoSchema>,
) -> Option<Arc<dyn SnapshotInterceptor>> {
    if !info_schema.has_temporary_table() {
        return None;
    }
    let data = context
        .session_variables()
        .temporary_table_data
        .lock()
        .unwrap()
        .clone()
        .map(|data| data as Arc<dyn Retriever>);
    Some(Arc::new(TemporaryTableSnapshotInterceptor::new(
        info_schema,
        data,
    )))
}

/// 从会话读取临时表键；普通表/全局临时表/空值删除均按 Go 语义报错或 KeyNotExist。
pub fn get_session_key(
    table_info: &TableInfo,
    session_data: Option<&dyn Retriever>,
    key: &[u8],
) -> Result<ValueEntry, TempTableError> {
    if table_info.temp_table_type == TempTableType::None {
        return Err(TempTableError::NormalTableSessionRead(table_info.id));
    }
    if session_data.is_none() || table_info.temp_table_type == TempTableType::Global {
        return Err(TempTableError::KeyNotExist);
    }
    let value = session_data.unwrap().get(key)?;
    if value.is_value_empty() {
        Err(TempTableError::KeyNotExist)
    } else {
        Ok(value)
    }
}

/// 关闭并清空可选迭代器。
fn close_iterator(iterator: &mut Option<Box<dyn KvIterator>>) {
    if let Some(mut iter) = iterator.take() {
        iter.close();
    }
}

/// 创建 UnionIter：可选 Snapshot + 会话；无会话则直接返回快照侧迭代器。
pub fn create_union_iter(
    session_data: Option<&dyn Retriever>,
    snapshot: Option<&dyn Snapshot>,
    start: &[u8],
    upper_bound: &[u8],
    reverse: bool,
) -> Result<Box<dyn KvIterator>, TempTableError> {
    let snap_iter: Option<Box<dyn KvIterator>> = if let Some(snapshot) = snapshot {
        Some(if reverse {
            snapshot.iter_reverse(upper_bound, start)?
        } else {
            snapshot.iter(start, upper_bound)?
        })
    } else {
        Some(Box::new(EmptyIterator::new()))
    };
    let mut snap_iter = snap_iter;

    let Some(session_data) = session_data else {
        return Ok(snap_iter.take().unwrap());
    };

    let session_iter = if reverse {
        session_data.iter_reverse(upper_bound, start)
    } else {
        session_data.iter(start, upper_bound)
    };

    let session_iter = match session_iter {
        Ok(iterator) => iterator,
        Err(error) => {
            close_iterator(&mut snap_iter);
            return Err(error);
        }
    };

    match UnionIter::new(session_iter, snap_iter.take().unwrap(), reverse) {
        Ok(iterator) => Ok(Box::new(iterator)),
        Err(error) => Err(error),
    }
}

/// 编码表前缀 `t` + 带符号位翻转的大端 table_id。
pub fn encode_table_prefix(table_id: i64) -> Key {
    let mut key = Vec::with_capacity(9);
    key.push(b't');
    key.extend_from_slice(&((table_id as u64) ^ (1_u64 << 63)).to_be_bytes());
    key
}

/// 从表前缀字节解码 table_id。
pub fn decode_table_id(key: &[u8]) -> Option<i64> {
    let encoded: [u8; 8] = key.get(1..9)?.try_into().ok()?;
    Some((u64::from_be_bytes(encoded) ^ (1_u64 << 63)) as i64)
}

/// 若键属于合法表前缀则返回 table_id，否则 None。
pub fn get_key_accessed_table_id(key: &[u8]) -> Option<i64> {
    if !key.starts_with(b"t") || key.len() < encode_table_prefix(1).len() {
        return None;
    }
    let table_id = decode_table_id(key)?;
    (table_id > 0 && table_id != i64::MAX).then_some(table_id)
}

/// 当 `[start,end)` 落在单一表前缀区间内时返回该 table_id。
pub fn get_range_accessed_table_id(start: &[u8], end: &[u8]) -> Option<i64> {
    let table_id = get_key_accessed_table_id(start)?;
    let table_start = encode_table_prefix(table_id);
    let table_end = encode_table_prefix(table_id + 1);
    (end.starts_with(&table_start) || end == table_end).then_some(table_id)
}

/// 判断范围是否完全落在非 `t` 表键空间。
pub fn not_table_range(start: &[u8], upper_bound: &[u8]) -> bool {
    let table_prefix = b"t";
    (start.as_ref() > table_prefix && !start.starts_with(table_prefix))
        || (!upper_bound.is_empty() && upper_bound.as_ref() < table_prefix)
}

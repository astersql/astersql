// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Hash Join v1 哈希表与构建侧行容器。
//
// 提供单线程 `UnsafeHashTable`、并发 `ConcurrentMapHashTable`，以及承载 chunk、
// NULL 桶与匹配标记的 `HashRowContainer`。对应 Go `hash_table.go` / v1 实现。

use crate::concurrent_map::ConcurrentMap;
use crate::joiner::Row;
use crate::row_table_builder::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};

/// 连接键哈希上下文：列下标、每行哈希值、NULL 位图。

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HashContext {
    pub key_indices: Vec<usize>,
    pub hash_values: Vec<u64>,
    pub has_null: Vec<bool>,
    pub null_bits: Vec<u64>,
}

impl HashContext {
    /// 用连接键列下标构造空上下文。
    pub fn new(key_indices: Vec<usize>) -> Self {
        Self {
            key_indices,
            ..Self::default()
        }
    }

    /// 对一批行编码连接键并计算哈希；记录每行是否含 NULL 及 null_bits。

    pub fn init_hash(&mut self, rows: &[Row]) -> Result<(), String> {
        self.hash_values.clear();
        self.has_null.clear();
        self.null_bits.clear();
        for row in rows {
            let mut encoded = Vec::new();
            let mut null_bits = 0_u64;
            let mut has_null = false;
            for (key_pos, index) in self.key_indices.iter().copied().enumerate() {
                let value = row
                    .get(index)
                    .ok_or_else(|| format!("join key index {index} is out of bounds"))?;
                if matches!(value, Value::Null) {
                    has_null = true;
                    if key_pos < 64 {
                        null_bits |= 1_u64 << key_pos;
                    }
                }
                encode_value(value, &mut encoded);
            }
            self.hash_values.push(hash_bytes(&encoded));
            self.has_null.push(has_null);
            self.null_bits.push(null_bits);
        }
        Ok(())
    }
}

/// 行在 chunk 数组中的位置（chunk 下标 + 行下标）。

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct RowPointer {
    pub chunk_index: usize,
    pub row_index: usize,
}

/// Null-aware join 用的 NULL 键桶：存放含 NULL 的构建侧行。

#[derive(Clone, Debug, Default)]
pub struct HashNaNullBucket {
    rows: Vec<(u64, RowPointer)>,
}

impl HashNaNullBucket {
    /// 放入条目（具体类型见实现）。
    pub fn put(&mut self, null_bits: u64, pointer: RowPointer) {
        self.rows.push((null_bits, pointer));
    }
    /// 元素个数。
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

/// 哈希表冲突链节点：行指针与下一节点。

#[derive(Clone, Debug)]
pub struct Entry {
    pub pointer: RowPointer,
    pub next: Option<Box<Entry>>,
}

/// Null-aware 条目：null_bits 与行指针。

#[derive(Clone, Debug, Default)]
pub struct NaEntry {
    pub null_bits: u64,
    pub pointer: RowPointer,
}

/// Entry / NaEntry 的线性存储池。

#[derive(Clone, Debug, Default)]
pub struct EntryStore {
    entries: Vec<Entry>,
    na_entries: Vec<NaEntry>,
}

impl EntryStore {
    /// 放入条目（具体类型见实现）。
    pub fn put(&mut self, entry: Entry) -> usize {
        self.entries.push(entry);
        self.entries.len() - 1
    }
    /// 存入 NaEntry 并返回下标。
    pub fn put_na(&mut self, entry: NaEntry) -> usize {
        self.na_entries.push(entry);
        self.na_entries.len() - 1
    }
    /// 按索引取 Entry。
    pub fn entry(&self, index: usize) -> Option<&Entry> {
        self.entries.get(index)
    }
    /// 按索引取 NaEntry。
    pub fn na_entry(&self, index: usize) -> Option<&NaEntry> {
        self.na_entries.get(index)
    }
    /// 清空两类条目。
    pub fn clear(&mut self) {
        self.entries.clear();
        self.na_entries.clear();
    }
}

/// 哈希表公共接口：插入、查找、遍历与内存增量。

pub trait BaseHashTable {
    /// 放入条目（具体类型见实现）。
    fn put(&mut self, hash: u64, pointer: RowPointer) -> i64;
    /// 按哈希取冲突链上的全部行指针。
    fn get(&self, hash: u64) -> Vec<RowPointer>;
    /// 元素个数。
    fn len(&self) -> usize;
    /// 遍历所有 (hash, 行指针) 对。
    fn for_each(&self, visitor: &mut dyn FnMut(u64, RowPointer));
    /// 取出并清零累计内存增量。
    fn get_and_clean_memory_delta(&self) -> i64;
    /// 是否为空。
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 单线程哈希表：`HashMap` 桶存行指针向量。

#[derive(Default)]
pub struct UnsafeHashTable {
    buckets: HashMap<u64, Vec<RowPointer>>,
    length: usize,
    memory_delta: AtomicI64,
}

impl UnsafeHashTable {
    /// 预分配桶容量。
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            buckets: HashMap::with_capacity(capacity),
            ..Self::default()
        }
    }
}

impl BaseHashTable for UnsafeHashTable {
    /// 放入条目（具体类型见实现）。
    fn put(&mut self, hash: u64, pointer: RowPointer) -> i64 {
        let is_new = !self.buckets.contains_key(&hash);
        self.buckets.entry(hash).or_default().push(pointer);
        self.length += 1;
        let delta = std::mem::size_of::<RowPointer>() as i64
            + if is_new {
                std::mem::size_of::<u64>() as i64
            } else {
                0
            };
        self.memory_delta.fetch_add(delta, Ordering::Relaxed);
        delta
    }
    /// 按哈希取冲突链上的全部行指针。
    fn get(&self, hash: u64) -> Vec<RowPointer> {
        self.buckets.get(&hash).cloned().unwrap_or_default()
    }
    /// 元素个数。
    fn len(&self) -> usize {
        self.length
    }
    /// 遍历所有 (hash, 行指针) 对。
    fn for_each(&self, visitor: &mut dyn FnMut(u64, RowPointer)) {
        for (hash, pointers) in &self.buckets {
            for pointer in pointers {
                visitor(*hash, *pointer);
            }
        }
    }
    /// 取出并清零累计内存增量。
    fn get_and_clean_memory_delta(&self) -> i64 {
        self.memory_delta.swap(0, Ordering::AcqRel)
    }
}

/// 基于 `ConcurrentMap` 的并发哈希表，冲突链存于分片 map。

pub struct ConcurrentMapHashTable {
    buckets: ConcurrentMap<RowPointer>,
    length: usize,
    memory_delta: AtomicI64,
}

impl Default for ConcurrentMapHashTable {
    fn default() -> Self {
        Self {
            buckets: ConcurrentMap::new(),
            length: 0,
            memory_delta: AtomicI64::new(0),
        }
    }
}

impl BaseHashTable for ConcurrentMapHashTable {
    /// 放入条目（具体类型见实现）。
    fn put(&mut self, hash: u64, pointer: RowPointer) -> i64 {
        let delta = self.buckets.insert(hash, pointer) + std::mem::size_of::<RowPointer>() as i64;
        self.length += 1;
        self.memory_delta.fetch_add(delta, Ordering::Relaxed);
        delta
    }
    /// 按哈希取冲突链上的全部行指针。
    fn get(&self, hash: u64) -> Vec<RowPointer> {
        let mut rows = Vec::new();
        let mut entry = self.buckets.get(hash);
        while let Some(current) = entry {
            rows.push(current.value);
            entry = current.next.clone();
        }
        rows
    }
    /// 元素个数。
    fn len(&self) -> usize {
        self.length
    }
    /// 遍历所有 (hash, 行指针) 对。
    fn for_each(&self, visitor: &mut dyn FnMut(u64, RowPointer)) {
        self.buckets.for_each(|hash, head| {
            let mut entry = Some(head.clone());
            while let Some(current) = entry {
                visitor(hash, current.value);
                entry = current.next.clone();
            }
        });
    }
    /// 取出并清零累计内存增量。
    fn get_and_clean_memory_delta(&self) -> i64 {
        self.memory_delta.swap(0, Ordering::AcqRel)
    }
}

/// 构建侧行容器：持有 chunk、哈希表、NULL 桶与 used 标记。

pub struct HashRowContainer {
    chunks: Vec<Vec<Row>>,
    hash_table: Box<dyn BaseHashTable + Send>,
    null_bucket: HashNaNullBucket,
    key_indices: Vec<usize>,
    used: Vec<Vec<bool>>,
    spilled: bool,
    memory_bytes: i64,
    disk_bytes: i64,
}

impl HashRowContainer {
    /// 用连接键列下标构造空上下文。
    pub fn new(key_indices: Vec<usize>, concurrent: bool, estimated_rows: usize) -> Self {
        let table: Box<dyn BaseHashTable + Send> = if concurrent {
            Box::new(ConcurrentMapHashTable::default())
        } else {
            Box::new(UnsafeHashTable::with_capacity(estimated_rows))
        };
        Self {
            chunks: Vec::new(),
            hash_table: table,
            null_bucket: HashNaNullBucket::default(),
            key_indices,
            used: Vec::new(),
            spilled: false,
            memory_bytes: 0,
            disk_bytes: 0,
        }
    }

    /// 写入一块构建侧行：算哈希，NULL 进 null_bucket，否则进哈希表。

    pub fn put_chunk(&mut self, rows: Vec<Row>) -> Result<(), String> {
        let mut context = HashContext::new(self.key_indices.clone());
        context.init_hash(&rows)?;
        let chunk_index = self.chunks.len();
        self.used.push(vec![false; rows.len()]);
        for row_index in 0..rows.len() {
            let pointer = RowPointer {
                chunk_index,
                row_index,
            };
            // 含 NULL 的连接键进入 null-aware 桶；其余进入普通哈希表。
            // 含 NULL 的连接键进入 null-aware 桶；其余进入普通哈希表。
            if context.has_null[row_index] {
                self.null_bucket.put(context.null_bits[row_index], pointer);
            } else {
                self.memory_bytes += self.hash_table.put(context.hash_values[row_index], pointer);
            }
        }
        self.memory_bytes += rows.iter().map(row_size).sum::<usize>() as i64;
        self.chunks.push(rows);
        Ok(())
    }

    /// 用容器自身键列探测匹配行。

    pub fn get_matched_rows(&self, probe: &Row) -> Result<Vec<RowPointer>, String> {
        self.get_matched_rows_by_indices(probe, &self.key_indices)
    }
    /// 按探测侧键列哈希查找并做键等值过滤；探测键含 NULL 则无匹配。
    pub fn get_matched_rows_by_indices(
        &self,
        probe: &Row,
        probe_key_indices: &[usize],
    ) -> Result<Vec<RowPointer>, String> {
        if probe_key_indices.len() != self.key_indices.len() {
            return Err("build and probe key counts differ".into());
        }
        let mut context = HashContext::new(probe_key_indices.to_vec());
        context.init_hash(std::slice::from_ref(probe))?;
        // 普通等值 join：探测键含 NULL 时不匹配任何构建行。
        // 普通等值 join：探测键含 NULL 时不匹配任何构建行。
        if context.has_null[0] {
            return Ok(Vec::new());
        }
        Ok(self
            .hash_table
            .get(context.hash_values[0])
            .into_iter()
            .filter(|pointer| {
                self.row(*pointer).is_some_and(|build| {
                    keys_equal_cross(build, probe, &self.key_indices, probe_key_indices)
                })
            })
            .collect())
    }
    /// Null-aware 查找：等值匹配并集兼容的 NULL 桶行。
    pub fn get_na_rows(&self, probe: &Row) -> Result<Vec<RowPointer>, String> {
        self.get_na_rows_by_indices(probe, &self.key_indices)
    }
    /// 按探测侧键列做 null-aware 匹配。
    pub fn get_na_rows_by_indices(
        &self,
        probe: &Row,
        probe_key_indices: &[usize],
    ) -> Result<Vec<RowPointer>, String> {
        let mut rows = self.get_matched_rows_by_indices(probe, probe_key_indices)?;
        rows.extend(self.null_bucket.rows.iter().filter_map(|(_, pointer)| {
            self.row(*pointer)
                .filter(|build| {
                    na_keys_compatible(build, probe, &self.key_indices, probe_key_indices)
                })
                .map(|_| *pointer)
        }));
        Ok(rows)
    }
    /// 按行指针取构建侧行。
    pub fn row(&self, pointer: RowPointer) -> Option<&Row> {
        self.chunks.get(pointer.chunk_index)?.get(pointer.row_index)
    }
    /// 标记该构建行已被匹配（供 outer/anti 扫未匹配行）。
    pub fn mark_used(&mut self, pointer: RowPointer) {
        if let Some(used) = self
            .used
            .get_mut(pointer.chunk_index)
            .and_then(|chunk| chunk.get_mut(pointer.row_index))
        {
            *used = true;
        }
    }
    /// 返回尚未被 mark_used 的构建侧行。
    pub fn unmatched_rows(&self) -> Vec<&Row> {
        self.chunks
            .iter()
            .enumerate()
            .flat_map(|(chunk_index, chunk)| {
                chunk
                    .iter()
                    .enumerate()
                    .filter(move |(row_index, _)| !self.used[chunk_index][*row_index])
                    .map(|(_, row)| row)
            })
            .collect()
    }
    /// 标记已 spill，并把内存字节计入磁盘字节。
    pub fn spill(&mut self) {
        if !self.spilled {
            self.spilled = true;
            self.disk_bytes += self.memory_bytes;
            self.memory_bytes = 0;
        }
    }
    /// 是否已 spill。
    pub fn already_spilled(&self) -> bool {
        self.spilled
    }
    /// 元素个数。
    pub fn len(&self) -> usize {
        self.used.iter().map(Vec::len).sum()
    }
    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// 当前估算内存占用。
    pub fn memory_bytes(&self) -> i64 {
        self.memory_bytes
    }
    /// 已计入磁盘的字节。
    pub fn disk_bytes(&self) -> i64 {
        self.disk_bytes
    }
    /// 释放 chunk 与 used 标记。
    pub fn close(&mut self) {
        self.chunks.clear();
        self.used.clear();
        self.null_bucket.rows.clear();
        self.memory_bytes = 0;
    }
}

/// 同侧键列等值比较；任一侧为 NULL 则不等。

fn keys_equal(left: &Row, right: &Row, indices: &[usize]) -> bool {
    indices
        .iter()
        .all(|index| match (left.get(*index), right.get(*index)) {
            (Some(Value::Null), _) | (_, Some(Value::Null)) => false,
            (Some(a), Some(b)) => a == b,
            _ => false,
        })
}
/// 构建/探测两侧不同键列下标的交叉等值比较。
fn keys_equal_cross(
    left: &Row,
    right: &Row,
    left_indices: &[usize],
    right_indices: &[usize],
) -> bool {
    left_indices
        .iter()
        .zip(right_indices)
        .all(
            |(left_index, right_index)| match (left.get(*left_index), right.get(*right_index)) {
                (Some(Value::Null), _) | (_, Some(Value::Null)) => false,
                (Some(a), Some(b)) => a == b,
                _ => false,
            },
        )
}
/// NAAJ 键比较：任一侧为 NULL 的列不参与比较，其余列必须相等。
fn na_keys_compatible(
    build: &Row,
    probe: &Row,
    build_indices: &[usize],
    probe_indices: &[usize],
) -> bool {
    build_indices
        .iter()
        .zip(probe_indices)
        .all(|(build_index, probe_index)| {
            match (build.get(*build_index), probe.get(*probe_index)) {
                (Some(Value::Null), _) | (_, Some(Value::Null)) => true,
                (Some(build_value), Some(probe_value)) => build_value == probe_value,
                _ => false,
            }
        })
}
/// FNV-1a 风格字节哈希。
fn hash_bytes(bytes: &[u8]) -> u64 {
    bytes.iter().fold(1469598103934665603, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(1099511628211)
    })
}
/// 把 `Value` 编码进字节缓冲供哈希。
fn encode_value(value: &Value, output: &mut Vec<u8>) {
    match value {
        Value::Null => output.push(0),
        Value::Bool(v) => output.extend_from_slice(&[1, u8::from(*v)]),
        Value::Int(v) => {
            output.push(2);
            output.extend_from_slice(&v.to_le_bytes());
        }
        Value::UInt(v) => {
            output.push(3);
            output.extend_from_slice(&v.to_le_bytes());
        }
        Value::Float(v) => {
            output.push(4);
            output.extend_from_slice(&v.to_bits().to_le_bytes());
        }
        Value::Bytes(v) => {
            output.push(5);
            output.extend_from_slice(&(v.len() as u64).to_le_bytes());
            output.extend_from_slice(v);
        }
        Value::Text(v) => {
            output.push(6);
            output.extend_from_slice(&(v.len() as u64).to_le_bytes());
            output.extend_from_slice(v.as_bytes());
        }
    }
}
/// 估算一行字节占用。
fn row_size(row: &Row) -> usize {
    row.iter()
        .map(|v| match v {
            Value::Bytes(v) => v.len(),
            Value::Text(v) => v.len(),
            _ => 8,
        })
        .sum()
}

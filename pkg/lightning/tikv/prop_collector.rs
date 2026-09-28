// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
// Copyright 2026 AsterSQL.

// SST 表属性收集器：MVCC 统计与 Range 索引。
//
// TiKV Ingest SST 时依赖这些属性（行数、版本时间戳范围、rows_index、
// range_index）做校验与切分。MVCC（多版本并发控制）键末 8 字节为时间戳。

use crate::TikvError;
use std::collections::HashMap;

/// RocksDB/Pebble 风格的内部键包装（此处仅保留 user_key）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InternalKey {
    /// 已编码的用户键（含 MVCC 后缀时由调用方保证）。
    pub user_key: Vec<u8>,
}

impl InternalKey {
    /// 由用户键字节构造。
    pub fn new(user_key: Vec<u8>) -> Self {
        Self { user_key }
    }
}

/// 表属性收集器接口：逐键 Add，结束时 Finish 写入属性字典。
pub trait TablePropertyCollector {
    /// 处理一条键值，更新内部统计。
    fn Add(&mut self, key: &InternalKey, value: &[u8]) -> Result<(), TikvError>;
    /// 将收集结果写入 `properties`。
    fn Finish(&mut self, properties: &mut HashMap<String, Vec<u8>>) -> Result<(), TikvError>;
    /// 收集器注册名。
    fn Name(&self) -> &str;
}

/// 空操作收集器，用于测试或占位。
pub struct MockCollector {
    name: String,
}

impl MockCollector {
    /// 以给定名称创建。
    pub fn new(name: &str) -> Self {
        Self { name: name.into() }
    }
}

impl TablePropertyCollector for MockCollector {
    fn Add(&mut self, _key: &InternalKey, _value: &[u8]) -> Result<(), TikvError> {
        Ok(())
    }
    fn Finish(&mut self, _properties: &mut HashMap<String, Vec<u8>>) -> Result<(), TikvError> {
        Ok(())
    }
    fn Name(&self) -> &str {
        &self.name
    }
}

/// rows_index 中的一个锚点：键前缀、区间大小与全局偏移。
#[derive(Clone)]
struct IndexHandle {
    key: Vec<u8>,
    size: u64,
    offset: u64,
}

/// MVCC 属性收集器：统计行数/版本，并按间隔生成 rows_index。
pub struct MvccPropCollector {
    /// SST 统一时间戳，写入 min_ts/max_ts。
    ts: u64,
    /// 累计行（版本）数。
    rows: u64,
    /// 最近一条去掉 ts 后缀后的行键。
    last_row: Vec<u8>,
    /// 距上一锚点的键数。
    current_size: u64,
    /// 全局已处理键数。
    current_offset: u64,
    /// rows_index 锚点列表。
    handles: Vec<IndexHandle>,
}

/// 按时间戳创建 `MvccPropCollector`（Go 风格命名）。
pub fn newMVCCPropCollector(ts: u64) -> MvccPropCollector {
    MvccPropCollector::new(ts)
}

impl MvccPropCollector {
    /// 初始化空收集器。
    pub fn new(ts: u64) -> Self {
        Self {
            ts,
            rows: 0,
            last_row: Vec::new(),
            current_size: 0,
            current_offset: 0,
            handles: Vec::new(),
        }
    }

    /// 计入一行；调用方须保证键包含 8 字节时间戳后缀。
    ///
    /// 每约 10000 键记一个 rows_index 锚点。
    pub fn Add(&mut self, key: &InternalKey, _value: &[u8]) -> Result<(), TikvError> {
        self.rows = self.rows.wrapping_add(1);
        self.current_size = self.current_size.wrapping_add(1);
        self.current_offset = self.current_offset.wrapping_add(1);
        // 去掉末 8 字节时间戳，得到可比的行键前缀。
        self.last_row = key.user_key[..key.user_key.len() - 8].to_vec();
        if self.current_offset == 1 || self.current_size >= 10_000 {
            self.handles.push(IndexHandle {
                key: self.last_row.clone(),
                size: self.current_size,
                offset: self.current_offset,
            });
            self.current_size = 0;
        }
        Ok(())
    }

    /// 冲刷尾部锚点，写入 tikv.* 统计与 rows_index。
    pub fn Finish(&mut self, properties: &mut HashMap<String, Vec<u8>>) -> Result<(), TikvError> {
        if self.current_size > 0 {
            self.handles.push(IndexHandle {
                key: self.last_row.clone(),
                size: self.current_size,
                offset: self.current_offset,
            });
        }
        for (name, value) in [
            ("tikv.min_ts", self.ts),
            ("tikv.max_ts", self.ts),
            ("tikv.num_rows", self.rows),
            ("tikv.num_puts", self.rows),
            ("tikv.num_deletes", 0),
            ("tikv.num_versions", self.rows),
            ("tikv.max_row_versions", 1),
            ("tikv.num_errors", 0),
        ] {
            properties.insert(name.into(), value.to_be_bytes().to_vec());
        }
        self.handles.sort_by(|left, right| left.key.cmp(&right.key));
        let mut index = Vec::new();
        for handle in &self.handles {
            put_bytes(&mut index, &handle.key);
            index.extend_from_slice(&handle.size.to_be_bytes());
            index.extend_from_slice(&handle.offset.to_be_bytes());
        }
        properties.insert("tikv.rows_index".into(), index);
        Ok(())
    }

    /// 收集器名称。
    pub fn Name(&self) -> &str {
        "tikv.mvcc-properties-collector"
    }
}

impl TablePropertyCollector for MvccPropCollector {
    fn Add(&mut self, key: &InternalKey, value: &[u8]) -> Result<(), TikvError> {
        MvccPropCollector::Add(self, key, value)
    }
    fn Finish(&mut self, properties: &mut HashMap<String, Vec<u8>>) -> Result<(), TikvError> {
        MvccPropCollector::Finish(self, properties)
    }
    fn Name(&self) -> &str {
        MvccPropCollector::Name(self)
    }
}

/// 区间内累计的字节与键数偏移。
#[derive(Clone, Copy, Default)]
struct RangeOffsets {
    size: u64,
    keys: u64,
}

/// range_index 中的一个采样点。
#[derive(Clone)]
struct RangeProperty {
    key: Vec<u8>,
    offsets: RangeOffsets,
}

/// 将 RangeProperty 列表编码为「len+key+size+keys」字节流。
fn encode(properties: &[RangeProperty]) -> Vec<u8> {
    let mut encoded = Vec::new();
    for property in properties {
        put_bytes(&mut encoded, &property.key);
        encoded.extend_from_slice(&property.offsets.size.to_be_bytes());
        encoded.extend_from_slice(&property.offsets.keys.to_be_bytes());
    }
    encoded
}

/// 追加「u64 长度 + 内容」。
fn put_bytes(output: &mut Vec<u8>, value: &[u8]) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value);
}

/// 按大小/键数距离采样的 Range 属性收集器。
pub struct RangePropertiesCollector {
    properties: Vec<RangeProperty>,
    last_offsets: RangeOffsets,
    last_key: Vec<u8>,
    current_offsets: RangeOffsets,
    /// 触发新锚点的字节距离阈值（默认 4MiB）。
    pub size_index_distance: u64,
    /// 触发新锚点的键数距离阈值（默认 40K）。
    pub keys_index_distance: u64,
}

/// 创建默认距离阈值的 Range 收集器。
pub fn newRangePropertiesCollector() -> RangePropertiesCollector {
    RangePropertiesCollector::new()
}

impl Default for RangePropertiesCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl RangePropertiesCollector {
    /// 使用 TiKV 默认采样距离初始化。
    pub fn new() -> Self {
        Self {
            properties: Vec::new(),
            last_offsets: RangeOffsets::default(),
            last_key: Vec::new(),
            current_offsets: RangeOffsets::default(),
            size_index_distance: 4 * 1024 * 1024,
            keys_index_distance: 40 * 1024,
        }
    }
    /// 自上一锚点以来累计的字节增量。
    pub fn sizeInLastRange(&self) -> u64 {
        self.current_offsets
            .size
            .wrapping_sub(self.last_offsets.size)
    }
    /// 自上一锚点以来累计的键数增量。
    pub fn keysInLastRange(&self) -> u64 {
        self.current_offsets
            .keys
            .wrapping_sub(self.last_offsets.keys)
    }
    /// 在当前位置插入一个 range_index 采样点。
    pub fn insertNewPoint(&mut self, key: &[u8]) {
        self.last_offsets = self.current_offsets;
        self.properties.push(RangeProperty {
            key: key.to_vec(),
            offsets: self.current_offsets,
        });
    }
    /// 累计 size/keys；达到距离阈值时插入锚点。
    pub fn Add(&mut self, key: &InternalKey, value: &[u8]) -> Result<(), TikvError> {
        self.current_offsets.size = self
            .current_offsets
            .size
            .wrapping_add(key.user_key.len() as u64)
            .wrapping_add(value.len() as u64);
        self.current_offsets.keys = self.current_offsets.keys.wrapping_add(1);
        if self.last_key.is_empty()
            || self.sizeInLastRange() >= self.size_index_distance
            || self.keysInLastRange() >= self.keys_index_distance
        {
            self.insertNewPoint(&key.user_key);
        }
        self.last_key = key.user_key.clone();
        Ok(())
    }
    /// 冲刷尾部区间并写入 `tikv.range_index`。
    pub fn Finish(&mut self, properties: &mut HashMap<String, Vec<u8>>) -> Result<(), TikvError> {
        if self.sizeInLastRange() > 0 || self.keysInLastRange() > 0 {
            let key = self.last_key.clone();
            self.insertNewPoint(&key);
        }
        properties.insert("tikv.range_index".into(), encode(&self.properties));
        Ok(())
    }
    /// 收集器名称。
    pub fn Name(&self) -> &str {
        "tikv.range-properties-collector"
    }
}

impl TablePropertyCollector for RangePropertiesCollector {
    fn Add(&mut self, key: &InternalKey, value: &[u8]) -> Result<(), TikvError> {
        RangePropertiesCollector::Add(self, key, value)
    }
    fn Finish(&mut self, properties: &mut HashMap<String, Vec<u8>>) -> Result<(), TikvError> {
        RangePropertiesCollector::Finish(self, properties)
    }
    fn Name(&self) -> &str {
        RangePropertiesCollector::Name(self)
    }
}

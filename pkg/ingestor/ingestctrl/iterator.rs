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

// 本地 ingest Engine 的键值迭代器与键适配器。
//
// 提供有序 KV 扫描（`PebbleIter`）、重复键检测迭代器（`DupDetectIter`）、
// 重复结果库迭代器（`DupDBIter`），以及将业务键与 row_id 编解码的 `KeyAdapter`。
// 重复检测通过在原始键后追加分隔符与编码后的 row_id，使同一业务键的多版本可区分排序。

use std::sync::{Arc, Mutex};

use crate::{Error, KvPair, Result};

/// 键适配器：在业务键与存储键之间做编解码（可嵌入 row_id 以区分重复）。
pub trait KeyAdapter: Send + Sync {
    /// 将业务键与行号编码为存储键。
    fn Encode(&self, key: &[u8], row_id: i64) -> Vec<u8>;
    /// 从存储键还原业务键。
    fn Decode(&self, key: &[u8]) -> Result<Vec<u8>>;
}

#[derive(Default)]
/// 空操作适配器：编解码均原样返回，不做重复键区分。
pub struct NoopKeyAdapter;

impl KeyAdapter for NoopKeyAdapter {
    fn Encode(&self, key: &[u8], _row_id: i64) -> Vec<u8> {
        key.to_vec()
    }

    fn Decode(&self, key: &[u8]) -> Result<Vec<u8>> {
        Ok(key.to_vec())
    }
}

#[derive(Default)]
/// 重复检测键适配器：mem-comparable 编码业务键，再追加 row_id 与其长度。
pub struct DupDetectKeyAdapter;

fn encode_memcomparable_bytes(key: &[u8]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity((key.len() / 8 + 1) * 9);
    for chunk in key.chunks(8) {
        encoded.extend_from_slice(chunk);
        let padding = 8 - chunk.len();
        encoded.extend(std::iter::repeat_n(0, padding));
        encoded.push(0xff - padding as u8);
    }
    if key.len().is_multiple_of(8) {
        encoded.extend_from_slice(&[0; 8]);
        encoded.push(0xf7);
    }
    encoded
}

fn decode_memcomparable_bytes(data: &[u8]) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    let mut offset = 0;
    while offset + 9 <= data.len() {
        let group = &data[offset..offset + 9];
        let padding = (0xff - group[8]) as usize;
        if padding > 8 || group[8 - padding..8].iter().any(|byte| *byte != 0) {
            return Err(Error::InvalidData(
                "invalid mem-comparable key encoding".into(),
            ));
        }
        decoded.extend_from_slice(&group[..8 - padding]);
        offset += 9;
        if padding > 0 && offset == data.len() {
            return Ok(decoded);
        }
        if padding > 0 {
            break;
        }
    }
    Err(Error::InvalidData(
        "insufficient bytes to decode duplicate-detect key".into(),
    ))
}

impl KeyAdapter for DupDetectKeyAdapter {
    fn Encode(&self, key: &[u8], row_id: i64) -> Vec<u8> {
        let mut encoded = encode_memcomparable_bytes(key);
        encoded.extend_from_slice(&(row_id ^ i64::MIN).to_be_bytes());
        encoded.extend_from_slice(&8_u16.to_be_bytes());
        encoded
    }

    fn Decode(&self, key: &[u8]) -> Result<Vec<u8>> {
        if key.len() < 2 {
            return Err(Error::InvalidData(
                "insufficient bytes to decode duplicate-detect key".into(),
            ));
        }
        let row_id_len = u16::from_be_bytes([key[key.len() - 2], key[key.len() - 1]]) as usize;
        let suffix_len = row_id_len + 2;
        if key.len() < suffix_len {
            return Err(Error::InvalidData(
                "insufficient bytes to decode duplicate-detect key".into(),
            ));
        }
        decode_memcomparable_bytes(&key[..key.len() - suffix_len])
    }
}

/// 基础有序迭代器接口（Valid/Next/Key/Value/Close/Error）。
pub trait Iter {
    /// 当前是否指向有效条目。
    fn Valid(&self) -> bool;
    /// 前进到下一条；返回是否仍有效。
    fn Next(&mut self) -> bool;
    /// 当前键（调用方需保证 Valid）。
    fn Key(&self) -> &[u8];
    /// 当前值。
    fn Value(&self) -> &[u8];
    /// 关闭迭代器并释放资源。
    fn Close(&mut self) -> Result<()>;
    /// 迭代过程中累积的错误（若有）。
    fn Error(&self) -> Option<&Error>;
}

/// 本地 Engine 迭代器扩展：支持定位首/末条与释放内部缓冲。
pub trait IngestLocalEngineIter: Iter {
    /// 定位到第一条；返回是否有效。
    fn First(&mut self) -> bool;
    /// 定位到最后一条；返回是否有效。
    fn Last(&mut self) -> bool;
    /// 释放内部可复用缓冲区（调用方若需保留 Key/Value 须先拷贝）。
    fn ReleaseBuf(&mut self);
}

/// 内存有序 KV 迭代器（模拟 Pebble 迭代行为，供本地 Engine 使用）。
pub struct PebbleIter {
    /// 已按键排序并按 [lower, upper) 过滤后的键值对。
    pairs: Vec<KvPair>,
    /// 当前游标；`None` 表示尚未定位或已关闭。
    current: Option<usize>,
    /// 是否已关闭。
    closed: bool,
}

impl PebbleIter {
    /// 排序并保留落在 `[lower, upper)` 内的键值对后构造迭代器；空 upper 表示无上界。
    pub fn new(mut pairs: Vec<KvPair>, lower: &[u8], upper: &[u8]) -> Self {
        pairs.sort_by(|left, right| left.key.cmp(&right.key));
        // 半开区间 [lower, upper)；空 upper 表示无上界
        pairs.retain(|pair| {
            pair.key.as_slice() >= lower && (upper.is_empty() || pair.key.as_slice() < upper)
        });
        Self {
            pairs,
            current: None,
            closed: false,
        }
    }
}

impl Iter for PebbleIter {
    fn Valid(&self) -> bool {
        !self.closed && self.current.is_some_and(|index| index < self.pairs.len())
    }

    fn Next(&mut self) -> bool {
        if let Some(current) = self.current.as_mut() {
            *current += 1;
        }
        self.Valid()
    }

    fn Key(&self) -> &[u8] {
        &self.pairs[self.current.expect("iterator is invalid")].key
    }

    fn Value(&self) -> &[u8] {
        &self.pairs[self.current.expect("iterator is invalid")].value
    }

    fn Close(&mut self) -> Result<()> {
        self.closed = true;
        self.pairs.clear();
        self.current = None;
        Ok(())
    }

    fn Error(&self) -> Option<&Error> {
        None
    }
}

impl IngestLocalEngineIter for PebbleIter {
    fn First(&mut self) -> bool {
        self.current = (!self.pairs.is_empty()).then_some(0);
        self.Valid()
    }

    fn Last(&mut self) -> bool {
        self.current = self.pairs.len().checked_sub(1);
        self.Valid()
    }

    fn ReleaseBuf(&mut self) {}
}

/// 重复键检测迭代器：解码业务键，若与前一条相同则记入 duplicates。
pub struct DupDetectIter {
    /// 底层按编码键排序的迭代器。
    inner: PebbleIter,
    /// 用于编解码业务键的适配器。
    adapter: Arc<dyn KeyAdapter>,
    /// 当前解码后的业务键缓存。
    decoded_key: Vec<u8>,
    /// 当前对外暴露的值；底层迭代器可能已越过同一业务键的重复项。
    current_value: Vec<u8>,
    /// 检测到的重复键值对收集器（共享）。
    duplicates: Arc<Mutex<Vec<KvPair>>>,
    /// 解码失败等迭代错误。
    error: Option<Error>,
}

impl DupDetectIter {
    /// 构造重复检测迭代器。
    pub fn new(
        pairs: Vec<KvPair>,
        adapter: Arc<dyn KeyAdapter>,
        // 用适配器编码上下界后构造；扫描时会把重复业务键写入 `duplicates`。
        duplicates: Arc<Mutex<Vec<KvPair>>>,
        lower: &[u8],
        upper: &[u8],
    ) -> Self {
        let encoded_lower = if lower.is_empty() {
            Vec::new()
        } else {
            adapter.Encode(lower, i64::MIN)
        };
        let encoded_upper = if upper.is_empty() {
            Vec::new()
        } else {
            adapter.Encode(upper, i64::MIN)
        };
        Self {
            inner: PebbleIter::new(pairs, &encoded_lower, &encoded_upper),
            adapter,
            decoded_key: Vec::new(),
            current_value: Vec::new(),
            duplicates,
            error: None,
        }
    }

    /// 解码当前底层键到 `decoded_key`；失败则记录 error 并返回 false。
    fn fill(&mut self) -> bool {
        match self.adapter.Decode(self.inner.Key()) {
            Ok(key) => {
                self.decoded_key = key;
                self.current_value = self.inner.Value().to_vec();
                true
            }
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }
}

impl Iter for DupDetectIter {
    fn Valid(&self) -> bool {
        self.error.is_none() && self.inner.Valid()
    }

    fn Next(&mut self) -> bool {
        if self.error.is_some() {
            return false;
        }
        let previous_key = self.decoded_key.clone();
        let previous_value = self.current_value.clone();
        let mut duplicate_group = Vec::new();
        while self.inner.Next() {
            let next_key = match self.adapter.Decode(self.inner.Key()) {
                Ok(key) => key,
                Err(error) => {
                    self.error = Some(error);
                    return false;
                }
            };
            let next_value = self.inner.Value().to_vec();
            if next_key != previous_key {
                self.decoded_key = next_key;
                self.current_value = next_value;
                if !duplicate_group.is_empty() {
                    match self.duplicates.lock() {
                        Ok(mut duplicates) => duplicates.extend(duplicate_group),
                        Err(_) => {
                            self.error = Some(Error::Poisoned);
                            return false;
                        }
                    }
                }
                return true;
            }
            if duplicate_group.is_empty() {
                duplicate_group.push(KvPair {
                    key: previous_key.clone(),
                    value: previous_value.clone(),
                });
            }
            duplicate_group.push(KvPair {
                key: next_key,
                value: next_value,
            });
        }
        if !duplicate_group.is_empty() {
            match self.duplicates.lock() {
                Ok(mut duplicates) => duplicates.extend(duplicate_group),
                Err(_) => self.error = Some(Error::Poisoned),
            }
        }
        false
    }

    fn Key(&self) -> &[u8] {
        &self.decoded_key
    }

    fn Value(&self) -> &[u8] {
        &self.current_value
    }

    fn Close(&mut self) -> Result<()> {
        self.inner.Close()
    }

    fn Error(&self) -> Option<&Error> {
        self.error.as_ref().or_else(|| self.inner.Error())
    }
}

impl IngestLocalEngineIter for DupDetectIter {
    fn First(&mut self) -> bool {
        self.inner.First() && self.fill()
    }

    fn Last(&mut self) -> bool {
        self.inner.Last() && self.fill()
    }

    fn ReleaseBuf(&mut self) {}
}

/// 重复结果库迭代器：遍历已写入 dup DB 的编码键并解码为业务键。
pub struct DupDBIter {
    inner: PebbleIter,
    adapter: Arc<dyn KeyAdapter>,
    decoded_key: Vec<u8>,
    error: Option<Error>,
}

impl DupDBIter {
    /// 构造重复库迭代器：对编码键范围解码后暴露业务键。
    pub fn new(
        pairs: Vec<KvPair>,
        adapter: Arc<dyn KeyAdapter>,
        lower: &[u8],
        upper: &[u8],
    ) -> Self {
        let encoded_lower = if lower.is_empty() {
            Vec::new()
        } else {
            adapter.Encode(lower, i64::MIN)
        };
        let encoded_upper = if upper.is_empty() {
            Vec::new()
        } else {
            adapter.Encode(upper, i64::MIN)
        };
        Self {
            inner: PebbleIter::new(pairs, &encoded_lower, &encoded_upper),
            adapter,
            decoded_key: Vec::new(),
            error: None,
        }
    }

    /// 解码当前键；失败时记录 error。
    fn decode(&mut self) -> bool {
        match self.adapter.Decode(self.inner.Key()) {
            Ok(key) => {
                self.decoded_key = key;
                true
            }
            Err(error) => {
                self.error = Some(error);
                false
            }
        }
    }

    /// 定位到第一条并解码。
    pub fn First(&mut self) -> bool {
        self.inner.First() && self.decode()
    }
    /// 定位到最后一条并解码。
    pub fn Last(&mut self) -> bool {
        self.inner.Last() && self.decode()
    }
}

impl Iter for DupDBIter {
    fn Valid(&self) -> bool {
        self.error.is_none() && self.inner.Valid()
    }
    fn Next(&mut self) -> bool {
        self.inner.Next() && self.decode()
    }
    fn Key(&self) -> &[u8] {
        &self.decoded_key
    }
    fn Value(&self) -> &[u8] {
        self.inner.Value()
    }
    fn Close(&mut self) -> Result<()> {
        self.inner.Close()
    }
    fn Error(&self) -> Option<&Error> {
        self.error.as_ref().or_else(|| self.inner.Error())
    }
}

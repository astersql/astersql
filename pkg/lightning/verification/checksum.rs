// Copyright 2019 PingCAP, Inc.
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
// Copyright 2026 AsterSQL.

// Lightning 导入校验用的 KV 校验和（checksum）实现。
//
// 对每对 (key, value) 做 ECMA CRC64 后 XOR 聚合，并统计字节数与 KV 条数。
// 支持 keyspace 前缀、分组（数据行 vs 索引）及可交换的 Add/Sub 合并。

use std::collections::HashMap;
use std::fmt;

/// ECMA 反射 CRC64 多项式（与 Go hash/crc64 ECMA 表一致）。
const ECMA_REVERSED_POLYNOMIAL: u64 = 0xc96c_5795_d787_0f42;

/// 一对原始键值。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KvPair {
    /// 键字节。
    pub key: Vec<u8>,
    /// 值字节。
    pub val: Vec<u8>,
}

/// 可增量更新的 KV 校验和状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KVChecksum {
    /// keyspace 前缀的 CRC 基底，Update 时先混入。
    base: u64,
    /// keyspace 字节长度，计入 SumSize。
    prefix_len: usize,
    /// 累计字节数（含 keyspace + key + value）。
    bytes: u64,
    /// 累计 KV 条数。
    kvs: u64,
    /// 各条 CRC 的 XOR 聚合结果。
    checksum: u64,
}

/// 反射表风格的 CRC64 更新：对输入/累加器取反，使分段更新等价于连续流。
fn update_crc(initial: u64, bytes: &[u8]) -> u64 {
    // This is hash/crc64.Update's reflected-table algorithm. Update
    // complements both the input and output accumulator, so repeated calls
    // are equivalent to feeding one continuous byte stream.
    let mut checksum = !initial;
    for byte in bytes {
        checksum ^= u64::from(*byte);
        for _ in 0..8 {
            checksum = if checksum & 1 == 0 {
                checksum >> 1
            } else {
                (checksum >> 1) ^ ECMA_REVERSED_POLYNOMIAL
            };
        }
    }
    !checksum
}

/// 创建空的校验和累加器。
pub fn NewKVChecksum() -> KVChecksum {
    KVChecksum::default()
}
/// 带 keyspace 前缀的校验和（先对 keyspace 做 CRC 作为 base）。
pub fn NewKVChecksumWithKeyspace(keyspace: &[u8]) -> KVChecksum {
    KVChecksum {
        base: update_crc(0, keyspace),
        prefix_len: keyspace.len(),
        ..Default::default()
    }
}
/// 由已有统计字段直接构造（用于远程聚合结果还原）。
pub fn MakeKVChecksum(bytes: u64, kvs: u64, checksum: u64) -> KVChecksum {
    KVChecksum {
        bytes,
        kvs,
        checksum,
        ..Default::default()
    }
}
/// 带 keyspace 的已有统计构造。
pub fn MakeKVChecksumWithKeyspace(
    keyspace: &[u8],
    bytes: u64,
    kvs: u64,
    checksum: u64,
) -> KVChecksum {
    KVChecksum {
        base: update_crc(0, keyspace),
        prefix_len: keyspace.len(),
        bytes,
        kvs,
        checksum,
    }
}

impl KVChecksum {
    /// 混入单条 KV：CRC(key)+CRC(val) 再 XOR 到总校验和。
    pub fn UpdateOne(&mut self, pair: &KvPair) {
        let sum = update_crc(update_crc(self.base, &pair.key), &pair.val);
        let pair_bytes = (self.prefix_len as u64)
            .wrapping_add(pair.key.len() as u64)
            .wrapping_add(pair.val.len() as u64);
        self.bytes = self.bytes.wrapping_add(pair_bytes);
        self.kvs = self.kvs.wrapping_add(1);
        self.checksum ^= sum;
    }
    /// 批量 UpdateOne。
    pub fn Update(&mut self, pairs: &[KvPair]) {
        for pair in pairs {
            self.UpdateOne(pair);
        }
    }
    /// 合并另一累加器（XOR 校验和，加法统计）。
    pub fn Add(&mut self, other: &KVChecksum) {
        self.bytes = self.bytes.wrapping_add(other.bytes);
        self.kvs = self.kvs.wrapping_add(other.kvs);
        self.checksum ^= other.checksum;
    }
    /// 从本累加器减去另一累加器（XOR 可逆）。
    pub fn Sub(&mut self, other: &KVChecksum) {
        self.bytes = self.bytes.wrapping_sub(other.bytes);
        self.kvs = self.kvs.wrapping_sub(other.kvs);
        self.checksum ^= other.checksum;
    }
    /// 当前 XOR 校验和。
    pub fn Sum(&self) -> u64 {
        self.checksum
    }
    /// 累计字节数。
    pub fn SumSize(&self) -> u64 {
        self.bytes
    }
    /// 累计 KV 条数。
    pub fn SumKVS(&self) -> u64 {
        self.kvs
    }
    /// 写入结构化日志字段。
    pub fn MarshalLogObject<E: LogEncoder + ?Sized>(
        &self,
        encoder: &mut E,
    ) -> Result<(), E::Error> {
        encoder.AddUint64("cksum", self.checksum);
        encoder.AddUint64("size", self.bytes);
        encoder.AddUint64("kvs", self.kvs);
        Ok(())
    }
    /// 序列化为 JSON 字节（与 String 同形）。
    pub fn MarshalJSON(&self) -> Vec<u8> {
        self.String().into_bytes()
    }
    /// 紧凑 JSON 字符串表示。
    pub fn String(&self) -> String {
        format!(
            "{{\"checksum\":{},\"size\":{},\"kvs\":{}}}",
            self.checksum, self.bytes, self.kvs
        )
    }
}

impl fmt::Display for KVChecksum {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.String())
    }
}

/// 结构化日志编码器接口（对齐 Go zap 风格字段写入）。
pub trait LogEncoder {
    /// 编码嵌套对象时可能返回的错误。
    type Error;

    /// 写入 u64 字段。
    fn AddUint64(&mut self, key: &str, value: u64);
    /// 写入嵌套的 KVChecksum 对象。
    fn AddObject(&mut self, key: &str, value: &KVChecksum) -> Result<(), Self::Error>;
}

/// 数据行 KV 所在分组 ID（索引组用正 index_id）。
pub const DataKVGroupID: i64 = -1;

/// 按组（数据 / 各索引）维护的校验和集合。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KVGroupChecksum {
    /// group id → 校验和。
    groups: HashMap<i64, KVChecksum>,
    /// 创建新组时复用的 keyspace。
    keyspace: Vec<u8>,
}

/// 带 keyspace 创建，并预置数据组。
pub fn NewKVGroupChecksumWithKeyspace(keyspace: &[u8]) -> KVGroupChecksum {
    let mut groups = HashMap::new();
    groups.insert(DataKVGroupID, NewKVChecksumWithKeyspace(keyspace));
    KVGroupChecksum {
        groups,
        keyspace: keyspace.to_vec(),
    }
}
/// 供 Add 合并用的空 keyspace 分组校验和。
pub fn NewKVGroupChecksumForAdd() -> KVGroupChecksum {
    NewKVGroupChecksumWithKeyspace(&[])
}

impl KVGroupChecksum {
    /// 更新数据行组。
    pub fn UpdateOneDataKV(&mut self, pair: &KvPair) {
        self.groups.get_mut(&DataKVGroupID).unwrap().UpdateOne(pair);
    }
    /// 更新指定索引组（不存在则懒创建）。
    pub fn UpdateOneIndexKV(&mut self, index_id: i64, pair: &KvPair) {
        let keyspace = self.keyspace.clone();
        self.groups
            .entry(index_id)
            .or_insert_with(|| NewKVChecksumWithKeyspace(&keyspace))
            .UpdateOne(pair);
    }
    /// 按组合并另一分组校验和。
    pub fn Add(&mut self, other: &KVGroupChecksum) {
        for (id, checksum) in &other.groups {
            self.getOrCreateOneGroup(*id).Add(checksum);
        }
    }
    /// 获取或创建指定分组。
    fn getOrCreateOneGroup(&mut self, id: i64) -> &mut KVChecksum {
        let keyspace = self.keyspace.clone();
        self.groups
            .entry(id)
            .or_insert_with(|| NewKVChecksumWithKeyspace(&keyspace))
    }
    /// 用原始统计字段累加到指定组。
    pub fn AddRawGroup(&mut self, id: i64, bytes: u64, kvs: u64, checksum: u64) {
        self.getOrCreateOneGroup(id)
            .Add(&MakeKVChecksum(bytes, kvs, checksum));
    }
    /// 分别返回数据组与全部索引组的字节合计。
    pub fn DataAndIndexSumSize(&self) -> (u64, u64) {
        self.groups
            .iter()
            .fold((0, 0), |(data, index), (id, checksum)| {
                if *id == DataKVGroupID {
                    (checksum.SumSize(), index)
                } else {
                    (data, index.wrapping_add(checksum.SumSize()))
                }
            })
    }
    /// 分别返回数据组与全部索引组的 KV 条数合计。
    pub fn DataAndIndexSumKVS(&self) -> (u64, u64) {
        self.groups
            .iter()
            .fold((0, 0), |(data, index), (id, checksum)| {
                if *id == DataKVGroupID {
                    (checksum.SumKVS(), index)
                } else {
                    (data, index.wrapping_add(checksum.SumKVS()))
                }
            })
    }
    /// 克隆内部各组校验和。
    pub fn GetInnerChecksums(&self) -> HashMap<i64, KVChecksum> {
        self.groups.clone()
    }
    /// 将所有组合并为单一 KVChecksum。
    pub fn MergedChecksum(&self) -> KVChecksum {
        let mut checksum = NewKVChecksum();
        for group in self.groups.values() {
            checksum.Add(group);
        }
        checksum
    }
    /// 按组写入日志对象。
    pub fn MarshalLogObject<E: LogEncoder + ?Sized>(
        &self,
        encoder: &mut E,
    ) -> Result<(), E::Error> {
        for (id, checksum) in &self.groups {
            encoder.AddObject(&format!("id={id}"), checksum)?;
        }
        Ok(())
    }
}

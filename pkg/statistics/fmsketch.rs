// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// FMSketch：Flajolet–Martin 基数草图，用于近似 NDV（不同值个数）。
//
// 对插入值做 MurmurHash3，仅保留哈希与 `mask` 低位全 0 的子集；
// 集合超限时扩大 `mask` 并压缩，估算 NDV ≈ `(mask + 1) * hashset.len()`。
// 另提供 tipb protobuf 编解码，供分布式 ANALYZE 合并草图。

use std::collections::HashSet;

use protobuf::Message as _;

/// FMSketch 哈希集合的默认上限（编解码恢复时写入 maxSize）。
pub const MaxSketchSize: usize = 10_000;

/// Flajolet–Martin 草图：用稀疏哈希集合近似基数。
#[derive(Clone, Debug)]
pub struct FMSketch {
    /// 当前保留的哈希值集合（均满足 `hash & mask == 0`）。
    hashset: HashSet<u64>,
    /// 低位掩码；越大表示采样越稀疏，NDV 放大倍数越高。
    mask: u64,
    /// 允许保留的最大哈希个数，超过则扩大 mask 并压缩。
    maxSize: usize,
}

/// 创建空的 FMSketch，预分配 `max_size` 容量。
pub fn NewFMSketch(max_size: usize) -> FMSketch {
    FMSketch {
        hashset: HashSet::with_capacity(max_size),
        mask: 0,
        maxSize: max_size,
    }
}

impl FMSketch {
    /// 深拷贝当前草图。
    pub fn Copy(&self) -> FMSketch {
        self.clone()
    }

    /// 估算 NDV：`(mask + 1) * 当前哈希集合大小`。
    pub fn NDV(&self) -> i64 {
        (self.mask + 1) as i64 * self.hashset.len() as i64
    }

    /// 插入一个哈希值；不满足 mask 过滤则丢弃，超限则扩大 mask 并压缩集合。
    fn insertHashValue(&mut self, hash_value: u64) {
        // 仅保留低位与 mask 按位与为 0 的哈希（等价于按 2^k 抽样）。
        if hash_value & self.mask != 0 {
            return;
        }
        self.hashset.insert(hash_value);
        if self.hashset.len() > self.maxSize {
            // mask = mask*2+1：多约束一位，过滤掉约一半元素。
            self.mask = self.mask * 2 + 1;
            self.hashset.retain(|value| value & self.mask == 0);
        }
    }

    /// 将单个 Datum 编码哈希后插入草图。
    pub fn InsertValue(
        &mut self,
        statement_context: &stmtctx::StatementContext,
        value: types::Datum,
    ) -> Result<(), astersql_errors::SharedError> {
        self.insertHashValue(hashDatum(statement_context, value)?);
        Ok(())
    }

    /// 将一行多个 Datum 拼接编码哈希后插入草图（用于复合索引键）。
    pub fn InsertRowValue(
        &mut self,
        statement_context: &stmtctx::StatementContext,
        values: &[types::Datum],
    ) -> Result<(), astersql_errors::SharedError> {
        self.insertHashValue(hashRow(statement_context, values)?);
        Ok(())
    }

    /// 合并另一草图：先对齐较大 mask，再逐个插入对方哈希。
    pub fn MergeFMSketch(&mut self, other: &FMSketch) {
        if self.mask < other.mask {
            self.mask = other.mask;
            self.hashset.retain(|value| value & self.mask == 0);
        }
        for value in &other.hashset {
            self.insertHashValue(*value);
        }
    }

    /// 粗略内存占用：固定头 16 字节 + 每个哈希 8 字节。
    pub fn MemoryUsage(&self) -> i64 {
        16 + 8 * self.hashset.len() as i64
    }

    /// 返回当前 mask。
    pub fn mask(&self) -> u64 {
        self.mask
    }

    /// 返回排序后的哈希值列表（便于稳定序列化/断言）。
    pub fn hash_values(&self) -> Vec<u64> {
        let mut values = self.hashset.iter().copied().collect::<Vec<_>>();
        values.sort_unstable();
        values
    }
}

/// 按会话时区把 Datum 列表编码为字节；出错时交由 StatementContext 处理。
fn encodeValue(
    statement_context: &stmtctx::StatementContext,
    values: Vec<types::Datum>,
) -> Result<Vec<u8>, astersql_errors::SharedError> {
    match codec::EncodeValue(statement_context.TimeZone(), Vec::new(), values) {
        Ok(encoded) => Ok(encoded),
        Err(error) => match statement_context.HandleError(Some(error)) {
            Some(error) => Err(error),
            None => Ok(Vec::new()),
        },
    }
}

/// 对单个 Datum 编码后做 MurmurHash3-64。
fn hashDatum(
    statement_context: &stmtctx::StatementContext,
    value: types::Datum,
) -> Result<u64, astersql_errors::SharedError> {
    Ok(murmur3Sum64(&encodeValue(statement_context, vec![value])?))
}

/// 对一行多个 Datum 分别编码后拼接，再做 MurmurHash3-64。
fn hashRow(
    statement_context: &stmtctx::StatementContext,
    values: &[types::Datum],
) -> Result<u64, astersql_errors::SharedError> {
    let mut encoded = Vec::new();
    for value in values {
        encoded.extend_from_slice(&encodeValue(statement_context, vec![value.clone()])?);
    }
    Ok(murmur3Sum64(&encoded))
}

/// MurmurHash3 128 位实现，返回 `(h1, h2)`；草图只用低 64 位 h1。
pub(crate) fn murmur3Sum128(data: &[u8]) -> (u64, u64) {
    const C1: u64 = 0x87c3_7b91_1142_53d5;
    const C2: u64 = 0x4cf5_ad43_2745_937f;

    let mut h1 = 0_u64;
    let mut h2 = 0_u64;
    let mut chunks = data.chunks_exact(16);
    // 按 16 字节块混合进 h1/h2。
    for chunk in &mut chunks {
        let mut first = [0_u8; 8];
        first.copy_from_slice(&chunk[..8]);
        let mut second = [0_u8; 8];
        second.copy_from_slice(&chunk[8..]);
        let mut k1 = u64::from_le_bytes(first);
        let mut k2 = u64::from_le_bytes(second);

        k1 = k1.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
        h1 ^= k1;
        h1 = h1
            .rotate_left(27)
            .wrapping_add(h2)
            .wrapping_mul(5)
            .wrapping_add(0x52dc_e729);

        k2 = k2.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1);
        h2 ^= k2;
        h2 = h2
            .rotate_left(31)
            .wrapping_add(h1)
            .wrapping_mul(5)
            .wrapping_add(0x3849_5ab5);
    }

    // 处理不足 16 字节的尾部。
    let tail = chunks.remainder();
    let mut k1 = 0_u64;
    let mut k2 = 0_u64;
    for (index, byte) in tail.iter().copied().enumerate() {
        if index < 8 {
            k1 |= (byte as u64) << (index * 8);
        } else {
            k2 |= (byte as u64) << ((index - 8) * 8);
        }
    }
    if tail.len() > 8 {
        k2 = k2.wrapping_mul(C2).rotate_left(33).wrapping_mul(C1);
        h2 ^= k2;
    }
    if !tail.is_empty() {
        k1 = k1.wrapping_mul(C1).rotate_left(31).wrapping_mul(C2);
        h1 ^= k1;
    }

    let length = data.len() as u64;
    h1 ^= length;
    h2 ^= length;
    h1 = h1.wrapping_add(h2);
    h2 = h2.wrapping_add(h1);
    h1 = fmix64(h1);
    h2 = fmix64(h2);
    h1 = h1.wrapping_add(h2);
    h2 = h2.wrapping_add(h1);
    (h1, h2)
}

/// MurmurHash3 的 64 位摘要（取 128 位结果的 h1）。
fn murmur3Sum64(data: &[u8]) -> u64 {
    murmur3Sum128(data).0
}

/// MurmurHash3 最终混合函数（fmix64）。
fn fmix64(mut value: u64) -> u64 {
    value ^= value >> 33;
    value = value.wrapping_mul(0xff51_afd7_ed55_8ccd);
    value ^= value >> 33;
    value = value.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    value ^ (value >> 33)
}

/// 将 FMSketch 转为 tipb::FmSketch protobuf 消息。
pub fn FMSketchToProto(sketch: Option<&FMSketch>) -> tipb::FmSketch {
    let mut proto = tipb::FmSketch::new();
    if let Some(sketch) = sketch {
        proto.set_mask(sketch.mask);
        proto.set_hashset(sketch.hash_values());
    }
    proto
}

/// 从 tipb::FmSketch 恢复 FMSketch（maxSize 置 0，Decode 路径会再写入默认上限）。
pub fn FMSketchFromProto(proto: Option<&tipb::FmSketch>) -> Option<FMSketch> {
    let proto = proto?;
    Some(FMSketch {
        hashset: proto.get_hashset().iter().copied().collect(),
        mask: proto.get_mask(),
        maxSize: 0,
    })
}

/// 将草图序列化为 protobuf 字节；`None` 返回空向量。
pub fn EncodeFMSketch(sketch: Option<&FMSketch>) -> Result<Vec<u8>, astersql_errors::SharedError> {
    let Some(sketch) = sketch else {
        return Ok(Vec::new());
    };
    FMSketchToProto(Some(sketch))
        .write_to_bytes()
        .map_err(|error| astersql_errors::New(error.to_string()))
}

/// 从 protobuf 字节反序列化草图，并设置 `maxSize = MaxSketchSize`。
pub fn DecodeFMSketch(
    data: Option<&[u8]>,
) -> Result<Option<FMSketch>, astersql_errors::SharedError> {
    let Some(data) = data else {
        return Ok(None);
    };
    let proto = protobuf::parse_from_bytes::<tipb::FmSketch>(data)
        .map_err(|error| astersql_errors::New(error.to_string()))?;
    let mut sketch = FMSketchFromProto(Some(&proto));
    if let Some(sketch) = sketch.as_mut() {
        sketch.maxSize = MaxSketchSize;
    }
    Ok(sketch)
}

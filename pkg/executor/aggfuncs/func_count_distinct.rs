// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// COUNT(DISTINCT ...) 与 APPROX_COUNT_DISTINCT 实现。
//
// 精确去重用 `HashSet` 记录已见值；多列去重先把行编码为字节再入集。
// 近似去重（APPROX_COUNT_DISTINCT）用有界哈希集合 + skip_degree 抽样，
// 在内存有限时估计基数（cardinality），并支持 serialize / read_and_merge 跨阶段合并。

use crate::aggfuncs::AggError;
use crate::func_sum::Decimal;
use std::collections::HashSet;
use std::hash::Hash;
use std::mem::size_of;

/// 单列精确 COUNT(DISTINCT) 状态：用哈希集合去重。
#[derive(Clone, Debug)]
pub struct CountDistinct<T> {
    pub(crate) values: HashSet<T>,
}

impl<T> Default for CountDistinct<T> {
    fn default() -> Self {
        Self {
            values: HashSet::new(),
        }
    }
}

impl<T: Eq + Hash> CountDistinct<T> {
    /// 清空去重集合。
    pub fn reset(&mut self) {
        self.values.clear();
    }
    /// 当前去重后的基数。
    pub fn count(&self) -> i64 {
        self.values.len() as i64
    }
    /// 插入非 NULL 值；返回因 capacity 增长产生的近似内存增量（字节）。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<T>>) -> i64 {
        let old_capacity = self.values.capacity();
        self.values.extend(values.into_iter().flatten());
        ((self.values.capacity() - old_capacity) * size_of::<T>()) as i64
    }
    /// 合并另一段 DISTINCT 部分结果。
    pub fn merge(&mut self, source: &Self) -> i64
    where
        T: Clone,
    {
        self.update(source.values.iter().cloned().map(Some))
    }
}

/// 整数列 DISTINCT COUNT。
pub type CountDistinctInt = CountDistinct<i64>;
/// 浮点列 DISTINCT COUNT（内部存 `f64::to_bits` 的 u64）。
pub type CountDistinctReal = CountDistinct<u64>;
/// Decimal 列 DISTINCT COUNT。
pub type CountDistinctDecimal = CountDistinct<Vec<u8>>;
/// Duration 列 DISTINCT COUNT（纳秒等整数表示）。
pub type CountDistinctDuration = CountDistinct<i64>;
/// 字符串列 DISTINCT COUNT（已按校对规则编码的字节）。
pub type CountDistinctString = CountDistinct<Vec<u8>>;
/// 部分阶段整数 DISTINCT。
pub type CountPartialWithDistinct4Int = CountDistinctInt;
/// 原始阶段整数 DISTINCT。
pub type CountOriginalWithDistinct4Int = CountDistinctInt;
/// 部分阶段浮点 DISTINCT。
pub type CountPartialWithDistinct4Real = CountDistinctReal;
/// 原始阶段浮点 DISTINCT。
pub type CountOriginalWithDistinct4Real = CountDistinctReal;
/// 部分阶段 Decimal DISTINCT。
pub type CountPartialWithDistinct4Decimal = CountDistinctDecimal;
/// 原始阶段 Decimal DISTINCT。
pub type CountOriginalWithDistinct4Decimal = CountDistinctDecimal;
/// 部分阶段 Duration DISTINCT。
pub type CountPartialWithDistinct4Duration = CountDistinctDuration;
/// 原始阶段 Duration DISTINCT。
pub type CountOriginalWithDistinct4Duration = CountDistinctDuration;
/// 部分阶段字符串 DISTINCT。
pub type CountPartialWithDistinct4String = CountDistinctString;
/// 原始阶段字符串 DISTINCT。
pub type CountOriginalWithDistinct4String = CountDistinctString;

/// 浮点 DISTINCT 更新：先 `to_bits` 再入集，避免 NaN 比较陷阱。
pub fn update_distinct_real(
    state: &mut CountDistinctReal,
    values: impl IntoIterator<Item = Option<f64>>,
) -> i64 {
    let old_capacity = state.values.capacity();
    for value in values.into_iter().flatten() {
        let mut bits = value.to_bits();
        if value == 0.0 {
            // Go map keys compare +0.0 and -0.0 equal.
            bits = 0.0_f64.to_bits();
        } else if value.is_nan() {
            // Conversely, NaN != NaN in Go, so every observed NaN occupies a
            // distinct map entry. Pick an unused NaN payload to retain that
            // behavior while storing hashable bit patterns in Rust.
            while state.values.contains(&bits) {
                let sign = bits & (1_u64 << 63);
                let payload =
                    ((bits & ((1_u64 << 52) - 1)).wrapping_add(1) & ((1_u64 << 52) - 1)).max(1);
                bits = sign | (0x7ff_u64 << 52) | payload;
            }
        }
        state.values.insert(bits);
    }
    ((state.values.capacity() - old_capacity) * size_of::<u64>()) as i64
}

/// 字符串 DISTINCT 更新：经 collator_key（校对键）规范化后再入集。
pub fn update_distinct_string(
    state: &mut CountDistinctString,
    values: impl IntoIterator<Item = Option<String>>,
    collator_key: impl Fn(&str) -> Vec<u8>,
) -> i64 {
    state.update(
        values
            .into_iter()
            .map(|value| value.map(|value| collator_key(&value))),
    )
}

/// 多列 DISTINCT 行中单个单元格的类型化取值。
#[derive(Clone, Debug, PartialEq)]
pub enum DistinctValue {
    Int(i64),
    Real(f64),
    Decimal(Decimal),
    Time {
        year: u16,
        month: u8,
        day: u8,
        hour: u8,
        minute: u8,
        second: u8,
        microsecond: u32,
        kind: u8,
        fsp: u8,
    },
    Duration {
        nanos: i64,
        fsp: i32,
    },
    Json(Vec<u8>),
    VectorFloat32(Vec<f32>),
    String(Vec<u8>),
}

/// 将 `DistinctValue` 追加编码到缓冲区，供多列去重哈希。
pub fn encode_distinct_value(
    destination: &mut Vec<u8>,
    value: &DistinctValue,
) -> Result<(), AggError> {
    match value {
        DistinctValue::Int(value) => destination.extend_from_slice(&value.to_ne_bytes()),
        DistinctValue::Real(value) => destination.extend_from_slice(&value.to_ne_bytes()),
        DistinctValue::Decimal(value) => {
            destination.extend_from_slice(&value.coefficient().to_ne_bytes());
            destination.extend_from_slice(&value.scale().to_ne_bytes());
        }
        DistinctValue::Time {
            year,
            month,
            day,
            hour,
            minute,
            second,
            microsecond,
            kind,
            fsp,
        } => {
            // 时间字段按固定布局编码，对齐 Go WriteTime 类序列化。
            destination.extend_from_slice(&year.to_be_bytes());
            destination.extend_from_slice(&[*month, *day, *hour, *minute, *second, 0]);
            destination.extend_from_slice(&microsecond.to_be_bytes());
            destination.extend_from_slice(&[*kind, *fsp, 0, 0]);
        }
        DistinctValue::Duration { nanos, fsp } => {
            destination.extend_from_slice(&nanos.to_ne_bytes());
            destination.extend_from_slice(&(*fsp as isize).to_ne_bytes());
        }
        DistinctValue::Json(value) | DistinctValue::String(value) => {
            // 长度前缀 + 原始字节，避免不同长度串碰撞。
            destination.extend_from_slice(&(value.len() as u64).to_be_bytes());
            destination.extend_from_slice(value);
        }
        DistinctValue::VectorFloat32(value) => {
            destination.extend_from_slice(&(value.len() as u64).to_be_bytes());
            for item in value {
                destination.extend_from_slice(&item.to_ne_bytes());
            }
        }
    }
    Ok(())
}

/// 多列 COUNT(DISTINCT col1, col2, ...)：整行编码后去重。
#[derive(Clone, Debug, Default)]
pub struct CountDistinctMulti {
    pub(crate) encoded_rows: HashSet<Vec<u8>>,
}

impl CountDistinctMulti {
    /// 清空已编码行集合。
    pub fn reset(&mut self) {
        self.encoded_rows.clear();
    }
    /// 去重后的行数。
    pub fn count(&self) -> i64 {
        self.encoded_rows.len() as i64
    }
    /// 更新多列行；任一分量为 NULL 则整行跳过（SQL COUNT(DISTINCT) 语义）。
    pub fn update(
        &mut self,
        rows: impl IntoIterator<Item = Vec<Option<DistinctValue>>>,
    ) -> Result<i64, AggError> {
        let old_capacity = self.encoded_rows.capacity();
        let mut owned_bytes = 0_i64;
        for row in rows {
            // 任一列 NULL => 整行不计入 DISTINCT。
            if row.iter().any(Option::is_none) {
                continue;
            }
            let mut encoded = Vec::new();
            for value in row.into_iter().flatten() {
                encode_distinct_value(&mut encoded, &value)?;
            }
            if self.encoded_rows.insert(encoded.clone()) {
                owned_bytes += encoded.len() as i64;
            }
        }
        Ok(owned_bytes
            + ((self.encoded_rows.capacity() - old_capacity) * size_of::<Vec<u8>>()) as i64)
    }
    /// 合并另一段多列 DISTINCT 部分结果，返回内存增量。
    pub fn merge(&mut self, source: &Self) -> i64 {
        let old_capacity = self.encoded_rows.capacity();
        let mut owned_bytes = 0_i64;
        for value in &source.encoded_rows {
            if self.encoded_rows.insert(value.clone()) {
                owned_bytes += value.len() as i64;
            }
        }
        owned_bytes + ((self.encoded_rows.capacity() - old_capacity) * size_of::<Vec<u8>>()) as i64
    }
}

/// 部分阶段多列 DISTINCT。
pub type CountPartialWithDistinct = CountDistinctMulti;
/// 原始阶段多列 DISTINCT。
pub type CountOriginalWithDistinct = CountDistinctMulti;

/// 近似哈希集合最大 size_degree（2^16 量级桶上限相关）。
const UNIQUES_HASH_MAX_SIZE_DEGREE: u8 = 17;
/// 近似集合在提升 skip 前允许的最大元素数。
const UNIQUES_HASH_MAX_SIZE: u32 = 1 << (UNIQUES_HASH_MAX_SIZE_DEGREE - 1);
/// 初始 size_degree（桶数为 2^4）。
const UNIQUES_HASH_SET_INITIAL_SIZE_DEGREE: u8 = 4;
/// 从 32 位哈希中取桶下标时右移的位数。
const UNIQUES_HASH_BITS_FOR_SKIP: u32 = 32 - UNIQUES_HASH_MAX_SIZE_DEGREE as u32;

/// APPROX_COUNT_DISTINCT 状态：有界开放寻址哈希表 + skip 抽样。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApproxCountDistinct {
    size: u32,
    size_degree: u8,
    skip_degree: u8,
    has_zero: bool,
    buffer: Vec<u32>,
}

impl Default for ApproxCountDistinct {
    fn default() -> Self {
        let mut value = Self {
            size: 0,
            size_degree: 0,
            skip_degree: 0,
            has_zero: false,
            buffer: Vec::new(),
        };
        value.reset();
        value
    }
}

impl ApproxCountDistinct {
    /// 分配初始桶并清空。
    pub fn reset(&mut self) {
        self.allocate(UNIQUES_HASH_SET_INITIAL_SIZE_DEGREE);
    }
    /// buffer 占用的内存字节数。
    pub fn memory_usage(&self) -> i64 {
        (self.buffer.len() * size_of::<u32>()) as i64
    }
    /// 插入 64 位哈希（取低 32 位）。
    pub fn insert_hash64(&mut self, hash: u64) {
        self.insert_hash(hash as u32);
    }
    /// 对字节序列做 FNV-1a 风格 hash64 后插入。
    pub fn insert_bytes(&mut self, bytes: &[u8]) {
        self.insert_hash64(hash64(bytes));
    }
    /// 估计基数；skip_degree>0 时用哈希空间对数修正。
    pub fn estimate(&self) -> u64 {
        if self.skip_degree == 0 {
            return self.size as u64;
        }
        // 将抽样集合放大到全哈希空间，再做 ln 修正（类似 HyperLogLog 思想的简化版）。
        let mut result = self.size as u64 * (1_u64 << self.skip_degree);
        result += int_hash64(self.size as u64) & ((1_u64 << self.skip_degree) - 1);
        let buckets = 1_u64 << 32;
        (buckets as f64 * ((buckets as f64).ln() - ((buckets - result) as f64).ln())).round() as u64
    }
    /// 序列化：skip_degree + uvarint(size) + 非零桶哈希（及可选的零哈希标记）。
    pub fn serialize(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(1 + 10 + self.size as usize * 4);
        result.push(self.skip_degree);
        encode_uvarint(self.size as u64, &mut result);
        if self.has_zero {
            result.extend_from_slice(&0_u32.to_le_bytes());
        }
        for value in &self.buffer {
            if *value != 0 {
                result.extend_from_slice(&value.to_le_bytes());
            }
        }
        result
    }
    /// 反序列化并合并另一段近似状态（partial merge）。
    pub fn read_and_merge(&mut self, bytes: &[u8]) -> Result<(), AggError> {
        let Some((&rhs_skip_degree, mut rest)) = bytes.split_first() else {
            return Err(AggError(
                "empty approximate COUNT(DISTINCT) state".to_owned(),
            ));
        };
        // 对方 skip 更高时先提升本地 skip 并 rehash，保证可比。
        if rhs_skip_degree > self.skip_degree {
            self.skip_degree = rhs_skip_degree;
            self.rehash();
        }
        let (rhs_size, used) = decode_uvarint(rest)?;
        rest = &rest[used..];
        if rhs_size > UNIQUES_HASH_MAX_SIZE as u64 {
            return Err(AggError(
                "Cannot read partialResult4ApproxCountDistinct: too large size degree".to_owned(),
            ));
        }
        if rest.len() != rhs_size as usize * 4 {
            return Err(AggError(
                "invalid approximate COUNT(DISTINCT) state length".to_owned(),
            ));
        }
        if self.buffer_size() < rhs_size as u32 {
            let degree = UNIQUES_HASH_SET_INITIAL_SIZE_DEGREE
                .max(((rhs_size.saturating_sub(1) as f64).log2() as u8) + 2);
            self.resize(Some(degree));
        }
        for value in rest.chunks_exact(4) {
            self.insert_hash(u32::from_le_bytes(value.try_into().unwrap()));
        }
        Ok(())
    }
    /// 合并内存中的另一近似状态。
    pub fn merge(&mut self, source: &Self) {
        if source.skip_degree > self.skip_degree {
            self.skip_degree = source.skip_degree;
            self.rehash();
        }
        if !self.has_zero && source.has_zero {
            self.has_zero = true;
            self.size += 1;
            self.shrink_if_needed();
        }
        for value in &source.buffer {
            if *value != 0 && self.good(*value) {
                self.insert_impl(*value);
                self.shrink_if_needed();
            }
        }
    }
    /// 按 degree 分配全零桶。
    fn allocate(&mut self, degree: u8) {
        self.size = 0;
        self.skip_degree = 0;
        self.has_zero = false;
        self.buffer = vec![0; 1_usize << degree];
        self.size_degree = degree;
    }
    fn buffer_size(&self) -> u32 {
        1_u32 << self.size_degree
    }
    fn mask(&self) -> u32 {
        self.buffer_size() - 1
    }
    /// 由哈希高位映射到桶下标。
    fn place(&self, value: u32) -> u32 {
        (value >> UNIQUES_HASH_BITS_FOR_SKIP) & self.mask()
    }
    /// 当前 skip_degree 下该哈希是否仍被保留（低位为 0）。
    fn good(&self, hash: u32) -> bool {
        hash == ((hash >> self.skip_degree) << self.skip_degree)
    }
    fn insert_hash(&mut self, hash: u32) {
        if self.good(hash) {
            self.insert_impl(hash);
            self.shrink_if_needed();
        }
    }
    /// 开放寻址插入；0 哈希用 has_zero 单独标记。
    fn insert_impl(&mut self, value: u32) {
        if value == 0 {
            if !self.has_zero {
                self.size += 1;
            }
            self.has_zero = true;
            return;
        }
        let mut place = self.place(value);
        while self.buffer[place as usize] != 0 && self.buffer[place as usize] != value {
            place = (place + 1) & self.mask();
        }
        if self.buffer[place as usize] == value {
            return;
        }
        self.buffer[place as usize] = value;
        self.size += 1;
    }
    /// 超过装填因子时扩容，或超过上限时提高 skip_degree 抽样。
    fn shrink_if_needed(&mut self) {
        if self.size <= self.max_fill() {
            return;
        }
        if self.size > UNIQUES_HASH_MAX_SIZE {
            while self.size > UNIQUES_HASH_MAX_SIZE {
                self.skip_degree += 1;
                self.rehash();
            }
        } else {
            self.resize(None);
        }
    }
    fn max_fill(&self) -> u32 {
        1_u32 << (self.size_degree - 1)
    }
    fn resize(&mut self, degree: Option<u8>) {
        let old = std::mem::replace(
            &mut self.buffer,
            vec![0; 1_usize << degree.unwrap_or(self.size_degree + 1)],
        );
        self.size_degree = degree.unwrap_or(self.size_degree + 1);
        for value in old {
            if value != 0 {
                self.reinsert(value);
            }
        }
    }
    /// 提升 skip 后剔除不合格哈希，并修正桶位置。
    fn rehash(&mut self) {
        for index in 0..self.buffer.len() {
            if self.buffer[index] != 0 && !self.good(self.buffer[index]) {
                self.buffer[index] = 0;
                self.size -= 1;
            }
        }
        for index in 0..self.buffer.len() {
            let value = self.buffer[index];
            if value != 0 && index as u32 != self.place(value) {
                self.buffer[index] = 0;
                self.reinsert(value);
            }
        }
    }
    fn reinsert(&mut self, value: u32) {
        let mut place = self.place(value);
        while self.buffer[place as usize] != 0 {
            place = (place + 1) & self.mask();
        }
        self.buffer[place as usize] = value;
    }
}

/// 原始阶段近似 DISTINCT。
pub type ApproxCountDistinctOriginal = ApproxCountDistinct;
/// Partial1 阶段近似 DISTINCT。
pub type ApproxCountDistinctPartial1 = ApproxCountDistinct;
/// Partial2 阶段近似 DISTINCT。
pub type ApproxCountDistinctPartial2 = ApproxCountDistinct;
/// Final 阶段近似 DISTINCT。
pub type ApproxCountDistinctFinal = ApproxCountDistinct;

/// 64 位整数混合哈希（Murmur 风格最终化），用于估计修正。
fn int_hash64(mut value: u64) -> u64 {
    value ^= value >> 33;
    value = value.wrapping_mul(0xff51afd7ed558ccd);
    value ^= value >> 33;
    value = value.wrapping_mul(0xc4ceb9fe1a85ec53);
    value ^ (value >> 33)
}

/// FNV-1a 64 位哈希。
fn hash64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// 无符号变长整数编码（protobuf/Go 风格 uvarint）。
fn encode_uvarint(mut value: u64, destination: &mut Vec<u8>) {
    while value >= 0x80 {
        destination.push(value as u8 | 0x80);
        value >>= 7;
    }
    destination.push(value as u8);
}

/// 解码 uvarint；最多 10 字节，非法编码返回错误。
fn decode_uvarint(bytes: &[u8]) -> Result<(u64, usize), AggError> {
    let mut value = 0_u64;
    for (index, byte) in bytes.iter().copied().enumerate().take(10) {
        if index == 9 && byte > 1 {
            break;
        }
        value |= ((byte & 0x7f) as u64) << (index * 7);
        if byte < 0x80 {
            return Ok((value, index + 1));
        }
    }
    Err(AggError("invalid unsigned varint".to_owned()))
}

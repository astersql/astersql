// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 聚合函数 spill（落盘）时的中间结果序列化辅助。
//
// Spill 指内存不足时把哈希聚合的 partial result（部分聚合中间态）写出磁盘。
// `SerializeHelper` 复用内部 buffer，把各类聚合 partial result 编码为字节，
// 供落盘与后续恢复使用。

use crate::aggfuncs::*;
use astersql_util_serialization as serialization;

/// 可复用的序列化缓冲：每次序列化前 reset，写出后返回内部切片视图。
pub struct SerializeHelper {
    buffer: Vec<u8>,
}

/// 默认构造与 `new` 相同，预分配 64 字节缓冲。
impl Default for SerializeHelper {
    fn default() -> Self {
        Self::new()
    }
}

impl SerializeHelper {
    /// 创建序列化辅助器，预分配较小容量以降低首次分配开销。
    pub fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(64),
        }
    }

    /// 清空缓冲以便复用，容量保留。
    fn reset(&mut self) {
        self.buffer.clear();
    }

    /// 向缓冲追加一个布尔值（用于 is_null / got_first_row 等标志位）。
    fn put_bool(&mut self, value: bool) {
        self.buffer = serialization::SerializeBool(value, std::mem::take(&mut self.buffer));
    }

    /// 按 SpillValue 变体分派，调用统一 SerializeInterface 写入缓冲。
    fn serialize_spill_value(&mut self, value: &SpillValue) {
        // 取出当前缓冲交给序列化函数，避免额外拷贝；返回值写回 self.buffer。
        let buffer = std::mem::take(&mut self.buffer);
        self.buffer = match value {
            SpillValue::Bool(value) => {
                serialization::SerializeInterface(value as &dyn std::any::Any, buffer)
            }
            SpillValue::Int64(value) => {
                serialization::SerializeInterface(value as &dyn std::any::Any, buffer)
            }
            SpillValue::Uint64(value) => {
                serialization::SerializeInterface(value as &dyn std::any::Any, buffer)
            }
            SpillValue::Float64(value) => {
                serialization::SerializeInterface(value as &dyn std::any::Any, buffer)
            }
            SpillValue::String(value) => {
                serialization::SerializeInterface(value as &dyn std::any::Any, buffer)
            }
            SpillValue::BinaryJson(value) => {
                serialization::SerializeInterface(value as &dyn std::any::Any, buffer)
            }
            SpillValue::Opaque(value) => {
                serialization::SerializeInterface(value as &dyn std::any::Any, buffer)
            }
            SpillValue::Time(value) => {
                serialization::SerializeInterface(value as &dyn std::any::Any, buffer)
            }
            SpillValue::Duration(value) => {
                serialization::SerializeInterface(value as &dyn std::any::Any, buffer)
            }
        };
    }

    /// 序列化 COUNT 的 partial result（单个 i64 计数）。
    pub fn serialize_count(&mut self, value: PartialResult4Count) -> &[u8] {
        self.reset();
        self.buffer = serialization::SerializeInt64(value, std::mem::take(&mut self.buffer));
        &self.buffer
    }

    /// 序列化 MAX/MIN(整数)：先写 is_null，再写具体数值。
    pub fn serialize_max_min_int(&mut self, value: &PartialResult4MaxMinInt) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer = serialization::SerializeInt64(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 MAX/MIN(无符号整数)。
    pub fn serialize_max_min_uint(&mut self, value: &PartialResult4MaxMinUint) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer = serialization::SerializeUint64(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 MAX/MIN(MyDecimal)。
    pub fn serialize_max_min_decimal(&mut self, value: &PartialResult4MaxMinDecimal) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer =
            serialization::SerializeMyDecimal(&value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 MAX/MIN(float32)。
    pub fn serialize_max_min_float32(&mut self, value: &PartialResult4MaxMinFloat32) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer =
            serialization::SerializeFloat32(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 MAX/MIN(float64)。
    pub fn serialize_max_min_float64(&mut self, value: &PartialResult4MaxMinFloat64) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer =
            serialization::SerializeFloat64(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 MAX/MIN(时间类型 Time)。
    pub fn serialize_max_min_time(&mut self, value: &PartialResult4MaxMinTime) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer = serialization::SerializeTime(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 MAX/MIN(Duration)。
    pub fn serialize_max_min_duration(&mut self, value: &PartialResult4MaxMinDuration) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer =
            serialization::SerializeTypesDuration(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 MAX/MIN(字符串)。
    pub fn serialize_max_min_string(&mut self, value: &PartialResult4MaxMinString) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer =
            serialization::SerializeString(&value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 MAX/MIN(JSON)。
    pub fn serialize_max_min_json(&mut self, value: &PartialResult4MaxMinJson) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer =
            serialization::SerializeBinaryJSON(&value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 MAX/MIN(ENUM)。
    pub fn serialize_max_min_enum(&mut self, value: &PartialResult4MaxMinEnum) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer = serialization::SerializeEnum(&value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 MAX/MIN(SET)。
    pub fn serialize_max_min_set(&mut self, value: &PartialResult4MaxMinSet) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer = serialization::SerializeSet(&value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }

    /// 序列化 AVG(decimal)：sum 与 count。
    pub fn serialize_avg_decimal(&mut self, value: &AvgDecimalPartialResult) -> &[u8] {
        self.reset();
        self.buffer =
            serialization::SerializeMyDecimal(&value.sum, std::mem::take(&mut self.buffer));
        self.buffer = serialization::SerializeInt64(value.count, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 AVG(float64)：sum 与 count。
    pub fn serialize_avg_float64(&mut self, value: &AvgFloat64PartialResult) -> &[u8] {
        self.reset();
        self.buffer = serialization::SerializeFloat64(value.sum, std::mem::take(&mut self.buffer));
        self.buffer = serialization::SerializeInt64(value.count, std::mem::take(&mut self.buffer));
        &self.buffer
    }

    /// 序列化 SUM(decimal)：累计值与非空行数。
    pub fn serialize_sum_decimal(&mut self, value: &PartialResult4SumDecimal) -> &[u8] {
        self.reset();
        self.buffer =
            serialization::SerializeMyDecimal(&value.value, std::mem::take(&mut self.buffer));
        self.buffer = serialization::SerializeInt64(
            value.not_null_row_count,
            std::mem::take(&mut self.buffer),
        );
        &self.buffer
    }
    /// 序列化 SUM(float64)。
    pub fn serialize_sum_float64(&mut self, value: &PartialResult4SumFloat64) -> &[u8] {
        self.reset();
        self.buffer =
            serialization::SerializeFloat64(value.value, std::mem::take(&mut self.buffer));
        self.buffer = serialization::SerializeInt64(
            value.not_null_row_count,
            std::mem::take(&mut self.buffer),
        );
        &self.buffer
    }
    /// 序列化 SUM(int64)。
    pub fn serialize_sum_int64(&mut self, value: &PartialResult4SumInt64) -> &[u8] {
        self.reset();
        self.buffer = serialization::SerializeInt64(value.value, std::mem::take(&mut self.buffer));
        self.buffer = serialization::SerializeInt64(
            value.not_null_row_count,
            std::mem::take(&mut self.buffer),
        );
        &self.buffer
    }
    /// 序列化 SUM(uint64)。
    pub fn serialize_sum_uint64(&mut self, value: &PartialResult4SumUint64) -> &[u8] {
        self.reset();
        self.buffer = serialization::SerializeUint64(value.value, std::mem::take(&mut self.buffer));
        self.buffer = serialization::SerializeInt64(
            value.not_null_row_count,
            std::mem::take(&mut self.buffer),
        );
        &self.buffer
    }

    /// 序列化 GROUP_CONCAT：先写是否有缓冲，再写缓冲内容。
    pub fn serialize_group_concat(&mut self, value: &GroupConcatPartialResult) -> &[u8] {
        self.reset();
        self.put_bool(value.buffer.is_some());
        // 仅在存在拼接缓冲时才写出内容，标志位已表达有无。
        if let Some(buffer) = &value.buffer {
            self.buffer =
                serialization::SerializeBytesBuffer(buffer, std::mem::take(&mut self.buffer));
        }
        &self.buffer
    }

    /// 序列化位运算聚合（BIT_AND/OR/XOR）的 u64 中间态。
    pub fn serialize_bit_func(&mut self, value: PartialResult4BitFunc) -> &[u8] {
        self.reset();
        self.buffer = serialization::SerializeUint64(value, std::mem::take(&mut self.buffer));
        &self.buffer
    }

    /// 序列化 JSON_ARRAYAGG：逐项写出 SpillValue。
    pub fn serialize_json_array(&mut self, value: &JsonArrayPartialResult) -> &[u8] {
        self.reset();
        for entry in &value.entries {
            self.serialize_spill_value(entry);
        }
        &self.buffer
    }

    /// 序列化 JSON_OBJECTAGG：逐对写出 key 与 SpillValue。
    pub fn serialize_json_object(&mut self, value: &JsonObjectPartialResult) -> &[u8] {
        self.reset();
        for (key, entry) in &value.entries {
            self.buffer = serialization::SerializeString(key, std::mem::take(&mut self.buffer));
            self.serialize_spill_value(entry);
        }
        &self.buffer
    }

    /// 写出 FIRST_ROW 公共状态：is_null 与是否已取到第一行。
    fn serialize_first_row_state(&mut self, value: &FirstRowState) {
        self.reset();
        self.put_bool(value.is_null);
        self.put_bool(value.got_first_row);
    }
    /// 序列化 FIRST_ROW(整数)。
    pub fn serialize_first_row_int(&mut self, value: &PartialResult4FirstRowInt) -> &[u8] {
        self.serialize_first_row_state(&value.state);
        self.buffer = serialization::SerializeInt64(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 FIRST_ROW(float32)。
    pub fn serialize_first_row_float32(&mut self, value: &PartialResult4FirstRowFloat32) -> &[u8] {
        self.serialize_first_row_state(&value.state);
        self.buffer =
            serialization::SerializeFloat32(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 FIRST_ROW(float64)。
    pub fn serialize_first_row_float64(&mut self, value: &PartialResult4FirstRowFloat64) -> &[u8] {
        self.serialize_first_row_state(&value.state);
        self.buffer =
            serialization::SerializeFloat64(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 FIRST_ROW(decimal)。
    pub fn serialize_first_row_decimal(&mut self, value: &PartialResult4FirstRowDecimal) -> &[u8] {
        self.serialize_first_row_state(&value.state);
        self.buffer =
            serialization::SerializeMyDecimal(&value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 FIRST_ROW(字符串)。
    pub fn serialize_first_row_string(&mut self, value: &PartialResult4FirstRowString) -> &[u8] {
        self.serialize_first_row_state(&value.state);
        self.buffer =
            serialization::SerializeString(&value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 FIRST_ROW(时间)。
    pub fn serialize_first_row_time(&mut self, value: &PartialResult4FirstRowTime) -> &[u8] {
        self.serialize_first_row_state(&value.state);
        self.buffer = serialization::SerializeTime(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 FIRST_ROW(Duration)。
    pub fn serialize_first_row_duration(
        &mut self,
        value: &PartialResult4FirstRowDuration,
    ) -> &[u8] {
        self.serialize_first_row_state(&value.state);
        self.buffer =
            serialization::SerializeTypesDuration(value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 FIRST_ROW(JSON)。
    pub fn serialize_first_row_json(&mut self, value: &PartialResult4FirstRowJson) -> &[u8] {
        self.serialize_first_row_state(&value.state);
        self.buffer =
            serialization::SerializeBinaryJSON(&value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 FIRST_ROW(ENUM)。
    pub fn serialize_first_row_enum(&mut self, value: &PartialResult4FirstRowEnum) -> &[u8] {
        self.serialize_first_row_state(&value.state);
        self.buffer = serialization::SerializeEnum(&value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
    /// 序列化 FIRST_ROW(SET)。
    pub fn serialize_first_row_set(&mut self, value: &PartialResult4FirstRowSet) -> &[u8] {
        self.serialize_first_row_state(&value.state);
        self.buffer = serialization::SerializeSet(&value.value, std::mem::take(&mut self.buffer));
        &self.buffer
    }
}

impl SerializeHelper {
    pub fn serialize_count_extrema<T: crate::func_max_min_count::CountValue>(
        &mut self,
        value: &crate::func_max_min_count::CountPartial<T>,
    ) -> &[u8] {
        self.reset();
        self.put_bool(value.is_null);
        self.buffer = serialization::SerializeInt64(value.count, std::mem::take(&mut self.buffer));
        self.buffer = value.value.write(std::mem::take(&mut self.buffer));
        &self.buffer
    }
}

impl SerializeHelper {
    pub fn serialize_state<T: SpillState>(&mut self, value: &T) -> &[u8] {
        self.reset();
        self.buffer = value.write_spill(std::mem::take(&mut self.buffer));
        &self.buffer
    }
}

/// Element codecs share the existing Go binary primitives; collection encoders
/// write a count first, except APPROX_COUNT_DISTINCT's own self-contained format.
pub trait SpillElement: Clone + Send + 'static {
    fn write_element(&self, buffer: Vec<u8>) -> Vec<u8>;
    fn read_element(input: &mut serialization::PosAndBuf) -> Self;
    fn heap_bytes(&self) -> i64 {
        0
    }
}
macro_rules! scalar_element {
    ($ty:ty, $write:ident, $read:ident) => {
        impl SpillElement for $ty {
            fn write_element(&self, buffer: Vec<u8>) -> Vec<u8> {
                serialization::$write(*self, buffer)
            }
            fn read_element(input: &mut serialization::PosAndBuf) -> Self {
                serialization::$read(input)
            }
        }
    };
}
scalar_element!(i64, SerializeInt64, DeserializeInt64);
scalar_element!(f64, SerializeFloat64, DeserializeFloat64);
impl SpillElement for u64 {
    // COUNT DISTINCT REAL stores float keys as hashable bit patterns.
    fn write_element(&self, buffer: Vec<u8>) -> Vec<u8> {
        serialization::SerializeFloat64(f64::from_bits(*self), buffer)
    }
    fn read_element(input: &mut serialization::PosAndBuf) -> Self {
        serialization::DeserializeFloat64(input).to_bits()
    }
}
impl SpillElement for Vec<u8> {
    fn write_element(&self, buffer: Vec<u8>) -> Vec<u8> {
        let mut buffer = serialization::SerializeInt(self.len() as isize, buffer);
        buffer.extend(self);
        buffer
    }
    fn read_element(input: &mut serialization::PosAndBuf) -> Self {
        let length = serialization::DeserializeInt(input) as usize;
        let start = input.Pos as usize;
        let value = input.Buf[start..start + length].to_vec();
        input.Pos += length as i64;
        value
    }
    fn heap_bytes(&self) -> i64 {
        self.len() as i64
    }
}
impl SpillElement for crate::func_sum::Decimal {
    fn write_element(&self, buffer: Vec<u8>) -> Vec<u8> {
        let negative = self.coefficient() < 0;
        let mut digits = self.coefficient().unsigned_abs().to_string();
        let scale = self.scale() as usize;
        if scale > 0 {
            if digits.len() <= scale {
                digits = format!("{}{}", "0".repeat(scale + 1 - digits.len()), digits);
            }
            digits.insert(digits.len() - scale, '.');
        }
        if negative {
            digits.insert(0, '-');
        }
        let mut decimal = serialization::types::MyDecimal::default();
        decimal
            .FromString(digits.as_bytes())
            .expect("valid accumulator decimal");
        serialization::SerializeMyDecimal(&decimal, buffer)
    }
    fn read_element(input: &mut serialization::PosAndBuf) -> Self {
        let text = serialization::DeserializeMyDecimal(input).String();
        let scale = text
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len() as u32);
        Self::new(
            text.replace('.', "")
                .parse()
                .expect("decimal coefficient fits accumulator"),
            scale,
        )
    }
}
impl SpillElement for crate::func_max_min::TimeValue {
    fn write_element(&self, buffer: Vec<u8>) -> Vec<u8> {
        let buffer = serialization::SerializeUint64(self.packed, buffer);
        let buffer = serialization::SerializeUint8(self.kind, buffer);
        serialization::SerializeInt32(self.fsp, buffer)
    }
    fn read_element(input: &mut serialization::PosAndBuf) -> Self {
        let packed = serialization::DeserializeUint64(input);
        Self {
            packed,
            kind: serialization::DeserializeUint8(input),
            fsp: serialization::DeserializeInt32(input),
        }
    }
}
impl SpillElement for crate::func_max_min::DurationValue {
    fn write_element(&self, buffer: Vec<u8>) -> Vec<u8> {
        let buffer = serialization::SerializeInt64(self.nanos, buffer);
        serialization::SerializeInt(self.fsp as isize, buffer)
    }
    fn read_element(input: &mut serialization::PosAndBuf) -> Self {
        Self {
            nanos: serialization::DeserializeInt64(input),
            fsp: serialization::DeserializeInt(input) as i32,
        }
    }
}
impl SpillElement for crate::func_max_min::VectorFloat32 {
    fn write_element(&self, mut buffer: Vec<u8>) -> Vec<u8> {
        // FIRST_ROW uses Vector.SerializeTo directly, with no outer byte length.
        buffer.extend((self.0.len() as u32).to_le_bytes());
        for value in &self.0 {
            buffer.extend(value.to_le_bytes());
        }
        buffer
    }
    fn read_element(input: &mut serialization::PosAndBuf) -> Self {
        let start = input.Pos as usize;
        let count = u32::from_le_bytes(input.Buf[start..start + 4].try_into().unwrap()) as usize;
        input.Pos += 4;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            let start = input.Pos as usize;
            values.push(f32::from_le_bytes(
                input.Buf[start..start + 4].try_into().unwrap(),
            ));
            input.Pos += 4;
        }
        Self(values)
    }
    fn heap_bytes(&self) -> i64 {
        (self.0.len() * 4 + 4) as i64
    }
}
fn write_elements<'a, T: SpillElement + 'a>(
    values: impl ExactSizeIterator<Item = &'a T>,
    buffer: Vec<u8>,
) -> Vec<u8> {
    let mut buffer = serialization::SerializeInt(values.len() as isize, buffer);
    for value in values {
        buffer = value.write_element(buffer);
    }
    buffer
}
fn read_elements<T: SpillElement>(input: &mut serialization::PosAndBuf) -> Vec<T> {
    let count = serialization::DeserializeInt(input);
    assert!(count >= 0, "negative spill collection size");
    (0..count).map(|_| T::read_element(input)).collect()
}
impl<T: SpillElement + Eq + std::hash::Hash> SpillState
    for crate::func_count_distinct::CountDistinct<T>
{
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        write_elements(self.values.iter(), buffer)
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.values = std::collections::HashSet::new();
        let values = read_elements::<T>(input);
        let heap: i64 = values.iter().map(SpillElement::heap_bytes).sum();
        self.update(values.into_iter().map(Some)) + heap
    }
}
impl SpillState for crate::func_count_distinct::CountDistinctMulti {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        write_elements(self.encoded_rows.iter(), buffer)
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.encoded_rows = read_elements::<Vec<u8>>(input).into_iter().collect();
        (self.encoded_rows.capacity() * std::mem::size_of::<Vec<u8>>()) as i64
            + self
                .encoded_rows
                .iter()
                .map(|v| v.len() as i64)
                .sum::<i64>()
    }
}
impl SpillState for crate::func_count_distinct::ApproxCountDistinct {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, _: Vec<u8>) -> Vec<u8> {
        self.serialize()
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.reset();
        let old = self.memory_usage();
        self.read_and_merge(&input.Buf)
            .unwrap_or_else(|e| panic!("{e}"));
        input.Pos = input.Buf.len() as i64;
        self.memory_usage() - old
    }
}
impl SpillState for crate::func_sum::DistinctFloatSum {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        write_elements(self.values.iter(), buffer)
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.values = Vec::new();
        self.update(read_elements::<f64>(input).into_iter().map(Some))
    }
}
impl SpillState for crate::func_sum::DistinctDecimalSum {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        let mut buffer = serialization::SerializeInt(self.values.len() as isize, buffer);
        for (key, value) in self.keys.iter().zip(&self.values) {
            buffer = key.write_element(buffer);
            buffer = value.write_element(buffer);
        }
        buffer
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.values = Vec::new();
        self.keys = Vec::new();
        let count = serialization::DeserializeInt(input);
        let mut heap = 0;
        for _ in 0..count {
            let key = Vec::<u8>::read_element(input);
            let value = crate::func_sum::Decimal::read_element(input);
            heap += std::mem::size_of::<serialization::types::MyDecimal>() as i64;
            heap += self.insert_keyed(key, value);
        }
        heap
    }
}
macro_rules! delegate_sum_spill {
    ($ty:ty) => {
        impl SpillState for $ty {
            fn copy_partial(&self) -> Self {
                self.clone()
            }
            fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
                self.sum.write_spill(buffer)
            }
            fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
                self.sum.read_spill(input)
            }
        }
    };
}
delegate_sum_spill!(crate::func_avg::DistinctFloatAvg);
delegate_sum_spill!(crate::func_avg::DistinctDecimalAvg);
impl SpillState for crate::func_sum_int::SumDistinctInt64 {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        write_elements(self.values.iter(), buffer)
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.reset();
        self.update(read_elements::<i64>(input).into_iter().map(Some))
    }
}
impl SpillState for crate::func_sum_int::SumDistinctUint64 {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        write_elements(self.bit_patterns.iter(), buffer)
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.reset();
        self.update(
            read_elements::<i64>(input)
                .into_iter()
                .map(|v| Some(v as u64)),
        )
    }
}
impl SpillState for crate::func_varpop::VarianceState {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        let buffer = serialization::SerializeInt64(self.count, buffer);
        let buffer = serialization::SerializeFloat64(self.sum, buffer);
        serialization::SerializeFloat64(self.variance, buffer)
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.count = serialization::DeserializeInt64(input);
        self.sum = serialization::DeserializeFloat64(input);
        self.variance = serialization::DeserializeFloat64(input);
        0
    }
}
impl SpillState for crate::func_varpop::DistinctVariance {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        write_elements(self.values.values(), buffer)
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.values = std::collections::HashMap::new();
        self.next_nan_payload = 1;
        self.update(read_elements::<f64>(input).into_iter().map(Some));
        (self.values.capacity() * std::mem::size_of::<(u64, f64)>()) as i64
    }
}
impl<T: SpillElement> SpillState for crate::func_percentile::Percentile<T> {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn fixed_spill_memory(&self) -> i64 {
        std::mem::size_of::<Vec<T>>() as i64
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        write_elements(self.data.iter(), buffer)
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.data = read_elements(input);
        // Go intentionally charges eight bytes for Time and Duration samples.
        let element_size = if std::any::TypeId::of::<T>()
            == std::any::TypeId::of::<crate::func_max_min::TimeValue>()
            || std::any::TypeId::of::<T>()
                == std::any::TypeId::of::<crate::func_max_min::DurationValue>()
        {
            8
        } else if std::any::TypeId::of::<T>() == std::any::TypeId::of::<crate::func_sum::Decimal>()
        {
            std::mem::size_of::<serialization::types::MyDecimal>()
        } else {
            std::mem::size_of::<T>()
        };
        (self.data.len() * element_size) as i64
    }
}
impl<T: SpillElement + Default> SpillState for crate::func_first_row::FirstRow<T> {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        let buffer = serialization::SerializeBool(self.is_null(), buffer);
        let buffer = serialization::SerializeBool(self.got_first_row(), buffer);
        let vector = std::any::TypeId::of::<T>()
            == std::any::TypeId::of::<crate::func_max_min::VectorFloat32>();
        match self.value() {
            Some(value) => value.write_element(buffer),
            None if vector => buffer,
            None => T::default().write_element(buffer),
        }
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        let is_null = serialization::DeserializeBool(input);
        let got_first_row = serialization::DeserializeBool(input);
        let vector = std::any::TypeId::of::<T>()
            == std::any::TypeId::of::<crate::func_max_min::VectorFloat32>();
        let value = if vector && (!got_first_row || is_null) {
            None
        } else {
            Some(T::read_element(input))
        };
        let memory = value.as_ref().map_or(0, SpillElement::heap_bytes);
        self.state = if !got_first_row {
            None
        } else if is_null {
            Some(None)
        } else {
            Some(value)
        };
        memory
    }
}
impl SpillState for crate::func_group_concat::GroupConcat {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        if let Some(values) = &self.distinct {
            let mut buffer = serialization::SerializeInt(values.len() as isize, buffer);
            for (key, value) in values {
                buffer = key.write_element(buffer);
                buffer = value.write_element(buffer);
            }
            buffer
        } else {
            let buffer = serialization::SerializeBool(self.has_value, buffer);
            if self.has_value {
                self.value.write_element(buffer)
            } else {
                buffer
            }
        }
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.reset();
        if self.distinct.is_some() {
            let count = serialization::DeserializeInt(input);
            let mut memory = 0;
            for _ in 0..count {
                let key = Vec::<u8>::read_element(input);
                let value = Vec::<u8>::read_element(input);
                memory += (key.len() + value.len()) as i64;
                self.update_keyed(key, value);
            }
            memory
                + (self.distinct.as_ref().unwrap().capacity() * std::mem::size_of::<Vec<u8>>())
                    as i64
        } else if serialization::DeserializeBool(input) {
            let value = Vec::<u8>::read_element(input);
            let memory = value.len() as i64;
            self.update([Some(value)]);
            memory
        } else {
            0
        }
    }
}
/// Unsupported PERCENTILE inputs have no partial data but still occupy a row.
#[derive(Clone, Default)]
pub struct NullPercentile;
impl SpillState for NullPercentile {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn has_spill_state(&self) -> bool {
        false
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        buffer
    }
    fn read_spill(&mut self, _: &mut serialization::PosAndBuf) -> i64 {
        0
    }
}

// Native SQL value codecs keep full MyDecimal precision and Time's packed
// type/FSP bits; callers need not convert through the older wrapper types.
impl SpillElement for serialization::types::MyDecimal {
    fn write_element(&self, buffer: Vec<u8>) -> Vec<u8> {
        serialization::SerializeMyDecimal(self, buffer)
    }
    fn read_element(input: &mut serialization::PosAndBuf) -> Self {
        serialization::DeserializeMyDecimal(input)
    }
}
impl SpillElement for serialization::types::Time {
    fn write_element(&self, buffer: Vec<u8>) -> Vec<u8> {
        serialization::SerializeTime(*self, buffer)
    }
    fn read_element(input: &mut serialization::PosAndBuf) -> Self {
        serialization::DeserializeTime(input)
    }
}
impl SpillElement for serialization::types::Duration {
    fn write_element(&self, buffer: Vec<u8>) -> Vec<u8> {
        serialization::SerializeTypesDuration(*self, buffer)
    }
    fn read_element(input: &mut serialization::PosAndBuf) -> Self {
        serialization::DeserializeTypesDuration(input)
    }
}
impl<T: SpillElement> SpillState for Vec<T> {
    fn copy_partial(&self) -> Self {
        self.clone()
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        write_elements(self.iter(), buffer)
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        *self = read_elements(input);
        let size = if std::any::TypeId::of::<T>()
            == std::any::TypeId::of::<serialization::types::Time>()
            || std::any::TypeId::of::<T>()
                == std::any::TypeId::of::<serialization::types::Duration>()
        {
            8
        } else {
            std::mem::size_of::<T>()
        };
        (self.len() * size) as i64
    }
}
impl SpillState for FirstRowPartialResult<serialization::types::VectorFloat32> {
    fn copy_partial(&self) -> Self {
        Self {
            state: self.state.clone(),
            value: clone_native_vector(&self.value),
        }
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        let buffer = serialization::SerializeBool(self.state.is_null, buffer);
        let buffer = serialization::SerializeBool(self.state.got_first_row, buffer);
        if self.state.got_first_row && !self.state.is_null {
            self.value.SerializeTo(buffer)
        } else {
            buffer
        }
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.state.is_null = serialization::DeserializeBool(input);
        self.state.got_first_row = serialization::DeserializeBool(input);
        self.value = serialization::types::VectorFloat32::default();
        if self.state.got_first_row && !self.state.is_null {
            let (value, remainder) = serialization::types::ZeroCopyDeserializeVectorFloat32(
                &input.Buf[input.Pos as usize..],
            )
            .unwrap_or_else(|e| panic!("{e}"));
            input.Pos = (input.Buf.len() - remainder.len()) as i64;
            self.value = value;
            self.value.SerializedSize() as i64
        } else {
            0
        }
    }
}
impl SpillState for MaxMinPartialResult<serialization::types::VectorFloat32> {
    fn copy_partial(&self) -> Self {
        Self {
            is_null: self.is_null,
            value: clone_native_vector(&self.value),
        }
    }
    fn write_spill(&self, buffer: Vec<u8>) -> Vec<u8> {
        let buffer = serialization::SerializeBool(self.is_null, buffer);
        serialization::SerializeVectorFloat32(&self.value, buffer)
    }
    fn read_spill(&mut self, input: &mut serialization::PosAndBuf) -> i64 {
        self.is_null = serialization::DeserializeBool(input);
        self.value = serialization::DeserializeVectorFloat32(input);
        self.value.SerializedSize() as i64
    }
}

impl SpillElement for String {
    fn write_element(&self, buffer: Vec<u8>) -> Vec<u8> {
        serialization::SerializeString(self, buffer)
    }
    fn read_element(input: &mut serialization::PosAndBuf) -> Self {
        serialization::DeserializeString(input)
    }
    fn heap_bytes(&self) -> i64 {
        self.len() as i64
    }
}

fn clone_native_vector(
    value: &serialization::types::VectorFloat32,
) -> serialization::types::VectorFloat32 {
    if value.SerializedSize() == 0 {
        return serialization::types::VectorFloat32::default();
    }
    serialization::types::ZeroCopyDeserializeVectorFloat32(value.ZeroCopySerialize())
        .expect("valid stored vector")
        .0
}

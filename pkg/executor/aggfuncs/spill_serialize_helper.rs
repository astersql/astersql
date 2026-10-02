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

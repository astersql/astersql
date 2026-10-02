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

// Spill 反序列化辅助：从落盘 chunk 列还原聚合部分结果。
//
// 对应 Go 的 `spill_deserialize_helper.go`。聚合执行器在内存不足时会把
// partial result（部分结果）序列化进 spill 列；本模块按行推进读指针，
// 用 `serialization` 工具把字节流还原为各聚合函数的部分结果结构。
// 多数 `deserialize_*` 返回 `bool` 表示是否还有下一行；`JSON_OBJECTAGG`
// 额外返回内存增量，供执行器做内存记账。

use crate::aggfuncs::*;
use astersql_util_serialization::{self as serialization, chunk::Column};
use std::collections::hash_map::Entry;
use std::io::Cursor;
use std::mem::size_of;

/// 从 spill 列按行反序列化 partial result 的辅助器。
///
/// 持有列引用、当前读行下标与可复用的 `PosAndBuf` 读缓冲。
pub struct DeserializeHelper<'a> {
    column: &'a Column,
    read_row_index: usize,
    total_row_count: usize,
    position_and_buffer: serialization::PosAndBuf,
}

impl<'a> DeserializeHelper<'a> {
    /// 绑定 spill 列与调用方声明的有效行数。
    pub fn new(column: &'a Column, row_count: usize) -> Self {
        Self {
            column,
            read_row_index: 0,
            total_row_count: row_count,
            position_and_buffer: serialization::PosAndBuf::default(),
        }
    }

    /// 推进到下一行：重置读缓冲后调用闭包反序列化；读完返回 `None`。
    fn next<T>(
        &mut self,
        deserialize: impl FnOnce(&mut serialization::PosAndBuf) -> T,
    ) -> Option<T> {
        if self.read_row_index >= self.total_row_count {
            return None;
        }
        // 定位到当前行在列中的字节区间，再交给具体类型的反序列化闭包。
        self.position_and_buffer
            .Reset(self.column, self.read_row_index);
        let value = deserialize(&mut self.position_and_buffer);
        self.read_row_index += 1;
        Some(value)
    }

    /// 反序列化 COUNT 的 int64 部分结果。
    pub fn deserialize_count(&mut self, destination: &mut PartialResult4Count) -> bool {
        let Some(value) = self.next(serialization::DeserializeInt64) else {
            return false;
        };
        *destination = value;
        true
    }

    /// 反序列化有符号整数 MAX/MIN：先 bool 空值标志，再 int64 值。
    pub fn deserialize_max_min_int(&mut self, destination: &mut PartialResult4MaxMinInt) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeInt64(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }
    /// 反序列化无符号整数 MAX/MIN：bool 空值标志 + uint64 值。
    pub fn deserialize_max_min_uint(&mut self, destination: &mut PartialResult4MaxMinUint) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeUint64(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }
    /// 反序列化十进制 MAX/MIN：bool 空值标志 + MyDecimal。
    pub fn deserialize_max_min_decimal(
        &mut self,
        destination: &mut PartialResult4MaxMinDecimal,
    ) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeMyDecimal(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }
    /// 反序列化 float32 MAX/MIN：bool 空值标志 + f32。
    pub fn deserialize_max_min_float32(
        &mut self,
        destination: &mut PartialResult4MaxMinFloat32,
    ) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeFloat32(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }
    /// 反序列化 float64 MAX/MIN：bool 空值标志 + f64。
    pub fn deserialize_max_min_float64(
        &mut self,
        destination: &mut PartialResult4MaxMinFloat64,
    ) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeFloat64(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }
    /// 反序列化时间类型 MAX/MIN：bool 空值标志 + Time。
    pub fn deserialize_max_min_time(&mut self, destination: &mut PartialResult4MaxMinTime) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeTime(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }
    /// 反序列化 Duration MAX/MIN：bool 空值标志 + Duration。
    pub fn deserialize_max_min_duration(
        &mut self,
        destination: &mut PartialResult4MaxMinDuration,
    ) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeTypesDuration(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }
    /// 反序列化字符串 MAX/MIN：bool 空值标志 + String。
    pub fn deserialize_max_min_string(
        &mut self,
        destination: &mut PartialResult4MaxMinString,
    ) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeString(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }
    /// 反序列化 JSON MAX/MIN：bool 空值标志 + BinaryJSON。
    pub fn deserialize_max_min_json(&mut self, destination: &mut PartialResult4MaxMinJson) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeBinaryJSON(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }
    /// 反序列化 ENUM MAX/MIN：bool 空值标志 + Enum。
    pub fn deserialize_max_min_enum(&mut self, destination: &mut PartialResult4MaxMinEnum) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeEnum(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }
    /// 反序列化 SET MAX/MIN：bool 空值标志 + Set。
    pub fn deserialize_max_min_set(&mut self, destination: &mut PartialResult4MaxMinSet) -> bool {
        let Some((is_null, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeSet(p),
            )
        }) else {
            return false;
        };
        destination.is_null = is_null;
        destination.value = value;
        true
    }

    /// 反序列化 AVG(decimal)：MyDecimal 累加和 + int64 计数。
    pub fn deserialize_avg_decimal(&mut self, destination: &mut AvgDecimalPartialResult) -> bool {
        let Some((sum, count)) = self.next(|p| {
            (
                serialization::DeserializeMyDecimal(p),
                serialization::DeserializeInt64(p),
            )
        }) else {
            return false;
        };
        destination.sum = sum;
        destination.count = count;
        true
    }
    /// 反序列化 AVG(float64)：f64 累加和 + int64 计数。
    pub fn deserialize_avg_float64(&mut self, destination: &mut AvgFloat64PartialResult) -> bool {
        let Some((sum, count)) = self.next(|p| {
            (
                serialization::DeserializeFloat64(p),
                serialization::DeserializeInt64(p),
            )
        }) else {
            return false;
        };
        destination.sum = sum;
        destination.count = count;
        true
    }

    /// 反序列化 SUM(decimal)：十进制和 + 非空行计数。
    pub fn deserialize_sum_decimal(&mut self, destination: &mut PartialResult4SumDecimal) -> bool {
        let Some((value, count)) = self.next(|p| {
            (
                serialization::DeserializeMyDecimal(p),
                serialization::DeserializeInt64(p),
            )
        }) else {
            return false;
        };
        destination.value = value;
        destination.not_null_row_count = count;
        true
    }
    /// 反序列化 SUM(float64)：浮点和 + 非空行计数。
    pub fn deserialize_sum_float64(&mut self, destination: &mut PartialResult4SumFloat64) -> bool {
        let Some((value, count)) = self.next(|p| {
            (
                serialization::DeserializeFloat64(p),
                serialization::DeserializeInt64(p),
            )
        }) else {
            return false;
        };
        destination.value = value;
        destination.not_null_row_count = count;
        true
    }
    /// 反序列化 SUM(int64)：有符号和 + 非空行计数。
    pub fn deserialize_sum_int64(&mut self, destination: &mut PartialResult4SumInt64) -> bool {
        let Some((value, count)) = self.next(|p| {
            (
                serialization::DeserializeInt64(p),
                serialization::DeserializeInt64(p),
            )
        }) else {
            return false;
        };
        destination.value = value;
        destination.not_null_row_count = count;
        true
    }
    /// 反序列化 SUM(uint64)：无符号和 + 非空行计数。
    pub fn deserialize_sum_uint64(&mut self, destination: &mut PartialResult4SumUint64) -> bool {
        let Some((value, count)) = self.next(|p| {
            (
                serialization::DeserializeUint64(p),
                serialization::DeserializeInt64(p),
            )
        }) else {
            return false;
        };
        destination.value = value;
        destination.not_null_row_count = count;
        true
    }

    /// 反序列化 GROUP_CONCAT：可选 bytes buffer；valsBuf 在还原后置空。
    pub fn deserialize_group_concat(&mut self, destination: &mut GroupConcatPartialResult) -> bool {
        // 先读是否存在 buffer：false 表示 Go 侧 nil，true 再反序列化字节缓冲。
        let Some(buffer) = self.next(|p| {
            if serialization::DeserializeBool(p) {
                Some(serialization::DeserializeBytesBuffer(p))
            } else {
                None
            }
        }) else {
            return false;
        };
        // valsBuf 在 spill 恢复后重新分配空 Cursor，真实拼接内容在 buffer 中。
        destination.values_buffer = Cursor::new(Vec::new());
        destination.buffer = buffer;
        true
    }

    /// 反序列化比特聚合（BIT_AND/OR/XOR）的 uint64 部分结果。
    pub fn deserialize_bit_func(&mut self, destination: &mut PartialResult4BitFunc) -> bool {
        let Some(value) = self.next(serialization::DeserializeUint64) else {
            return false;
        };
        *destination = value;
        true
    }

    /// 反序列化 JSON_ARRAYAGG：循环读完缓冲区内全部 SpillValue 条目。
    pub fn deserialize_json_array(&mut self, destination: &mut JsonArrayPartialResult) -> bool {
        // 行缓冲内顺序追加异构 SpillValue，直到读指针到达缓冲末尾。
        let Some(entries) = self.next(|p| {
            let mut entries = Vec::new();
            while p.Pos < p.Buf.len() as i64 {
                entries.push(deserialize_spill_value(p));
            }
            entries
        }) else {
            return false;
        };
        destination.entries.extend(entries);
        true
    }

    /// 反序列化 JSON_OBJECTAGG：重建 key/value 映射并累计堆外内存增量。
    pub fn deserialize_json_object(
        &mut self,
        destination: &mut JsonObjectPartialResult,
    ) -> (bool, i64) {
        let Some(entries) = self.next(|p| {
            let mut entries = Vec::new();
            while p.Pos < p.Buf.len() as i64 {
                entries.push((
                    serialization::DeserializeString(p),
                    deserialize_spill_value(p),
                ));
            }
            entries
        }) else {
            destination.entries.clear();
            return (false, 0);
        };

        // 先清空再重建：只在新 key 上累计字符串与 SpillValue 堆外增量，并计入 map 扩容。
        destination.entries.clear();
        let mut memory_delta = 0;
        for (key, value) in entries {
            let old_capacity = destination.entries.capacity();
            match destination.entries.entry(key) {
                Entry::Vacant(entry) => {
                    memory_delta += entry.key().len() as i64 + value_memory_delta(&value);
                    entry.insert(value);
                    let growth = destination.entries.capacity().saturating_sub(old_capacity);
                    memory_delta += (growth * size_of::<(String, SpillValue)>()) as i64;
                }
                Entry::Occupied(mut entry) => {
                    // 同 key 覆盖写不增加新条目内存记账（与 Go 行为对齐）。
                    entry.insert(value);
                }
            }
        }
        (true, memory_delta)
    }

    /// FIRST_ROW 通用路径：还原 is_null、got_first_row 标志与具体值。
    fn deserialize_first_row<T>(
        &mut self,
        destination: &mut FirstRowPartialResult<T>,
        deserialize_value: impl FnOnce(&mut serialization::PosAndBuf) -> T,
    ) -> bool {
        let Some((is_null, got_first_row, value)) = self.next(|p| {
            (
                serialization::DeserializeBool(p),
                serialization::DeserializeBool(p),
                deserialize_value(p),
            )
        }) else {
            return false;
        };
        destination.state.is_null = is_null;
        destination.state.got_first_row = got_first_row;
        destination.value = value;
        true
    }

    /// 反序列化 FIRST_ROW(int64)。
    pub fn deserialize_first_row_int(
        &mut self,
        destination: &mut PartialResult4FirstRowInt,
    ) -> bool {
        self.deserialize_first_row(destination, serialization::DeserializeInt64)
    }
    /// 反序列化 FIRST_ROW(float32)。
    pub fn deserialize_first_row_float32(
        &mut self,
        destination: &mut PartialResult4FirstRowFloat32,
    ) -> bool {
        self.deserialize_first_row(destination, serialization::DeserializeFloat32)
    }
    /// 反序列化 FIRST_ROW(float64)。
    pub fn deserialize_first_row_float64(
        &mut self,
        destination: &mut PartialResult4FirstRowFloat64,
    ) -> bool {
        self.deserialize_first_row(destination, serialization::DeserializeFloat64)
    }
    /// 反序列化 FIRST_ROW(decimal)。
    pub fn deserialize_first_row_decimal(
        &mut self,
        destination: &mut PartialResult4FirstRowDecimal,
    ) -> bool {
        self.deserialize_first_row(destination, serialization::DeserializeMyDecimal)
    }
    /// 反序列化 FIRST_ROW(string)。
    pub fn deserialize_first_row_string(
        &mut self,
        destination: &mut PartialResult4FirstRowString,
    ) -> bool {
        self.deserialize_first_row(destination, serialization::DeserializeString)
    }
    /// 反序列化 FIRST_ROW(time)。
    pub fn deserialize_first_row_time(
        &mut self,
        destination: &mut PartialResult4FirstRowTime,
    ) -> bool {
        self.deserialize_first_row(destination, serialization::DeserializeTime)
    }
    /// 反序列化 FIRST_ROW(duration)。
    pub fn deserialize_first_row_duration(
        &mut self,
        destination: &mut PartialResult4FirstRowDuration,
    ) -> bool {
        self.deserialize_first_row(destination, serialization::DeserializeTypesDuration)
    }
    /// 反序列化 FIRST_ROW(json)。
    pub fn deserialize_first_row_json(
        &mut self,
        destination: &mut PartialResult4FirstRowJson,
    ) -> bool {
        self.deserialize_first_row(destination, serialization::DeserializeBinaryJSON)
    }
    /// 反序列化 FIRST_ROW(enum)。
    pub fn deserialize_first_row_enum(
        &mut self,
        destination: &mut PartialResult4FirstRowEnum,
    ) -> bool {
        self.deserialize_first_row(destination, serialization::DeserializeEnum)
    }
    /// 反序列化 FIRST_ROW(set)。
    pub fn deserialize_first_row_set(
        &mut self,
        destination: &mut PartialResult4FirstRowSet,
    ) -> bool {
        self.deserialize_first_row(destination, serialization::DeserializeSet)
    }
}

/// 按 interface 标签反序列化 JSON 聚合条目中的异构 `SpillValue`。
fn deserialize_spill_value(position_and_buffer: &mut serialization::PosAndBuf) -> SpillValue {
    match serialization::DeserializeInterface(position_and_buffer) {
        serialization::DeserializedInterface::Bool(value) => SpillValue::Bool(value),
        serialization::DeserializedInterface::Int64(value) => SpillValue::Int64(value),
        serialization::DeserializedInterface::Uint64(value) => SpillValue::Uint64(value),
        serialization::DeserializedInterface::Float64(value) => SpillValue::Float64(value),
        serialization::DeserializedInterface::String(value) => SpillValue::String(value),
        serialization::DeserializedInterface::BinaryJSON(value) => SpillValue::BinaryJson(value),
        serialization::DeserializedInterface::Opaque(value) => SpillValue::Opaque(value),
        serialization::DeserializedInterface::Time(value) => SpillValue::Time(value),
        serialization::DeserializedInterface::Duration(value) => SpillValue::Duration(value),
    }
}

/// 估算单个 `SpillValue` 相对 interface 头的额外堆外字节，用于内存增量记账。
fn value_memory_delta(value: &SpillValue) -> i64 {
    DEF_INTERFACE_SIZE
        + match value {
            SpillValue::Bool(_) => DEF_BOOL_SIZE,
            SpillValue::Int64(_) => DEF_INT64_SIZE,
            SpillValue::Uint64(_) => DEF_UINT64_SIZE,
            SpillValue::Float64(_) => DEF_FLOAT64_SIZE,
            SpillValue::String(value) => value.len() as i64,
            SpillValue::BinaryJson(value) => value.Value.len() as i64 + 1,
            SpillValue::Opaque(value) => value.Buf.len() as i64 + 1,
            SpillValue::Time(_) => DEF_TIME_SIZE,
            SpillValue::Duration(_) => DEF_DURATION_SIZE,
        }
}

impl DeserializeHelper<'_> {
    pub fn deserialize_count_extrema<T: crate::func_max_min_count::CountValue>(
        &mut self,
    ) -> Option<crate::func_max_min_count::CountPartial<T>> {
        self.next(|p| {
            let is_null = serialization::DeserializeBool(p);
            let count = serialization::DeserializeInt64(p);
            let value = T::read(p);
            crate::func_max_min_count::CountPartial {
                value,
                count,
                is_null,
            }
        })
    }
}

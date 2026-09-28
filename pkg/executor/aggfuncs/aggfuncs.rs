// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 聚合函数（aggregate function）公共类型与 trait。
//
// 聚合函数在分组（GROUP BY）或窗口（window）计算中维护部分结果（partial result），
// 再合并（merge）并写出最终值。本文件定义：
// - 各基本类型大小常量（用于内存记账）；
// - `AggFunc` / `SlidingWindowAggFunc` 等核心 trait；
// - COUNT/SUM/AVG/MAX/MIN/GROUP_CONCAT/JSON/FIRST_ROW 等的部分结果结构；
// - spill（溢出到磁盘）相关的序列化值类型。

use crate::spill_deserialize_helper::DeserializeHelper;
use crate::spill_serialize_helper::SerializeHelper;
pub use astersql_expression::Expression;
pub use astersql_expression_exprctx::EvalContext;
pub use astersql_types::field::FieldType;
use astersql_util_serialization::{
    chunk::{Chunk, Row},
    types,
};
use std::any::Any;
use std::collections::HashMap;
use std::fmt;
use std::io::Cursor;
use std::mem::size_of;
use std::sync::Arc;

/// u32 占用字节数，用于部分结果内存增量估算。
pub const DEF_UINT32_SIZE: i64 = size_of::<u32>() as i64;
/// u64 占用字节数。
pub const DEF_UINT64_SIZE: i64 = size_of::<u64>() as i64;
/// i64 占用字节数。
pub const DEF_INT64_SIZE: i64 = size_of::<i64>() as i64;
/// f64 占用字节数。
pub const DEF_FLOAT64_SIZE: i64 = size_of::<f64>() as i64;
/// Time 类型占用字节数。
pub const DEF_TIME_SIZE: i64 = size_of::<types::Time>() as i64;
/// Row 占用字节数。
pub const DEF_ROW_SIZE: i64 = size_of::<Row>() as i64;
/// bool 占用字节数。
pub const DEF_BOOL_SIZE: i64 = size_of::<bool>() as i64;
// A Go interface occupies two machine words.
/// Go interface 占两个机器字（类型指针+数据指针），Rust 侧按此估算。
pub const DEF_INTERFACE_SIZE: i64 = (size_of::<usize>() * 2) as i64;
/// MyDecimal 占用字节数。
pub const DEF_MY_DECIMAL_SIZE: i64 = size_of::<types::MyDecimal>() as i64;
/// Duration 占用字节数。
pub const DEF_DURATION_SIZE: i64 = size_of::<types::Duration>() as i64;

/// A type-erased aggregate state. Box ownership is the safe Rust equivalent of
/// the Go implementation's `unsafe.Pointer` partial result.
///
/// 类型擦除的聚合部分结果；Box 对应 Go 中 `unsafe.Pointer` 持有的状态。
pub type PartialResult = Box<dyn Any + Send>;
/// Go copies a `[]PartialResult` slice header into every map slot. `Arc` keeps
/// the same shared-backing-storage ownership while the MemAwareMap accounts for
/// the map table itself.
///
/// 分组键到部分结果向量的内存感知映射（MemAwareMap 记账 map 表本身）。
pub type AggPartialResultMapper =
    Box<astersql_util_hack::map_abi::MemAwareMap<String, Arc<Vec<PartialResult>>>>;

/// 创建空的聚合部分结果映射。
pub fn new_agg_partial_result_mapper() -> AggPartialResultMapper {
    new_agg_partial_result_mapper_with_capacity(0)
}

/// 按容量预分配创建聚合部分结果映射。
pub fn new_agg_partial_result_mapper_with_capacity(capacity: usize) -> AggPartialResultMapper {
    astersql_util_hack::map_abi::NewMemAwareMap(capacity)
}

/// 聚合函数执行错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggError(pub String);

impl fmt::Display for AggError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AggError {}

/// 部分结果序列化/反序列化接口（用于 spill 落盘与恢复）。
pub trait Serializer {
    /// 将单个部分结果序列化进 chunk（借助 spill_helper）。
    fn serialize_partial_result(
        &self,
        partial_result: &PartialResult,
        chunk: &mut Chunk,
        spill_helper: &mut SerializeHelper,
    );

    /// 从 chunk 反序列化出部分结果列表及内存增量。
    fn deserialize_partial_result(&self, source: &Chunk) -> (Vec<PartialResult>, i64);
}

/// 聚合函数核心接口：分配/重置/更新/合并部分结果，并写出最终列值。
pub trait AggFunc: Serializer {
    /// 分配新的部分结果，返回 (状态, 初始内存占用)。
    fn alloc_partial_result(&self) -> (PartialResult, i64);
    /// 重置部分结果到初始状态。
    fn reset_partial_result(&self, partial_result: &mut PartialResult);
    /// 用组内若干行更新部分结果，返回内存增量。
    fn update_partial_result(
        &self,
        context: &dyn EvalContext,
        rows_in_group: &[Row],
        partial_result: &mut PartialResult,
    ) -> Result<i64, AggError>;
    /// 将 source 合并进 destination，返回内存增量。
    fn merge_partial_result(
        &self,
        context: &dyn EvalContext,
        source: &PartialResult,
        destination: &mut PartialResult,
    ) -> Result<i64, AggError>;
    /// 将最终聚合值追加到输出 chunk。
    fn append_final_result_to_chunk(
        &self,
        context: &dyn EvalContext,
        partial_result: &PartialResult,
        chunk: &mut Chunk,
    ) -> Result<(), AggError>;
}

/// 聚合函数基类：持有参数表达式、结果列序号与返回类型。
#[derive(Default)]
pub struct BaseAggFunc {
    /// 聚合参数表达式列表。
    pub args: Vec<Box<dyn Expression>>,
    /// 结果在输出 chunk 中的列序号（ordinal）。
    pub ordinal: usize,
    /// 聚合结果返回类型。
    pub return_type: Option<FieldType>,
}

impl BaseAggFunc {
    /// 默认合并：子类应覆盖；未实现则 panic。
    pub fn merge_partial_result(
        &self,
        _context: &dyn EvalContext,
        _source: &PartialResult,
        _destination: &mut PartialResult,
    ) -> Result<i64, AggError> {
        panic!("Not implemented")
    }

    /// 默认序列化：子类应覆盖；未实现则 panic。
    pub fn serialize_partial_result(
        &self,
        _partial_result: &PartialResult,
        _chunk: &mut Chunk,
        _spill_helper: &mut SerializeHelper,
    ) {
        panic!("Not implemented")
    }

    /// 默认反序列化：子类应覆盖；未实现则 panic。
    pub fn deserialize_partial_result(&self, _source: &Chunk) -> (Vec<PartialResult>, i64) {
        panic!("Not implemented")
    }
}

/// 滑动窗口聚合：窗口边界平移时增量更新部分结果（slide）。
pub trait SlidingWindowAggFunc {
    /// 根据窗口起止与位移增量更新部分结果。
    fn slide(
        &self,
        context: &dyn EvalContext,
        get_row: &mut dyn FnMut(u64) -> Row,
        last_start: u64,
        last_end: u64,
        shift_start: u64,
        shift_end: u64,
        partial_result: &mut PartialResult,
    ) -> Result<(), AggError>;
}

/// MAX/MIN 滑动窗口专用：设置当前窗口起点。
pub trait MaxMinSlidingWindowAggFunc {
    /// 记录滑动窗口当前起始行号。
    fn set_window_start(&mut self, start: u64);
}

/// 通用反序列化循环：按行调用 deserialize 闭包，并校验行数一致。
pub fn deserialize_partial_result_common<F>(
    source: &Chunk,
    ordinal: usize,
    mut deserialize: F,
) -> (Vec<PartialResult>, i64)
where
    F: FnMut(&mut DeserializeHelper<'_>) -> (Option<PartialResult>, i64),
{
    let mut helper = DeserializeHelper::new(source.Column(ordinal), source.NumRows());
    let mut total_memory_delta = 0;
    let mut partial_results = Vec::with_capacity(source.NumRows());

    while let (Some(partial_result), memory_delta) = deserialize(&mut helper) {
        partial_results.push(partial_result);
        total_memory_delta += memory_delta;
    }

    assert_eq!(
        partial_results.len(),
        source.NumRows(),
        "Fail to deserialize partial result"
    );
    (partial_results, total_memory_delta)
}

/// COUNT 聚合的部分结果（计数值）。
pub type PartialResult4Count = i64;
/// 按位聚合（BIT_AND/OR/XOR）的部分结果。
pub type PartialResult4BitFunc = u64;

/// MAX/MIN 通用部分结果：是否为空及当前极值。
#[derive(Clone, Debug, PartialEq)]
pub struct MaxMinPartialResult<T> {
    /// 当前组是否尚无有效值（全为 NULL）。
    pub is_null: bool,
    /// 当前极值。
    pub value: T,
}

impl<T: Default> Default for MaxMinPartialResult<T> {
    fn default() -> Self {
        Self {
            is_null: false,
            value: T::default(),
        }
    }
}

/// MAX/MIN 整数部分结果。
pub type PartialResult4MaxMinInt = MaxMinPartialResult<i64>;
/// MAX/MIN 无符号整数部分结果。
pub type PartialResult4MaxMinUint = MaxMinPartialResult<u64>;
/// MAX/MIN Decimal 部分结果。
pub type PartialResult4MaxMinDecimal = MaxMinPartialResult<types::MyDecimal>;
/// MAX/MIN Float32 部分结果。
pub type PartialResult4MaxMinFloat32 = MaxMinPartialResult<f32>;
/// MAX/MIN Float64 部分结果。
pub type PartialResult4MaxMinFloat64 = MaxMinPartialResult<f64>;
/// MAX/MIN 时间部分结果。
pub type PartialResult4MaxMinTime = MaxMinPartialResult<types::Time>;
/// MAX/MIN Duration 部分结果。
pub type PartialResult4MaxMinDuration = MaxMinPartialResult<types::Duration>;
/// MAX/MIN 字符串部分结果。
pub type PartialResult4MaxMinString = MaxMinPartialResult<String>;
/// MAX/MIN JSON 部分结果。
pub type PartialResult4MaxMinJson = MaxMinPartialResult<types::BinaryJSON>;
/// MAX/MIN ENUM 部分结果。
pub type PartialResult4MaxMinEnum = MaxMinPartialResult<types::Enum>;
/// MAX/MIN SET 部分结果。
pub type PartialResult4MaxMinSet = MaxMinPartialResult<types::Set>;

/// AVG(Decimal) 部分结果：累加和与非空行数。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AvgDecimalPartialResult {
    /// Decimal 累加和。
    pub sum: types::MyDecimal,
    /// 参与平均的非空行数。
    pub count: i64,
}

/// AVG(Float64) 部分结果：累加和与非空行数。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AvgFloat64PartialResult {
    /// 浮点累加和。
    pub sum: f64,
    /// 参与平均的非空行数。
    pub count: i64,
}

/// SUM 通用部分结果：累加值与非空行计数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SumPartialResult<T> {
    /// 累加值。
    pub value: T,
    /// 非空行数量（用于最终是否输出 NULL）。
    pub not_null_row_count: i64,
}

/// SUM(Decimal) 部分结果。
pub type PartialResult4SumDecimal = SumPartialResult<types::MyDecimal>;
/// SUM(Float64) 部分结果。
pub type PartialResult4SumFloat64 = SumPartialResult<f64>;
/// SUM(Int64) 部分结果。
pub type PartialResult4SumInt64 = SumPartialResult<i64>;
/// SUM(Uint64) 部分结果。
pub type PartialResult4SumUint64 = SumPartialResult<u64>;

/// GROUP_CONCAT 部分结果：值缓冲与可选额外缓冲。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GroupConcatPartialResult {
    /// 已拼接字符串的字节缓冲。
    pub values_buffer: Cursor<Vec<u8>>,
    /// 可选辅助缓冲。
    pub buffer: Option<Cursor<Vec<u8>>>,
}

/// spill 时序列化的标量/复合值枚举。
#[derive(Clone, Debug, PartialEq)]
pub enum SpillValue {
    Bool(bool),
    Int64(i64),
    Uint64(u64),
    Float64(f64),
    String(String),
    BinaryJson(types::BinaryJSON),
    Opaque(types::Opaque),
    Time(types::Time),
    Duration(types::Duration),
}

impl SpillValue {
    /// 变长类型额外占用的堆内存（容量口径）。
    pub fn memory_usage(&self) -> i64 {
        match self {
            Self::String(value) => value.capacity() as i64,
            Self::BinaryJson(value) => value.Value.capacity() as i64,
            Self::Opaque(value) => value.Buf.capacity() as i64,
            _ => 0,
        }
    }
}

/// JSON_ARRAYAGG 部分结果：元素列表。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsonArrayPartialResult {
    /// 已聚合的 JSON 数组元素。
    pub entries: Vec<SpillValue>,
}

/// JSON_OBJECTAGG 部分结果：键值映射。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsonObjectPartialResult {
    /// 已聚合的 JSON 对象键值对。
    pub entries: HashMap<String, SpillValue>,
}

/// FIRST_ROW 状态：是否为空、是否已取到第一行。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FirstRowState {
    /// 第一行值为 NULL。
    pub is_null: bool,
    /// 是否已捕获第一行。
    pub got_first_row: bool,
}

/// FIRST_ROW 通用部分结果。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FirstRowPartialResult<T> {
    /// 捕获状态。
    pub state: FirstRowState,
    /// 第一行的值。
    pub value: T,
}

/// FIRST_ROW(Int) 部分结果。
pub type PartialResult4FirstRowInt = FirstRowPartialResult<i64>;
/// FIRST_ROW(Float32) 部分结果。
pub type PartialResult4FirstRowFloat32 = FirstRowPartialResult<f32>;
/// FIRST_ROW(Float64) 部分结果。
pub type PartialResult4FirstRowFloat64 = FirstRowPartialResult<f64>;
/// FIRST_ROW(Decimal) 部分结果。
pub type PartialResult4FirstRowDecimal = FirstRowPartialResult<types::MyDecimal>;
/// FIRST_ROW(String) 部分结果。
pub type PartialResult4FirstRowString = FirstRowPartialResult<String>;
/// FIRST_ROW(Time) 部分结果。
pub type PartialResult4FirstRowTime = FirstRowPartialResult<types::Time>;
/// FIRST_ROW(Duration) 部分结果。
pub type PartialResult4FirstRowDuration = FirstRowPartialResult<types::Duration>;
/// FIRST_ROW(JSON) 部分结果。
pub type PartialResult4FirstRowJson = FirstRowPartialResult<types::BinaryJSON>;
/// FIRST_ROW(ENUM) 部分结果。
pub type PartialResult4FirstRowEnum = FirstRowPartialResult<types::Enum>;
/// FIRST_ROW(SET) 部分结果。
pub type PartialResult4FirstRowSet = FirstRowPartialResult<types::Set>;

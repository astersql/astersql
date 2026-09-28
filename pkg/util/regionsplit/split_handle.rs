// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 手动拆分 Region 时的切分键生成与 handle 编码。
//
// Region 是 TiKV 中一段连续键空间；`SPLIT TABLE/INDEX` 需在上下界之间按段数
// 插入边界键。本模块按整数主键或 common handle（聚簇索引多列主键编码）编码
// 记录/索引键，并做最小步长校验，避免拆出过小的 Region。

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::atomic::{AtomicI64, Ordering};

/// 单段 Region 允许的最小步长（记录 ID 差值）；过小则拒绝拆分。
pub static MinRegionStepValue: AtomicI64 = AtomicI64::new(1000);

/// 拆分边界或 datum 编码失败时的错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SplitError {
    /// 上下界区间或拆分段数非法。
    InvalidRanges(String),
    /// 行数据不足以构造 handle。
    InvalidDatum(String),
}

impl Display for SplitError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRanges(message) => formatter.write_str(message),
            Self::InvalidDatum(message) => formatter.write_str(message),
        }
    }
}

impl Error for SplitError {}

/// 简化版 SQL datum，用于表达拆分边界中的列值。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Datum {
    Null,
    Int(i64),
    UInt(u64),
    Bytes(Vec<u8>),
    String(String),
}

impl Datum {
    /// 取有符号整数视图；非数值类型返回 0（对齐 Go 转换习惯）。
    fn as_i64(&self) -> i64 {
        match self {
            Self::Int(value) => *value,
            Self::UInt(value) => *value as i64,
            _ => 0,
        }
    }

    /// 取无符号整数视图；非数值类型返回 0。
    fn as_u64(&self) -> u64 {
        match self {
            Self::UInt(value) => *value,
            Self::Int(value) => *value as u64,
            _ => 0,
        }
    }
}

impl Display for Datum {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => formatter.write_str("NULL"),
            Self::Int(value) => Display::fmt(value, formatter),
            Self::UInt(value) => Display::fmt(value, formatter),
            Self::Bytes(value) => write!(formatter, "{:?}", value),
            Self::String(value) => formatter.write_str(value),
        }
    }
}

/// 语句级上下文占位（时区等）；迁移期仅保留字段形状。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StatementContext {
    pub time_zone: String,
}

/// 索引元信息：物理 index id 与名称。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexInfo {
    pub id: i64,
    pub name: String,
}

/// 表元信息：主键/common handle 标志与索引列表。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TableInfo {
    pub name: String,
    /// 是否以整数主键列作为行 handle（非 common handle）。
    pub pk_is_handle: bool,
    /// 整数主键是否为无符号类型。
    pub pk_is_unsigned: bool,
    /// 是否为 common handle（聚簇索引，主键编码进行键）。
    pub is_common_handle: bool,
    pub indices: Vec<IndexInfo>,
}

/// 行定位 handle：整数主键或 common handle 字节序列。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Handle {
    Int(i64),
    Common(Vec<u8>),
}

impl Handle {
    /// 编码为可拼接到记录前缀后的字节。
    fn encoded(&self) -> Vec<u8> {
        match self {
            // Go `kv.IntHandle.Encoded` is exactly `codec.EncodeInt`: an
            // eight-byte comparable integer without a datum type tag.
            Self::Int(value) => encode_i64(*value).to_vec(),
            Self::Common(value) => value.clone(),
        }
    }
}

/// 由一行 datum 构造拆分用 handle 的策略接口。
pub trait SplitHandleCols {
    /// 将边界行编码为 Handle。
    fn BuildHandleByDatums(
        &self,
        statement_context: &StatementContext,
        row: &[Datum],
    ) -> Result<Handle, SplitError>;
    /// 是否为整数 handle（可走等差步长路径）。
    fn IsInt(&self) -> bool;
}

/// 按上下界与段数计算整数 handle 的起点与步长，并校验最小步长。
fn calculate_int_bound_value(
    table: &TableInfo,
    lower: &[Datum],
    upper: &[Datum],
    number: usize,
) -> Result<(i64, i64), SplitError> {
    if number == 0 || lower.is_empty() || upper.is_empty() {
        return Err(SplitError::InvalidRanges(
            "split region bounds and count must be non-empty".to_owned(),
        ));
    }
    // 无符号主键用 u64 差值；有符号用 wrapping 减法再除，对齐 Go 溢出语义。
    let (lower_value, step) = if table.pk_is_handle && table.pk_is_unsigned {
        let lower_record_id = lower[0].as_u64();
        let upper_record_id = upper[0].as_u64();
        if upper_record_id <= lower_record_id {
            return Err(SplitError::InvalidRanges(format!(
                "lower value {lower_record_id} should less than the upper value {upper_record_id}"
            )));
        }
        (
            lower_record_id as i64,
            ((upper_record_id - lower_record_id) / number as u64) as i64,
        )
    } else {
        let lower_record_id = lower[0].as_i64();
        let upper_record_id = upper[0].as_i64();
        if upper_record_id <= lower_record_id {
            return Err(SplitError::InvalidRanges(format!(
                "lower value {lower_record_id} should less than the upper value {upper_record_id}"
            )));
        }
        (
            lower_record_id,
            (upper_record_id.wrapping_sub(lower_record_id) as u64 / number as u64) as i64,
        )
    };
    let minimum = MinRegionStepValue.load(Ordering::Acquire);
    if step < minimum {
        return Err(SplitError::InvalidRanges(format!(
            "the region size is too small, expected at least {minimum}, but got {step}"
        )));
    }
    Ok((lower_value, step))
}

/// 生成拆分表 Region 的中间记录键列表。
///
/// `physical_id` 为物理表/分区 id；`number` 为期望拆成的段数。
/// 若表含次级索引，会先插入记录区前缀，便于与索引键空间一并切分。
pub fn GetSplitTableKeys(
    statement_context: &StatementContext,
    table: &TableInfo,
    handle_columns: &dyn SplitHandleCols,
    physical_id: i64,
    lower: &[Datum],
    upper: &[Datum],
    number: usize,
    mut keys: Vec<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, SplitError> {
    let record_prefix = gen_table_record_prefix(physical_id);
    // common handle 且仅一个索引时，索引即主键，无需额外插记录前缀。
    let contains_index =
        !table.indices.is_empty() && !(table.is_common_handle && table.indices.len() == 1);
    if contains_index {
        keys.push(record_prefix.clone());
    }

    if handle_columns.IsInt() {
        let (lower_value, step) = calculate_int_bound_value(table, lower, upper, number)?;
        let mut record_id = lower_value;
        // 插入 number-1 个等分点（不含上下界本身）。
        for _ in 1..number {
            record_id = record_id.wrapping_add(step);
            keys.push(encode_record_key(&record_prefix, &Handle::Int(record_id)));
        }
        return Ok(keys);
    }

    let lower_handle = handle_columns.BuildHandleByDatums(statement_context, lower)?;
    let upper_handle = handle_columns.BuildHandleByDatums(statement_context, upper)?;
    if lower_handle.encoded() >= upper_handle.encoded() {
        return Err(SplitError::InvalidRanges(format!(
            "Split table `{}` region lower value {} should less than the upper value {}",
            table.name,
            datum_slice_to_string(lower),
            datum_slice_to_string(upper)
        )));
    }
    let low = encode_record_key(&record_prefix, &lower_handle);
    let upper = encode_record_key(&record_prefix, &upper_handle);
    get_values_list(&low, &upper, number, keys)
}

/// 为指定索引追加物理起始键与下一索引前缀，用于索引区间边界。
pub fn GetSplitIdxPhysicalStartAndOtherIdxKeys(
    table: &TableInfo,
    index: &IndexInfo,
    physical_id: i64,
    mut keys: Vec<Vec<u8>>,
) -> Vec<Vec<u8>> {
    // 非首个索引时先推入本索引前缀，保证切分点落在该索引键空间。
    if table
        .indices
        .first()
        .is_some_and(|first| first.id != index.id)
    {
        keys.push(encode_table_index_prefix(physical_id, index.id));
    }
    keys.push(encode_table_index_prefix(physical_id, index.id + 1));
    keys
}

/// 生成拆分索引 Region 的中间索引键列表。
pub fn GetSplitIndexKeys(
    _statement_context: &StatementContext,
    table: &TableInfo,
    index: &IndexInfo,
    physical_id: i64,
    lower: &[Datum],
    upper: &[Datum],
    number: usize,
    keys: Vec<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, SplitError> {
    if number == 0 {
        return Err(SplitError::InvalidRanges(
            "split region count must be positive".to_owned(),
        ));
    }
    let new_keys = GetSplitIdxPhysicalStartAndOtherIdxKeys(table, index, physical_id, keys.clone());
    // handle 取 i64::MIN 作为索引键后缀占位，与 Go 拆分路径一致。
    let lower_index_key = encode_index_key(physical_id, index.id, lower, i64::MIN)?;
    let upper_index_key = encode_index_key(physical_id, index.id, upper, i64::MIN)?;
    if lower_index_key >= upper_index_key {
        return Err(SplitError::InvalidRanges(format!(
            "Split index `{}` region lower value {} should less than the upper value {}",
            index.name,
            datum_slice_to_string(lower),
            datum_slice_to_string(upper)
        )));
    }
    get_values_list(&lower_index_key, &upper_index_key, number, new_keys)
}

/// 将 datum 切片格式化为错误信息中的 `(v1,v2,...)` 字符串。
fn datum_slice_to_string(datums: &[Datum]) -> String {
    format!(
        "({})",
        datums
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// 整数主键 handle 列策略。
pub struct intHandleCols;

impl SplitHandleCols for intHandleCols {
    fn BuildHandleByDatums(
        &self,
        _statement_context: &StatementContext,
        row: &[Datum],
    ) -> Result<Handle, SplitError> {
        row.first()
            .map(|datum| Handle::Int(datum.as_i64()))
            .ok_or_else(|| SplitError::InvalidDatum("integer handle requires one datum".to_owned()))
    }

    fn IsInt(&self) -> bool {
        true
    }
}

/// Common handle（聚簇多列主键）列策略：将整行 datum 编码为字节。
pub struct commonHandleCols;

impl SplitHandleCols for commonHandleCols {
    fn BuildHandleByDatums(
        &self,
        _statement_context: &StatementContext,
        row: &[Datum],
    ) -> Result<Handle, SplitError> {
        Ok(Handle::Common(encode_datums(row)?))
    }

    fn IsInt(&self) -> bool {
        false
    }
}

/// 按表是否 common handle 选择拆分用的 handle 列策略。
pub fn BuildHandleColsForSplit(table: &TableInfo) -> Box<dyn SplitHandleCols> {
    if table.is_common_handle {
        Box::new(commonHandleCols)
    } else {
        Box::new(intHandleCols)
    }
}

/// 有符号整数的可比较编码：异或符号位后大端写出（对齐 TiDB codec）。
fn encode_i64(value: i64) -> [u8; 8] {
    ((value as u64) ^ (1_u64 << 63)).to_be_bytes()
}

/// 生成表记录键前缀 `t{table_id}_r`。
fn gen_table_record_prefix(table_id: i64) -> Vec<u8> {
    let mut key = Vec::with_capacity(11);
    key.push(b't');
    key.extend_from_slice(&encode_i64(table_id));
    key.extend_from_slice(b"_r");
    key
}

/// 记录前缀拼接 handle，得到完整行键。
fn encode_record_key(prefix: &[u8], handle: &Handle) -> Vec<u8> {
    let mut key = prefix.to_vec();
    key.extend_from_slice(&handle.encoded());
    key
}

/// 生成索引键前缀 `t{table_id}_i{index_id}`。
fn encode_table_index_prefix(table_id: i64, index_id: i64) -> Vec<u8> {
    let mut key = Vec::with_capacity(19);
    key.push(b't');
    key.extend_from_slice(&encode_i64(table_id));
    key.extend_from_slice(b"_i");
    key.extend_from_slice(&encode_i64(index_id));
    key
}

/// 编码完整索引键：前缀 + 索引列值 + handle 后缀。
fn encode_index_key(
    table_id: i64,
    index_id: i64,
    values: &[Datum],
    handle: i64,
) -> Result<Vec<u8>, SplitError> {
    let mut key = encode_table_index_prefix(table_id, index_id);
    key.extend_from_slice(&encode_datums(values)?);
    key.push(INT_HANDLE_FLAG);
    key.extend_from_slice(&encode_i64(handle));
    Ok(key)
}

/// 将 datum 序列编码为可比较字节（带类型标签）。
fn encode_datums(datums: &[Datum]) -> Result<Vec<u8>, SplitError> {
    let mut encoded = Vec::new();
    for datum in datums {
        match datum {
            Datum::Null => encoded.push(0),
            Datum::Int(value) => {
                encoded.push(INT_FLAG);
                encoded.extend_from_slice(&encode_i64(*value));
            }
            Datum::UInt(value) => {
                encoded.push(UINT_FLAG);
                encoded.extend_from_slice(&value.to_be_bytes());
            }
            Datum::Bytes(value) => {
                encoded.push(BYTES_FLAG);
                encode_memcomparable_bytes(value, &mut encoded);
            }
            Datum::String(value) => {
                encoded.push(BYTES_FLAG);
                encode_memcomparable_bytes(value.as_bytes(), &mut encoded);
            }
        }
    }
    Ok(encoded)
}

/// `codec.EncodeKey` 的可比较 datum 标志。
const BYTES_FLAG: u8 = 1;
const INT_FLAG: u8 = 3;
const UINT_FLAG: u8 = 4;
const INT_HANDLE_FLAG: u8 = INT_FLAG;

/// 按 Go `codec.EncodeBytes` 的 8 字节分组 + marker 编码字节串。
fn encode_memcomparable_bytes(value: &[u8], output: &mut Vec<u8>) {
    let mut offset = 0;
    while offset <= value.len() {
        let remaining = value.len() - offset;
        let copied = remaining.min(8);
        output.extend_from_slice(&value[offset..offset + copied]);
        output.extend(std::iter::repeat_n(0, 8 - copied));
        output.push(0xff - (8 - copied) as u8);
        offset += 8;
    }
}

/// 在两条有序键之间按段数等分，生成中间切分键。
///
/// 先取公共前缀，再将后续字节补齐为 u64 做等差插值，最后写回前缀+大端步长。
fn get_values_list(
    lower: &[u8],
    upper: &[u8],
    number: usize,
    mut values: Vec<Vec<u8>>,
) -> Result<Vec<Vec<u8>>, SplitError> {
    if number == 0 {
        return Err(SplitError::InvalidRanges(
            "split region count must be positive".to_owned(),
        ));
    }
    let common_prefix = lower
        .iter()
        .zip(upper)
        .take_while(|(left, right)| left == right)
        .count();
    let lower_value = padded_u64(&lower[common_prefix..], 0);
    let upper_value = padded_u64(&upper[common_prefix..], 0xff);
    let step = upper_value.wrapping_sub(lower_value) / number as u64;
    let mut current = lower_value;
    for _ in 0..number.saturating_sub(1) {
        current = current.wrapping_add(step);
        let mut value = Vec::with_capacity(common_prefix + 8);
        value.extend_from_slice(&lower[..common_prefix]);
        value.extend_from_slice(&current.to_be_bytes());
        values.push(value);
    }
    Ok(values)
}

/// 将后缀字节左对齐填入 8 字节，不足处用 padding 填充，再解释为大端 u64。
fn padded_u64(bytes: &[u8], padding: u8) -> u64 {
    let mut buffer = [padding; 8];
    let copied = bytes.len().min(8);
    buffer[..copied].copy_from_slice(&bytes[..copied]);
    u64::from_be_bytes(buffer)
}

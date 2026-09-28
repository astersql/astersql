// Copyright 2026 AsterSQL.
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

// Vectorized comparison primitives from `builtin_compare_vec.go`.
//
// The package integration layer supplies TiDB expressions and chunk columns.
// This file owns the row-independent algorithms: NULL propagation, signed and
// unsigned integer ordering, comparison result mapping, INTERVAL searches and
// greatest/least aggregation. Keeping those algorithms independent makes the
// Go control flow testable without inventing replacements for TiDB services.

//
// 向量化比较原语（对齐 `builtin_compare_vec.go`）：
// NULL 传播、有无符号整型列比较、比较结果映射、INTERVAL 线性/二分搜索，
// 以及 GREATEST/LEAST 按类型族的列式聚合。

use rust_decimal::Decimal;
use std::cmp::Ordering;
use std::error::Error;
use std::fmt;

/// 向量比较前/中的错误（参数缺失、长度不一致、NULL 下标非法、转换失败）。
/// Errors raised before or while evaluating a vector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompareVecError {
    /// 未提供任何比较参数列。
    NoArguments,
    /// 各参数列长度不一致。
    LengthMismatch { expected: usize, actual: usize },
    /// NULL 下标超出列长度。
    InvalidNullIndex { index: usize, len: usize },
    /// 类型/时间转换失败。
    Conversion(String),
}

impl fmt::Display for CompareVecError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoArguments => formatter.write_str("comparison requires at least one argument"),
            Self::LengthMismatch { expected, actual } => {
                write!(
                    formatter,
                    "column length mismatch: expected {expected}, got {actual}"
                )
            }
            Self::InvalidNullIndex { index, len } => {
                write!(
                    formatter,
                    "NULL index {index} is outside column length {len}"
                )
            }
            Self::Conversion(message) => formatter.write_str(message),
        }
    }
}

impl Error for CompareVecError {}

/// 定宽列及其 NULL 位图。
/// A fixed-width column and its NULL bitmap.
#[derive(Clone, Debug, PartialEq)]
pub struct NullableColumn<T> {
    pub values: Vec<T>,
    nulls: Vec<bool>,
}

impl<T> NullableColumn<T> {
    /// 全非空列。
    pub fn new(values: Vec<T>) -> Self {
        let nulls = vec![false; values.len()];
        Self { values, nulls }
    }

    /// 按下标设置 NULL 位；越界返回 InvalidNullIndex。
    pub fn with_nulls(values: Vec<T>, null_indices: &[usize]) -> Result<Self, CompareVecError> {
        let len = values.len();
        let mut nulls = vec![false; len];
        for &index in null_indices {
            if index >= len {
                return Err(CompareVecError::InvalidNullIndex { index, len });
            }
            nulls[index] = true;
        }
        Ok(Self { values, nulls })
    }

    /// 行数。
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// 是否为空列。
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// 指定行是否为 NULL。
    pub fn is_null(&self, row: usize) -> bool {
        self.nulls[row]
    }

    /// 迭代所有 NULL 行下标。
    pub fn null_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.nulls
            .iter()
            .enumerate()
            .filter_map(|(index, is_null)| is_null.then_some(index))
    }
}

/// 校验多列等长，返回行数。
fn validate_columns<T>(columns: &[NullableColumn<T>]) -> Result<usize, CompareVecError> {
    let expected = columns.first().ok_or(CompareVecError::NoArguments)?.len();
    for column in &columns[1..] {
        if column.len() != expected {
            return Err(CompareVecError::LengthMismatch {
                expected,
                actual: column.len(),
            });
        }
    }
    Ok(expected)
}

/// 按行聚合极值：任一侧 NULL 则结果行置 NULL。
fn extremum_by<T, F>(
    columns: &[NullableColumn<T>],
    mut take_candidate: F,
) -> Result<NullableColumn<T>, CompareVecError>
where
    T: Clone,
    F: FnMut(&T, &T) -> bool,
{
    let rows = validate_columns(columns)?;
    let mut result = columns[0].clone();
    for argument in &columns[1..] {
        for row in 0..rows {
            // 任一侧为 NULL：结果行置 NULL，跳过数值比较。
            if result.nulls[row] || argument.nulls[row] {
                result.nulls[row] = true;
                continue;
            }
            if take_candidate(&argument.values[row], &result.values[row]) {
                result.values[row] = argument.values[row].clone();
            }
        }
    }
    Ok(result)
}

/// DECIMAL 列 GREATEST。
pub fn greatest_decimal(
    columns: &[NullableColumn<Decimal>],
) -> Result<NullableColumn<Decimal>, CompareVecError> {
    extremum_by(columns, |candidate, current| candidate > current)
}

/// DECIMAL 列 LEAST。
pub fn least_decimal(
    columns: &[NullableColumn<Decimal>],
) -> Result<NullableColumn<Decimal>, CompareVecError> {
    extremum_by(columns, |candidate, current| candidate < current)
}

/// i64 列 GREATEST。
pub fn greatest_i64(
    columns: &[NullableColumn<i64>],
) -> Result<NullableColumn<i64>, CompareVecError> {
    extremum_by(columns, |candidate, current| candidate > current)
}

/// i64 列 LEAST。
pub fn least_i64(columns: &[NullableColumn<i64>]) -> Result<NullableColumn<i64>, CompareVecError> {
    extremum_by(columns, |candidate, current| candidate < current)
}

/// f64 列 GREATEST（NaN/±0 与 Go `>` 分支一致：保留先出现的操作数）。
pub fn greatest_f64(
    columns: &[NullableColumn<f64>],
) -> Result<NullableColumn<f64>, CompareVecError> {
    // Direct comparison intentionally keeps the first operand for NaN and for
    // equal signed zero, exactly like the Go `>` branch.
    extremum_by(columns, |candidate, current| candidate > current)
}

/// f64 列 LEAST。
pub fn least_f64(columns: &[NullableColumn<f64>]) -> Result<NullableColumn<f64>, CompareVecError> {
    extremum_by(columns, |candidate, current| candidate < current)
}

/// 字符串列 GREATEST，比较器由调用方注入（排序规则）。
pub fn greatest_string_by<F>(
    columns: &[NullableColumn<String>],
    mut compare: F,
) -> Result<NullableColumn<String>, CompareVecError>
where
    F: FnMut(&str, &str) -> Ordering,
{
    extremum_by(columns, |candidate, current| {
        // Go keeps `src` only when it is strictly greater; collation-equal
        // strings therefore select the later argument (`arg`).
        compare(candidate, current) != Ordering::Less
    })
}

/// 字符串列 LEAST，比较器由调用方注入。
pub fn least_string_by<F>(
    columns: &[NullableColumn<String>],
    mut compare: F,
) -> Result<NullableColumn<String>, CompareVecError>
where
    F: FnMut(&str, &str) -> Ordering,
{
    extremum_by(columns, |candidate, current| {
        // Go keeps `src` only when it is strictly less; collation-equal
        // strings therefore select the later argument (`arg`).
        compare(candidate, current) != Ordering::Greater
    })
}

/// 六种普通比较算子加上 MySQL 的 NULL-safe 相等（<=>）。
/// The six ordinary comparison operators plus MySQL's NULL-safe equality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompareOp {
    Lt,
    Le,
    Eq,
    Ne,
    Gt,
    Ge,
    NullEq,
}

/// 将原始 -1/0/1 比较结果映射为 0/1，对齐 Go `vecResOf*`。
/// Maps raw -1/0/1 comparison values using the same loops as vecResOf* in Go.
pub fn map_compare_results(results: &[i64], operation: CompareOp) -> Vec<i64> {
    results
        .iter()
        .map(|&result| {
            let matches = match operation {
                CompareOp::Lt => result < 0,
                CompareOp::Le => result <= 0,
                CompareOp::Eq | CompareOp::NullEq => result == 0,
                CompareOp::Ne => result != 0,
                CompareOp::Gt => result > 0,
                CompareOp::Ge => result >= 0,
            };
            i64::from(matches)
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 整型列底层：有符号或无符号向量。
enum IntValues {
    Signed(Vec<i64>),
    Unsigned(Vec<u64>),
}

/// 整型列保留 MySQL 无符号标志，避免将 u64 收窄为 i64。
/// Integer values retain their MySQL unsigned flag instead of narrowing u64.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntColumn {
    values: IntValues,
    nulls: Vec<bool>,
}

impl IntColumn {
    /// 构造有符号整型列。
    pub fn signed(values: Vec<i64>, null_indices: &[usize]) -> Result<Self, CompareVecError> {
        Self::build(IntValues::Signed(values), null_indices)
    }

    /// 构造无符号整型列。
    pub fn unsigned(values: Vec<u64>, null_indices: &[usize]) -> Result<Self, CompareVecError> {
        Self::build(IntValues::Unsigned(values), null_indices)
    }

    /// 共用构造：写入 NULL 位图。
    fn build(values: IntValues, null_indices: &[usize]) -> Result<Self, CompareVecError> {
        let len = match &values {
            IntValues::Signed(values) => values.len(),
            IntValues::Unsigned(values) => values.len(),
        };
        let mut nulls = vec![false; len];
        for &index in null_indices {
            if index >= len {
                return Err(CompareVecError::InvalidNullIndex { index, len });
            }
            nulls[index] = true;
        }
        Ok(Self { values, nulls })
    }

    /// 行数。
    pub fn len(&self) -> usize {
        self.nulls.len()
    }

    /// 是否为空列。
    pub fn is_empty(&self) -> bool {
        self.nulls.is_empty()
    }

    /// 指定行是否为 NULL。
    pub fn is_null(&self, row: usize) -> bool {
        self.nulls[row]
    }

    /// 是否为无符号列。
    pub fn is_unsigned(&self) -> bool {
        matches!(self.values, IntValues::Unsigned(_))
    }
}

/// 单行有无符号混比（UU/UI/IU/II）。
fn compare_int_at(left: &IntColumn, right: &IntColumn, row: usize) -> Ordering {
    match (&left.values, &right.values) {
        (IntValues::Signed(left), IntValues::Signed(right)) => left[row].cmp(&right[row]),
        (IntValues::Unsigned(left), IntValues::Unsigned(right)) => left[row].cmp(&right[row]),
        (IntValues::Unsigned(left), IntValues::Signed(right)) => {
            // 无符号 vs 负有符号：无符号更大。
            if right[row] < 0 {
                Ordering::Greater
            } else {
                left[row].cmp(&(right[row] as u64))
            }
        }
        (IntValues::Signed(left), IntValues::Unsigned(right)) => {
            if left[row] < 0 {
                Ordering::Less
            } else {
                (left[row] as u64).cmp(&right[row])
            }
        }
    }
}

/// Ordering → -1/0/1。
fn ordering_value(ordering: Ordering) -> i64 {
    match ordering {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// 校验两整型列等长。
fn validate_int_columns(left: &IntColumn, right: &IntColumn) -> Result<usize, CompareVecError> {
    if left.len() != right.len() {
        return Err(CompareVecError::LengthMismatch {
            expected: left.len(),
            actual: right.len(),
        });
    }
    Ok(left.len())
}

/// 向量化整型比较：含混符号与 NULL 传播；NullEq 单独处理两 NULL。
/// Vectorized integer comparison, including UU/UI/IU/II and NULL propagation.
pub fn compare_int_columns(
    left: &IntColumn,
    right: &IntColumn,
    operation: CompareOp,
) -> Result<NullableColumn<i64>, CompareVecError> {
    let rows = validate_int_columns(left, right)?;
    let raw: Vec<i64> = (0..rows)
        .map(|row| ordering_value(compare_int_at(left, right, row)))
        .collect();

    // <=> ：两 NULL 为真；一 NULL 为假；否则看相等。结果列本身非空。
    if operation == CompareOp::NullEq {
        let values = (0..rows)
            .map(|row| match (left.nulls[row], right.nulls[row]) {
                (true, true) => 1,
                (true, false) | (false, true) => 0,
                (false, false) => i64::from(raw[row] == 0),
            })
            .collect();
        return Ok(NullableColumn::new(values));
    }

    let values = map_compare_results(&raw, operation);
    let nulls = (0..rows)
        .map(|row| left.nulls[row] || right.nulls[row])
        .collect();
    Ok(NullableColumn { values, nulls })
}

/// 校验 INTERVAL 整型边界列与目标等长。
fn validate_int_boundaries(
    target: &IntColumn,
    boundaries: &[IntColumn],
) -> Result<usize, CompareVecError> {
    let expected = target.len();
    for boundary in boundaries {
        if boundary.len() != expected {
            return Err(CompareVecError::LengthMismatch {
                expected,
                actual: boundary.len(),
            });
        }
    }
    Ok(expected)
}

/// INTERVAL 整型：有 NULL 边界时线性扫描，否则二分；目标 NULL 产出 -1。
/// Implements builtinIntervalIntSig's per-row linear/binary search choice.
pub fn interval_int(
    target: &IntColumn,
    boundaries: &[IntColumn],
    has_nullable: bool,
) -> Result<Vec<i64>, CompareVecError> {
    let rows = validate_int_boundaries(target, boundaries)?;
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        if target.nulls[row] {
            result.push(-1);
            continue;
        }

        // 存在可空边界：只能线性扫描；无 NULL 边界时可二分。
        let index = if has_nullable {
            let mut index = 0;
            while index < boundaries.len() {
                let boundary = &boundaries[index];
                if !boundary.nulls[row] && compare_int_at(target, boundary, row) == Ordering::Less {
                    break;
                }
                index += 1;
            }
            index
        } else {
            let (mut low, mut high) = (0, boundaries.len());
            while low < high {
                let middle = low + (high - low) / 2;
                let boundary = &boundaries[middle];
                let target_is_less =
                    !boundary.nulls[row] && compare_int_at(target, boundary, row) == Ordering::Less;
                if target_is_less {
                    high = middle;
                } else {
                    low = middle + 1;
                }
            }
            low
        };
        result.push(index as i64);
    }
    Ok(result)
}

/// 校验 INTERVAL 实数边界列与目标等长。
fn validate_real_boundaries(
    target: &NullableColumn<f64>,
    boundaries: &[NullableColumn<f64>],
) -> Result<usize, CompareVecError> {
    let expected = target.len();
    for boundary in boundaries {
        if boundary.len() != expected {
            return Err(CompareVecError::LengthMismatch {
                expected,
                actual: boundary.len(),
            });
        }
    }
    Ok(expected)
}

/// INTERVAL 实数：目标 NULL 产出 -1（不是 SQL NULL）。
/// Implements builtinIntervalRealSig. NULL targets produce -1, not SQL NULL.
pub fn interval_real(
    target: &NullableColumn<f64>,
    boundaries: &[NullableColumn<f64>],
    has_nullable: bool,
) -> Result<Vec<i64>, CompareVecError> {
    let rows = validate_real_boundaries(target, boundaries)?;
    let mut result = Vec::with_capacity(rows);
    for row in 0..rows {
        if target.nulls[row] {
            result.push(-1);
            continue;
        }

        let index = if has_nullable {
            let mut index = 0;
            while index < boundaries.len() {
                let boundary = &boundaries[index];
                if !boundary.nulls[row] && target.values[row] < boundary.values[row] {
                    break;
                }
                index += 1;
            }
            index
        } else {
            let (mut low, mut high) = (0, boundaries.len());
            while low < high {
                let middle = low + (high - low) / 2;
                let boundary = &boundaries[middle];
                if boundary.nulls[row] || !(target.values[row] < boundary.values[row]) {
                    low = middle + 1;
                } else {
                    high = middle;
                }
            }
            low
        };
        result.push(index as i64);
    }
    Ok(result)
}

/// 字符串按时间语义转换后再做 GREATEST/LEAST。
fn string_extremum_as_time<F, C>(
    columns: &[NullableColumn<String>],
    mut convert: C,
    mut take_candidate: F,
) -> Result<NullableColumn<String>, CompareVecError>
where
    F: FnMut(&str, &str) -> bool,
    C: FnMut(&str) -> Result<String, CompareVecError>,
{
    let rows = validate_columns(columns)?;
    let mut values = vec![String::new(); rows];
    let mut nulls = vec![false; rows];

    for (argument_index, argument) in columns.iter().enumerate() {
        for row in 0..rows {
            nulls[row] = nulls[row] || argument.nulls[row];
            if nulls[row] {
                continue;
            }
            let converted = convert(&argument.values[row])?;
            if argument_index == 0 || take_candidate(&converted, &values[row]) {
                values[row] = converted;
            }
        }
    }
    Ok(NullableColumn { values, nulls })
}

/// 字符串按时间转换后的 GREATEST。
pub fn greatest_string_as_time<C>(
    columns: &[NullableColumn<String>],
    convert: C,
) -> Result<NullableColumn<String>, CompareVecError>
where
    C: FnMut(&str) -> Result<String, CompareVecError>,
{
    string_extremum_as_time(columns, convert, |candidate, current| candidate > current)
}

/// 字符串按时间转换后的 LEAST。
pub fn least_string_as_time<C>(
    columns: &[NullableColumn<String>],
    convert: C,
) -> Result<NullableColumn<String>, CompareVecError>
where
    C: FnMut(&str) -> Result<String, CompareVecError>,
{
    string_extremum_as_time(columns, convert, |candidate, current| candidate < current)
}

/// 时间极值聚合后，对每个结果槽（含 NULL 槽）再跑一次转换，暴露转换错误。
fn time_extremum_by<T, F, C>(
    columns: &[NullableColumn<T>],
    take_candidate: F,
    mut convert_result: C,
) -> Result<NullableColumn<T>, CompareVecError>
where
    T: Clone,
    F: FnMut(&T, &T) -> bool,
    C: FnMut(T) -> Result<T, CompareVecError>,
{
    let mut result = extremum_by(columns, take_candidate)?;
    // Go converts every result slot after aggregation, including slots whose
    // NULL bit is set; retain that ordering so conversion errors are visible.
    for value in &mut result.values {
        *value = convert_result(value.clone())?;
    }
    Ok(result)
}

/// 时间列 GREATEST，并在聚合后转换结果。
pub fn greatest_time_by<T, C>(
    columns: &[NullableColumn<T>],
    convert_result: C,
) -> Result<NullableColumn<T>, CompareVecError>
where
    T: Clone + Ord,
    C: FnMut(T) -> Result<T, CompareVecError>,
{
    time_extremum_by(
        columns,
        |candidate, current| candidate > current,
        convert_result,
    )
}

/// 时间列 LEAST，并在聚合后转换结果。
pub fn least_time_by<T, C>(
    columns: &[NullableColumn<T>],
    convert_result: C,
) -> Result<NullableColumn<T>, CompareVecError>
where
    T: Clone + Ord,
    C: FnMut(T) -> Result<T, CompareVecError>,
{
    time_extremum_by(
        columns,
        |candidate, current| candidate < current,
        convert_result,
    )
}

/// 时长列 GREATEST。
pub fn greatest_duration<T>(
    columns: &[NullableColumn<T>],
) -> Result<NullableColumn<T>, CompareVecError>
where
    T: Clone + Ord,
{
    extremum_by(columns, |candidate, current| candidate > current)
}

/// 时长列 LEAST。
pub fn least_duration<T>(
    columns: &[NullableColumn<T>],
) -> Result<NullableColumn<T>, CompareVecError>
where
    T: Clone + Ord,
{
    extremum_by(columns, |candidate, current| candidate < current)
}

/// Go 文件中各签名均声明支持向量化执行。
/// All signatures implemented by the Go file advertise vectorized execution.
pub trait Vectorized {
    fn vectorized(&self) -> bool {
        true
    }
}

/// 为每个签名生成空结构体并实现 `Vectorized`。
macro_rules! vectorized_signatures {
    ($($signature:ident),+ $(,)?) => {
        $(
            #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
            pub struct $signature;

            impl Vectorized for $signature {}
        )+
    };
}

vectorized_signatures!(
    BuiltinGreatestDecimalSig,
    BuiltinLeastDecimalSig,
    BuiltinLeastIntSig,
    BuiltinGreatestIntSig,
    BuiltinGeIntSig,
    BuiltinLeastRealSig,
    BuiltinLeastStringSig,
    BuiltinEqIntSig,
    BuiltinNeIntSig,
    BuiltinGtIntSig,
    BuiltinNullEqIntSig,
    BuiltinIntervalIntSig,
    BuiltinIntervalRealSig,
    BuiltinLeIntSig,
    BuiltinLtIntSig,
    BuiltinGreatestCmpStringAsTimeSig,
    BuiltinGreatestRealSig,
    BuiltinLeastCmpStringAsTimeSig,
    BuiltinGreatestStringSig,
    BuiltinGreatestTimeSig,
    BuiltinLeastTimeSig,
    BuiltinGreatestDurationSig,
    BuiltinLeastDurationSig,
);

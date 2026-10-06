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

// 缓冲式窗口函数执行器与窗口原语。
//
// 提供 Value/Chunk、帧边界（ROWS/RANGE）、各类窗口函数（ROW_NUMBER、LAG、
// SUM/AVG/MIN/MAX 等）、分区 GroupChecker，以及按整分区缓冲后计算的 `WindowExec`。
// 执行计划（physical plan）中的窗口节点在此落地为可迭代的 Chunk 流。

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::fmt;
use std::mem::size_of;
use std::sync::atomic::{AtomicI64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Eq)]
/// 定点十进制：系数 + 小数位数（scale）。
pub struct Decimal {
    coefficient: i128,
    scale: u32,
}

impl Decimal {
    /// 构造 Decimal。
    pub const fn new(coefficient: i128, scale: u32) -> Self {
        Self { coefficient, scale }
    }

    /// 返回未缩放系数。
    pub const fn coefficient(self) -> i128 {
        self.coefficient
    }

    /// 返回小数位数。
    pub const fn scale(self) -> u32 {
        self.scale
    }

    /// 去掉尾随零，得到规范表示。
    fn normalized(mut self) -> Self {
        while self.scale > 0 && self.coefficient % 10 == 0 {
            self.coefficient /= 10;
            self.scale -= 1;
        }
        self
    }

    /// 计算 10^scale，溢出则报错。
    fn checked_pow10(scale: u32) -> Result<i128> {
        10_i128
            .checked_pow(scale)
            .ok_or_else(|| Error::new(format!("decimal scale {scale} overflows i128")))
    }

    /// 将系数对齐到更大 scale（禁止缩小以免舍入）。
    fn coefficient_at_scale(self, scale: u32) -> Result<i128> {
        if scale < self.scale {
            return Err(Error::new(format!(
                "cannot reduce decimal scale from {} to {scale} without rounding",
                self.scale
            )));
        }
        self.coefficient
            .checked_mul(Self::checked_pow10(scale - self.scale)?)
            .ok_or_else(|| Error::new("decimal coefficient overflows i128"))
    }

    /// 转为近似 f64（比较/AVG 浮点路径）。
    fn to_f64(self) -> f64 {
        self.coefficient as f64 / 10_f64.powi(self.scale as i32)
    }
}

/// 按规范化后的系数与 scale 比较相等。
impl PartialEq for Decimal {
    fn eq(&self, other: &Self) -> bool {
        self.normalized().coefficient == other.normalized().coefficient
            && self.normalized().scale == other.normalized().scale
    }
}

#[derive(Clone, Debug, PartialEq)]
/// 窗口计算用的简化 SQL 值类型。
pub enum Value {
    Null,
    Int(i64),
    UInt(u64),
    Real(f64),
    Decimal(Decimal),
    Text(String),
    Bool(bool),
}

impl Value {
    fn heap_memory_usage(&self) -> i64 {
        match self {
            Self::Text(value) => value.capacity() as i64,
            _ => 0,
        }
    }

    /// 提取数值近似；非数值返回 None。
    fn numeric(&self) -> Option<f64> {
        match self {
            Self::Int(value) => Some(*value as f64),
            Self::UInt(value) => Some(*value as f64),
            Self::Real(value) => Some(*value),
            Self::Decimal(value) => Some(value.to_f64()),
            _ => None,
        }
    }

    /// Compare numeric values without routing exact integer/decimal pairs through `f64`.
    fn numeric_cmp(&self, other: &Self) -> Option<Ordering> {
        match (self, other) {
            (Self::Int(left), Self::Int(right)) => Some(left.cmp(right)),
            (Self::UInt(left), Self::UInt(right)) => Some(left.cmp(right)),
            (Self::Int(left), Self::UInt(right)) => Some(if *left < 0 {
                Ordering::Less
            } else {
                (*left as u64).cmp(right)
            }),
            (Self::UInt(left), Self::Int(right)) => Some(if *right < 0 {
                Ordering::Greater
            } else {
                left.cmp(&(*right as u64))
            }),
            (Self::Decimal(left), Self::Decimal(right)) => {
                let scale = left.scale.max(right.scale);
                match (
                    left.coefficient_at_scale(scale),
                    right.coefficient_at_scale(scale),
                ) {
                    (Ok(left), Ok(right)) => Some(left.cmp(&right)),
                    _ => left.to_f64().partial_cmp(&right.to_f64()),
                }
            }
            (left, right) => left
                .numeric()
                .zip(right.numeric())
                .and_then(|(left, right)| left.partial_cmp(&right)),
        }
    }

    /// SQL 风格比较：NULL 最小，数值按大小，文本/布尔字典序。
    fn sql_cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::Null, _) => Ordering::Less,
            (_, Self::Null) => Ordering::Greater,
            (left, right) if left.numeric().is_some() && right.numeric().is_some() => {
                left.numeric_cmp(right).unwrap_or(Ordering::Equal)
            }
            (Self::Text(left), Self::Text(right)) => left.cmp(right),
            (Self::Bool(left), Self::Bool(right)) => left.cmp(right),
            (left, right) => format!("{left:?}").cmp(&format!("{right:?}")),
        }
    }
}

/// 一行：列值向量。
pub type Row = Vec<Value>;

#[derive(Clone, Debug, Default, PartialEq)]
/// 列式批处理块的行列表简化表示。
pub struct Chunk {
    pub rows: Vec<Row>,
}

impl Chunk {
    /// 由行列表构造 Chunk。
    pub fn new(rows: Vec<Row>) -> Self {
        Self { rows }
    }

    /// 行数。
    pub fn num_rows(&self) -> usize {
        self.rows.len()
    }

    /// 清空所有行。
    pub fn reset(&mut self) {
        self.rows.clear();
    }

    /// 与另一 Chunk 交换行缓冲（零拷贝移交）。
    pub fn swap_columns(&mut self, other: &mut Self) {
        std::mem::swap(&mut self.rows, &mut other.rows);
    }

    /// 投影前 `columns` 列，得到新 Chunk。
    pub fn projected(&self, columns: usize) -> Self {
        Self::new(
            self.rows
                .iter()
                .map(|row| row.iter().take(columns).cloned().collect())
                .collect(),
        )
    }

    /// 向指定行追加窗口函数结果列。
    pub fn append_results(&mut self, row: usize, values: Vec<Value>) -> Result<()> {
        let output = self
            .rows
            .get_mut(row)
            .ok_or_else(|| Error::new(format!("result row {row} is out of range")))?;
        output.extend(values);
        Ok(())
    }

    /// Return the bytes owned by this chunk, including nested row/value buffers.
    pub fn memory_usage(&self) -> i64 {
        size_of::<Self>() as i64
            + (self.rows.capacity() * size_of::<Row>()) as i64
            + self
                .rows
                .iter()
                .map(|row| {
                    (row.capacity() * size_of::<Value>()) as i64
                        + row.iter().map(Value::heap_memory_usage).sum::<i64>()
                })
                .sum::<i64>()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 窗口执行错误。
pub struct Error(String);

impl Error {
    /// 由消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}
/// 本模块 Result 别名。
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Default)]
/// A composable byte counter used by executor-local and statement-level tracking.
pub struct MemoryTracker(Arc<AtomicI64>);

impl MemoryTracker {
    pub fn consume(&self, bytes: i64) {
        self.0.fetch_add(bytes, AtomicOrdering::Relaxed);
    }

    pub fn bytes_consumed(&self) -> i64 {
        self.0.load(AtomicOrdering::Relaxed)
    }
}

#[derive(Clone, Debug, Default)]
/// 执行上下文，包含语句级内存跟踪器。
pub struct ExecContext {
    pub statement_memory_tracker: MemoryTracker,
}

#[derive(Debug)]
struct WindowMemoryState {
    local: MemoryTracker,
    parent: Option<MemoryTracker>,
    initial_partial_result_memory: i64,
    partial_result_memory: Vec<i64>,
}

#[derive(Clone, Debug)]
pub struct WindowMemoryTracker(Arc<Mutex<WindowMemoryState>>);

impl WindowMemoryTracker {
    pub fn new(initial_partial_result_memory: Vec<i64>) -> Self {
        let function_count = initial_partial_result_memory.len();
        Self(Arc::new(Mutex::new(WindowMemoryState {
            local: MemoryTracker::default(),
            parent: None,
            initial_partial_result_memory: initial_partial_result_memory.iter().sum(),
            partial_result_memory: vec![0; function_count],
        })))
    }

    pub fn open(&self, statement: &MemoryTracker) {
        self.close();
        let initial = {
            let mut state = self.0.lock().unwrap();
            state.parent = Some(statement.clone());
            state.partial_result_memory.fill(0);
            state.initial_partial_result_memory
        };
        self.consume(initial);
    }

    pub fn consume(&self, bytes: i64) {
        if bytes == 0 {
            return;
        }
        let state = self.0.lock().unwrap();
        state.local.consume(bytes);
        if let Some(parent) = &state.parent {
            parent.consume(bytes);
        }
    }

    pub fn update_partial_result(&self, index: usize, bytes: i64) {
        let delta = {
            let mut state = self.0.lock().unwrap();
            let delta = bytes - state.partial_result_memory[index];
            state.partial_result_memory[index] = bytes;
            delta
        };
        self.consume(delta);
    }

    pub fn release_partial_result(&self, index: usize) {
        self.update_partial_result(index, 0);
    }

    pub fn bytes_consumed(&self) -> i64 {
        self.0.lock().unwrap().local.bytes_consumed()
    }

    pub fn close(&self) {
        let (bytes, parent) = {
            let mut state = self.0.lock().unwrap();
            let bytes = state.local.bytes_consumed();
            state.local.consume(-bytes);
            state.partial_result_memory.fill(0);
            (bytes, state.parent.take())
        };
        if let Some(parent) = parent {
            parent.consume(-bytes);
        }
    }
}

/// 子执行器：提供已排序/分区的输入 Chunk 流。
pub trait ChildExecutor: Send {
    fn open(&mut self, _context: &ExecContext) -> Result<()> {
        Ok(())
    }
    fn next(&mut self, context: &ExecContext) -> Result<Option<Chunk>>;
    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}

/// 测试用：预置 Chunk 队列的子执行器。
pub struct VecChunkExecutor {
    chunks: VecDeque<Chunk>,
}

impl VecChunkExecutor {
    /// 用 Chunk 列表构造。
    pub fn new(chunks: Vec<Chunk>) -> Self {
        Self {
            chunks: chunks.into(),
        }
    }
}

impl ChildExecutor for VecChunkExecutor {
    fn next(&mut self, _context: &ExecContext) -> Result<Option<Chunk>> {
        Ok(self.chunks.pop_front())
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 帧边界类型：向前/当前行/向后。
pub enum BoundType {
    /// 当前行之前 N 行/值。
    Preceding,
    #[default]
    /// 当前行。
    CurrentRow,
    Following,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 帧模式：按行偏移（ROWS）或按值域（RANGE）。
pub enum FrameType {
    Rows,
    #[default]
    Range,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// ORDER BY 单项：列下标与是否降序。
pub struct OrderBy {
    pub column: usize,
    pub descending: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 窗口帧的一端边界定义。
pub struct FrameBound {
    pub bound_type: BoundType,
    pub unbounded: bool,
    pub num: u64,
    compare_columns: Vec<usize>,
}

impl FrameBound {
    /// 构造 PRECEDING n。
    pub fn preceding(num: u64) -> Self {
        Self {
            bound_type: BoundType::Preceding,
            num,
            ..Self::default()
        }
    }

    /// 构造 FOLLOWING n。
    pub fn following(num: u64) -> Self {
        Self {
            bound_type: BoundType::Following,
            num,
            ..Self::default()
        }
    }

    /// 构造无界边界。
    pub fn unbounded(bound_type: BoundType) -> Self {
        Self {
            bound_type,
            unbounded: true,
            ..Self::default()
        }
    }

    /// RANGE 帧绑定 ORDER BY 比较列。
    pub fn update_compare_cols(&mut self, order_by: &[OrderBy]) -> Result<()> {
        if order_by.is_empty() {
            return Err(Error::new("RANGE frame requires an ORDER BY column"));
        }
        self.compare_columns = order_by.iter().map(|item| item.column).collect();
        Ok(())
    }

    /// 按 PRECEDING/FOLLOWING 与升降序计算数值边界。
    fn boundary_number(&self, current: f64, descending: bool) -> f64 {
        let offset = self.num as f64;
        match (self.bound_type, descending) {
            (BoundType::Preceding, false) | (BoundType::Following, true) => current - offset,
            (BoundType::Following, false) | (BoundType::Preceding, true) => current + offset,
            (BoundType::CurrentRow, _) => current,
        }
    }

    /// 将候选行与当前行在边界值上比较。
    fn candidate_cmp_boundary(
        &self,
        candidate: &Row,
        current: &Row,
        order_by: &[OrderBy],
    ) -> Result<Ordering> {
        for (index, item) in order_by.iter().enumerate() {
            let column = *self.compare_columns.get(index).unwrap_or(&item.column);
            let candidate_value = candidate
                .get(column)
                .ok_or_else(|| Error::new(format!("ORDER BY column {column} is out of range")))?;
            let current_value = current
                .get(column)
                .ok_or_else(|| Error::new(format!("ORDER BY column {column} is out of range")))?;
            let ordering =
                if index == 0 && self.bound_type != BoundType::CurrentRow && self.num != 0 {
                    match (candidate_value.numeric(), current_value.numeric()) {
                        (Some(candidate), Some(current)) => candidate
                            .partial_cmp(&self.boundary_number(current, item.descending))
                            .unwrap_or(Ordering::Equal),
                        _ => candidate_value.sql_cmp(current_value),
                    }
                } else {
                    candidate_value.sql_cmp(current_value)
                };
            if ordering != Ordering::Equal {
                return Ok(ordering);
            }
        }
        Ok(Ordering::Equal)
    }

    /// 候选行是否仍在帧起点之前（应继续推进 start）。
    pub fn before_start(
        &self,
        candidate: &Row,
        current: &Row,
        order_by: &[OrderBy],
    ) -> Result<bool> {
        let ordering = self.candidate_cmp_boundary(candidate, current, order_by)?;
        Ok(if order_by.first().is_some_and(|item| item.descending) {
            ordering == Ordering::Greater
        } else {
            ordering == Ordering::Less
        })
    }

    /// 候选行是否已越过帧终点。
    pub fn beyond_end(&self, candidate: &Row, current: &Row, order_by: &[OrderBy]) -> Result<bool> {
        let ordering = self.candidate_cmp_boundary(candidate, current, order_by)?;
        Ok(if order_by.first().is_some_and(|item| item.descending) {
            ordering == Ordering::Less
        } else {
            ordering == Ordering::Greater
        })
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 完整窗口帧：类型 + 起止边界。
pub struct WindowFrame {
    pub frame_type: FrameType,
    pub start: FrameBound,
    pub end: FrameBound,
}

/// 窗口函数接口：reset/update/result，可选滑动 slide。
pub trait WindowFunction: Send {
    fn reset(&mut self);
    fn update(&mut self, rows: &[Row]) -> Result<()>;
    fn result(&mut self) -> Result<Value>;
    fn slide(&mut self, _rows: &[Row], _start: u64, _end: u64) -> Result<bool> {
        Ok(false)
    }
    fn set_window_start(&mut self, _start: u64) {}
    /// Whether the function ignores the frame and evaluates over the partition.
    fn ignores_frame(&self) -> bool {
        false
    }
    /// Fixed bytes allocated with the partial result.
    fn initial_partial_result_memory_usage(&self) -> i64 {
        size_of::<usize>() as i64
    }
    /// Additional bytes retained while updating the partial result.
    fn partial_result_memory_usage(&self) -> i64 {
        0
    }
}

#[derive(Default)]
/// ROW_NUMBER()：分区内从 1 递增的行号。
pub struct RowNumber {
    current: u64,
}

impl WindowFunction for RowNumber {
    fn reset(&mut self) {
        self.current = 0;
    }

    fn update(&mut self, _rows: &[Row]) -> Result<()> {
        Ok(())
    }

    fn result(&mut self) -> Result<Value> {
        self.current += 1;
        Ok(Value::UInt(self.current))
    }

    fn slide(&mut self, _rows: &[Row], _start: u64, _end: u64) -> Result<bool> {
        Ok(true)
    }

    fn ignores_frame(&self) -> bool {
        true
    }
}

/// LAG(col, offset, default)：取向前 offset 行的列值。
pub struct Lag {
    column: usize,
    offset: usize,
    default: Value,
    rows: Vec<Row>,
    current: usize,
}

impl Lag {
    /// 构造 LAG。
    pub fn new(column: usize, offset: usize, default: Value) -> Self {
        Self {
            column,
            offset,
            default,
            rows: Vec::new(),
            current: 0,
        }
    }
}

impl WindowFunction for Lag {
    fn reset(&mut self) {
        self.rows.clear();
        self.current = 0;
    }

    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.rows = rows.to_vec();
        Ok(())
    }

    fn result(&mut self) -> Result<Value> {
        let result = if let Some(source) = self.current.checked_sub(self.offset) {
            self.rows
                .get(source)
                .and_then(|row| row.get(self.column))
                .cloned()
                .ok_or_else(|| {
                    Error::new(format!(
                        "LAG source row {source} or column {} is out of range",
                        self.column
                    ))
                })?
        } else {
            self.default.clone()
        };
        self.current += 1;
        Ok(result)
    }

    fn slide(&mut self, _rows: &[Row], _start: u64, _end: u64) -> Result<bool> {
        Ok(true)
    }

    fn partial_result_memory_usage(&self) -> i64 {
        (self.rows.capacity() * size_of::<Row>()) as i64
            + self
                .rows
                .iter()
                .map(|row| {
                    (row.capacity() * size_of::<Value>()) as i64
                        + row.iter().map(Value::heap_memory_usage).sum::<i64>()
                })
                .sum::<i64>()
    }

    fn initial_partial_result_memory_usage(&self) -> i64 {
        size_of::<Self>() as i64
    }
}

/// BIT_XOR 聚合：窗口内按位异或，支持滑动。
pub struct BitXor {
    column: usize,
    value: u64,
    last_start: u64,
    last_end: u64,
}

impl BitXor {
    /// 构造 BIT_XOR。
    pub fn new(column: usize) -> Self {
        Self {
            column,
            value: 0,
            last_start: 0,
            last_end: 0,
        }
    }

    fn row_value(&self, row: &Row) -> Result<Option<u64>> {
        match row.get(self.column) {
            Some(Value::Null) => Ok(None),
            Some(Value::Int(value)) => Ok(Some(*value as u64)),
            Some(Value::UInt(value)) => Ok(Some(*value)),
            Some(Value::Real(value)) => Ok(Some(*value as u64)),
            Some(value) => Err(Error::new(format!(
                "BIT_XOR cannot evaluate {value:?} at column {}",
                self.column
            ))),
            None => Err(Error::new(format!(
                "BIT_XOR column {} is out of range",
                self.column
            ))),
        }
    }
}

impl WindowFunction for BitXor {
    fn reset(&mut self) {
        self.value = 0;
    }

    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.value = 0;
        for row in rows {
            if let Some(value) = self.row_value(row)? {
                self.value ^= value;
            }
        }
        self.last_end = self.last_start + rows.len() as u64;
        Ok(())
    }

    fn result(&mut self) -> Result<Value> {
        Ok(Value::UInt(self.value))
    }

    /// 滑动：窗口后退则重建，否则异或移出/移入行。
    fn slide(&mut self, rows: &[Row], start: u64, end: u64) -> Result<bool> {
        if start < self.last_start || end < self.last_end {
            self.last_start = start;
            self.update(&rows[start as usize..end as usize])?;
            return Ok(true);
        }
        for row in &rows[self.last_start as usize..start as usize] {
            if let Some(value) = self.row_value(row)? {
                self.value ^= value;
            }
        }
        for row in &rows[self.last_end as usize..end as usize] {
            if let Some(value) = self.row_value(row)? {
                self.value ^= value;
            }
        }
        self.last_start = start;
        self.last_end = end;
        Ok(true)
    }

    fn set_window_start(&mut self, start: u64) {
        self.last_start = start;
        self.last_end = start;
    }
}

/// AVG 浮点聚合，支持滑动增删。
pub struct Average {
    column: usize,
    sum: f64,
    count: u64,
    last_start: u64,
    last_end: u64,
}

impl Average {
    /// 指定参与计算的输入列下标。
    pub fn new(column: usize) -> Self {
        Self {
            column,
            sum: 0.0,
            count: 0,
            last_start: 0,
            last_end: 0,
        }
    }

    fn row_value(&self, row: &Row) -> Result<Option<f64>> {
        row.get(self.column)
            .map(Value::numeric)
            .ok_or_else(|| Error::new(format!("AVG column {} is out of range", self.column)))
    }

    fn add_rows(&mut self, rows: &[Row]) -> Result<()> {
        for row in rows {
            if let Some(value) = self.row_value(row)? {
                self.sum += value;
                self.count += 1;
            }
        }
        Ok(())
    }

    fn remove_rows(&mut self, rows: &[Row]) -> Result<()> {
        for row in rows {
            if let Some(value) = self.row_value(row)? {
                self.sum -= value;
                self.count = self
                    .count
                    .checked_sub(1)
                    .ok_or_else(|| Error::new("AVG sliding count underflow"))?;
            }
        }
        Ok(())
    }
}

impl WindowFunction for Average {
    fn reset(&mut self) {
        self.sum = 0.0;
        self.count = 0;
    }

    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.reset();
        self.add_rows(rows)?;
        self.last_end = self.last_start + rows.len() as u64;
        Ok(())
    }

    fn result(&mut self) -> Result<Value> {
        Ok(if self.count == 0 {
            Value::Null
        } else {
            Value::Real(self.sum / self.count as f64)
        })
    }

    /// 滑动：增减帧两端行以更新 sum/count。
    fn slide(&mut self, rows: &[Row], start: u64, end: u64) -> Result<bool> {
        if start < self.last_start || end < self.last_end {
            self.last_start = start;
            self.update(&rows[start as usize..end as usize])?;
            return Ok(true);
        }
        self.remove_rows(&rows[self.last_start as usize..start as usize])?;
        self.add_rows(&rows[self.last_end as usize..end as usize])?;
        self.last_start = start;
        self.last_end = end;
        Ok(true)
    }

    fn set_window_start(&mut self, start: u64) {
        self.last_start = start;
        self.last_end = start;
    }
}

/// VAR_SAMP 聚合：返回帧内非 NULL 数值的样本方差。
pub struct VarSamp {
    column: usize,
    values: Vec<f64>,
}

impl VarSamp {
    /// 指定参与计算的输入列下标。
    pub fn new(column: usize) -> Self {
        Self {
            column,
            values: Vec::new(),
        }
    }

    fn add_rows(&mut self, rows: &[Row]) -> Result<()> {
        for row in rows {
            let value = row.get(self.column).ok_or_else(|| {
                Error::new(format!("VAR_SAMP column {} is out of range", self.column))
            })?;
            if let Some(number) = value.numeric() {
                self.values.push(number);
            }
        }
        Ok(())
    }
}

impl WindowFunction for VarSamp {
    fn reset(&mut self) {
        self.values.clear();
    }

    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.reset();
        self.add_rows(rows)
    }

    fn result(&mut self) -> Result<Value> {
        if self.values.len() < 2 {
            return Ok(Value::Null);
        }
        let mean = self.values.iter().sum::<f64>() / self.values.len() as f64;
        let squared_deviation = self
            .values
            .iter()
            .map(|value| (value - mean) * (value - mean))
            .sum::<f64>();
        Ok(Value::Real(
            squared_deviation / (self.values.len() - 1) as f64,
        ))
    }

    fn slide(&mut self, rows: &[Row], start: u64, end: u64) -> Result<bool> {
        self.update(&rows[start as usize..end as usize])?;
        Ok(true)
    }

    fn partial_result_memory_usage(&self) -> i64 {
        (self.values.capacity() * size_of::<f64>()) as i64
    }

    fn initial_partial_result_memory_usage(&self) -> i64 {
        size_of::<Self>() as i64
    }
}

/// DECIMAL AVG：精确系数累加后相除。
pub struct DecimalAverage {
    column: usize,
    coefficient: i128,
    scale: u32,
    count: u64,
    last_start: u64,
    last_end: u64,
}

impl DecimalAverage {
    /// 指定参与计算的输入列下标。
    pub fn new(column: usize) -> Self {
        Self {
            column,
            coefficient: 0,
            scale: 0,
            count: 0,
            last_start: 0,
            last_end: 0,
        }
    }

    fn row_value(&self, row: &Row) -> Result<Option<Decimal>> {
        match row.get(self.column) {
            Some(Value::Null) => Ok(None),
            Some(Value::Decimal(value)) => Ok(Some(*value)),
            Some(value) => Err(Error::new(format!(
                "DECIMAL AVG cannot evaluate {value:?} at column {}",
                self.column
            ))),
            None => Err(Error::new(format!(
                "DECIMAL AVG column {} is out of range",
                self.column
            ))),
        }
    }

    fn align_scale(&mut self, scale: u32) -> Result<()> {
        if scale > self.scale {
            self.coefficient =
                Decimal::new(self.coefficient, self.scale).coefficient_at_scale(scale)?;
            self.scale = scale;
        }
        Ok(())
    }

    fn add_rows(&mut self, rows: &[Row]) -> Result<()> {
        for row in rows {
            if let Some(value) = self.row_value(row)? {
                self.align_scale(value.scale())?;
                self.coefficient = self
                    .coefficient
                    .checked_add(value.coefficient_at_scale(self.scale)?)
                    .ok_or_else(|| Error::new("DECIMAL AVG sum overflows i128"))?;
                self.count += 1;
            }
        }
        Ok(())
    }

    fn remove_rows(&mut self, rows: &[Row]) -> Result<()> {
        for row in rows {
            if let Some(value) = self.row_value(row)? {
                self.align_scale(value.scale())?;
                self.coefficient = self
                    .coefficient
                    .checked_sub(value.coefficient_at_scale(self.scale)?)
                    .ok_or_else(|| Error::new("DECIMAL AVG sum overflows i128"))?;
                self.count = self
                    .count
                    .checked_sub(1)
                    .ok_or_else(|| Error::new("DECIMAL AVG sliding count underflow"))?;
            }
        }
        Ok(())
    }

    fn average(&self) -> Result<Decimal> {
        let divisor = self.count as i128;
        if self.coefficient % divisor == 0 {
            return Ok(Decimal::new(self.coefficient / divisor, self.scale));
        }
        let extra_scale = 12;
        let scaled = self
            .coefficient
            .checked_mul(Decimal::checked_pow10(extra_scale)?)
            .ok_or_else(|| Error::new("DECIMAL AVG division overflows i128"))?;
        Ok(Decimal::new(scaled / divisor, self.scale + extra_scale))
    }
}

impl WindowFunction for DecimalAverage {
    fn reset(&mut self) {
        self.coefficient = 0;
        self.scale = 0;
        self.count = 0;
    }

    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.reset();
        self.add_rows(rows)?;
        self.last_end = self.last_start + rows.len() as u64;
        Ok(())
    }

    fn result(&mut self) -> Result<Value> {
        Ok(if self.count == 0 {
            Value::Null
        } else {
            Value::Decimal(self.average()?)
        })
    }

    fn slide(&mut self, rows: &[Row], start: u64, end: u64) -> Result<bool> {
        if start < self.last_start || end < self.last_end {
            self.last_start = start;
            self.update(&rows[start as usize..end as usize])?;
            return Ok(true);
        }
        self.remove_rows(&rows[self.last_start as usize..start as usize])?;
        self.add_rows(&rows[self.last_end as usize..end as usize])?;
        self.last_start = start;
        self.last_end = end;
        Ok(true)
    }

    fn set_window_start(&mut self, start: u64) {
        self.last_start = start;
        self.last_end = start;
    }
}

/// DECIMAL SUM：复用 DecimalAverage 的累加状态。
pub struct DecimalSum {
    state: DecimalAverage,
}

impl DecimalSum {
    /// 指定参与计算的输入列下标。
    pub fn new(column: usize) -> Self {
        Self {
            state: DecimalAverage::new(column),
        }
    }
}

impl WindowFunction for DecimalSum {
    fn reset(&mut self) {
        self.state.reset();
    }

    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.state.update(rows)
    }

    fn result(&mut self) -> Result<Value> {
        Ok(if self.state.count == 0 {
            Value::Null
        } else {
            Value::Decimal(Decimal::new(self.state.coefficient, self.state.scale))
        })
    }

    fn slide(&mut self, rows: &[Row], start: u64, end: u64) -> Result<bool> {
        self.state.slide(rows, start, end)
    }

    fn set_window_start(&mut self, start: u64) {
        self.state.set_window_start(start);
    }
}

/// MIN/MAX 共享状态：维护帧内值多重集合。
struct ExtremeState {
    column: usize,
    values: Vec<Value>,
    last_start: u64,
    last_end: u64,
}

impl ExtremeState {
    fn new(column: usize) -> Self {
        Self {
            column,
            values: Vec::new(),
            last_start: 0,
            last_end: 0,
        }
    }

    fn row_value(&self, row: &Row) -> Result<Option<Value>> {
        match row.get(self.column) {
            Some(Value::Null) => Ok(None),
            Some(value) => Ok(Some(value.clone())),
            None => Err(Error::new(format!(
                "window aggregate column {} is out of range",
                self.column
            ))),
        }
    }

    fn reset(&mut self) {
        self.values.clear();
    }

    fn add_rows(&mut self, rows: &[Row]) -> Result<()> {
        for row in rows {
            if let Some(value) = self.row_value(row)? {
                self.values.push(value);
            }
        }
        Ok(())
    }

    /// 从状态多重集合中移除移出帧的值。
    fn remove_rows(&mut self, rows: &[Row]) -> Result<()> {
        for row in rows {
            let Some(value) = self.row_value(row)? else {
                continue;
            };
            let index = self
                .values
                .iter()
                .position(|candidate| candidate.sql_cmp(&value) == Ordering::Equal)
                .ok_or_else(|| {
                    Error::new("sliding window value is missing from aggregate state")
                })?;
            self.values.remove(index);
        }
        Ok(())
    }

    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.reset();
        self.add_rows(rows)?;
        self.last_end = self.last_start + rows.len() as u64;
        Ok(())
    }

    fn slide(&mut self, rows: &[Row], start: u64, end: u64) -> Result<()> {
        if start < self.last_start || end < self.last_end {
            self.last_start = start;
            return self.update(&rows[start as usize..end as usize]);
        }
        self.remove_rows(&rows[self.last_start as usize..start as usize])?;
        self.add_rows(&rows[self.last_end as usize..end as usize])?;
        self.last_start = start;
        self.last_end = end;
        Ok(())
    }
}

/// MAX 窗口聚合。
pub struct MaxValue {
    state: ExtremeState,
}

impl MaxValue {
    /// 指定参与计算的输入列下标。
    pub fn new(column: usize) -> Self {
        Self {
            state: ExtremeState::new(column),
        }
    }
}

impl WindowFunction for MaxValue {
    fn reset(&mut self) {
        self.state.reset();
    }

    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.state.update(rows)
    }

    fn result(&mut self) -> Result<Value> {
        Ok(self
            .state
            .values
            .iter()
            .max_by(|left, right| left.sql_cmp(right))
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn slide(&mut self, rows: &[Row], start: u64, end: u64) -> Result<bool> {
        self.state.slide(rows, start, end)?;
        Ok(true)
    }

    fn set_window_start(&mut self, start: u64) {
        self.state.last_start = start;
        self.state.last_end = start;
    }

    fn partial_result_memory_usage(&self) -> i64 {
        (self.state.values.capacity() * size_of::<Value>()) as i64
            + self
                .state
                .values
                .iter()
                .map(Value::heap_memory_usage)
                .sum::<i64>()
    }

    fn initial_partial_result_memory_usage(&self) -> i64 {
        size_of::<Self>() as i64
    }
}

/// MIN 窗口聚合。
pub struct MinValue {
    state: ExtremeState,
}

impl MinValue {
    /// 指定参与计算的输入列下标。
    pub fn new(column: usize) -> Self {
        Self {
            state: ExtremeState::new(column),
        }
    }
}

impl WindowFunction for MinValue {
    fn reset(&mut self) {
        self.state.reset();
    }

    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.state.update(rows)
    }

    fn result(&mut self) -> Result<Value> {
        Ok(self
            .state
            .values
            .iter()
            .min_by(|left, right| left.sql_cmp(right))
            .cloned()
            .unwrap_or(Value::Null))
    }

    fn slide(&mut self, rows: &[Row], start: u64, end: u64) -> Result<bool> {
        self.state.slide(rows, start, end)?;
        Ok(true)
    }

    fn set_window_start(&mut self, start: u64) {
        self.state.last_start = start;
        self.state.last_end = start;
    }

    fn partial_result_memory_usage(&self) -> i64 {
        (self.state.values.capacity() * size_of::<Value>()) as i64
            + self
                .state
                .values
                .iter()
                .map(Value::heap_memory_usage)
                .sum::<i64>()
    }

    fn initial_partial_result_memory_usage(&self) -> i64 {
        size_of::<Self>() as i64
    }
}

#[derive(Default)]
/// COUNT(*)：帧内行数。
pub struct CountRows {
    count: u64,
}

impl WindowFunction for CountRows {
    fn reset(&mut self) {
        self.count = 0;
    }
    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.count = rows.len() as u64;
        Ok(())
    }
    fn result(&mut self) -> Result<Value> {
        Ok(Value::UInt(self.count))
    }
    fn slide(&mut self, rows: &[Row], start: u64, end: u64) -> Result<bool> {
        self.update(&rows[start as usize..end as usize])?;
        Ok(true)
    }
}

/// SUM 浮点聚合。
pub struct Sum {
    pub column: usize,
    value: f64,
    has_value: bool,
}

impl Sum {
    /// 指定参与计算的输入列下标。
    pub fn new(column: usize) -> Self {
        Self {
            column,
            value: 0.0,
            has_value: false,
        }
    }
}

impl WindowFunction for Sum {
    fn reset(&mut self) {
        self.value = 0.0;
        self.has_value = false;
    }
    fn update(&mut self, rows: &[Row]) -> Result<()> {
        self.reset();
        for row in rows {
            let value = row
                .get(self.column)
                .ok_or_else(|| Error::new(format!("SUM column {} is out of range", self.column)))?;
            if let Some(number) = value.numeric() {
                self.value += number;
                self.has_value = true;
            }
        }
        Ok(())
    }
    fn result(&mut self) -> Result<Value> {
        Ok(if self.has_value {
            Value::Real(self.value)
        } else {
            Value::Null
        })
    }
    fn slide(&mut self, rows: &[Row], start: u64, end: u64) -> Result<bool> {
        self.update(&rows[start as usize..end as usize])?;
        Ok(true)
    }
}

/// 按 PARTITION BY 列切分 Chunk 内连续分组。
pub struct GroupChecker {
    columns: Vec<usize>,
    groups: Vec<(usize, usize)>,
    next_group: usize,
    previous_last_key: Option<Row>,
}

impl GroupChecker {
    /// 用分区列下标构造。
    pub fn new(columns: Vec<usize>) -> Self {
        Self {
            columns,
            groups: Vec::new(),
            next_group: 0,
            previous_last_key: None,
        }
    }

    /// 提取分区键。
    fn key(&self, row: &Row) -> Result<Row> {
        self.columns
            .iter()
            .map(|column| {
                row.get(*column)
                    .cloned()
                    .ok_or_else(|| Error::new(format!("partition column {column} is out of range")))
            })
            .collect()
    }

    /// 将 Chunk 切成组；返回首组是否承接上一 Chunk 尾组。
    pub fn split_into_groups(&mut self, chunk: &Chunk) -> Result<bool> {
        if chunk.rows.is_empty() {
            return Err(Error::new("group checker requires a non-empty chunk"));
        }
        let first_key = self.key(&chunk.rows[0])?;
        let same_as_previous = self.previous_last_key.as_ref() == Some(&first_key);
        self.groups.clear();
        self.next_group = 0;
        let mut begin = 0;
        let mut previous = first_key;
        for index in 1..chunk.rows.len() {
            let key = self.key(&chunk.rows[index])?;
            if key != previous {
                self.groups.push((begin, index));
                begin = index;
                previous = key;
            }
        }
        self.groups.push((begin, chunk.rows.len()));
        self.previous_last_key = Some(previous);
        Ok(same_as_previous)
    }

    /// 取出下一组 `[begin, end)`。
    pub fn next_group(&mut self) -> Result<(usize, usize)> {
        let group = self
            .groups
            .get(self.next_group)
            .copied()
            .ok_or_else(|| Error::new("group checker is exhausted"))?;
        self.next_group += 1;
        Ok(group)
    }

    /// 当前 Chunk 内分组是否已取完。
    pub fn is_exhausted(&self) -> bool {
        self.next_group >= self.groups.len()
    }

    pub fn reset(&mut self) {
        self.groups.clear();
        self.next_group = 0;
        self.previous_last_key = None;
    }
}

/// 分区行消费与结果追加的处理器抽象。
pub trait WindowProcessor: Send {
    fn consume_group_rows(&mut self, rows: Vec<Row>) -> Result<Vec<Row>>;
    fn append_result(&mut self, rows: &[Row], remained: usize) -> Result<Vec<Vec<Value>>>;
    fn reset_partial_result(&mut self);
}

pub(crate) fn update_partial_result_and_track_memory(
    tracker: &WindowMemoryTracker,
    index: usize,
    function: &mut dyn WindowFunction,
    rows: &[Row],
) -> Result<()> {
    let result = function.update(rows);
    tracker.update_partial_result(index, function.partial_result_memory_usage());
    result
}

pub(crate) fn reset_partial_result_and_release_memory(
    tracker: &WindowMemoryTracker,
    index: usize,
    function: &mut dyn WindowFunction,
) {
    function.reset();
    tracker.release_partial_result(index);
}

pub(crate) fn reset_partial_results_and_release_memory(
    tracker: &WindowMemoryTracker,
    functions: &mut [Box<dyn WindowFunction>],
) {
    for (index, function) in functions.iter_mut().enumerate() {
        reset_partial_result_and_release_memory(tracker, index, function.as_mut());
    }
}

/// 整分区聚合处理器（无显式帧）。
pub struct AggWindowProcessor {
    pub window_functions: Vec<Box<dyn WindowFunction>>,
    pub memory_tracker: WindowMemoryTracker,
}

impl WindowProcessor for AggWindowProcessor {
    /// 用整分区行更新聚合状态；不保留行缓冲。
    fn consume_group_rows(&mut self, rows: Vec<Row>) -> Result<Vec<Row>> {
        if !rows.is_empty() {
            for (index, function) in self.window_functions.iter_mut().enumerate() {
                update_partial_result_and_track_memory(
                    &self.memory_tracker,
                    index,
                    function.as_mut(),
                    &rows,
                )?;
            }
        }
        Ok(Vec::new())
    }

    fn append_result(&mut self, _rows: &[Row], remained: usize) -> Result<Vec<Vec<Value>>> {
        (0..remained)
            .map(|_| {
                self.window_functions
                    .iter_mut()
                    .map(|function| function.result())
                    .collect()
            })
            .collect()
    }

    fn reset_partial_result(&mut self) {
        reset_partial_results_and_release_memory(&self.memory_tracker, &mut self.window_functions);
    }
}

/// ROWS 帧处理器：按行偏移计算每行帧。
pub struct RowFrameWindowProcessor {
    pub window_functions: Vec<Box<dyn WindowFunction>>,
    pub start: FrameBound,
    pub end: FrameBound,
    pub current_row: u64,
    pub initialized_sliding_window: bool,
    pub memory_tracker: WindowMemoryTracker,
}

impl RowFrameWindowProcessor {
    /// 当前行 ROWS 帧起点。
    pub fn start_offset(&self, rows: u64) -> u64 {
        if self.start.unbounded {
            return 0;
        }
        match self.start.bound_type {
            BoundType::Preceding => self.current_row.saturating_sub(self.start.num),
            BoundType::Following => self.current_row.saturating_add(self.start.num).min(rows),
            BoundType::CurrentRow => self.current_row,
        }
    }

    /// 当前行 ROWS 帧终点（半开）。
    pub fn end_offset(&self, rows: u64) -> u64 {
        if self.end.unbounded {
            return rows;
        }
        match self.end.bound_type {
            BoundType::Preceding => {
                if self.current_row >= self.end.num {
                    self.current_row - self.end.num + 1
                } else {
                    0
                }
            }
            BoundType::Following => self
                .current_row
                .saturating_add(self.end.num)
                .saturating_add(1)
                .min(rows),
            BoundType::CurrentRow => self.current_row.saturating_add(1).min(rows),
        }
    }
}

/// 对多组 `[start,end)` 调用窗口函数（优先 slide）。
fn calculate_frames(
    functions: &mut [Box<dyn WindowFunction>],
    memory_tracker: &WindowMemoryTracker,
    rows: &[Row],
    frames: impl IntoIterator<Item = (u64, u64)>,
    initialized_sliding: &mut bool,
) -> Result<Vec<Vec<Value>>> {
    let mut output = Vec::new();
    for (start, end) in frames {
        // SQL permits syntactically valid frames whose start is after their end.
        // Represent those as an empty half-open range so sliding implementations
        // never receive an invalid slice while still advancing their state.
        let end = end.max(start);
        let frame = &rows[start as usize..end as usize];
        let mut values = Vec::with_capacity(functions.len());
        for (index, function) in functions.iter_mut().enumerate() {
            if function.ignores_frame() {
                values.push(function.result()?);
                continue;
            }
            let slid = if *initialized_sliding {
                function.slide(rows, start, end)?
            } else {
                false
            };
            if !slid {
                function.set_window_start(start);
                reset_partial_result_and_release_memory(memory_tracker, index, function.as_mut());
                update_partial_result_and_track_memory(
                    memory_tracker,
                    index,
                    function.as_mut(),
                    frame,
                )?;
            } else {
                memory_tracker.update_partial_result(index, function.partial_result_memory_usage());
            }
            values.push(function.result()?);
        }
        *initialized_sliding = true;
        output.push(values);
    }
    Ok(output)
}

impl WindowProcessor for RowFrameWindowProcessor {
    fn consume_group_rows(&mut self, rows: Vec<Row>) -> Result<Vec<Row>> {
        Ok(rows)
    }

    fn append_result(&mut self, rows: &[Row], remained: usize) -> Result<Vec<Vec<Value>>> {
        let row_count = rows.len() as u64;
        let frames = (0..remained)
            .map(|_| {
                let frame = (self.start_offset(row_count), self.end_offset(row_count));
                self.current_row += 1;
                frame
            })
            .collect::<Vec<_>>();
        calculate_frames(
            &mut self.window_functions,
            &self.memory_tracker,
            rows,
            frames,
            &mut self.initialized_sliding_window,
        )
    }

    fn reset_partial_result(&mut self) {
        self.current_row = 0;
        self.initialized_sliding_window = false;
        reset_partial_results_and_release_memory(&self.memory_tracker, &mut self.window_functions);
    }
}

/// RANGE 帧处理器：按 ORDER BY 值域推进边界。
pub struct RangeFrameWindowProcessor {
    pub window_functions: Vec<Box<dyn WindowFunction>>,
    pub start: FrameBound,
    pub end: FrameBound,
    pub current_row: u64,
    pub last_start_offset: u64,
    pub last_end_offset: u64,
    pub order_by: Vec<OrderBy>,
    pub initialized_sliding_window: bool,
    pub memory_tracker: WindowMemoryTracker,
}

impl RangeFrameWindowProcessor {
    /// 推进 RANGE 起点到不再 before_start。
    pub fn start_offset(&mut self, rows: &[Row]) -> Result<u64> {
        if self.start.unbounded {
            return Ok(0);
        }
        while self.last_start_offset < rows.len() as u64
            && self.start.before_start(
                &rows[self.last_start_offset as usize],
                &rows[self.current_row as usize],
                &self.order_by,
            )?
        {
            self.last_start_offset += 1;
        }
        Ok(self.last_start_offset)
    }

    /// 推进 RANGE 终点到 beyond_end。
    pub fn end_offset(&mut self, rows: &[Row]) -> Result<u64> {
        if self.end.unbounded {
            return Ok(rows.len() as u64);
        }
        while self.last_end_offset < rows.len() as u64
            && !self.end.beyond_end(
                &rows[self.last_end_offset as usize],
                &rows[self.current_row as usize],
                &self.order_by,
            )?
        {
            self.last_end_offset += 1;
        }
        Ok(self.last_end_offset)
    }
}

impl WindowProcessor for RangeFrameWindowProcessor {
    fn consume_group_rows(&mut self, rows: Vec<Row>) -> Result<Vec<Row>> {
        Ok(rows)
    }

    fn append_result(&mut self, rows: &[Row], remained: usize) -> Result<Vec<Vec<Value>>> {
        let mut frames = Vec::with_capacity(remained);
        for _ in 0..remained {
            let start = self.start_offset(rows)?;
            let end = self.end_offset(rows)?;
            self.current_row += 1;
            frames.push((start, end));
        }
        calculate_frames(
            &mut self.window_functions,
            &self.memory_tracker,
            rows,
            frames,
            &mut self.initialized_sliding_window,
        )
    }

    fn reset_partial_result(&mut self) {
        self.current_row = 0;
        self.last_start_offset = 0;
        self.last_end_offset = 0;
        self.initialized_sliding_window = false;
        reset_partial_results_and_release_memory(&self.memory_tracker, &mut self.window_functions);
    }
}

/// 缓冲式窗口执行器：攒齐分区再写回结果块。
pub struct WindowExec {
    /// 子执行器。
    pub child: Box<dyn ChildExecutor>,
    /// 分区切分组。
    pub group_checker: GroupChecker,
    /// 最近一次子计划块。
    pub child_result: Option<Chunk>,
    /// 是否已处理完所有输入。
    pub executed: bool,
    /// 待输出的结果块队列。
    pub result_chunks: VecDeque<Chunk>,
    /// 各结果块尚需填充的窗口结果行数。
    pub remaining_rows_in_chunk: VecDeque<usize>,
    /// 输入列数。
    pub input_columns: usize,
    /// 帧/聚合处理器。
    pub processor: Box<dyn WindowProcessor>,
    pub memory_tracker: WindowMemoryTracker,
    pub(crate) result_queue_memory: i64,
}

impl WindowExec {
    /// 复位状态并打开子执行器。
    pub fn open(&mut self, context: &ExecContext) -> Result<()> {
        self.memory_tracker.open(&context.statement_memory_tracker);
        self.executed = false;
        self.result_chunks.clear();
        self.remaining_rows_in_chunk.clear();
        self.child_result = None;
        self.group_checker.reset();
        self.child.open(context)
    }

    /// 关闭子执行器。
    pub fn close(&mut self) -> Result<()> {
        self.child_result = None;
        self.result_chunks.clear();
        self.remaining_rows_in_chunk.clear();
        self.group_checker.reset();
        self.result_queue_memory = 0;
        self.memory_tracker.close();
        self.child.close()
    }

    /// 消费分区直至有完整结果块可弹出。
    pub fn next(&mut self, context: &ExecContext, output: &mut Chunk) -> Result<()> {
        output.reset();
        // 边消费分区边等待可返回的结果 Chunk。
        while !self.executed && !self.prepared_chunk_available() {
            if let Err(error) = self.consume_one_group(context) {
                self.executed = true;
                return Err(error);
            }
        }
        if let Some(mut result) = self.result_chunks.pop_front() {
            self.memory_tracker.consume(-result.memory_usage());
            output.swap_columns(&mut result);
            self.remaining_rows_in_chunk.pop_front();
            if self.result_chunks.is_empty() {
                self.memory_tracker.consume(-self.result_queue_memory);
                self.result_queue_memory = 0;
            }
        }
        Ok(())
    }

    /// 队首结果块是否已填满窗口列。
    pub fn prepared_chunk_available(&self) -> bool {
        !self.result_chunks.is_empty() && self.remaining_rows_in_chunk.front().copied() == Some(0)
    }

    /// 拉取并拼接跨 Chunk 的同一分区，交给处理器。
    pub fn consume_one_group(&mut self, context: &ExecContext) -> Result<()> {
        let mut group_rows = Vec::new();
        // 跨多个输入 Chunk 拼出完整分区（PARTITION BY 组）。
        if self.group_checker.is_exhausted() {
            if self.fetch_child(context)? {
                self.executed = true;
                return self.consume_tracked_group_rows(group_rows);
            }
            self.group_checker
                .split_into_groups(self.child_result.as_ref().unwrap())?;
        }
        let (mut begin, mut end) = self.group_checker.next_group()?;
        group_rows.extend_from_slice(&self.child_result.as_ref().unwrap().rows[begin..end]);
        let mut meets_last_group = end == self.child_result.as_ref().unwrap().num_rows();
        // 分区可能跨多个 Chunk：继续拉取直到键变化或耗尽。
        while meets_last_group {
            if self.fetch_child(context)? {
                self.executed = true;
                return self.consume_tracked_group_rows(group_rows);
            }
            let same = self
                .group_checker
                .split_into_groups(self.child_result.as_ref().unwrap())?;
            if !same {
                break;
            }
            (begin, end) = self.group_checker.next_group()?;
            group_rows.extend_from_slice(&self.child_result.as_ref().unwrap().rows[begin..end]);
            meets_last_group = end == self.child_result.as_ref().unwrap().num_rows();
        }
        self.consume_tracked_group_rows(group_rows)
    }

    fn consume_tracked_group_rows(&mut self, rows: Vec<Row>) -> Result<()> {
        let memory = rows_memory_usage(&rows);
        self.memory_tracker.consume(memory);
        let result = self.consume_group_rows(rows);
        self.memory_tracker.consume(-memory);
        result
    }

    /// 将分区行写入结果块的窗口列，并更新 remaining 计数。
    pub fn consume_group_rows(&mut self, mut rows: Vec<Row>) -> Result<()> {
        let mut remaining_group = rows.len();
        if remaining_group == 0 {
            return Ok(());
        }
        // 将本分区结果按各结果块剩余槽位依次填入。
        for index in 0..self.result_chunks.len() {
            let remaining_chunk = self.remaining_rows_in_chunk[index];
            let remained = remaining_chunk.min(remaining_group);
            self.remaining_rows_in_chunk[index] -= remained;
            remaining_group -= remained;
            let old_chunk_memory = self.result_chunks[index].memory_usage();
            rows = self.processor.consume_group_rows(rows)?;
            let values = self.processor.append_result(&rows, remained)?;
            let first_row = self.result_chunks[index].num_rows() - remaining_chunk;
            let append_result = values
                .into_iter()
                .enumerate()
                .try_for_each(|(offset, result)| {
                    self.result_chunks[index].append_results(first_row + offset, result)
                });
            self.memory_tracker
                .consume(self.result_chunks[index].memory_usage() - old_chunk_memory);
            append_result?;
            if remaining_group == 0 {
                self.processor.reset_partial_result();
                break;
            }
        }
        Ok(())
    }

    /// 拉子计划下一块；投影输入列并登记待填行数。
    pub fn fetch_child(&mut self, context: &ExecContext) -> Result<bool> {
        let Some(child) = self.child.next(context)? else {
            return Ok(true);
        };
        if child.rows.is_empty() {
            return Ok(true);
        }
        self.result_chunks
            // 新块先投影输入列，remaining 记待填窗口列行数。
            .push_back(child.projected(self.input_columns));
        self.remaining_rows_in_chunk.push_back(child.num_rows());
        let result = self.result_chunks.back().unwrap();
        self.memory_tracker.consume(result.memory_usage());
        let new_queue_memory = (self.result_chunks.capacity() * size_of::<Chunk>()) as i64
            + (self.remaining_rows_in_chunk.capacity() * size_of::<usize>()) as i64;
        self.memory_tracker
            .consume(new_queue_memory - self.result_queue_memory);
        self.result_queue_memory = new_queue_memory;
        self.child_result = Some(child);
        Ok(false)
    }

    pub fn memory_bytes(&self) -> i64 {
        self.memory_tracker.bytes_consumed()
    }
}

fn rows_memory_usage(rows: &[Row]) -> i64 {
    (rows.len() * size_of::<Row>()) as i64
        + rows
            .iter()
            .map(|row| {
                (row.capacity() * size_of::<Value>()) as i64
                    + row.iter().map(Value::heap_memory_usage).sum::<i64>()
            })
            .sum::<i64>()
}

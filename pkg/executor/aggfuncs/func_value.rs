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

// FIRST_VALUE / LAST_VALUE / NTH_VALUE 窗口函数求值器。
//
// 窗口函数按分区内行序取值，不改变结果集行数。本模块提供通用 `ValueEvaluator`
// 及三类窗口语义：首行、末行、第 N 行；并通过对变长类型的 `ValueMemory`
// 追踪求值器之外持有的内存增量（对齐 Go 内存记账）。

use crate::func_max_min::{BinaryJson, DurationValue, TimeValue, VectorFloat32};
use crate::func_sum::Decimal;
use std::mem::size_of;

/// Reports memory retained outside the evaluator itself. This mirrors the Go
/// evaluators' memory delta accounting for strings, JSON and vectors.
/// 报告求值器本身之外持有的内存；对齐 Go 对字符串、JSON、向量的内存增量记账。
pub trait ValueMemory {
    /// 返回当前值额外占用的字节数（固定宽度类型为 0）。
    fn retained_size(&self) -> usize;
}

/// 为固定宽度类型生成 `ValueMemory`：额外内存恒为 0。
macro_rules! fixed_value_memory {
    ($($type:ty),+ $(,)?) => {
        $(impl ValueMemory for $type {
            fn retained_size(&self) -> usize { 0 }
        })+
    };
}

fixed_value_memory!(i64, f32, f64, Decimal, TimeValue, DurationValue);

impl ValueMemory for String {
    fn retained_size(&self) -> usize {
        self.len()
    }
}

impl ValueMemory for BinaryJson {
    fn retained_size(&self) -> usize {
        self.value.len()
    }
}

impl ValueMemory for VectorFloat32 {
    fn retained_size(&self) -> usize {
        self.0.len() * size_of::<f32>()
    }
}

/// `None` in `value` is SQL NULL; `evaluated` distinguishes it from an empty
/// window, exactly like Go's `gotFirstValue`/`gotLastValue` flags.
/// `value` 为 `None` 表示 SQL NULL；`evaluated` 区分「已求值（含 NULL）」与「空窗口」，
/// 对齐 Go 的 `gotFirstValue` / `gotLastValue` 标志。
#[derive(Clone, Debug, PartialEq)]
pub struct ValueEvaluator<T> {
    evaluated: bool,
    value: Option<T>,
}

impl<T> Default for ValueEvaluator<T> {
    fn default() -> Self {
        Self {
            evaluated: false,
            value: None,
        }
    }
}

impl<T: ValueMemory> ValueEvaluator<T> {
    /// 写入新值并标记已求值；返回相对旧值的内存增量（字节差）。
    pub fn evaluate(&mut self, value: Option<T>) -> i64 {
        let before = self.value.as_ref().map_or(0, ValueMemory::retained_size);
        let after = value.as_ref().map_or(0, ValueMemory::retained_size);
        self.value = value;
        self.evaluated = true;
        after as i64 - before as i64
    }

    /// 仅清除「已求值」标志，不释放当前值缓冲（对齐 Go reset presence）。
    pub fn reset_presence(&mut self) {
        self.evaluated = false;
    }

    /// 若已求值则返回 `Some(Option<&T>)`（内层 None 为 SQL NULL）；否则 None。
    pub fn result(&self) -> Option<Option<&T>> {
        self.evaluated.then_some(self.value.as_ref())
    }
}

/// 整型值求值器别名。
pub type Value4Int = ValueEvaluator<i64>;
/// Float32 值求值器别名。
pub type Value4Float32 = ValueEvaluator<f32>;
/// Float64 值求值器别名。
pub type Value4Float64 = ValueEvaluator<f64>;
/// Decimal 值求值器别名。
pub type Value4Decimal = ValueEvaluator<Decimal>;
/// 时间（DATE/DATETIME 等）值求值器别名。
pub type Value4Time = ValueEvaluator<TimeValue>;
/// Duration 值求值器别名。
pub type Value4Duration = ValueEvaluator<DurationValue>;
/// 字符串值求值器别名。
pub type Value4String = ValueEvaluator<String>;
/// JSON 值求值器别名。
pub type Value4Json = ValueEvaluator<BinaryJson>;
/// 向量 Float32 值求值器别名。
pub type Value4VectorFloat32 = ValueEvaluator<VectorFloat32>;

/// 将 f64 窄化为 f32 后写入 Float32 求值器，返回内存增量。
pub fn evaluate_float32(evaluator: &mut Value4Float32, value: Option<f64>) -> i64 {
    evaluator.evaluate(value.map(|value| value as f32))
}

/// FIRST_VALUE：分区窗口内取第一行的值。
#[derive(Clone, Debug, PartialEq)]
pub struct FirstValue<T> {
    evaluator: ValueEvaluator<T>,
}

impl<T> Default for FirstValue<T> {
    fn default() -> Self {
        Self {
            evaluator: ValueEvaluator::default(),
        }
    }
}

impl<T: ValueMemory> FirstValue<T> {
    /// 重置「已取到首行」状态，准备下一分区。
    pub fn reset(&mut self) {
        self.evaluator.reset_presence();
    }

    /// 仅在尚未求值时取 `rows` 首元素；已求值则内存增量返回 0。
    pub fn update(&mut self, rows: &[Option<T>]) -> i64
    where
        T: Clone,
    {
        if self.evaluator.evaluated {
            return 0;
        }
        rows.first()
            .map_or(0, |value| self.evaluator.evaluate(value.clone()))
    }

    /// 转发求值器结果。
    pub fn result(&self) -> Option<Option<&T>> {
        self.evaluator.result()
    }
}

/// LAST_VALUE：分区窗口内取最后一行的值（可被后续批次覆盖）。
#[derive(Clone, Debug, PartialEq)]
pub struct LastValue<T> {
    evaluator: ValueEvaluator<T>,
}

impl<T> Default for LastValue<T> {
    fn default() -> Self {
        Self {
            evaluator: ValueEvaluator::default(),
        }
    }
}

impl<T: ValueMemory> LastValue<T> {
    /// 重置求值状态。
    pub fn reset(&mut self) {
        self.evaluator.reset_presence();
    }

    /// 取当前批次末行并覆盖旧值，返回内存增量。
    pub fn update(&mut self, rows: &[Option<T>]) -> i64
    where
        T: Clone,
    {
        rows.last()
            .map_or(0, |value| self.evaluator.evaluate(value.clone()))
    }

    /// 转发求值器结果。
    pub fn result(&self) -> Option<Option<&T>> {
        self.evaluator.result()
    }
}

/// NTH_VALUE：取分区内第 `nth` 行（1-based）；`nth==0` 恒为无效。
#[derive(Clone, Debug, PartialEq)]
pub struct NthValue<T> {
    nth: u64,
    seen_rows: u64,
    evaluator: ValueEvaluator<T>,
}

impl<T> NthValue<T> {
    /// 构造第 `nth` 行求值器（`nth` 从 1 起计）。
    pub fn new(nth: u64) -> Self {
        Self {
            nth,
            seen_rows: 0,
            evaluator: ValueEvaluator::default(),
        }
    }
}

impl<T: ValueMemory> NthValue<T> {
    /// 重置已见行数与求值状态。
    pub fn reset(&mut self) {
        self.seen_rows = 0;
        self.evaluator.reset_presence();
    }

    /// 若目标第 N 行落在本批 `rows` 内则求值；累加 `seen_rows`。
    pub fn update(&mut self, rows: &[Option<T>]) -> i64
    where
        T: Clone,
    {
        if self.nth == 0 {
            return 0;
        }
        let row_count = rows.len() as u64;
        // 目标下标相对本批：nth - seen_rows - 1。
        let memory_delta = if self.nth > self.seen_rows && self.nth - self.seen_rows <= row_count {
            let index = (self.nth - self.seen_rows - 1) as usize;
            self.evaluator.evaluate(rows[index].clone())
        } else {
            0
        };
        self.seen_rows += row_count;
        memory_delta
    }

    /// `nth==0` 或行数不足时返回 None；否则返回已求值结果。
    pub fn result(&self) -> Option<Option<&T>> {
        if self.nth == 0 || self.seen_rows < self.nth {
            None
        } else {
            self.evaluator.result()
        }
    }
}

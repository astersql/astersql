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

// FIRST_ROW 聚合函数：保留分组内遇到的第一行取值。
//
// 对应 SQL 语义「取首行」；在并行聚合中，各分片产出 partial result 后，
// merge 仍保留「先到达」的那一侧首行，不会被后续行覆盖。
// `Option<Option<T>>` 区分「尚未见到任何行」与「首行是 SQL NULL」。

use crate::func_max_min::{BinaryJson, DurationValue, NamedValue, TimeValue, VectorFloat32};
use crate::func_sum::Decimal;

/// `None` means no row has been seen; `Some(None)` is a first row containing
/// SQL NULL. This preserves Go's separate gotFirstRow/isNull flags.
///
/// FIRST_ROW 的 partial 状态：`None` 表示尚未见到行；`Some(None)` 表示首行为 SQL NULL；
/// `Some(Some(v))` 表示已捕获首行非空值。对齐 Go 的 gotFirstRow / isNull 双标志。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FirstRow<T> {
    /// 嵌套 Option：外层是否已见首行，内层是否为 NULL。
    state: Option<Option<T>>,
}

impl<T> FirstRow<T> {
    /// 清空状态，回到「尚未见到行」。
    pub fn reset(&mut self) {
        self.state = None;
    }
    /// 是否已经捕获过首行（含首行为 NULL 的情况）。
    pub fn got_first_row(&self) -> bool {
        self.state.is_some()
    }
    /// 首行是否为 SQL NULL（仅在 got_first_row 为真时有意义）。
    pub fn is_null(&self) -> bool {
        matches!(self.state, Some(None))
    }
    /// 返回已捕获的非空首行值；无行或首行为 NULL 时返回 None。
    pub fn value(&self) -> Option<&T> {
        self.state.as_ref().and_then(Option::as_ref)
    }

    /// 从输入迭代器取第一个元素作为首行；若已有首行则忽略后续输入。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<T>>) {
        // 已锁定首行后不再更新，保证 FIRST_ROW 语义。
        if self.state.is_some() {
            return;
        }
        if let Some(value) = values.into_iter().next() {
            self.state = Some(value);
        }
    }

    /// 合并另一分片的 partial：仅当本侧尚无首行时采纳对侧状态。
    pub fn merge(&mut self, source: &Self)
    where
        T: Clone,
    {
        if self.state.is_none() {
            self.state = source.state.clone();
        }
    }

    /// 消费自身并返回最终状态（外层 Option 即 gotFirstRow）。
    pub fn into_result(self) -> Option<Option<T>> {
        self.state
    }
}

/// 整数列上的 FIRST_ROW。
pub type FirstRow4Int = FirstRow<i64>;
/// float32 列上的 FIRST_ROW。
pub type FirstRow4Float32 = FirstRow<f32>;
/// float64 列上的 FIRST_ROW。
pub type FirstRow4Float64 = FirstRow<f64>;
/// Decimal（精确十进制）列上的 FIRST_ROW。
pub type FirstRow4Decimal = FirstRow<Decimal>;
/// 字符串列上的 FIRST_ROW。
pub type FirstRow4String = FirstRow<String>;
/// 时间/日期列上的 FIRST_ROW。
pub type FirstRow4Time = FirstRow<TimeValue>;
/// Duration（时间间隔）列上的 FIRST_ROW。
pub type FirstRow4Duration = FirstRow<DurationValue>;
/// JSON（BinaryJson）列上的 FIRST_ROW。
pub type FirstRow4Json = FirstRow<BinaryJson>;
/// VectorFloat32 向量列上的 FIRST_ROW。
pub type FirstRow4VectorFloat32 = FirstRow<VectorFloat32>;
/// ENUM 列上的 FIRST_ROW。
pub type FirstRow4Enum = FirstRow<NamedValue>;
/// SET 列上的 FIRST_ROW。
pub type FirstRow4Set = FirstRow<NamedValue>;

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

// MAX/MIN 聚合与滑动窗口极值实现。
//
// 提供通用 `MaxMin<T>`（整组聚合）与基于单调双端队列的 `SlidingMaxMin<T>`
// （窗口滑动时 O(1) 维护极值）。辅助类型覆盖时间、时长、枚举/集合、JSON、向量等 SQL 类型。
// 窗口函数场景下 frame 滑动会剔除过期下标并入队新值，队头始终为当前窗口极值。

use crate::func_sum::Decimal;
use astersql_types::json_functions::{BinaryJSON as TypesBinaryJson, CompareBinaryJSON};
use std::cmp::Ordering;
use std::collections::VecDeque;

/// 时间值包装：打包整数、类型码与小数秒精度（fsp）。
#[derive(Clone, Debug, Default, Eq)]
pub struct TimeValue {
    pub packed: u64,
    pub kind: u8,
    pub fsp: i32,
}

impl PartialEq for TimeValue {
    fn eq(&self, other: &Self) -> bool {
        self.packed == other.packed
    }
}

impl Ord for TimeValue {
    fn cmp(&self, other: &Self) -> Ordering {
        self.packed.cmp(&other.packed)
    }
}

impl PartialOrd for TimeValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 时长值包装：纳秒计数与小数秒精度。
#[derive(Clone, Debug, Default, Eq)]
pub struct DurationValue {
    pub nanos: i64,
    pub fsp: i32,
}

impl PartialEq for DurationValue {
    fn eq(&self, other: &Self) -> bool {
        self.nanos == other.nanos
    }
}

impl Ord for DurationValue {
    fn cmp(&self, other: &Self) -> Ordering {
        self.nanos.cmp(&other.nanos)
    }
}

impl PartialOrd for DurationValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 带名称的枚举/集合元素：极值比较只使用 `name`，`value` 为关联载荷。
#[derive(Clone, Debug, Default, Eq)]
pub struct NamedValue {
    pub name: String,
    pub value: u64,
}

impl PartialEq for NamedValue {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl Ord for NamedValue {
    fn cmp(&self, other: &Self) -> Ordering {
        self.name.cmp(&other.name)
    }
}

impl PartialOrd for NamedValue {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 二进制 JSON 值：类型码加原始字节。
#[derive(Clone, Debug, Default, Eq)]
pub struct BinaryJson {
    pub type_code: u8,
    pub value: Vec<u8>,
}

impl BinaryJson {
    fn as_types_binary_json(&self) -> TypesBinaryJson {
        TypesBinaryJson {
            TypeCode: self.type_code,
            Value: self.value.clone(),
        }
    }
}

impl PartialEq for BinaryJson {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Ord for BinaryJson {
    fn cmp(&self, other: &Self) -> Ordering {
        CompareBinaryJSON(&self.as_types_binary_json(), &other.as_types_binary_json()).cmp(&0)
    }
}

impl PartialOrd for BinaryJson {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// 浮点向量类型；比较走 `total_cmp` 以处理 NaN。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VectorFloat32(pub Vec<f32>);

impl VectorFloat32 {
    /// 逐元素 `total_cmp`，再比较长度，得到稳定全序。
    pub fn compare(&self, other: &Self) -> Ordering {
        for (left, right) in self.0.iter().zip(&other.0) {
            let ordering = left.total_cmp(right);
            if ordering != Ordering::Equal {
                return ordering;
            }
        }
        self.0.len().cmp(&other.0.len())
    }
}

/// MAX/MIN 部分结果：`is_max` 决定取较大还是较小；`None` 表示尚未见到非 NULL 值。
#[derive(Clone, Debug, PartialEq)]
pub struct MaxMin<T> {
    is_max: bool,
    value: Option<T>,
}

impl<T> MaxMin<T> {
    /// `is_max=true` 为 MAX，否则为 MIN。
    pub fn new(is_max: bool) -> Self {
        Self {
            is_max,
            value: None,
        }
    }
    /// 清空已保存的极值。
    pub fn reset(&mut self) {
        self.value = None;
    }
    /// 当前极值；无有效输入时为 `None`（对应 SQL NULL）。
    pub fn value(&self) -> Option<&T> {
        self.value.as_ref()
    }

    /// 用自定义比较器吸收一批可选值；`None`（SQL NULL）被跳过。
    pub fn update_by(
        &mut self,
        values: impl IntoIterator<Item = Option<T>>,
        compare: impl Fn(&T, &T) -> Ordering,
    ) {
        // flatten 跳过 NULL；首个非空直接采纳，其后按 MAX/MIN 规则替换。
        for value in values.into_iter().flatten() {
            let replace = match &self.value {
                None => true,
                Some(current) => match compare(&value, current) {
                    Ordering::Greater => self.is_max,
                    Ordering::Less => !self.is_max,
                    Ordering::Equal => false,
                },
            };
            if replace {
                self.value = Some(value);
            }
        }
    }

    /// 合并另一同方向（同为 MAX 或同为 MIN）的部分结果。
    pub fn merge_by(&mut self, source: &Self, compare: impl Fn(&T, &T) -> Ordering)
    where
        T: Clone,
    {
        assert_eq!(
            self.is_max, source.is_max,
            "cannot merge MAX and MIN states"
        );
        self.update_by([source.value.clone()], compare);
    }
}

impl<T: Ord> MaxMin<T> {
    /// 使用类型默认全序更新。
    pub fn update(&mut self, values: impl IntoIterator<Item = Option<T>>) {
        self.update_by(values, Ord::cmp);
    }
    /// 使用类型默认全序合并。
    pub fn merge(&mut self, source: &Self)
    where
        T: Clone,
    {
        self.merge_by(source, Ord::cmp);
    }
}

/// f32 MAX/MIN：对齐 Go `cmp.Compare`，NaN 排在所有非 NaN 值之前。
pub fn update_float32(state: &mut MaxMin<f32>, values: impl IntoIterator<Item = Option<f32>>) {
    state.update_by(values, compare_float32_like_go);
}
/// f64 MAX/MIN：同上，两个 NaN 视为相等。
pub fn update_float64(state: &mut MaxMin<f64>, values: impl IntoIterator<Item = Option<f64>>) {
    state.update_by(values, compare_float64_like_go);
}

fn compare_float32_like_go(left: &f32, right: &f32) -> Ordering {
    if left < right || (left.is_nan() && !right.is_nan()) {
        Ordering::Less
    } else if left > right || (!left.is_nan() && right.is_nan()) {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

fn compare_float64_like_go(left: &f64, right: &f64) -> Ordering {
    if left < right || (left.is_nan() && !right.is_nan()) {
        Ordering::Less
    } else if left > right || (!left.is_nan() && right.is_nan()) {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}
/// 向量 MAX/MIN：委托 `VectorFloat32::compare`。
pub fn update_vector(
    state: &mut MaxMin<VectorFloat32>,
    values: impl IntoIterator<Item = Option<VectorFloat32>>,
) {
    state.update_by(values, VectorFloat32::compare);
}
/// 带校对规则的字符串 MAX/MIN：比较器由调用方按 collation 提供。
pub fn update_collated_string(
    state: &mut MaxMin<String>,
    values: impl IntoIterator<Item = Option<String>>,
    compare: impl Fn(&str, &str) -> Ordering,
) {
    state.update_by(values, |left, right| compare(left, right));
}

/// 双端队列中的带下标元素：`index` 为窗口内行号，用于过期剔除。
#[derive(Clone, Debug, PartialEq)]
pub struct Pair<T> {
    pub index: u64,
    pub item: T,
}

/// 维护滑动窗口极值的单调双端队列（deque）。
///
/// 队头为当前极值；入队时从队尾弹出被新值支配的元素，保证单调性。
#[derive(Clone, Debug, PartialEq)]
pub struct MinMaxDeque<T> {
    is_max: bool,
    values: VecDeque<Pair<T>>,
}

impl<T> MinMaxDeque<T> {
    /// 构造空队列；`is_max` 决定单调方向。
    pub fn new(is_max: bool) -> Self {
        Self {
            is_max,
            values: VecDeque::new(),
        }
    }
    /// 清空全部元素。
    pub fn reset(&mut self) {
        self.values.clear();
    }
    /// 队列是否为空。
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
    /// 队头（当前极值候选）。
    pub fn front(&self) -> Option<&Pair<T>> {
        self.values.front()
    }
    /// 队尾。
    pub fn back(&self) -> Option<&Pair<T>> {
        self.values.back()
    }
    /// 弹出队头。
    pub fn pop_front(&mut self) -> Option<Pair<T>> {
        self.values.pop_front()
    }
    /// 弹出队尾。
    pub fn pop_back(&mut self) -> Option<Pair<T>> {
        self.values.pop_back()
    }
    /// 剔除下标小于 `boundary` 的过期元素（窗口左边界右移）。
    pub fn dequeue(&mut self, boundary: u64) {
        while self
            .values
            .front()
            .is_some_and(|pair| pair.index < boundary)
        {
            self.values.pop_front();
        }
    }
    /// 入队新元素：先弹出队尾中被支配者（含相等旧值），再追加到队尾。
    pub fn enqueue(&mut self, index: u64, item: T, compare: impl Fn(&T, &T) -> Ordering) {
        // MAX 时弹出更小的队尾；MIN 时弹出更大的队尾，保持单调。
        while let Some(back) = self.values.back() {
            let ordering = compare(&item, &back.item);
            if (self.is_max && ordering != Ordering::Less)
                || (!self.is_max && ordering != Ordering::Greater)
            {
                self.values.pop_back();
            } else {
                break;
            }
        }
        self.values.push_back(Pair { index, item });
    }
}

/// 滑动窗口 MAX/MIN：在 `MinMaxDeque` 之上跟踪窗口起始下标。
#[derive(Clone, Debug, PartialEq)]
pub struct SlidingMaxMin<T> {
    deque: MinMaxDeque<T>,
    window_start: u64,
}

impl<T: Clone> SlidingMaxMin<T> {
    /// 构造空滑动状态。
    pub fn new(is_max: bool) -> Self {
        Self {
            deque: MinMaxDeque::new(is_max),
            window_start: 0,
        }
    }
    /// 设置窗口起始行号（用于后续相对 offset 换算绝对下标）。
    pub fn set_window_start(&mut self, start: u64) {
        self.window_start = start;
    }
    /// 清空队列内容（不改 `window_start` 语义由调用方决定）。
    pub fn reset(&mut self) {
        self.deque.reset();
    }
    /// 当前窗口极值（队头）。
    pub fn value(&self) -> Option<&T> {
        self.deque.front().map(|pair| &pair.item)
    }
    /// 向窗口追加一批值，绝对下标 = `window_start + offset`。
    pub fn update_by(
        &mut self,
        values: impl IntoIterator<Item = Option<T>>,
        compare: impl Fn(&T, &T) -> Ordering + Copy,
    ) {
        for (offset, value) in values.into_iter().enumerate() {
            if let Some(value) = value {
                self.deque
                    .enqueue(self.window_start + offset as u64, value, compare);
            }
        }
    }
    /// 窗口左边界移到 `new_start`，并入队从 `incoming_start` 起的新行。
    pub fn slide_by(
        &mut self,
        new_start: u64,
        incoming_start: u64,
        incoming: impl IntoIterator<Item = Option<T>>,
        compare: impl Fn(&T, &T) -> Ordering + Copy,
    ) {
        // 先删过期，再入队新值，最后更新窗口起点。
        self.deque.dequeue(new_start);
        for (offset, value) in incoming.into_iter().enumerate() {
            if let Some(value) = value {
                self.deque
                    .enqueue(incoming_start + offset as u64, value, compare);
            }
        }
        self.window_start = new_start;
    }
}

/// 各 SQL 类型的 MAX/MIN 与滑动窗口别名，命名与 Go 侧对齐。
pub type MaxMin4Int = MaxMin<i64>;
pub type MaxMin4Uint = MaxMin<u64>;
pub type MaxMin4Float32 = MaxMin<f32>;
pub type MaxMin4Float64 = MaxMin<f64>;
pub type MaxMin4Decimal = MaxMin<Decimal>;
pub type MaxMin4String = MaxMin<String>;
pub type MaxMin4Time = MaxMin<TimeValue>;
pub type MaxMin4Duration = MaxMin<DurationValue>;
pub type MaxMin4Json = MaxMin<BinaryJson>;
pub type MaxMin4VectorFloat32 = MaxMin<VectorFloat32>;
pub type MaxMin4Enum = MaxMin<NamedValue>;
pub type MaxMin4Set = MaxMin<NamedValue>;
pub type MaxMin4IntSliding = SlidingMaxMin<i64>;
pub type MaxMin4UintSliding = SlidingMaxMin<u64>;
pub type MaxMin4Float32Sliding = SlidingMaxMin<f32>;
pub type MaxMin4Float64Sliding = SlidingMaxMin<f64>;
pub type MaxMin4DecimalSliding = SlidingMaxMin<Decimal>;
pub type MaxMin4StringSliding = SlidingMaxMin<String>;
pub type MaxMin4TimeSliding = SlidingMaxMin<TimeValue>;
pub type MaxMin4DurationSliding = SlidingMaxMin<DurationValue>;

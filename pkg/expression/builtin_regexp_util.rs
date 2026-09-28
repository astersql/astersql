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

// 正则内建函数的任务本地辅助类型与缓冲管理。
//
// 提供可空列、记忆化签名、函数参数与缓冲分配器，以及 IN/结果 NULL
// 判定与越界 position 检查，对应 Go `builtin_regexp_util.go` 中的
// 向量化 IO 辅助逻辑，不依赖具体物理字符串编码。

/// 任务本地表达式测试用的小型可空列。
///
/// 与 TiDB `chunk.Column` 相同的按行可空模型，但不绑定具体物理字符串表示。
/// A small native column used by the task-local expression harness.  It keeps
/// the same nullable row model used by TiDB's `chunk.Column` without coupling
/// these helpers to a particular physical string representation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Column<T> {
    values: Vec<Option<T>>,
}

impl<T> Column<T> {
    /// 由可空值向量构造列。
    pub fn new(values: Vec<Option<T>>) -> Self {
        Self { values }
    }

    /// 返回底层可空值切片。
    pub fn values(&self) -> &[Option<T>] {
        &self.values
    }

    /// 判断指定行是否为 NULL；与 Go `chunk.Column` 一样，越界会 panic。
    pub fn is_null(&self, row: usize) -> bool {
        self.values[row].is_none()
    }

    /// 追加一行 NULL。
    pub fn append_null(&mut self) {
        self.values.push(None);
    }

    /// 清空已有行并预留容量，对应 Go `ReserveString`。
    pub fn reserve(&mut self, capacity: usize) {
        self.values.clear();
        self.values.reserve(capacity);
    }
}

/// 记忆化正则签名，对应 Go `regexpMemorizedSig`。
///
/// 编译成功与失败都会被缓存，避免同一常量模式重复编译。
/// The cached value mirrors Go's `regexpMemorizedSig`: compilation failures
/// are cached alongside successful compiled expressions.
#[derive(Clone, Debug)]
pub struct RegexpMemorizedSig<T, E> {
    /// 成功编译的正则（若有）。
    pub memorized_regexp: Option<T>,
    /// 编译失败时缓存的错误（若有）。
    pub memorized_err: Option<E>,
}

/// 函数参数：常量无列，或持有物化后的列缓冲。
#[derive(Clone, Debug)]
pub struct FuncParam<T> {
    column: Option<Column<T>>,
}

impl<T> FuncParam<T> {
    /// 构造不占用列缓冲的常量参数。
    pub fn constant() -> Self {
        Self { column: None }
    }

    /// 构造持有列缓冲的参数。
    pub fn column(column: Column<T>) -> Self {
        Self {
            column: Some(column),
        }
    }

    /// 若参数已物化为列则返回引用。
    pub fn get_col(&self) -> Option<&Column<T>> {
        self.column.as_ref()
    }
}

/// 简单缓冲分配器：回收已释放的列，便于统计归还次数。
#[derive(Clone, Debug, Default)]
pub struct BufferAllocator<T> {
    returned: Vec<Column<T>>,
}

impl<T> BufferAllocator<T> {
    /// 将列归还到分配器。
    pub fn put(&mut self, column: Column<T>) {
        self.returned.push(column);
    }

    /// 已归还列的数量。
    pub fn returned_len(&self) -> usize {
        self.returned.len()
    }
}

/// 按参数顺序收集所有已物化的列缓冲。
/// Returns every materialized parameter buffer in parameter order.
pub fn get_buffers<T>(params: &[FuncParam<T>]) -> Vec<&Column<T>> {
    params.iter().filter_map(FuncParam::get_col).collect()
}

/// 仅释放来自分配器的缓冲；常量参数无列，按 Go 循环语义跳过。
/// Releases only buffers that came from the allocator. Constant parameters do
/// not own a column and are deliberately skipped, matching the Go loop.
pub fn release_buffers<T: Clone>(allocator: &mut BufferAllocator<T>, params: &[FuncParam<T>]) {
    for param in params {
        // Go 将指针归还池中但不会清空 funcParam.col；此本地值模型通过克隆
        // 保留同样的参数可观察状态。
        if let Some(column) = param.get_col().cloned() {
            allocator.put(column);
        }
    }
}

/// 任一物化输入列为 NULL 时，该结果行视为 NULL。
/// A result row is NULL when any materialized input column is NULL.
pub fn is_result_null<T>(columns: &[Column<T>], row: usize) -> bool {
    columns.iter().any(|column| column.is_null(row))
}

/// 先预留容量，再追加恰好 `num` 行 NULL 字符串。
/// Reserves the requested capacity before appending exactly `num` NULL rows.
pub fn fill_null_string_into_result(result: &mut Column<String>, num: usize) {
    result.reserve(num);
    for _ in 0..num {
        result.append_null();
    }
}

/// 检查 position 是否越界；唯一合法边界是空串且 position=1。
///
/// 返回 `true` 表示越界（应报错或按调用方处理）。
/// When a position is outside the normal range, the sole accepted edge case
/// is position one on an empty string.
pub fn check_out_range_pos(str_len: usize, pos: i64) -> bool {
    str_len != 0 || pos != 1
}

// Copyright 2020 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 第 k 小选择算法（introselect）实现。
//
// 对应 Go `pkg/util/selection`：对外提供 `Select`，内部在 quickselect
// 与 median-of-medians 之间切换，避免最坏情况下递归过深。

use rand::Rng;

// Interface is alias of sort.Interface
// Interface 对应 Go 中 `type Interface = sort.Interface` 的别名语义。
// Rust 没有直接引用 Go sort.Interface 的能力，因此用 trait 保留 Len/Less/Swap 三个方法形状。
/// 可比较可交换序列的接口，形状对齐 Go `sort.Interface`。
pub trait Interface {
    fn Len(&self) -> isize;
    fn Less(&self, i: isize, j: isize) -> bool;
    fn Swap(&mut self, i: isize, j: isize);
}

// Select performs introselect algorithm on data and return index of the k-th smallest value.
// Select 对应 Go 的导出函数：对传入数据执行 introselect，并返回第 k 小元素的索引。
// Go 代码把用户传入的 k 当作 1-based 排名，这里继续在调用 introselect 前减一。
/// 对数据执行 introselect，返回第 k 小元素的索引（k 为 1-based；空数据返回 -1）。
pub fn Select(data: &mut dyn Interface, k: isize) -> isize {
    // 空数据时保持 Go 的哨兵返回值 -1；非空时右边界是 Len()-1。
    let len = data.Len();
    if len > 0 {
        return introselect(data, 0, len - 1, k - 1, 6);
    }
    -1
}

// introselect will perform quickselect at beginning, and switch to linear-time algorithm if it recurses too much times.
// Source paper: http://www.cs.rpi.edu/~musser/gp/introsort.ps
// introselect 先按 quickselect 递归；递归深度耗尽时切换到线性时间的 median-of-medians。
fn introselect(
    data: &mut dyn Interface,
    left: isize,
    right: isize,
    k: isize,
    depth: isize,
) -> isize {
    // 单元素区间已经定位到目标索引，直接返回左边界。
    if left == right {
        return left;
    }
    if depth <= 0 {
        // Use median of medians algorithm(linear-time selection) when recurses too much times.
        // 深度耗尽说明随机 pivot 递归过深，沿用 Go 代码切到更稳定的线性选择算法。
        return medianOfMedians(data, left, right, k);
    }
    // TODO: use a better pivot function
    // Go 这里随机选 pivot；后续 partition 会把 pivot 放到最终排序位置。
    let mut pivotIndex = randomPivot(data, left, right);
    pivotIndex = partition(data, left, right, pivotIndex);
    if k == pivotIndex {
        return k;
    } else if k < pivotIndex {
        // 目标索引在 pivot 左侧时只递归左半区间，并消耗一层深度预算。
        return introselect(data, left, pivotIndex - 1, k, depth - 1);
    }
    // 目标索引在 pivot 右侧时只递归右半区间。
    introselect(data, pivotIndex + 1, right, k, depth - 1)
}

// quickselect is used in test for comparison.
// nolint: unused
// quickselect 保留 Go 测试中用于对照的快速选择实现；当前任务不迁移测试文件。
/// 纯 quickselect 实现，供测试与 introselect 对照；k 为 0-based 索引。
pub(crate) fn quickselect(data: &mut dyn Interface, left: isize, right: isize, k: isize) -> isize {
    if left == right {
        return left;
    }
    let mut pivotIndex = randomPivot(data, left, right);
    pivotIndex = partition(data, left, right, pivotIndex);
    if k == pivotIndex {
        return k;
    } else if k < pivotIndex {
        return quickselect(data, left, pivotIndex - 1, k);
    }
    quickselect(data, pivotIndex + 1, right, k)
}

// medianOfMedians 对应 Go 的线性时间选择入口，使用 medianOfMediansPivot 选出更稳健的 pivot。
fn medianOfMedians(data: &mut dyn Interface, left: isize, right: isize, k: isize) -> isize {
    if left == right {
        return left;
    }
    let mut pivotIndex = medianOfMediansPivot(data, left, right);
    // partitionIntro 会特别处理等于 pivot 的元素，避免大量重复值导致继续无效递归。
    pivotIndex = partitionIntro(data, left, right, pivotIndex, k);
    if k == pivotIndex {
        return k;
    } else if k < pivotIndex {
        return medianOfMedians(data, left, pivotIndex - 1, k);
    }
    medianOfMedians(data, pivotIndex + 1, right, k)
}

// randomPivot 对应 Go 的随机 pivot 选择函数。
fn randomPivot(_data: &mut dyn Interface, left: isize, right: isize) -> isize {
    rand::thread_rng().gen_range(left..=right) // #nosec G404
}

// medianOfMediansPivot 把区间按 5 个元素一组求中位数，再递归选择这些中位数的中位数。
fn medianOfMediansPivot(data: &mut dyn Interface, left: isize, right: isize) -> isize {
    if right - left < 5 {
        return partition5(data, left, right);
    }
    let mut i = left;
    while i <= right {
        let subRight = std::cmp::min(i + 4, right);
        let median5 = partition5(data, i, subRight);
        // 将每组的中位数搬到左侧紧凑区域，供后续递归选择 pivot。
        data.Swap(median5, left + (i - left) / 5);
        i += 5;
    }
    let mid = (right - left) / 10 + left + 1;
    medianOfMedians(data, left, left + (right - left) / 5, mid)
}

// partition 对应 Go 的普通 partition：小于 pivot 的元素移到左侧，最后返回 pivot 的最终索引。
fn partition(data: &mut dyn Interface, left: isize, right: isize, pivotIndex: isize) -> isize {
    data.Swap(pivotIndex, right);
    let mut storeIndex = left;
    let mut i = left;
    while i < right {
        // Less(i, right) 保留 Go sort.Interface 的严格小于比较。
        if data.Less(i, right) {
            data.Swap(storeIndex, i);
            storeIndex += 1;
        }
        i += 1;
    }
    data.Swap(right, storeIndex);
    storeIndex
}

// partitionIntro 是 median-of-medians 路径使用的 partition，额外把等于 pivot 的元素聚到一起。
fn partitionIntro(
    data: &mut dyn Interface,
    left: isize,
    right: isize,
    pivotIndex: isize,
    k: isize,
) -> isize {
    data.Swap(pivotIndex, right);
    let mut storeIndex = left;
    // Move all elements smaller than pivot to left side
    // 第一段循环把所有严格小于 pivot 的元素移动到左侧。
    let mut i = left;
    while i < right {
        if data.Less(i, right) {
            data.Swap(storeIndex, i);
            storeIndex += 1;
        }
        i += 1;
    }
    let mut storeIndexEq = storeIndex;
    // Move all elements equal to pivot right after
    // 第二段循环把与 pivot 相等的元素紧跟在“小于区间”后面。
    i = storeIndex;
    while i < right {
        // data[i] == data[right]
        // Go 通过两次 Less 都为 false 判断相等；这里保留该比较约定，不要求元素实现 Eq。
        if !data.Less(i, right) && !data.Less(right, i) {
            data.Swap(storeIndexEq, i);
            storeIndexEq += 1;
        }
        i += 1;
    }
    // Move pivot to final place
    // pivot 被放到等值区间末尾；随后根据 k 的位置返回继续递归所需的边界索引。
    data.Swap(right, storeIndexEq);
    if k < storeIndex {
        return storeIndex;
    }
    if k <= storeIndexEq {
        return k;
    }
    storeIndexEq
}

// partition5 对最多 5 个元素的小区间做插入排序，并返回该小区间的中位数索引。
fn partition5(data: &mut dyn Interface, left: isize, right: isize) -> isize {
    let mut i = left + 1;
    while i <= right {
        let mut j = i;
        // 这里机械保留 Go 的插入排序：相邻元素逆序时反复 Swap，直到当前位置有序。
        while j > left && data.Less(j, j - 1) {
            data.Swap(j, j - 1);
            j = j - 1;
        }
        i += 1;
    }
    (left + right) / 2
}

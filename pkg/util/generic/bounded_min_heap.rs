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

// 有界最小堆（Bounded Min-Heap）：高效维护“最好的 N 个”元素。
//
// 对应 Go `pkg/util/generic/bounded_min_heap.go`。内部用最小堆把当前
// 最差元素放在根，满容量时用更好元素替换根并向下调整；比较函数沿用
// Go 三值约定（>0 表示左侧更好）。

// 本文件由 pkg/util/generic/bounded_min_heap.go 迁移而来，保留 Go 实现结构与行为。
//
// Rust 实现用 Vec 和本地 sift_up/sift_down 保留 container/heap 的调用效果。

use std::cmp::Ordering;

// internalHeap is an unexported heap implementation backing BoundedMinHeap.
// it keeps the worst item at the root according to cmp.
// internalHeap 对应 Go 的 heap.Interface 实现，cmp 返回值仍沿用 “大于 0 表示左侧更好” 的约定。
/// 支撑 `BoundedMinHeap` 的内部堆：按 `cmp` 将最差元素保持在根。
pub struct internalHeap<T, F>
where
    F: Fn(&T, &T) -> i32,
{
    cmp: F,
    items: Vec<T>,
}

impl<T, F> internalHeap<T, F>
where
    F: Fn(&T, &T) -> i32,
{
    // Len implements heap.Interface.
    /// 返回堆中元素个数。
    pub fn Len(&self) -> usize {
        self.items.len()
    }

    // Less implements heap.Interface; the min-heap keeps the worst item at the root.
    // Less 保留 Go 里的 cmp(items[i], items[j]) < 0，因此堆顶是当前最差元素。
    /// `cmp(i,j)<0` 时视为 i 更差，从而最小堆顶为最差元素。
    pub fn Less(&self, i: usize, j: usize) -> bool {
        (self.cmp)(&self.items[i], &self.items[j]) < 0
    }

    // Swap implements heap.Interface.
    /// 交换下标 i 与 j 处的元素。
    pub fn Swap(&mut self, i: usize, j: usize) {
        self.items.swap(i, j);
    }

    // Push implements heap.Interface.
    // Go 的 Push 接收 any 再断言为 T；Rust 实现直接接收 T，并把 heap.Push 的上浮动作合并到本地实现。
    /// 追加元素并上浮，恢复堆序。
    pub fn Push(&mut self, x: T) {
        self.items.push(x);
        let last = self.items.len() - 1;
        self.sift_up(last);
    }

    // Pop implements heap.Interface.
    // 在 Go container/heap 中 Pop 只弹出尾部；这里保留同名方法用于表达接口语义。
    /// 弹出尾部元素（对齐 Go `heap.Interface.Pop` 语义，不含下沉）。
    pub fn Pop(&mut self) -> Option<T> {
        self.items.pop()
    }

    // heap.Fix(&h.data, 0) 的本地等价：根元素被替换后向下调整，重新满足最小堆性质。
    fn fix_root(&mut self) {
        if !self.items.is_empty() {
            self.sift_down(0);
        }
    }

    /// 子节点上浮直至不优于父节点。
    fn sift_up(&mut self, mut child: usize) {
        while child > 0 {
            let parent = (child - 1) / 2;
            if !self.Less(child, parent) {
                break;
            }
            self.Swap(parent, child);
            child = parent;
        }
    }

    /// 根节点下沉：在左右子中选更差者交换，直至满足堆性质。
    fn sift_down(&mut self, mut root: usize) {
        let len = self.items.len();
        loop {
            let left = root * 2 + 1;
            if left >= len {
                break;
            }

            // Go 的 heap 包会选择更小的子节点；这里按 Less 保持“最差元素靠近根”的方向。
            let right = left + 1;
            let mut child = left;
            if right < len && self.Less(right, left) {
                child = right;
            }

            if !self.Less(child, root) {
                break;
            }
            self.Swap(root, child);
            root = child;
        }
    }
}

// BoundedMinHeap maintains the best N items efficiently using an internal min-heap.
// It keeps the N best items according to the comparison function.
// The root of the internal heap is always the worst item, making it easy to remove when a better item arrives.
// BoundedMinHeap 对应 Go 的导出类型，maxSize 控制最多保留多少个“最好”的元素。
/// 有界最小堆：最多保留 `maxSize` 个按比较函数判定的最好元素。
pub struct BoundedMinHeap<T, F>
where
    F: Fn(&T, &T) -> i32,
{
    data: internalHeap<T, F>,
    maxSize: usize,
}

// NewBoundedMinHeap creates a new bounded min-heap with the specified maximum size and comparison function.
// NewBoundedMinHeap 保留 maxSize 负数 panic；Go 的 nil 函数检查在 Rust 泛型闭包形态下没有直接等价物。
/// 创建有界最小堆；`maxSize < 0` 时 panic。
pub fn NewBoundedMinHeap<T, F>(maxSize: isize, cmpFunc: F) -> BoundedMinHeap<T, F>
where
    F: Fn(&T, &T) -> i32,
{
    if maxSize < 0 {
        panic!("maxSize cannot be negative");
    }

    BoundedMinHeap {
        data: internalHeap {
            items: Vec::with_capacity(maxSize as usize),
            cmp: cmpFunc,
        },
        maxSize: maxSize as usize,
    }
}

impl<T, F> BoundedMinHeap<T, F>
where
    F: Fn(&T, &T) -> i32,
{
    // Len returns the number of items in the heap.
    /// 当前已保存的元素个数。
    pub fn Len(&self) -> usize {
        self.data.Len()
    }

    // Add adds an item to the bounded min-heap. If the heap is full and the new item
    // is better than the worst item, it replaces the worst item.
    /// 未满则直接插入；已满且新元素优于根（最差）则替换并 `fix_root`。
    pub fn Add(&mut self, item: T) {
        // handle zero capacity case
        // 容量为 0 时 Go 直接返回；Rust 实现同样不保存传入 item。
        if self.maxSize == 0 {
            return;
        }

        if self.data.items.len() < self.maxSize {
            // heap not full, just add the item
            self.data.Push(item);
            return;
        }

        // heap is full, check if new item is better than the worst (root of min-heap)
        // cmp(item, root)>0 表示新元素比当前最差元素更好，因此替换根并执行 heap.Fix。
        if (self.data.cmp)(&item, &self.data.items[0]) > 0 {
            // new item is better, replace the worst
            self.data.items[0] = item;
            self.data.fix_root();
        }
    }
}

impl<T, F> BoundedMinHeap<T, F>
where
    T: Clone,
    F: Fn(&T, &T) -> i32,
{
    // ToSortedSlice returns all items in the heap as a sorted slice (best to worst).
    /// 返回从最好到最差排序的克隆切片；空堆返回 `None`。
    pub fn ToSortedSlice(&self) -> Option<Vec<T>> {
        if self.data.items.is_empty() {
            return None;
        }

        // copy items to avoid modifying the original heap
        // Go 可以直接 copy 任意 T；Rust 返回独立 Vec 需要 T: Clone。
        let mut result = self.data.items.clone();

        // sort from best to worst using a negated comparator
        result.sort_by(|a, b| cmp_to_order((self.data.cmp)(a, b).saturating_neg()));

        Some(result)
    }
}

// cmp_to_order 把 Go slices.SortFunc 的负/零/正规约定转换为 Rust Ordering。
fn cmp_to_order(v: i32) -> Ordering {
    if v < 0 {
        Ordering::Less
    } else if v > 0 {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

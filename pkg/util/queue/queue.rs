// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 环形缓冲队列：对齐 Go `Queue[T]` 的 Push/Pop、扩容与清空语义。
//
// 用 `Option<Vec<Option<T>>>` 近似 Go 的 nil/非 nil slice 与可按下标写入的槽位；
// 零值可用，满时容量倍增并重排逻辑顺序。

#![allow(non_snake_case)]

// Queue is a circular buffer implementation of queue.
// Queue 对应 Go 的泛型结构体 Queue[T any]，字段顺序保持 elements、head、tail、size。
/// 环形缓冲队列，字段顺序对齐 Go：elements、head、tail、size。
pub struct Queue<T> {
    // elements 对应 Go 的 []T。Rust 不能在不要求 T: Default 的情况下直接创建长度为 capacity 的 Vec<T>，
    // 因此这里用 Option<T> 槽位表达 Go make([]T, capacity) 后可按下标写入的缓冲区。
    // 外层 Option 用来近似 Go 里的 nil slice：Queue 零值时为 None，NewQueue 创建后为 Some。
    elements: Option<Vec<Option<T>>>,
    // head/tail/size 分别对应 Go 代码中的队头、下一次写入位置和当前元素数量。
    // 游标与元素数量只参与 slice 下标和容量计算，因此使用 usize。
    head: usize,
    tail: usize,
    size: usize,
}

// Default 对应 Go 中 Queue[T] 的零值形态：elements 为 nil，三个计数器为 0。
impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self {
            elements: None,
            head: 0,
            tail: 0,
            size: 0,
        }
    }
}

// NewQueue creates a new queue with the given capacity.
// NewQueue 对应 Go 的 NewQueue[T any](capacity int) *Queue[T]。
// Go 返回指针；Rust 用 Box<Queue<T>> 表达调用者拥有堆上队列实例。
/// 按给定容量创建队列；负容量 panic。返回堆上 `Box<Queue<T>>`。
pub fn NewQueue<T>(capacity: isize) -> Box<Queue<T>> {
    let capacity = usize::try_from(capacity).expect("queue capacity must not be negative");
    Box::new(Queue {
        // 对应 Go 的 make([]T, capacity)，这里通过辅助函数创建可按下标写入的空槽位。
        elements: Some(make_slots(capacity)),
        head: 0,
        tail: 0,
        size: 0,
    })
}

impl<T> Queue<T> {
    // Push pushes an element to the queue.
    // Push 对应 Go 的 (*Queue[T]).Push：向环形队列尾部写入一个元素，满时扩容为两倍。
    /// 入队；满时倍增扩容并重排，零值首次 Push 分配容量 1。
    pub fn Push(&mut self, element: T) {
        if self.elements.is_none() {
            // Go 中 nil slice 的队列第一次 Push 会分配长度为 1 的 slice；
            // 这里保持同样的零值可用语义。
            self.elements = Some(make_slots(1));
        }

        let current_len = self.elements.as_ref().map_or(0, Vec::len);
        if self.size == current_len {
            // Double capacity when full
            // 队列满时按 Go 实现扩容为原容量的两倍，并从 head 开始把逻辑顺序重排到新缓冲区头部。
            // 如果 NewQueue(0) 产生零容量缓冲区，Go 代码也会扩成 0 后在写入时 panic；
            // 这里保留相同的控制流，不额外修正容量。
            let mut new_elements = make_slots(current_len * 2);
            if current_len > 0 {
                if let Some(elements) = self.elements.as_mut() {
                    for i in 0..self.size {
                        let old_index = (self.head + i) % current_len;
                        // Go 赋值会复制/移动 T 的值到新 slice；Rust 需要显式取走 Option 槽位中的值。
                        // 这会清空旧槽位，是 Rust 所有权下对 Go slice 重排的近似表达。
                        new_elements[i] = elements[old_index].take();
                    }
                }
            }
            self.elements = Some(new_elements);
            self.head = 0;
            self.tail = self.size;
        }

        let elements = self
            .elements
            .as_mut()
            .expect("Queue elements should be initialized before Push");
        // 对应 Go 的 r.elements[r.tail] = element；若容量为 0，这里会像 Go 一样触发越界 panic。
        elements[self.tail] = Some(element);
        self.tail = (self.tail + 1) % elements.len();
        self.size += 1;
    }

    // Pop pops an element from the queue.
    // Pop 对应 Go 的 (*Queue[T]).Pop：队列为空时 panic，否则取出 head 位置的元素并前移队头。
    /// 出队；空队列 panic `"Queue is empty"`。
    pub fn Pop(&mut self) -> T {
        if self.size == 0 {
            // 保留 Go 原始 panic 文案，便于人工对照迁移结果。
            panic!("Queue is empty");
        }
        let elements = self
            .elements
            .as_mut()
            .expect("Queue elements should be initialized before Pop");
        // Go 返回 r.elements[r.head] 后不会清空槽位；Rust 为了移出 T 使用 take() 清空 Option。
        // 这是 Rust 所有权模型下“返回队头元素”的等价实现。
        let element = elements[self.head]
            .take()
            .expect("Queue slot should contain an element");
        self.head = (self.head + 1) % elements.len();
        self.size -= 1;
        element
    }

    // Len returns the number of elements in the queue.
    // Len 对应 Go 的 (*Queue[T]).Len，返回当前已入队且未弹出的元素数量。
    /// 当前元素个数。
    pub fn Len(&self) -> usize {
        self.size
    }

    // IsEmpty returns true if the queue is empty.
    // IsEmpty 对应 Go 的 (*Queue[T]).IsEmpty，直接判断 size 是否为 0。
    /// 是否为空队列。
    pub fn IsEmpty(&self) -> bool {
        self.size == 0
    }

    // Clear clears the queue.
    // Clear 对应 Go 的 (*Queue[T]).Clear，只重置游标和数量，不释放底层缓冲区容量。
    /// 逻辑清空：重置游标与 size，保留底层容量。
    pub fn Clear(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.size = 0;
    }

    // ClearAndExpandIfNeed clears the queue and try to expand the elements
    // ClearAndExpandIfNeed 对应 Go 的同名方法：先清空逻辑队列，再在当前容量不足时扩容。
    /// 先 Clear；若 `size > 0` 且当前容量不足则扩到 `size`。
    pub fn ClearAndExpandIfNeed(&mut self, size: isize) {
        self.Clear();

        let current_len = self.elements.as_ref().map_or(0, Vec::len);
        if size > 0 && current_len < size as usize {
            // Go 这里用 make([]T, size) 替换底层 slice；Rust 重新创建同等长度的空槽位。
            self.elements = Some(make_slots(size as usize));
        }
    }

    // Cap returns the capacity of the queue.
    // Cap 对应 Go 的 (*Queue[T]).Cap，返回底层 elements slice 的长度；nil slice 近似为 0。
    /// 底层缓冲容量；nil 近似为 0。
    pub fn Cap(&self) -> usize {
        self.elements.as_ref().map_or(0, Vec::len)
    }
}

// make_slots 是局部辅助函数，用于表达 Go 的 make([]T, capacity)。
// 它只创建指定数量的空槽位，不会构造 T，因此不需要给泛型 T 增加 Default 或 Clone 约束。
fn make_slots<T>(capacity: usize) -> Vec<Option<T>> {
    let mut elements = Vec::with_capacity(capacity);
    elements.resize_with(capacity, || None);
    elements
}

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

// 队列迁移回归：对照 Go 行为验证 FIFO 扩容复用与有符号容量边界。
//
// 覆盖绕回后倍增容量、`ClearAndExpandIfNeed` 复用，以及负容量 panic / 不扩容语义。

use super::{NewQueue, Queue};

/// 验证 Pop 后 Push 触发倍增扩容，且 ClearAndExpandIfNeed 后可复用缓冲。
#[test]
fn migration_queue_matches_go_fifo_growth_and_reuse() {
    let mut queue = NewQueue::<String>(3);
    queue.Push("one".to_owned());
    queue.Push("two".to_owned());
    assert_eq!(queue.Pop(), "one");

    // 绕回后继续 Push，满时容量从 3 扩到 6，FIFO 顺序不变。
    queue.Push("three".to_owned());
    queue.Push("four".to_owned());
    queue.Push("five".to_owned());
    assert_eq!(queue.Cap(), 6);
    assert_eq!(
        [queue.Pop(), queue.Pop(), queue.Pop(), queue.Pop()],
        ["two", "three", "four", "five"]
    );

    // 清空并扩到 8，再 Push/Pop 验证缓冲可复用。
    queue.ClearAndExpandIfNeed(8);
    assert!(queue.IsEmpty());
    assert_eq!(queue.Cap(), 8);
    queue.Push("reused".to_owned());
    assert_eq!(queue.Pop(), "reused");
}

/// 验证 NewQueue 负容量 panic，以及 ClearAndExpandIfNeed(负值) 不清扩已有容量。
#[test]
fn migration_queue_matches_go_signed_capacity_boundaries() {
    let negative_capacity = -1_isize;
    assert!(std::panic::catch_unwind(|| NewQueue::<i32>(negative_capacity)).is_err());

    // 零值队列 Push 后容量为 1；负 size 的 ClearAndExpandIfNeed 只清空、不扩容。
    let mut queue = Queue::<i32>::default();
    queue.Push(7);
    queue.ClearAndExpandIfNeed(-1);
    assert!(queue.IsEmpty());
    assert_eq!(queue.Cap(), 1);
}

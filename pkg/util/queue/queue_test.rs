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

// `Queue` 单元测试：FIFO、绕回扩容、清空扩容、零值与 panic 边界。
//
// 对齐 Go 队列语义，包括空 Pop 文案与零容量 Push 的越界 panic。

use super::{NewQueue, Queue};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// 将 panic payload 转为字符串，便于断言 Go 风格消息。
fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "non-string panic".to_owned()
    }
}

/// 基本 Push/Pop 保序，满时容量倍增。
#[test]
fn basic_operations_preserve_fifo_order_and_double_capacity() {
    let mut queue = NewQueue::<i32>(2);

    assert!(queue.IsEmpty());
    assert_eq!(queue.Len(), 0);
    assert_eq!(queue.Cap(), 2);

    queue.Push(1);
    queue.Push(2);
    assert_eq!(queue.Len(), 2);
    assert!(!queue.IsEmpty());

    // 第三次 Push 触发 2→4 扩容，随后 Pop 仍按 FIFO。
    queue.Push(3);
    assert_eq!(queue.Cap(), 4);
    assert_eq!(queue.Pop(), 1);
    assert_eq!(queue.Pop(), 2);
    assert_eq!(queue.Pop(), 3);
    assert!(queue.IsEmpty());
}

/// 绕回后再 Push 触发扩容时，逻辑顺序与 Go 一致。
#[test]
fn wraparound_growth_preserves_logical_order() {
    let mut queue = NewQueue::<String>(3);
    queue.Push("one".to_owned());
    queue.Push("two".to_owned());
    assert_eq!(queue.Pop(), "one");
    queue.Push("three".to_owned());
    queue.Push("four".to_owned());
    queue.Push("five".to_owned());

    assert_eq!(queue.Cap(), 6);
    assert_eq!(queue.Pop(), "two");
    assert_eq!(queue.Pop(), "three");
    assert_eq!(queue.Pop(), "four");
    assert_eq!(queue.Pop(), "five");
}

/// Clear 保留容量，且非 Copy 类型可再次入队复用。
#[test]
fn clear_keeps_capacity_and_allows_non_copy_values_to_be_reused() {
    let mut queue = NewQueue::<String>(4);
    queue.Push("a".to_owned());
    queue.Push("b".to_owned());
    queue.Push("c".to_owned());

    queue.Clear();
    assert!(queue.IsEmpty());
    assert_eq!(queue.Len(), 0);
    assert_eq!(queue.Cap(), 4);

    queue.Push("after-clear".to_owned());
    assert_eq!(queue.Pop(), "after-clear");
}

/// ClearAndExpandIfNeed 仅在目标更大时扩容，否则保持原容量。
#[test]
fn clear_and_expand_only_grows_capacity() {
    let mut queue = NewQueue::<String>(2);
    queue.Push("discarded".to_owned());

    queue.ClearAndExpandIfNeed(5);
    assert!(queue.IsEmpty());
    assert_eq!(queue.Cap(), 5);

    // 目标更小不会缩容。
    queue.ClearAndExpandIfNeed(3);
    assert_eq!(queue.Cap(), 5);
    queue.Push("kept".to_owned());
    assert_eq!(queue.Pop(), "kept");
}

/// 零值队列首次 Push 分配容量 1，可正常 Pop。
#[test]
fn zero_value_queue_is_usable() {
    let mut queue = Queue::<String>::default();
    assert_eq!(queue.Cap(), 0);

    queue.Push("value".to_owned());
    assert_eq!(queue.Cap(), 1);
    assert_eq!(queue.Pop(), "value");
}

/// 空队列 Pop 的 panic 文案与 Go 一致。
#[test]
fn empty_pop_panics_with_go_message() {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        NewQueue::<i32>(1).Pop();
    }))
    .expect_err("Pop on an empty queue must panic");

    assert_eq!(panic_message(panic), "Queue is empty");
}

/// 零容量 Push 像 Go 下标越界一样 panic。
#[test]
fn zero_capacity_push_panics_like_go_indexing() {
    let panic = catch_unwind(AssertUnwindSafe(|| {
        NewQueue::<String>(0).Push("value".to_owned());
    }));

    assert!(panic.is_err());
}

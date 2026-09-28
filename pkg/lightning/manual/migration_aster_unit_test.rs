// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Lightning `manual` 包迁移对齐单元测试。
//
// 核对清零分配、`Free` 接受零长度切片、Allocator 引用计数泄漏检测，
// 以及无 cgo 回退路径对负长度的 panic 行为。

use std::sync::{
    Arc,
    atomic::{AtomicI64, Ordering},
};

use super::{Allocator, Free, New, manual_nocgo};

/// 验证 New 返回长度=容量且全零的缓冲，Free 可安全接收。
#[test]
fn new_returns_zeroed_memory_and_free_accepts_it() {
    let bytes = New(32);
    assert_eq!(bytes.len(), 32);
    assert_eq!(bytes.capacity(), 32);
    assert!(bytes.iter().all(|byte| *byte == 0));
    Free(bytes);

    let empty = New(0);
    assert!(empty.is_empty());
    assert_eq!(empty.capacity(), 0);
    Free(empty);
}

/// 验证截断后仍保留后备存储的零长度切片也能 Free（对齐 Go 可释放非 nil 空切片）。
#[test]
fn free_accepts_a_zero_length_slice_with_backing_storage() {
    let mut bytes = New(16);
    bytes.truncate(0);
    assert_eq!(bytes.capacity(), 16);
    Free(bytes);
}

/// 验证启用 RefCnt 时 Alloc/Free 增减计数，未归零时 CheckRefCnt 报泄漏。
#[test]
fn allocator_tracks_allocations_and_reports_leaks() {
    let ref_cnt = Arc::new(AtomicI64::new(0));
    let allocator = Allocator {
        RefCnt: Some(Arc::clone(&ref_cnt)),
    };

    let bytes = allocator.Alloc(8);
    assert_eq!(ref_cnt.load(Ordering::Relaxed), 1);
    assert_eq!(
        allocator.CheckRefCnt(),
        Err("memory leak detected, refCnt: 1".to_owned())
    );

    allocator.Free(bytes);
    assert_eq!(ref_cnt.load(Ordering::Relaxed), 0);
    assert_eq!(allocator.CheckRefCnt(), Ok(()));
}

/// 验证 RefCnt 为 None 时保持 Go 零值：不计数，CheckRefCnt 恒成功。
#[test]
fn allocator_without_counter_keeps_the_go_zero_value_behavior() {
    let allocator = Allocator { RefCnt: None };
    let bytes = allocator.Alloc(4);
    assert_eq!(bytes, vec![0; 4]);
    allocator.Free(bytes);
    assert_eq!(allocator.CheckRefCnt(), Ok(()));
}

/// 验证 nocgo 回退路径同样分配清零内存并可 Free。
#[test]
fn nocgo_fallback_allocates_zeroed_memory() {
    let bytes = manual_nocgo::New(7);
    assert_eq!(bytes, vec![0; 7]);
    manual_nocgo::Free(bytes);
}

/// 验证 nocgo 对负长度 panic，错误信息与 Go makeslice 一致。
#[test]
#[should_panic(expected = "makeslice: len out of range")]
fn nocgo_fallback_rejects_negative_lengths_like_go() {
    let _ = manual_nocgo::New(-1);
}

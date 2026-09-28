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

// `BoundedHandleSet` / `HandleFilter` 的单元测试。
//
// 验证有界句柄集合的容量限制、跨集合共享计数、合并（Merge）
// 以及过滤器对已收录 Handle 的跳过语义，与 Go 侧 `row_handle_test.go` 对齐。

// 自 Go 包同名测试移植。
// Ported from pkg/dxf/importinto/conflictedkv/row_handle_test.go.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use astersql_kv::IntHandle;

use crate::{BoundedHandleSet, HandleFilter, NewBoundedHandleSet, NewHandleFilter};

/// Go `unsafe.Sizeof("") + unsafe.Sizeof(true)` 在 64 位平台的精确值。
///
/// 这里故意不从 Rust 生产实现重算，避免把 `String` 的三字长布局
/// 误当成 Go 两字长 string header，从而提前耗尽共享内存预算。
fn entry_shallow_size() -> i64 {
    17
}

#[test]
/// 过滤器：无集合时不跳过；加入句柄后仅跳过已收录者。
fn test_handle_filter() {
    // `None` 过滤器（对应 Go 的 nil *HandleFilter）不得跳过任何句柄。
    // A `None` filter (Go's nil *HandleFilter) must never skip anything.
    let no_filter: Option<HandleFilter> = None;
    assert!(
        !no_filter
            .as_ref()
            .is_some_and(|filter| filter.needSkip(&IntHandle(1)))
    );

    let shared_size = Arc::new(AtomicI64::new(0));
    let mut set = NewBoundedHandleSet(shared_size, 1024);
    set.Add(&IntHandle(1));
    let filter = NewHandleFilter(Arc::new(set));

    assert!(filter.needSkip(&IntHandle(1)));
    assert!(!filter.needSkip(&IntHandle(2)));
}

#[test]
/// 有界集合：填满后 Add 静默失败；Merge(None) 空操作；Merge(Some) 拷贝句柄。
fn test_bounded_handle_set() {
    let shared_size = Arc::new(AtomicI64::new(0));
    let limit = 3 * (1 + entry_shallow_size());
    let mut set = NewBoundedHandleSet(shared_size.clone(), limit);

    assert!(!set.Contains(&IntHandle(1)));
    for i in 0..3_i64 {
        set.Add(&IntHandle(i + 1));
        assert!(set.Contains(&IntHandle(i + 1)));
    }
    assert_eq!(limit, shared_size.load(Ordering::Acquire));
    assert!(set.BoundExceeded());

    // 超限后的 Add 为静默空操作。
    // Adding beyond the bound is a silent no-op.
    set.Add(&IntHandle(4));
    assert!(!set.Contains(&IntHandle(4)));

    let mut set2 = NewBoundedHandleSet(shared_size.clone(), limit);
    assert!(set2.BoundExceeded());
    set2.Add(&IntHandle(5));
    assert!(!set2.Contains(&IntHandle(5)));

    // 合并 `None` 集合为空操作。
    // Merging a `None` set is a no-op.
    set2.Merge(None);
    assert_eq!(0, set2.Len());

    set2.Merge(Some(&set));
    assert_eq!(set.Len(), set2.Len());
    for i in 0..3_i64 {
        assert!(set2.Contains(&IntHandle(i + 1)));
    }
}

#[test]
/// 共享计数未达上限时，新建集合仍可接受条目（容量预检回归）。
fn test_bounded_handle_set_new_below_capacity_starts_with_room() {
    // 共享计数低于限制时，新建集合应仍能接受条目
    // （回归守护 `NewBoundedHandleSet` 内的容量预检）。
    // When the shared counter is already below the limit, a freshly created
    // set should still be able to accept entries (regression guard for the
    // capacity pre-check inside `NewBoundedHandleSet`).
    let shared_size = Arc::new(AtomicI64::new(0));
    let set = NewBoundedHandleSet(shared_size, 64);
    assert_eq!(0, set.Len());
    assert_eq!(0, set.SharedSize());
    assert!(!set.BoundExceeded());
}

#[test]
/// 两个集合共享同一原子计数时彼此可见增长，模拟生产中本地与全局集合协作。
fn test_bounded_handle_set_shared_across_sets() {
    // 共享计数器的两个集合互相观察增长，对应生产中
    // 各 collector 本地集合与全局集合的交互方式。
    // Two sets sharing the same counter observe each other's growth, mirroring
    // how per-collector local sets and the shared global set interact in
    // production.
    let shared_size = Arc::new(AtomicI64::new(0));
    let limit = 2 * (1 + entry_shallow_size());
    let mut a: BoundedHandleSet = NewBoundedHandleSet(shared_size.clone(), limit);
    let mut b = NewBoundedHandleSet(shared_size.clone(), limit);

    a.Add(&IntHandle(1));
    assert!(!a.BoundExceeded());
    assert!(!b.BoundExceeded());

    b.Add(&IntHandle(2));
    assert!(a.BoundExceeded());
    assert!(b.BoundExceeded());

    // 共享上限被突破后，任一集合上的后续 Add 都静默丢弃。
    // Once the shared bound is exceeded, further adds on either set are
    // dropped silently.
    a.Add(&IntHandle(3));
    assert!(!a.Contains(&IntHandle(3)));
}

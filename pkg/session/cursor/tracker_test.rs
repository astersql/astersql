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

// 游标跟踪器的单元测试。
//
// 覆盖新建/查询/遍历/关闭以及并发创建与删除路径，对齐 Go `tracker_test`。

#![allow(non_snake_case)]

use super::state::State;
use super::tracker::{NewTracker, Tracker};
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

/// 验证连续新建游标时 ID 从 1、2 递增。
#[test]
fn TestNewCursor() {
    let tracker = NewTracker();
    let cursor = tracker.NewCursor(State::default());
    assert_eq!(cursor.ID(), 1);

    let cursor2 = tracker.NewCursor(State::default());
    assert_eq!(cursor2.ID(), 2);
}

/// Go 将递增后的 `int64` 转为 `int`；在 64 位平台上越界按补码回绕。
#[test]
fn TestNewCursorIDWrapsLikeGoAtomicInt64() {
    let tracker = NewTracker();
    tracker.set_id_alloc_for_test(i64::MAX);

    let cursor = tracker.NewCursor(State::default());

    assert_eq!(cursor.ID(), i64::MIN);
}

/// 验证 `GetCursor` 返回与 `NewCursor` 相同的 `Arc` 句柄。
#[test]
fn TestGetCursor() {
    let tracker = NewTracker();
    let new_cursor = tracker.NewCursor(State::default());
    let retrieved_cursor = tracker
        .GetCursor(new_cursor.ID())
        .expect("new cursor must be present in the tracker");

    assert!(Arc::ptr_eq(&new_cursor, &retrieved_cursor));
}

/// 验证 `RangeCursor` 会回调到已存在游标，且返回 `false` 可中断。
#[test]
fn TestRangeCursor() {
    let tracker = NewTracker();
    tracker.NewCursor(State::default());

    let mut called = false;
    tracker.RangeCursor(|cursor| {
        called = true;
        assert_eq!(cursor.ID(), 1);
        false
    });

    assert!(called);
}

/// 验证 `Close` 后按 ID 查询返回 `None`。
#[test]
fn TestCursorHandleClose() {
    let tracker = NewTracker();
    let cursor = tracker.NewCursor(State::default());
    let id = cursor.ID();
    cursor.Close();

    assert!(tracker.GetCursor(id).is_none());
}

/// 并发创建与遍历关闭应在压力下保持安全。
#[test]
fn TestCursorTrackerConcurrentCreateDelete() {
    const CREATOR_THREADS: usize = 8;
    const CLOSER_THREADS: usize = 8;
    const ROUNDS: usize = 16;

    let tracker = NewTracker();
    let round_start = Arc::new(Barrier::new(CREATOR_THREADS + CLOSER_THREADS));
    let round_end = Arc::new(Barrier::new(CREATOR_THREADS + CLOSER_THREADS));
    let range_count = Arc::new(AtomicUsize::new(0));
    let close_count = Arc::new(AtomicUsize::new(0));
    let mut creators = Vec::with_capacity(CREATOR_THREADS);
    let mut closers = Vec::with_capacity(CLOSER_THREADS);

    // 每轮先创建一个种子 cursor，保证屏障放行后至少一次 Range 能观察到数据；
    // 随后的 NewCursor 与 Range/Close 在同一轮真实并发。
    for _ in 0..CREATOR_THREADS {
        let tracker = Arc::clone(&tracker);
        let round_start = Arc::clone(&round_start);
        let round_end = Arc::clone(&round_end);
        creators.push(thread::spawn(move || {
            let mut ids = Vec::with_capacity(ROUNDS * 2);
            for _ in 0..ROUNDS {
                ids.push(tracker.NewCursor(State::default()).ID());
                round_start.wait();
                ids.push(tracker.NewCursor(State::default()).ID());
                round_end.wait();
            }
            ids
        }));
    }

    for _ in 0..CLOSER_THREADS {
        let tracker = Arc::clone(&tracker);
        let round_start = Arc::clone(&round_start);
        let round_end = Arc::clone(&round_end);
        let range_count = Arc::clone(&range_count);
        let close_count = Arc::clone(&close_count);
        closers.push(thread::spawn(move || {
            for _ in 0..ROUNDS {
                round_start.wait();
                range_count.fetch_add(1, Ordering::Relaxed);
                tracker.RangeCursor(|cursor| {
                    cursor.Close();
                    close_count.fetch_add(1, Ordering::Relaxed);
                    true
                });
                round_end.wait();
            }
        }));
    }

    let mut ids = Vec::with_capacity(CREATOR_THREADS * ROUNDS * 2);
    for creator in creators {
        ids.extend(creator.join().expect("cursor creator must not panic"));
    }
    for closer in closers {
        closer.join().expect("cursor closer must not panic");
    }

    assert_eq!(ids.len(), CREATOR_THREADS * ROUNDS * 2);
    assert_eq!(ids.iter().copied().collect::<HashSet<_>>().len(), ids.len());
    assert_eq!(range_count.load(Ordering::Relaxed), CLOSER_THREADS * ROUNDS);
    assert!(close_count.load(Ordering::Relaxed) >= ROUNDS);

    tracker.RangeCursor(|cursor| {
        cursor.Close();
        true
    });
    assert_eq!(tracker.cursor_count_for_test(), 0);
}

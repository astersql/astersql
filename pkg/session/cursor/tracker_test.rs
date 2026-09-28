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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

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
    let tracker = NewTracker();
    let stop = Arc::new(AtomicBool::new(false));
    let mut threads = Vec::new();
    let threads_for_each_operation = 100;

    // 创建侧：持续 NewCursor。
    for _ in 0..threads_for_each_operation {
        let tracker = Arc::clone(&tracker);
        let stop = Arc::clone(&stop);
        threads.push(thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                tracker.NewCursor(State::default());
            }
        }));
    }

    // 删除侧：RangeCursor 中 Close 全部句柄。
    for _ in 0..threads_for_each_operation {
        let tracker = Arc::clone(&tracker);
        let stop = Arc::clone(&stop);
        threads.push(thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                tracker.RangeCursor(|cursor| {
                    cursor.Close();
                    true
                });
            }
        }));
    }

    thread::sleep(Duration::from_secs(2));
    stop.store(true, Ordering::Release);
    for thread in threads {
        thread.join().expect("cursor worker must not panic");
    }
}

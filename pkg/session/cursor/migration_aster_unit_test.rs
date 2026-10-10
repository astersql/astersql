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

// 游标子系统的 Aster 迁移单元测试。
//
// 通过本地 `path` 挂接 `state`/`tracker`，验证 ID 分配、句柄同一性、
// 遍历中断、关闭幂等以及并发创建/删除的线程安全，对齐 Go 侧行为。

#[path = "state.rs"]
mod state;
#[path = "tracker.rs"]
mod tracker;

#[cfg(test)]
mod tests {
    use super::state::State;
    use super::tracker::{NewTracker, Tracker};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier};

    /// 验证新建游标的 ID 从 1 递增，且状态与创建时传入的 StartTS 一致。
    #[test]
    fn state_and_cursor_ids_match_go_behavior() {
        let tracker = NewTracker();
        let first = tracker.NewCursor(State { StartTS: 42 });
        let second = tracker.NewCursor(State { StartTS: 84 });

        assert_eq!(first.ID(), 1);
        assert_eq!(second.ID(), 2);
        assert_eq!(first.GetState(), State { StartTS: 42 });
        assert_eq!(second.GetState(), State { StartTS: 84 });
    }

    /// 验证按 ID 取回的句柄与创建时为同一 `Arc`，缺失 ID 返回 `None`。
    #[test]
    fn get_cursor_returns_the_same_handle_and_missing_is_none() {
        let tracker = NewTracker();
        let cursor = tracker.NewCursor(State { StartTS: 1 });

        let retrieved = tracker.GetCursor(cursor.ID()).expect("cursor must exist");
        assert!(Arc::ptr_eq(&cursor, &retrieved));
        assert!(tracker.GetCursor(99).is_none());
    }

    /// 验证 `RangeCursor` 在回调返回 `false` 时立即停止后续遍历。
    #[test]
    fn range_cursor_stops_when_callback_returns_false() {
        let tracker = NewTracker();
        tracker.NewCursor(State { StartTS: 1 });
        tracker.NewCursor(State { StartTS: 2 });

        let mut calls = 0;
        tracker.RangeCursor(|_| {
            calls += 1;
            false
        });

        assert_eq!(calls, 1);
    }

    /// 验证关闭游标后无法再按 ID 取回，且重复 `Close` 幂等。
    #[test]
    fn close_removes_cursor_and_is_idempotent() {
        let tracker = NewTracker();
        let cursor = tracker.NewCursor(State { StartTS: 7 });
        let id = cursor.ID();

        cursor.Close();
        cursor.Close();

        assert!(tracker.GetCursor(id).is_none());
    }

    /// 并发创建与遍历删除不应引发数据竞争或 panic。
    #[test]
    fn concurrent_create_and_range_delete_is_safe() {
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

        // 每轮在屏障前放入种子 cursor；屏障后创建与遍历关闭真实并发。
        for _ in 0..CREATOR_THREADS {
            let tracker = Arc::clone(&tracker);
            let round_start = Arc::clone(&round_start);
            let round_end = Arc::clone(&round_end);
            creators.push(std::thread::spawn(move || {
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
            closers.push(std::thread::spawn(move || {
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
            ids.extend(creator.join().expect("cursor creator panicked"));
        }
        for closer in closers {
            closer.join().expect("cursor closer panicked");
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
}

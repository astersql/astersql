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
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

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
        const THREADS_FOR_EACH_OPERATION: usize = 100;
        let tracker = NewTracker();
        let stop = Arc::new(AtomicBool::new(false));
        let mut workers = Vec::with_capacity(THREADS_FOR_EACH_OPERATION * 2);

        // 一组线程持续新建游标。
        for _ in 0..THREADS_FOR_EACH_OPERATION {
            let tracker = Arc::clone(&tracker);
            let stop = Arc::clone(&stop);
            workers.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    tracker.NewCursor(State::default());
                }
            }));
        }

        // 另一组线程遍历并关闭游标，模拟协议层清理。
        for _ in 0..THREADS_FOR_EACH_OPERATION {
            let tracker = Arc::clone(&tracker);
            let stop = Arc::clone(&stop);
            workers.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    tracker.RangeCursor(|cursor| {
                        cursor.Close();
                        true
                    });
                }
            }));
        }

        std::thread::sleep(Duration::from_secs(2));
        stop.store(true, Ordering::Release);
        for worker in workers {
            worker.join().expect("cursor worker panicked");
        }
    }
}

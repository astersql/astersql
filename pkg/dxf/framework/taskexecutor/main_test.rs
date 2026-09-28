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

// taskexecutor 包级测试辅助（对应 Go `TestMain`/`ReduceCheckInterval`）。
//
// Rust 无包级 `TestMain`；本文件提供缩短全局轮询间隔的 RAII 守卫，
// 避免 Manager/Executor 轮询拖慢测试。

// Ported from pkg/dxf/framework/taskexecutor/main_test.go. Go's `TestMain`
// only wires up `testsetup.SetupForCommonTest()` and a `goleak` allowlist
// around the whole `go test` binary; neither concept (a package-wide test
// entry point, or goroutine-leak detection) exists for `cargo test`, which
// runs every `#[test]` in its own harness without a package-level `main`,
// so there is nothing faithful to port for that half of the file.
//
// `ReduceCheckInterval` *does* have a direct, testable Rust equivalent: it
// mirrors Go's package-level `var` reassignment by temporarily shrinking the
// atomically stored intervals (see `SetTaskCheckIntervalForTest` in
// manager.rs and `SetSubtaskCheckIntervalForTest` in task_executor.rs) and
// restoring the originals once the caller is done, exactly like Go's
// `t.Cleanup` restoring the backed-up values.

// 引入可被测试缩短的全局轮询间隔。
use crate::{
    MaxSubtaskCheckInterval, SetSubtaskCheckIntervalForTest, SetTaskCheckIntervalForTest,
    SubtaskCheckInterval, TaskCheckInterval,
};
use std::time::Duration;

/// RAII guard returned by [`ReduceCheckInterval`]; restores the original
/// interval values on drop, mirroring Go's `t.Cleanup(func() { ... })`.
/// 缩短轮询间隔的 RAII 守卫；Drop 时恢复原值。
/// 对应 Go 的 `t.Cleanup`。
pub struct CheckIntervalGuard {
    /// 原 `TaskCheckInterval`。
    task: Duration,
    /// 原 `SubtaskCheckInterval`。
    subtask: Duration,
    /// 原 `MaxSubtaskCheckInterval`。
    max_subtask: Duration,
}
/// 退出作用域时写回原间隔。
impl Drop for CheckIntervalGuard {
    fn drop(&mut self) {
        SetTaskCheckIntervalForTest(self.task);
        SetSubtaskCheckIntervalForTest(self.subtask, self.max_subtask);
    }
}

/// ReduceCheckInterval shrinks `TaskCheckInterval`, `SubtaskCheckInterval`,
/// and `MaxSubtaskCheckInterval` to one millisecond each, matching Go's
/// `ReduceCheckInterval(t *testing.T)` helper used by other test files in
/// this package to keep polling loops from slowing the test suite down.
/// 将三类检查间隔缩到 1ms，加快测试中的轮询循环。
pub fn ReduceCheckInterval() -> CheckIntervalGuard {
    let task = SetTaskCheckIntervalForTest(Duration::from_millis(1));
    let (subtask, max_subtask) =
        SetSubtaskCheckIntervalForTest(Duration::from_millis(1), Duration::from_millis(1));
    CheckIntervalGuard {
        task,
        subtask,
        max_subtask,
    }
}

#[test]
/// 验证守卫持有期间间隔为 1ms。
fn test_reduce_check_interval_shrinks_intervals() {
    // These intervals are process-global atomics shared with every other
    // test in this crate that also calls `ReduceCheckInterval`-style
    // helpers (see `task_executor_test.rs`'s `new_env`), all of which only
    // ever shrink them to one millisecond. So asserting the shrunk value
    // while the guard is held is race-free; asserting an exact restore to
    // whatever value happened to be active before this specific test ran
    // would not be, since a concurrently running test could legitimately
    // be holding its own one-millisecond shrink at that moment.
    let _guard = ReduceCheckInterval();
    assert_eq!(Duration::from_millis(1), TaskCheckInterval());
    assert_eq!(Duration::from_millis(1), SubtaskCheckInterval());
    assert_eq!(Duration::from_millis(1), MaxSubtaskCheckInterval());
}

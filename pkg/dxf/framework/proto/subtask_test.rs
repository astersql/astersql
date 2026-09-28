// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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
//
// 子任务状态终态判断与 `Allocatable` 并发分配的单元测试。

use super::*;
use std::sync::Arc;
use std::thread;

/// 仅 succeed/failed/canceled 视为完成；pending/running/paused 否。
#[test]
fn test_subtask_is_done() {
    let cases = [
        (SubtaskStatePending, false),
        (SubtaskStateRunning, false),
        (SubtaskStateSucceed, true),
        (SubtaskStateFailed, true),
        (SubtaskStatePaused, false),
        (SubtaskStateCanceled, true),
    ];
    for (state, done) in cases {
        let mut subtask = NewSubtask(StepInit, 0, TaskTypeExample, String::new(), 0, vec![], 0);
        subtask.State = state;
        assert_eq!(subtask.IsDone(), done);
    }
}

/// 覆盖容量边界、Alloc/Free，以及多线程 CAS 分配后 used 归零。
#[test]
fn test_allocatable() {
    let allocatable = Arc::new(NewAllocatable(123456));
    assert_eq!(allocatable.Capacity(), 123456);
    assert_eq!(allocatable.Used(), 0);

    // 超过容量应失败且不改变 used。
    assert!(!allocatable.Alloc(123457));
    assert_eq!(allocatable.Used(), 0);
    assert!(allocatable.Alloc(123));
    assert_eq!(allocatable.Used(), 123);
    allocatable.Free(123);
    assert_eq!(allocatable.Used(), 0);

    // 多 worker 并发 Alloc/Free，最终 used 必须回到 0。
    let mut handles = Vec::with_capacity(10);
    for worker in 0..10_i64 {
        let allocatable = Arc::clone(&allocatable);
        handles.push(thread::spawn(move || {
            for iteration in 0..10_000_i64 {
                let n = (worker * 997 + iteration * 37) % 1000;
                if allocatable.Alloc(n) {
                    allocatable.Free(n);
                }
            }
        }));
    }
    for handle in handles {
        handle.join().expect("allocation worker panicked");
    }
    assert_eq!(allocatable.Used(), 0);
}

/// Go 的 atomic.Int64 算术按二补数回绕，边界输入不能在 debug 构建中 panic。
#[test]
fn test_allocatable_integer_boundary_matches_go() {
    let allocatable = NewAllocatable(i64::MAX);
    assert!(allocatable.Alloc(i64::MAX));
    assert!(allocatable.Alloc(1));
    assert_eq!(allocatable.Used(), i64::MIN);

    let allocatable = NewAllocatable(0);
    allocatable.Free(i64::MIN);
    assert_eq!(allocatable.Used(), i64::MIN);
}

/// docker/go-units BytesSize 使用完整二进制单位表和四位有效数字。
#[test]
fn test_step_resource_string_matches_go_bytes_size() {
    let cases = [
        (1234, "[CPU=1, Mem=1.205KiB]"),
        (1_i64 << 60, "[CPU=1, Mem=1EiB]"),
        (i64::MAX, "[CPU=1, Mem=8EiB]"),
        (-99999, "[CPU=1, Mem=-1e+05B]"),
        (-123456, "[CPU=1, Mem=-1.235e+05B]"),
    ];

    for (memory, expected) in cases {
        let resource = StepResource {
            CPU: NewAllocatable(1),
            Mem: NewAllocatable(memory),
        };
        assert_eq!(resource.String(), expected);
    }
}

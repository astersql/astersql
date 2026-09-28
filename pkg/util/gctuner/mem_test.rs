// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// `mem::readMemoryInuse` 行为测试。
//
// 验证大块分配后占用统计可见，且使用与 Go `HeapInuse` 对应的包级权威统计。

use crate::mem::{readMemoryInuse, releaseUnusedMemory};
use task_memory::memstats::ReadMemStats;

/// Process heap samples must not race other tests' allocations or finalizers.
pub(crate) fn run_in_isolated_process(test: &str) -> bool {
    if std::env::var("ASTERSQL_GCTUNER_MEMORY_TEST").as_deref() == Ok(test) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env("ASTERSQL_GCTUNER_MEMORY_TEST", test)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

/// 分配约 100MB 并按页写入，断言 `readMemoryInuse` 至少增长约 95MB。
#[test]
fn test_mem() {
    if run_in_isolated_process("mem_test::test_mem") {
        return;
    }
    const MB: usize = 1024 * 1024;

    let before = readMemoryInuse();
    let mut heap = vec![0_u8; 100 * MB + 1];
    // Touch each page so both allocator-backed and RSS fallback implementations
    // observe the complete allocation on supported platforms.
    // 按页触碰，使分配器统计与 RSS 回退路径都能观察到完整分配。
    for byte in heap.iter_mut().step_by(4096) {
        *byte = 1;
    }
    heap[100 * MB] = 1;

    let inuse = readMemoryInuse();
    println!("memory in use: {} MB", inuse / MB as u64);
    assert!(
        inuse >= before.saturating_add(95 * MB as u64),
        "expected the 100 MB allocation to be visible: before={before}, after={inuse}"
    );

    // 防止编译器优化掉堆缓冲。
    std::hint::black_box(&heap);
}

/// `readMemoryInuse` 必须直接采用 memory 包的 `HeapInuse` 语义，不能误报 `HeapAlloc`。
#[test]
fn memory_inuse_matches_force_read_mem_stats() {
    if run_in_isolated_process("mem_test::memory_inuse_matches_force_read_mem_stats") {
        return;
    }
    // ForceReadMemStats stores the exact sample returned by readMemoryInuse.
    // Comparing a second fresh sample would race allocator activity.
    assert_eq!(readMemoryInuse(), ReadMemStats().heap_inuse);
}

/// 受支持平台必须接入真实系统分配器回收接口，不能只更新进程内原子状态。
#[test]
fn allocator_collection_boundary_is_connected() {
    let supported = releaseUnusedMemory();
    if cfg!(any(
        target_os = "macos",
        all(target_os = "linux", target_env = "gnu")
    )) {
        assert!(supported);
    }
}

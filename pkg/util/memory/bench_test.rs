// Copyright 2018 PingCAP, Inc.
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

// 内存工具微基准：探测 MemTotal/MemUsed 读取与 Tracker.Consume 并发开销。
//
// 对应 Go `bench_test.go`；此处用单元测试外壳驱动少量迭代，便于迁移期冒烟。

use super::meminfo::{MemTotal, MemUsed};
use super::tracker::{NewTracker, Tracker};
use std::sync::{Arc, Barrier};

/// 反复调用全局 `MemTotal` 函数指针，测量系统总内存查询路径。
fn benchmark_mem_total(iterations: usize) {
    for _ in 0..iterations {
        let _ = (*MemTotal.read().expect("MemTotal lock poisoned"))();
    }
}

/// 反复调用全局 `MemUsed` 函数指针，测量已用内存查询路径。
fn benchmark_mem_used(iterations: usize) {
    for _ in 0..iterations {
        let _ = (*MemUsed.read().expect("MemUsed lock poisoned"))();
    }
}

/// 多 worker 子 Tracker 挂到父节点后并发 Consume，再 Detach。
fn benchmark_consume(iterations: usize, workers: usize) -> (i64, String) {
    let mut tracker = NewTracker(1, -1);
    // 将父 Tracker 地址转为 usize 以便跨线程 AttachTo（与 Go 指针挂接语义对应）。
    let tracker_address = (&mut *tracker as *mut Tracker) as usize;
    let consumed = iterations as i64 * workers as i64 * (256 << 20);
    let ready = Arc::new(Barrier::new(workers + 1));
    let release = Arc::new(Barrier::new(workers + 1));
    let mut snapshot = String::new();
    std::thread::scope(|scope| {
        for _worker in 0..workers {
            let ready = Arc::clone(&ready);
            let release = Arc::clone(&release);
            scope.spawn(move || {
                let mut child = NewTracker(2, -1);
                child.AttachTo(tracker_address as *mut Tracker);
                for _ in 0..iterations {
                    child.Consume(256 << 20);
                }
                ready.wait();
                release.wait();
                child.Detach();
            });
        }
        ready.wait();
        snapshot = tracker.String();
        assert_eq!(tracker.BytesConsumed(), consumed);
        release.wait();
    });
    (consumed, snapshot)
}

/// 冒烟跑 MemTotal 基准（少量迭代）。
#[test]
fn BenchmarkMemTotal() {
    benchmark_mem_total(8);
}

/// 冒烟跑 MemUsed 基准（少量迭代）。
#[test]
fn BenchmarkMemUsed() {
    benchmark_mem_used(8);
}

/// 冒烟跑多线程 Consume 基准。
#[test]
fn BenchmarkConsume() {
    let (consumed, snapshot) = benchmark_consume(4, 4);
    assert_eq!(consumed, 4_i64 * 4 * (256 << 20));
    assert_eq!(snapshot.matches("\"2\"{").count(), 4);
    assert!(!snapshot.contains("\"3\"{"));
}

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

// 语句摘要 `Add` 路径的有界冒烟基准（对应 Go benchmark）。
//
// 在稳定 Rust 上跑与 Go 基准相同的累加路径：单线程、并行同 digest、并行多 digest。

use std::thread;
use task_stmtsummary_v2::{GenerateStmtExecInfo4Test, NewStmtSummary4Test};

/// 每个 worker / 单线程循环中的 `Add` 次数。
const ITERATIONS: usize = 1_000;
/// 并行基准的工作线程数。
const WORKERS: usize = 8;

// Bounded smoke counterparts execute the same Add paths as the Go benchmarks on stable Rust.
/// 单线程反复 `Add` 同一 digest，窗口长度应为 1。
#[test]
fn benchmark_stmt_summary_add_single_workload() {
    let summary = NewStmtSummary4Test(1000);
    let info = GenerateStmtExecInfo4Test("digest_test");
    for _ in 0..ITERATIONS {
        summary.Add(&info);
    }
    assert_eq!(summary.Len(), 1);
    summary.Close();
}

/// 多线程并行 `Add` 同一 digest；因 `StmtExecInfo` 含非 Send 对象，fixture 在线程内构造。
#[test]
fn benchmark_stmt_summary_add_parallel_single_workload() {
    let summary = NewStmtSummary4Test(1000);
    // StmtExecInfo intentionally contains non-Send trait objects in the Rust port,
    // so workers construct the selected fixture after crossing the thread boundary.
    // 每个 worker 使用相同 digest，最终 Len 仍为 1。
    let digests: Vec<_> = (0..1000).map(|_| "digest_test".to_owned()).collect();
    let mut workers = Vec::new();
    for (worker, digest) in digests.into_iter().take(WORKERS).enumerate() {
        let summary = std::sync::Arc::clone(&summary);
        workers.push(thread::spawn(move || {
            let info = GenerateStmtExecInfo4Test(digest);
            for _ in 0..ITERATIONS / WORKERS {
                summary.Add(&info);
            }
            worker
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(summary.Len(), 1);
    summary.Close();
}

/// 多线程并行 `Add` 不同 digest，期望窗口中保留 WORKERS 条记录。
#[test]
fn benchmark_stmt_summary_add_parallel_multi_workload() {
    let summary = NewStmtSummary4Test(1000);
    // 为每个 worker 分配唯一 digest，验证并发写入多键路径。
    let digests: Vec<_> = (0..1000)
        .map(|index| format!("digest_test_{index}"))
        .collect();
    let mut workers = Vec::new();
    for digest in digests.into_iter().take(WORKERS) {
        let summary = std::sync::Arc::clone(&summary);
        workers.push(thread::spawn(move || {
            let info = GenerateStmtExecInfo4Test(digest);
            for _ in 0..ITERATIONS / WORKERS {
                summary.Add(&info);
            }
        }));
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(summary.Len(), WORKERS);
    summary.Close();
}

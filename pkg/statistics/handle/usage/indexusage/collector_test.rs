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

// 索引使用率采集器单元测试。
//
// 覆盖访问比例分桶边界、会话累加与 Flush、并发 Report/Flush 一致性，
// 以及语句级 QueryTotal 去重；另保留对齐 Go 的基准负载辅助函数。

use crate::{
    Collector, GlobalIndexID, NewCollector, NewSample, NewStmtIndexUsageCollector, Sample,
};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// 在约 1 秒内轮询直到谓词为真（等待异步 worker 合并完成）。
fn require_eventually(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(1));
    }
    assert!(
        predicate(),
        "condition did not become true within one second"
    );
}

/// 断言指定表/索引的使用样本各字段与期望一致。
fn assert_usage(
    collector: &Collector,
    table_id: i64,
    index_id: i64,
    query_total: u64,
    kv_req_total: u64,
    row_access_total: u64,
    percentage_access: [u64; 7],
) {
    require_eventually(|| {
        let usage = collector.GetIndexUsage(table_id, index_id);
        usage.QueryTotal == query_total
            && usage.KvReqTotal == kv_req_total
            && usage.RowAccessTotal == row_access_total
            && usage.PercentageAccess == percentage_access
    });
    let usage = collector.GetIndexUsage(table_id, index_id);
    assert_eq!(usage.QueryTotal, query_total);
    assert_eq!(usage.KvReqTotal, kv_req_total);
    assert_eq!(usage.RowAccessTotal, row_access_total);
    assert_eq!(usage.PercentageAccess, percentage_access);
}

// TestGetBucket: cover the same eleven boundaries through NewSample's observable histogram.
/// 校验 NewSample 在十一组边界上的直方图分桶与 Go 一致。
#[test]
fn test_get_bucket() {
    let test_cases = [
        (0, 1, 0),
        (5, 1000, 1),
        (1, 100, 2),
        (5, 100, 2),
        (1, 10, 3),
        (15, 100, 3),
        (1, 5, 4),
        (2, 5, 4),
        (1, 2, 5),
        (7, 10, 5),
        (1, 1, 6),
    ];
    for (row_access, total_rows, expected_bucket) in test_cases {
        let sample = NewSample(0, 0, row_access, total_rows);
        let mut expected = [0; 7];
        expected[expected_bucket] = 1;
        assert_eq!(sample.PercentageAccess, expected);
    }
}

// TestUpdateIndex: consecutive updates accumulate query, KV, row and bucket totals.
/// 连续 Update+Flush 后查询/KV/行数与分桶计数应正确累加。
#[test]
fn test_update_index() {
    let global_collector = NewCollector();
    global_collector.StartWorker();
    let collector = global_collector.SpawnSessionCollector();

    collector.Update(1, 1, NewSample(1, 1, 1, 1));
    collector.Flush();
    assert_usage(&global_collector, 1, 1, 1, 1, 1, [0, 0, 0, 0, 0, 0, 1]);

    collector.Update(1, 1, NewSample(10, 10, 5, 50));
    collector.Flush();
    assert_usage(&global_collector, 1, 1, 11, 11, 6, [0, 0, 0, 1, 0, 0, 1]);

    collector.Update(1, 1, NewSample(10, 10, 5, 0));
    collector.Flush();
    assert_usage(&global_collector, 1, 1, 21, 21, 11, [0, 0, 0, 1, 0, 0, 2]);
    global_collector.Close();
}

/// 并发测试中的单次操作：样本、目标索引、是否穿插 Report。
#[derive(Clone)]
struct TestOp {
    info: Sample,
    idx: GlobalIndexID,
    report: bool,
}

/// 按固定种子生成可复现的随机索引使用操作。
struct TestOpGenerator {
    table_count: i64,
    index_per_table_count: i64,
    max_query_total: u64,
    max_kv_req_total: u64,
    max_table_total_rows: u64,
    rng: StdRng,
}

impl TestOpGenerator {
    /// 使用给定种子初始化生成器。
    fn new(seed: u64) -> Self {
        Self {
            table_count: 10,
            index_per_table_count: 10,
            max_query_total: 10_000,
            max_kv_req_total: 10_000,
            max_table_total_rows: 10_000,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    /// 生成一条带单调 LastUsedAt 的随机操作。
    fn generate_test_op(&mut self, sequence: u64) -> TestOp {
        let idx = GlobalIndexID {
            TableID: self.rng.gen_range(0..self.table_count),
            IndexID: self.rng.gen_range(0..self.index_per_table_count),
        };
        let query_total = self.rng.gen_range(0..self.max_query_total);
        let kv_req_total = self.rng.gen_range(0..self.max_kv_req_total);
        let total_rows = self.rng.gen_range(0..self.max_table_total_rows);
        let row_access = if total_rows == 0 {
            0
        } else {
            self.rng.gen_range(0..total_rows)
        };
        let mut info = NewSample(query_total, kv_req_total, row_access, total_rows);
        info.LastUsedAt = UNIX_EPOCH + Duration::from_nanos(sequence);
        TestOp {
            info,
            idx,
            report: self.rng.gen_range(0..4) == 1,
        }
    }
}

// TestFlushConcurrentIndexCollector: 64 sessions and 100000 operations per session,
// matching the Go test's load and random Report/Flush interleaving.
/// 64 会话并发累加后结果应与串行聚合完全一致。
#[test]
fn test_flush_concurrent_index_collector() {
    const SESSION_COUNT: usize = 64;
    const OP_PER_SESSION: usize = 100_000;

    let expected = NewCollector();
    expected.StartWorker();
    let expected_session = expected.SpawnSessionCollector();
    for session_id in 0..SESSION_COUNT {
        let mut generator = TestOpGenerator::new(session_id as u64 + 1);
        for op_id in 0..OP_PER_SESSION {
            let sequence = (session_id * OP_PER_SESSION + op_id + 1) as u64;
            let op = generator.generate_test_op(sequence);
            expected_session.Update(op.idx.TableID, op.idx.IndexID, op.info);
        }
    }
    expected_session.Flush();

    let actual = Arc::new(NewCollector());
    actual.StartWorker();
    let mut workers = Vec::with_capacity(SESSION_COUNT);
    for session_id in 0..SESSION_COUNT {
        let actual = Arc::clone(&actual);
        workers.push(thread::spawn(move || {
            let session = actual.SpawnSessionCollector();
            let mut generator = TestOpGenerator::new(session_id as u64 + 1);
            for op_id in 0..OP_PER_SESSION {
                let sequence = (session_id * OP_PER_SESSION + op_id + 1) as u64;
                let op = generator.generate_test_op(sequence);
                session.Update(op.idx.TableID, op.idx.IndexID, op.info);
                if op.report {
                    session.Report();
                }
            }
            session.Flush();
        }));
    }
    for worker in workers {
        worker.join().expect("index usage worker panicked");
    }

    expected.Close();
    actual.Close();
    for table_id in 0..10 {
        for index_id in 0..10 {
            assert_eq!(
                actual.GetIndexUsage(table_id, index_id),
                expected.GetIndexUsage(table_id, index_id),
                "table={table_id}, index={index_id}",
            );
        }
    }
}

// BenchmarkIndexCollector's three Go sub-benchmarks are retained as a native
// parallel workload helper. Cargo's stable test harness does not execute benchmarks.
/// 并行负载辅助：多 worker 共享操作序列，按间隔调用 Report。
#[allow(dead_code)]
fn benchmark_index_collector(iterations: usize, report_per_op: usize) -> Duration {
    let mut generator = TestOpGenerator::new(1);
    let operations: Arc<Vec<_>> = Arc::new(
        (0..iterations)
            .map(|sequence| generator.generate_test_op(sequence as u64 + 1))
            .collect(),
    );
    let collector = Arc::new(NewCollector());
    collector.StartWorker();
    let next = Arc::new(AtomicUsize::new(0));
    let worker_count = thread::available_parallelism().map_or(1, usize::from);
    let started = Instant::now();
    let mut workers = Vec::with_capacity(worker_count);
    for _ in 0..worker_count {
        let operations = Arc::clone(&operations);
        let collector = Arc::clone(&collector);
        let next = Arc::clone(&next);
        workers.push(thread::spawn(move || {
            let session = collector.SpawnSessionCollector();
            let mut local_counter = 0;
            loop {
                let index = next.fetch_add(1, Ordering::Relaxed);
                let Some(op) = operations.get(index) else {
                    break;
                };
                session.Update(op.idx.TableID, op.idx.IndexID, op.info.clone());
                if local_counter % report_per_op == 0 {
                    session.Report();
                }
                local_counter += 1;
            }
            session.Flush();
        }));
    }
    for worker in workers {
        worker.join().expect("benchmark worker panicked");
    }
    collector.Close();
    started.elapsed()
}

/// 分别以每 1/4/8 次操作 Report 一次跑三组负载。
#[allow(dead_code)]
fn benchmark_index_collector_report_variants(iterations: usize) -> [(usize, Duration); 3] {
    [1, 4, 8].map(|report_per_op| {
        (
            report_per_op,
            benchmark_index_collector(iterations, report_per_op),
        )
    })
}

// TestStmtIndexUsageCollector: a statement counts each index once, including
// a first sample whose incoming QueryTotal is zero.
/// 语句级采集器对同一索引只计一次 QueryTotal（含入参为 0 的首次样本）。
#[test]
fn test_stmt_index_usage_collector() {
    let collector = NewCollector();
    collector.StartWorker();
    let session = collector.SpawnSessionCollector();
    let statement = NewStmtIndexUsageCollector(session.clone());

    statement.Update(1, 1, NewSample(10, 0, 0, 0));
    session.Flush();
    require_eventually(|| collector.GetIndexUsage(1, 1).QueryTotal == 1);
    assert_eq!(collector.GetIndexUsage(1, 1).QueryTotal, 1);

    statement.Update(1, 1, NewSample(10, 0, 0, 0));
    session.Flush();
    require_eventually(|| collector.GetIndexUsage(1, 1).QueryTotal == 1);

    statement.Update(1, 2, NewSample(10, 0, 0, 0));
    session.Flush();
    require_eventually(|| collector.GetIndexUsage(1, 2).QueryTotal == 1);
    assert_eq!(collector.GetIndexUsage(1, 2).QueryTotal, 1);

    statement.Update(1, 3, NewSample(0, 0, 0, 0));
    session.Flush();
    require_eventually(|| collector.GetIndexUsage(1, 3).QueryTotal == 1);
    assert_eq!(collector.GetIndexUsage(1, 3).QueryTotal, 1);
    collector.Close();
}

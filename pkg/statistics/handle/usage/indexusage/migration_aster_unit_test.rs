// Copyright 2023 PingCAP, Inc.
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

// Aster 迁移补充单元测试：对齐 Go 索引使用率采集语义。
//
// 覆盖分桶边界、会话累加、Report 拒绝后仍保留增量、并发一致性、
// 语句级去重与 Reset，以及按元数据 GC 缺失表/索引。

use super::{IndexInfo, NewCollector, NewSample, NewStmtIndexUsageCollector, Sample, TableInfo};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Go `time.Time{}` 是公元 1 年零值，不能用 Unix epoch 代替。
#[test]
fn default_sample_uses_go_zero_time() {
    let go_zero_time = UNIX_EPOCH
        .checked_sub(Duration::from_secs(62_135_596_800))
        .expect("platform SystemTime must represent Go's zero time");
    assert_eq!(Sample::default().LastUsedAt, go_zero_time);
}

/// 等待异步合并在约 1 秒内满足条件。
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

/// 分桶边界与 NewSample 字段应与 Go 实现一致。
#[test]
fn bucket_and_new_sample_match_go_boundaries() {
    let cases = [
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

    for (rows, total, expected_bucket) in cases {
        let sample = NewSample(3, 4, rows, total);
        let mut expected = [0; 7];
        expected[expected_bucket] = 1;
        assert_eq!(sample.PercentageAccess, expected);
        assert_eq!(sample.QueryTotal, 3);
        assert_eq!(sample.KvReqTotal, 4);
        assert_eq!(sample.RowAccessTotal, rows);
    }

    assert_eq!(
        NewSample(0, 0, 5, 0).PercentageAccess,
        [0, 0, 0, 0, 0, 0, 1]
    );
}

/// 同会话多次 Update 后 Flush，全局应看到累加结果；未记录索引返回默认样本。
#[test]
fn session_updates_accumulate_and_flush_to_global() {
    let global = NewCollector();
    global.StartWorker();
    let session = global.SpawnSessionCollector();

    session.Update(1, 1, NewSample(1, 1, 1, 1));
    session.Update(1, 1, NewSample(10, 10, 5, 50));
    session.Update(1, 1, NewSample(10, 10, 5, 0));
    session.Flush();
    global.Close();

    let usage = global.GetIndexUsage(1, 1);
    assert_eq!(usage.QueryTotal, 21);
    assert_eq!(usage.KvReqTotal, 21);
    assert_eq!(usage.RowAccessTotal, 11);
    assert_eq!(usage.PercentageAccess, [0, 0, 0, 1, 0, 0, 2]);
    assert_eq!(global.GetIndexUsage(999, 999), Sample::default());
}

/// 通道满时非阻塞 Report 被拒绝，Flush 后仍能合并该待定增量。
#[test]
fn report_rejection_keeps_pending_delta() {
    let global = NewCollector();

    // Match the Go collector's channel capacity. With no worker, the eleventh
    // non-blocking report is rejected and must remain pending in its session.
    for index_id in 0..10 {
        let session = global.SpawnSessionCollector();
        session.Update(1, index_id, NewSample(1, 0, 0, 0));
        session.Report();
    }
    let pending = global.SpawnSessionCollector();
    pending.Update(1, 99, NewSample(7, 0, 0, 0));
    pending.Report();

    global.StartWorker();
    pending.Flush();
    global.Close();

    assert_eq!(global.GetIndexUsage(1, 99).QueryTotal, 7);
}

/// 按种子生成与并发测试相同分布的随机样本。
fn generated_sample(rng: &mut StdRng, sequence: u64) -> (i64, i64, Sample, bool) {
    let table_id = rng.gen_range(0..10);
    let index_id = rng.gen_range(0..10);
    let query_total = rng.gen_range(0..10_000);
    let kv_req_total = rng.gen_range(0..10_000);
    let table_total_rows = rng.gen_range(0..10_000);
    let row_access = if table_total_rows == 0 {
        0
    } else {
        rng.gen_range(0..table_total_rows)
    };
    let mut sample = NewSample(query_total, kv_req_total, row_access, table_total_rows);
    sample.LastUsedAt = UNIX_EPOCH + Duration::from_nanos(sequence);
    let report = rng.gen_range(0..4) == 1;
    (table_id, index_id, sample, report)
}

/// 并发 Report/Flush 聚合结果应与串行累加一致。
#[test]
fn concurrent_flush_matches_serial_aggregation() {
    // The same-path Go parity test keeps the original 64×100000 stress load.
    // This migration supplement exercises independent seeds and interleavings
    // without repeating that complete stress workload a second time.
    const SESSION_COUNT: usize = 16;
    const OP_PER_SESSION: usize = 10_000;

    let expected = NewCollector();
    expected.StartWorker();
    let expected_session = expected.SpawnSessionCollector();
    for session_id in 0..SESSION_COUNT {
        let mut rng = StdRng::seed_from_u64(session_id as u64 + 1);
        for op_id in 0..OP_PER_SESSION {
            let sequence = (session_id * OP_PER_SESSION + op_id + 1) as u64;
            let (table_id, index_id, sample, _) = generated_sample(&mut rng, sequence);
            expected_session.Update(table_id, index_id, sample);
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
            let mut rng = StdRng::seed_from_u64(session_id as u64 + 1);
            for op_id in 0..OP_PER_SESSION {
                let sequence = (session_id * OP_PER_SESSION + op_id + 1) as u64;
                let (table_id, index_id, sample, report) = generated_sample(&mut rng, sequence);
                session.Update(table_id, index_id, sample);
                if report {
                    session.Report();
                }
            }
            session.Flush();
        }));
    }
    for worker in workers {
        worker.join().unwrap();
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

/// 语句级去重后 Reset 可再次计入同一索引。
#[test]
fn statement_deduplicates_query_total_and_reset_reenables_counting() {
    let global = NewCollector();
    global.StartWorker();
    let session = global.SpawnSessionCollector();
    let statement = NewStmtIndexUsageCollector(session.clone());

    statement.Update(1, 1, NewSample(10, 0, 0, 0));
    statement.Update(1, 1, NewSample(10, 0, 0, 0));
    statement.Update(1, 2, NewSample(10, 0, 0, 0));
    statement.Update(1, 3, NewSample(0, 0, 0, 0));
    session.Flush();

    require_eventually(|| {
        global.GetIndexUsage(1, 1).QueryTotal == 1
            && global.GetIndexUsage(1, 2).QueryTotal == 1
            && global.GetIndexUsage(1, 3).QueryTotal == 1
    });

    assert_eq!(global.GetIndexUsage(1, 1).QueryTotal, 1);
    assert_eq!(global.GetIndexUsage(1, 2).QueryTotal, 1);
    assert_eq!(global.GetIndexUsage(1, 3).QueryTotal, 1);

    statement.Reset();
    statement.Update(1, 1, NewSample(0, 0, 0, 0));
    session.Flush();
    require_eventually(|| global.GetIndexUsage(1, 1).QueryTotal == 2);
    global.Close();
    assert_eq!(global.GetIndexUsage(1, 1).QueryTotal, 2);
}

/// GC 应删除元数据中不存在的表，以及表内已删除的索引记录。
#[test]
fn gc_removes_missing_tables_and_indexes() {
    let global = NewCollector();
    global.StartWorker();
    let session = global.SpawnSessionCollector();
    session.Update(1, 10, NewSample(1, 0, 0, 0));
    session.Update(1, 11, NewSample(1, 0, 0, 0));
    session.Update(2, 10, NewSample(1, 0, 0, 0));
    session.Flush();
    require_eventually(|| {
        global.GetIndexUsage(1, 10).QueryTotal == 1
            && global.GetIndexUsage(1, 11).QueryTotal == 1
            && global.GetIndexUsage(2, 10).QueryTotal == 1
    });

    let table = TableInfo {
        ID: 1,
        Indices: vec![IndexInfo {
            ID: 10,
            ..IndexInfo::default()
        }],
        ..TableInfo::default()
    };
    global.GCIndexUsage(|table_id| {
        if table_id == 1 {
            (Some(table.clone()), true)
        } else {
            (None, false)
        }
    });
    global.Close();

    assert_eq!(global.GetIndexUsage(1, 10).QueryTotal, 1);
    assert_eq!(global.GetIndexUsage(1, 11), Sample::default());
    assert_eq!(global.GetIndexUsage(2, 10), Sample::default());
}

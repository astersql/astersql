// Copyright 2026 PingCAP, Inc.
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

// Aggregator `drain_and_push_ru` 容量/规模基准形测试：验证 10k key 上限裁剪。

#![allow(non_snake_case)]

use std::sync::Arc;

use super::stmtstats_tests::*;

/// 向 StatementStats 写入 `num_users * sqls_per_user` 条已完成 RU 样本。
fn makeRUBatchForBench(
    stats: &Arc<StatementStats>,
    num_users: usize,
    sqls_per_user: usize,
    user_offset: usize,
) {
    let total = num_users * sqls_per_user;
    for user in 0..num_users {
        for sql in 0..sqls_per_user {
            let user_name = format!("u{:04}", user_offset + user);
            let sql_digest = format!("sql{:04}_{:04}", user_offset + user, sql);
            let details = ru_details((total - user * sqls_per_user - sql) as f64, 0.0, 0.0, 0.0);
            stats.OnExecutionBegin(
                sql_digest.as_bytes(),
                b"plan",
                Some(&ExecBeginInfo {
                    RUDetails: Some(details.clone()),
                    User: user_name.clone(),
                    RUVersion: RU_VERSION_V1,
                    TopRUEnabled: true,
                    ..Default::default()
                }),
            );
            stats.OnExecutionFinished(
                sql_digest.as_bytes(),
                b"plan",
                Some(&ExecFinishInfo {
                    RUDetails: Some(details),
                    User: user_name,
                    ExecDuration: SignedDuration::from_nanos(1),
                    TopRUEnabled: true,
                    ..Default::default()
                }),
            );
        }
    }
}

/// 注册若干 StatementStats、执行一轮 drain，返回收集到的 RU key 数。
fn drainShape(num_stats: usize, num_users: usize, sqls_per_user: usize) -> usize {
    reset_top_state();
    topsql_state::EnableTopRU();
    let aggregator = Aggregator::new();
    for index in 0..num_stats {
        let stats = Arc::new(StatementStats::new());
        makeRUBatchForBench(&stats, num_users, sqls_per_user, index * num_users);
        aggregator.register(stats);
    }
    let collector = Arc::new(TestRUCollector::default());
    aggregator.register_ru_collector(collector.clone());
    aggregator.drain_and_push_ru();
    let batches = collector.batches.lock().unwrap();
    let len = batches[0].0.len();
    drop(batches);
    reset_top_state();
    len
}

/// 恰好 10k distinct key 时应全部保留。
#[test]
fn BenchmarkDrainAndPushRUAt10kCap() {
    let _guard = super::test_support::stmtstats_guard();
    assert_eq!(drainShape(1, 100, 100), 10_000);
}

/// 超过 10k 时裁剪到上限。
#[test]
fn BenchmarkDrainAndPushRUOver10kCap() {
    let _guard = super::test_support::stmtstats_guard();
    assert_eq!(drainShape(1, 120, 100), 10_000);
}

/// 16 个会话共约 160k key，合并后仍裁到 10k。
#[test]
fn BenchmarkDrainAndPushRU160KKeys() {
    let _guard = super::test_support::stmtstats_guard();
    assert_eq!(drainShape(16, 100, 100), 10_000);
}

/// 与 Go preloaded 基准一致：先构建全部批次再单次 drain。
#[test]
fn BenchmarkDrainAndPushRU160KKeysPreloaded() {
    let _guard = super::test_support::stmtstats_guard();
    // All 16 batches are fully built before the single measured drain, matching
    // the Go preloaded benchmark's separation of setup from merge/drain work.
    assert_eq!(drainShape(16, 100, 100), 10_000);
}

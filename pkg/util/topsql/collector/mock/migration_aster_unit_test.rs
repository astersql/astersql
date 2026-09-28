// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// TopSQL collector mock 迁移基线单元测试。
//
// 验证 RegisterSQL/Plan 首次写入与大 plan 丢弃、Collect 聚合与过滤、
// Go 风格 digest 拼接哈希冲突语义，以及与真实 digester 的接口对接。

use std::sync::Arc;

use crate::{GenSQLDigest, NewTopSQLCollector};
use collector::{Collector, SQLCPUTimeRecord};
use parser::digester_impl as parser;

/// 构造一条 SQLCPUTimeRecord 测试数据。
fn record(sql_digest: &[u8], plan_digest: &[u8], cpu_time_ms: u32) -> SQLCPUTimeRecord {
    SQLCPUTimeRecord {
        SQLDigest: sql_digest.to_vec(),
        PlanDigest: plan_digest.to_vec(),
        CPUTimeMs: cpu_time_ms,
    }
}

/// 校验 SQL/Plan 注册保留首值、二进制 digest 可用，且 isLarge plan 被丢弃。
#[test]
fn registration_matches_go_first_value_binary_and_large_plan_behavior() {
    let collector = NewTopSQLCollector();
    let digest_a = [0x80, 0x00, 0xff];
    let digest_b = [0x81, 0x00, 0xfe];

    collector.RegisterSQL(&digest_a, "first".to_owned(), false);
    collector.RegisterSQL(&digest_a, "replacement".to_owned(), true);
    collector.RegisterSQL(&digest_b, "second".to_owned(), false);
    assert_eq!(collector.GetSQL(&digest_a), "first");
    assert_eq!(collector.GetSQL(&digest_b), "second");

    collector.RegisterPlan(&digest_a, "plan-a".to_owned(), false);
    collector.RegisterPlan(&digest_a, "replacement".to_owned(), false);
    collector.RegisterPlan(&digest_b, "too-large".to_owned(), true);
    assert_eq!(collector.GetPlan(&digest_a), "plan-a");
    assert_eq!(collector.GetPlan(&digest_b), "");
}

/// 校验 Collect 累加 CPU、按是否有 plan 过滤，且空批次也计入 CollectCnt。
#[test]
fn collect_aggregates_cpu_filters_plans_and_counts_every_call() {
    let collector = NewTopSQLCollector();
    let sql = "select * from t where a = 1";
    let sql_digest = GenSQLDigest(sql);
    let plan_digest = b"plan";

    collector.RegisterSQL(sql_digest.Bytes(), sql.to_owned(), false);
    collector.RegisterPlan(plan_digest, "TableReader".to_owned(), false);
    Collector::Collect(
        collector.as_ref(),
        vec![
            record(sql_digest.Bytes(), plan_digest, 4),
            record(sql_digest.Bytes(), plan_digest, 6),
            record(sql_digest.Bytes(), b"", 3),
        ],
    );
    Collector::Collect(collector.as_ref(), Vec::new());

    assert_eq!(collector.CollectCnt(), 2);
    assert_eq!(collector.GetSQLCPUTimeBySQL(sql), 13);
    let all = collector.GetSQLStatsBySQL(sql, false);
    assert_eq!(all.len(), 2);
    let with_plan = collector.GetSQLStatsBySQL(sql, true);
    assert_eq!(with_plan, vec![record(sql_digest.Bytes(), plan_digest, 10)]);
    assert_eq!(collector.GetSQLStatsBySQLWithRetry(sql, true), with_plan);
}

/// 校验 mock 哈希为 SQLDigest‖PlanDigest 字节拼接，冲突时保留首对并累加时间。
#[test]
fn collect_preserves_go_digest_concatenation_hash_behavior() {
    let collector = NewTopSQLCollector();
    let sql = "select * from t where id = 7";
    let digest = GenSQLDigest(sql);
    let mut second_sql_digest = digest.Bytes().to_vec();
    second_sql_digest.push(b'b');
    Collector::Collect(
        collector.as_ref(),
        vec![
            record(digest.Bytes(), b"bc", 5),
            record(&second_sql_digest, b"c", 7),
        ],
    );

    // mock.go hashes string(SQLDigest) + string(PlanDigest), so these pairs
    // intentionally address the same entry and retain the first pair.
    let stats = collector.GetSQLStatsBySQL(sql, false);
    assert_eq!(stats, vec![record(digest.Bytes(), b"bc", 12)]);
    assert_eq!(collector.CollectCnt(), 1);

    collector.Reset();
    assert_eq!(collector.CollectCnt(), 0);
    assert_eq!(collector.GetSQL(b"a"), "");
    assert_eq!(collector.GetPlan(b"bc"), "");
}

/// 校验 GenSQLDigest 与 parser::NormalizeDigest 一致，以及 Start/Close 等空实现可调用。
#[test]
fn digest_and_noop_interface_methods_use_real_dependency_types() {
    let collector = NewTopSQLCollector();
    let digest = GenSQLDigest("select 42");
    let (_, expected) = parser::NormalizeDigest("select 42");
    assert_eq!(digest, expected);

    collector.Start();
    collector.BindKeyspaceName(b"keyspace");
    collector.CollectStmtStatsMap(Default::default());
    collector.Close();

    let shared: Arc<_> = collector;
    Collector::Collect(shared.as_ref(), Vec::new());
    assert_eq!(shared.CollectCnt(), 1);
}

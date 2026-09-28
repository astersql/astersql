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

// TiDB server 公共协议回归测试（part2）：TopSQL 统计与资源标签。
//
// TopSQL（Top SQL）按 SQL digest 与执行计划 digest 聚合语句耗时、网络字节与 KV
// 目标计数；资源组标签（resource group tag）可携带二进制 SQL digest。

use std::sync::{Arc, Mutex};

use astersql_util_topsql_stmtstats::{
    Aggregator, Collector, ExecBeginInfo, ExecFinishInfo, SQLPlanDigest, SignedDuration,
    StatementStats, StatementStatsMap,
};

/// 串行化 TopSQL 全局开关相关用例，避免并发 Enable/Disable 互相干扰。
static TOP_SQL_STATE: Mutex<()> = Mutex::new(());

/// RAII 守卫：构造时启用 TopSQL，析构时关闭，保证用例结束恢复全局状态。
struct TopSqlStateGuard;

impl TopSqlStateGuard {
    /// 启用 TopSQL 并返回守卫。
    fn enable() -> Self {
        astersql_util_topsql_state::EnableTopSQL();
        Self
    }
}

impl Drop for TopSqlStateGuard {
    fn drop(&mut self) {
        astersql_util_topsql_state::DisableTopSQL();
    }
}

/// 收集 Aggregator 下发的语句统计批次，供断言合并结果。
#[derive(Default)]
struct MockCollector {
    /// 每次 `CollectStmtStatsMap` 收到的一批 digest→统计映射。
    batches: Mutex<Vec<StatementStatsMap>>,
}

impl Collector for MockCollector {
    /// 将一批语句统计追加到 `batches`。
    fn CollectStmtStatsMap(&self, stats: StatementStatsMap) {
        self.batches
            .lock()
            .expect("collector lock poisoned")
            .push(stats);
    }
}

/// 模拟一次语句执行：先 `OnExecutionBegin` 再 `OnExecutionFinished`，写入网络与耗时。
fn record_execution(
    stats: &StatementStats,
    sql_digest: &[u8],
    plan_digest: &[u8],
    input_bytes: u64,
    output_bytes: u64,
    duration_ns: i64,
) {
    stats.OnExecutionBegin(
        sql_digest,
        plan_digest,
        Some(&ExecBeginInfo {
            InNetworkBytes: input_bytes,
            ..ExecBeginInfo::default()
        }),
    );
    stats.OnExecutionFinished(
        sql_digest,
        plan_digest,
        Some(&ExecFinishInfo {
            OutNetworkBytes: output_bytes,
            ExecDuration: SignedDuration::from_nanos(duration_ns),
            ..ExecFinishInfo::default()
        }),
    );
}

/// 验证多会话按 (sql_digest, plan_digest) 合并 ExecCount/耗时/网络字节，聚合后清空会话池。
#[test]
fn top_sql_statement_stats_merge_sessions_by_sql_and_plan_digest() {
    let _serial = TOP_SQL_STATE.lock().expect("TopSQL state lock poisoned");
    let _top_sql = TopSqlStateGuard::enable();
    let aggregator = Aggregator::new();
    let collector = Arc::new(MockCollector::default());
    aggregator.register_collector(collector.clone());

    // 会话一：同一 SQL+计划执行两次。
    let first_session = Arc::new(StatementStats::new());
    record_execution(&first_session, b"select-t", b"point-get", 11, 17, 3_000);
    record_execution(&first_session, b"select-t", b"point-get", 13, 19, 5_000);
    first_session.SetFinished();

    // 会话二：一次 select（同 digest）与一次 update（不同 digest）。
    let second_session = Arc::new(StatementStats::new());
    record_execution(&second_session, b"select-t", b"point-get", 23, 29, 7_000);
    record_execution(&second_session, b"update-t", b"table-scan", 31, 37, 11_000);
    second_session.SetFinished();

    aggregator.register(first_session);
    aggregator.register(second_session);
    aggregator.aggregate_all();

    let batches = collector.batches.lock().expect("collector lock poisoned");
    assert_eq!(batches.len(), 1);
    let batch = &batches[0];
    // select 跨会话合并为 3 次执行，耗时与网络字节求和。
    let select = batch
        .get(&SQLPlanDigest::new(b"select-t", b"point-get"))
        .expect("merged select stats");
    assert_eq!(select.ExecCount, 3);
    assert_eq!(select.DurationCount, 3);
    assert_eq!(select.SumDurationNs, 15_000);
    assert_eq!(select.NetworkInBytes, 47);
    assert_eq!(select.NetworkOutBytes, 65);

    let update = batch
        .get(&SQLPlanDigest::new(b"update-t", b"table-scan"))
        .expect("update stats");
    assert_eq!(update.ExecCount, 1);
    assert_eq!(update.SumDurationNs, 11_000);
    assert_eq!(aggregator.stats_len(), 0);
}

/// 验证 KV 目标计数：同一目标多次记录只计一次，关闭 TopSQL 后不再计入。
#[test]
fn top_sql_kv_targets_are_counted_once_per_statement_execution() {
    let _serial = TOP_SQL_STATE.lock().expect("TopSQL state lock poisoned");
    let stats = StatementStats::new();

    let _top_sql = TopSqlStateGuard::enable();
    let counter = stats.CreateKvExecCounter(b"select-t", b"point-get");
    // 同一 tikv 目标重复 record 仍只计一次。
    counter.record_target("tikv-1");
    counter.record_target("tikv-1");
    counter
        .intercept("tikv-2", (), |target, request| {
            assert_eq!(target, "tikv-2");
            Ok::<_, ()>(request)
        })
        .expect("request interceptor");
    // 关闭 TopSQL 后再 record 的目标不得进入统计。
    astersql_util_topsql_state::DisableTopSQL();
    counter.record_target("tikv-disabled");

    let data = stats.Take();
    let counts = data
        .get(&SQLPlanDigest::new(b"select-t", b"point-get"))
        .and_then(|item| item.KvStatsItem.KvExecCount.as_ref())
        .expect("KV target counts");
    assert_eq!(counts.get("tikv-1"), Some(&1));
    assert_eq!(counts.get("tikv-2"), Some(&1));
    assert!(!counts.contains_key("tikv-disabled"));
}

/// 验证资源组标签 protobuf 可编解码 32 字节 SQL digest，空标签返回 None。
#[test]
fn top_sql_resource_tag_carries_the_binary_sql_digest() {
    // 手工构造 length-delimited protobuf：field 1 (0x0a) + len + digest。
    let digest: Vec<u8> = (0_u8..32).collect();
    let mut encoded = vec![0x0a, digest.len() as u8];
    encoded.extend_from_slice(&digest);

    assert_eq!(
        astersql_util_resourcegrouptag::DecodeResourceGroupTag(&encoded)
            .expect("valid protobuf resource tag"),
        Some(digest)
    );
    assert_eq!(
        astersql_util_resourcegrouptag::DecodeResourceGroupTag(&[]).expect("empty resource tag"),
        None
    );
}

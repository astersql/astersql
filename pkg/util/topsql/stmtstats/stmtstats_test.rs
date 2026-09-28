// Copyright 2021 PingCAP, Inc.
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

// stmtstats 单元测试：合并、Take、RU 采样、版本切换与并发 tick 语义。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::{Arc, Barrier};
use std::thread;

use super::stmtstats_tests::*;

/// 一毫秒对应的纳秒数。
const MILLISECOND_NS: i64 = 1_000_000;
/// 一秒对应的纳秒数。
const SECOND_NS: i64 = 1_000_000_000;

/// 由 SQL/Plan 文本构造聚合键。
fn digest(sql: &str, plan: &str) -> SQLPlanDigest {
    SQLPlanDigest::new(sql.as_bytes(), plan.as_bytes())
}

/// 由用户与 SQL/Plan 文本构造 RUKey。
fn ru_key(user: &str, sql: &str, plan: &str) -> RUKey {
    RUKey::new(user, sql.as_bytes(), plan.as_bytes())
}

/// 构造语句开始信息（可开关 TopRU）。
fn begin_info(
    user: &str,
    enabled: bool,
    details: Option<SharedRUDetails>,
    version: RUVersion,
) -> ExecBeginInfo {
    ExecBeginInfo {
        RUDetails: details,
        User: user.to_owned(),
        RUVersion: version,
        TopRUEnabled: enabled,
        ..ExecBeginInfo::default()
    }
}

/// 构造语句结束信息。
fn finish_info(
    user: &str,
    enabled: bool,
    details: Option<SharedRUDetails>,
    duration_ns: i64,
) -> ExecFinishInfo {
    ExecFinishInfo {
        RUDetails: details,
        User: user.to_owned(),
        ExecDuration: SignedDuration::from_nanos(duration_ns),
        TopRUEnabled: enabled,
        ..ExecFinishInfo::default()
    }
}

/// 启动一条已启用 TopRU 的语句，返回统计实例、明细与 RUKey。
fn begin_ru_case(
    user: &str,
    sql: &str,
    plan: &str,
) -> (Arc<StatementStats>, SharedRUDetails, RUKey) {
    let stats = CreateStatementStats();
    let details = ru_details(0.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        sql.as_bytes(),
        plan.as_bytes(),
        Some(&begin_info(
            user,
            true,
            Some(details.clone()),
            RU_VERSION_V1,
        )),
    );
    (stats, details, ru_key(user, sql, plan))
}

/// 验证 KvStatementStatsItem 按目标地址累加合并。
#[test]
fn TestKvStatementStatsItemMerge() {
    let mut item1 = NewKvStatementStatsItem();
    item1.KvExecCount = Some(HashMap::from([
        ("127.0.0.1:10001".to_owned(), 1),
        ("127.0.0.1:10002".to_owned(), 2),
    ]));
    let item2 = KvStatementStatsItem {
        KvExecCount: Some(HashMap::from([
            ("127.0.0.1:10002".to_owned(), 2),
            ("127.0.0.1:10003".to_owned(), 3),
        ])),
    };
    item1.Merge(item2.clone());
    let merged = item1.KvExecCount.expect("merged counts");
    assert_eq!(merged.len(), 3);
    assert_eq!(merged["127.0.0.1:10001"], 1);
    assert_eq!(merged["127.0.0.1:10002"], 4);
    assert_eq!(merged["127.0.0.1:10003"], 3);
    assert_eq!(item2.KvExecCount.expect("source counts").len(), 2);
}

/// 验证 StatementStatsItem 字段累加与 Merge(None) 为 no-op。
#[test]
fn TestStatementsStatsItemMerge() {
    let mut item1 = StatementStatsItem {
        ExecCount: 1,
        SumDurationNs: 100,
        DurationCount: 1,
        NetworkInBytes: 10,
        NetworkOutBytes: 20,
        ..NewStatementStatsItem()
    };
    let item2 = StatementStatsItem {
        ExecCount: 2,
        SumDurationNs: 50,
        DurationCount: 2,
        NetworkInBytes: 50,
        NetworkOutBytes: 60,
        ..NewStatementStatsItem()
    };
    item1.Merge(Some(&item2));
    assert_eq!(item1.ExecCount, 3);
    assert_eq!(item1.SumDurationNs, 150);
    assert_eq!(item1.DurationCount, 3);
    assert_eq!(item1.NetworkInBytes, 60);
    assert_eq!(item1.NetworkOutBytes, 80);
    item1.Merge(None);
    assert_eq!(item1.ExecCount, 3);
}

/// 验证 StatementStatsMap 同键合并、异键插入。
#[test]
fn TestStatementStatsMapMerge() {
    let item = |count, duration, kv2| StatementStatsItem {
        ExecCount: count,
        SumDurationNs: duration,
        KvStatsItem: KvStatementStatsItem {
            KvExecCount: Some(HashMap::from([
                ("KV-1".to_owned(), 1),
                ("KV-2".to_owned(), kv2),
            ])),
        },
        ..StatementStatsItem::default()
    };
    let mut first = StatementStatsMap::from([
        (digest("SQL-1", ""), item(1, 100, 2)),
        (digest("SQL-2", ""), item(1, 200, 2)),
    ]);
    let second = StatementStatsMap::from([
        (digest("SQL-2", ""), item(1, 100, 2)),
        (digest("SQL-3", ""), item(1, 50, 2)),
    ]);
    first.Merge(second.clone());
    assert_eq!(first.len(), 3);
    assert_eq!(second.len(), 2);
    assert_eq!(first[&digest("SQL-2", "")].ExecCount, 2);
    assert_eq!(first[&digest("SQL-2", "")].SumDurationNs, 300);
    assert_eq!(
        first[&digest("SQL-2", "")]
            .KvStatsItem
            .KvExecCount
            .as_ref()
            .unwrap()["KV-2"],
        4
    );
    first.Merge(StatementStatsMap::new());
    assert_eq!(first.len(), 3);
}

/// 验证 CreateStatementStats 可注册并 Take 空映射。
#[test]
fn TestCreateStatementStats() {
    let before = global_aggregator().stats_len();
    let stats = CreateStatementStats();
    assert!(global_aggregator().stats_len() >= before + 1);
    assert!(!stats.Finished());
    stats.SetFinished();
    assert!(stats.Finished());
}

/// 验证 RU v2 权重采样与 finish 结算。
#[test]
fn TestStatementStatsRUV2Sampling() {
    let stats = CreateStatementStats();
    let details = ru_details(0.0, 0.0, 11.0, 0.0);
    let metrics = Arc::new(execdetails::RUV2Metrics::default());
    metrics.AddPlanCnt(3);
    let weights = execdetails::RUV2Weights {
        RUScale: 1.0,
        PlanCnt: 2.0,
        ..Default::default()
    };
    let mut info = begin_info("u1", true, Some(details.clone()), RU_VERSION_V2);
    info.RUV2Metrics = Some(metrics.clone());
    info.RUV2Weights = weights;
    stats.OnExecutionBegin(b"sql", b"plan", Some(&info));
    let key = ru_key("u1", "sql", "plan");
    let first = stats.MergeRUInto();
    assert_eq!(first[&key].ExecCount, 1);
    assert!((first[&key].TotalRU - 17.0).abs() < 1e-9);
    add_ru(&details, 0.0, 0.0, 5.0, 0.0);
    metrics.AddPlanCnt(1);
    assert!((stats.MergeRUInto()[&key].TotalRU - 7.0).abs() < 1e-9);
    add_ru(&details, 0.0, 0.0, 4.0, 0.0);
    metrics.AddPlanCnt(2);
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", true, Some(details), SECOND_NS)),
    );
    let last = stats.MergeRUInto();
    assert!((last[&key].TotalRU - 8.0).abs() < 1e-9);
    assert_eq!(last[&key].ExecDuration, SECOND_NS as u64);

    let stats = CreateStatementStats();
    let metrics = Arc::new(execdetails::RUV2Metrics::default());
    metrics.AddPlanCnt(3);
    let mut info = begin_info("u1", true, None, RU_VERSION_V2);
    info.RUV2Metrics = Some(metrics.clone());
    info.RUV2Weights = weights;
    stats.OnExecutionBegin(b"sql", b"plan", Some(&info));
    assert!((stats.MergeRUInto()[&key].TotalRU - 6.0).abs() < 1e-9);
    metrics.AddPlanCnt(1);
    assert!((stats.MergeRUInto()[&key].TotalRU - 2.0).abs() < 1e-9);

    let stats = CreateStatementStats();
    let details = ru_details(0.0, 0.0, 11.0, 0.0);
    stats.OnExecutionBegin(
        b"sql",
        b"plan",
        Some(&begin_info(
            "u1",
            true,
            Some(details.clone()),
            RU_VERSION_V2,
        )),
    );
    assert!((stats.MergeRUInto()[&key].TotalRU - 11.0).abs() < 1e-9);
    add_ru(&details, 0.0, 0.0, 4.0, 0.0);
    assert!((stats.MergeRUInto()[&key].TotalRU - 4.0).abs() < 1e-9);
}

/// 验证在途采样排除仅 drain 字段，避免噪声。
#[test]
fn TestStatementStatsRUV2InFlightSamplingExcludesDrainOnlyFields() {
    let stats = CreateStatementStats();
    let details = ru_details(0.0, 0.0, 0.0, 0.0);
    let metrics = Arc::new(execdetails::RUV2Metrics::default());
    let weights = execdetails::RUV2Weights {
        RUScale: 1.0,
        PlanCnt: 1.0,
        ResourceManagerReadCnt: 0.02,
        ResourceManagerWriteCnt: 0.07,
        ..Default::default()
    };
    metrics.AddPlanCnt(1);
    let mut info = begin_info("u1", true, Some(details.clone()), RU_VERSION_V2);
    info.RUV2Metrics = Some(metrics.clone());
    info.RUV2Weights = weights;
    stats.OnExecutionBegin(b"sql", b"plan", Some(&info));
    let key = ru_key("u1", "sql", "plan");
    let in_flight = stats.MergeRUInto();
    assert!((in_flight[&key].TotalRU - 1.0).abs() < 1e-9);
    metrics.AddResourceManagerReadCnt(5);
    metrics.AddResourceManagerWriteCnt(3);
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", true, Some(details), SECOND_NS)),
    );
    let finish = stats.MergeRUInto();
    assert!((finish[&key].TotalRU - 0.31).abs() < 1e-9);
    assert!((in_flight[&key].TotalRU + finish[&key].TotalRU - 1.31).abs() < 1e-9);
}

/// 版本切换清空 RU 状态但保留语句统计。
#[test]
fn TestStatementStatsResetRUStateOnVersionChangePreservesStmtStats() {
    for (from, current, context_kept) in [
        (RU_VERSION_V1, RU_VERSION_V2, false),
        (RU_VERSION_V2, RU_VERSION_V2, true),
    ] {
        let stats = CreateStatementStats();
        let details = ru_details(0.0, 0.0, 0.0, 0.0);
        stats.OnExecutionBegin(
            b"sql",
            b"plan",
            Some(&begin_info("u1", true, Some(details.clone()), from)),
        );
        stats.ResetRUStateOnVersionChange(current);
        add_ru(&details, 0.0, 0.0, 1.0, 0.0);
        let ru = stats.MergeRUInto();
        assert_eq!(!ru.is_empty(), context_kept);
        assert_eq!(stats.Take().len(), 1);
    }
}

/// 验证执行计数累加与 Take 清空语义。
#[test]
fn TestExecCounterAddExecCountTake() {
    let stats = CreateStatementStats();
    assert!(stats.Take().is_empty());
    stats.OnExecutionBegin(b"SQL-1", b"", Some(&ExecBeginInfo::default()));
    for _ in 0..2 {
        stats.OnExecutionBegin(b"SQL-2", b"", Some(&ExecBeginInfo::default()));
        stats.OnExecutionFinished(
            b"SQL-2",
            b"",
            Some(&finish_info("", false, None, SECOND_NS)),
        );
    }
    for _ in 0..3 {
        stats.OnExecutionBegin(b"SQL-3", b"", Some(&ExecBeginInfo::default()));
        stats.OnExecutionFinished(
            b"SQL-3",
            b"",
            Some(&finish_info("", false, None, MILLISECOND_NS)),
        );
    }
    stats.OnExecutionFinished(
        b"SQL-3",
        b"",
        Some(&finish_info("", false, None, -MILLISECOND_NS)),
    );
    let data = stats.Take();
    assert_eq!(data.len(), 3);
    assert_eq!(data[&digest("SQL-1", "")].ExecCount, 1);
    assert_eq!(data[&digest("SQL-1", "")].SumDurationNs, 0);
    assert_eq!(data[&digest("SQL-2", "")].ExecCount, 2);
    assert_eq!(
        data[&digest("SQL-2", "")].SumDurationNs,
        (2 * SECOND_NS) as u64
    );
    assert_eq!(data[&digest("SQL-3", "")].ExecCount, 3);
    assert_eq!(
        data[&digest("SQL-3", "")].SumDurationNs,
        (3 * MILLISECOND_NS) as u64
    );
    assert!(stats.Take().is_empty());
}

/// 验证入站/出站网络字节在 begin/finish 中累加。
#[test]
fn TestNetworkBytesAccumulation() {
    let stats = CreateStatementStats();
    for bytes in [100, 200, 300] {
        stats.OnExecutionBegin(
            b"SQL-1",
            b"PLAN-1",
            Some(&ExecBeginInfo {
                InNetworkBytes: bytes,
                ..Default::default()
            }),
        );
    }
    let key = digest("SQL-1", "PLAN-1");
    let data = stats.Take();
    assert_eq!(data[&key].NetworkInBytes, 600);
    assert_eq!(data[&key].ExecCount, 3);
    for bytes in [50, 150, 250] {
        stats.OnExecutionFinished(
            b"SQL-1",
            b"PLAN-1",
            Some(&ExecFinishInfo {
                OutNetworkBytes: bytes,
                ExecDuration: SignedDuration::from_nanos(SECOND_NS),
                ..Default::default()
            }),
        );
    }
    let data = stats.Take();
    assert_eq!(data[&key].NetworkOutBytes, 450);
    assert_eq!(data[&key].DurationCount, 3);
}

/// 验证 TopRU 开启时 begin/finish 的 RU ExecCount 与 TotalRU。
#[test]
fn TestOnExecutionBeginFinishRU() {
    let stats = CreateStatementStats();
    let details = ru_details(10.0, 20.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sql1",
        b"plan1",
        Some(&begin_info(
            "user1",
            true,
            Some(details.clone()),
            RU_VERSION_V1,
        )),
    );
    stats.OnExecutionFinished(
        b"sql1",
        b"plan1",
        Some(&finish_info("user1", true, Some(details), SECOND_NS)),
    );
    let data = stats.MergeRUInto();
    let item = &data[&ru_key("user1", "sql1", "plan1")];
    assert_eq!(item.ExecCount, 1);
    assert_eq!(item.TotalRU, 30.0);
    assert_eq!(item.ExecDuration, SECOND_NS as u64);
}

/// 验证 MergeRUInto 在途采样与 finish 去重。
#[test]
fn TestMergeRUIntoInFlightSamplingAndFinishDedup() {
    let (stats, details, key) = begin_ru_case("user1", "sql1", "plan1");
    let mut total = RUIncrementMap::new();
    for delta in [10.0, 5.0] {
        add_ru(&details, delta, 0.0, 0.0, 0.0);
        total.Merge(stats.MergeRUInto());
    }
    add_ru(&details, 7.0, 0.0, 0.0, 0.0);
    stats.OnExecutionFinished(
        b"sql1",
        b"plan1",
        Some(&finish_info(
            "user1",
            true,
            Some(details.clone()),
            2 * SECOND_NS,
        )),
    );
    total.Merge(stats.MergeRUInto());
    assert_eq!(total[&key].ExecCount, 1);
    assert!((total[&key].TotalRU - 22.0).abs() < 1e-9);
    add_ru(&details, 3.0, 0.0, 0.0, 0.0);
    assert!(stats.MergeRUInto().is_empty());
}

/// 验证 RU 重置与空明细时 MergeRUInto 行为。
#[test]
fn TestMergeRUIntoHandlesRUResetAndNilRUDetails() {
    let stats = CreateStatementStats();
    let details = ru_details(10.0, 0.0, 0.0, 0.0);
    let key = ru_key("user2", "sql2", "plan2");
    stats.OnExecutionBegin(
        b"sql2",
        b"plan2",
        Some(&begin_info(
            "user2",
            true,
            Some(details.clone()),
            RU_VERSION_V1,
        )),
    );
    assert!((stats.MergeRUInto()[&key].TotalRU - 10.0).abs() < 1e-9);
    details.write().unwrap().read_ru = 0.0;
    assert!(stats.MergeRUInto().is_empty());
    add_ru(&details, 5.0, 0.0, 0.0, 0.0);
    assert!((stats.MergeRUInto()[&key].TotalRU - 5.0).abs() < 1e-9);
    stats.OnExecutionFinished(
        b"sql2",
        b"plan2",
        Some(&finish_info("user2", true, None, SECOND_NS)),
    );
    add_ru(&details, 1.0, 0.0, 0.0, 0.0);
    assert!(stats.MergeRUInto().is_empty());
}

/// 长运行语句跨多个 tick 的 ExecCount 语义。
#[test]
fn TestExecCountBeginBasedLongRunningAcrossTicks() {
    let (stats, details, key) = begin_ru_case("u1", "sql", "plan");
    let mut total = RUIncrementMap::new();
    for (index, delta) in [4.0, 6.0].into_iter().enumerate() {
        add_ru(&details, delta, 0.0, 0.0, 0.0);
        let bucket = stats.MergeRUInto();
        assert_eq!(bucket[&key].ExecCount, u64::from(index == 0));
        total.Merge(bucket);
    }
    add_ru(&details, 5.0, 0.0, 0.0, 0.0);
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", true, Some(details), 3 * SECOND_NS)),
    );
    let finish = stats.MergeRUInto();
    assert_eq!(finish[&key].ExecCount, 0);
    total.Merge(finish);
    assert_eq!(total[&key].ExecCount, 1);
    assert!((total[&key].TotalRU - 15.0).abs() < 1e-9);
}

/// 执行中途开关 TopRU 的矩阵组合。
#[test]
fn TestTopRUToggleMidExecutionMatrix() {
    let (stats, details, key) = begin_ru_case("u1", "sql", "plan");
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", false, Some(details.clone()), SECOND_NS)),
    );
    let data = stats.MergeRUInto();
    assert_eq!(data[&key].ExecCount, 1);
    assert_eq!(data[&key].TotalRU, 0.0);
    add_ru(&details, 2.0, 0.0, 0.0, 0.0);
    assert!(stats.MergeRUInto().is_empty());

    let stats = CreateStatementStats();
    let details = ru_details(20.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sql",
        b"plan",
        Some(&begin_info("u1", false, None, RU_VERSION_V1)),
    );
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", true, Some(details), SECOND_NS)),
    );
    assert!(stats.MergeRUInto().is_empty());

    let (stats, details, key) = begin_ru_case("u1", "sql", "plan");
    add_ru(&details, 10.0, 0.0, 0.0, 0.0);
    let first = stats.MergeRUInto();
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", false, Some(details), SECOND_NS)),
    );
    assert!(stats.MergeRUInto().is_empty());
    assert!((first[&key].TotalRU - 10.0).abs() < 1e-9);

    let (stats, details, key) = begin_ru_case("u1", "sql", "plan");
    add_ru(&details, 10.0, 0.0, 0.0, 0.0);
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", true, Some(details), SECOND_NS)),
    );
    let second_details = ru_details(20.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sql",
        b"plan",
        Some(&begin_info("u1", false, None, RU_VERSION_V1)),
    );
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", true, Some(second_details), SECOND_NS)),
    );
    let data = stats.MergeRUInto();
    assert!((data[&key].TotalRU - 10.0).abs() < 1e-9);
    assert_eq!(data[&key].ExecCount, 1);
}

/// RU 为零时不产生噪声增量。
#[test]
fn TestExecCountBeginBasedRUZeroNoNoise() {
    let (stats, details, key) = begin_ru_case("u3", "sql", "plan");
    let data = stats.MergeRUInto();
    assert_eq!(data[&key].ExecCount, 1);
    assert_eq!(data[&key].TotalRU, 0.0);
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u3", true, Some(details), SECOND_NS)),
    );
    assert!(stats.MergeRUInto().is_empty());
}

/// 同一 tick 内桶合并语义。
#[test]
fn TestExecCountBeginBasedBucketMergeSameTick() {
    let stats = CreateStatementStats();
    let key = ru_key("u1", "sql", "plan");
    let first = ru_details(6.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sql",
        b"plan",
        Some(&begin_info("u1", true, Some(first.clone()), RU_VERSION_V1)),
    );
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", true, Some(first), SECOND_NS)),
    );
    let second = ru_details(0.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sql",
        b"plan",
        Some(&begin_info("u1", true, Some(second.clone()), RU_VERSION_V1)),
    );
    add_ru(&second, 4.0, 0.0, 0.0, 0.0);
    let data = stats.MergeRUInto();
    assert!((data[&key].TotalRU - 10.0).abs() < 1e-9);
    assert_eq!(data[&key].ExecCount, 2);
}

/// finish 与 tick 并发不丢计数。
#[test]
fn TestExecCountBeginBasedFinishAndTickConcurrent() {
    let key = ru_key("u1", "sql", "plan");
    for _ in 0..100 {
        let (stats, details, _) = begin_ru_case("u1", "sql", "plan");
        add_ru(&details, 10.0, 0.0, 0.0, 0.0);
        let start = Arc::new(Barrier::new(3));
        let tick_stats = stats.clone();
        let tick_start = start.clone();
        let tick = thread::spawn(move || {
            tick_start.wait();
            tick_stats.MergeRUInto()
        });
        let finish_stats = stats.clone();
        let finish_start = start.clone();
        let finish_details = details.clone();
        let finish = thread::spawn(move || {
            finish_start.wait();
            finish_stats.OnExecutionFinished(
                b"sql",
                b"plan",
                Some(&finish_info("u1", true, Some(finish_details), SECOND_NS)),
            );
        });
        start.wait();
        let mut total = tick.join().expect("tick thread");
        finish.join().expect("finish thread");
        total.Merge(stats.MergeRUInto());
        assert_eq!(total.len(), 1);
        assert!((total[&key].TotalRU - 10.0).abs() < 1e-9);
        assert_eq!(total[&key].ExecCount, 1);
    }
}

/// finish 与 tick 的桶归属语义。
#[test]
fn TestExecCountBeginBasedFinishTickBucketSemantics() {
    for tick_first in [true, false] {
        let (stats, details, key) = begin_ru_case("u1", "sql", "plan");
        add_ru(&details, 10.0, 0.0, 0.0, 0.0);
        let mut bucket = RUIncrementMap::new();
        if tick_first {
            bucket = stats.MergeRUInto();
        }
        stats.OnExecutionFinished(
            b"sql",
            b"plan",
            Some(&finish_info("u1", true, Some(details), SECOND_NS)),
        );
        if !tick_first {
            bucket = stats.MergeRUInto();
        }
        assert!((bucket[&key].TotalRU - 10.0).abs() < 1e-9);
        assert_eq!(bucket[&key].ExecCount, 1);
        assert!(stats.MergeRUInto().is_empty());
    }
}

/// tick 后再增长 RU 的增量采样。
#[test]
fn TestExecCountBeginBasedTickThenGrow() {
    let (stats, details, key) = begin_ru_case("u1", "sql", "plan");
    add_ru(&details, 10.0, 0.0, 0.0, 0.0);
    let first = stats.MergeRUInto();
    assert_eq!(first[&key].ExecCount, 1);
    assert_eq!(first[&key].ExecDuration, 0);
    add_ru(&details, 5.0, 0.0, 0.0, 0.0);
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", true, Some(details), 2 * SECOND_NS)),
    );
    let second = stats.MergeRUInto();
    assert!((second[&key].TotalRU - 5.0).abs() < 1e-9);
    assert_eq!(second[&key].ExecCount, 0);
    assert_eq!(second[&key].ExecDuration, (2 * SECOND_NS) as u64);
    let mut total = first;
    total.Merge(second);
    assert!((total[&key].TotalRU - 15.0).abs() < 1e-9);
    assert_eq!(total[&key].ExecCount, 1);
}

/// tick 后立即 finish 的结算。
#[test]
fn TestExecCountBeginBasedTickThen() {
    let (stats, details, key) = begin_ru_case("u1", "sql", "plan");
    add_ru(&details, 10.0, 0.0, 0.0, 0.0);
    let first = stats.MergeRUInto();
    assert!((first[&key].TotalRU - 10.0).abs() < 1e-9);
    add_ru(&details, 5.0, 0.0, 0.0, 0.0);
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", false, Some(details), 2 * SECOND_NS)),
    );
    assert!(stats.MergeRUInto().is_empty());
}

/// tick 后版本重置再采样。
#[test]
fn TestExecCountBeginBasedTickThenReset() {
    let (stats, details, key) = begin_ru_case("u1", "sql", "plan");
    add_ru(&details, 10.0, 0.0, 0.0, 0.0);
    let first = stats.MergeRUInto();
    assert!((first[&key].TotalRU - 10.0).abs() < 1e-9);
    details.write().unwrap().read_ru = 0.0;
    add_ru(&details, 5.0, 0.0, 0.0, 0.0);
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", true, Some(details), 2 * SECOND_NS)),
    );
    assert!(stats.MergeRUInto().is_empty());
}

/// 切换 RUKey 不发生跨键污染。
#[test]
fn TestExecCountBeginBasedKeySwitchNoCrossPollution() {
    let stats = CreateStatementStats();
    let key_a = ru_key("u1", "sqlA", "planA");
    let key_b = ru_key("u1", "sqlB", "planB");
    let details_a = ru_details(0.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sqlA",
        b"planA",
        Some(&begin_info(
            "u1",
            true,
            Some(details_a.clone()),
            RU_VERSION_V1,
        )),
    );
    add_ru(&details_a, 10.0, 0.0, 0.0, 0.0);
    assert!((stats.MergeRUInto()[&key_a].TotalRU - 10.0).abs() < 1e-9);
    let details_b = ru_details(0.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sqlB",
        b"planB",
        Some(&begin_info(
            "u1",
            true,
            Some(details_b.clone()),
            RU_VERSION_V1,
        )),
    );
    add_ru(&details_a, 5.0, 0.0, 0.0, 0.0);
    stats.OnExecutionFinished(
        b"sqlA",
        b"planA",
        Some(&finish_info("u1", true, Some(details_a), 2 * SECOND_NS)),
    );
    let pseudo_b = stats.MergeRUInto();
    assert!(!pseudo_b.contains_key(&key_a));
    assert_eq!(pseudo_b[&key_b].ExecCount, 1);
    assert_eq!(pseudo_b[&key_b].TotalRU, 0.0);
    add_ru(&details_b, 7.0, 0.0, 0.0, 0.0);
    stats.OnExecutionFinished(
        b"sqlB",
        b"planB",
        Some(&finish_info("u1", true, Some(details_b), SECOND_NS)),
    );
    let second = stats.MergeRUInto();
    assert!((second[&key_b].TotalRU - 7.0).abs() < 1e-9);
    let mut total = pseudo_b;
    total.Merge(second);
    assert_eq!(total[&key_b].ExecCount, 1);
    assert!((total[&key_b].TotalRU - 7.0).abs() < 1e-9);
}

/// 多 tick delta 之和等于最终总量。
#[test]
fn TestMultiTickDeltaSumEqualsFinalTotal() {
    let (stats, details, key) = begin_ru_case("u1", "sql", "plan");
    let mut total = RUIncrementMap::new();
    for delta in [10.0, 15.0, 8.0] {
        add_ru(&details, delta, 0.0, 0.0, 0.0);
        total.Merge(stats.MergeRUInto());
    }
    add_ru(&details, 17.0, 0.0, 0.0, 0.0);
    stats.OnExecutionFinished(
        b"sql",
        b"plan",
        Some(&finish_info("u1", true, Some(details), 5 * SECOND_NS)),
    );
    total.Merge(stats.MergeRUInto());
    assert!((total[&key].TotalRU - 50.0).abs() < 1e-9);
    assert_eq!(total[&key].ExecCount, 1);
    assert!(stats.MergeRUInto().is_empty());
}

/// 多 tick 路径的轻量基准式压力测试。
#[test]
fn BenchmarkExecCountBeginBasedAcrossManyTicks() {
    let stats = CreateStatementStats();
    let details = ru_details(0.0, 0.0, 0.0, 0.0);
    stats.OnExecutionBegin(
        b"sql-bench",
        b"plan-bench",
        Some(&begin_info(
            "u-bench",
            true,
            Some(details.clone()),
            RU_VERSION_V1,
        )),
    );
    for _ in 0..64 {
        add_ru(&details, 1.0, 0.0, 0.0, 0.0);
        let _ = stats.MergeRUInto();
    }
    add_ru(&details, 3.0, 0.0, 0.0, 0.0);
    stats.OnExecutionFinished(
        b"sql-bench",
        b"plan-bench",
        Some(&finish_info("u-bench", true, Some(details), MILLISECOND_NS)),
    );
    let tail = stats.MergeRUInto();
    assert!((tail[&ru_key("u-bench", "sql-bench", "plan-bench")].TotalRU - 3.0).abs() < 1e-9);
}

/// 多活跃上下文下的轻量基准式压力测试。
#[test]
fn BenchmarkExecCountBeginBasedManyActiveContexts() {
    let mut active = Vec::with_capacity(256);
    for index in 0..256 {
        let stats = CreateStatementStats();
        let details = ru_details(0.0, 0.0, 0.0, 0.0);
        let sql = format!("sql-bench-{index}");
        let user = format!("u-bench-{index}");
        stats.OnExecutionBegin(
            sql.as_bytes(),
            b"plan-bench",
            Some(&begin_info(
                &user,
                true,
                Some(details.clone()),
                RU_VERSION_V1,
            )),
        );
        active.push((stats, details));
    }
    for (stats, details) in active {
        add_ru(&details, 1.0, 0.0, 0.0, 0.0);
        let data = stats.MergeRUInto();
        assert_eq!(data.len(), 1);
    }
}

// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// execdetails 工具函数单测。
//
// 覆盖 context 初始化/继承指针语义、RU 同步与 TiKV 明细快照、
// Percentile 精确/digest 路径，以及 FormatDuration 与 Go 用例表对齐。

use super::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;

#[test]
/// 验证初始化后补齐保留 Arc 指针，继承不覆盖已有 RU/metrics。
fn context_initialization_preserves_and_inherits_go_pointer_semantics() {
    // 全新 context：三类明细均应就绪，且 statement 带 RUv2 metrics。
    let initialized = ContextWithInitializedExecDetails(context::Context::default());
    let exec = initialized
        .value::<Arc<util::ExecDetails>, _>(&util::ExecDetailsKey)
        .expect("exec details must be initialized");
    let ru = initialized
        .value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
        .expect("RU details must be initialized");
    let stmt = initialized
        .value::<Arc<StmtExecDetails>, _>(&StmtExecDetailKey)
        .expect("statement details must be initialized");
    assert!(stmt.getRUV2Metrics().is_some());

    // 已完整初始化后再补齐：指针应保持不变。
    let completed = ContextWithMissingExecDetailsInitialized(initialized.clone());
    assert!(Arc::ptr_eq(
        &exec,
        &completed
            .value::<Arc<util::ExecDetails>, _>(&util::ExecDetailsKey)
            .unwrap()
    ));
    assert!(Arc::ptr_eq(
        &ru,
        &completed
            .value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
            .unwrap()
    ));

    // 空目标应从 source 继承同一 RU / metrics Arc。
    let target = context::Context::default();
    let inherited = ContextWithInheritedRUV2Details(target, Some(completed.clone()));
    assert!(Arc::ptr_eq(
        &ru,
        &inherited
            .value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
            .unwrap()
    ));
    assert!(Arc::ptr_eq(
        &RUV2MetricsFromContext(&completed).unwrap(),
        &RUV2MetricsFromContext(&inherited).unwrap()
    ));

    // 目标已有自己的对象时不得被 source 覆盖。
    let own_target = ContextWithInitializedExecDetails(context::Context::default());
    let own_ru = own_target
        .value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
        .unwrap();
    let own_metrics = RUV2MetricsFromContext(&own_target).unwrap();
    let preserved = ContextWithInheritedRUV2Details(own_target, Some(completed.clone()));
    assert!(Arc::ptr_eq(
        &own_ru,
        &preserved
            .value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
            .unwrap()
    ));
    assert!(Arc::ptr_eq(
        &own_metrics,
        &RUV2MetricsFromContext(&preserved).unwrap()
    ));

    assert!(
        ContextWithInheritedRUV2Details(inherited.clone(), None)
            .value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
            .is_some()
    );

    // 没有 statement details 时，Go 将 metrics 暂存到独立 context key；
    // 后续补齐 statement details 时必须继承同一指针，而不是新建或覆盖。
    let standalone_metrics = Arc::new(RUV2Metrics::default());
    let with_standalone = ContextWithRUV2Metrics(
        context::Context::default(),
        Some(standalone_metrics.clone()),
    );
    assert!(Arc::ptr_eq(
        &standalone_metrics,
        &RUV2MetricsFromContext(&with_standalone).unwrap()
    ));
    let completed_standalone = ContextWithMissingExecDetailsInitialized(with_standalone);
    assert!(Arc::ptr_eq(
        &standalone_metrics,
        &RUV2MetricsFromContext(&completed_standalone).unwrap()
    ));

    // nil metrics 与 nil source 都是无副作用路径。
    let without_metrics = ContextWithRUV2Metrics(context::Context::default(), None);
    assert!(RUV2MetricsFromContext(&without_metrics).is_none());
}

#[test]
/// 验证 RU 同步幂等，以及 LoadTiKVExecDetails 快照不受后续原子写影响。
fn context_sync_and_exec_snapshot_match_go_behavior() {
    let ctx = ContextWithInitializedExecDetails(context::Context::default());
    let ru = ctx
        .value::<Arc<util::RUDetails>, _>(&util::RUDetailsCtxKey)
        .unwrap();
    ru.add_for_test(17);
    let metrics = SyncRUV2MetricsFromContext(&ctx).expect("metrics must exist");
    assert_eq!(metrics.total_for_test(), 17);
    assert_eq!(
        SyncRUV2MetricsFromContext(&ctx).unwrap().total_for_test(),
        17
    );

    // 快照后改原子字段，不应影响已拷贝值；None 得到全零。
    let source = util::ExecDetails::default();
    let expected = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
    source.set_all_for_test(expected);
    let snapshot = LoadTiKVExecDetails(Some(&source));
    source.BackoffCount.store(99, Ordering::Relaxed);
    assert_eq!(snapshot.values_for_test(), expected);
    assert_eq!(LoadTiKVExecDetails(None).values_for_test(), [0; 12]);

    let stmt = Arc::new(StmtExecDetails {
        WriteSQLRespDuration: Duration::from_millis(7),
        ..StmtExecDetails::default()
    });
    let details_ctx = context::WithValue(ctx, &StmtExecDetailKey, stmt);
    let (write_duration, context_snapshot, context_ru) = GetExecDetailsFromContext(&details_ctx);
    assert_eq!(write_duration, Duration::from_millis(7));
    assert_eq!(context_snapshot.values_for_test(), [0; 12]);
    assert!(Arc::ptr_eq(&ru, &context_ru));

    // 空 context 与 Go 一样没有可同步 metrics；读取时返回零值明细并补一个 RUDetails。
    let empty = context::Context::default();
    assert!(SyncRUV2MetricsFromContext(&empty).is_none());
    let (empty_duration, empty_snapshot, empty_ru) = GetExecDetailsFromContext(&empty);
    assert_eq!(empty_duration, Duration::default());
    assert_eq!(empty_snapshot.values_for_test(), [0; 12]);
    assert_eq!(empty_ru.drain_for_test(), 0);
}

#[test]
/// 验证小样本精确分位、大样本 digest、以及 MergePercentile 汇总。
fn percentile_exact_and_digest_paths_match_go_accounting() {
    // 精确路径：排序后按下标取分位。
    let mut exact = Percentile::<Int64>::default();
    for value in [9, 1, 5, 3] {
        exact.Add(value);
    }
    assert_eq!(exact.Size(), 4);
    assert_eq!(exact.Sum(), 18.0);
    assert_eq!(exact.GetMin(), Some(1));
    assert_eq!(exact.GetMax(), Some(9));
    assert_eq!(exact.GetPercentile(0.0), 1.0);
    assert_eq!(exact.GetPercentile(0.5), 5.0);
    assert_eq!(exact.GetPercentile(0.75), 9.0);

    // 超过阈值后走 t-digest；p90 落在合理近似区间。
    let mut digest = Percentile::<Int64>::default();
    for value in 0..1005 {
        digest.Add(value);
    }
    assert_eq!(digest.Size(), 1005);
    assert_eq!(digest.Sum(), (0..1005).sum::<i64>() as f64);
    assert_eq!(digest.GetMin(), Some(0));
    assert_eq!(digest.GetMax(), Some(1004));
    let p90 = digest.GetPercentile(0.9);
    assert!((890.0..=915.0).contains(&p90), "p90={p90}");

    let mut left = Percentile::<Int64>::default();
    left.Add(-2);
    left.Add(-1);
    let mut right = Percentile::<Int64>::default();
    right.Add(4);
    right.Add(8);
    left.MergePercentile(&right);
    assert_eq!(left.Size(), 4);
    assert_eq!(left.Sum(), 9.0);
    assert_eq!(left.GetPercentile(0.5), 4.0);

    left.MergePercentile(&digest);
    assert_eq!(left.Size(), 1009);
    assert_eq!(left.Sum(), 9.0 + digest.Sum());
}

#[test]
/// 对照 Go 用例表断言 FormatDuration 输出。
fn format_duration_matches_go_table() {
    let cases = [
        (0, "0s"),
        (1, "1ns"),
        (9, "9ns"),
        (10, "10ns"),
        (999, "999ns"),
        (1_000, "1µs"),
        (1_123, "1.12µs"),
        (1_023, "1.02µs"),
        (1_003, "1µs"),
        (10_456, "10.5µs"),
        (10_956, "11µs"),
        (999_056, "999.1µs"),
        (999_988, "1ms"),
        (1_123_000, "1.12ms"),
        (1_023_000, "1.02ms"),
        (1_003_000, "1ms"),
        (10_456_000, "10.5ms"),
        (10_956_000, "11ms"),
        (999_056_000, "999.1ms"),
        (999_988_000, "1s"),
        (1_123_000_000, "1.12s"),
        (1_023_000_000, "1.02s"),
        (1_003_000_000, "1s"),
        (10_456_000_000, "10.5s"),
        (10_956_000_000, "11s"),
        (999_056_000_000, "16m39.1s"),
        (999_988_000_000, "16m40s"),
        (87_399_388_662_000, "24h16m39.4s"),
        (9_412_345, "9.41ms"),
        (10_412_345, "10.4ms"),
        (5_999_000_000, "6s"),
        (100_450, "100.5µs"),
    ];

    // 逐条对照纳秒输入与 Go 期望字符串。
    for (nanos, expected) in cases {
        assert_eq!(
            FormatDuration(Duration::from_nanos(nanos)),
            expected,
            "{nanos}ns"
        );
    }
}

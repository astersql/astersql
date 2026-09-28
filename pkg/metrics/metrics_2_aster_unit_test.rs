// Copyright 2026 AsterSQL.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// Aster 侧 metrics / telemetry 行为边界单测。
//
// 校验 telemetry 快照减法与 Go 一致、表分区用量最大值规则、
// 包级 InitMetrics 成功路径，以及 channelz 内部目标判定。

use astersql_metrics::telemetry::{
    AccountLockCounter, CTEUsageCounter, DDLUsageCounter, FairLockingUsageCounter,
    NonTransactionalStmtCounter, StoreBatchCoprCounter, TablePartitionUsageCounter,
};
use prometheus::core::Collector;

/// 验证 CTE / 账户锁 / 非事务语句计数器的 sub 减法与 Go 一致。
#[test]
fn telemetry_snapshots_match_go_subtraction_behavior() {
    let cte = CTEUsageCounter::new(13, 8, 5).sub(&CTEUsageCounter::new(3, 2, 1));
    assert_eq!(cte.values(), (10, 6, 4));

    let account = AccountLockCounter::new(9, 7, 11).sub(&AccountLockCounter::new(4, 2, 3));
    assert_eq!(account.values(), (5, 5, 8));

    let dml = NonTransactionalStmtCounter::new(10, 20, 30)
        .sub(&NonTransactionalStmtCounter::new(1, 2, 3));
    assert_eq!(dml.values(), (9, 18, 27));
}

/// 验证表分区用量 cal 在差值与当前值之间取较大者，对齐 Go 最大值规则。
#[test]
fn table_partition_cal_preserves_go_maximum_rule() {
    let current =
        TablePartitionUsageCounter::from_values([20, 9, 8, 7, 6, 5, 4, 3, 2, 13, 8, 7, 6, 5, 4]);
    let previous =
        TablePartitionUsageCounter::from_values([3, 2, 1, 1, 1, 1, 1, 1, 1, 10, 2, 2, 2, 2, 2]);
    let delta = current.cal(&previous);
    assert_eq!(
        delta.values(),
        [17, 7, 7, 6, 5, 4, 3, 2, 1, 10, 6, 5, 4, 3, 2]
    );

    // 当增长量大于上一快照中的历史峰值时，结果应取增长量。
    let delta_when_growth_is_larger =
        TablePartitionUsageCounter::from_values([0, 0, 0, 0, 0, 0, 0, 0, 0, 30, 0, 0, 0, 0, 0])
            .cal(&TablePartitionUsageCounter::from_values([
                0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0,
            ]));
    assert_eq!(delta_when_growth_is_larger.values()[9], 26);
}

/// 验证 Go sub 故意忽略的字段在 Rust 中被置零 / 置 false。
#[test]
fn fields_intentionally_omitted_by_go_sub_are_zeroed() {
    let store = StoreBatchCoprCounter::new(64, 10, 20, 30, 40)
        .sub(&StoreBatchCoprCounter::new(32, 1, 2, 3, 4));
    assert_eq!(store.values(), (0, 9, 18, 27, 36));

    let ddl = DDLUsageCounter::new(8, true, 7, 6).sub(&DDLUsageCounter::new(1, true, 2, 3));
    assert_eq!(ddl.values(), (7, false, 5, 3));
}

/// 验证公平锁（fair locking）两类计数均参与减法。
#[test]
fn fair_locking_subtracts_both_go_counters() {
    let value = FairLockingUsageCounter::new(12, 9).sub(&FairLockingUsageCounter::new(5, 4));
    assert_eq!(value.values(), (7, 5));
}

/// 验证 Prometheus 标签值与 Go 一致，且 CounterVec 读回正确。
#[test]
fn prometheus_snapshots_use_the_same_label_values_as_go() {
    let metrics = astersql_metrics::telemetry::TelemetryMetrics::new().unwrap();
    metrics.cte.with_label_values(&["nonRecurCTE"]).inc_by(4.0);
    metrics.cte.with_label_values(&["recurCTE"]).inc_by(3.0);
    metrics.cte.with_label_values(&["notCTE"]).inc_by(2.0);
    assert_eq!(metrics.cte_counter().values(), (4, 3, 2));

    metrics
        .account_lock
        .with_label_values(&["lockUser"])
        .inc_by(5.0);
    metrics
        .account_lock
        .with_label_values(&["unlockUser"])
        .inc_by(6.0);
    metrics
        .account_lock
        .with_label_values(&["createOrAlterUser"])
        .inc_by(7.0);
    assert_eq!(metrics.account_lock_counter().values(), (5, 6, 7));
}

/// 覆盖 InitMetrics 成功边界、结果标签、owner/rawkv/stmtsummary 与 channelz 判定。
#[test]
fn package_metrics_initialization_covers_go_success_boundary_and_error_paths() {
    crate::main_test::ensure_test_env();
    unsafe { astersql_metrics::metrics::InitMetrics().expect("initialize metrics package") };

    // RetLabel / ExecuteErrorToLabel 与 Go 成功、失败、未知边界一致。
    assert_eq!(astersql_metrics::metrics::RetLabel::<&str>(None), "ok");
    assert_eq!(astersql_metrics::metrics::RetLabel(Some(&"failure")), "err");
    assert_eq!(
        astersql_metrics::server::ExecuteErrorToLabel(None),
        "unknown"
    );
    assert_eq!(
        astersql_metrics::server::ExecuteErrorToLabel(Some("global:2")),
        "global:2"
    );

    unsafe {
        // owner 直方图全名应与 Go 完全一致。
        let owner = astersql_metrics::owner::NEW_SESSION_HISTOGRAM
            .as_ref()
            .expect("owner metrics initialized");
        assert_eq!(
            owner.desc()[0].fq_name,
            "tidb_owner_new_session_duration_seconds"
        );

        // RawKV 直方图桶数应对齐 Go 的 17 个指数桶。
        let rawkv = astersql_metrics::rawkv::RAW_KV_BATCH_PUT_DURATION_SECONDS
            .as_ref()
            .expect("rawkv metrics initialized");
        rawkv.with_label_values(&["default"]).observe(0.001);
        assert_eq!(
            rawkv.collect()[0].get_metric()[0]
                .get_histogram()
                .get_bucket()
                .len(),
            17
        );

        // 语句摘要窗口指标写入后可读回。
        astersql_metrics::stmtsummary::SetStmtSummaryWindowMetrics(
            astersql_metrics::stmtsummary::StmtSummaryTypeV1,
            3.0,
            1.0,
        );
        assert_eq!(
            astersql_metrics::stmtsummary::StmtSummaryWindowRecordCount
                .as_ref()
                .unwrap()
                .with_label_values(&[astersql_metrics::stmtsummary::StmtSummaryTypeV1])
                .get(),
            3.0
        );
    }

    // RU v2 执行器计数：已知 level/label 命中，未知 level 返回 None。
    assert!(astersql_metrics::ru_v2::RUV2ExecutorCounter(1, "BatchPointGetExec").is_some());
    assert!(astersql_metrics::ru_v2::RUV2ExecutorCounter(2, "FutureExecutor").is_some());
    assert!(astersql_metrics::ru_v2::RUV2ExecutorCounter(99, "FutureExecutor").is_none());

    // channelz 内部目标与空 socket 判定。
    assert!(astersql_metrics::metrics::is_internal_channelz_target(
        "bufnet"
    ));
    assert!(astersql_metrics::metrics::is_internal_channelz_target(
        "passthrough:///bufnet"
    ));
    assert!(!astersql_metrics::metrics::is_internal_channelz_target(
        "dns:///tikv"
    ));
    assert!(astersql_metrics::metrics::is_internal_channelz_socket(
        None, ""
    ));
    assert!(!astersql_metrics::metrics::is_internal_channelz_socket(
        Some("127.0.0.1"),
        ""
    ));
}

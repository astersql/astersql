// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// `execdetails` 集成风格单元测试。
//
// 对照 Go 覆盖 String/zap、cop 运行时统计、RU v2 指标快照与同步、执行器指标录制，
// 以及 Root/Commit/向量检索等运行时统计的格式与合并语义。

use execdetails_integration::{execdetails as exec, ruv2_metrics as ruv2, util as executil};
use std::sync::atomic::{AtomicI32, AtomicI64};
use std::time::Duration;

/// 测试用 RU v2 权重，数值与 Go 测试向量对齐。
fn default_ruv2_weights_for_test() -> ruv2::RUV2Weights {
    ruv2::RUV2Weights {
        RUScale: 2.01,
        ResultChunkCells: 0.00010000,
        ExecutorL1: 0.00013278,
        ExecutorL2: 0.00000383,
        ExecutorL3: 0.00141739,
        ExecutorL5InsertRows: 0.00472572,
        PlanCnt: 0.15392217,
        PlanDeriveStatsPaths: 0.24968182,
        ResourceManagerReadCnt: 0.02072003,
        ResourceManagerWriteCnt: 0.07179779,
        WriteKeys: 0.330760861554226,
        SessionParserTotal: 0.19230499,
        TxnCnt: 0.03013709,
    }
}

/// 按相对/绝对容差比较浮点 RU 值。
fn assert_close(expected: f64, actual: f64) {
    let tolerance = expected.abs().max(1.0) * 0.01;
    assert!(
        (expected - actual).abs() <= tolerance,
        "expected {expected}, got {actual}"
    );
}

/// 构造精简的 tipb 执行摘要，供 RecordOneCopTask 等用例。
fn cop_summary(time_ns: u64, rows: u64, iterations: u64) -> exec::tipb::ExecutorExecutionSummary {
    exec::tipb::ExecutorExecutionSummary {
        TimeProcessedNs: Some(time_ns),
        NumProducedRows: Some(rows),
        NumIterations: Some(iterations),
        ..Default::default()
    }
}

/// 构造 protobuf Ruv2 计数器载荷。
fn raw_ru(read: u64, write: u64, batch_get: u64, get: u64) -> ruv2::kvrpcpb::Ruv2 {
    let mut raw = ruv2::kvrpcpb::Ruv2::new();
    raw.set_read_rpc_count(read);
    raw.set_write_rpc_count(write);
    raw.set_storage_processed_keys_batch_get(batch_get);
    raw.set_storage_processed_keys_get(get);
    raw
}

#[test]
/// ExecDetails::String 非零字段顺序与格式应匹配 Go。
fn test_string() {
    let mut commit = exec::util::CommitDetails {
        PrewriteTime: Duration::from_secs(1),
        CommitTime: Duration::from_secs(1),
        GetCommitTsTime: Duration::from_secs(1),
        GetLatestTsTime: Duration::from_secs(1),
        LocalLatchTime: Duration::from_secs(1),
        WriteKeys: 1,
        WriteSize: 1,
        PrewriteRegionNum: AtomicI32::new(1),
        TxnRetry: 1,
        ..Default::default()
    };
    commit.ResolveLock.ResolveLockTime = AtomicI64::new(1_000_000_000);
    {
        let mut mu = commit.Mu.Lock();
        mu.CommitBackoffTime = 1_000_000_000;
        mu.PrewriteBackoffTypes = vec!["backoff1".into(), "backoff2".into()];
        mu.CommitBackoffTypes = vec!["commit1".into(), "commit2".into()];
    }
    let detail = exec::ExecDetails {
        CopTime: Duration::from_millis(1003),
        RequestCount: 1,
        LockKeysDetail: Some(exec::util::LockKeysDetails {
            TotalTime: Duration::from_secs(1),
            ..Default::default()
        }),
        CommitDetail: Some(commit),
        CopExecDetails: exec::CopExecDetails {
            BackoffTime: Duration::from_secs(1),
            TimeDetail: exec::util::TimeDetail {
                ProcessTime: Duration::from_millis(2005),
                WaitTime: Duration::from_secs(1),
            },
            ScanDetail: Some(exec::util::ScanDetail {
                ProcessedKeys: 10,
                TotalKeys: 100,
                RocksdbBlockReadDuration: Duration::from_millis(1),
                ..Default::default()
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    let rendered = detail.String();
    for expected in [
        "Cop_time: 1.003",
        "Process_time: 2.005",
        "Wait_time: 1",
        "LockKeys_time: 1",
        "Prewrite_time: 1",
        "Write_keys: 1",
        "Process_keys: 10",
        "Rocksdb_block_read_time: 0.001",
    ] {
        assert!(
            rendered.contains(expected),
            "missing {expected} in {rendered}"
        );
    }
    assert!(
        rendered.contains("Prewrite_Backoff_types: [backoff1 backoff2]"),
        "Go formats []string without Rust debug quotes/commas: {rendered}"
    );
    assert!(
        rendered.contains("Commit_Backoff_types: [commit1 commit2]"),
        "Go formats []string without Rust debug quotes/commas: {rendered}"
    );

    let fields = detail.ToZapFields();
    for (key, expected) in [
        ("Prewrite_Backoff_types", "[backoff1 backoff2]"),
        ("Commit_Backoff_types", "[commit1 commit2]"),
    ] {
        let field = fields
            .iter()
            .find(|field| field.key == key)
            .unwrap_or_else(|| panic!("missing zap field {key}"));
        assert_eq!(field.value, exec::zap::Value::String(expected.to_owned()));
    }
    assert_eq!(exec::ExecDetails::default().String(), "");

    let raw = executil::util::ExecDetails::default();
    raw.set_all_for_test([2, 3, 4, 5, 11, 12, 13, 14, 15, 16, 17, 18]);
    let snapshot = executil::LoadTiKVExecDetails(Some(&raw));
    raw.set_all_for_test([0; 12]);
    assert_eq!(
        snapshot.values_for_test(),
        [2, 3, 4, 5, 11, 12, 13, 14, 15, 16, 17, 18]
    );
}

#[test]
/// cop 运行时统计的累计、格式化与扫描明细合并。
fn test_cop_runtime_stats() {
    let mut stats = exec::NewRuntimeStatsColl(None);
    for summary in [cop_summary(1, 1, 1), cop_summary(2, 2, 2)] {
        stats.RecordOneCopTask(1, exec::kv::TiKV, &summary);
    }
    for summary in [cop_summary(3, 3, 3), cop_summary(4, 4, 4)] {
        stats.RecordOneCopTask(2, exec::kv::TiKV, &summary);
    }
    stats.RecordCopStats(
        1,
        exec::kv::TiKV,
        Some(&exec::util::ScanDetail {
            TotalKeys: 15,
            ProcessedKeys: 10,
            RocksdbDeleteSkippedCount: 5,
            RocksdbBlockReadByte: 100,
            ..Default::default()
        }),
        exec::util::TimeDetail::default(),
        None,
    );
    assert!(stats.ExistsCopStats(1));
    assert_eq!(stats.GetCopCountAndRows(1), (2, 3));
    let mut cop = stats.GetCopStats(1).cloned().expect("table scan stats");
    assert_eq!(
        cop.String(),
        "tikv_task:{proc max:2ns, min:1ns, avg: 1ns, p80:2ns, p95:2ns, iters:3, tasks:2}, processed_keys:10, total_keys:15"
    );
    assert_eq!(cop.stats.String(), "time:3ns, loops:3");
    assert_eq!(stats.GetCopCountAndRows(2), (2, 7));
    assert!(!stats.ExistsRootStats(3));
    stats.GetRootStats(3);
    assert!(stats.ExistsRootStats(3));
    assert_eq!(exec::util::ScanDetail::default().String(), "");
    let mut zero = exec::CopRuntimeStats::default();
    assert_eq!(zero.String(), "");
}

#[test]
/// Go 的非 nil 空 BackoffInfo 不触发 Reset，已有百分位样本必须保留。
fn test_p90_merge_preserves_samples_with_initialized_empty_backoff_map() {
    let mut summary = exec::P90Summary::default();
    summary.ProcessTimePercentile.Add(exec::DurationWithAddr {
        D: Duration::from_secs(1),
        Addr: "existing".to_owned(),
    });
    summary.WaitTimePercentile.Add(exec::DurationWithAddr {
        D: Duration::from_secs(1),
        Addr: "existing".to_owned(),
    });

    summary.Merge(
        &std::collections::HashMap::new(),
        &std::collections::HashMap::new(),
        "new",
        exec::util::TimeDetail {
            ProcessTime: Duration::from_secs(2),
            WaitTime: Duration::from_secs(2),
        },
    );

    assert_eq!(summary.NumCopTasks, 1);
    assert_eq!(summary.ProcessTimePercentile.Size(), 2);
    assert_eq!(summary.WaitTimePercentile.Size(), 2);
}

#[test]
/// RU v2 指标快照应能按权重算出读写 RU。
fn test_ruv2_metrics_snapshot_calculate_ru_values() {
    let weights = default_ruv2_weights_for_test();
    let metrics = ruv2::NewRUV2Metrics();
    metrics.AddResultChunkCells(1000);
    metrics.AddExecutorMetric(1, "TableReader", 5);
    metrics.AddExecutorMetric(1, "Projection", 7);
    metrics.AddExecutorMetric(2, "Selection", 11);
    metrics.AddExecutorMetric(3, "HashJoin", 13);
    metrics.AddExecutorL5InsertRows(17);
    metrics.AddPlanCnt(19);
    metrics.AddPlanDeriveStatsPaths(23);
    metrics.AddResourceManagerReadCnt(29);
    metrics.AddResourceManagerWriteCnt(31);
    metrics.AddWriteKeys(3);
    metrics.AddWriteSize(66);
    metrics.AddSessionParserTotal(37);
    metrics.AddTxnCnt(41);
    metrics.AddTiKVKVEngineCacheMiss(43);
    metrics.AddTiKVCoprocessorWorkTotal("BatchSelection", 53);
    metrics.AddTiKVCoprocessorWorkTotal("BatchTopN", 59);
    metrics.AddTiKVCoprocessorExecutorIterations(61);
    metrics.AddTiKVCoprocessorResponseBytes(67);
    metrics.AddTiKVRaftstoreStoreWriteTriggerWB(71);
    metrics.AddTiKVStorageProcessedKeysBatchGet(73);
    metrics.AddTiKVStorageProcessedKeysGet(79);
    assert_close(42.2851783309, metrics.CalculateRUValues(weights));
    assert_close(
        181980.2851783309,
        metrics.TotalRU(weights, 157258.0, 24680.0),
    );
    assert_eq!((metrics.WriteKeys(), metrics.WriteSize()), (3, 66));

    let mut zero_scale = weights;
    zero_scale.RUScale = 0.0;
    assert_eq!(metrics.CalculateRUValues(zero_scale), 0.0);
    assert_eq!(metrics.TotalRU(zero_scale, 3.0, 4.0), 7.0);
    assert_eq!(ruv2::FormatRUV2Total(None, weights, 3.0, 4.0), "7.00");

    let bypassed = ruv2::NewRUV2Metrics();
    bypassed.SetBypass(true);
    bypassed.AddResultChunkCells(1000);
    assert_eq!(bypassed.TotalRU(weights, 3.0, 4.0), 0.0);
    assert_eq!(
        ruv2::FormatRUV2Summary(Some(&bypassed), weights, 3.0, 4.0),
        (String::new(), String::new())
    );
}

#[test]
/// 从提交明细更新 RU v2 指标。
fn test_update_ruv2_metrics_from_commit_details() {
    let metrics = ruv2::NewRUV2Metrics();
    let weights = default_ruv2_weights_for_test();
    let before = metrics.CalculateRUValues(weights);
    ruv2::UpdateRUV2MetricsFromCommitDetails(
        Some(&metrics),
        Some(&ruv2::tikvutil::CommitDetails {
            WriteKeys: 3,
            WriteSize: 66,
        }),
    );
    assert_eq!((metrics.WriteKeys(), metrics.WriteSize()), (3, 66));
    assert_close(
        before + 3.0 * weights.WriteKeys * weights.RUScale,
        metrics.CalculateRUValues(weights),
    );
    let detail = ruv2::FormatRUV2Metrics(Some(&metrics), weights, 0.0, 0.0);
    assert!(detail.contains("write_keys:3"));
    assert!(detail.contains("write_size:66"));

    let bypassed = ruv2::NewRUV2Metrics();
    bypassed.SetBypass(true);
    ruv2::UpdateRUV2MetricsFromCommitDetails(
        Some(&bypassed),
        Some(&ruv2::tikvutil::CommitDetails {
            WriteKeys: 1,
            WriteSize: 2,
        }),
    );
    assert_eq!((bypassed.WriteKeys(), bypassed.WriteSize()), (0, 0));
}

#[test]
/// 快照后 RU 值应冻结，不受后续累计影响。
fn test_ruv2_metrics_snapshot_freezes_ru_values() {
    let weights = default_ruv2_weights_for_test();
    let metrics = ruv2::NewRUV2Metrics();
    metrics.AddResultChunkCells(1000);
    metrics.AddPlanCnt(2);
    let baseline = metrics.CalculateRUValues(weights);
    let snapshot = metrics.Clone();
    metrics.AddPlanCnt(10);
    assert_eq!(snapshot.PlanCnt(), 2);
    assert_eq!(snapshot.CalculateRUValues(weights), baseline);
    let mut updated = weights;
    updated.ResultChunkCells *= 10.0;
    updated.PlanCnt *= 10.0;
    assert_ne!(baseline, snapshot.CalculateRUValues(updated));
}

#[test]
/// 从 kv Ruv2 计数更新指标。
fn test_update_ruv2_metrics_from_ruv2() {
    let mut raw = raw_ru(2, 3, 17, 19);
    raw.set_kv_engine_cache_miss(5);
    raw.set_coprocessor_executor_iterations(7);
    raw.set_coprocessor_response_bytes(11);
    raw.set_raftstore_store_write_trigger_wb_bytes(13);
    raw.mut_executor_inputs()
        .set_tikv_coprocessor_executor_work_total_batch_fast_hash_aggr(47);
    let metrics = ruv2::NewRUV2Metrics();
    ruv2::UpdateRUV2MetricsFromRUV2(Some(&metrics), Some(&raw));
    assert_eq!(metrics.ResourceManagerReadCnt(), 2);
    assert_eq!(metrics.ResourceManagerWriteCnt(), 3);
    assert_eq!(metrics.TiKVKVEngineCacheMiss(), 5);
    assert_eq!(metrics.TiKVCoprocessorExecutorIterations(), 7);
    assert_eq!(metrics.TiKVCoprocessorResponseBytes(), 11);
    assert_eq!(metrics.TiKVRaftstoreStoreWriteTriggerWB(), 13);
    assert_eq!(metrics.TiKVStorageProcessedKeysBatchGet(), 17);
    assert_eq!(metrics.TiKVStorageProcessedKeysGet(), 19);
    let detail = ruv2::FormatRUV2Metrics(Some(&metrics), default_ruv2_weights_for_test(), 0.0, 0.0);
    for item in [
        "resource_manager_read_cnt:2",
        "resource_manager_write_cnt:3",
        "BatchFastHashAggr:47",
    ] {
        assert!(detail.contains(item), "missing {item} in {detail}");
    }
}

#[test]
/// 从 RUDetails 增量同步到 RU v2 指标。
fn test_sync_ruv2_metrics_from_ru_details_incremental() {
    let metrics = ruv2::NewRUV2Metrics();
    let details = ruv2::tikvutil::NewRUDetails();
    let mut first = raw_ru(2, 3, 7, 19);
    first.set_kv_engine_cache_miss(5);
    first
        .mut_executor_inputs()
        .set_tikv_coprocessor_executor_work_total_batch_index_scan(11);
    details.AddRUV2(&first);
    ruv2::SyncRUV2MetricsFromRUDetails(Some(&metrics), Some(&details));
    assert_eq!(
        (
            metrics.ResourceManagerReadCnt(),
            metrics.ResourceManagerWriteCnt()
        ),
        (2, 3)
    );
    assert_eq!(metrics.TiKVStorageProcessedKeysBatchGet(), 7);
    ruv2::SyncRUV2MetricsFromRUDetails(Some(&metrics), Some(&details));
    assert_eq!(metrics.ResourceManagerReadCnt(), 2);
    details.AddRUV2(&raw_ru(10, 0, 100, 0));
    ruv2::SyncRUV2MetricsFromRUDetails(Some(&metrics), Some(&details));
    assert_eq!(metrics.ResourceManagerReadCnt(), 12);
    assert_eq!(metrics.TiKVStorageProcessedKeysBatchGet(), 107);
}

#[test]
/// bypass 路径下从 RUDetails 同步指标。
fn test_sync_ruv2_metrics_from_ru_details_bypass() {
    let metrics = ruv2::NewRUV2Metrics();
    metrics.SetBypass(true);
    let details = ruv2::tikvutil::NewRUDetails();
    details.AddRUV2(&raw_ru(1, 1, 7, 0));
    ruv2::SyncRUV2MetricsFromRUDetails(Some(&metrics), Some(&details));
    assert_eq!(
        (
            metrics.ResourceManagerReadCnt(),
            metrics.ResourceManagerWriteCnt(),
            metrics.TiKVStorageProcessedKeysBatchGet()
        ),
        (0, 0, 0)
    );
}

#[test]
/// bypass 路径下从 Ruv2 更新指标。
fn test_update_ruv2_metrics_from_ruv2_bypass() {
    let metrics = ruv2::NewRUV2Metrics();
    metrics.SetBypass(true);
    ruv2::UpdateRUV2MetricsFromRUV2(Some(&metrics), Some(&raw_ru(1, 1, 1, 0)));
    assert_eq!(
        (
            metrics.ResourceManagerReadCnt(),
            metrics.ResourceManagerWriteCnt(),
            metrics.TiKVStorageProcessedKeysBatchGet()
        ),
        (0, 0, 0)
    );
}

#[test]
/// 执行器指标录制快路径行为。
fn test_executor_metric_recorder_fast_path() {
    for label in ["BatchPointGetExec", "PointGetExecutor", "LimitExec"] {
        assert!(ruv2::ResolveExecutorMetric(1, label).Available(), "{label}");
    }
    assert!(!ruv2::ResolveExecutorMetric(1, "Unknown").Available());
    assert!(!ruv2::ResolveExecutorMetric(2, "HashAggExec").Available());
    assert!(!ruv2::ExecutorMetricRecorder::default().Available());

    let fast = ruv2::NewRUV2Metrics();
    ruv2::ResolveExecutorMetric(1, "BatchPointGetExec").Record(&fast, 7);
    ruv2::ResolveExecutorMetric(1, "PointGetExecutor").Record(&fast, 3);
    ruv2::ResolveExecutorMetric(1, "LimitExec").Record(&fast, 5);
    let slow = ruv2::NewRUV2Metrics();
    slow.AddExecutorMetric(1, "BatchPointGetExec", 7);
    slow.AddExecutorMetric(1, "PointGetExecutor", 3);
    slow.AddExecutorMetric(1, "LimitExec", 5);
    let mut weights = ruv2::RUV2Weights::default();
    weights.RUScale = 1.0;
    weights.ExecutorL1 = 1.0;
    assert_eq!(
        fast.CalculateRUValues(weights),
        slow.CalculateRUValues(weights)
    );
}

#[test]
/// 格式化输出应先给出 RU 数值再列分项。
fn test_format_ruv2_metrics_includes_ru_values_first() {
    let weights = default_ruv2_weights_for_test();
    let metrics = ruv2::NewRUV2Metrics();
    metrics.AddResultChunkCells(1000);
    metrics.AddResourceManagerWriteCnt(20);
    metrics.AddTiKVCoprocessorWorkTotal("BatchTopN", 10);
    let (total, formatted) = ruv2::FormatRUV2Summary(Some(&metrics), weights, 10987.0, 246.0);
    assert_eq!(total, "11236.09");
    assert_eq!(
        total,
        ruv2::FormatRUV2Total(Some(&metrics), weights, 10987.0, 246.0)
    );
    assert_eq!(
        formatted,
        ruv2::FormatRUV2Metrics(Some(&metrics), weights, 10987.0, 246.0)
    );
    let parts: Vec<_> = formatted.split(", ").collect();
    assert_eq!(parts.len(), 7);
    assert_eq!(
        &parts[..4],
        [
            "total_ru:11236.09",
            "tidb_ru:3.09",
            "tikv_ru:10987.00",
            "tiflash_ru:246.00"
        ]
    );
}

/// 构造 RUVersionV2 的 RURuntimeStats 测试夹具。
fn v2_runtime_stats(tikv_ru: f64, tiflash_ru: f64) -> exec::RURuntimeStats {
    exec::RURuntimeStats {
        RUDetails: Some(exec::util::RUDetails {
            tikv_ru_v2: tikv_ru,
            tiflash_ru,
            ..Default::default()
        }),
        Metrics: Some(exec::RUV2Metrics::default()),
        Weights: exec::RUV2Weights { RUScale: 1.0 },
        RUVersion: exec::rmclient::RUVersionV2,
    }
}

#[test]
/// v2 String 应包含 TiFlash RU。
fn test_ru_runtime_stats_string_includes_ti_flash_ru() {
    assert_eq!(v2_runtime_stats(200.0, 300.0).String(), "RU:500.00");
}

#[test]
/// TiFlash cop 运行时统计路径。
fn test_cop_runtime_stats_for_ti_flash() {
    let mut stats = exec::NewRuntimeStatsColl(None);
    let first = exec::tipb::ExecutorExecutionSummary {
        TimeProcessedNs: Some(1),
        NumProducedRows: Some(1),
        NumIterations: Some(1),
        Concurrency: 1,
        ExecutorId: "tablescan_1".into(),
        TiflashScanContext: Some(Default::default()),
        TiflashWaitSummary: Some(Default::default()),
        TiflashNetworkSummary: Some(Default::default()),
        ..Default::default()
    };
    let second = exec::tipb::ExecutorExecutionSummary {
        TimeProcessedNs: Some(2),
        NumProducedRows: Some(2),
        NumIterations: Some(2),
        Concurrency: 1,
        ExecutorId: "tablescan_1".into(),
        TiflashScanContext: Some(Default::default()),
        ..Default::default()
    };
    stats.RecordOneCopTask(1, exec::kv::TiFlash, &first);
    stats.RecordOneCopTask(1, exec::kv::TiFlash, &second);
    let mut cop = stats.GetCopStats(1).cloned().expect("tiflash stats");
    let rendered = cop.String();
    for expected in [
        "tiflash_task:{proc max:2ns",
        "iters:3",
        "tasks:2",
        "threads:2",
        "wait_summary",
        "network_summary",
        "tiflash_scan",
    ] {
        assert!(
            rendered.contains(expected),
            "missing {expected} in {rendered}"
        );
    }
    assert!(stats.GetStmtCopRuntimeStats().TiflashNetworkStats.is_some());
}

#[test]
/// 向量检索运行时统计格式。
fn test_vector_search_stats() {
    let stats = ruv2::TiFlashScanContext {
        vectorIdxLoadFromS3: 1,
        ..Default::default()
    };
    assert!(
        stats
            .String()
            .starts_with("vector_idx:{load:{total:0ms,from_s3:1,from_disk:0,from_cache:0}")
    );
}

#[test]
/// 列存扫描上下文统计。
fn test_columnar_scan_context_stats() {
    let mut stats = ruv2::TiFlashColumnarScanContext {
        hasStats: true,
        regions: 2,
        readTasks: 4,
        physicalTables: 3,
        columns: 5,
        userReadBytes: 2048,
        mvccInputRows: 100,
        mvccInputBytes: 4096,
        mvccOutputRows: 80,
        totalReadBlockMs: 7,
        totalSerializeBlockMs: 8,
        totalInitReaderMs: 9,
        totalPrefetchMs: 10,
        roughCheckTotalPacks: 11,
        roughCheckSelectedPacks: 12,
        roughCheckSkippedPacks: 13,
        roughCheckUnknownPacks: 14,
        remoteSegments: 15,
        totalSegments: 16,
        totalDeserializeBlockMs: 17,
    };
    stats.Merge(ruv2::TiFlashColumnarScanContext {
        hasStats: true,
        regions: 4,
        readTasks: 6,
        physicalTables: 2,
        columns: 4,
        userReadBytes: 1024,
        mvccInputRows: 10,
        mvccInputBytes: 2048,
        mvccOutputRows: 8,
        totalReadBlockMs: 1,
        totalSerializeBlockMs: 2,
        totalInitReaderMs: 3,
        totalPrefetchMs: 4,
        roughCheckTotalPacks: 5,
        roughCheckSelectedPacks: 6,
        roughCheckSkippedPacks: 7,
        roughCheckUnknownPacks: 8,
        remoteSegments: 9,
        totalSegments: 10,
        totalDeserializeBlockMs: 11,
    });
    assert_eq!(stats.mvccInputRows, 110);
    assert_eq!(stats.mvccInputBytes, 6144);
    assert_eq!(stats.mvccOutputRows, 88);
    let rendered = stats.String();
    assert!(rendered.contains("regions:6"));
    assert!(rendered.contains("rough_check:{total:16, selected:18, skipped:20, unknown:22}"));
    assert!(rendered.starts_with("columnar_scan:{"));
}

#[test]
/// 带提交/加锁信息的运行时统计格式化。
fn test_runtime_stats_with_commit() {
    let commit = exec::util::CommitDetails {
        PrewriteTime: Duration::from_secs(1),
        GetCommitTsTime: Duration::from_secs(1),
        CommitTime: Duration::from_secs(1),
        ResolveLock: exec::util::ResolveLockDetails {
            ResolveLockTime: AtomicI64::new(1_000_000_000),
        },
        WriteKeys: 3,
        WriteSize: 66,
        PrewriteRegionNum: AtomicI32::new(5),
        TxnRetry: 2,
        ..Default::default()
    };
    let stats = exec::RuntimeStatsWithCommit {
        Commit: Some(commit),
        ..Default::default()
    };
    assert_eq!(
        stats.String(),
        "commit_txn: {prewrite:1s, get_commit_ts:1s, commit:1s, resolve_lock: 1s, region_num:5, write_keys:3, write_byte:66, txn_retry:2}"
    );

    let lock = exec::util::LockKeysDetails {
        TotalTime: Duration::from_secs(1),
        RegionNum: 2,
        LockKeys: 10,
        ResolveLock: exec::util::ResolveLockDetails {
            ResolveLockTime: AtomicI64::new(2_000_000_000),
        },
        LockRPCTime: 5_000_000_000,
        LockRPCCount: 50,
        RetryCount: 2,
        ..Default::default()
    };
    let mut lock_stats = exec::RuntimeStatsWithCommit {
        LockKeys: Some(lock.Clone()),
        SharedLockKeys: Some(lock.Clone()),
        ..Default::default()
    };
    let rendered = lock_stats.String();
    assert!(rendered.contains("lock_keys: {time:1s, region:2, keys:10, resolve_lock:2s"));
    assert!(rendered.contains("shared_lock_keys: {time:1s"));
    assert_eq!(lock_stats.Clone().String(), rendered);

    lock_stats.MergeCommitStats(&exec::RuntimeStatsWithCommit {
        SharedLockKeys: Some(exec::util::LockKeysDetails {
            RegionNum: 3,
            LockKeys: 5,
            ..Default::default()
        }),
        ..Default::default()
    });
    let shared = lock_stats.SharedLockKeys.as_ref().unwrap();
    assert_eq!((shared.RegionNum, shared.LockKeys), (5, 15));
    let mut empty = exec::RuntimeStatsWithCommit::default();
    empty.MergeCommitStats(&exec::RuntimeStatsWithCommit {
        SharedLockKeys: Some(exec::util::LockKeysDetails {
            RegionNum: 3,
            LockKeys: 5,
            ..Default::default()
        }),
        ..Default::default()
    });
    assert_eq!(
        empty
            .SharedLockKeys
            .as_ref()
            .map(|v| (v.RegionNum, v.LockKeys)),
        Some((3, 5))
    );
}

#[test]
/// 根节点 RuntimeStats 聚合与展示。
fn test_root_runtime_stats() {
    let mut stats = exec::NewRuntimeStatsColl(None);
    stats
        .GetBasicRuntimeStats(1, true)
        .unwrap()
        .RecordOpen(Duration::from_millis(10));
    stats
        .GetBasicRuntimeStats(1, true)
        .unwrap()
        .Record(Duration::from_secs(1), 20);
    stats
        .GetBasicRuntimeStats(1, false)
        .unwrap()
        .Record(Duration::from_secs(2), 30);
    stats
        .GetBasicRuntimeStats(1, false)
        .unwrap()
        .RecordClose(Duration::from_millis(100));
    let mut concurrency = exec::RuntimeStatsWithConcurrencyInfo::default();
    concurrency.SetConcurrencyInfo(vec![exec::NewConcurrencyInfo("worker".into(), 15)]);
    stats.RegisterStats(1, Box::new(concurrency));
    stats.RegisterStats(
        1,
        Box::new(exec::RuntimeStatsWithCommit {
            Commit: Some(exec::util::CommitDetails {
                PrewriteTime: Duration::from_secs(1),
                GetCommitTsTime: Duration::from_secs(1),
                CommitTime: Duration::from_secs(1),
                WriteKeys: 3,
                WriteSize: 66,
                PrewriteRegionNum: AtomicI32::new(5),
                TxnRetry: 2,
                ..Default::default()
            }),
            ..Default::default()
        }),
    );
    let rendered = stats.GetRootStats(1).String();
    assert!(rendered.starts_with("total_time:3.11s, total_open:10ms, total_close:100ms, loops:2"));
    assert!(rendered.contains("worker:15"));
    assert!(rendered.contains("commit_txn: {prewrite:1s"));
}

#[test]
/// EXPLAIN 用耗时格式化边界。
fn test_format_duration_for_explain() {
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
    for (nanos, expected) in cases {
        assert_eq!(
            executil::FormatDuration(Duration::from_nanos(nanos)),
            expected,
            "{nanos}ns"
        );
    }
}

#[test]
/// cop 运行时统计补充用例。
fn test_cop_runtime_stats2() {
    let mut stats = exec::NewRuntimeStatsColl(None);
    let scan = exec::util::ScanDetail {
        TotalKeys: 15,
        ProcessedKeys: 10,
        RocksdbDeleteSkippedCount: 5,
        RocksdbBlockReadByte: 100,
        ..Default::default()
    };
    stats.RecordCopStats(
        1,
        exec::kv::TiKV,
        Some(&scan),
        exec::util::TimeDetail::default(),
        None,
    );
    let time = exec::util::TimeDetail {
        ProcessTime: Duration::from_millis(10),
        WaitTime: Duration::from_millis(30),
    };
    for _ in 0..1005 {
        let summary = cop_summary(2, 2, 2);
        stats.RecordCopStats(1, exec::kv::TiKV, Some(&scan), time, Some(&summary));
    }
    assert_eq!(stats.GetCopCountAndRows(1), (1005, 2010));
    let mut cop = stats.GetCopStats(1).cloned().unwrap();
    assert_eq!(
        (cop.scanDetail.ProcessedKeys, cop.scanDetail.TotalKeys),
        (10060, 15090)
    );
    assert_eq!(
        executil::FormatDuration(cop.timeDetail.ProcessTime),
        "10.1s"
    );
    let first = cop.String();
    assert_eq!(first, cop.String());
    assert!(first.contains("tasks:1005"));
}

#[test]
/// RU v1 String 展示。
fn test_ru_runtime_stats_string_v1() {
    let stats = exec::RURuntimeStats {
        RUDetails: Some(exec::util::RUDetails {
            read_ru: 10.5,
            write_ru: 20.3,
            ..Default::default()
        }),
        RUVersion: 1,
        ..Default::default()
    };
    assert_eq!(stats.String(), "RU:30.80");
}

#[test]
/// v1 且 details 为空时的 String。
fn test_ru_runtime_stats_string_v1_nil_details() {
    assert_eq!(
        exec::RURuntimeStats {
            RUVersion: 1,
            ..Default::default()
        }
        .String(),
        ""
    );
}

#[test]
/// RU v2 String 展示。
fn test_ru_runtime_stats_string_v2() {
    assert_eq!(v2_runtime_stats(200.0, 300.0).String(), "RU:500.00");
}

#[test]
/// v2 且 RU 为 0 时的 String。
fn test_ru_runtime_stats_string_v2_zero_ru() {
    assert_eq!(v2_runtime_stats(0.0, 0.0).String(), "");
}

#[test]
/// 默认 RU 版本下的 String。
fn test_ru_runtime_stats_string_default_version() {
    let stats = exec::RURuntimeStats {
        RUDetails: Some(exec::util::RUDetails {
            read_ru: 10.5,
            write_ru: 20.3,
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(stats.String(), "RU:30.80");
}

#[test]
/// Clone 应保留 RUVersion。
fn test_ru_runtime_stats_clone_preserves_ru_version() {
    let stats = exec::RURuntimeStats {
        RUDetails: Some(exec::util::RUDetails {
            read_ru: 10.0,
            write_ru: 20.0,
            ..Default::default()
        }),
        RUVersion: 1,
        ..Default::default()
    };
    let cloned = stats.Clone();
    assert_eq!(cloned.RUVersion, 1);
    assert_eq!(cloned.String(), stats.String());
}

#[test]
/// nil/空 Clone 应保留零版本。
fn test_ru_runtime_stats_clone_nil_preserves_zero_version() {
    let stats: Option<exec::RURuntimeStats> = None;
    let cloned = stats
        .as_ref()
        .map(exec::RURuntimeStats::Clone)
        .unwrap_or_default();
    assert_eq!(cloned.RUVersion, 0);
}

#[test]
/// Merge 时应传播 RUVersion。
fn test_ru_runtime_stats_merge_ru_version() {
    let mut dst = exec::RURuntimeStats::default();
    let src = v2_runtime_stats(0.0, 0.0);
    dst.MergeRURuntimeStats(&src);
    assert_eq!(dst.RUVersion, exec::rmclient::RUVersionV2);
}

#[test]
/// Merge 时保留已有 RUVersion，不被对方覆盖。
fn test_ru_runtime_stats_merge_keeps_existing_ru_version() {
    let mut dst = exec::RURuntimeStats {
        RUVersion: 1,
        ..Default::default()
    };
    dst.MergeRURuntimeStats(&v2_runtime_stats(0.0, 0.0));
    assert_eq!(dst.RUVersion, 1);
}

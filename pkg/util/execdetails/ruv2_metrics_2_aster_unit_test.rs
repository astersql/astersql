// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// RUv2 指标与 TiFlash 统计的单元测试：对照 Go 行为校验计算、合并、格式化与 protobuf 合并。
//
// 覆盖 RU 权重求和、bypass、扫描/列存/等待/网络摘要，以及官方 tipb Consumption 解码。

use super::*;
use protobuf::Message;
use std::collections::HashMap;
use std::sync::atomic::Ordering;

/// 构造一组固定权重，便于手工验算期望 RU。
fn weights() -> RUV2Weights {
    RUV2Weights {
        RUScale: 0.5,
        ResultChunkCells: 0.1,
        ExecutorL1: 1.0,
        ExecutorL2: 2.0,
        ExecutorL3: 3.0,
        ExecutorL5InsertRows: 5.0,
        PlanCnt: 7.0,
        PlanDeriveStatsPaths: 11.0,
        ResourceManagerReadCnt: 13.0,
        ResourceManagerWriteCnt: 17.0,
        WriteKeys: 19.0,
        SessionParserTotal: 23.0,
        TxnCnt: 29.0,
    }
}

#[test]
/// 校验 CalculateRUValues/TotalRU、Clone 冻结、Merge 与 FormatRUV2Summary。
fn ruv2_calculation_merge_clone_and_format_match_go() {
    metrics::InitRUV2Metrics();
    let metrics = NewRUV2Metrics();
    metrics.AddResultChunkCells(10);
    metrics.AddExecutorMetric(1, "zeta", 2);
    metrics.AddExecutorMetric(1, "alpha", 3);
    metrics.AddExecutorMetric(2, "selection", 5);
    metrics.AddExecutorMetric(3, "join", 7);
    metrics.AddExecutorL5InsertRows(11);
    metrics.AddPlanCnt(13);
    metrics.AddPlanDeriveStatsPaths(17);
    metrics.AddResourceManagerReadCnt(19);
    metrics.AddResourceManagerWriteCnt(23);
    metrics.AddWriteKeys(29);
    metrics.AddWriteSize(31);
    metrics.AddSessionParserTotal(37);
    metrics.AddTxnCnt(41);

    let expected = 0.5
        * (10.0 * 0.1
            + 5.0 * 1.0
            + 5.0 * 2.0
            + 7.0 * 3.0
            + 11.0 * 5.0
            + 13.0 * 7.0
            + 17.0 * 11.0
            + 19.0 * 13.0
            + 23.0 * 17.0
            + 29.0 * 19.0
            + 37.0 * 23.0
            + 41.0 * 29.0);
    assert_eq!(metrics.CalculateRUValues(weights()), expected);
    assert_eq!(metrics.TotalRU(weights(), 3.25, 7.5), expected + 10.75);

    let cloned = metrics.Clone();
    metrics.AddPlanCnt(100);
    assert_eq!(
        cloned.PlanCnt(),
        13,
        "Clone must freeze the current counters"
    );

    let merged = NewRUV2Metrics();
    merged.Merge(Some(&cloned));
    assert_eq!(merged.CalculateRUValues(weights()), expected);

    let (total, detail) = FormatRUV2Summary(Some(&cloned), weights(), 3.25, 7.5);
    assert_eq!(total, format!("{:.2}", expected + 10.75));
    assert!(detail.starts_with(&format!(
        "total_ru:{:.2}, tidb_ru:{:.2}, tikv_ru:3.25, tiflash_ru:7.50",
        expected + 10.75,
        expected
    )));
    assert!(detail.contains("executor_l1:{alpha:3,zeta:2}"));
    assert!(detail.contains("write_size:31"));
}

#[test]
/// 校验从原始 Ruv2 更新指标，以及 bypass 时不计费。
fn ruv2_raw_counters_and_bypass_match_go() {
    metrics::InitRUV2Metrics();
    let mut raw = kvrpcpb::Ruv2::new();
    raw.set_read_rpc_count(2);
    raw.set_write_rpc_count(3);
    raw.set_kv_engine_cache_miss(5);
    raw.set_storage_processed_keys_batch_get(7);
    raw.set_storage_processed_keys_get(11);
    let inputs = raw.mut_executor_inputs();
    inputs.set_tikv_coprocessor_executor_work_total_batch_selection(13);
    inputs.set_tikv_coprocessor_executor_work_total_batch_top_n(17);

    let metrics = NewRUV2Metrics();
    UpdateRUV2MetricsFromRUV2(Some(&metrics), Some(&raw));
    assert_eq!(metrics.ResourceManagerReadCnt(), 0);
    assert_eq!(metrics.ResourceManagerWriteCnt(), 0);
    assert_eq!(metrics.TiKVKVEngineCacheMiss(), 0);
    assert_eq!(metrics.TiKVStorageProcessedKeysBatchGet(), 0);
    assert_eq!(metrics.TiKVStorageProcessedKeysGet(), 0);
    let detail = FormatRUV2Metrics(Some(&metrics), weights(), 0.0, 0.0);
    assert!(!detail.contains("BatchSelection:13"));
    assert!(!detail.contains("BatchTopN:17"));
    assert!(metrics.IsZero());

    let bypassed = NewRUV2Metrics();
    bypassed.SetBypass(true);
    UpdateRUV2MetricsFromRUV2(Some(&bypassed), Some(&raw));
    assert!(bypassed.IsZero());
    assert_eq!(bypassed.TotalRU(weights(), 9.0, 10.0), 0.0);
    assert_eq!(
        FormatRUV2Summary(Some(&bypassed), weights(), 9.0, 10.0),
        (String::new(), String::new())
    );
}

#[test]
/// 校验 TiFlashScanContext 的 Merge/Clone/String/Empty。
fn tiflash_scan_clone_merge_empty_and_string_match_go() {
    let mut left = TiFlashScanContext::default();
    assert!(left.Empty());
    left.dmfileDataScannedRows = 10;
    left.mvccInputRows = 20;
    left.localRegions = 2;
    left.minLocalStreamMs = 9;
    left.maxLocalStreamMs = 11;
    left.regionsOfInstance = HashMap::from([("b".to_string(), 4), ("a".to_string(), 2)]);

    let mut right = TiFlashScanContext::default();
    right.dmfileDataScannedRows = 5;
    right.remoteRegions = 3;
    right.minLocalStreamMs = 4;
    right.maxLocalStreamMs = 15;
    right.regionsOfInstance = HashMap::from([("a".to_string(), 7)]);
    left.Merge(right);

    assert_eq!(left.dmfileDataScannedRows, 15);
    assert_eq!(left.minLocalStreamMs, 4);
    assert_eq!(left.maxLocalStreamMs, 15);
    assert_eq!(left.regionsOfInstance["a"], 9);
    let cloned = left.Clone();
    left.regionsOfInstance.insert("a".to_string(), 99);
    assert_eq!(cloned.regionsOfInstance["a"], 9);
    let output = cloned.String();
    assert!(output.contains("region_balance:{instance_num: 2, max/min: 9/4=2.250000}"));
    assert!(output.contains("data_scanned_rows:15"));
}

#[test]
/// 校验列存扫描、等待摘要与网络流量合并及回写 ExecDetails。
fn tiflash_columnar_wait_and_network_behaviors_match_go() {
    let mut columnar = TiFlashColumnarScanContext {
        hasStats: true,
        regions: 2,
        physicalTables: 3,
        columns: 4,
        ..Default::default()
    };
    columnar.Merge(TiFlashColumnarScanContext {
        hasStats: false,
        regions: 5,
        physicalTables: 2,
        columns: 9,
        ..Default::default()
    });
    assert_eq!(columnar.regions, 7);
    assert_eq!(columnar.physicalTables, 3);
    assert_eq!(columnar.columns, 9);
    assert!(!columnar.Empty());
    assert!(columnar.String().contains("regions:7"));

    let mut wait = TiFlashWaitSummary {
        executionTime: 100,
        minTSOWaitTime: 2_000_000,
        pipelineBreakerWaitTime: 3_000_000,
        pipelineQueueWaitTime: 500_000,
    };
    wait.Merge(TiFlashWaitSummary {
        executionTime: 90,
        minTSOWaitTime: 99_000_000,
        ..Default::default()
    });
    assert_eq!(wait.minTSOWaitTime, 2_000_000);
    assert_eq!(
        wait.String(),
        "tiflash_wait: {minTSO_wait: 2ms, pipeline_breaker_wait: 3ms}"
    );

    let mut network = TiFlashNetworkTrafficSummary {
        innerZoneSendBytes: 10,
        interZoneSendBytes: 20,
        innerZoneReceiveBytes: 30,
        interZoneReceiveBytes: 40,
    };
    network.Merge(TiFlashNetworkTrafficSummary {
        interZoneSendBytes: 2,
        ..Default::default()
    });
    assert_eq!(network.GetInterZoneTrafficBytes(), 22);
    let details = tikvutil::ExecDetails::default();
    network.UpdateTiKVExecDetails(Some(&details));
    assert_eq!(
        details
            .UnpackedBytesSentMPPCrossZone
            .load(Ordering::Relaxed),
        22
    );
    assert_eq!(
        details.UnpackedBytesSentMPPTotal.load(Ordering::Relaxed),
        32
    );
    assert_eq!(
        details
            .UnpackedBytesReceivedMPPCrossZone
            .load(Ordering::Relaxed),
        40
    );
    assert_eq!(
        details
            .UnpackedBytesReceivedMPPTotal
            .load(Ordering::Relaxed),
        70
    );
}

#[test]
/// 校验官方 tipb 摘要合并与 Consumption 解码进 RUDetails。
fn tiflash_official_protobuf_summaries_and_ru_consumption_match_go() {
    let mut scan_summary = tipb::TiFlashScanContext::new();
    scan_summary.set_dmfile_data_scanned_rows(12);
    scan_summary.set_min_local_stream_ms(8);
    scan_summary.set_max_local_stream_ms(16);
    let mut region = tipb::TiFlashRegionNumOfInstance::new();
    region.set_instance_id("store-1".to_string());
    region.set_region_num(3);
    scan_summary.mut_regions_of_instance().push(region);
    let mut scan = TiFlashScanContext::default();
    scan.mergeExecSummary(Some(&scan_summary));
    assert_eq!(scan.dmfileDataScannedRows, 12);
    assert_eq!(scan.minLocalStreamMs, 8);
    assert_eq!(scan.maxLocalStreamMs, 16);
    assert_eq!(scan.regionsOfInstance["store-1"], 3);

    let mut columnar_summary = tipb::ColumnarScanContext::new();
    columnar_summary.set_regions(5);
    columnar_summary.set_physical_tables(7);
    columnar_summary.set_columns(9);
    let mut columnar = TiFlashColumnarScanContext::default();
    columnar.mergeExecSummary(Some(&columnar_summary));
    assert!(columnar.hasStats);
    assert_eq!(columnar.regions, 5);
    assert_eq!(columnar.physicalTables, 7);
    assert_eq!(columnar.columns, 9);

    let mut wait_summary = tipb::TiFlashWaitSummary::new();
    wait_summary.set_min_tso_wait_ns(2_000_000);
    wait_summary.set_pipeline_queue_wait_ns(3_000_000);
    let mut wait = TiFlashWaitSummary::default();
    wait.mergeExecSummary(Some(&wait_summary), 100);
    assert_eq!(wait.minTSOWaitTime, 2_000_000);
    assert_eq!(wait.pipelineQueueWaitTime, 3_000_000);

    let mut network_summary = tipb::TiFlashNetWorkSummary::new();
    network_summary.set_inner_zone_send_bytes(11);
    network_summary.set_inter_zone_send_bytes(13);
    let mut network = TiFlashNetworkTrafficSummary::default();
    network.mergeExecSummary(Some(&network_summary));
    assert_eq!(network.innerZoneSendBytes, 11);
    assert_eq!(network.interZoneSendBytes, 13);

    let mut consumption = resource_manager::Consumption::new();
    consumption.set_r_r_u(1.25);
    consumption.set_w_r_u(2.75);
    let encoded = consumption
        .write_to_bytes()
        .expect("serialize official Consumption");
    let mut execution = tipb::ExecutorExecutionSummary::new();
    execution.set_ru_consumption(encoded);
    let mut details = tikvutil::NewRUDetails();
    MergeTiFlashRUConsumption(&[None, Some(execution)], &mut details)
        .expect("decode official Consumption");
    assert_eq!(details.TiFlashRU(), 4.0);
}

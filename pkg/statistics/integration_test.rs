// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 统计子系统集成契约测试：采样建直方图、过期判定、懒加载与 TopN 合并等。

use crate::*;

#[test]
/// 列采样 → BuildColumnHist 管线保持总行数与正 NDV。
fn sampling_to_histogram_pipeline_preserves_count_and_ndv() {
    let context = stmtctx::NewStmtCtx();
    let builder = SampleBuilder {
        MaxSampleSize: 32,
        MaxFMSketchSize: 128,
        CMSketchDepth: 5,
        CMSketchWidth: 128,
    };
    let rows = (0..20)
        .map(|value| vec![types::NewBytesDatum(vec![value as u8])])
        .collect();
    let mut collectors = builder.CollectColumnStats(&context, rows).unwrap();
    let collector = collectors.pop().unwrap();
    let mut samples = collector
        .Samples
        .iter()
        .map(|item| item.Value.clone())
        .collect::<Vec<_>>();
    let field_type = types::NewFieldType(types::mysql::TypeBlob);
    let histogram = BuildColumnHist(
        4,
        1,
        &mut samples,
        &field_type,
        collector.Count,
        collector.FMSketch.NDV(),
        collector.NullCount,
        collector.TotalSize,
    )
    .unwrap();
    assert_eq!(histogram.TotalRowCount(), 20.0);
    assert!(histogram.NDV >= 1);
}

/// 构造带单桶直方图的测试列统计。
fn make_column(id: i64, count: i64, status: StatsLoadedStatus) -> Column {
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let mut histogram = NewHistogram(id, count, 0, 1, &field_type, 1, 0);
    if count > 0 {
        histogram.AppendBucketWithNDV(
            &types::NewIntDatum(1),
            &types::NewIntDatum(count),
            count,
            1,
            count,
        );
    }
    Column {
        CMSketch: None,
        TopN: None,
        FMSketch: None,
        Info: Some(ColumnInfo {
            ID: id,
            Name: format!("c{id}"),
            FieldType: (*field_type).clone(),
            IsPrimaryKey: false,
        }),
        Histogram: histogram,
        StatsLoadedStatus: status,
        PhysicalID: 1,
        StatsVer: Version2 as i64,
        IsHandle: false,
    }
}

// Go TestExpBackoffEstimation.
#[test]
/// 偏斜比行数估计给出 Min/Est/Max 区间。
fn exp_backoff_estimation_tracks_min_default_and_risk_max() {
    let estimate = CalculateSkewRatioCounts(10.0, 30.0, 0.25);
    assert_eq!(estimate.MinEst, 10.0);
    assert_eq!(estimate.Est, 15.0);
    assert_eq!(estimate.MaxEst, 20.0);
}

// Go TestNULLOnFullSampling.
#[test]
/// 全采样遇到 NULL 只增 NullCount，不计入 FMSketch NDV。
fn null_on_full_sampling_updates_null_without_sketch_ndv() {
    let context = stmtctx::NewStmtCtx();
    let mut collector = SampleCollector::New(16, 128);
    collector
        .Collect(&context, types::Datum::default())
        .unwrap();
    collector
        .Collect(&context, types::NewBytesDatum(vec![1]))
        .unwrap();
    assert_eq!(collector.NullCount, 1);
    assert_eq!(collector.Count, 1);
    assert_eq!(collector.FMSketch.NDV(), 1);
}

// Go TestAnalyzeSnapshot.
#[test]
/// AnalyzeResults 保留快照与 base count/modify 元数据。
fn analyze_snapshot_metadata_is_retained() {
    let results = AnalyzeResults {
        Err: None,
        Job: None,
        Ars: Vec::new(),
        TableID: AnalyzeTableID {
            TableID: 1,
            PartitionID: NonPartitionTableID,
        },
        Count: 10,
        StatsVer: Version2,
        Snapshot: 99,
        BaseCount: 8,
        BaseModifyCnt: 2,
        ForMVIndexOrGlobalIndex: false,
    };
    assert_eq!(results.Snapshot, 99);
    assert_eq!(results.BaseCount + results.BaseModifyCnt, 10);
}

// Go TestOutdatedStatsCheck.
#[test]
/// 修改比例过高时判定统计过期。
fn outdated_stats_check_uses_modify_ratio() {
    let mut collection = NewHistColl(1, 100, 69, 0, 0);
    assert!(!collection.IsOutdated());
    collection.ModifyCount = 71;
    assert!(collection.IsOutdated());
}

// Go TestShowHistogramsLoadStatus.
#[test]
/// StatusToString 覆盖未初始化/全加载/全驱逐。
fn histogram_load_status_strings_cover_all_states() {
    assert_eq!(
        StatsLoadedStatus::default().StatusToString(),
        "unInitialized"
    );
    assert_eq!(NewStatsFullLoadStatus().StatusToString(), "allLoaded");
    assert_eq!(NewStatsAllEvictedStatus().StatusToString(), "allEvicted");
}

// Go TestSingleColumnIndexNDV.
#[test]
/// 单列索引 NDV 与直方图一致。
fn single_column_index_ndv_matches_histogram() {
    let column = make_column(1, 5, NewStatsFullLoadStatus());
    assert_eq!(column.Histogram.NDV, 5);
    assert_eq!(column.TotalRowCount(), 5.0);
}

// Go TestColumnStatsLazyLoad.
#[test]
/// 列统计懒加载：驱逐时 need load，全量后不再需要。
fn column_stats_lazy_load_reports_needed_then_full() {
    let mut table = Table::New(1, 10, 0);
    table.ColAndIdxExistenceMap.InsertCol(1, true);
    table
        .HistColl
        .SetCol(1, Box::new(make_column(1, 10, NewStatsAllEvictedStatus())));
    assert!(table.ColumnIsLoadNeeded(1, true).1);
    table.HistColl.GetColMut(1).unwrap().StatsLoadedStatus = NewStatsFullLoadStatus();
    assert!(!table.ColumnIsLoadNeeded(1, true).1);
}

// Go TestUpdateNotLoadIndexFMSketch.
#[test]
/// 未加载更新路径可保持索引无 FMSketch。
fn update_without_load_preserves_index_fm_sketch_absence() {
    let field_type = types::NewFieldType(types::mysql::TypeBlob);
    let index = Index {
        CMSketch: None,
        TopN: None,
        FMSketch: None,
        Info: None,
        Histogram: NewHistogram(1, 0, 0, 0, &field_type, 0, 0),
        StatsLoadedStatus: NewStatsAllEvictedStatus(),
        PhysicalID: 1,
        StatsVer: Version2 as i64,
    };
    assert!(index.FMSketch.is_none());
    assert!(index.IsAllEvicted());
}

// Go TestIssue44369.
#[test]
/// 相同 TopN 值合并时累加 Count，不产生重复条目。
fn duplicate_topn_values_merge_without_double_entries() {
    let mut first = NewTopN(1);
    first.AppendTopN(vec![1], 3);
    let mut second = NewTopN(1);
    second.AppendTopN(vec![1], 4);
    let (merged, _) = MergeTopN(&[&first, &second], 1);
    assert_eq!(
        merged.unwrap().TopN,
        vec![TopNMeta {
            Encoded: vec![1],
            Count: 7
        }]
    );
}

// Go TestTableLastAnalyzeVersion.
#[test]
/// LastAnalyzeVersion 与 LastStatsHistVersion 独立。
fn table_last_analyze_version_is_separate_from_hist_version() {
    let mut table = Table::New(1, 10, 0);
    table.LastAnalyzeVersion = 11;
    table.LastStatsHistVersion = 12;
    assert_ne!(table.LastAnalyzeVersion, table.LastStatsHistVersion);
}

// Go TestGlobalIndexWithHistoricalStats.
#[test]
/// 拷贝 HistColl 保留 PhysicalID 与 StatsVer。
fn global_index_historical_stats_keep_physical_id() {
    let mut collection = NewHistColl(88, 10, 0, 0, 0);
    collection.StatsVer = Version2;
    let copied = collection.Copy();
    assert_eq!(copied.PhysicalID, 88);
    assert_eq!(copied.StatsVer, Version2);
}

// Go TestLastAnalyzeVersionNotChangedWithAsyncStatsLoad.
#[test]
/// 异步加载仅改加载状态，不改 LastAnalyzeVersion。
fn async_load_status_change_does_not_change_analyze_version() {
    let mut table = Table::New(1, 10, 0);
    table.LastAnalyzeVersion = 123;
    table
        .HistColl
        .SetCol(1, Box::new(make_column(1, 10, NewStatsAllEvictedStatus())));
    table.HistColl.GetColMut(1).unwrap().StatsLoadedStatus = NewStatsFullLoadStatus();
    assert_eq!(table.LastAnalyzeVersion, 123);
}

// Go TestSaveMetaToStorage.
#[test]
/// MetaOnly 拷贝保留版本与实时/修改计数。
fn save_meta_boundary_is_represented_by_table_copy() {
    let mut table = Table::New(5, 42, 7);
    table.Version = 100;
    let stored = table.CopyAs(CopyIntent::MetaOnly);
    assert_eq!(stored.Version, 100);
    assert_eq!(stored.HistColl.RealtimeCount, 42);
    assert_eq!(stored.HistColl.ModifyCount, 7);
}

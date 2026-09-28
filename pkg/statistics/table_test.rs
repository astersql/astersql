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

// 表级统计单元测试：列/索引存在图、伪表、ANALYZE 版本匹配与 CopyAs。

use crate::*;

/// 存在图应区分“有条目”与“已分析”，克隆后应相等。
#[test]
fn existence_map_distinguishes_present_and_analyzed() {
    let mut map = NewColAndIndexExistenceMap(2, 2);
    map.InsertCol(1, false);
    map.InsertIndex(2, true);
    assert!(map.Has(1, false));
    assert!(!map.HasAnalyzed(1, false));
    assert!(map.HasAnalyzed(2, true));
    let copy = map.CloneMap();
    assert!(ColAndIdxExistenceMapIsEqual(&map, &copy));
}

/// HistColl 过期判定与 PseudoTable 伪行数应与常量一致。
#[test]
fn hist_collection_and_pseudo_table_preserve_counts() {
    let mut collection = NewHistColl(7, 120, 10, 0, 0);
    assert!(!collection.IsOutdated());
    collection.ModifyCount = 100;
    assert!(collection.IsOutdated());

    let pseudo = PseudoTable(9);
    assert!(pseudo.HistColl.Pseudo);
    assert_eq!(pseudo.HistColl.RealtimeCount, PseudoRowCount);
    let copied = pseudo.Copy(CopyIntent::AllDataWritable);
    assert_eq!(copied.HistColl.PhysicalID, 9);
}

fn full_column(id: i64, count: i64) -> Column {
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    let mut histogram = NewHistogram(id, count, 0, 1, &field_type, 1, 0);
    histogram.AppendBucketWithNDV(
        &types::NewIntDatum(1),
        &types::NewIntDatum(count),
        count,
        1,
        count,
    );
    Column {
        CMSketch: None,
        TopN: None,
        FMSketch: None,
        Info: None,
        Histogram: histogram,
        StatsLoadedStatus: NewStatsFullLoadStatus(),
        PhysicalID: 1,
        StatsVer: Version2 as i64,
        IsHandle: false,
    }
}

#[test]
fn table_control_flow_matches_go_contracts() {
    let mut table = Table::New(1, AutoAnalyzeMinCnt - 1, 0);
    assert!(!table.IsEligibleForAnalysis());
    table.RealtimeCount = AutoAnalyzeMinCnt;
    assert!(table.IsEligibleForAnalysis());
    table.Pseudo = true;
    assert!(!table.IsEligibleForAnalysis());

    let empty = Table::New(1, 0, 0);
    assert!(!empty.IsInitialized(), "Go requires any initialized item");

    let mut load = Table::New(1, 0, 0);
    load.ColAndIdxExistenceMap.InsertCol(7, true);
    let (column, needed, analyzed) = load.ColumnIsLoadNeeded(7, false);
    assert!(column.is_none());
    assert!(needed);
    assert!(analyzed);
    load.ColAndIdxExistenceMap.InsertIndex(8, true);
    let (index, needed) = load.IndexIsLoadNeeded(8);
    assert!(index.is_none());
    assert!(needed);
}

#[test]
fn outdated_uses_analyze_row_count_like_go() {
    let mut table = Table::New(1, 10_000, 100);
    table.SetCol(1, Box::new(full_column(1, 100)));
    assert!(table.IsOutdated());
}

#[test]
fn tracking_memory_excludes_fm_sketch_like_go() {
    let column = ColumnMemUsage {
        TotalMemUsage: 100,
        HistogramMemUsage: 10,
        CMSketchMemUsage: 20,
        TopNMemUsage: 30,
        FMSketchMemUsage: 40,
        ..Default::default()
    };
    assert_eq!(column.TrackingMemUsage(), 60);
    let index = IndexMemUsage {
        TotalMemUsage: 100,
        HistogramMemUsage: 10,
        CMSketchMemUsage: 20,
        TopNMemUsage: 30,
        FMSketchMemUsage: 40,
        ..Default::default()
    };
    assert_eq!(index.TrackingMemUsage(), 60);
}

/// 未分析/伪统计无需改写；已分析统计仅在版本相同时匹配。
#[test]
fn analyze_version_requires_v2_table_and_analyze_timestamp() {
    let mut table = Table::New(1, 10, 0);
    assert!(AnalyzeVersionMatchesForTableStats(None, Version2));
    assert!(AnalyzeVersionMatchesForTableStats(Some(&table), Version2));
    table.HistColl.Pseudo = true;
    table.HistColl.StatsVer = Version1;
    assert!(AnalyzeVersionMatchesForTableStats(Some(&table), Version2));
    table.HistColl.Pseudo = false;
    assert!(!AnalyzeVersionMatchesForTableStats(Some(&table), Version2));
    table.HistColl.StatsVer = Version2;
    assert!(AnalyzeVersionMatchesForTableStats(Some(&table), Version2));
}

// Go TestCopyAs.
/// 对应 Go TestCopyAs：各 CopyIntent 拷贝独立，修改拷贝不影响原存在图。
#[test]
fn copy_as_all_intents_preserve_values_and_independent_maps() {
    let mut table = Table::New(1, 10, 2);
    table.ColAndIdxExistenceMap.InsertCol(1, true);
    for intent in [
        CopyIntent::MetaOnly,
        CopyIntent::ColumnMapWritable,
        CopyIntent::IndexMapWritable,
        CopyIntent::BothMapsWritable,
        CopyIntent::AllDataWritable,
    ] {
        let mut copied = table.CopyAs(intent);
        copied.ColAndIdxExistenceMap.DeleteColNotFound(1);
        assert!(table.ColAndIdxExistenceMap.Has(1, false));
        assert_eq!(copied.HistColl.RealtimeCount, 10);
    }
}

#[test]
fn analyzed_state_and_health_match_go_last_analyze_timestamp_and_float_rounding() {
    let mut table = Table::New(1, 2000, 1100);
    table.StatsVer = 2;
    assert!(
        !table.IsAnalyzed(),
        "StatsVer alone is not an analyze timestamp"
    );
    table.LastAnalyzeVersion = 42;
    table.StatsVer = 0;
    assert!(table.IsAnalyzed());
    assert_eq!(table.GetStatsHealthy(), (44, true));
    for (rows, modified, expected) in [
        (2000, 920, 54),
        (2000, 200, 90),
        (2000, 0, 100),
        (800, 500, 37),
        (0, 0, 100),
        (0, 1, 0),
        (2000, 3000, 0),
    ] {
        table.RealtimeCount = rows;
        table.ModifyCount = modified;
        assert_eq!(table.GetStatsHealthy(), (expected, true));
    }
    table.Pseudo = true;
    assert_eq!(table.GetStatsHealthy(), (0, false));
}

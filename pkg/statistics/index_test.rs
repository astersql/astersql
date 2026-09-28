// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

use crate::*;

fn index_with_payload() -> Index {
    let field_type = types::NewFieldType(types::mysql::TypeBlob);
    let mut histogram = NewHistogram(7, 2, 0, 1, &field_type, 1, 0);
    histogram.AppendBucket(
        &types::NewBytesDatum(vec![1]),
        &types::NewBytesDatum(vec![2]),
        2,
        1,
    );
    Index {
        CMSketch: Some(NewCMSketch(5, 16)),
        TopN: Some(NewTopN(1)),
        FMSketch: Some(NewFMSketch(8)),
        Info: Some(IndexInfo {
            ID: 7,
            Name: "idx_ab".to_owned(),
            Columns: vec![IndexColumnInfo::default(), IndexColumnInfo::default()],
            ..IndexInfo::default()
        }),
        Histogram: histogram,
        StatsLoadedStatus: NewStatsFullLoadStatus(),
        PhysicalID: 1,
        StatsVer: Version2 as i64,
    }
}

#[test]
fn string_uses_the_index_column_count() {
    let index = index_with_payload();
    assert_eq!(index.String(), index.Histogram.ToString(2));
}

#[test]
fn evict_all_stats_only_drops_go_index_payloads() {
    let mut index = index_with_payload();
    let bounds_len = index.Histogram.Bounds.len();
    let scalars_len = index.Histogram.Scalars.len();

    index.EvictAllStats();

    assert!(index.CMSketch.is_none());
    assert!(index.TopN.is_none());
    assert!(index.Histogram.Buckets.is_empty());
    assert!(index.FMSketch.is_some());
    assert_eq!(index.Histogram.Bounds.len(), bounds_len);
    assert_eq!(index.Histogram.Scalars.len(), scalars_len);
    assert_eq!(index.GetEvictedStatus(), AllEvicted);
}

#[test]
fn memory_usage_ignores_fm_sketch_like_go() {
    let index = index_with_payload();
    let expected = index.Histogram.MemoryUsage()
        + index.CMSketch.as_ref().unwrap().MemoryUsage()
        + index.TopN.as_ref().unwrap().MemoryUsage();
    assert_eq!(index.MemoryUsage(), expected);
}

#[test]
fn evicted_nonempty_index_remains_valid_like_go() {
    let mut index = index_with_payload();
    index.StatsLoadedStatus = NewStatsAllEvictedStatus();
    assert!(!IndexStatsIsInvalid(Some(&index), false));
}

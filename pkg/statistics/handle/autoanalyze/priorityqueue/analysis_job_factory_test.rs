// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use crate::{
    AnalysisJobFactory, DynamicPartitionedTableAnalysisJob, IndexInfo, NewAnalysisJobFactory,
    NewAutoAnalysisTimeWindow, NewPartitionIDAndName, NonPartitionedTableAnalysisJob,
    PartitionDefinition, PartitionIDAndName, PartitionStatsProvider, SessionContext,
    StaticPartitionedTableAnalysisJob, TableInfo, TableStats,
};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn factory(auto_analyze_ratio: f64, current_ts: u64) -> AnalysisJobFactory {
    NewAnalysisJobFactory(
        SessionContext { analyze_version: 2 },
        auto_analyze_ratio,
        current_ts,
    )
}

fn tso(time: SystemTime) -> u64 {
    (time
        .duration_since(UNIX_EPOCH)
        .expect("test timestamps are after the Unix epoch")
        .as_millis() as u64)
        << 18
}

fn time_at(day: u64, hour: u64, minute: u64, second: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(day * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn analyzed_stats(realtime_count: i64, modify_count: i64) -> TableStats {
    TableStats {
        realtime_count,
        modify_count,
        analyze_row_count: realtime_count,
        column_count: 1,
        stats_version: 2,
        eligible: true,
        analyzed: true,
        ..TableStats::default()
    }
}

#[test]
fn calculate_change_percentage_matches_go_cases() {
    let unanalyzed = TableStats {
        realtime_count: 1_001,
        eligible: true,
        ..TableStats::default()
    };
    assert_eq!(1.0, factory(0.5, 0).CalculateChangePercentage(&unanalyzed));

    let above = analyzed_stats(100, 60);
    assert!((factory(0.5, 0).CalculateChangePercentage(&above) - 0.6).abs() < 0.001);

    let below = analyzed_stats(100, 40);
    assert_eq!(0.0, factory(0.5, 0).CalculateChangePercentage(&below));
    assert_eq!(0.0, factory(0.0, 0).CalculateChangePercentage(&above));

    // Go returns +Inf for a positive modify count over a zero row count.
    let zero_denominator = TableStats {
        modify_count: 1,
        analyzed: true,
        stats_version: 2,
        ..TableStats::default()
    };
    assert!(
        factory(0.5, 0)
            .CalculateChangePercentage(&zero_denominator)
            .is_infinite()
    );
}

#[test]
fn get_table_last_analyze_duration_matches_go_cases() {
    let current = time_at(3, 10, 0, 0);
    let last = current - Duration::from_secs(24 * 60 * 60);
    let analyzed = TableStats {
        analyzed: true,
        last_analyze_version: tso(last),
        ..TableStats::default()
    };
    assert_eq!(
        Duration::from_secs(24 * 60 * 60),
        factory(0.0, tso(current)).GetTableLastAnalyzeDuration(&analyzed)
    );
    assert_eq!(
        Duration::from_secs(30 * 60),
        factory(0.0, tso(current)).GetTableLastAnalyzeDuration(&TableStats::default())
    );
}

#[test]
fn check_indexes_need_analyze_matches_go_filters() {
    let info = TableInfo {
        id: 1,
        indices: vec![
            IndexInfo {
                id: 1,
                is_public: true,
                ..IndexInfo::default()
            },
            IndexInfo {
                id: 2,
                is_public: true,
                is_columnar: true,
                ..IndexInfo::default()
            },
            IndexInfo {
                id: 3,
                is_public: false,
                ..IndexInfo::default()
            },
            IndexInfo {
                id: 4,
                is_public: true,
                ..IndexInfo::default()
            },
        ],
    };
    let mut stats = analyzed_stats(100, 0);
    stats.index_stats.insert(4);
    assert_eq!(
        HashSet::from([1]),
        factory(0.0, 0)
            .CheckIndexesNeedAnalyze(&info, &stats)
            .into_keys()
            .collect()
    );
    assert!(
        factory(0.0, 0)
            .CheckIndexesNeedAnalyze(&info, &TableStats::default())
            .is_empty()
    );
    stats.analyzed_ids.insert(1);
    assert!(
        factory(0.0, 0)
            .CheckIndexesNeedAnalyze(&info, &stats)
            .is_empty()
    );
}

#[test]
fn create_non_partitioned_job_uses_requested_stats_version() {
    let info = TableInfo {
        id: 7,
        indices: Vec::new(),
    };
    let mut stats = analyzed_stats(100, 60);
    stats.stats_version = 1;
    let job = factory(0.5, tso(time_at(2, 0, 0, 0)))
        .CreateNonPartitionedTableAnalysisJob(&info, Some(&stats))
        .expect("change above threshold creates a job");
    let job = job
        .as_any()
        .downcast_ref::<NonPartitionedTableAnalysisJob>()
        .expect("factory creates the non-partitioned job type");
    assert_eq!(7, job.TableID);
    assert_eq!(2, job.TableStatsVer);
    assert!(job.NeedVersionRewriteWarn);
    assert_eq!(0.6, job.indicators.ChangePercentage);
}

#[test]
fn create_static_and_dynamic_jobs_propagate_go_factory_fields() {
    let info = TableInfo {
        id: 11,
        indices: vec![IndexInfo {
            id: 8,
            is_public: true,
            ..IndexInfo::default()
        }],
    };
    let mut static_stats = analyzed_stats(100, 0);
    static_stats.stats_version = 1;
    let static_job = factory(0.0, tso(time_at(2, 0, 0, 0)))
        .CreateStaticPartitionAnalysisJob(&info, 101, Some(&static_stats))
        .expect("missing public index creates a static-partition job");
    let static_job = static_job
        .as_any()
        .downcast_ref::<StaticPartitionedTableAnalysisJob>()
        .expect("factory creates the static-partition job type");
    assert_eq!(11, static_job.GlobalTableID);
    assert_eq!(101, static_job.StaticPartitionID);
    assert_eq!(
        HashSet::from([8]),
        static_job.IndexIDs.keys().copied().collect()
    );
    assert!(static_job.NeedVersionRewriteWarn);

    let global = TableStats {
        column_count: 1,
        eligible: true,
        analyzed: true,
        stats_version: 2,
        ..TableStats::default()
    };
    let partitions = HashMap::from([(
        NewPartitionIDAndName("p0".to_owned(), 101),
        TableStats {
            realtime_count: 10,
            stats_version: 1,
            analyzed: false,
            ..TableStats::default()
        },
    )]);
    let dynamic_job = factory(0.5, tso(time_at(2, 0, 0, 0)))
        .CreateDynamicPartitionedTableAnalysisJob(&info, Some(&global), &partitions)
        .expect("unanalyzed partition creates a dynamic-partition job");
    let dynamic_job = dynamic_job
        .as_any()
        .downcast_ref::<DynamicPartitionedTableAnalysisJob>()
        .expect("factory creates the dynamic-partition job type");
    assert_eq!(11, dynamic_job.GlobalTableID);
    assert_eq!(
        HashSet::from([101]),
        dynamic_job.PartitionIDs.keys().copied().collect()
    );
    assert!(dynamic_job.NeedVersionRewriteWarn);
}

#[test]
fn analyze_version_matching_uses_stats_version_like_go() {
    let factory = factory(0.5, 0);
    assert!(factory.AnalyzeVersionMatches(&TableStats {
        analyzed: true,
        stats_version: 0,
        ..TableStats::default()
    }));
    assert!(factory.AnalyzeVersionMatches(&TableStats {
        pseudo: true,
        analyzed: true,
        stats_version: 1,
        ..TableStats::default()
    }));
    assert!(factory.AnalyzeVersionMatches(&TableStats {
        analyzed: false,
        stats_version: 2,
        ..TableStats::default()
    }));
    assert!(!factory.AnalyzeVersionMatches(&TableStats {
        analyzed: false,
        stats_version: 1,
        ..TableStats::default()
    }));

    let global = TableStats {
        stats_version: 2,
        ..TableStats::default()
    };
    let mut partitions = HashMap::from([(
        NewPartitionIDAndName("p0".to_owned(), 1),
        TableStats {
            stats_version: 2,
            ..TableStats::default()
        },
    )]);
    assert!(factory.PartitionedTableAnalyzeVersionMatches(&global, &partitions));
    partitions.values_mut().next().unwrap().stats_version = 1;
    assert!(!factory.PartitionedTableAnalyzeVersionMatches(&global, &partitions));
}

#[test]
fn calculate_indicators_for_partitions_matches_go_table_cases() {
    let current = time_at(4, 10, 0, 0);
    let last = current - Duration::from_secs(24 * 60 * 60);
    let global = TableStats {
        column_count: 2,
        ..TableStats::default()
    };
    let unanalyzed = HashMap::from([
        (
            NewPartitionIDAndName("p0".to_owned(), 1),
            TableStats {
                realtime_count: 1_001,
                ..TableStats::default()
            },
        ),
        (
            NewPartitionIDAndName("p1".to_owned(), 2),
            TableStats {
                realtime_count: 1_001,
                ..TableStats::default()
            },
        ),
    ]);
    let (change, size, duration, ids) =
        factory(0.5, tso(current)).CalculateIndicatorsForPartitions(&global, &unanalyzed);
    assert_eq!(1.0, change);
    assert_eq!(2_002.0, size);
    assert_eq!(Duration::from_secs(30 * 60), duration);
    assert_eq!(HashSet::from([1, 2]), ids.keys().copied().collect());

    let analyzed = HashMap::from([
        (
            NewPartitionIDAndName("p0".to_owned(), 1),
            TableStats {
                realtime_count: 1_001,
                modify_count: 2_002,
                analyze_row_count: 1_001,
                last_analyze_version: tso(last),
                analyzed: true,
                stats_version: 2,
                ..TableStats::default()
            },
        ),
        (
            NewPartitionIDAndName("p1".to_owned(), 2),
            TableStats {
                realtime_count: 1_001,
                analyze_row_count: 1_001,
                last_analyze_version: tso(last),
                analyzed: true,
                stats_version: 2,
                ..TableStats::default()
            },
        ),
    ]);
    let (change, size, duration, ids) =
        factory(0.5, tso(current)).CalculateIndicatorsForPartitions(&global, &analyzed);
    assert_eq!(2.0, change);
    assert_eq!(2_002.0, size);
    assert_eq!(Duration::from_secs(24 * 60 * 60), duration);
    assert_eq!(HashSet::from([1]), ids.keys().copied().collect());

    let (change, size, duration, ids) =
        factory(3.0, tso(current)).CalculateIndicatorsForPartitions(&global, &analyzed);
    assert_eq!(
        (0.0, 0.0, crate::AnalysisDuration::ZERO),
        (change, size, duration)
    );
    assert!(ids.is_empty());
}

#[test]
fn check_new_indexes_for_partitioned_table_matches_go_filters() {
    let info = TableInfo {
        id: 1,
        indices: vec![
            IndexInfo {
                id: 1,
                is_public: true,
                ..IndexInfo::default()
            },
            IndexInfo {
                id: 2,
                is_public: true,
                ..IndexInfo::default()
            },
            IndexInfo {
                id: 3,
                is_public: true,
                is_columnar: true,
                ..IndexInfo::default()
            },
            IndexInfo {
                id: 4,
                is_public: true,
                is_special_global: true,
                ..IndexInfo::default()
            },
            IndexInfo {
                id: 5,
                is_public: false,
                ..IndexInfo::default()
            },
        ],
    };
    let partitions = HashMap::from([
        (
            NewPartitionIDAndName("p0".to_owned(), 1),
            TableStats::default(),
        ),
        (
            NewPartitionIDAndName("p1".to_owned(), 2),
            TableStats {
                index_stats: HashSet::from([2]),
                ..TableStats::default()
            },
        ),
    ]);
    let mut indexes =
        factory(0.0, 0).CheckNewlyAddedIndexesNeedAnalyzeForPartitionedTable(&info, &partitions);
    for ids in indexes.values_mut() {
        ids.sort_unstable();
    }
    assert_eq!(HashMap::from([(1, vec![1, 2]), (2, vec![1])]), indexes);
}

#[derive(Default)]
struct TestStatsProvider {
    stats: HashMap<i64, TableStats>,
}

impl PartitionStatsProvider for TestStatsProvider {
    fn get_non_pseudo_physical_table_stats(&self, id: i64) -> Option<TableStats> {
        self.stats.get(&id).cloned()
    }
}

#[test]
fn get_partition_stats_keeps_only_found_eligible_stats() {
    let provider = TestStatsProvider {
        stats: HashMap::from([
            (
                1,
                TableStats {
                    eligible: true,
                    ..TableStats::default()
                },
            ),
            (2, TableStats::default()),
        ]),
    };
    let definitions = vec![
        PartitionDefinition {
            id: 1,
            name: "p0".to_owned(),
        },
        PartitionDefinition {
            id: 2,
            name: "p1".to_owned(),
        },
        PartitionDefinition {
            id: 3,
            name: "p2".to_owned(),
        },
    ];
    let stats = crate::GetPartitionStats(&provider, &definitions);
    assert_eq!(1, stats.len());
    assert!(stats.contains_key(&PartitionIDAndName {
        id: 1,
        name: "p0".to_owned(),
    }));
}

#[test]
fn auto_analysis_time_window_matches_go_cases_and_precision() {
    let within = NewAutoAnalysisTimeWindow(time_at(1, 1, 0, 0), time_at(1, 5, 0, 0));
    assert!(within.IsWithinTimeWindow(time_at(1, 3, 0, 0)));
    assert!(!within.IsWithinTimeWindow(time_at(1, 6, 0, 0)));

    let empty = NewAutoAnalysisTimeWindow(UNIX_EPOCH, UNIX_EPOCH);
    assert!(!empty.IsWithinTimeWindow(UNIX_EPOCH));

    let cross_midnight = NewAutoAnalysisTimeWindow(time_at(1, 22, 0, 0), time_at(1, 6, 0, 0));
    assert!(cross_midnight.IsWithinTimeWindow(time_at(2, 1, 0, 0)));
    assert!(!cross_midnight.IsWithinTimeWindow(time_at(1, 12, 0, 0)));

    // Go discards seconds and compares only UTC hours and minutes.
    let minute_precision = NewAutoAnalysisTimeWindow(time_at(1, 1, 0, 59), time_at(1, 5, 0, 1));
    assert!(minute_precision.IsWithinTimeWindow(time_at(1, 1, 0, 30)));
}

#[test]
fn future_analysis_time_preserves_negative_go_duration() {
    let current = time_at(1, 12, 0, 0);
    let mut stats = TableStats::default();
    stats.analyzed = true;
    stats.last_analyze_version = tso(current + Duration::from_secs(30));
    assert_eq!(
        factory(0.0, tso(current))
            .GetTableLastAnalyzeDuration(&stats)
            .as_secs_f64(),
        -30.0
    );
}

#[test]
fn partition_duration_average_preserves_sign_and_go_overflow() {
    let current = time_at(1, 12, 0, 0);
    let mut past = analyzed_stats(100, 100);
    past.last_analyze_version = tso(current - Duration::from_secs(10));
    let mut future = past.clone();
    future.last_analyze_version = tso(current + Duration::from_secs(30));
    let partitions = HashMap::from([
        (NewPartitionIDAndName("p0".into(), 1), past),
        (NewPartitionIDAndName("p1".into(), 2), future),
    ]);
    let global = analyzed_stats(100, 100);
    let (_, _, duration, ids) =
        factory(0.5, tso(current)).CalculateIndicatorsForPartitions(&global, &partitions);
    assert_eq!(duration.as_secs_f64(), -10.0);
    assert_eq!(ids.len(), 2);

    let ancient = analyzed_stats(100, 100);
    let partitions = HashMap::from([
        (NewPartitionIDAndName("p0".into(), 1), ancient.clone()),
        (NewPartitionIDAndName("p1".into(), 2), ancient),
    ]);
    let (_, _, duration, _) =
        factory(0.5, u64::MAX).CalculateIndicatorsForPartitions(&global, &partitions);
    assert_eq!(
        duration.as_nanos(),
        -1,
        "Go sums two saturated MaxInt64 durations with wrapping addition before dividing by 2"
    );
}

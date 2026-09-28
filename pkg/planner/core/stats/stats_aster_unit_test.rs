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

//! `pkg/planner/core/stats/stats.go` 的行为回归测试。
//!
//! 测试使用内存 StatsSource 模拟 Go 侧 domain/statistics handle，不模拟算法本身。

use std::cell::RefCell;
use std::collections::BTreeMap;

use super::stats::{
    OptimizationObjective, PartitionInfo, StatisticsTable, StatsSource, TableInfo, get_stats_table,
    load_table_stats, table_info_for_used_stats,
};

#[derive(Default)]
struct TestStatsSource {
    table_lookup: BTreeMap<i64, (TableInfo, Option<PartitionInfo>)>,
    stats: BTreeMap<i64, StatisticsTable>,
    requested_ids: RefCell<Vec<i64>>,
}

impl StatsSource for TestStatsSource {
    fn table_by_physical_id(&self, id: i64) -> Option<(TableInfo, Option<PartitionInfo>)> {
        self.table_lookup.get(&id).cloned()
    }

    fn physical_stats(&self, physical_id: i64, _table: &TableInfo) -> Option<StatisticsTable> {
        self.requested_ids.borrow_mut().push(physical_id);
        self.stats.get(&physical_id).cloned()
    }
}

fn table() -> TableInfo {
    TableInfo {
        id: 10,
        name: "orders".to_owned(),
        partitions: vec![
            PartitionInfo {
                id: 11,
                name: "p0".to_owned(),
            },
            PartitionInfo {
                id: 12,
                name: "p1".to_owned(),
            },
        ],
    }
}

fn stats(realtime_count: i64, modify_count: i64, analyze_count: i64) -> StatisticsTable {
    StatisticsTable {
        realtime_count,
        modify_count,
        analyze_count,
        version: 42,
        initialized: true,
        outdated: false,
        pseudo: false,
        allow_pseudo_loading: false,
    }
}

#[test]
fn used_stats_name_matches_go_table_partition_global_and_missing_cases() {
    let metadata = table();
    let mut source = TestStatsSource::default();
    source.table_lookup.insert(10, (metadata.clone(), None));
    source
        .table_lookup
        .insert(11, (metadata.clone(), Some(metadata.partitions[0].clone())));

    assert_eq!(
        table_info_for_used_stats(&source, 10),
        ("orders global".to_owned(), Some(metadata.clone()))
    );
    assert_eq!(
        table_info_for_used_stats(&source, 11),
        ("orders p0".to_owned(), Some(metadata.clone()))
    );
    assert_eq!(
        table_info_for_used_stats(&source, 99),
        ("tableID 99".to_owned(), None)
    );

    let plain = TableInfo {
        partitions: Vec::new(),
        ..metadata
    };
    source.table_lookup.insert(20, (plain.clone(), None));
    assert_eq!(
        table_info_for_used_stats(&source, 20),
        ("orders".to_owned(), Some(plain))
    );
}

#[test]
fn stats_table_routes_global_and_dynamic_partition_requests_like_go() {
    let metadata = table();
    let mut source = TestStatsSource::default();
    source.stats.insert(10, stats(100, 4, 90));
    source.stats.insert(11, stats(25, 2, 20));

    let partition_stats = get_stats_table(
        Some(&source),
        &metadata,
        11,
        false,
        OptimizationObjective::Default,
        false,
    );
    assert_eq!(partition_stats.realtime_count, 25);

    let global_stats = get_stats_table(
        Some(&source),
        &metadata,
        11,
        true,
        OptimizationObjective::Default,
        false,
    );
    assert_eq!(global_stats.realtime_count, 100);
    assert_eq!(&*source.requested_ids.borrow(), &[11, 10]);
}

#[test]
fn determinate_objective_uses_analyze_count_without_mutating_source() {
    let metadata = table();
    let mut source = TestStatsSource::default();
    source.stats.insert(11, stats(100, 7, 60));

    let selected = get_stats_table(
        Some(&source),
        &metadata,
        11,
        false,
        OptimizationObjective::Determinate,
        false,
    );
    assert_eq!(selected.realtime_count, 60);
    assert_eq!(selected.modify_count, 0);
    assert_eq!(selected.version, 42);
    assert_eq!(source.stats[&11].realtime_count, 100);
    assert_eq!(source.stats[&11].modify_count, 7);
}

#[test]
fn zero_analyze_count_returns_loadable_pseudo_table_for_real_stats() {
    let metadata = table();
    let mut source = TestStatsSource::default();
    source.stats.insert(11, stats(100, 7, 0));

    let selected = get_stats_table(
        Some(&source),
        &metadata,
        11,
        false,
        OptimizationObjective::Determinate,
        false,
    );
    assert_eq!(selected.realtime_count, 10_000);
    assert_eq!(selected.version, 0);
    assert!(selected.pseudo);
    assert!(selected.allow_pseudo_loading);
}

#[test]
fn missing_handle_and_zero_rows_return_non_loadable_pseudo_table() {
    let metadata = table();
    let missing = get_stats_table(
        None,
        &metadata,
        10,
        false,
        OptimizationObjective::Default,
        false,
    );
    assert_eq!(missing.realtime_count, 10_000);
    assert!(missing.pseudo);
    assert!(!missing.allow_pseudo_loading);

    let mut source = TestStatsSource::default();
    source.stats.insert(10, stats(0, 0, 0));
    let empty = get_stats_table(
        Some(&source),
        &metadata,
        10,
        false,
        OptimizationObjective::Default,
        false,
    );
    assert_eq!(empty.realtime_count, 10_000);
    assert!(empty.pseudo);
    assert!(!empty.allow_pseudo_loading);
}

#[test]
fn initialization_and_outdated_switches_match_go_pseudo_rules() {
    let metadata = table();
    let mut source = TestStatsSource::default();
    let mut uninitialized = stats(100, 4, 90);
    uninitialized.initialized = false;
    source.stats.insert(10, uninitialized);

    let uninitialized_result = get_stats_table(
        Some(&source),
        &metadata,
        10,
        false,
        OptimizationObjective::Default,
        false,
    );
    assert!(uninitialized_result.pseudo);
    assert_eq!(uninitialized_result.realtime_count, 100);
    assert_eq!(uninitialized_result.version, 42);

    let mut outdated = stats(100, 4, 90);
    outdated.outdated = true;
    source.stats.insert(10, outdated);
    assert!(
        get_stats_table(
            Some(&source),
            &metadata,
            10,
            false,
            OptimizationObjective::Default,
            true,
        )
        .pseudo
    );
    assert!(
        !get_stats_table(
            Some(&source),
            &metadata,
            10,
            false,
            OptimizationObjective::Default,
            false,
        )
        .pseudo
    );
}

#[test]
fn load_table_stats_records_partition_name_and_skips_duplicate_ids() {
    let metadata = table();
    let mut source = TestStatsSource::default();
    source.stats.insert(11, stats(25, 2, 20));
    let mut record = BTreeMap::new();

    load_table_stats(
        &mut record,
        Some(&source),
        &metadata,
        11,
        false,
        OptimizationObjective::Default,
        false,
    );
    assert_eq!(record.len(), 1);
    assert_eq!(record[&11].name, "orders p0");
    assert_eq!(record[&11].realtime_count, 25);
    assert_eq!(record[&11].modify_count, 2);
    assert_eq!(record[&11].version, 42);

    source.stats.insert(11, stats(999, 0, 999));
    load_table_stats(
        &mut record,
        Some(&source),
        &metadata,
        11,
        false,
        OptimizationObjective::Default,
        false,
    );
    assert_eq!(record[&11].realtime_count, 25);
    assert_eq!(record[&11].version, 42);
}

#[test]
fn load_table_stats_for_pseudo_table_records_pseudo_version() {
    let metadata = table();
    let mut source = TestStatsSource::default();
    let mut pseudo = stats(100, 0, 100);
    pseudo.pseudo = true;
    source.stats.insert(11, pseudo);
    let mut record = BTreeMap::new();

    load_table_stats(
        &mut record,
        Some(&source),
        &metadata,
        11,
        false,
        OptimizationObjective::Default,
        false,
    );
    assert_eq!(record[&11].version, 0);
}

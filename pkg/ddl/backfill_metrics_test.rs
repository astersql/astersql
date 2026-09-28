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

use crate::backfill_metrics::{
    BackfillAction, BackfillMetrics, LABEL_ADD_INDEX, LABEL_ADD_INDEX_MERGE,
    LABEL_CLEANUP_INDEX_RATE, LABEL_MODIFY_COLUMN, LABEL_REORG_PARTITION,
    LABEL_REORG_PARTITION_RATE, MetricKey, ReorganizationMetricInfo, backfill_metrics_table_id,
    backfill_progress_label,
};

#[test]
fn backfill_progress_labels_match_go_actions() {
    assert_eq!(LABEL_ADD_INDEX, "add_index");
    assert_eq!(LABEL_ADD_INDEX_MERGE, "add_index_merge_tmp");
    assert_eq!(LABEL_MODIFY_COLUMN, "modify_column");
    assert_eq!(LABEL_REORG_PARTITION, "reorg_partition");
    assert_eq!(LABEL_REORG_PARTITION_RATE, "reorg_partition_rate");
    assert_eq!(LABEL_CLEANUP_INDEX_RATE, "cleanup_idx_rate");
    let cases = [
        (BackfillAction::AddIndex, false, LABEL_ADD_INDEX),
        (BackfillAction::AddPrimaryKey, true, LABEL_ADD_INDEX_MERGE),
        (BackfillAction::ModifyColumn, false, LABEL_MODIFY_COLUMN),
        (
            BackfillAction::ReorganizePartition,
            false,
            LABEL_REORG_PARTITION,
        ),
        (
            BackfillAction::AlterTablePartitioning,
            false,
            LABEL_REORG_PARTITION,
        ),
        (
            BackfillAction::RemovePartitioning,
            false,
            LABEL_REORG_PARTITION,
        ),
        (BackfillAction::Other, false, ""),
    ];
    for (action, merging, expected) in cases {
        assert_eq!(backfill_progress_label(action, merging), expected);
    }
}

#[test]
fn production_metrics_store_accumulates_and_cleans_by_table_id() {
    let table_id = 12_345;
    let other_table_id = 12_346;
    let key = MetricKey {
        table_id,
        label: LABEL_ADD_INDEX.to_owned(),
        schema_name: "test_db_1".to_owned(),
        table_name: "t".to_owned(),
        object_name: "idx".to_owned(),
    };
    let other_key = MetricKey {
        table_id: other_table_id,
        ..key.clone()
    };
    let mut metrics = BackfillMetrics::default();
    metrics.add_total(key.clone(), 40.0);
    metrics.add_total(key.clone(), 60.0);
    metrics.set_progress(key.clone(), 50.0);
    metrics.add_total(other_key.clone(), 1.0);

    assert_eq!(metrics.total(&key), 100.0);
    assert_eq!(metrics.progress(&key), 50.0);
    metrics.clear_table(table_id);
    assert_eq!(metrics.total(&key), 0.0);
    assert_eq!(metrics.progress(&key), 0.0);
    assert_eq!(metrics.total(&other_key), 1.0);
    metrics.clear_table(table_id);
}

#[test]
fn metric_table_id_selection_matches_go_job_presence_rules() {
    let logical_table_id = 300;
    let physical_table_id = 301;
    let cases = [
        (
            BackfillAction::ReorganizePartition,
            Some(logical_table_id),
            LABEL_REORG_PARTITION,
            logical_table_id,
        ),
        (
            BackfillAction::ReorganizePartition,
            Some(logical_table_id),
            LABEL_REORG_PARTITION_RATE,
            logical_table_id,
        ),
        (
            BackfillAction::ReorganizePartition,
            Some(logical_table_id),
            "reorg_partition_rate-conflict",
            logical_table_id,
        ),
        (
            BackfillAction::AlterTablePartitioning,
            Some(logical_table_id),
            LABEL_REORG_PARTITION_RATE,
            logical_table_id,
        ),
        (
            BackfillAction::RemovePartitioning,
            Some(logical_table_id),
            LABEL_REORG_PARTITION,
            logical_table_id,
        ),
        (
            BackfillAction::DropTablePartition,
            Some(logical_table_id),
            LABEL_CLEANUP_INDEX_RATE,
            logical_table_id,
        ),
        (
            BackfillAction::TruncateTablePartition,
            Some(logical_table_id),
            LABEL_CLEANUP_INDEX_RATE,
            logical_table_id,
        ),
        (
            BackfillAction::AddIndex,
            Some(logical_table_id),
            LABEL_CLEANUP_INDEX_RATE,
            physical_table_id,
        ),
        (
            BackfillAction::ReorganizePartition,
            None,
            LABEL_REORG_PARTITION_RATE,
            physical_table_id,
        ),
        (
            BackfillAction::DropTablePartition,
            None,
            LABEL_CLEANUP_INDEX_RATE,
            physical_table_id,
        ),
    ];

    for (action, logical_table_id, label, expected) in cases {
        let info = ReorganizationMetricInfo {
            action,
            logical_table_id,
            physical_table_id,
        };
        assert_eq!(backfill_metrics_table_id(Some(&info), label), expected);
    }
    assert_eq!(backfill_metrics_table_id(None, LABEL_ADD_INDEX), 0);
}

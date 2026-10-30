// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// executor 包级结果字段、Index Join 范围与慢查询统计的单元测试。
//
// 验证 `colNames2ResultFields`：库名回退、原始列/表名、以及别名长度截断
//（MySQL 兼容上限 256 字符）。

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::adapter::{FieldName, SchemaColumn, colNames2ResultFields};
use crate::builder::{IndexJoinLookUpContent, LogicalRange, buildRangesForIndexJoin};
use crate::distsql::{
    IndexLookUpExecutor, IndexLookUpRunTimeStats, IndexReaderExecutor, KeyRange,
    newIndexLookUpExecutorContext, newIndexReaderExecutorContext,
};
use crate::slow_query::slowQueryRuntimeStats;
use crate::table_readers_required_rows_test::RequiredRowsBackend;
use astersql_executor_internal_exec::adaptive_limit_controller::{
    AdaptiveLimitConfig, AdaptiveLimitController,
};
use astersql_util_memory::tracker::Tracker;

/// 空 db_name 回退到会话库；过长 alias 截断到 256；表名用 original。
#[test]
fn executor_result_fields_apply_database_fallback_original_name_and_alias_limit() {
    let alias = "x".repeat(300);
    let fields = colNames2ResultFields(
        &[SchemaColumn {
            field_type: astersql_parser_types::NewFieldType(
                astersql_parser_mysql::r#type::TypeLonglong,
            ),
        }],
        &[FieldName {
            db_name: String::new(),
            table_name: "orders_alias".into(),
            original_table_name: "orders".into(),
            column_name: alias,
            original_column_name: "order_id".into(),
        }],
        "shop",
    );
    assert_eq!(fields[0].database_name, "shop");
    assert_eq!(fields[0].column_name, "order_id");
    assert_eq!(fields[0].column_alias.chars().count(), 256);
    assert_eq!(fields[0].table_name, "orders");
    assert_eq!(fields[0].table_alias, "orders_alias");
}

/// 对齐 Go `colNames2ResultFields` 的其余分支：无表名时不回退数据库，
/// 缺少原始列名时使用别名并设置 `EmptyOrgName`。
#[test]
fn executor_result_fields_preserve_empty_database_and_fall_back_to_alias_name() {
    let fields = colNames2ResultFields(
        &[SchemaColumn::default()],
        &[FieldName {
            column_name: "computed".into(),
            ..FieldName::default()
        }],
        "shop",
    );

    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].database_name, "");
    assert_eq!(fields[0].column_name, "computed");
    assert_eq!(fields[0].column_alias, "computed");
    assert!(fields[0].empty_original_name);
}

/// 对齐 Go `TestBuildKvRangesForIndexJoinWithoutCwc` 的核心范围展开契约。
#[test]
fn index_join_ranges_replace_each_join_key_offset_without_mutating_templates() {
    let templates = vec![
        LogicalRange {
            low: vec![1, 10, 2, 20, 3],
            high: vec![1, 10, 2, 20, 3],
        },
        LogicalRange {
            low: vec![4, 10, 5, 20, 6],
            high: vec![4, 10, 5, 20, 6],
        },
    ];
    let lookups = vec![
        IndexJoinLookUpContent {
            partition_id: 0,
            handle: Vec::new(),
            key_values: vec![7, 8],
        },
        IndexJoinLookUpContent {
            partition_id: 0,
            handle: Vec::new(),
            key_values: vec![9, 10],
        },
    ];

    let ranges =
        buildRangesForIndexJoin(&lookups, &templates, &[1, 3]).expect("expand index join ranges");

    assert_eq!(ranges.len(), 4);
    assert_eq!(ranges[0].low, vec![1, 7, 2, 8, 3]);
    assert_eq!(ranges[1].high, vec![4, 7, 5, 8, 6]);
    assert_eq!(ranges[2].low, vec![1, 9, 2, 10, 3]);
    assert_eq!(ranges[3].high, vec![4, 9, 5, 10, 6]);
    assert_eq!(templates[0].low, vec![1, 10, 2, 20, 3]);
}

#[test]
fn index_reader_partition_ranges_use_index_join_memory_tracker() {
    let backend = RequiredRowsBackend::new(Vec::new(), Duration::ZERO);
    let tracker = Arc::new(Tracker::new(1, -1));
    let mut reader = IndexReaderExecutor {
        context: newIndexReaderExecutorContext(Arc::new(backend), 1, false),
        table_id: 101,
        index_id: 1,
        plans: Vec::new(),
        ranges: vec![KeyRange {
            start: vec![1],
            end: vec![2],
        }],
        access_conditions: Vec::new(),
        index_columns: Vec::new(),
        column_lengths: Vec::new(),
        table_ids: vec![101, 102],
        by_items: Vec::new(),
        descending: false,
        keep_order: false,
        dummy: true,
        result: None,
        merged_rows: VecDeque::new(),
        runtime_rows: 0,
        range_mem_tracker: Some(Arc::clone(&tracker)),
    };

    reader.Open().expect("build partition index ranges");
    assert!(tracker.BytesConsumed() > 0);
}

#[test]
fn index_lookup_partition_ranges_prefer_index_join_tracker_and_fall_back_to_executor_tracker() {
    let backend = RequiredRowsBackend::new(Vec::new(), Duration::ZERO);
    let index_join_tracker = Arc::new(Tracker::new(1, -1));
    let executor_tracker = Arc::new(Tracker::new(2, -1));
    let mut reader = IndexLookUpExecutor {
        context: newIndexLookUpExecutorContext(Arc::new(backend), 1, false),
        table_id: 101,
        index_id: 1,
        idx_plans: Vec::new(),
        tbl_plans: Vec::new(),
        ranges: vec![KeyRange {
            start: vec![1],
            end: vec![2],
        }],
        grouped_kv_ranges: Vec::new(),
        grouped_ranges: Vec::new(),
        partition_range_map: BTreeMap::from([(
            102,
            vec![KeyRange {
                start: vec![3],
                end: vec![4],
            }],
        )]),
        handle_offsets: Vec::new(),
        common_handle: false,
        partition_mode: true,
        keep_order: false,
        descending: false,
        pushed_limit: None,
        batch_size: 1,
        max_batch_size: 1,
        check_index_value: None,
        dummy: true,
        cancelled: Arc::default(),
        result_tx: None,
        result_rx: None,
        table_tx: None,
        index_join: None,
        table_joins: Vec::new(),
        pending: BTreeMap::new(),
        next_task_id: 0,
        current: VecDeque::new(),
        adaptive_current: VecDeque::new(),
        stats: Arc::new(Mutex::new(IndexLookUpRunTimeStats::default())),
        adaptive_limit_controller: None,
        report_adaptive_limit_stats: false,
        mem_tracker: Some(Arc::clone(&executor_tracker)),
        range_mem_tracker: Some(Arc::clone(&index_join_tracker)),
    };

    reader
        .buildTableKeyRanges()
        .expect("build index join ranges");
    assert!(index_join_tracker.BytesConsumed() > 0);
    assert_eq!(executor_tracker.BytesConsumed(), 0);

    reader.range_mem_tracker = None;
    reader
        .buildTableKeyRanges()
        .expect("build regular index lookup ranges");
    assert!(executor_tracker.BytesConsumed() > 0);
}

#[test]
fn ordered_index_lookup_reports_completed_tasks_to_adaptive_limit_controller() {
    let backend = RequiredRowsBackend::new(
        vec![
            vec![crate::distsql::Datum::Signed(1)],
            vec![crate::distsql::Datum::Signed(2)],
        ],
        Duration::ZERO,
    );
    let controller = Arc::new(AdaptiveLimitController::NewAdaptiveLimitLookupController(
        AdaptiveLimitConfig {
            demand_rows: 2,
            initial_lookup_window: 2,
            max_lookup_window: 8,
            initial_lookup_batch_size: 2,
            max_lookup_batch_size: 8,
            ..AdaptiveLimitConfig::default()
        },
    ));
    let mut reader = IndexLookUpExecutor {
        context: newIndexLookUpExecutorContext(Arc::new(backend), 2, false),
        table_id: 101,
        index_id: 1,
        idx_plans: Vec::new(),
        tbl_plans: Vec::new(),
        ranges: vec![KeyRange {
            start: vec![1],
            end: vec![2],
        }],
        grouped_kv_ranges: Vec::new(),
        grouped_ranges: Vec::new(),
        partition_range_map: BTreeMap::new(),
        handle_offsets: vec![0],
        common_handle: false,
        partition_mode: false,
        keep_order: true,
        descending: false,
        pushed_limit: None,
        batch_size: 2,
        max_batch_size: 8,
        check_index_value: None,
        dummy: false,
        cancelled: Arc::default(),
        result_tx: None,
        result_rx: None,
        table_tx: None,
        index_join: None,
        table_joins: Vec::new(),
        pending: BTreeMap::new(),
        next_task_id: 0,
        current: VecDeque::new(),
        adaptive_current: VecDeque::new(),
        stats: Arc::new(Mutex::new(IndexLookUpRunTimeStats::default())),
        adaptive_limit_controller: Some(Arc::clone(&controller)),
        report_adaptive_limit_stats: true,
        mem_tracker: None,
        range_mem_tracker: None,
    };

    reader.Open().unwrap();
    assert_eq!(reader.Next(1).unwrap().len(), 1);
    assert_eq!(controller.Snapshot().lookup_reserved, 2);
    assert_eq!(reader.Next(1).unwrap().len(), 1);
    assert_eq!(controller.Snapshot().lookup_handles, 2);
    assert_eq!(controller.Snapshot().lookup_rows, 2);
    assert!(controller.Snapshot().stopped);
    reader.Close().unwrap();
    assert!(
        reader
            .stats
            .lock()
            .unwrap()
            .String()
            .contains("adaptive:{lookup:2/2")
    );
}

/// 完整保留 Go `TestSlowQueryRuntimeStats` 的 String/Clone/Merge 断言。
#[test]
fn slow_query_runtime_stats_match_go_format_clone_and_merge_contract() {
    let mut stats = slowQueryRuntimeStats {
        totalFileNum: 2,
        readFileNum: 2,
        readFile: Duration::from_secs(1),
        initialize: Duration::from_millis(1),
        readFileSize: 1024 * 1024 * 1024,
        parseLog: Duration::from_millis(100),
        concurrent: 15,
    };
    assert_eq!(
        stats.String(),
        "initialize: 1ms, read_file: 1s, parse_log: {time:100ms, concurrency:15}, total_file: 2, read_file: 2, read_size: 1024 MB"
    );
    assert_eq!(stats.Clone().String(), stats.String());

    stats.Merge(&stats.Clone());
    assert_eq!(
        stats.String(),
        "initialize: 2ms, read_file: 2s, parse_log: {time:200ms, concurrency:15}, total_file: 4, read_file: 4, read_size: 2 GB"
    );
}

#[test]
fn port_37a4cd4f43_complete_stats_string_merge_clone_reset() {
    use crate::mview_complete_delta_apply::{
        CompleteDeltaApplyRuntimeStats as S, RefreshWriterStats as W,
    };
    use astersql_util_execdetails::execdetails::{
        RuntimeStats, TpMViewCompleteDeltaApplyRuntimeStats,
    };
    let mut left = S {
        writer_time: Duration::from_millis(5),
        writer_detail: W {
            chunks: 1,
            row_ops: 4,
            insert_rows: 1,
            update_rows: 2,
            delete_rows: 1,
            ..Default::default()
        },
    };
    let right = S {
        writer_time: Duration::from_millis(7),
        writer_detail: W {
            chunks: 2,
            row_ops: 5,
            update_rows: 1,
            delete_rows: 1,
            ..Default::default()
        },
    };
    left.Merge(&right);
    assert_eq!(left.writer_time, Duration::from_millis(12));
    assert_eq!(
        left.writer_detail,
        W {
            chunks: 3,
            row_ops: 9,
            insert_rows: 1,
            update_rows: 3,
            delete_rows: 2,
            ..Default::default()
        }
    );
    assert_eq!(
        left.String(),
        "mview_complete_delta_apply:{writer:{time:12ms, chunks:3, row_ops:9, rows:{insert:1, update:3, delete:2}}}"
    );
    let cloned = left.CloneBox();
    assert_eq!(cloned.String(), left.String());
    assert_eq!(cloned.Tp(), TpMViewCompleteDeltaApplyRuntimeStats);
    left.Merge(&crate::mviewdeltamergeagg::MergeRuntimeStats::new(0));
    assert_eq!(cloned.String(), left.String());
    left.reset();
    assert_eq!(left.writer_detail, W::default());
    assert_eq!(left.writer_time, Duration::ZERO);
    assert!(cloned.String().contains("time:12ms"));
}

#[test]
fn port_37a4cd4f43_merge_stats_string_merge_clone_reset() {
    use crate::mview_complete_delta_apply::RefreshWriterStats as W;
    use crate::mviewdeltamergeagg::MergeRuntimeStats as S;
    use astersql_util_execdetails::execdetails::{RuntimeStats, TpMViewDeltaMergeAggRuntimeStats};
    let mut stats = S::new(2);
    stats.reader_time = Duration::from_secs(1);
    stats.writer_time = Duration::from_secs(2);
    stats.merge_worker_time[0] = Duration::from_secs(3);
    stats.writer_detail = W {
        chunks: 1,
        row_ops: 2,
        insert_rows: 1,
        noop_rows: 1,
        ..Default::default()
    };
    let clone = stats.CloneBox();
    assert_eq!(clone.String(), stats.String());
    assert_eq!(clone.Tp(), TpMViewDeltaMergeAggRuntimeStats);
    let mut other = S::new(2);
    other.reader_time = Duration::from_secs(1);
    other.merge_worker_time[1] = Duration::from_secs(4);
    other.writer_detail = W {
        chunks: 2,
        row_ops: 3,
        update_rows: 2,
        ..Default::default()
    };
    stats.Merge(&other);
    assert_eq!(stats.reader_time, Duration::from_secs(2));
    assert_eq!(stats.writer_detail.chunks, 3);
    assert_eq!(
        stats.String(),
        "mview_delta_merge_agg:{merge_worker:{total:2, active:2, min:3s, max:4s, avg:3.5s}, reader:2s, writer:{time:2s, chunks:3, row_ops:5, rows:{insert:1, update:2, delete:0, noop:1}}}"
    );
    stats.Merge(&S::new(4));
    assert_eq!(stats.merge_worker_time.len(), 4);
    assert!(clone.String().contains("active:1"));
    stats.Merge(&crate::mview_complete_delta_apply::CompleteDeltaApplyRuntimeStats::default());
    stats.reset(1);
    assert_eq!(stats.merge_worker_time, vec![Duration::ZERO]);
    assert_eq!(stats.writer_detail, W::default());
    stats.reset(-1);
    assert!(stats.merge_worker_time.is_empty());
    assert_eq!(
        S::new(-1).String(),
        "mview_delta_merge_agg:{merge_worker:{total:0, active:0}, reader:0s, writer:{time:0s, chunks:0, row_ops:0, rows:{insert:0, update:0, delete:0, noop:0}}}"
    );
}

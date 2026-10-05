// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

use crate::backfilling_read_index::{
    DistTaskRowCountCollector, ReadIndexStepExecutor, SortedKvMeta, StepResource, Subtask,
    SubtaskSummary,
};

#[test]
fn nonempty_cloud_uri_uses_global_sort_like_go() {
    let mut executor = ReadIndexStepExecutor::new(1, vec![11], 7);

    executor.init("   ").unwrap();

    assert!(executor.use_cloud_storage);
}

#[test]
fn cloud_task_meta_change_does_not_update_local_backend_speed() {
    let mut executor = ReadIndexStepExecutor::new(1, vec![11], 7);
    executor.init("s3://bucket/prefix").unwrap();
    executor.max_write_speed = 100;

    executor.task_meta_modified(512, 200);

    assert_eq!(512, executor.batch_size);
    assert_eq!(100, executor.max_write_speed);
}

#[test]
fn global_sort_completion_records_all_index_ids_in_subtask_meta() {
    let mut executor = ReadIndexStepExecutor::new(1, vec![11, 12], 7);
    executor.init("s3://bucket/prefix").unwrap();
    let mut subtask = Subtask::default();

    executor
        .run_subtask(
            &mut subtask,
            StepResource { cpu: 4 },
            vec![SortedKvMeta::default(), SortedKvMeta::default()],
            SubtaskSummary::default(),
        )
        .unwrap();

    assert_eq!(vec![11, 12], subtask.meta.element_ids);
}

#[test]
fn counters_preserve_go_cast_and_overflow_semantics() {
    let mut collector = DistTaskRowCountCollector::default();
    collector.accepted(-1);
    assert_eq!(u64::MAX, collector.cluster_read_bytes);

    collector.summary.processed_bytes = i64::MAX;
    collector.summary.row_count = i64::MAX;
    collector.metric_row_count = i64::MAX;
    collector.processed(1, 1);
    assert_eq!(i64::MIN, collector.summary.processed_bytes);
    assert_eq!(i64::MIN, collector.summary.row_count);
    assert_eq!(i64::MIN, collector.metric_row_count);
}

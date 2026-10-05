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

// Hash Join V2 Inner Join 的 spill（内存落盘）单元测试。
//
// 校验 spill helper 选择最大分区写入 build/probe 并 restore、OOM action
// 在可 spill / 不可 spill 时的状态迁移，以及 reset 与 round 上限清理。

use crate::hash_join_spill::{HashJoinSpillAction, OomAction, has_enough_data_to_spill};
use crate::hash_join_spill_helper::{HashJoinSpillHelper, MemoryTracker, SpillStatus};
use crate::hash_join_v2::{HashJoinCtxV2, HashJoinV2Exec};
use crate::join_row_table::{RowTable, RowTableSegment};
use crate::join_table_meta::EncodedRow;
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use astersql_util_execdetails::execdetails::NewRuntimeStatsColl;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn execute_inner_join(
    right_as_build_side: bool,
    memory_limit: Option<i64>,
    condition: Option<Predicate>,
) -> (HashJoinV2Exec, Vec<Row>) {
    let context = HashJoinCtxV2::new(
        JoinType::Inner,
        vec![0, 1],
        vec![0, 1],
        right_as_build_side,
        false,
        2,
        2,
        memory_limit,
    )
    .unwrap();
    let joiner = Joiner::new(
        JoinType::Inner,
        right_as_build_side,
        vec![],
        condition.into_iter().collect(),
        Some([vec![0, 2], vec![0, 2]]),
        false,
        2,
    )
    .unwrap();
    let build = vec![vec![
        vec![Value::Int(1), Value::Text("a".into()), Value::Int(10)],
        vec![Value::Int(2), Value::Text("b".into()), Value::Int(20)],
        vec![Value::Int(3), Value::Text("c".into()), Value::Int(30)],
    ]];
    let probe = vec![vec![
        vec![Value::Int(1), Value::Text("a".into()), Value::Int(9)],
        vec![Value::Int(2), Value::Text("b".into()), Value::Int(25)],
        vec![Value::Int(4), Value::Text("d".into()), Value::Int(40)],
    ]];
    let mut executor = HashJoinV2Exec::new(context, joiner, build, probe).unwrap();
    let rows = executor.execute_all().unwrap();
    (executor, rows)
}

fn sorted(mut rows: Vec<Row>) -> Vec<Row> {
    rows.sort_by_key(|row| format!("{row:?}"));
    rows
}

/// 与 Go 的 no-spill oracle 对照，覆盖左右 build side、部分分区 spill 与 restore。
#[test]
fn inner_join_spill_matches_no_spill_for_both_build_sides() {
    for right_as_build_side in [false, true] {
        let (_, expected) = execute_inner_join(right_as_build_side, None, None);
        let (spilled, actual) = execute_inner_join(right_as_build_side, Some(1), None);
        assert_eq!(expected.len(), 2);
        assert_eq!(sorted(actual), sorted(expected));
        assert!(
            spilled
                .stats
                .spill
                .spilled_partition_num
                .iter()
                .sum::<usize>()
                > 0
        );
        assert!(spilled.stats.spill.spilled_bytes.iter().sum::<i64>() > 0);
        assert!(spilled.stats.spill.restored_bytes.iter().sum::<i64>() > 0);
    }
}

/// Go 的 other condition 会过滤已命中的 joined row；spill 前后结果必须相同。
#[test]
fn inner_join_spill_preserves_other_condition() {
    let condition: Predicate = Arc::new(|row| match row.first() {
        Some(Value::Int(key)) => Ok(Some(*key == 1)),
        _ => Ok(None),
    });
    let (_, expected) = execute_inner_join(false, None, Some(condition.clone()));
    let (_, actual) = execute_inner_join(false, Some(1), Some(condition));
    assert_eq!(sorted(actual), sorted(expected.clone()));
    assert_eq!(expected.len(), 1);
}

/// Go under-apply 场景会对同一 executor 重复 Close/Open，每轮结果保持一致。
#[test]
fn inner_join_spill_executor_can_close_and_reopen_repeatedly() {
    let (mut executor, expected) = execute_inner_join(true, Some(1), None);
    for _ in 0..10 {
        executor.close();
        executor.open().unwrap();
        assert_eq!(
            sorted(executor.execute_all().unwrap()),
            sorted(expected.clone())
        );
    }
}

/// Typed hash-state evidence merges completed repeated opens and is poisoned by a failed build.
#[test]
fn hash_join_hash_state_tracks_repeated_open_and_failure() {
    let context = HashJoinCtxV2::new(
        JoinType::Inner,
        vec![0],
        vec![0],
        true,
        false,
        2,
        8,
        Some(1),
    )
    .unwrap();
    let joiner = Joiner::new(
        JoinType::Inner,
        true,
        vec![],
        vec![],
        Some([vec![0], vec![0]]),
        false,
        2,
    )
    .unwrap();
    let build = vec![vec![
        vec![Value::Int(1)],
        vec![Value::Int(2)],
        vec![Value::Int(3)],
    ]];
    let probe = vec![vec![vec![Value::Int(1)], vec![Value::Int(3)]]];
    let runtime_stats = Arc::new(Mutex::new(NewRuntimeStatsColl(None)));
    let mut executor = HashJoinV2Exec::new(context, joiner, build.clone(), probe)
        .unwrap()
        .with_runtime_stats(9, runtime_stats.clone());

    for _ in 0..2 {
        executor.open().unwrap();
        assert_eq!(executor.execute_all().unwrap().len(), 2);
        executor.close();
    }
    let snapshot = runtime_stats
        .lock()
        .unwrap()
        .GetRootHashStateRowsSnapshot(9)
        .unwrap();
    assert!(snapshot.Complete());
    assert_eq!(snapshot.Rows, 6);

    executor.set_build_chunks(vec![vec![Vec::new()]]);
    executor.open().unwrap();
    assert!(executor.execute_all().is_err());
    executor.close();
    let snapshot = runtime_stats
        .lock()
        .unwrap()
        .GetRootHashStateRowsSnapshot(9)
        .unwrap();
    assert!(snapshot.Invalid());
    assert!(!snapshot.Complete());
}

/// 构造单行单 segment 的 `RowTable` 夹具，指定 hash 与字节占用。
fn table(hash: u64, bytes: usize) -> RowTable {
    let mut table = RowTable::default();
    table.segments_mut().push(RowTableSegment {
        rows: vec![EncodedRow {
            bytes: vec![7; bytes],
            null_map: vec![0],
            key_offset: 0,
            key_length: bytes.min(8),
            row_data_offset: bytes.min(8),
            used: AtomicBool::new(false),
        }],
        hash_values: vec![hash],
        valid_key_count: 1,
        ..Default::default()
    });
    table
}

/// 选择最大分区 spill build，再写 probe chunk，restore 后两侧均有数据。
#[test]
fn spill_helper_selects_largest_partitions_writes_build_probe_and_restores() {
    let helper = HashJoinSpillHelper::new(4, 2, 3, 100).unwrap();
    helper.memory_tracker.consume(400);
    helper.set_can_spill_flag(true);
    let mut tables = vec![
        vec![table(1, 20), table(2, 40), table(3, 80), table(4, 10)],
        vec![table(5, 20), table(6, 40), table(7, 80), table(8, 10)],
    ];
    // 分区内存估计驱动选择最大分区落盘。
    let released = helper
        .spill_row_tables(&mut tables, &[40, 80, 160, 20], None)
        .unwrap();
    assert!(released >= 160);
    assert!(helper.spilled_partition_count() >= 1);
    assert!(helper.build_spill_bytes() > 0);

    let partition = helper.spilled_partitions()[0];
    let probe_rows = vec![vec![Value::Int(9)]];
    helper.spill_probe_chunk(0, partition, &probe_rows).unwrap();
    assert!(helper.probe_spill_bytes() > 0);
    helper.prepare_for_restoring(0).unwrap();
    let mut restored = Vec::new();
    while let Some(partition) = helper.pop_restore_partition() {
        restored.push(partition);
    }
    assert!(restored.iter().all(|partition| partition.round == 1));
    assert!(
        restored
            .iter()
            .all(|partition| !partition.build_side_chunks.is_empty())
    );
    assert!(
        restored
            .iter()
            .any(|partition| !partition.probe_side_chunks.is_empty())
    );
}

/// 不可 spill 时的 OOM 回退 action，计数调用次数。
#[derive(Default)]
struct Fallback(AtomicUsize);
impl OomAction for Fallback {
    fn priority(&self) -> i64 {
        1
    }
    fn action(&self, _tracker: &MemoryTracker) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// 可 spill 时进入 NeedSpill；关闭 spill 标志后应触发 fallback action。
#[test]
fn spill_action_transitions_need_spill_and_falls_back_when_spill_disabled() {
    let helper = Arc::new(HashJoinSpillHelper::new(2, 1, 2, 100).unwrap());
    helper.memory_tracker.consume(20);
    helper.set_can_spill_flag(true);
    let tracker = MemoryTracker::new(100);
    tracker.consume(101);
    assert!(has_enough_data_to_spill(&helper.memory_tracker, &tracker));
    let fallback = Arc::new(Fallback::default());
    let action = HashJoinSpillAction::new(helper.clone()).with_fallback(fallback.clone());
    action.action(&tracker);
    assert_eq!(helper.status(), SpillStatus::NeedSpill);

    helper.set_not_spilled();
    helper.set_can_spill_flag(false);
    action.action(&tracker);
    assert_eq!(fallback.0.load(Ordering::SeqCst), 1);
}

/// reset 清空 spilled 状态；超过 round 上限时 prepare_for_restoring 应失败。
#[test]
fn spill_reset_and_round_limit_cleanup_all_state() {
    let helper = HashJoinSpillHelper::new(2, 1, 1, 64).unwrap();
    helper.set_partition_spilled(&[0, 1]).unwrap();
    assert!(helper.are_all_partitions_spilled());
    helper.reset();
    assert_eq!(helper.spilled_partition_count(), 0);
    assert!(!helper.is_spill_triggered());
    assert!(helper.prepare_for_restoring(1).is_err());
    helper.close();
}

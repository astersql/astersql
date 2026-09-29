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

// Outer Join spill（落盘）相关单元测试。
//
// 验证 probe 剩余行 spill/恢复后仍保持 Left Outer 未匹配语义，以及
// `HashJoinSpillHelper` 对 build/probe 两侧的写入、恢复与 round 限制。

use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::hash_join_spill_helper::HashJoinSpillHelper;
use crate::join_row_table::RowTableSegment;
use crate::join_table_meta::EncodedRow;
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::atomic::AtomicBool;

/// 构造单列 Int 行。
fn row(value: i64) -> Row {
    vec![Value::Int(value)]
}

/// 构造右表为 build 的 Left Outer Join probe。
fn outer_probe() -> Box<dyn Probe> {
    let joiner = Joiner::new(JoinType::LeftOuter, false, row(-1), vec![], None, false, 32).unwrap();
    let context = HashJoinContext::new(vec![row(1)], vec![0], vec![0], joiner, true, true, 32);
    new_join_probe(context, 0, JoinType::LeftOuter, true, false).unwrap()
}

/// 构造最小可用的 build 侧 row table segment，供 spill helper 写入。
fn segment() -> RowTableSegment {
    RowTableSegment {
        rows: vec![EncodedRow {
            bytes: vec![1; 16],
            null_map: vec![0],
            key_offset: 0,
            key_length: 8,
            row_data_offset: 8,
            used: AtomicBool::new(false),
        }],
        hash_values: vec![7],
        valid_key_count: 1,
        ..Default::default()
    }
}

/// Spill 剩余 probe 行后恢复，匹配/未匹配语义与直接 probe 一致。
#[test]
fn outer_probe_spill_remaining_restores_unmatched_semantics() {
    let mut probe = outer_probe();
    probe.set_chunk_for_probe(vec![row(1), row(2)]).unwrap();
    let spilled = probe.spill_remaining_probe_chunks();
    assert_eq!(spilled, vec![vec![row(1), row(2)]]);
    probe
        .set_restored_chunk_for_probe(spilled[0].clone())
        .unwrap();
    assert_eq!(
        probe.probe().rows,
        vec![
            vec![Value::Int(1), Value::Int(1)],
            vec![Value::Int(2), Value::Int(-1)],
        ]
    );
}

/// Spill helper 同时落盘 build segment 与 probe chunk，恢复后两侧内容完整。
#[test]
fn outer_spill_writes_build_and_probe_rows_then_restores_both_sides() {
    let helper = HashJoinSpillHelper::new(2, 1, 2, 64).unwrap();
    helper.set_partition_spilled(&[1]).unwrap();
    helper.spill_build_segments(0, 1, &[segment()]).unwrap();
    let probe_chunk = vec![row(1), row(2)];
    helper.spill_probe_chunk(0, 1, &probe_chunk).unwrap();
    assert!(helper.build_spill_bytes() > 0);
    assert!(helper.probe_spill_bytes() > 0);
    helper.prepare_for_restoring(0).unwrap();
    let restored = helper.pop_restore_partition().unwrap();
    assert_eq!(restored.round, 1);
    assert_eq!(restored.build_side_chunks.len(), 1);
    assert_eq!(restored.build_side_chunks[0].len(), 1);
    let restored_build = &restored.build_side_chunks[0][0];
    assert_eq!(restored_build.hash_value, 7);
    assert!(restored_build.valid_join_key);
    assert_eq!(restored_build.row_bytes, vec![1; 16]);
    assert_eq!(restored.probe_side_chunks, vec![vec![row(1), row(2)]]);
}

/// reset 清空 spill 状态；超出 round 上限的恢复准备应失败。
#[test]
fn outer_spill_round_limit_and_reset_clear_state() {
    let helper = HashJoinSpillHelper::new(2, 1, 1, 64).unwrap();
    helper.set_partition_spilled(&[0, 1]).unwrap();
    assert!(helper.are_all_partitions_spilled());
    helper.reset();
    assert_eq!(helper.spilled_partition_count(), 0);
    assert!(helper.prepare_for_restoring(1).is_err());
    helper.close();
}

#[test]
/// Left outer probe returns a default inner row for every unmatched probe row.
fn outer_probe_reports_each_unmatched_probe_row_once() {
    let mut probe = outer_probe();
    probe.set_chunk_for_probe(vec![row(2), row(3)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        [
            vec![Value::Int(2), Value::Int(-1)],
            vec![Value::Int(3), Value::Int(-1)]
        ]
    );
}

#[test]
/// Capacity exhaustion resumes the same outer probe chunk on the following call.
fn outer_probe_resumes_after_capacity_boundary() {
    let joiner = Joiner::new(JoinType::LeftOuter, false, row(-1), vec![], None, false, 1).unwrap();
    let context = HashJoinContext::new(vec![row(1)], vec![0], vec![0], joiner, true, true, 1);
    let mut probe = new_join_probe(context, 0, JoinType::LeftOuter, true, false).unwrap();
    probe.set_chunk_for_probe(vec![row(1), row(2)]).unwrap();
    assert_eq!(probe.probe().rows, [vec![Value::Int(1), Value::Int(1)]]);
    assert_eq!(probe.probe().rows, [vec![Value::Int(2), Value::Int(-1)]]);
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
/// Right outer joins scan an outer build side after probing to emit only unused rows.
fn right_outer_probe_scans_unmatched_build_rows() {
    let joiner = Joiner::new(JoinType::RightOuter, true, row(-1), vec![], None, false, 8).unwrap();
    let context = HashJoinContext::new(
        vec![row(1), row(2)],
        vec![0],
        vec![0],
        joiner,
        true,
        true,
        8,
    );
    let mut probe = new_join_probe(context, 0, JoinType::RightOuter, true, false).unwrap();
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();
    assert_eq!(probe.probe().rows, [vec![Value::Int(1), Value::Int(1)]]);
    assert!(probe.need_scan_row_table());
    probe.init_for_scan_row_table();
    assert_eq!(
        probe.scan_row_table().rows,
        [vec![Value::Int(-1), Value::Int(2)]]
    );
}

#[test]
/// A rejected match still receives the outer-side default row.
fn outer_probe_condition_rejection_preserves_outer_row() {
    let condition: Predicate = std::sync::Arc::new(|_| Ok(Some(false)));
    let joiner = Joiner::new(
        JoinType::LeftOuter,
        false,
        row(-1),
        vec![condition],
        None,
        false,
        8,
    )
    .unwrap();
    let context = HashJoinContext::new(vec![row(1)], vec![0], vec![0], joiner, true, true, 8);
    let mut probe = new_join_probe(context, 0, JoinType::LeftOuter, true, false).unwrap();
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();
    assert_eq!(probe.probe().rows, [vec![Value::Int(1), Value::Int(-1)]]);
}

#[test]
/// Spill helper accepts an empty probe chunk without creating a bogus restore partition.
fn outer_spill_empty_probe_chunk_round_trips() {
    let helper = HashJoinSpillHelper::new(1, 1, 2, 64).unwrap();
    helper.set_partition_spilled(&[0]).unwrap();
    let empty: crate::row_table_builder::Chunk = Vec::new();
    helper.spill_probe_chunk(0, 0, &empty).unwrap();
    helper.prepare_for_restoring(0).unwrap();
    assert!(helper.pop_restore_partition().is_none());
}

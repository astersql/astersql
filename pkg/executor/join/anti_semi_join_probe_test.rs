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

// Anti Semi Join Probe 单元测试。
//
// 活跃测试覆盖：重复 build key 只输出未匹配行、NULL/恢复 Chunk/reset 路径、
// 左侧 build 后扫描未命中 build 行，以及 spill 仅保留未处理 probe 行。


use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 构造单列 Row，便于测试里快速拼装键值。
fn row(value: Value) -> Row {
    vec![value]
}

/// 创建 AntiSemi Join Probe：build 键与 probe 键均为第 0 列。
fn anti_probe(build: Vec<Row>, right_build: bool, max_chunk: usize) -> Box<dyn Probe> {
    let joiner = Joiner::new(
        JoinType::AntiSemi,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        max_chunk,
    )
    .unwrap();
    let context = HashJoinContext::new(
        build,
        vec![0],
        vec![0],
        joiner,
        right_build,
        true,
        max_chunk,
    );
    new_join_probe(context, 0, JoinType::AntiSemi, right_build, false).unwrap()
}

#[test]
/// 右侧 build 且存在重复 key 时，仅输出在 build 侧找不到匹配的 probe 行。
fn anti_semi_probe_returns_only_unmatched_rows_with_duplicate_build_keys() {
    let mut probe = anti_probe(
        vec![row(Value::Int(1)), row(Value::Int(1)), row(Value::Int(3))],
        true,
        32,
    );
    probe
        .set_chunk_for_probe(vec![
            row(Value::Int(1)),
            row(Value::Int(2)),
            row(Value::Int(3)),
        ])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(Value::Int(2))]);
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
/// 覆盖 restored Chunk、NULL probe 行输出，以及 reset 后状态清空。
fn anti_semi_probe_preserves_null_restored_and_reset_paths() {
    let mut probe = anti_probe(vec![row(Value::Int(1))], true, 1);
    probe
        .set_restored_chunk_for_probe(vec![row(Value::Null), row(Value::Int(2))])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(Value::Null)]);
    assert_eq!(probe.probe().rows, [row(Value::Int(2))]);
    probe.reset_probe();
    assert!(probe.is_current_chunk_probe_done());
}

#[test]
/// 左侧 build：probe 阶段不输出，随后扫描 build 表得到未命中行。
fn anti_semi_left_build_scans_unmatched_build_rows_after_probe() {
    let mut probe = anti_probe(
        vec![row(Value::Int(1)), row(Value::Int(2)), row(Value::Int(3))],
        false,
        8,
    );
    probe
        .set_chunk_for_probe(vec![row(Value::Int(1)), row(Value::Int(3))])
        .unwrap();
    assert!(probe.probe().rows.is_empty());
    probe.init_for_scan_row_table();
    assert_eq!(probe.scan_row_table().rows, [row(Value::Int(2))]);
    assert!(probe.is_scan_row_table_done());
}

#[test]
/// Go parity: left-build anti semi treats a NULL other-condition result as used.
fn anti_semi_left_build_null_condition_suppresses_build_row() {
    let condition: Predicate = Arc::new(|_| Ok(None));
    let joiner = Joiner::new(
        JoinType::AntiSemi,
        false,
        Vec::new(),
        vec![condition],
        None,
        false,
        8,
    )
    .unwrap();
    let context = HashJoinContext::new(
        vec![row(Value::Int(1))],
        vec![0],
        vec![0],
        joiner,
        false,
        true,
        8,
    );
    let mut probe = new_join_probe(context, 0, JoinType::AntiSemi, false, false).unwrap();
    probe.set_chunk_for_probe(vec![row(Value::Int(1))]).unwrap();

    assert!(probe.probe().rows.is_empty());
    probe.init_for_scan_row_table();
    assert!(probe.scan_row_table().rows.is_empty());
}

#[test]
/// spill 只带走尚未处理的 probe 行，已输出的匹配结果不重复刷盘。
fn anti_semi_spills_only_unprocessed_probe_rows() {
    let mut probe = anti_probe(vec![row(Value::Int(1))], true, 1);
    probe
        .set_chunk_for_probe(vec![
            row(Value::Int(1)),
            row(Value::Int(2)),
            row(Value::Int(3)),
        ])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(Value::Int(2))]);
    assert_eq!(
        probe.spill_remaining_probe_chunks(),
        [vec![row(Value::Int(3))]]
    );
}

#[test]
/// NULL build keys do not match either NULL or non-NULL probe keys in ordinary anti join.
fn anti_semi_null_build_key_is_not_equal() {
    let mut probe = anti_probe(vec![row(Value::Null), row(Value::Int(1))], true, 8);
    probe
        .set_chunk_for_probe(vec![
            row(Value::Null),
            row(Value::Int(1)),
            row(Value::Int(2)),
        ])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(Value::Null), row(Value::Int(2))]);
}

#[test]
/// A capacity-limited anti probe resumes at the next probe row without duplicating output.
fn anti_semi_probe_resumes_after_required_rows() {
    let mut probe = anti_probe(vec![row(Value::Int(1))], true, 1);
    probe
        .set_chunk_for_probe(vec![
            row(Value::Int(1)),
            row(Value::Int(2)),
            row(Value::Int(3)),
        ])
        .unwrap();
    assert_eq!(probe.probe().rows, [row(Value::Int(2))]);
    assert_eq!(probe.probe().rows, [row(Value::Int(3))]);
    assert!(probe.is_current_chunk_probe_done());
}

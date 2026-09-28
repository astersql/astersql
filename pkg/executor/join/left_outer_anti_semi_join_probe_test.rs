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

// 左外反半连接（Left Outer Anti Semi Join）探测路径的单元测试。
//
// 左外反半连接对每条探测行输出探测列加上布尔标记：命中构建侧时为 false，
// 未命中时为 true（表示“不存在匹配”）；other condition 只改变标记，不丢行。
// Spill（溢写）场景下未处理完的探测块应保留。


use crate::base_join_probe::{HashJoinContext, Probe, new_join_probe};
use crate::joiner::{JoinType, Joiner, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 构造单列整型探测/构建行。
fn row(value: i64) -> Row {
    vec![Value::Int(value)]
}

/// 以构建侧行为 [1,3] 构造 AntiLeftOuterSemi 探测器；`max_chunk` 限制单次输出行数。
fn anti_outer_probe(condition: Vec<Predicate>, max_chunk: usize) -> Box<dyn Probe> {
    let joiner = Joiner::new(
        JoinType::AntiLeftOuterSemi,
        false,
        Vec::new(),
        condition,
        None,
        false,
        max_chunk,
    )
    .unwrap();
    let context = HashJoinContext::new(
        vec![row(1), row(3)],
        vec![0],
        vec![0],
        joiner,
        true,
        true,
        max_chunk,
    );
    new_join_probe(context, 0, JoinType::AntiLeftOuterSemi, true, false).unwrap()
}

/// 命中输出 false 标记，未命中输出 true 标记，探测列始终保留。
#[test]
fn left_outer_anti_semi_emits_false_for_match_and_true_for_miss() {
    let mut probe = anti_outer_probe(Vec::new(), 8);
    probe
        .set_chunk_for_probe(vec![row(1), row(2), row(3)])
        .unwrap();
    assert_eq!(
        probe.probe().rows,
        [
            vec![Value::Int(1), Value::Bool(false)],
            vec![Value::Int(2), Value::Bool(true)],
            vec![Value::Int(3), Value::Bool(false)],
        ]
    );
}

/// other condition 不成立时标记变 true，但不会像 inner join 那样丢弃探测行。
#[test]
fn left_outer_anti_semi_other_condition_changes_marker_without_dropping_rows() {
    let condition: Predicate = Arc::new(|joined| match (&joined[0], &joined[1]) {
        (Value::Int(build), Value::Int(probe)) => Ok(Some(build + probe > 10)),
        _ => Ok(None),
    });
    let mut probe = anti_outer_probe(vec![condition], 8);
    probe.set_chunk_for_probe(vec![row(1), row(2)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        [
            vec![Value::Int(1), Value::Bool(true)],
            vec![Value::Int(2), Value::Bool(true)],
        ]
    );
}

/// Go 的期望结果生成器会保留 other condition 的 UNKNOWN：没有 true 匹配时输出 NULL。
#[test]
fn left_outer_anti_semi_other_condition_null_keeps_unknown_marker() {
    let condition: Predicate = Arc::new(|_| Ok(None));
    let mut probe = anti_outer_probe(vec![condition], 8);
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();
    assert_eq!(probe.probe().rows, [vec![Value::Int(1), Value::Null]]);
}

/// other condition 的求值错误必须像 Go 的 EvalBool 错误一样向 worker result 传播。
#[test]
fn left_outer_anti_semi_other_condition_error_is_reported() {
    let condition: Predicate = Arc::new(|_| Err("condition failed".into()));
    let mut probe = anti_outer_probe(vec![condition], 8);
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();
    let result = probe.probe();
    assert!(result.rows.is_empty());
    assert_eq!(result.error.as_deref(), Some("condition failed"));
}

/// Go 的 all-join-keys 用例包含复合键；全部键相等才算命中。
#[test]
fn left_outer_anti_semi_requires_all_composite_join_keys() {
    let joiner = Joiner::new(
        JoinType::AntiLeftOuterSemi,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    let context = HashJoinContext::new(
        vec![vec![Value::Int(1), Value::Text("a".into())]],
        vec![0, 1],
        vec![0, 1],
        joiner,
        true,
        true,
        8,
    );
    let mut probe = new_join_probe(context, 0, JoinType::AntiLeftOuterSemi, true, false).unwrap();
    probe
        .set_chunk_for_probe(vec![
            vec![Value::Int(1), Value::Text("a".into())],
            vec![Value::Int(1), Value::Text("b".into())],
        ])
        .unwrap();
    assert_eq!(
        probe.probe().rows,
        [
            vec![Value::Int(1), Value::Text("a".into()), Value::Bool(false),],
            vec![Value::Int(1), Value::Text("b".into()), Value::Bool(true),],
        ]
    );
}

/// max_chunk=1 时只吐出一行，剩余探测块经 spill 接口完整返回。
#[test]
fn left_outer_anti_semi_capacity_and_spill_keep_unprocessed_rows() {
    let mut probe = anti_outer_probe(Vec::new(), 1);
    probe
        .set_chunk_for_probe(vec![row(1), row(2), row(4)])
        .unwrap();
    assert_eq!(probe.probe().rows.len(), 1);
    assert_eq!(probe.spill_remaining_probe_chunks(), [vec![row(2), row(4)]]);
}

#[test]
/// Restoring a left outer anti probe preserves the anti marker for a null-key row.
fn left_outer_anti_semi_restored_null_row_keeps_unknown_marker() {
    let mut probe = anti_outer_probe(vec![], 8);
    probe
        .set_restored_chunk_for_probe(vec![vec![Value::Null]])
        .unwrap();
    assert_eq!(probe.probe().rows, [vec![Value::Null, Value::Bool(true)]]);
}

#[test]
/// A duplicate build key still produces exactly one anti marker per probe row.
fn left_outer_anti_semi_duplicate_build_keys_do_not_duplicate_rows() {
    let mut probe = anti_outer_probe(vec![], 8);
    probe.set_chunk_for_probe(vec![row(7), row(7)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        [
            vec![Value::Int(7), Value::Bool(true)],
            vec![Value::Int(7), Value::Bool(true)],
        ]
    );
}

#[test]
/// Left outer anti probing does not expose a build-side scan path in this probe factory.
fn left_outer_anti_semi_probe_uses_right_build_side() {
    let joiner = Joiner::new(
        JoinType::AntiLeftOuterSemi,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    let context = HashJoinContext::new(
        vec![row(1), row(2)],
        vec![0],
        vec![0],
        joiner,
        false,
        true,
        8,
    );
    let mut probe = new_join_probe(context, 0, JoinType::AntiLeftOuterSemi, true, false).unwrap();
    probe.set_chunk_for_probe(vec![row(1)]).unwrap();
    assert_eq!(
        probe.probe().rows,
        [vec![Value::Int(1), Value::Bool(false)]]
    );
    assert!(!probe.need_scan_row_table());
}

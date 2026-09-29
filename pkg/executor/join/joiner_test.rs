// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Joiner 匹配、投影与三值逻辑标记的单元测试。
//
// 覆盖 Inner/LeftOuter 投影列序、条件求值的 Matched/Unmatched/HasNull，
// 以及 Semi / AntiLeftOuterSemi（含 null-aware）与非法构造参数。

use crate::joiner::{JoinType, Joiner, NaajType, OuterRowStatus, Predicate, Row};
use crate::row_table_builder::Value;
use std::sync::Arc;

/// 由整型切片构造测试行。
fn row(values: &[i64]) -> Row {
    values.iter().copied().map(Value::Int).collect()
}

/// Inner 投影保留指定列；LeftOuter 未匹配时拼默认内表值。
#[test]
fn joiner_inner_outer_and_projection_paths_preserve_column_order() {
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        Vec::new(),
        Some([vec![0], vec![1]]),
        false,
        32,
    )
    .unwrap();
    let mut output = Vec::new();
    let result = joiner
        .try_to_match_inners(
            &row(&[7, 8]),
            &[row(&[1, 2])],
            &mut output,
            NaajType::Unknown,
        )
        .unwrap();
    assert!(result.matched);
    assert_eq!(output, [vec![Value::Int(7), Value::Int(2)]]);

    let outer = Joiner::new(
        JoinType::LeftOuter,
        false,
        vec![Value::Int(-1)],
        Vec::new(),
        None,
        false,
        32,
    )
    .unwrap();
    let mut missed = Vec::new();
    outer.on_miss_match(false, &row(&[9]), &mut missed);
    assert_eq!(missed, [vec![Value::Int(9), Value::Int(-1)]]);
}

/// other condition 对正数/负数/NULL 分别对应 Matched、Unmatched、HasNull。
#[test]
fn joiner_conditions_report_false_null_and_match_statuses() {
    let condition: Predicate = Arc::new(|joined| match joined.first() {
        Some(Value::Null) => Ok(None),
        Some(Value::Int(value)) => Ok(Some(*value > 0)),
        _ => Ok(Some(false)),
    });
    let joiner = Joiner::new(
        JoinType::Inner,
        false,
        Vec::new(),
        vec![condition],
        None,
        false,
        8,
    )
    .unwrap();
    let mut output = Vec::new();
    let statuses = joiner
        .try_to_match_outers(
            &[row(&[10]), row(&[-1]), vec![Value::Null]],
            &row(&[1]),
            &mut output,
        )
        .unwrap();
    assert_eq!(
        statuses,
        [
            OuterRowStatus::Matched,
            OuterRowStatus::Unmatched,
            OuterRowStatus::HasNull,
        ]
    );
    assert_eq!(output.len(), 1);
}

/// Semi 无条件早停；AntiLeftOuterSemi 未匹配写 true；非法 null-aware/chunk size 报错。
#[test]
fn semi_anti_and_null_aware_markers_match_three_valued_logic() {
    let semi = Joiner::new(
        JoinType::Semi,
        false,
        Vec::new(),
        Vec::new(),
        None,
        false,
        8,
    )
    .unwrap();
    assert!(semi.is_semi_join_without_condition());
    let mut output = Vec::new();
    semi.try_to_match_inners(
        &row(&[1]),
        &[row(&[1]), row(&[1])],
        &mut output,
        NaajType::Unknown,
    )
    .unwrap();
    assert_eq!(output, [row(&[1])]);

    let anti = Joiner::new(
        JoinType::AntiLeftOuterSemi,
        false,
        Vec::new(),
        Vec::new(),
        None,
        true,
        8,
    )
    .unwrap();
    let mut missed = Vec::new();
    anti.on_miss_match(false, &row(&[3]), &mut missed);
    assert_eq!(missed, [vec![Value::Int(3), Value::Bool(true)]]);
    assert!(
        Joiner::new(
            JoinType::Inner,
            false,
            Vec::new(),
            Vec::new(),
            None,
            true,
            8
        )
        .is_err()
    );
    assert!(
        Joiner::new(
            JoinType::Inner,
            false,
            Vec::new(),
            Vec::new(),
            None,
            false,
            0
        )
        .is_err()
    );
}

/// Go SemiJoin 忽略条件 NULL；普通 Inner/Outer Join 的 isNull 返回值也始终为 false。
#[test]
fn semi_and_regular_join_do_not_expose_condition_nullness() {
    let null_condition: Predicate = Arc::new(|_| Ok(None));
    for join_type in [
        JoinType::Semi,
        JoinType::Inner,
        JoinType::LeftOuter,
        JoinType::RightOuter,
    ] {
        let joiner = Joiner::new(
            join_type,
            false,
            Vec::new(),
            vec![null_condition.clone()],
            None,
            false,
            8,
        )
        .unwrap();
        let result = joiner
            .try_to_match_inners(&row(&[1]), &[row(&[2])], &mut Vec::new(), NaajType::Unknown)
            .unwrap();
        assert!(!result.matched);
        assert!(!result.has_null);
    }

    let semi = Joiner::new(
        JoinType::Semi,
        false,
        Vec::new(),
        vec![null_condition],
        None,
        false,
        8,
    )
    .unwrap();
    assert_eq!(
        semi.try_to_match_outers(&[row(&[1])], &row(&[2]), &mut Vec::new())
            .unwrap(),
        [OuterRowStatus::Unmatched]
    );
}

/// Go null-aware Anti 的 other condition 把 false/NULL 都视为无效 inner，不传播 NULL。
#[test]
fn null_aware_anti_join_ignores_other_condition_nullness() {
    let null_condition: Predicate = Arc::new(|_| Ok(None));
    for join_type in [JoinType::AntiSemi, JoinType::AntiLeftOuterSemi] {
        let joiner = Joiner::new(
            join_type,
            false,
            Vec::new(),
            vec![null_condition.clone()],
            None,
            true,
            8,
        )
        .unwrap();
        let result = joiner
            .try_to_match_inners(
                &row(&[1]),
                &[row(&[2])],
                &mut Vec::new(),
                NaajType::LeftNotNullRightNotNull,
            )
            .unwrap();
        assert!(!result.matched);
        assert!(!result.has_null);
    }
}

/// CNF 中 false 支配此前的 NULL，与 Go expression.EvalBool 一致。
#[test]
fn false_condition_overrides_earlier_null() {
    let joiner = Joiner::new(
        JoinType::AntiSemi,
        false,
        Vec::new(),
        vec![Arc::new(|_| Ok(None)), Arc::new(|_| Ok(Some(false)))],
        None,
        false,
        8,
    )
    .unwrap();
    let result = joiner
        .try_to_match_inners(&row(&[1]), &[row(&[2])], &mut Vec::new(), NaajType::Unknown)
        .unwrap();
    assert!(!result.has_null);
}

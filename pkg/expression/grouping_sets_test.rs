// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// grouping_sets 模块单元测试。
//
// 验证 merge/target_one 单布局规则、ROLLUP 可空性与去重还原，以及
// distinct_size 在阈值上下是否分配 grouping id。

use crate::grouping_sets_kernel::*;
use crate::*;

/// 构造带 NotNull 的整型测试列。
fn column(unique_id: i64) -> Column {
    let mut ret_type = *types::NewFieldType(mysql::TypeLonglong);
    ret_type.SetFlag(mysql::NotNullFlag);
    Column {
        RetType: Some(ret_type),
        UniqueID: unique_id,
        Index: unique_id as isize,
        ..Default::default()
    }
}

/// 将列包装为 ExprBox。
fn expression(unique_id: i64) -> ExprBox {
    Box::new(column(unique_id))
}

/// 由列 ID 列表构造 GroupingExprs。
fn grouping_exprs(ids: &[i64]) -> GroupingExprs {
    GroupingExprs(ids.iter().copied().map(expression).collect())
}

/// 前缀合并后列并集正确；跨 layout 聚合参数应返回 -1。
#[test]
fn merge_and_target_one_follow_prefix_and_single_layout_rules() {
    let merged = GroupingSets(vec![
        GroupingSet(vec![grouping_exprs(&[1, 2, 3])]),
        GroupingSet(vec![grouping_exprs(&[1, 2])]),
        GroupingSet(vec![grouping_exprs(&[1])]),
        GroupingSet(vec![grouping_exprs(&[4])]),
    ])
    .merge();
    assert_eq!(merged.0.len(), 2);
    assert_eq!(merged.0[0].all_col_ids(), GroupingIds::from([1, 2, 3]));
    assert_eq!(merged.0[1].all_col_ids(), GroupingIds::from([4]));

    assert_eq!(merged.target_one(&[expression(1)]), 0);
    assert_eq!(merged.target_one(&[expression(4)]), 1);
    assert_eq!(merged.target_one(&[expression(1), expression(4)]), -1);
    assert_eq!(GroupingSets(Vec::new()).target_one(&[expression(1)]), -1);
}

/// ROLLUP 仅清除分组列 NotNull；去重下标可完整还原原始序列。
#[test]
fn rollup_adjusts_only_nullable_grouping_columns_and_restores_duplicates() {
    let rollup = rollup_grouping_sets(&[expression(1), expression(2)]);
    assert_eq!(
        rollup
            .0
            .iter()
            .map(|set| set.0[0].0.len())
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );

    let mut schema = NewSchema(vec![column(1), column(2), column(9)]);
    adjust_nullability_from_grouping_sets(&rollup, &mut schema);
    assert_eq!(
        schema.Columns[0].RetType.as_ref().unwrap().GetFlag() & mysql::NotNullFlag,
        0
    );
    assert_eq!(
        schema.Columns[1].RetType.as_ref().unwrap().GetFlag() & mysql::NotNullFlag,
        0
    );
    assert_ne!(
        schema.Columns[2].RetType.as_ref().unwrap().GetFlag() & mysql::NotNullFlag,
        0
    );

    let (distinct, positions) =
        deduplicate_gby_expression(&[expression(1), expression(1), expression(2)]);
    assert_eq!(distinct.len(), 2);
    assert_eq!(positions, vec![0, 0, 1]);
    let restored = restore_gby_expression(&[column(1), column(2)], &positions);
    assert_eq!(
        restored
            .iter()
            .map(|item| item.as_column().unwrap().UniqueID)
            .collect::<Vec<_>>(),
        vec![1, 1, 2]
    );
}

/// 超过阈值分配 gid；不超过阈值时 gid 映射均为 None。
#[test]
fn distinct_size_emits_grouping_ids_only_above_threshold() {
    let grouping_sets = GroupingSets(vec![
        GroupingSet(vec![grouping_exprs(&[1])]),
        GroupingSet(vec![grouping_exprs(&[1])]),
        GroupingSet(vec![grouping_exprs(&[2])]),
    ]);

    let (size, gids, id_to_gids) = grouping_sets.distinct_size_with_threshold(1);
    assert_eq!(size, 2);
    assert_eq!(gids.unwrap(), vec![0, 0, 1]);
    let id_to_gids = id_to_gids.unwrap();
    assert_eq!(
        id_to_gids.get(&1).unwrap(),
        &std::collections::BTreeSet::from([0])
    );
    assert_eq!(
        id_to_gids.get(&2).unwrap(),
        &std::collections::BTreeSet::from([1])
    );

    let (size, gids, id_to_gids) = grouping_sets.distinct_size_with_threshold(2);
    assert_eq!(size, 2);
    assert!(gids.is_none());
    assert!(id_to_gids.is_none());
}

/// Go uses a mandatory `*Column` assertion in `GroupingSet.ExtractCols`.
#[test]
#[should_panic(expected = "grouping expression must be a column")]
fn extract_cols_rejects_non_column_grouping_expressions() {
    let grouping_set = GroupingSet(vec![GroupingExprs(vec![Box::new(Constant::null(
        mysql::TypeLonglong,
    ))])]);
    let _ = grouping_set.extract_cols();
}

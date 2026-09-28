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

// grouping_sets 运行时内核迁移回归测试。
//
// 覆盖 merge/target_one 前缀规则、ROLLUP 可空性调整、GROUP BY 去重/还原，
// 以及 distinct_size 超过阈值后分配 grouping id 的行为。

use crate::grouping_sets_kernel::*;
use crate::*;

/// 构造带 NotNull 标志的简易整型列，UniqueID/Index 取同一值便于断言。
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

/// 由列 ID 列表构造一组 GroupingExprs。
fn grouping_exprs(ids: &[i64]) -> GroupingExprs {
    GroupingExprs(ids.iter().copied().map(expression).collect())
}

/// 构造 Go 测试中的复合聚合参数（`lhs + rhs`）。
fn plus(ctx: &dyn BuildContext, lhs: ExprBox, rhs: ExprBox) -> ExprBox {
    NewFunctionInternal(
        ctx,
        ast::Plus,
        *types::NewFieldType(mysql::TypeLonglong),
        vec![lhs, rhs],
    )
    .expect("plus expression must be constructible")
}

/// 前缀可合并的 sets 应并成一条链；target_one 按“不会 NULL 填补引用列”选布局。
#[test]
fn merge_and_target_one_match_go_prefix_rules() {
    let merged = GroupingSets(vec![
        GroupingSet(vec![grouping_exprs(&[1, 2, 3])]),
        GroupingSet(vec![grouping_exprs(&[1, 2])]),
        GroupingSet(vec![grouping_exprs(&[1])]),
        GroupingSet(vec![grouping_exprs(&[4])]),
    ])
    .merge();

    assert_eq!(merged.0.len(), 2);
    assert_eq!(
        merged.0[0]
            .0
            .iter()
            .map(|item| item.0.len())
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(merged.0[1].all_col_ids(), GroupingIds::from([4]));

    let layouts = GroupingSets(vec![
        GroupingSet(vec![grouping_exprs(&[1, 2])]),
        GroupingSet(vec![grouping_exprs(&[3])]),
    ]);
    assert_eq!(layouts.target_one(&[expression(9)]), 0);
    assert_eq!(layouts.target_one(&[expression(1)]), 0);
    assert_eq!(layouts.target_one(&[expression(3)]), 1);
    assert_eq!(layouts.target_one(&[expression(1), expression(3)]), -1);

    // Go TestGroupSetsTargetOneCompoundArgs：复合表达式必须递归提取 Column。
    let ctx = exprstatic::NewExprContext(Vec::new());
    assert_eq!(
        layouts.target_one(&[plus(&ctx, expression(9), expression(3))]),
        1
    );
    assert_eq!(
        layouts.target_one(&[plus(&ctx, expression(9), expression(1))]),
        0
    );
    assert_eq!(
        layouts.target_one(&[plus(
            &ctx,
            expression(9),
            plus(&ctx, expression(1), expression(3)),
        )]),
        -1
    );
}

/// ROLLUP 展开、Schema NotNull 清除，以及去重/还原映射与 Go 一致。
#[test]
fn rollup_nullability_and_deduplication_match_go() {
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

    // 去掉空 grouping set 后，列 1 出现在每个 layout 中，仍应保持 NotNull。
    let mut partial_schema = NewSchema(vec![column(1), column(2), column(9)]);
    let partial_rollup = GroupingSets(rollup.0[1..].to_vec());
    adjust_nullability_from_grouping_sets(&partial_rollup, &mut partial_schema);
    assert_ne!(
        partial_schema.Columns[0]
            .RetType
            .as_ref()
            .unwrap()
            .GetFlag()
            & mysql::NotNullFlag,
        0
    );
    assert_eq!(
        partial_schema.Columns[1]
            .RetType
            .as_ref()
            .unwrap()
            .GetFlag()
            & mysql::NotNullFlag,
        0
    );
    assert_ne!(
        partial_schema.Columns[2]
            .RetType
            .as_ref()
            .unwrap()
            .GetFlag()
            & mysql::NotNullFlag,
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

/// 阈值过小时应分配 gid 与列→gid 反查表。
#[test]
fn distinct_size_allocates_go_grouping_ids_after_threshold() {
    // Go TestDistinctGroupingSets case 3：ROLLUP(1, 2, 2, 3)。
    let grouping_sets =
        rollup_grouping_sets(&[expression(1), expression(2), expression(2), expression(3)]);

    let (size, gids, id_to_gids) = grouping_sets.distinct_size_with_threshold(4);
    assert_eq!(size, 4);
    assert!(gids.is_none());
    assert!(id_to_gids.is_none());

    let (size, gids, id_to_gids) = grouping_sets.distinct_size_with_threshold(3);
    assert_eq!(size, 4);
    assert_eq!(gids.unwrap(), vec![0, 1, 2, 2, 3]);
    let id_to_gids = id_to_gids.unwrap();
    assert_eq!(
        id_to_gids.get(&1).unwrap(),
        &std::collections::BTreeSet::from([1, 2, 3])
    );
    assert_eq!(
        id_to_gids.get(&2).unwrap(),
        &std::collections::BTreeSet::from([2, 3])
    );
    assert_eq!(
        id_to_gids.get(&3).unwrap(),
        &std::collections::BTreeSet::from([3])
    );
}

// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

// 逻辑计划推导与执行边界回归测试。
// 覆盖空 CTE（公共表表达式）分支 Join 在大 LIMIT 下的统计信息推导。

use logicalop::*;

/// 构造指定 ID 的整型列，便于拼装最小逻辑计划树。
fn column(id: i64) -> Column {
    Column::new(
        *expression::types::NewFieldType(mysql::r#type::TypeLonglong),
        id,
        id + 100,
        0,
    )
}

/// Regression for #58743 at the logical-operator boundary: two empty grouped
/// CTE branches joined together must remain empty and derive statistics without
/// overflowing the very large LIMIT from the original SQL.
/// 回归 #58743：两路空分组 CTE 经 Join 后行数仍为 0，且不被超大 LIMIT 放大溢出。
#[test]
fn TestIssue58743() {
    let left_column = column(1);
    let right_column = column(2);
    // 左支：空 Dual（0 行）+ 超大 Limit。
    let mut left = LogicalTableDual {
        RowCount: 0,
        ..Default::default()
    };
    left.SetSchema(expression::NewSchema(vec![left_column.clone()]));
    let mut right = LogicalTableDual {
        RowCount: 0,
        ..Default::default()
    };
    right.SetSchema(expression::NewSchema(vec![right_column.clone()]));
    let mut left_limit = LogicalLimit {
        Count: 772_780_933,
        ..Default::default()
    };
    left_limit.SetSchema(left.Schema().Clone());
    left_limit.SetChildren(vec![Box::new(left)]);
    let mut join = LogicalJoin::default();
    join.SetSchema(expression::NewSchema(vec![left_column, right_column]));
    join.SetChildren(vec![Box::new(left_limit), Box::new(right)]);
    // DeriveStats：自底向上推导基数（行数）等统计信息。
    let (stats, changed) = join
        .DeriveStats(true)
        .expect("empty CTE join statistics must derive");
    assert!(changed);
    assert_eq!(stats.RowCount, 0.0);
    assert_eq!(join.EqualCondOutCnt, 0.0);
}

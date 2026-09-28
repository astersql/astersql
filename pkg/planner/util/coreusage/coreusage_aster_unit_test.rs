// Copyright 2026 AsterSQL.

// coreusage 迁移单元测试：相关列前序收集、Schema 去重与聚合 CAST 模式跳过。

use std::sync::Arc;
use std::{cell::RefCell, rc::Rc};

use aggregation::{CompleteMode, FinalMode, Partial1Mode, Partial2Mode};
use expression::{Column, CorrelatedColumn, NewCorrelatedDatum};

use crate::{ExtractCorColumnsBySchema, ExtractCorrelatedCols4LogicalPlan, WrapCastForAggFuncs};

/// 构造简易长整型列桩。
fn column(unique_id: i64, index: isize) -> Column {
    Column::new(
        *types::field::NewFieldType(expression::mysql::TypeLonglong),
        unique_id,
        unique_id,
        index,
    )
}

/// 构造带整型 datum 的相关列。
fn correlated(unique_id: i64) -> CorrelatedColumn {
    CorrelatedColumn {
        column: column(unique_id, -1),
        data: Some(NewCorrelatedDatum(types::datum::NewIntDatum(unique_id))),
    }
}

/// 验证递归收集保持 Go 前序（根 → 左子树 → 右子树）顺序。
#[test]
fn recursive_collection_keeps_go_preorder() {
    struct Node {
        columns: Vec<CorrelatedColumn>,
        children: Vec<Node>,
    }

    let tree = Node {
        columns: vec![correlated(1)],
        children: vec![
            Node {
                columns: vec![correlated(2)],
                children: vec![Node {
                    columns: vec![correlated(3)],
                    children: Vec::new(),
                }],
            },
            Node {
                columns: vec![correlated(4)],
                children: Vec::new(),
            },
        ],
    };

    let result = super::correlated_misc::extract_correlated_cols_for_test(
        &tree,
        |node| node.columns.iter().map(CorrelatedColumn::Clone).collect(),
        |node| node.children.iter().collect(),
    );
    assert_eq!(
        result
            .iter()
            .map(|column| column.column.UniqueID)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
}

/// CTE 自身必须递归提取种子与递归计划中的相关列，不能只调用基类默认实现。
#[test]
fn cte_collection_includes_seed_and_recursive_parts() {
    let seed = logicalop::LogicalProjection {
        Exprs: vec![Box::new(correlated(41))],
        ..logicalop::LogicalProjection::default()
    };
    let recursive = logicalop::LogicalProjection {
        Exprs: vec![Box::new(correlated(42))],
        ..logicalop::LogicalProjection::default()
    };
    let cte = logicalop::LogicalCTE {
        Cte: Rc::new(RefCell::new(logicalop::CTEClass {
            SeedPartLogicalPlan: Some(Box::new(seed)),
            RecursivePartLogicalPlan: Some(Box::new(recursive)),
            ..logicalop::CTEClass::default()
        })),
        ..logicalop::LogicalCTE::default()
    };

    let result = ExtractCorrelatedCols4LogicalPlan(&cte);
    assert_eq!(
        result
            .iter()
            .map(|column| column.column.UniqueID)
            .collect::<Vec<_>>(),
        vec![41, 42]
    );
}

/// 验证 Schema 去重共享 datum，以及物理模式下解析列下标。
#[test]
fn schema_deduplication_shares_datum_and_resolves_physical_index() {
    let schema = expression::NewSchema(vec![column(20, -1), column(10, -1)]);
    let outside_slot = NewCorrelatedDatum(types::datum::NewIntDatum(99));
    let mut input = vec![
        correlated(10),
        correlated(20),
        correlated(10),
        CorrelatedColumn {
            column: column(99, -1),
            data: Some(outside_slot.clone()),
        },
    ];

    // 逻辑模式：按 Schema 顺序去重，Index 保持 -1。
    let logical = ExtractCorColumnsBySchema(&mut input, &schema, false);
    assert_eq!(
        logical
            .iter()
            .map(|column| column.column.UniqueID)
            .collect::<Vec<_>>(),
        vec![20, 10]
    );
    assert_eq!(logical[0].column.Index, -1);
    assert!(Arc::ptr_eq(
        logical[1].data.as_ref().unwrap(),
        input[0].data.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(
        input[0].data.as_ref().unwrap(),
        input[2].data.as_ref().unwrap()
    ));
    assert!(Arc::ptr_eq(input[3].data.as_ref().unwrap(), &outside_slot));

    // 写入共享槽位后，同列其他出现应可见新值。
    *logical[1]
        .data
        .as_ref()
        .unwrap()
        .write()
        .expect("write correlated datum") = types::datum::NewIntDatum(42);
    assert_eq!(
        input[0]
            .data
            .as_ref()
            .unwrap()
            .read()
            .expect("read correlated datum")
            .GetInt64(),
        42
    );

    // 物理模式：写入 Schema 中的物理列下标。
    let physical = ExtractCorColumnsBySchema(&mut input, &schema, true);
    assert_eq!(physical[0].column.Index, 0);
    assert_eq!(physical[1].column.Index, 1);
}

/// 构造指定 Mode 的 SUM 聚合描述符。
fn aggregate(mode: aggregation::AggFunctionMode) -> aggregation::AggFuncDesc {
    let context = exprstatic::NewExprContext(Vec::new());
    let argument: expression::ExprBox = Box::new(column(7, 0));
    let mut descriptor =
        aggregation::NewAggFuncDesc(&context, expression::ast::AggFuncSum, vec![argument], false)
            .expect("build aggregate descriptor");
    descriptor.Mode = mode;
    descriptor
}

/// Final/Partial2 模式跳过 CAST；Complete/Partial1 应包装参数。
#[test]
fn aggregate_cast_skips_only_final_and_partial2_modes() {
    let context = exprstatic::NewExprContext(Vec::new());
    let mut descriptors = vec![
        aggregate(CompleteMode),
        aggregate(FinalMode),
        aggregate(Partial1Mode),
        aggregate(Partial2Mode),
    ];

    WrapCastForAggFuncs(&context, &mut descriptors);

    assert!(!descriptors[0].Args[0].as_any().is::<Column>());
    assert!(descriptors[1].Args[0].as_any().is::<Column>());
    assert!(!descriptors[2].Args[0].as_any().is::<Column>());
    assert!(descriptors[3].Args[0].as_any().is::<Column>());
}

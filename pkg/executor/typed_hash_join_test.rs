// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_meta_model::ColumnInfo;
use astersql_parser_ast::NewCIStr;
use astersql_parser_mysql::r#type::TypeLonglong;
use astersql_planner_core_base::{JoinType, PhysicalPlan as _, Plan as _};
use astersql_planner_core_operator_physicalop::{
    BasePhysicalJoin, BasePhysicalPlan, NewPhysicalHashJoin, PhysicalSchemaProducer,
    PhysicalTableScan,
};

use crate::builder::TypedScanBinding;
use crate::physical_plan_runtime_test::{MemoryRetriever, context, encode_row};
use crate::typed_kv_scan::KeyRange;

fn scan(table_id: i64, name: &str) -> PhysicalTableScan {
    let mut scan = PhysicalTableScan::New(context());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: table_id,
        Name: NewCIStr(name),
        PKIsHandle: true,
        ..Default::default()
    });
    scan.Columns = vec![ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
        ..Default::default()
    }];
    scan
}

fn ranges(table_id: i64) -> Vec<KeyRange> {
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(table_id).0);
    vec![KeyRange {
        end: start.PrefixNext(),
        start,
    }]
}

fn join_plan(join_type: JoinType) -> astersql_planner_core_operator_physicalop::PhysicalHashJoin {
    let context = context();
    let producer =
        PhysicalSchemaProducer::New(BasePhysicalPlan::New(context.clone(), "HashJoin", 0));
    let mut base = BasePhysicalJoin::New(producer, join_type);
    base.LeftJoinKeys = vec![astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(TypeLonglong),
        1,
        1,
        0,
    )];
    base.RightJoinKeys = vec![astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(TypeLonglong),
        1,
        1,
        0,
    )];
    let mut join = NewPhysicalHashJoin(base, 1, false);
    join.set_children(vec![Box::new(scan(101, "l")), Box::new(scan(102, "r"))]);
    join
}

fn encode_null_row(table_id: i64, handle: i64) -> (astersql_kv::Key, Vec<u8>) {
    let key = astersql_tablecodec::EncodeRowKeyWithHandle(
        table_id,
        Box::new(astersql_tablecodec::kv::IntHandle(handle)),
    );
    let value = astersql_tablecodec::EncodeRow(
        Some(astersql_tablecodec::time::UTC),
        vec![astersql_types::datum::Datum::default()],
        vec![1],
        Vec::new(),
        None,
        None,
        astersql_tablecodec::rowcodec::Encoder::new(true),
    )
    .unwrap();
    (astersql_kv::Key(key.0), value)
}

fn execute(join_type: JoinType) -> Vec<Vec<Option<i64>>> {
    let source = Arc::new(MemoryRetriever::default());
    for (table, handle, value) in [
        (101, 1, 10),
        (101, 2, 20),
        (102, 3, 10),
        (102, 4, 10),
        (102, 5, 30),
    ] {
        let (key, value) = encode_row(table, handle, value, "unused");
        source.Put(key, value);
    }
    let bindings = vec![
        TypedScanBinding {
            table_id: 101,
            retriever: source.clone(),
            ranges: ranges(101),
        },
        TypedScanBinding {
            table_id: 102,
            retriever: source,
            ranges: ranges(102),
        },
    ];
    let mut executor =
        crate::builder::BuildTypedPhysicalPlanWithBindings(&join_plan(join_type), bindings, 1, 1)
            .unwrap();
    executor.Open().unwrap();
    let fields = executor
        .Schema()
        .iter()
        .map(|column| column.field_type.clone())
        .collect::<Vec<_>>();
    let mut result = Vec::new();
    let mut output = executor.NewChunk();
    loop {
        executor.Next(&mut output).unwrap();
        if output.NumRows() == 0 {
            break;
        }
        for index in 0..output.NumRows() {
            result.push(
                output
                    .GetRow(index)
                    .GetDatumRow(&fields)
                    .into_iter()
                    .map(|value| (!value.IsNull()).then(|| value.GetInt64()))
                    .collect(),
            );
        }
    }
    assert!(executor.TakeLockKeys().is_empty());
    assert!(executor.Detach().is_none());
    executor.Close().unwrap();
    executor.Close().unwrap();
    result
}

#[test]
fn typed_hash_join_covers_join_types_duplicates_outer_rows_and_paging() {
    assert_eq!(
        execute(JoinType::InnerJoin),
        vec![vec![Some(10), Some(10)], vec![Some(10), Some(10)]]
    );
    assert_eq!(
        execute(JoinType::LeftOuterJoin),
        vec![
            vec![Some(10), Some(10)],
            vec![Some(10), Some(10)],
            vec![Some(20), None],
        ]
    );
    assert_eq!(
        execute(JoinType::RightOuterJoin),
        vec![
            vec![Some(10), Some(10)],
            vec![Some(10), Some(10)],
            vec![None, Some(30)],
        ]
    );
    assert_eq!(
        execute(JoinType::FullOuterJoin),
        vec![
            vec![Some(10), Some(10)],
            vec![Some(10), Some(10)],
            vec![Some(20), None],
            vec![None, Some(30)],
        ]
    );
    assert_eq!(execute(JoinType::SemiJoin), vec![vec![Some(10)]]);
    assert_eq!(execute(JoinType::AntiSemiJoin), vec![vec![Some(20)]]);
    assert_eq!(
        execute(JoinType::LeftOuterSemiJoin),
        vec![vec![Some(10), Some(1)], vec![Some(20), Some(0)]]
    );
    assert_eq!(
        execute(JoinType::AntiLeftOuterSemiJoin),
        vec![vec![Some(10), Some(0)], vec![Some(20), Some(1)]]
    );
}

#[test]
fn typed_hash_join_honors_cancellation_and_closes_both_children() {
    let source = Arc::new(MemoryRetriever::default());
    let bindings = vec![
        TypedScanBinding {
            table_id: 101,
            retriever: source.clone(),
            ranges: ranges(101),
        },
        TypedScanBinding {
            table_id: 102,
            retriever: source,
            ranges: ranges(102),
        },
    ];
    let mut executor = crate::builder::BuildTypedPhysicalPlanWithBindings(
        &join_plan(JoinType::InnerJoin),
        bindings,
        1,
        1,
    )
    .unwrap();
    executor.Open().unwrap();
    let killer = Arc::new(astersql_util_sqlkiller::sqlkiller::SQLKiller::default());
    killer.SendKillSignal(astersql_util_sqlkiller::sqlkiller::QueryInterrupted);
    let context = crate::adapter::ExecutionContext {
        sql_killer: Some(killer),
        ..Default::default()
    };
    let mut output = executor.NewChunk();
    assert!(executor.NextWithContext(&context, &mut output).is_err());
    executor.Close().unwrap();
}

#[test]
fn typed_hash_join_applies_null_equality_and_residual_conditions() {
    let source = Arc::new(MemoryRetriever::default());
    let (left_key, left_value) = encode_null_row(101, 1);
    let (right_key, right_value) = encode_null_row(102, 2);
    source.Put(left_key, left_value);
    source.Put(right_key, right_value);
    let bindings = vec![
        TypedScanBinding {
            table_id: 101,
            retriever: source.clone(),
            ranges: ranges(101),
        },
        TypedScanBinding {
            table_id: 102,
            retriever: source,
            ranges: ranges(102),
        },
    ];
    let mut ordinary = join_plan(JoinType::InnerJoin);
    let mut executor =
        crate::builder::BuildTypedPhysicalPlanWithBindings(&ordinary, bindings.clone(), 1, 1)
            .unwrap();
    executor.Open().unwrap();
    let mut output = executor.NewChunk();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    executor.Close().unwrap();

    ordinary.BasePhysicalJoin.IsNullEQ = vec![true];
    let mut executor =
        crate::builder::BuildTypedPhysicalPlanWithBindings(&ordinary, bindings.clone(), 1, 1)
            .unwrap();
    executor.Open().unwrap();
    let mut output = executor.NewChunk();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 1);
    executor.Close().unwrap();

    let false_residual = astersql_expression::NewFunction(
        ordinary.s_ctx().GetExprCtx(),
        astersql_parser_ast::GT,
        *astersql_expression::types::NewFieldType(astersql_expression::mysql::TypeTiny),
        vec![
            Box::new(astersql_expression::NewInt64Const(0)),
            Box::new(astersql_expression::NewInt64Const(1)),
        ],
    )
    .unwrap();
    ordinary.BasePhysicalJoin.OtherConditions = vec![false_residual];
    let mut executor =
        crate::builder::BuildTypedPhysicalPlanWithBindings(&ordinary, bindings, 1, 1).unwrap();
    executor.Open().unwrap();
    let mut output = executor.NewChunk();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    executor.Close().unwrap();
}

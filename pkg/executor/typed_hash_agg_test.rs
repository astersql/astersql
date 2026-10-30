// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_meta_model::ColumnInfo;
use astersql_parser_ast::NewCIStr;
use astersql_parser_mysql::r#type::TypeLonglong;
use astersql_planner_core_base::{PhysicalPlan as _, Plan as _};
use astersql_planner_core_operator_physicalop::{
    BasePhysicalAgg, BasePhysicalPlan, PhysicalHashAgg, PhysicalSchemaProducer, PhysicalTableScan,
};

use crate::physical_plan_runtime_test::{MemoryRetriever, context, encode_row};
use crate::typed_kv_scan::KeyRange;

fn hash_agg_plan(grouped: bool) -> PhysicalHashAgg {
    let context = context();
    let column = astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(TypeLonglong),
        1,
        1,
        0,
    );
    let mut scan = PhysicalTableScan::New(context.clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 98,
        Name: NewCIStr("t"),
        ..Default::default()
    });
    scan.Columns = vec![ColumnInfo {
        ID: 1,
        Name: NewCIStr("a"),
        FieldType: astersql_parser_types::NewFieldType(TypeLonglong),
        ..Default::default()
    }];
    let count = astersql_expression_aggregation::NewAggFuncDesc(
        context.GetExprCtx(),
        astersql_parser_ast::AggFuncCount,
        vec![Box::new(astersql_expression::NewInt64Const(1))],
        false,
    )
    .unwrap();
    let mut aggregate = PhysicalHashAgg {
        BasePhysicalAgg: BasePhysicalAgg::New(PhysicalSchemaProducer::New(BasePhysicalPlan::New(
            context, "HashAgg", 0,
        ))),
        TiflashPreAggMode: String::new(),
    };
    aggregate.BasePhysicalAgg.AggFuncs = vec![count];
    if grouped {
        aggregate.BasePhysicalAgg.GroupByItems = vec![Box::new(column)];
    }
    aggregate.set_children(vec![Box::new(scan)]);
    aggregate
}

fn ranges() -> Vec<KeyRange> {
    let start = astersql_kv::Key(astersql_tablecodec::GenTableRecordPrefix(98).0);
    vec![KeyRange {
        end: start.PrefixNext(),
        start,
    }]
}

#[test]
fn typed_hash_agg_groups_across_input_pages_and_pages_output() {
    let retriever = Arc::new(MemoryRetriever::default());
    for (handle, value) in [(1, 10), (2, 10), (3, 20)] {
        let (key, row) = encode_row(98, handle, value, "unused");
        retriever.Put(key, row);
    }
    let mut executor =
        crate::builder::BuildTypedPhysicalPlan(&hash_agg_plan(true), retriever, ranges(), 1, 1)
            .expect("build typed grouped HashAgg");
    executor.Open().unwrap();
    let mut output = executor.NewChunk();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 1);
    assert_eq!(output.GetRow(0).GetInt64(0), 2);
    assert!(executor.TakeLockKeys().is_empty());
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 1);
    assert_eq!(output.GetRow(0).GetInt64(0), 1);
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    assert!(executor.Detach().is_none());
    executor.Close().unwrap();
}

#[test]
fn typed_hash_agg_scalar_empty_input_emits_count_zero() {
    let retriever = Arc::new(MemoryRetriever::default());
    let mut executor =
        crate::builder::BuildTypedPhysicalPlan(&hash_agg_plan(false), retriever, ranges(), 1, 1)
            .expect("build typed scalar HashAgg");
    executor.Open().unwrap();
    let mut output = executor.NewChunk();
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 1);
    assert_eq!(output.GetRow(0).GetInt64(0), 0);
    executor.Next(&mut output).unwrap();
    assert_eq!(output.NumRows(), 0);
    executor.Close().unwrap();
}

#[test]
fn typed_hash_agg_honors_cancellation_before_consuming_child() {
    let retriever = Arc::new(MemoryRetriever::default());
    let mut executor =
        crate::builder::BuildTypedPhysicalPlan(&hash_agg_plan(false), retriever, ranges(), 1, 1)
            .expect("build typed scalar HashAgg");
    executor.Open().unwrap();
    let killer = Arc::new(astersql_util_sqlkiller::sqlkiller::SQLKiller::default());
    killer.SendKillSignal(astersql_util_sqlkiller::sqlkiller::QueryInterrupted);
    let execution_context = crate::adapter::ExecutionContext {
        sql_killer: Some(killer),
        ..Default::default()
    };
    let mut output = executor.NewChunk();
    let error = executor
        .NextWithContext(&execution_context, &mut output)
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("Query execution was interrupted")
    );
    executor.Close().unwrap();
}

#[test]
fn typed_hash_agg_count_extrema_real_scan_and_empty_input() {
    let retriever = Arc::new(MemoryRetriever::default());
    for (handle, value) in [(1, 1), (2, 1), (3, 2), (4, 2), (5, 2)] {
        let (key, row) = encode_row(98, handle, value, "unused");
        retriever.Put(key, row);
    }
    let mut plan = hash_agg_plan(false);
    let ctx = plan.s_ctx().clone();
    plan.BasePhysicalAgg.AggFuncs = [
        astersql_parser_ast::AggFuncMaxCount,
        astersql_parser_ast::AggFuncMinCount,
    ]
    .into_iter()
    .map(|name| {
        astersql_expression_aggregation::NewAggFuncDesc(
            ctx.GetExprCtx(),
            name,
            vec![Box::new(astersql_expression::Column::new(
                *astersql_expression::types::NewFieldType(TypeLonglong),
                1,
                1,
                0,
            ))],
            false,
        )
        .unwrap()
    })
    .collect();
    for (input, expected) in [
        (retriever, [3, 2]),
        (Arc::new(MemoryRetriever::default()), [0, 0]),
    ] {
        let mut executor =
            crate::builder::BuildTypedPhysicalPlan(&plan, input, ranges(), 1, 2).unwrap();
        executor.Open().unwrap();
        let mut output = executor.NewChunk();
        executor.Next(&mut output).unwrap();
        assert_eq!(output.NumRows(), 1);
        assert_eq!(output.GetRow(0).GetInt64(0), expected[0]);
        assert_eq!(output.GetRow(0).GetInt64(1), expected[1]);
        executor.Close().unwrap();
    }
}

#[test]
fn pg09_01_execution() {
    typed_hash_agg_scalar_empty_input_emits_count_zero();
    typed_hash_agg_groups_across_input_pages_and_pages_output();
    let mut e = crate::builder::BuildTypedPhysicalPlan(
        &hash_agg_plan(true),
        Arc::new(MemoryRetriever::default()),
        ranges(),
        1,
        1,
    )
    .unwrap();
    e.Open().unwrap();
    let mut out = e.NewChunk();
    e.Next(&mut out).unwrap();
    assert_eq!(out.NumRows(), 0);
    e.Close().unwrap();
}

#[test]
fn pg09_01_count_overflow() {
    let source = Arc::new(MemoryRetriever::default());
    for (h, v) in [(1, i64::MAX), (2, 1)] {
        let (k, r) = encode_row(98, h, v, "unused");
        source.Put(k, r);
    }
    let mut p = hash_agg_plan(false);
    p.BasePhysicalAgg.AggFuncs[0].Mode =
        astersql_expression_aggregation::AggFunctionMode::FinalMode;
    p.BasePhysicalAgg.AggFuncs[0].Args = vec![Box::new(astersql_expression::Column::new(
        *astersql_expression::types::NewFieldType(TypeLonglong),
        1,
        1,
        0,
    ))];
    let mut e = crate::builder::BuildTypedPhysicalPlan(&p, source, ranges(), 1, 1).unwrap();
    e.Open().unwrap();
    let mut out = e.NewChunk();
    let result = e.Next(&mut out);
    e.Close().unwrap();
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("COUNT bigint out of range")
    );
}

#[test]
fn pg09_01_null_groups_and_pages() {
    let source = Arc::new(MemoryRetriever::default());
    for (index, value) in [None, Some(7), None, Some(7), Some(9)].iter().enumerate() {
        let key = astersql_tablecodec::EncodeRowKeyWithHandle(
            98,
            Box::new(astersql_tablecodec::kv::IntHandle(index as i64 + 1)),
        );
        let value = astersql_tablecodec::EncodeRow(
            Some(astersql_tablecodec::time::UTC),
            vec![value.map_or_else(Default::default, astersql_types::datum::NewIntDatum)],
            vec![1],
            Vec::new(),
            None,
            None,
            astersql_tablecodec::rowcodec::Encoder::new(true),
        )
        .unwrap();
        source.Put(astersql_kv::Key(key.0), value);
    }
    for grouped in [false, true] {
        let mut p = hash_agg_plan(grouped);
        let mut count = p.BasePhysicalAgg.AggFuncs[0].clone();
        count.Args = vec![Box::new(astersql_expression::Column::new(
            *astersql_expression::types::NewFieldType(TypeLonglong),
            1,
            1,
            0,
        ))];
        p.BasePhysicalAgg.AggFuncs.push(count);
        let mut e =
            crate::builder::BuildTypedPhysicalPlan(&p, source.clone(), ranges(), 1, 1).unwrap();
        e.Open().unwrap();
        let mut out = e.NewChunk();
        let mut actual = vec![];
        loop {
            e.Next(&mut out).unwrap();
            if out.NumRows() == 0 {
                break;
            }
            assert_eq!(out.NumRows(), 1);
            actual.push((out.GetRow(0).GetInt64(0), out.GetRow(0).GetInt64(1)));
        }
        e.Close().unwrap();
        assert_eq!(
            actual,
            if grouped {
                vec![(2, 0), (2, 2), (1, 1)]
            } else {
                vec![(5, 3)]
            }
        );
    }
    assert_eq!(
        crate::typed_hash_agg::CountNonNull(context(), vec![false; 2050]).unwrap(),
        0
    );
    assert_eq!(
        crate::typed_hash_agg::CountNonNull(context(), (0..2050).map(|i| i % 2 == 0).collect())
            .unwrap(),
        1025
    );
}

#[test]
fn pg09_02_execution() {
    use crate::typed_hash_agg::SumSmallInt;
    for (values, expected) in [
        (vec![], None),
        (vec![None], None),
        (vec![Some(1), None, Some(2)], Some(3)),
        (vec![Some(i32::MAX as i64); 2], Some(4294967294)),
        (vec![Some(i64::MIN), Some(1)], Some(i64::MIN + 1)),
    ] {
        assert_eq!(SumSmallInt(context(), values).unwrap(), expected);
    }
    for values in [
        vec![Some(i64::MAX), Some(1)],
        vec![Some(i64::MIN), Some(-1)],
    ] {
        assert!(
            SumSmallInt(context(), values)
                .unwrap_err()
                .to_string()
                .contains("SUM bigint out of range")
        );
    }
}

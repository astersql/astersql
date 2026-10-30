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
    hash_agg_plan_context(grouped, context())
}

fn hash_agg_plan_context(
    grouped: bool,
    context: astersql_planner_core_base::ContextRef,
) -> PhysicalHashAgg {
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

#[test]
fn pg09_03_execution() {
    use crate::typed_hash_agg::SumNumeric;
    for (values, expected) in [
        (vec![], None),
        (vec![None], None),
        (vec![Some("0"), None], Some("0")),
        (
            vec![Some("9223372036854775807"); 2],
            Some("18446744073709551614"),
        ),
        (
            vec![
                Some("999999999999999999999999999999999999.999"),
                Some("0.001"),
            ],
            Some("1000000000000000000000000000000000000.000"),
        ),
        (vec![Some("1.20"), Some("-1.2")], Some("0.00")),
        (vec![Some("Infinity"), Some("-Infinity")], Some("NaN")),
        (vec![Some("NaN"), Some("1")], Some("NaN")),
        (vec![Some("-Infinity"), Some("1")], Some("-Infinity")),
    ] {
        assert_eq!(
            SumNumeric(
                context(),
                values.into_iter().map(|v| v.map(str::to_owned)).collect()
            )
            .unwrap()
            .as_deref(),
            expected
        );
    }
    assert_eq!(
        SumNumeric(context(), vec![Some("0.01".into()); 2050])
            .unwrap()
            .as_deref(),
        Some("20.50")
    );
    assert!(SumNumeric(context(), vec![Some("invalid".into())]).is_err());
    assert_eq!(
        SumNumeric(context(), vec![Some("1".into())])
            .unwrap()
            .as_deref(),
        Some("1")
    );
}

#[test]
fn pg09_04_execution() {
    use crate::typed_hash_agg::AvgNumeric;
    for (values, expected) in [
        (vec![], None),
        (vec![None], None),
        (vec![Some("1"), None, Some("2")], Some("1.5000000000000000")),
        (
            vec![Some("1"), Some("0"), Some("0")],
            Some("0.33333333333333333333"),
        ),
        (
            vec![Some("1.20"), Some("-1.2")],
            Some("0.00000000000000000000"),
        ),
        (
            vec![Some("9223372036854775807"); 2],
            Some("9223372036854775807"),
        ),
    ] {
        assert_eq!(
            AvgNumeric(
                context(),
                values.into_iter().map(|v| v.map(str::to_owned)).collect()
            )
            .unwrap()
            .as_deref(),
            expected
        );
    }
    assert_eq!(
        AvgNumeric(context(), vec![Some("0.01".into()); 2050])
            .unwrap()
            .as_deref(),
        Some("0.01000000000000000000")
    );
    assert!(AvgNumeric(context(), vec![Some("invalid".into())]).is_err());
}

#[test]
fn pg09_05_execution() {
    for (tp, name, values, expected) in [
        (
            astersql_parser_mysql::r#type::TypeFloat,
            "pg_sum_float4",
            vec![16777216.0, 1.0, -16777216.0],
            0.0,
        ),
        (
            astersql_parser_mysql::r#type::TypeDouble,
            "pg_sum_float8",
            vec![9007199254740992.0, 1.0, -9007199254740992.0],
            0.0,
        ),
    ] {
        let source = Arc::new(MemoryRetriever::default());
        for (i, v) in values.into_iter().enumerate() {
            let key = astersql_tablecodec::EncodeRowKeyWithHandle(
                98,
                Box::new(astersql_tablecodec::kv::IntHandle(i as i64 + 1)),
            );
            let value = astersql_tablecodec::EncodeRow(
                Some(astersql_tablecodec::time::UTC),
                vec![astersql_types::datum::NewFloat64Datum(v)],
                vec![1],
                Vec::new(),
                None,
                None,
                astersql_tablecodec::rowcodec::Encoder::new(true),
            )
            .unwrap();
            source.Put(astersql_kv::Key(key.0), value);
        }
        let mut p = hash_agg_plan(false);
        let ft = astersql_parser_types::NewFieldType(tp);
        let mut scan = PhysicalTableScan::New(context());
        scan.Table = Some(astersql_meta_model::TableInfo {
            ID: 98,
            Name: NewCIStr("t"),
            ..Default::default()
        });
        scan.Columns = vec![ColumnInfo {
            ID: 1,
            Name: NewCIStr("a"),
            FieldType: ft.clone(),
            ..Default::default()
        }];
        p.set_children(vec![Box::new(scan)]);
        let mut f = p.BasePhysicalAgg.AggFuncs[0].clone();
        f.Name = name.into();
        f.Args = vec![Box::new(astersql_expression::Column::new(
            ft.clone(),
            1,
            1,
            0,
        ))];
        f.RetTp = Some(ft);
        p.BasePhysicalAgg.AggFuncs = vec![f];
        let mut e = crate::builder::BuildTypedPhysicalPlan(&p, source, ranges(), 1, 1).unwrap();
        e.Open().unwrap();
        let mut out = e.NewChunk();
        e.Next(&mut out).unwrap();
        assert_eq!(out.NumRows(), 1);
        let actual = if tp == astersql_parser_mysql::r#type::TypeFloat {
            f64::from(out.GetRow(0).GetFloat32(0))
        } else {
            out.GetRow(0).GetFloat64(0)
        };
        assert_eq!(actual, expected);
        e.Next(&mut out).unwrap();
        assert_eq!(out.NumRows(), 0);
        e.Close().unwrap();
    }
}

#[test]
fn pg09_05_execution_boundaries() {
    use crate::typed_hash_agg::SumFloat;
    for single in [true, false] {
        assert_eq!(SumFloat(context(), vec![], single).unwrap(), None);
        assert_eq!(SumFloat(context(), vec![None; 2050], single).unwrap(), None);
        assert_eq!(
            SumFloat(context(), vec![Some("1".into()); 2050], single).unwrap(),
            Some(2050.0)
        );
        assert_eq!(
            SumFloat(context(), vec![Some("-0".into())], single)
                .unwrap()
                .unwrap()
                .to_bits(),
            (-0f64).to_bits()
        );
        assert!(
            SumFloat(
                context(),
                vec![Some("NaN".into()), Some("1".into())],
                single
            )
            .unwrap()
            .unwrap()
            .is_nan()
        );
        assert!(
            SumFloat(
                context(),
                vec![Some("Infinity".into()), Some("-Infinity".into())],
                single
            )
            .unwrap()
            .unwrap()
            .is_nan()
        );
        assert_eq!(
            SumFloat(context(), vec![Some("-Infinity".into()), None], single).unwrap(),
            Some(f64::NEG_INFINITY)
        );
        for value in [
            if single { "3e38" } else { "1e308" },
            if single { "-3e38" } else { "-1e308" },
        ] {
            let error = SumFloat(context(), vec![Some(value.into()); 2], single).unwrap_err();
            assert!(
                error
                    .downcast_ref::<crate::typed_hash_agg::FloatSumOverflow>()
                    .is_some()
            );
        }
        assert!(SumFloat(context(), vec![Some("invalid".into())], single).is_err());
        assert_eq!(
            SumFloat(
                context(),
                vec![Some("1".into()), None, Some("2".into())],
                single
            )
            .unwrap(),
            Some(3.0)
        );
    }
}

#[test]
fn pg09_06_execution() {
    for (tp, name, values, expected) in [
        (
            astersql_parser_mysql::r#type::TypeFloat,
            "pg_avg_float4",
            vec![16777216.0, 1.0, -16777216.0],
            1.0 / 3.0,
        ),
        (
            astersql_parser_mysql::r#type::TypeDouble,
            "pg_avg_float8",
            vec![9007199254740992.0, 1.0, -9007199254740992.0],
            0.0,
        ),
    ] {
        let source = Arc::new(MemoryRetriever::default());
        for (i, v) in values.into_iter().enumerate() {
            let key = astersql_tablecodec::EncodeRowKeyWithHandle(
                98,
                Box::new(astersql_tablecodec::kv::IntHandle(i as i64 + 1)),
            );
            let value = astersql_tablecodec::EncodeRow(
                Some(astersql_tablecodec::time::UTC),
                vec![astersql_types::datum::NewFloat64Datum(v)],
                vec![1],
                Vec::new(),
                None,
                None,
                astersql_tablecodec::rowcodec::Encoder::new(true),
            )
            .unwrap();
            source.Put(astersql_kv::Key(key.0), value);
        }
        let mut p = hash_agg_plan(false);
        let ft = astersql_parser_types::NewFieldType(tp);
        let mut scan = PhysicalTableScan::New(context());
        scan.Table = Some(astersql_meta_model::TableInfo {
            ID: 98,
            Name: NewCIStr("t"),
            ..Default::default()
        });
        scan.Columns = vec![ColumnInfo {
            ID: 1,
            Name: NewCIStr("a"),
            FieldType: ft.clone(),
            ..Default::default()
        }];
        p.set_children(vec![Box::new(scan)]);
        let mut f = p.BasePhysicalAgg.AggFuncs[0].clone();
        f.Name = name.into();
        f.Args = vec![Box::new(astersql_expression::Column::new(
            ft.clone(),
            1,
            1,
            0,
        ))];
        f.RetTp = Some(astersql_parser_types::NewFieldType(
            astersql_parser_mysql::r#type::TypeDouble,
        ));
        p.BasePhysicalAgg.AggFuncs = vec![f];
        let mut e = crate::builder::BuildTypedPhysicalPlan(&p, source, ranges(), 1, 1).unwrap();
        e.Open().unwrap();
        let mut out = e.NewChunk();
        e.Next(&mut out).unwrap();
        assert_eq!(out.NumRows(), 1);
        let actual = out.GetRow(0).GetFloat64(0);
        assert_eq!(actual, expected);
        e.Next(&mut out).unwrap();
        assert_eq!(out.NumRows(), 0);
        e.Close().unwrap();
    }
}

#[test]
fn pg09_06_execution_boundaries() {
    use crate::typed_hash_agg::AvgFloat;
    for single in [true, false] {
        assert_eq!(AvgFloat(context(), vec![], single).unwrap(), None);
        assert_eq!(AvgFloat(context(), vec![None; 2050], single).unwrap(), None);
        assert_eq!(
            AvgFloat(context(), vec![Some("2".into()); 2050], single).unwrap(),
            Some(2.0)
        );
        assert_eq!(
            AvgFloat(
                context(),
                vec![Some("1".into()), None, Some("2".into())],
                single
            )
            .unwrap(),
            Some(1.5)
        );
        assert_eq!(
            AvgFloat(context(), vec![Some("-0".into())], single)
                .unwrap()
                .unwrap()
                .to_bits(),
            0f64.to_bits()
        );
        for (a, b) in [("NaN", "1"), ("Infinity", "-Infinity")] {
            assert!(
                AvgFloat(context(), vec![Some(a.into()), Some(b.into())], single)
                    .unwrap()
                    .unwrap()
                    .is_nan()
            );
        }
        for (value, expected) in [
            ("Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
        ] {
            assert_eq!(
                AvgFloat(
                    context(),
                    vec![Some(value.into()), Some("1".into())],
                    single
                )
                .unwrap(),
                Some(expected)
            );
        }
        assert!(AvgFloat(context(), vec![Some("invalid".into())], single).is_err());
    }
    for values in [
        vec![Some("1e308".into()); 2],
        vec![Some("1e154".into()), Some("-1e154".into())],
    ] {
        let error = AvgFloat(context(), values, false).unwrap_err();
        assert!(
            error
                .downcast_ref::<crate::typed_hash_agg::FloatSumOverflow>()
                .is_some()
        );
    }
    assert!(
        AvgFloat(context(), vec![Some("3e38".into()); 2], true)
            .unwrap()
            .unwrap()
            .is_finite()
    );
}

#[test]
fn pg09_07_execution() {
    // Each input page contains one row; filters must neither remove groups nor
    // leak selection into the sibling aggregate, including after reopen.
    for grouped in [false, true] {
        let source = Arc::new(MemoryRetriever::default());
        for (handle, value) in [(1, 10), (2, 10), (3, 20)] {
            let (key, row) = encode_row(98, handle, value, "unused");
            source.Put(key, row);
        }
        let mut p = hash_agg_plan(grouped);
        let all = p.BasePhysicalAgg.AggFuncs[0].clone();
        let mut excluded = all.Clone();
        excluded.Filter = Some(Box::new(astersql_expression::NewInt64Const(0)));
        let mut null_filter = all.Clone();
        null_filter.Filter = Some(Box::new(astersql_expression::NewNull()));
        let mut sum = excluded.Clone();
        sum.Name = "sum".into();
        sum.Args = vec![Box::new(astersql_expression::Column::new(
            *astersql_expression::types::NewFieldType(TypeLonglong),
            1,
            1,
            0,
        ))];
        let mut avg = sum.Clone();
        avg.Name = "avg".into();
        assert!(!all.Equal(context().GetExprCtx().GetEvalCtx(), &excluded));
        assert!(excluded.Equal(context().GetExprCtx().GetEvalCtx(), &excluded.Clone()));
        p.BasePhysicalAgg.AggFuncs = vec![all, excluded, null_filter, sum, avg];
        let mut e = crate::builder::BuildTypedPhysicalPlan(&p, source, ranges(), 1, 1).unwrap();
        for _ in 0..2 {
            e.Open().unwrap();
            let mut out = e.NewChunk();
            let mut counts = vec![];
            loop {
                e.Next(&mut out).unwrap();
                if out.NumRows() == 0 {
                    break;
                }
                let r = out.GetRow(0);
                counts.push(r.GetInt64(0));
                assert_eq!(r.GetInt64(1), 0);
                assert_eq!(r.GetInt64(2), 0);
                assert!(r.IsNull(3));
                assert!(r.IsNull(4));
            }
            assert_eq!(counts, if grouped { vec![2, 1] } else { vec![3] });
            e.Close().unwrap();
        }
    }
}

#[test]
fn pg09_08_execution() {
    // Each input page contains one row; filters must neither remove groups nor
    // leak selection into the sibling aggregate, including after reopen.
    for grouped in [false, true] {
        let source = Arc::new(MemoryRetriever::default());
        for (handle, value) in [(1, 10), (2, 10), (3, 20)] {
            let (key, row) = encode_row(98, handle, value, "unused");
            source.Put(key, row);
        }
        let mut p = hash_agg_plan(grouped);
        let mut all = p.BasePhysicalAgg.AggFuncs[0].clone();
        all.HasDistinct = true;
        all.Args = vec![Box::new(astersql_expression::Column::new(
            *astersql_expression::types::NewFieldType(TypeLonglong),
            1,
            1,
            0,
        ))];
        let mut excluded = all.Clone();
        excluded.Filter = Some(Box::new(astersql_expression::NewInt64Const(0)));
        let mut null_filter = all.Clone();
        null_filter.Filter = Some(Box::new(astersql_expression::NewNull()));
        let mut sum = excluded.Clone();
        sum.Name = "sum".into();
        sum.Args = vec![Box::new(astersql_expression::Column::new(
            *astersql_expression::types::NewFieldType(TypeLonglong),
            1,
            1,
            0,
        ))];
        let mut avg = sum.Clone();
        avg.Name = "avg".into();
        assert!(!all.Equal(context().GetExprCtx().GetEvalCtx(), &excluded));
        assert!(excluded.Equal(context().GetExprCtx().GetEvalCtx(), &excluded.Clone()));
        p.BasePhysicalAgg.AggFuncs = vec![all, excluded, null_filter, sum, avg];
        let mut e = crate::builder::BuildTypedPhysicalPlan(&p, source, ranges(), 1, 1).unwrap();
        for _ in 0..2 {
            e.Open().unwrap();
            let mut out = e.NewChunk();
            let mut counts = vec![];
            loop {
                e.Next(&mut out).unwrap();
                if out.NumRows() == 0 {
                    break;
                }
                let r = out.GetRow(0);
                counts.push(r.GetInt64(0));
                assert_eq!(r.GetInt64(1), 0);
                assert_eq!(r.GetInt64(2), 0);
                assert!(r.IsNull(3));
                assert!(r.IsNull(4));
            }
            assert_eq!(counts, if grouped { vec![1, 1] } else { vec![2] });
            e.Close().unwrap();
        }
    }
}

#[test]
fn pg09_08_numeric_keys() {
    use crate::typed_hash_agg::NumericDistinctKey as key;
    for oid in [1700, 700, 701] {
        for (left, right) in [
            ("1", "1.00"),
            ("0", "-0.00"),
            ("NaN", "nan"),
            ("Infinity", "inf"),
        ] {
            assert_eq!(
                key(left, oid).unwrap(),
                key(right, oid).unwrap(),
                "{oid}: {left}, {right}"
            );
        }
        assert_ne!(
            key("Infinity", oid).unwrap(),
            key("-Infinity", oid).unwrap()
        );
        assert!(key("invalid", oid).is_err());
    }
    assert_ne!(
        key("9007199254740992", 1700).unwrap(),
        key("9007199254740993", 1700).unwrap()
    );
    assert_eq!(key("1e10000", 1700).unwrap(), key("10e9999", 1700).unwrap());
}

#[test]
fn pg09_09_execution() {
    // A page per input proves ORDER BY is applied across child chunks.
    for desc in [false, true] {
        let source = Arc::new(MemoryRetriever::default());
        for (handle, value) in [(1, 10), (2, 30), (3, 20)] {
            let (key, row) = encode_row(98, handle, value, "unused");
            source.Put(key, row);
        }
        let mut p = hash_agg_plan(false);
        let mut first = p.BasePhysicalAgg.AggFuncs[0].clone();
        first.Name = "firstrow".into();
        first.Args = vec![Box::new(astersql_expression::Column::new(
            *astersql_expression::types::NewFieldType(TypeLonglong),
            1,
            1,
            0,
        ))];
        first.OrderByItems = vec![astersql_planner_util::ByItems {
            Expr: first.Args[0].clone(),
            Desc: desc,
        }];
        p.BasePhysicalAgg.AggFuncs = vec![first];
        let mut e = crate::builder::BuildTypedPhysicalPlan(&p, source, ranges(), 1, 1).unwrap();
        for _ in 0..2 {
            e.Open().unwrap();
            let mut out = e.NewChunk();
            e.Next(&mut out).unwrap();
            assert_eq!(out.NumRows(), 1);
            assert_eq!(out.GetRow(0).GetInt64(0), if desc { 30 } else { 10 });
            e.Next(&mut out).unwrap();
            assert_eq!(out.NumRows(), 0);
            e.Close().unwrap();
        }
    }
}

#[test]
fn pg09_10_execution_rejects_lossy_partial_states() {
    use astersql_expression_aggregation::AggFunctionMode::*;
    for mode in [Partial1Mode, Partial2Mode, FinalMode] {
        for name in ["pg_avg_float4", "pg_avg_float8", "pg_avg_numeric", "avg"] {
            let mut p = hash_agg_plan_context(
                false,
                crate::physical_plan_runtime_test::context_with_dialect(
                    astersql_parser_ast::SqlDialect::PostgreSQL,
                ),
            );
            p.BasePhysicalAgg.AggFuncs[0].Name = name.into();
            p.BasePhysicalAgg.AggFuncs[0].Mode = mode;
            assert!(
                crate::builder::BuildTypedPhysicalPlan(
                    &p,
                    Arc::new(MemoryRetriever::default()),
                    ranges(),
                    1,
                    1
                )
                .is_err(),
                "{name}: {mode:?} must not consume a scalar as float AVG state"
            );
        }
        for ordered in [false, true] {
            let mut p = hash_agg_plan_context(
                false,
                crate::physical_plan_runtime_test::context_with_dialect(
                    astersql_parser_ast::SqlDialect::PostgreSQL,
                ),
            );
            let f = &mut p.BasePhysicalAgg.AggFuncs[0];
            f.Mode = mode;
            if ordered {
                f.OrderByItems = vec![astersql_planner_util::ByItems {
                    Expr: f.Args[0].clone(),
                    Desc: false,
                }];
            } else {
                f.HasDistinct = true;
            }
            assert!(
                crate::builder::BuildTypedPhysicalPlan(
                    &p,
                    Arc::new(MemoryRetriever::default()),
                    ranges(),
                    1,
                    1
                )
                .is_err(),
                "local deduplication/order must not masquerade as global state"
            );
        }
    }
}

// Real typed KV scan, one row per page. No aggregate values are mocked.
fn pg10_run(
    functions: Vec<astersql_expression_aggregation::AggFuncDesc>,
    fields: Vec<astersql_expression::types::FieldType>,
    rows: Vec<Vec<astersql_types::datum::Datum>>,
) -> Vec<astersql_types::datum::Datum> {
    let source = Arc::new(MemoryRetriever::default());
    for (index, values) in rows.into_iter().enumerate() {
        let key = astersql_tablecodec::EncodeRowKeyWithHandle(
            98,
            Box::new(astersql_tablecodec::kv::IntHandle(index as i64 + 1)),
        );
        let encoded = astersql_tablecodec::EncodeRow(
            Some(astersql_tablecodec::time::UTC),
            values,
            (1..=fields.len() as i64).collect(),
            Vec::new(),
            None,
            None,
            astersql_tablecodec::rowcodec::Encoder::new(true),
        )
        .unwrap();
        source.Put(astersql_kv::Key(key.0), encoded);
    }
    let mut p = hash_agg_plan(false);
    let mut scan = PhysicalTableScan::New(p.s_ctx().clone());
    scan.Table = Some(astersql_meta_model::TableInfo {
        ID: 98,
        Name: NewCIStr("t"),
        ..Default::default()
    });
    scan.Columns = fields
        .into_iter()
        .enumerate()
        .map(|(index, field)| ColumnInfo {
            ID: index as i64 + 1,
            Name: NewCIStr(&format!("c{index}")),
            FieldType: field,
            ..Default::default()
        })
        .collect();
    let expected_types = functions
        .iter()
        .map(|f| f.RetTp.clone().unwrap())
        .collect::<Vec<_>>();
    p.BasePhysicalAgg.AggFuncs = functions;
    p.set_children(vec![Box::new(scan)]);
    let mut e = crate::builder::BuildTypedPhysicalPlan(&p, source, ranges(), 1, 1).unwrap();
    assert_eq!(
        e.Schema()
            .iter()
            .map(|c| c.field_type.clone())
            .collect::<Vec<_>>(),
        expected_types
    );
    let mut result = vec![];
    for _ in 0..2 {
        e.Open().unwrap();
        let mut out = e.NewChunk();
        e.Next(&mut out).unwrap();
        assert_eq!(out.NumRows(), 1);
        result = out.GetRow(0).GetDatumRow(&expected_types);
        e.Next(&mut out).unwrap();
        assert_eq!(out.NumRows(), 0);
        e.Close().unwrap();
    }
    result
}

#[test]
fn pg09_10_execution_numeric_state_and_identity() {
    use astersql_expression_aggregation::{NewAggFuncDesc, Partial2Mode};
    use astersql_types::datum::{NewIntDatum, NewStringDatum};
    let ctx = context();
    let mut numeric =
        *astersql_expression::types::NewFieldType(astersql_parser_mysql::r#type::TypeVarString);
    let mut count = NewAggFuncDesc(
        ctx.GetExprCtx(),
        "count",
        vec![Box::new(astersql_expression::Column::new(
            numeric.clone(),
            1,
            1,
            0,
        ))],
        false,
    )
    .unwrap();
    let mut sum = count.Clone();
    sum.Name = "pg_sum_numeric".into();
    sum.RetTp = Some(numeric.clone());
    let mut avg = sum.Clone();
    avg.Name = "pg_avg_numeric".into();
    let mut plan = hash_agg_plan(false);
    plan.BasePhysicalAgg.AggFuncs = vec![count.Clone(), sum.Clone(), avg.Clone()];
    let (partial, final_info) =
        astersql_planner_core_operator_physicalop::BuildFinalModeAggregation(
            &plan.BasePhysicalAgg,
            false,
        )
        .unwrap();
    assert_eq!(
        partial.AggFuncs.len(),
        4,
        "AVG must carry count and sum, never a finalized mean"
    );
    assert_eq!(final_info.AggFuncs[2].Args.len(), 2);
    assert_eq!(final_info.AggFuncs[2].RetTp, avg.RetTp);
    let partial_fields = partial
        .Schema
        .Columns
        .iter()
        .map(|c| c.RetType.clone().unwrap())
        .collect::<Vec<_>>();
    for (shards, expected) in [
        (
            vec![
                vec![Some("1")],
                vec![Some("2"), Some("3"), Some("4")],
                vec![],
            ],
            [Some("4"), Some("10"), Some("2.5000000000000000")],
        ),
        (vec![vec![], vec![]], [Some("0"), None, None]),
        (vec![vec![None], vec![None]], [Some("0"), None, None]),
        (
            vec![
                vec![Some("9223372036854775807")],
                vec![Some("9223372036854775807")],
            ],
            [
                Some("2"),
                Some("18446744073709551614"),
                Some("9223372036854775807"),
            ],
        ),
    ] {
        let rows = shards
            .iter()
            .flat_map(|s| s.iter())
            .map(|v| vec![v.map_or_else(Default::default, |v| NewStringDatum(v.to_owned()))])
            .collect::<Vec<_>>();
        let complete = pg10_run(
            plan.BasePhysicalAgg.AggFuncs.clone(),
            vec![numeric.clone()],
            rows,
        );
        assert_eq!(
            complete
                .iter()
                .map(|v| if v.IsNull() {
                    None
                } else {
                    Some(v.ToString().unwrap())
                })
                .collect::<Vec<_>>(),
            expected.map(|v| v.map(str::to_owned))
        );
        let partial_rows = shards
            .into_iter()
            .map(|shard| {
                pg10_run(
                    partial.AggFuncs.clone(),
                    vec![numeric.clone()],
                    shard
                        .into_iter()
                        .map(|v| {
                            vec![v.map_or_else(Default::default, |v| NewStringDatum(v.to_owned()))]
                        })
                        .collect(),
                )
            })
            .collect::<Vec<_>>();
        let final_values = pg10_run(
            final_info.AggFuncs.clone(),
            partial_fields.clone(),
            partial_rows.clone(),
        );
        let repr = |values: &[astersql_types::datum::Datum]| {
            values
                .iter()
                .map(|v| {
                    (
                        v.Kind(),
                        if v.IsNull() {
                            None
                        } else {
                            Some(v.ToString().unwrap())
                        },
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(repr(&final_values), repr(&complete));
        // Exercise the real planner again for Final -> Partial2 -> Final.
        let mut merge_plan = hash_agg_plan(false);
        merge_plan.BasePhysicalAgg.AggFuncs = vec![final_info.AggFuncs[2].Clone()];
        let (intermediate, final_again) =
            astersql_planner_core_operator_physicalop::BuildFinalModeAggregation(
                &merge_plan.BasePhysicalAgg,
                false,
            )
            .unwrap();
        assert!(intermediate.AggFuncs.iter().all(|f| f.Mode == Partial2Mode));
        let state = pg10_run(intermediate.AggFuncs, partial_fields.clone(), partial_rows);
        let fields = intermediate
            .Schema
            .Columns
            .iter()
            .map(|c| c.RetType.clone().unwrap())
            .collect();
        assert_eq!(
            repr(&pg10_run(final_again.AggFuncs, fields, vec![state])),
            repr(&complete[2..])
        );
    }
    // Count is checked independently of nullable sum/AVG state.
    assert_eq!(
        pg10_run(
            vec![count],
            vec![numeric],
            vec![vec![NewStringDatum("1".into())]]
        )[0]
        .GetInt64(),
        1
    );
}

#[test]
fn pg09_10_execution_global_distinct_and_filter() {
    use astersql_types::datum::NewStringDatum;
    let ctx = crate::physical_plan_runtime_test::context_with_dialect(
        astersql_parser_ast::SqlDialect::PostgreSQL,
    );
    let numeric =
        *astersql_expression::types::NewFieldType(astersql_parser_mysql::r#type::TypeVarString);
    let mut count = astersql_expression_aggregation::NewAggFuncDesc(
        ctx.GetExprCtx(),
        "count",
        vec![Box::new(astersql_expression::Column::new(
            numeric.clone(),
            1,
            1,
            0,
        ))],
        true,
    )
    .unwrap();
    let mut sum = count.Clone();
    sum.Name = "pg_sum_numeric".into();
    sum.RetTp = Some(numeric.clone());
    let mut avg = sum.Clone();
    avg.Name = "pg_avg_numeric".into();
    // Shards [1], [1,2], [] feed raw rows to a single complete DISTINCT.
    let rows = ["1", "1", "2"]
        .into_iter()
        .map(|v| vec![NewStringDatum(v.into())])
        .collect();
    let result = pg10_run(
        vec![count.Clone(), sum.Clone(), avg.Clone()],
        vec![numeric.clone()],
        rows,
    );
    assert_eq!(result[0].GetInt64(), 2);
    assert_eq!(result[1].GetString(), "3");
    assert_eq!(result[2].GetString(), "1.5000000000000000");
    let mut p = hash_agg_plan_context(false, ctx);
    for ordered in [false, true] {
        let mut f = avg.Clone();
        if ordered {
            f.HasDistinct = false;
            f.OrderByItems = vec![astersql_planner_util::ByItems {
                Expr: f.Args[0].clone(),
                Desc: true,
            }];
        }
        assert!(!f.CanSplitForPostgreSQL());
        p.BasePhysicalAgg.AggFuncs = vec![f.Clone()];
        assert!(
            astersql_planner_core_operator_physicalop::BuildFinalModeAggregation(
                &p.BasePhysicalAgg,
                false
            )
            .is_err()
        );
        assert!(f.Equal(p.s_ctx().GetExprCtx().GetEvalCtx(), &f.Clone()));
    }
    avg.HasDistinct = false;
    avg.Filter = Some(Box::new(astersql_expression::NewInt64Const(0)));
    p.BasePhysicalAgg.AggFuncs = vec![avg.Clone()];
    let (partial, final_info) =
        astersql_planner_core_operator_physicalop::BuildFinalModeAggregation(
            &p.BasePhysicalAgg,
            false,
        )
        .unwrap();
    assert!(partial.AggFuncs.iter().all(|f| f.Filter.is_some()));
    assert!(final_info.AggFuncs.iter().all(|f| f.Filter.is_none()));
    let state = pg10_run(
        partial.AggFuncs,
        vec![numeric.clone()],
        vec![vec![NewStringDatum("999".into())]],
    );
    assert_eq!(state[0].GetInt64(), 0);
    assert!(state[1].IsNull());
    let fields = partial
        .Schema
        .Columns
        .iter()
        .map(|c| c.RetType.clone().unwrap())
        .collect();
    assert!(pg10_run(final_info.AggFuncs, fields, vec![state])[0].IsNull());
    assert!(pg10_run(vec![avg], vec![numeric], vec![])[0].IsNull());
    // The legacy MySQL distinct descriptor split remains available.
    count.HasDistinct = true;
    let mut mysql = hash_agg_plan(false);
    mysql.BasePhysicalAgg.AggFuncs = vec![count];
    assert!(
        astersql_planner_core_operator_physicalop::BuildFinalModeAggregation(
            &mysql.BasePhysicalAgg,
            false
        )
        .is_ok()
    );
}

#[test]
fn pg09_10_execution_count_and_smallint_sum_states() {
    use astersql_types::datum::NewIntDatum;
    let field = *astersql_expression::types::NewFieldType(TypeLonglong);
    let mut p = hash_agg_plan_context(
        false,
        crate::physical_plan_runtime_test::context_with_dialect(
            astersql_parser_ast::SqlDialect::PostgreSQL,
        ),
    );
    p.BasePhysicalAgg.AggFuncs = ["count", "sum_int"]
        .into_iter()
        .map(|name| {
            astersql_expression_aggregation::NewAggFuncDesc(
                p.s_ctx().GetExprCtx(),
                name,
                vec![Box::new(astersql_expression::Column::new(
                    field.clone(),
                    1,
                    1,
                    0,
                ))],
                false,
            )
            .unwrap()
        })
        .collect();
    let (partial, final_info) =
        astersql_planner_core_operator_physicalop::BuildFinalModeAggregation(
            &p.BasePhysicalAgg,
            false,
        )
        .unwrap();
    assert!(
        final_info
            .AggFuncs
            .iter()
            .all(|f| f.RetTp.as_ref().unwrap().PostgreSQLTypeOid() == Some(20))
    );
    let state_fields = partial
        .Schema
        .Columns
        .iter()
        .map(|c| c.RetType.clone().unwrap())
        .collect::<Vec<_>>();
    for (shards, expected_count, expected_sum) in [
        (
            vec![vec![Some(1)], vec![Some(2), Some(3), Some(4)], vec![]],
            4,
            Some(10),
        ),
        (
            vec![vec![Some(i32::MAX as i64)], vec![Some(i32::MAX as i64)]],
            2,
            Some(4294967294),
        ),
        (vec![vec![], vec![None]], 0, None),
    ] {
        let states = shards
            .into_iter()
            .map(|shard| {
                pg10_run(
                    partial.AggFuncs.clone(),
                    vec![field.clone()],
                    shard
                        .into_iter()
                        .map(|v| vec![v.map_or_else(Default::default, NewIntDatum)])
                        .collect(),
                )
            })
            .collect();
        let actual = pg10_run(final_info.AggFuncs.clone(), state_fields.clone(), states);
        assert_eq!(actual[0].GetInt64(), expected_count);
        assert_eq!(
            (!actual[1].IsNull()).then(|| actual[1].GetInt64()),
            expected_sum
        );
    }
}

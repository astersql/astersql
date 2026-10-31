// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.

// `baseFuncDesc` / 类型推断相关单元测试入口。
//
// 具体断言委托给 `aggregation_aster_unit_test` 中与 Go 契约对齐的共享用例。

use crate::aggregation_aster_unit_test as shared;
use crate::*;

/// 校验聚合描述符 Hash 覆盖 Go 侧全部身份字段。
#[test]
fn TestClone() {
    shared::aggregate_descriptor_hash_tracks_every_go_identity_field();
}

/// 校验常见聚合返回类型推断与 Go 一致。
#[test]
fn TestBaseFunc_InferAggRetType() {
    shared::aggregate_type_inference_matches_go_contract();
}

/// 校验 AVG/SUM 类型推断，以及 AVG Final 合并局部 count/sum。
#[test]
fn TestTypeInfer4AvgSum() {
    shared::aggregate_type_inference_matches_go_contract();
    shared::avg_final_mode_combines_partial_count_and_sum();
}

/// APPROX_PERCENTILE 应隐藏百分位常量求值的底层转换错误，与 Go 的公开错误契约一致。
#[test]
fn approx_percentile_reports_invalid_percentage_argument() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let value = Box::new(expression::Constant::with_type(
        types::NewIntDatum(1),
        *types::NewFieldType(mysql::TypeLonglong),
    ));
    let percentage = Box::new(expression::Constant::with_type(
        types::NewStringDatum("not-a-number".to_owned()),
        *types::NewFieldType(mysql::TypeVarchar),
    ));

    let error = newBaseFuncDesc(&ctx, ast::AggFuncApproxPercentile, vec![value, percentage])
        .err()
        .expect("invalid percentage must fail type inference");

    assert_eq!(
        error.to_string(),
        "APPROX_PERCENTILE: Invalid argument not-a-number"
    );
}

#[test]
fn pg09_19_direct_fraction_survives_aggregate_casts() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let input = Box::new(expression::Constant::with_type(
        types::NewIntDatum(2),
        *types::NewFieldType(mysql::TypeLong),
    ));
    let fraction = Box::new(expression::Constant::with_type(
        types::NewFloat64Datum(0.5),
        *types::NewFieldType(mysql::TypeDouble),
    ));
    let mut desc =
        NewAggFuncDesc(&ctx, "pg_percentile_disc", vec![input, fraction], false).unwrap();
    desc.WrapCastForAggArgs(&ctx);
    assert_eq!(desc.RetTp.as_ref().unwrap().GetType(), mysql::TypeLong);
    assert_eq!(
        desc.Args[1].GetType(ctx.GetEvalCtx()).GetType(),
        mysql::TypeDouble
    );
    assert_eq!(
        desc.Args[1]
            .Eval(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap()
            .GetFloat64(),
        0.5
    );
    assert!(!desc.CanSplitForPostgreSQL());
}

// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 聚合函数与描述符的 Aster 侧单元测试辅助与用例。
//
// 覆盖模式往返 PB、类型推断、下推分类，以及 Avg/Sum/Count/Bit/Concat/MaxMin 等
// 与 Go 行为对齐的输入输出用例；供 `aggregation_test` 与本文件测试共用。

use crate::*;
use expression::Expression as _;
use std::sync::Arc;

struct SupportedPushDownClient;

impl kv::Client for SupportedPushDownClient {
    fn Send(
        &self,
        _ctx: &kv::Context,
        _request: &kv::Request,
        _variables: &dyn std::any::Any,
        _option: &kv::ClientSendOption,
    ) -> Option<Box<dyn kv::Response>> {
        panic!("window pushdown test must not send a request")
    }

    fn IsRequestTypeSupported(&self, _request_type: i64, _sub_type: i64) -> bool {
        true
    }
}

/// 构造整型常量表达式。
fn int_constant(value: i64) -> expression::ExprBox {
    Box::new(expression::Constant::with_type(
        types::NewIntDatum(value),
        *types::NewFieldType(mysql::TypeLonglong),
    ))
}

/// 构造指定列下标的整型列表达式。
fn int_column(index: isize) -> expression::ExprBox {
    Box::new(expression::Column::new(
        *types::NewFieldType(mysql::TypeLonglong),
        index as i64 + 1,
        index as i64 + 1,
        index,
    ))
}

/// 构造指定列下标的 Decimal 列表达式。
fn decimal_column(index: isize) -> expression::ExprBox {
    Box::new(expression::Column::new(
        *types::NewFieldType(mysql::TypeNewDecimal),
        index as i64 + 1,
        index as i64 + 1,
        index,
    ))
}

/// 构造指定列下标的字符串列表达式。
fn string_column(index: isize) -> expression::ExprBox {
    Box::new(expression::Column::new(
        *types::NewFieldType(mysql::TypeVarchar),
        index as i64 + 1,
        index as i64 + 1,
        index,
    ))
}

/// 由整型切片构造一行 chunk::Row。
fn int_row(values: &[i64]) -> chunk::Row {
    chunk::mutrow::MutRowFromDatums(values.iter().copied().map(types::NewIntDatum).collect())
        .ToRow()
        .CopyConstruct()
}

/// 构造单列 MySQL Decimal 行。
fn decimal_row(value: &str) -> chunk::Row {
    chunk::mutrow::MutRowFromDatums(vec![types::NewDecimalDatum(
        types::NewDecFromStringForTest(value),
    )])
    .ToRow()
    .CopyConstruct()
}

/// 构造单列 NULL 行。
fn null_row() -> chunk::Row {
    chunk::mutrow::MutRowFromDatums(vec![types::Datum::default()])
        .ToRow()
        .CopyConstruct()
}

/// 构造 GROUP_CONCAT 用的两列行：可空整型值与分隔符字符串。
fn concat_row(value: Option<i64>, separator: &str) -> chunk::Row {
    chunk::mutrow::MutRowFromDatums(vec![
        value.map_or_else(types::Datum::default, types::NewIntDatum),
        types::NewStringDatum(separator.to_owned()),
    ])
    .ToRow()
    .CopyConstruct()
}

/// 新建空的表达式求值上下文。
fn eval_context() -> Arc<dyn expression::EvalContext> {
    Arc::new(exprstatic::NewEvalContext(Vec::new()))
}

/// 断言 Datum 中的 MySQL Decimal 与期望十进制字符串相等。
fn assert_decimal(datum: &types::Datum, expected: &str) {
    let expected = types::NewDecFromStringForTest(expected);
    assert_eq!(datum.GetMysqlDecimal().Compare(&expected), 0);
}

/// 计算聚合描述符的 Hash64，用于身份字段敏感性测试。
fn agg_hash(desc: &AggFuncDesc) -> u64 {
    let mut hasher = expression::base::NewHashEqualer();
    desc.Hash64(hasher.as_mut());
    hasher.Sum64()
}

/// 校验各 AggFunctionMode 的 ToString 与 tipb 往返，缺省为 Partial1。
#[test]
pub(crate) fn aggregate_modes_round_trip_to_pb() {
    let modes = [
        (CompleteMode, "complete"),
        (FinalMode, "final"),
        (Partial1Mode, "partial1"),
        (Partial2Mode, "partial2"),
        (DedupMode, "deduplicate"),
    ];
    for (mode, text) in modes {
        assert_eq!(mode.ToString(), text);
        assert_eq!(
            PBAggFuncModeToAggFuncMode(Some(AggFunctionModeToPB(mode))),
            mode
        );
    }
    assert_eq!(PBAggFuncModeToAggFuncMode(None), Partial1Mode);
}

/// COUNT/SUM/SUM_INT 返回类型与默认值应符合 Go 契约。
#[test]
pub(crate) fn aggregate_type_inference_matches_go_contract() {
    let ctx = exprstatic::NewExprContext(Vec::new());

    let count = NewAggFuncDesc(&ctx, ast::AggFuncCount, vec![int_constant(7)], false).unwrap();
    let count_type = count.RetTp.as_ref().unwrap();
    assert_eq!(count_type.GetType(), mysql::TypeLonglong);
    assert!(mysql::HasNotNullFlag(count_type.GetFlag()));
    assert_eq!(count.GetDefaultValue().GetInt64(), 0);

    let sum = NewAggFuncDesc(&ctx, ast::AggFuncSum, vec![int_constant(7)], false).unwrap();
    let sum_type = sum.RetTp.as_ref().unwrap();
    assert_eq!(sum_type.GetType(), mysql::TypeNewDecimal);
    assert_eq!(sum_type.GetDecimal(), 0);
    assert!(sum.GetDefaultValue().IsNull());

    let sum_int = NewAggFuncDesc(&ctx, ast::AggFuncSumInt, vec![int_constant(7)], false).unwrap();
    assert_eq!(
        sum_int.RetTp.as_ref().unwrap().GetType(),
        mysql::TypeLonglong
    );
}

/// Split 应将 Complete COUNT 拆成 Partial1 与 Final，且 Final 参数指向给定列下标。
#[test]
pub(crate) fn split_count_builds_owned_partial_and_final_descriptors() {
    let ctx = exprstatic::NewExprContext(Vec::new());
    let mut count = NewAggFuncDesc(&ctx, ast::AggFuncCount, vec![int_constant(1)], false).unwrap();
    count.Mode = CompleteMode;
    let (partial, final_desc) = count.Split(&[3]);

    assert_eq!(partial.Mode, Partial1Mode);
    assert_eq!(final_desc.Mode, FinalMode);
    assert_eq!(final_desc.Args.len(), 1);
    let column = final_desc.Args[0]
        .as_any()
        .downcast_ref::<expression::Column>()
        .unwrap();
    assert_eq!(column.Index, 3);
}

/// NeedCount/NeedValue/NeedFrame 与 BitAnd tipb 映射、SumInt 下推分类对齐 Go。
#[test]
pub(crate) fn pushdown_and_frame_classification_matches_go_lists() {
    assert!(NeedCount(ast::AggFuncAvg));
    assert!(NeedValue(ast::AggFuncGroupConcat));
    assert!(!NeedValue(ast::AggFuncCount));
    assert!(!NeedFrame(ast::WindowFuncRank));
    assert!(NeedFrame(ast::WindowFuncFirstValue));
    assert!(UseDefaultFrame(ast::WindowFuncRowNumber).0);

    let base = baseFuncDesc {
        Name: ast::AggFuncBitAnd.to_owned(),
        Args: Vec::new(),
        RetTp: None,
    };
    assert_eq!(base.GetTiPBExpr(false), tipb::ExprType::AggBitAnd);

    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let sum_int =
        NewAggFuncDesc(&expr_ctx, ast::AggFuncSumInt, vec![int_column(0)], false).unwrap();
    assert!(CheckAggPushDown(
        expr_ctx.GetEvalCtx(),
        &sum_int,
        kv::StoreType::TiFlash,
    ));
    assert!(CheckAggPushDown(
        expr_ctx.GetEvalCtx(),
        &sum_int,
        kv::StoreType::TiKV,
    ));
}

/// 加权输入下 AVG/SUM 及 DISTINCT 变体结果与 Go 用例一致。
#[test]
pub(crate) fn avg_and_sum_match_go_weighted_input_and_distinct_cases() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let statement_ctx = stmtctx::NewStmtCtx();

    let avg_desc = NewAggFuncDesc(&expr_ctx, ast::AggFuncAvg, vec![int_column(0)], false).unwrap();
    let mut avg = avg_desc.GetAggFunc(&expr_ctx);
    let mut avg_state = avg.CreateContext(eval_context());
    assert!(avg.GetResult(&avg_state).IsNull());

    let sum_desc = NewAggFuncDesc(&expr_ctx, ast::AggFuncSum, vec![int_column(0)], false).unwrap();
    let mut sum = sum_desc.GetAggFunc(&expr_ctx);
    let mut sum_state = sum.CreateContext(eval_context());
    assert!(sum.GetResult(&sum_state).IsNull());

    // 值 v 重复 v 次，形成加权输入；末尾再喂入 NULL。
    for value in 1..=100 {
        for _ in 0..value {
            avg.Update(&mut avg_state, &statement_ctx, int_row(&[value]))
                .unwrap();
            sum.Update(&mut sum_state, &statement_ctx, int_row(&[value]))
                .unwrap();
        }
    }
    avg.Update(&mut avg_state, &statement_ctx, null_row())
        .unwrap();
    sum.Update(&mut sum_state, &statement_ctx, null_row())
        .unwrap();
    assert_decimal(&avg.GetResult(&avg_state), "67");
    assert_decimal(&sum.GetResult(&sum_state), "338350");
    assert_decimal(&sum.GetPartialResult(&sum_state)[0], "338350");

    // DISTINCT：每个值只计一次，AVG=50.5，SUM=5050。
    let distinct_avg_desc =
        NewAggFuncDesc(&expr_ctx, ast::AggFuncAvg, vec![int_column(0)], true).unwrap();
    let mut distinct_avg = distinct_avg_desc.GetAggFunc(&expr_ctx);
    let mut distinct_avg_state = distinct_avg.CreateContext(eval_context());
    let distinct_sum_desc =
        NewAggFuncDesc(&expr_ctx, ast::AggFuncSum, vec![int_column(0)], true).unwrap();
    let mut distinct_sum = distinct_sum_desc.GetAggFunc(&expr_ctx);
    let mut distinct_sum_state = distinct_sum.CreateContext(eval_context());
    for value in 1..=100 {
        for _ in 0..value {
            distinct_avg
                .Update(&mut distinct_avg_state, &statement_ctx, int_row(&[value]))
                .unwrap();
            distinct_sum
                .Update(&mut distinct_sum_state, &statement_ctx, int_row(&[value]))
                .unwrap();
        }
    }
    assert_decimal(&distinct_avg.GetResult(&distinct_avg_state), "50.5");
    let avg_partial = distinct_avg.GetPartialResult(&distinct_avg_state);
    assert_eq!(avg_partial[0].GetInt64(), 100);
    assert_decimal(&avg_partial[1], "5050");
    assert_decimal(&distinct_sum.GetResult(&distinct_sum_state), "5050");
}

/// FinalMode AVG 以 (count, sum) 两列部分结果合并，期望均值 67。
#[test]
pub(crate) fn avg_final_mode_combines_partial_count_and_sum() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let statement_ctx = stmtctx::NewStmtCtx();
    let mut desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncAvg,
        vec![int_column(0), int_column(1)],
        false,
    )
    .unwrap();
    desc.Mode = FinalMode;
    let mut avg = desc.GetAggFunc(&expr_ctx);
    let mut state = avg.CreateContext(eval_context());
    // 每行提供局部 count=value 与局部 sum=value^2。
    for value in 1..=100 {
        avg.Update(&mut state, &statement_ctx, int_row(&[value, value * value]))
            .unwrap();
    }
    assert_decimal(&avg.GetResult(&state), "67");
}

/// COUNT 忽略 NULL、DISTINCT 计唯一值，FinalMode 累加部分计数。
#[test]
pub(crate) fn count_matches_go_null_distinct_and_final_mode_cases() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let statement_ctx = stmtctx::NewStmtCtx();
    let count_desc =
        NewAggFuncDesc(&expr_ctx, ast::AggFuncCount, vec![int_column(0)], false).unwrap();
    let mut count = count_desc.GetAggFunc(&expr_ctx);
    let mut count_state = count.CreateContext(eval_context());
    for value in 1..=100 {
        for _ in 0..value {
            count
                .Update(&mut count_state, &statement_ctx, int_row(&[value]))
                .unwrap();
        }
    }
    count
        .Update(&mut count_state, &statement_ctx, null_row())
        .unwrap();
    assert_eq!(count.GetResult(&count_state).GetInt64(), 5050);

    let distinct_desc =
        NewAggFuncDesc(&expr_ctx, ast::AggFuncCount, vec![int_column(0)], true).unwrap();
    let mut distinct = distinct_desc.GetAggFunc(&expr_ctx);
    let mut distinct_state = distinct.CreateContext(eval_context());
    for value in 1..=100 {
        for _ in 0..value {
            distinct
                .Update(&mut distinct_state, &statement_ctx, int_row(&[value]))
                .unwrap();
        }
    }
    assert_eq!(distinct.GetResult(&distinct_state).GetInt64(), 100);

    let mut final_desc = count_desc.Clone();
    final_desc.Mode = FinalMode;
    let mut final_count = final_desc.GetAggFunc(&expr_ctx);
    let mut final_state = final_count.CreateContext(eval_context());
    for value in [7, 11, 13] {
        final_count
            .Update(&mut final_state, &statement_ctx, int_row(&[value]))
            .unwrap();
    }
    assert_eq!(final_count.GetResult(&final_state).GetInt64(), 31);
}

/// BIT_AND/OR/XOR 空集初值、忽略 NULL、Reset 及 Decimal 转整数均与 Go 一致。
#[test]
pub(crate) fn bit_aggregates_preserve_empty_values_nulls_and_reset() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let statement_ctx = stmtctx::NewStmtCtx();
    for (name, empty, expected) in [
        (ast::AggFuncBitAnd, u64::MAX, 0_u64),
        (ast::AggFuncBitOr, 0, 3),
        (ast::AggFuncBitXor, 0, 0),
    ] {
        let desc = NewAggFuncDesc(&expr_ctx, name, vec![int_column(0)], false).unwrap();
        let mut aggregate = desc.GetAggFunc(&expr_ctx);
        let mut state = aggregate.CreateContext(eval_context());
        assert_eq!(aggregate.GetResult(&state).GetUint64(), empty);
        for value in [1, 3, 2] {
            aggregate
                .Update(&mut state, &statement_ctx, int_row(&[value]))
                .unwrap();
        }
        aggregate
            .Update(&mut state, &statement_ctx, null_row())
            .unwrap();
        assert_eq!(aggregate.GetResult(&state).GetUint64(), expected);
        assert_eq!(aggregate.GetPartialResult(&state)[0].GetUint64(), expected);

        aggregate.ResetContext(eval_context(), &mut state);
        assert_eq!(aggregate.GetResult(&state).GetUint64(), empty);

        let decimal_desc = NewAggFuncDesc(&expr_ctx, name, vec![decimal_column(0)], false).unwrap();
        let mut decimal_aggregate = decimal_desc.GetAggFunc(&expr_ctx);
        let mut decimal_state = decimal_aggregate.CreateContext(eval_context());
        assert_eq!(
            decimal_aggregate.GetResult(&decimal_state).GetUint64(),
            empty
        );

        let (decimal_values, decimal_expected): (&[&str], u64) = match name {
            ast::AggFuncBitAnd => (&["1.234", "3.012", "2.12345678"], 0),
            ast::AggFuncBitOr => (&["12.234", "1.012", "15.12345678", "16.00"], 31),
            ast::AggFuncBitXor => (&["1.234", "1.012", "2.12345678"], 2),
            _ => unreachable!(),
        };
        for value in decimal_values {
            decimal_aggregate
                .Update(&mut decimal_state, &statement_ctx, decimal_row(value))
                .unwrap();
        }
        assert_eq!(
            decimal_aggregate.GetResult(&decimal_state).GetUint64(),
            decimal_expected
        );
    }
}

/// GROUP_CONCAT 拼接、跳过 NULL、Reset 保留生命周期分隔符，以及 DISTINCT 去重。
#[test]
pub(crate) fn group_concat_matches_go_separator_null_distinct_and_reset_cases() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let statement_ctx = stmtctx::NewStmtCtx();
    let desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncGroupConcat,
        vec![int_column(0), string_column(1)],
        false,
    )
    .unwrap();
    let mut concat = desc.GetAggFunc(&expr_ctx);
    let mut state = concat.CreateContext(eval_context());
    assert!(concat.GetResult(&state).IsNull());
    concat
        .Update(&mut state, &statement_ctx, concat_row(Some(1), "x"))
        .unwrap();
    concat
        .Update(&mut state, &statement_ctx, concat_row(Some(2), "x"))
        .unwrap();
    concat
        .Update(&mut state, &statement_ctx, concat_row(None, "x"))
        .unwrap();
    assert_eq!(concat.GetResult(&state).GetString(), "1x2");
    assert_eq!(concat.GetPartialResult(&state)[0].GetString(), "1x2");

    concat.ResetContext(eval_context(), &mut state);
    assert!(concat.GetResult(&state).IsNull());
    concat
        .Update(&mut state, &statement_ctx, concat_row(Some(3), "|"))
        .unwrap();
    concat
        .Update(&mut state, &statement_ctx, concat_row(Some(4), "|"))
        .unwrap();
    assert_eq!(concat.GetResult(&state).GetString(), "3x4");

    let distinct_desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncGroupConcat,
        vec![int_column(0), string_column(1)],
        true,
    )
    .unwrap();
    let mut distinct = distinct_desc.GetAggFunc(&expr_ctx);
    let mut distinct_state = distinct.CreateContext(eval_context());
    for _ in 0..2 {
        distinct
            .Update(
                &mut distinct_state,
                &statement_ctx,
                concat_row(Some(1), "x"),
            )
            .unwrap();
    }
    assert_eq!(distinct.GetResult(&distinct_state).GetString(), "1");
}

/// 首个 GROUP_CONCAT 值即使是空字符串，也必须初始化缓冲区并保留后续分隔符。
#[test]
fn group_concat_preserves_separator_after_empty_first_value() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let statement_ctx = stmtctx::NewStmtCtx();
    let desc = NewAggFuncDesc(
        &expr_ctx,
        ast::AggFuncGroupConcat,
        vec![string_column(0), string_column(1)],
        false,
    )
    .unwrap();
    let mut concat = desc.GetAggFunc(&expr_ctx);
    let mut state = concat.CreateContext(eval_context());
    let empty = chunk::mutrow::MutRowFromDatums(vec![
        types::NewStringDatum(String::new()),
        types::NewStringDatum("|".to_owned()),
    ])
    .ToRow()
    .CopyConstruct();
    let value = chunk::mutrow::MutRowFromDatums(vec![
        types::NewStringDatum("x".to_owned()),
        types::NewStringDatum("|".to_owned()),
    ])
    .ToRow()
    .CopyConstruct();

    concat.Update(&mut state, &statement_ctx, empty).unwrap();
    assert!(!concat.GetResult(&state).IsNull());
    assert_eq!(concat.GetResult(&state).GetString(), "");
    concat.Update(&mut state, &statement_ctx, value).unwrap();
    assert_eq!(concat.GetResult(&state).GetString(), "|x");
}

/// Go 的 CanExprsPushDown 会拒绝 TiFlash 不支持计算的 ENUM 类型，即使它能编码为 PB。
#[test]
fn window_pushdown_rejects_enum_for_tiflash() {
    let expression_context = exprstatic::NewExprContext(Vec::new());
    let pushdown_context = expression::NewPushDownContext(
        Arc::new(expression_context),
        Some(Arc::new(SupportedPushDownClient)),
        false,
        None,
        None,
        1024,
    );
    let mut enum_type = *types::NewFieldType(mysql::TypeEnum);
    enum_type.SetElems(vec!["one".to_owned()]);
    let descriptor = WindowFuncDesc {
        baseFuncDesc: baseFuncDesc {
            Name: ast::WindowFuncFirstValue.to_owned(),
            Args: vec![Box::new(expression::Column::new(
                enum_type.clone(),
                1,
                1,
                0,
            ))],
            RetTp: Some(enum_type),
        },
    };
    assert!(!descriptor.CanPushDownToTiFlash(&pushdown_context));
}

/// FIRST_ROW 取首行；MAX/MIN 忽略 NULL 并取极值。
#[test]
pub(crate) fn first_row_max_and_min_match_go_row_order_and_null_cases() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let statement_ctx = stmtctx::NewStmtCtx();

    let first_desc =
        NewAggFuncDesc(&expr_ctx, ast::AggFuncFirstRow, vec![int_column(0)], false).unwrap();
    let mut first = first_desc.GetAggFunc(&expr_ctx);
    let mut first_state = first.CreateContext(eval_context());
    first
        .Update(&mut first_state, &statement_ctx, int_row(&[1]))
        .unwrap();
    first
        .Update(&mut first_state, &statement_ctx, int_row(&[2]))
        .unwrap();
    assert_eq!(first.GetResult(&first_state).GetInt64(), 1);
    assert_eq!(first.GetPartialResult(&first_state)[0].GetInt64(), 1);

    let max_desc = NewAggFuncDesc(&expr_ctx, ast::AggFuncMax, vec![int_column(0)], false).unwrap();
    let min_desc = NewAggFuncDesc(&expr_ctx, ast::AggFuncMin, vec![int_column(0)], false).unwrap();
    let mut max = max_desc.GetAggFunc(&expr_ctx);
    let mut min = min_desc.GetAggFunc(&expr_ctx);
    let mut max_state = max.CreateContext(eval_context());
    let mut min_state = min.CreateContext(eval_context());
    assert!(max.GetResult(&max_state).IsNull());
    assert!(min.GetResult(&min_state).IsNull());
    for row in [int_row(&[2]), int_row(&[3]), int_row(&[1]), null_row()] {
        max.Update(&mut max_state, &statement_ctx, row.clone())
            .unwrap();
        min.Update(&mut min_state, &statement_ctx, row).unwrap();
    }
    assert_eq!(max.GetResult(&max_state).GetInt64(), 3);
    assert_eq!(min.GetResult(&min_state).GetInt64(), 1);
    assert_eq!(min.GetPartialResult(&min_state)[0].GetInt64(), 1);
}

/// 描述符 Hash64 应对 Name/Args/Mode/DISTINCT/RetTp/OrderBy 任一变化敏感。
#[test]
pub(crate) fn aggregate_descriptor_hash_tracks_every_go_identity_field() {
    let expr_ctx = exprstatic::NewExprContext(Vec::new());
    let desc = NewAggFuncDesc(&expr_ctx, ast::AggFuncSum, vec![int_column(0)], false).unwrap();
    let baseline = agg_hash(&desc);
    assert_eq!(baseline, agg_hash(&desc.Clone()));

    let mut changed = desc.Clone();
    changed.HasDistinct = true;
    assert_ne!(baseline, agg_hash(&changed));

    changed = desc.Clone();
    changed.Mode = FinalMode;
    assert_ne!(baseline, agg_hash(&changed));

    changed = desc.Clone();
    changed.Name = "whatever".to_owned();
    assert_ne!(baseline, agg_hash(&changed));

    changed = desc.Clone();
    changed.Args.clear();
    assert_ne!(baseline, agg_hash(&changed));

    changed = desc.Clone();
    changed.RetTp = Some(*types::NewFieldType(mysql::TypeNewDecimal));
    assert_ne!(baseline, agg_hash(&changed));

    changed = desc.Clone();
    changed.OrderByItems = vec![plannerutil::ByItems {
        Expr: int_column(0),
        Desc: true,
    }];
    assert_ne!(baseline, agg_hash(&changed));
}

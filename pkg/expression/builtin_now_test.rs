// Copyright 2026 AsterSQL.
use crate::{BuildContext, Expression};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
fn context(time: &str, location: chrono_tz::Tz) -> exprstatic::ExprContext {
    let fixed = chrono::DateTime::parse_from_rfc3339(time)
        .unwrap()
        .with_timezone(&location);
    let eval = Arc::new(exprstatic::NewEvalContext(vec![
        exprstatic::WithLocation(location),
        exprstatic::WithCurrentTime(Arc::new(move || Ok(fixed))),
    ]));
    exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(eval)])
}
fn now(ctx: &dyn BuildContext, args: Vec<crate::ExprBox>) -> Result<crate::ExprBox, crate::Error> {
    crate::NewFunctionBase(
        ctx,
        "now",
        *crate::types::NewFieldType(crate::mysql::TypeDatetime),
        args,
    )
}
#[test]
fn normal_ddl_mlog_now_precision_and_timezone() {
    let ctx = context("2026-10-01T12:34:56.987654Z", chrono_tz::Asia::Shanghai);
    for precision in 0..=6 {
        let function = now(&ctx, vec![Box::new(crate::NewInt64Const(precision))]).unwrap();
        let (time, null) = function
            .EvalTime(ctx.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert!(!null);
        assert_eq!(time.Hour(), 20);
        assert_eq!(time.Second(), 56);
        assert_eq!(
            time.Microsecond(),
            987654 / 10_i32.pow((6 - precision) as u32) * 10_i32.pow((6 - precision) as u32)
        );
        assert_eq!(time.Fsp(), precision as i32);
        assert_eq!(
            function.GetType(ctx.GetEvalCtx()).GetDecimal(),
            precision as isize
        );
        assert_eq!(
            function.GetType(ctx.GetEvalCtx()).GetFlen(),
            19 + if precision > 0 {
                precision as isize + 1
            } else {
                0
            }
        );
        let scalar = function
            .as_any()
            .downcast_ref::<crate::ScalarFunction>()
            .unwrap();
        assert_eq!(
            scalar.Function.PbCode(),
            tipb::ScalarFuncSig::NowWithArg as i32
        );
    }
    for args in [vec![], vec![Box::new(crate::NewNull()) as crate::ExprBox]] {
        let function = now(&ctx, args).unwrap();
        let (time, null) = function
            .EvalTime(ctx.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert!(!null);
        assert_eq!(time.Microsecond(), 0);
    }
}
#[test]
fn normal_ddl_mlog_now_statement_cache_clone_and_errors() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let eval = Arc::new(exprstatic::NewEvalContext(vec![
        exprstatic::WithCurrentTime(Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            Ok(
                chrono::DateTime::parse_from_rfc3339("2026-10-01T00:00:00.999999Z")
                    .unwrap()
                    .with_timezone(&chrono_tz::UTC),
            )
        })),
    ]));
    let ctx = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(eval)]);
    let function = now(&ctx, vec![]).unwrap();
    let first = function
        .EvalTime(ctx.GetEvalCtx(), crate::chunk::Row::default())
        .unwrap();
    assert_eq!(
        first,
        function
            .CloneExpr()
            .EvalTime(ctx.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let next = context("2026-10-02T00:00:00Z", chrono_tz::UTC);
    assert_eq!(
        function
            .EvalTime(next.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap()
            .0
            .Day(),
        2
    );
    for precision in [-1, 7, 2147483648] {
        assert!(now(&ctx, vec![Box::new(crate::NewInt64Const(precision))]).is_err());
    }
    assert!(
        now(
            &ctx,
            vec![Box::new(crate::NewOne()), Box::new(crate::NewOne())]
        )
        .is_err()
    );
}
#[test]
fn normal_ddl_mlog_now_runtime_precision_and_clock_error() {
    let ctx = context("2026-10-01T00:00:00.987654999Z", chrono_tz::UTC);
    let ty = *crate::types::NewFieldType(crate::mysql::TypeLonglong);
    let column = crate::Column::new(ty.clone(), 1, 101, 0);
    let function = now(&ctx, vec![Box::new(column)]).unwrap();
    let mut input = crate::chunk::NewChunkWithCapacity(vec![ty], 6);
    for fsp in [0, 3, 6, -1, 7, 2147483648] {
        input.AppendInt64(0, fsp);
    }
    for index in 0..3 {
        let (time, null) = function
            .EvalTime(ctx.GetEvalCtx(), input.GetRow(index))
            .unwrap();
        assert!(!null);
        assert_eq!(time.Second(), 0);
        assert_eq!(time.Fsp(), [0, 3, 6][index]);
    }
    for index in 3..6 {
        assert!(
            function
                .EvalTime(ctx.GetEvalCtx(), input.GetRow(index))
                .is_err()
        );
    }
    let eval = Arc::new(exprstatic::NewEvalContext(vec![
        exprstatic::WithCurrentTime(Arc::new(|| {
            Err(exprstatic::contextutil::errors::NewNoStackError(
                "statement clock unavailable",
            ))
        })),
    ]));
    let failed = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(eval)]);
    assert!(
        function
            .EvalTime(failed.GetEvalCtx(), input.GetRow(0))
            .unwrap_err()
            .to_string()
            .contains("statement clock unavailable")
    );
}
#[test]
fn normal_ddl_mlog_now_aliases_and_date_add_chain() {
    let ctx = context("2026-01-31T23:59:59.987654Z", chrono_tz::UTC);
    for name in ["now", "current_timestamp", "localtimestamp", "localtime"] {
        let function = crate::NewFunctionBase(
            &ctx,
            name,
            *crate::types::NewFieldType(crate::mysql::TypeDatetime),
            vec![Box::new(crate::NewInt64Const(6))],
        )
        .unwrap();
        assert_eq!(
            function
                .EvalTime(ctx.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap()
                .0
                .String(),
            "2026-01-31 23:59:59.987654"
        );
    }
    for (name, amount, unit, expected) in [
        ("date_add", 40, "MINUTE", "2026-02-01 00:39:59.987654"),
        ("date_add", 1, "MONTH", "2026-02-28 23:59:59.987654"),
        ("date_sub", 1, "DAY", "2026-01-30 23:59:59.987654"),
    ] {
        let unit = crate::Constant::with_type(
            crate::types::NewStringDatum(unit.to_owned()),
            *crate::types::NewFieldType(crate::mysql::TypeVarString),
        );
        let function = crate::NewFunctionBase(
            &ctx,
            name,
            *crate::types::NewFieldType(crate::mysql::TypeDatetime),
            vec![
                now(&ctx, vec![Box::new(crate::NewInt64Const(6))]).unwrap(),
                Box::new(crate::NewInt64Const(amount)),
                Box::new(unit),
            ],
        )
        .unwrap();
        assert_eq!(
            function
                .EvalTime(ctx.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap()
                .0
                .String(),
            expected
        );
    }
}

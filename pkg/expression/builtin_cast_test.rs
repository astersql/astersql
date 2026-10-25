// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// CAST 表达式标量单元测试入口。
//
// 将 `builtin_cast_kernel` 的公开符号再导出，并经 `#[path]` 挂载
// `builtin_cast_4_aster_unit_test.rs` 中与 Go 对齐的用例集合。

/// 再导出 CAST 内核符号，供本测试包与 `go_parity` 子模块共用。
pub use crate::builtin_cast_kernel::*;

/// 与 Go `builtin_cast_test.go` 对齐的 CAST 语义用例。
#[path = "builtin_cast_4_aster_unit_test.rs"]
mod go_parity;

#[test]
fn pg_core_int4_cast_kernel() {
    for (input, expected) in [
        ("2147483648", PgInt4Error::OutOfRange),
        ("-2147483649", PgInt4Error::OutOfRange),
        ("12x", PgInt4Error::InvalidSyntax),
        ("", PgInt4Error::InvalidSyntax),
    ] {
        assert_eq!(pg_cast_int4(Some(input)), Err(expected));
    }
    for (input, expected) in [
        ("-2147483648", i32::MIN),
        ("2147483647", i32::MAX),
        (" +00042 ", 42),
        ("-0", 0),
    ] {
        assert_eq!(pg_cast_int4(Some(input)), Ok(Some(expected)));
    }
    assert_eq!(pg_cast_int4(None), Ok(None));
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let mysql_ctx = exprstatic::NewExprContext(vec![]);
    for (datum, source_type, expected) in [
        (types::NewIntDatum(2147483648), mysql::TypeLonglong, Err(())),
        (
            types::NewIntDatum(-2147483649),
            mysql::TypeLonglong,
            Err(()),
        ),
        (
            types::NewStringDatum("12x".into()),
            mysql::TypeVarString,
            Err(()),
        ),
        (
            types::NewStringDatum(" +00042 ".into()),
            mysql::TypeVarString,
            Ok((42, false)),
        ),
        (
            types::NewIntDatum(-2147483648),
            mysql::TypeLonglong,
            Ok((-2147483648, false)),
        ),
        (types::Datum::default(), mysql::TypeNull, Ok((0, true))),
    ] {
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            datum,
            *types::NewFieldType(source_type),
        ));
        let expression = crate::formal_registry::BuildCastFunction(
            &pg,
            &source,
            &types::NewFieldType(mysql::TypeLong),
        );
        assert_eq!(
            expression
                .EvalInt(pg.GetEvalCtx(), crate::chunk::Row::default())
                .map_err(|_| ()),
            expected
        );
    }
    let source = Expression::constant(Value::Int(2147483648), FieldType::new(FieldKind::Int));
    assert_eq!(
        BuildCastFunction(source, FieldType::new(FieldKind::Int))
            .eval(&mut CastContext::default())
            .unwrap(),
        Value::Int(2147483648)
    );
    assert_eq!(mysql_ctx.SQLDialect(), crate::ast::SqlDialect::MySQL);
}

#[test]
fn pg_core_int8_cast_kernel() {
    assert_eq!(pg_cast_int8(None), Ok(None));
    for (input, expected) in [
        ("-9223372036854775808", i64::MIN),
        ("9223372036854775807", i64::MAX),
        (" +09007199254740993 ", 9007199254740993),
    ] {
        assert_eq!(pg_cast_int8(Some(input)), Ok(Some(expected)));
    }
    for input in [
        "9223372036854775808",
        "-9223372036854775809",
        "18446744073709551615",
    ] {
        assert_eq!(pg_cast_int8(Some(input)), Err(PgInt4Error::OutOfRange));
    }
    for input in ["", "12x", "1.0", "+ 1", "　1"] {
        assert_eq!(pg_cast_int8(Some(input)), Err(PgInt4Error::InvalidSyntax));
    }
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (datum, expected) in [
        (types::NewIntDatum(i64::MIN), Ok((i64::MIN, false))),
        (types::NewIntDatum(i64::MAX), Ok((i64::MAX, false))),
        (
            types::NewIntDatum(9007199254740993),
            Ok((9007199254740993, false)),
        ),
        (
            types::NewStringDatum(" -9223372036854775808 ".into()),
            Ok((i64::MIN, false)),
        ),
        (
            types::NewStringDatum("9223372036854775807".into()),
            Ok((i64::MAX, false)),
        ),
        (
            types::NewStringDatum("9007199254740993".into()),
            Ok((9007199254740993, false)),
        ),
        (types::NewUintDatum(u64::MAX), Err(())),
        (types::NewStringDatum("9223372036854775808".into()), Err(())),
        (
            types::NewStringDatum("-9223372036854775809".into()),
            Err(()),
        ),
        (
            types::NewStringDatum("18446744073709551615".into()),
            Err(()),
        ),
        (types::NewStringDatum("12x".into()), Err(())),
        (types::Datum::default(), Ok((0, true))),
    ] {
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            datum,
            *types::NewFieldType(mysql::TypeVarString),
        ));
        let cast = crate::formal_registry::BuildCastFunction(
            &pg,
            &source,
            &types::NewFieldType(mysql::TypeLonglong),
        );
        assert_eq!(cast.GetType(pg.GetEvalCtx()).GetType(), mysql::TypeLonglong);
        assert_eq!(
            cast.EvalInt(pg.GetEvalCtx(), crate::chunk::Row::default())
                .map_err(|_| ()),
            expected
        );
    }
    let source = Expression::constant(
        Value::UInt(u64::MAX),
        FieldType::new(FieldKind::Int).with_unsigned(true),
    );
    let mut context = CastContext::default();
    assert_eq!(
        BuildCastFunction(source, FieldType::new(FieldKind::Int))
            .eval(&mut context)
            .unwrap(),
        Value::Int(-1)
    );
    assert_eq!(context.warnings, vec![CastWarning::CastAsSignedOverflow]);
}

#[test]
fn pg_scalar_01_formal_boolean_target() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let context = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let mut target = *types::NewFieldType(mysql::TypeTiny);
    target.AddFlag(mysql::IsBooleanFlag);
    for (datum, source_type, expected) in [
        (types::NewIntDatum(1), mysql::TypeTiny, (1, false)),
        (types::NewIntDatum(0), mysql::TypeTiny, (0, false)),
        (types::Datum::default(), mysql::TypeNull, (0, true)),
    ] {
        // These are canonical booleans carried by integer Datums, not int8 casts.
        let mut source_field_type = *types::NewFieldType(source_type);
        if source_type == mysql::TypeTiny {
            source_field_type.AddFlag(mysql::IsBooleanFlag);
        }
        let source: Box<dyn FormalExpression> =
            Box::new(crate::Constant::with_type(datum, source_field_type));
        let cast = crate::formal_registry::BuildCastFunction(&context, &source, &target);
        assert_ne!(
            cast.GetType(context.GetEvalCtx()).GetFlag() & mysql::IsBooleanFlag,
            0
        );
        assert_eq!(
            cast.EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap(),
            expected
        );
    }
}

#[test]
fn pg_scalar_03_boolean_cast_kernel_and_formal_dispatch() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let mut boolean = *types::NewFieldType(mysql::TypeTiny);
    boolean.AddFlag(mysql::IsBooleanFlag);
    let int4 = *types::NewFieldType(mysql::TypeLong);
    let text = *types::NewFieldType(mysql::TypeVarString);
    let int8 = *types::NewFieldType(mysql::TypeLonglong);
    assert_eq!(
        pg_boolean_cast_kind(&int8, &boolean),
        Err(PgBoolError::Unsupported)
    );
    assert_eq!(
        pg_boolean_cast_kind(&boolean, &int8),
        Err(PgBoolError::Unsupported)
    );
    for (input, expected) in [
        ("true", "1"),
        ("false", "0"),
        ("yes", "1"),
        ("no", "0"),
        ("on", "1"),
        ("off", "0"),
        ("1", "1"),
        ("0", "0"),
        ("tr", "1"),
        ("of", "0"),
        (" \tTrUe\n", "1"),
    ] {
        assert_eq!(
            pg_boolean_cast(Some(input), PgBooleanCast::TextToBool),
            Ok(Some(expected.into()))
        );
    }
    for invalid in ["", "o", "2", "truth", "unknown", "\u{a0}true"] {
        assert_eq!(
            pg_boolean_cast(Some(invalid), PgBooleanCast::TextToBool),
            Err(PgBoolError::InvalidSyntax)
        );
    }
    for (datum, source_type, target, expected) in [
        (types::NewIntDatum(-1), int4.clone(), boolean.clone(), "1"),
        (types::NewIntDatum(2), int4.clone(), boolean.clone(), "1"),
        (types::NewIntDatum(0), int4.clone(), boolean.clone(), "0"),
        (types::NewIntDatum(1), boolean.clone(), int4.clone(), "1"),
        (types::NewIntDatum(0), boolean.clone(), int4.clone(), "0"),
        (types::NewIntDatum(1), boolean.clone(), text.clone(), "true"),
        (
            types::NewIntDatum(0),
            boolean.clone(),
            text.clone(),
            "false",
        ),
        (
            types::NewStringDatum("yes".into()),
            text.clone(),
            boolean.clone(),
            "1",
        ),
    ] {
        let source: Box<dyn FormalExpression> =
            Box::new(crate::Constant::with_type(datum, source_type));
        let expression = crate::formal_registry::BuildCastFunction(&pg, &source, &target);
        let output = expression
            .Eval(pg.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(output.ToString().unwrap(), expected);
    }
    for (source_type, target) in [
        (&int4, &boolean),
        (&boolean, &int4),
        (&boolean, &text),
        (&text, &boolean),
    ] {
        let kind = pg_boolean_cast_kind(source_type, target).unwrap().unwrap();
        assert_eq!(pg_boolean_cast(None, kind), Ok(None));
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            types::Datum::default(),
            source_type.clone(),
        ));
        let expression = crate::formal_registry::BuildCastFunction(&pg, &source, target);
        assert!(
            expression
                .Eval(pg.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap()
                .IsNull()
        );
    }
}

#[test]
fn pg_scalar_03_formal_rejects_int8_boolean_cast_even_for_null() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let mut boolean = *types::NewFieldType(mysql::TypeTiny);
    boolean.AddFlag(mysql::IsBooleanFlag);
    for datum in [types::NewIntDatum(1), types::Datum::default()] {
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            datum,
            *types::NewFieldType(mysql::TypeLonglong),
        ));
        let cast = crate::formal_registry::BuildCastFunction(&pg, &source, &boolean);
        assert!(
            cast.Eval(pg.GetEvalCtx(), crate::chunk::Row::default())
                .is_err()
        );
    }
}

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

#[test]
fn pg_scalar_05_int2_kernel_and_formal_cast() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, pg_cast_int2, types};
    assert_eq!(pg_cast_int2(None), Ok(None));
    for (text, expected) in [("-32768", i16::MIN), ("32767", i16::MAX), ("0", 0)] {
        assert_eq!(pg_cast_int2(Some(text)), Ok(Some(expected)));
    }
    for text in [
        "-32769",
        "32768",
        "9223372036854775807",
        "18446744073709551615",
    ] {
        assert_eq!(pg_cast_int2(Some(text)), Err(PgInt4Error::OutOfRange));
    }
    assert_eq!(pg_cast_int2(Some("1x")), Err(PgInt4Error::InvalidSyntax));
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (datum, source_type, expected) in [
        (
            types::NewIntDatum(-32768),
            mysql::TypeLong,
            Ok((-32768, false)),
        ),
        (
            types::NewIntDatum(32767),
            mysql::TypeLonglong,
            Ok((32767, false)),
        ),
        (
            types::NewStringDatum("32767".into()),
            mysql::TypeVarString,
            Ok((32767, false)),
        ),
        (types::NewIntDatum(-32769), mysql::TypeLong, Err(())),
        (types::NewIntDatum(32768), mysql::TypeLonglong, Err(())),
        (types::NewUintDatum(u64::MAX), mysql::TypeLonglong, Err(())),
        (types::Datum::default(), mysql::TypeNull, Ok((0, true))),
    ] {
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            datum,
            *types::NewFieldType(source_type),
        ));
        let cast = crate::formal_registry::BuildCastFunction(
            &pg,
            &source,
            &types::NewFieldType(mysql::TypeShort),
        );
        assert_eq!(cast.GetType(pg.GetEvalCtx()).GetType(), mysql::TypeShort);
        assert_eq!(
            cast.EvalInt(pg.GetEvalCtx(), crate::chunk::Row::default())
                .map_err(|_| ()),
            expected
        );
    }
}

#[test]
fn pg_integer_text_grammar() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    assert_eq!(pg_cast_int2(Some("-0x8000")), Ok(Some(i16::MIN)));
    assert_eq!(pg_cast_int2(Some("0x7fff")), Ok(Some(i16::MAX)));
    for target in [mysql::TypeShort, mysql::TypeLong, mysql::TypeLonglong] {
        for (input, expected) in [
            ("0x2a", Some(42)),
            ("0o_52", Some(42)),
            ("-0B10_1010", Some(-42)),
            ("+0X_2_a", Some(42)),
            ("0_042", Some(42)),
            (" \x0b\x0c+42\r\n", Some(42)),
            ("1__0", None),
            ("_1", None),
            ("0x_", None),
            ("0x__1", None),
            ("0b2", None),
            ("0o8", None),
            ("1x", None),
            ("1\u{a0}", None),
        ] {
            let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
                types::NewStringDatum(input.into()),
                *types::NewFieldType(mysql::TypeVarString),
            ));
            let expression = crate::formal_registry::BuildCastFunction(
                &pg,
                &source,
                &types::NewFieldType(target),
            );
            let actual = expression.EvalInt(pg.GetEvalCtx(), crate::chunk::Row::default());
            if let Some(expected) = expected {
                assert_eq!(actual.unwrap(), (expected, false), "{target} {input}");
            } else {
                assert!(actual.is_err(), "{target} {input}: {actual:?}");
            }
        }
    }
    assert_eq!(pg_cast_int4(Some("-0x8000_0000")), Ok(Some(i32::MIN)));
    assert_eq!(
        pg_cast_int8(Some("-0x8000_0000_0000_0000")),
        Ok(Some(i64::MIN))
    );
    assert_eq!(
        pg_cast_int8(Some("0x7fff_ffff_ffff_ffff")),
        Ok(Some(i64::MAX))
    );
    // PG checks accumulation before later junk, then checks signed endpoints
    // after syntax. The threshold must use the target width, not always int8.
    for (input, expected) in [
        ("32768x", PgInt4Error::InvalidSyntax),
        ("327680x", PgInt4Error::OutOfRange),
        ("0x8000x", PgInt4Error::InvalidSyntax),
        ("0x80000x", PgInt4Error::OutOfRange),
    ] {
        assert_eq!(pg_cast_int2(Some(input)), Err(expected), "{input}");
    }
}

#[test]
fn pg_finite_numeric_cast_factory() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let target = types::NewFieldType(mysql::TypeNewDecimal);
    for value in [
        Some("12.340".to_string()),
        Some("9007199254740993".into()),
        Some("9".repeat(82)),
        Some("9".repeat(131072)),
        Some(format!("{}.{}", "9".repeat(131072), "9".repeat(16383))),
        Some(format!("0.{}1", "0".repeat(16382))),
        None,
    ] {
        let datum = value
            .as_ref()
            .map(|v| types::NewStringDatum(v.clone()))
            .unwrap_or_default();
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            datum,
            *types::NewFieldType(mysql::TypeVarString),
        ));
        let expression = crate::formal_registry::BuildCastFunction(&pg, &source, &target);
        assert_eq!(
            expression.GetType(pg.GetEvalCtx()).GetType(),
            mysql::TypeNewDecimal
        );
        let result = expression
            .Eval(pg.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(result.IsNull(), value.is_none());
        if let Some(expected) = value {
            assert_eq!(result.ToString().unwrap(), expected);
        }
    }
}

#[test]
fn pg_numeric_modifier_cast_factory() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (input, p, s, expected) in [
        (Some("1.25"), 3, 1, Some("1.3")),
        (Some("-1250"), 2, -2, Some("-1300")),
        (Some("NaN"), 3, 1, Some("NaN")),
        (Some("Infinity"), -1, -1, Some("Infinity")),
        (Some("-inf"), -1, -1, Some("-Infinity")),
        (None, 3, -1, None),
    ] {
        let mut target = types::NewFieldType(mysql::TypeNewDecimal);
        target.SetFlen(p);
        target.SetDecimal(s);
        let datum = input
            .map(|v| types::NewStringDatum(v.into()))
            .unwrap_or_default();
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            datum,
            *types::NewFieldType(mysql::TypeVarString),
        ));
        let expression = crate::formal_registry::BuildCastFunction(&pg, &source, &target);
        let result = expression
            .Eval(pg.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(result.IsNull(), expected.is_none());
        if let Some(value) = expected {
            assert_eq!(result.ToString().unwrap(), value);
        }
    }
}

#[test]
fn pg_scalar_11_formal_numeric_integer() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for target in [mysql::TypeShort, mysql::TypeLong, mysql::TypeLonglong] {
        for (input, expected) in [
            ("1.5", Some(2)),
            ("2.5", Some(3)),
            ("-1.5", Some(-2)),
            ("-2.5", Some(-3)),
            ("NaN", None),
            ("Infinity", None),
            ("-Infinity", None),
        ] {
            let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
                types::NewStringDatum(input.into()),
                *types::NewFieldType(mysql::TypeNewDecimal),
            ));
            let cast = crate::formal_registry::BuildCastFunction(
                &pg,
                &source,
                &types::NewFieldType(target),
            );
            let actual = cast.EvalInt(pg.GetEvalCtx(), crate::chunk::Row::default());
            match expected {
                Some(v) => assert_eq!(actual.unwrap(), (v, false)),
                None => assert!(actual.is_err()),
            }
        }
    }
}

#[test]
fn pg_scalar_16_binary32_kernel_and_formal_cast() {
    use crate::{
        BuildContext, Expression as FormalExpression, PgFloat4Error, mysql, pg_cast_float4, types,
    };
    for input in ["1e39", "-1e39", "1e-100", "-1e-100"] {
        assert_eq!(pg_cast_float4(Some(input)), Err(PgFloat4Error::OutOfRange));
    }
    for input in ["", " ", "1x", "1e", "0x1p0", "1_0", "1\u{a0}"] {
        assert_eq!(
            pg_cast_float4(Some(input)),
            Err(PgFloat4Error::InvalidSyntax)
        );
    }
    assert_eq!(pg_cast_float4(None), Ok(None));
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for (input, expected) in [
        ("7.038531e-26", f32::from_bits(0x15ae43fd)),
        ("-0", -0.0),
        ("1.401298464324817e-45", f32::from_bits(1)),
        ("Infinity", f32::INFINITY),
        ("NaN", f32::NAN),
    ] {
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            types::NewStringDatum(input.into()),
            *types::NewFieldType(mysql::TypeVarString),
        ));
        let cast = crate::formal_registry::BuildCastFunction(
            &pg,
            &source,
            &types::NewFieldType(mysql::TypeFloat),
        );
        let (actual, null) = cast
            .EvalReal(pg.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert!(!null);
        if expected.is_nan() {
            assert!(actual.is_nan());
        } else {
            assert_eq!((actual as f32).to_bits(), expected.to_bits(), "{input}");
        }
    }
    for datum in [
        types::NewStringDatum("1e39".into()),
        types::NewStringDatum("1x".into()),
        types::Datum::default(),
    ] {
        let null = datum.IsNull();
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            datum,
            *types::NewFieldType(mysql::TypeVarString),
        ));
        let cast = crate::formal_registry::BuildCastFunction(
            &pg,
            &source,
            &types::NewFieldType(mysql::TypeFloat),
        );
        assert_eq!(cast.GetType(pg.GetEvalCtx()).GetType(), mysql::TypeFloat);
        let result = cast.EvalReal(pg.GetEvalCtx(), crate::chunk::Row::default());
        if null {
            assert!(result.unwrap().1);
        } else {
            assert!(result.is_err());
        }
    }
}

#[test]
fn pg_scalar_18_invalid_precision_state() {
    assert_eq!(crate::PgFloat4Error::InvalidPrecision.sqlstate(), "22023");
}

#[test]
fn pg_scalar_19_float_integer_formal() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for source_type in [mysql::TypeFloat, mysql::TypeDouble] {
        for target in [mysql::TypeShort, mysql::TypeLong, mysql::TypeLonglong] {
            for (value, expected) in [
                (2.5, Some(2)),
                (-2.5, Some(-2)),
                (3.5, Some(4)),
                (f64::NAN, None),
                (f64::INFINITY, None),
            ] {
                let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
                    if source_type == mysql::TypeFloat {
                        types::NewFloat32Datum(value as f32)
                    } else {
                        types::NewFloat64Datum(value)
                    },
                    *types::NewFieldType(source_type),
                ));
                let cast = crate::formal_registry::BuildCastFunction(
                    &pg,
                    &source,
                    &types::NewFieldType(target),
                );
                let actual = cast.EvalInt(pg.GetEvalCtx(), crate::chunk::Row::default());
                match expected {
                    Some(expected) => assert_eq!(actual.unwrap(), (expected, false)),
                    None => assert!(actual.is_err()),
                }
            }
        }
    }
    for (value, expected) in [
        (1.0000000596046448, Some(1.0)),
        (1e39, None),
        (1e-100, None),
        (f64::INFINITY, Some(f64::INFINITY)),
    ] {
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            types::NewFloat64Datum(value),
            *types::NewFieldType(mysql::TypeDouble),
        ));
        let cast = crate::formal_registry::BuildCastFunction(
            &pg,
            &source,
            &types::NewFieldType(mysql::TypeFloat),
        );
        let actual = cast.EvalReal(pg.GetEvalCtx(), crate::chunk::Row::default());
        match expected {
            Some(expected) => assert_eq!(actual.unwrap(), (expected, false)),
            None => assert!(actual.is_err()),
        }
    }
    assert_eq!(pg_narrow_float4(1.0000000596046448), Ok(1.0));
    assert_eq!(pg_narrow_float4(1e39), Err(PgFloat4Error::OutOfRange));
    assert_eq!(pg_narrow_float4(1e-100), Err(PgFloat4Error::OutOfRange));
    for bits in [16, 32, 64] {
        let limit = 2_f64.powi(bits - 1);
        assert_eq!(
            pg_float_to_integer(-limit, bits as u32),
            Ok((-limit) as i64)
        );
        assert_eq!(
            pg_float_to_integer(limit, bits as u32),
            Err(PgInt4Error::OutOfRange)
        );
    }
}

#[test]
fn pg_scalar_20_float8_text() {
    use crate::{PgFloat8Error, pg_cast_float8};
    assert_eq!(pg_cast_float8(None), Ok(None));
    assert_eq!(
        pg_cast_float8(Some("-0")).unwrap().unwrap().to_bits(),
        (-0.0_f64).to_bits()
    );
    assert_eq!(
        pg_cast_float8(Some("4.9406564584124654e-324"))
            .unwrap()
            .unwrap()
            .to_bits(),
        1
    );
    for input in ["bad", ""] {
        assert_eq!(
            pg_cast_float8(Some(input)),
            Err(PgFloat8Error::InvalidSyntax)
        );
    }
    for input in ["1e9999", "1e-9999"] {
        assert_eq!(pg_cast_float8(Some(input)), Err(PgFloat8Error::OutOfRange));
    }
}

#[test]
fn pg_scalar_20_formal_float8_cast() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for input in [
        "-0",
        "0.1",
        "4.9406564584124654e-324",
        "bad",
        "1e9999",
        "1e-9999",
    ] {
        let source: Box<dyn FormalExpression> = Box::new(crate::Constant::with_type(
            types::NewStringDatum(input.into()),
            *types::NewFieldType(mysql::TypeVarString),
        ));
        let cast = crate::formal_registry::BuildCastFunction(
            &pg,
            &source,
            &types::NewFieldType(mysql::TypeDouble),
        );
        let actual = cast.EvalReal(pg.GetEvalCtx(), crate::chunk::Row::default());
        match crate::pg_cast_float8(Some(input)) {
            Ok(Some(expected)) => {
                let (value, null) = actual.unwrap();
                assert!(!null);
                assert_eq!(value.to_bits(), expected.to_bits());
            }
            Err(_) => assert!(actual.is_err(), "{input}"),
            _ => unreachable!(),
        }
    }
}

#[test]
fn pg_scalar_21_formal_text_identity() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for oid in [25, 1042, 1043] {
        let mut target = *types::NewFieldType(mysql::TypeVarString);
        target.SetCharset("utf8mb4".into());
        target.SetCollate("utf8mb4_bin".into());
        target.SetPostgreSQLTextOid(oid);
        for value in [Some(""), Some("e\u{301}é😀"), None] {
            let datum = value
                .map(|v| types::NewStringDatum(v.into()))
                .unwrap_or_default();
            let mut source_type = *types::NewFieldType(mysql::TypeVarString);
            source_type.SetCharset("utf8mb4".into());
            source_type.SetCollate("utf8mb4_bin".into());
            let source: Box<dyn FormalExpression> =
                Box::new(crate::Constant::with_type(datum, source_type));
            let cast = crate::formal_registry::BuildCastFunction(&pg, &source, &target);
            assert_eq!(cast.GetType(pg.GetEvalCtx()).PostgreSQLTextOid(), Some(oid));
            let actual = cast
                .Eval(pg.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap();
            match value {
                Some(value) => assert_eq!(actual.ToString().unwrap().as_bytes(), value.as_bytes()),
                None => assert!(actual.IsNull()),
            }
        }
    }
}

#[test]
fn pg_scalar_22_formal_varchar() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let mut target = *types::NewFieldType(mysql::TypeVarString);
    target.SetCharset("utf8mb4".into());
    target.SetCollate("utf8mb4_bin".into());
    target.SetPostgreSQLTextOid(1043);
    target.SetFlen(2);
    for (input, expected) in [
        (Some("é😀x"), Some("é😀")),
        (Some("e\u{301}x"), Some("e\u{301}")),
        (Some("a  "), Some("a ")),
        (None, None),
    ] {
        let datum = input
            .map(|v| types::NewStringDatum(v.into()))
            .unwrap_or_default();
        let source: Box<dyn FormalExpression> =
            Box::new(crate::Constant::with_type(datum, target.clone()));
        let cast = crate::formal_registry::BuildCastFunction(&pg, &source, &target);
        let actual = cast
            .Eval(pg.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(actual.IsNull(), expected.is_none());
        if let Some(expected) = expected {
            assert_eq!(actual.ToString().unwrap(), expected);
        }
    }
}

#[test]
fn pg_scalar_23_formal_bpchar() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    let mut target = *types::NewFieldType(mysql::TypeVarString);
    target.SetCharset("utf8mb4".into());
    target.SetCollate("utf8mb4_bin".into());
    target.SetPostgreSQLTextOid(1042);
    target.SetFlen(2);
    for (input, expected) in [
        (Some("é😀x"), Some("é😀")),
        (Some("e\u{301}x"), Some("e\u{301}")),
        (Some("a  "), Some("a ")),
        (Some("é"), Some("é ")),
        (Some(""), Some("  ")),
        (None, None),
    ] {
        let datum = input
            .map(|v| types::NewStringDatum(v.into()))
            .unwrap_or_default();
        let source: Box<dyn FormalExpression> =
            Box::new(crate::Constant::with_type(datum, target.clone()));
        let cast = crate::formal_registry::BuildCastFunction(&pg, &source, &target);
        let actual = cast
            .Eval(pg.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap();
        assert_eq!(actual.IsNull(), expected.is_none());
        if let Some(expected) = expected {
            assert_eq!(actual.ToString().unwrap(), expected);
        }
    }
}

#[test]
fn pg_scalar_23_formal_bpchar_to_text() {
    use crate::{BuildContext, Expression as FormalExpression, mysql, types};
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
            crate::ast::SqlDialect::PostgreSQL,
        )]),
    ))]);
    for source_oid in [1042, 25, 1043] {
        for target_oid in [25, 1043] {
            let mut source_type = *types::NewFieldType(mysql::TypeVarString);
            source_type.SetCharset("utf8mb4".into());
            source_type.SetCollate("utf8mb4_bin".into());
            source_type.SetPostgreSQLTextOid(source_oid);
            source_type.SetFlen(-1);
            let mut target = source_type.clone();
            target.SetPostgreSQLTextOid(target_oid);
            for input in [Some("é  "), Some("a　 "), None] {
                let datum = input
                    .map(|v| types::NewStringDatum(v.into()))
                    .unwrap_or_default();
                let source: Box<dyn FormalExpression> =
                    Box::new(crate::Constant::with_type(datum, source_type.clone()));
                let cast = crate::formal_registry::BuildCastFunction(&pg, &source, &target);
                let actual = cast
                    .Eval(pg.GetEvalCtx(), crate::chunk::Row::default())
                    .unwrap();
                assert_eq!(actual.IsNull(), input.is_none());
                if let Some(input) = input {
                    assert_eq!(
                        actual.ToString().unwrap(),
                        if source_oid == 1042 {
                            input.trim_end_matches(' ')
                        } else {
                            input
                        }
                    );
                }
            }
        }
    }
}

#[test]
fn pg05_array_text_input_delegates_real_int4_scalar_codec() {
    use types_dependency::pg_array::{PgArray, PgArrayElement::Int4, PgArrayTextError};
    let parse = |bytes: &[u8]| {
        PgArray::from_text_with(bytes, 23, -1, b',', |s, _| {
            pg_cast_int4(Some(s)).map(|n| Int4(n.expect("non-null input")))
        })
    };
    let a = parse(b"[0:1][-2:-1]={{1,2},{3,4}}").unwrap();
    assert_eq!(a.element_oid(), 23);
    assert_eq!(a.shape(), [2, 2]);
    assert_eq!(a.lower_bounds(), [0, -2]);
    assert_eq!(
        a.elements(),
        [Some(Int4(1)), Some(Int4(2)), Some(Int4(3)), Some(Int4(4))]
    );
    let a = parse(br#"{-0x8000_0000,0b10,2_147_483_647,NULL,null," +42 "}"#).unwrap();
    assert_eq!(
        a.elements(),
        [
            Some(Int4(i32::MIN)),
            Some(Int4(2)),
            Some(Int4(i32::MAX)),
            None,
            None,
            Some(Int4(42))
        ]
    );
    for (input, state) in [
        ("{x}", "22P02"),
        ("{2147483648}", "22003"),
        ("{1,-2147483649,2}", "22003"),
        ("{\"NULL\"}", "22P02"),
        (r"{N\ULL}", "22P02"),
        ("{\"\"}", "22P02"),
    ] {
        let PgArrayTextError::Element(error) = parse(input.as_bytes()).unwrap_err() else {
            panic!("expected scalar input error for {input}")
        };
        assert_eq!(error.sqlstate(), state);
        assert!(parse(b"{1,NULL}").is_ok());
    }
    assert_eq!(parse(b"{{x,2},{3}}").unwrap_err().sqlstate(), Some("22P02"));
}

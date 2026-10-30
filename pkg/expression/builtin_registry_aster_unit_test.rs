// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// 内建函数注册表与工厂构建的 Go 对齐单元测试。
//
// 校验完整函数表规模、参数元数规则、展示名过滤、TiFlash/TiPB 签名，
// 以及 REGEXP/字符串/控制流/LIKE 等工厂在最小/最大 arity 下的求值语义。

use crate::formal_registry::{
    GetBuiltinList, GetDisplayName, IsFunctionSupported, VerifyArgsWrapper, funcs,
};
use std::any::Any;

struct PushdownCapabilityClient;

impl crate::kv::Client for PushdownCapabilityClient {
    fn Send(
        &self,
        _ctx: &crate::kv::Context,
        _req: &crate::kv::Request,
        _vars: &dyn Any,
        _option: &crate::kv::ClientSendOption,
    ) -> Option<Box<dyn crate::kv::Response>> {
        panic!("pushdown capability test never sends requests")
    }

    fn IsRequestTypeSupported(&self, _request_type: i64, _sub_type: i64) -> bool {
        true
    }
}

#[test]
/// 验证注册表规模与解析器运算符别名均已注册。
fn builtin_registry_matches_complete_go_function_table() {
    const GO_BUILTIN_COUNT: usize = 310;
    const SQL_OPERATOR_ALIASES: &[&str] = &[
        "=",
        "!=",
        "<>",
        "<",
        "<=",
        ">",
        ">=",
        "+",
        "-",
        "*",
        "/",
        "%",
        "||",
        "is distinct from",
        "is not distinct from",
    ];
    // Keep the Go inventory separate from the bounded PostgreSQL extensions.
    const PG_EXTENSIONS: &[&str] = &[
        "date_trunc",
        "date_part",
        "to_char",
        "array_length",
        "cardinality",
        "array_append",
        "int4range",
        "numrange",
        "isempty",
        "current_database",
        "current_schema",
        "pg_typeof",
        "pg_backend_pid",
    ];
    assert_eq!(
        funcs.len(),
        GO_BUILTIN_COUNT + SQL_OPERATOR_ALIASES.len() + PG_EXTENSIONS.len()
    );
    for name in PG_EXTENSIONS {
        assert!(
            IsFunctionSupported(name),
            "missing PostgreSQL extension {name}"
        );
    }
    for alias in SQL_OPERATOR_ALIASES {
        assert!(
            IsFunctionSupported(alias),
            "missing parser operator alias {alias}"
        );
    }
    assert!(IsFunctionSupported("coalesce"));
    assert!(IsFunctionSupported("tidb_decode_sql_digests"));
    assert!(IsFunctionSupported("vec_cosine_distance"));
    assert!(IsFunctionSupported("embed_text"));
    assert!(IsFunctionSupported("fts_match_word"));
    assert!(!IsFunctionSupported("definitely_not_a_builtin"));
}

#[test]
/// 验证参数个数上下界（含不定长、可选参数）对齐 Go。
fn builtin_argument_ranges_match_go_unbounded_and_optional_rules() {
    assert!(VerifyArgsWrapper("pi", 0).is_ok());
    assert!(VerifyArgsWrapper("pi", 1).is_err());

    assert!(VerifyArgsWrapper("log", 1).is_ok());
    assert!(VerifyArgsWrapper("log", 2).is_ok());
    assert!(VerifyArgsWrapper("log", 3).is_err());

    assert!(VerifyArgsWrapper("concat", 1).is_ok());
    assert!(VerifyArgsWrapper("concat", 128).is_ok());
    assert!(VerifyArgsWrapper("concat", 0).is_err());

    // Go wrapper assumes the caller already checked support and therefore
    // deliberately returns nil for a missing class.
    assert!(VerifyArgsWrapper("not_registered", 999).is_ok());
}

#[test]
/// 验证 GetBuiltinList 过滤与运算符展示名映射。
fn builtin_listing_and_operator_display_follow_go_filters() {
    assert_eq!(GetDisplayName("nulleq"), "<=>");
    assert_eq!(GetDisplayName("intdiv"), "DIV");
    assert_eq!(GetDisplayName("json_extract"), "json_extract");

    let names = GetBuiltinList();
    assert!(names.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(names.contains(&"json_extract".to_owned()));
    assert!(!names.contains(&"row".to_owned()));
    assert!(!names.contains(&"istrue_with_null".to_owned()));
    assert!(!names.iter().any(|name| name.starts_with("'tidb`.(")));
}

#[test]
/// 验证 TiFlash 相关标量工厂产出与 Go 一致的 TiPB 签名。
fn tiflash_scalar_factories_build_the_go_pb_signatures() {
    let context = exprstatic::NewExprContext(Vec::new());
    let string_type = || {
        let mut field_type = *crate::types::NewFieldType(crate::mysql::TypeVarchar);
        field_type.SetCharset("utf8mb4".to_owned());
        field_type.SetCollate("utf8mb4_bin".to_owned());
        field_type
    };
    let column = |id: i64, field_type: crate::types::FieldType| {
        Box::new(crate::Column::new(
            field_type,
            id,
            id + 100,
            id as isize - 1,
        )) as crate::ExprBox
    };
    let integer = || *crate::types::NewFieldType(crate::mysql::TypeLonglong);
    let duration = || *crate::types::NewFieldType(crate::mysql::TypeDuration);

    let cases: Vec<(&str, Vec<crate::ExprBox>, tipb::ScalarFuncSig)> = vec![
        (
            crate::ast::functions::TimeToSec,
            vec![column(1, duration())],
            tipb::ScalarFuncSig::TimeToSec,
        ),
        (
            crate::ast::functions::IsIPv4,
            vec![column(1, string_type())],
            tipb::ScalarFuncSig::IsIPv4,
        ),
        (
            crate::ast::functions::IsIPv6,
            vec![column(1, string_type())],
            tipb::ScalarFuncSig::IsIPv6,
        ),
        (
            crate::ast::functions::RegexpInStr,
            vec![
                column(1, string_type()),
                column(2, string_type()),
                column(3, integer()),
                column(4, integer()),
                column(5, integer()),
                column(6, string_type()),
            ],
            tipb::ScalarFuncSig::RegexpInStrUtf8Sig,
        ),
        (
            crate::ast::functions::RegexpSubstr,
            vec![
                column(1, string_type()),
                column(2, string_type()),
                column(3, integer()),
                column(4, integer()),
                column(5, string_type()),
            ],
            tipb::ScalarFuncSig::RegexpSubstrUtf8Sig,
        ),
        (
            crate::ast::functions::RegexpReplace,
            vec![
                column(1, string_type()),
                column(2, string_type()),
                column(3, string_type()),
                column(4, integer()),
                column(5, integer()),
                column(6, string_type()),
            ],
            tipb::ScalarFuncSig::RegexpReplaceUtf8Sig,
        ),
    ];

    for (name, arguments, expected_signature) in cases {
        let expression = crate::NewFunctionBase(
            &context,
            name,
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            arguments,
        )
        .unwrap_or_else(|error| panic!("build {name}: {error}"));
        let scalar = expression
            .as_any()
            .downcast_ref::<crate::ScalarFunction>()
            .unwrap_or_else(|| panic!("{name} must remain a scalar function"));
        assert_eq!(
            scalar.Function.PbCode(),
            expected_signature as i32,
            "wrong TiPB signature for {name}"
        );
    }
}

#[test]
fn comparison_and_logic_factories_install_the_go_pb_signatures() {
    let context = exprstatic::NewExprContext(Vec::new());
    let cases = [
        (crate::ast::EQ, tipb::ScalarFuncSig::EqInt),
        (crate::ast::GE, tipb::ScalarFuncSig::GeInt),
        (crate::ast::LT, tipb::ScalarFuncSig::LtInt),
        (crate::ast::LogicAnd, tipb::ScalarFuncSig::LogicalAnd),
    ];
    for (name, expected) in cases {
        let expression = crate::NewFunctionBase(
            &context,
            name,
            *crate::types::NewFieldType(crate::mysql::TypeTiny),
            vec![
                Box::new(crate::NewInt64Const(1)),
                Box::new(crate::NewInt64Const(2)),
            ],
        )
        .unwrap_or_else(|error| panic!("build {name}: {error}"));
        let scalar = expression
            .as_any()
            .downcast_ref::<crate::ScalarFunction>()
            .unwrap_or_else(|| panic!("{name} must remain a scalar function"));
        assert_eq!(scalar.Function.PbCode(), expected as i32, "{name}");
        let encoded = crate::NewPBConverter(&PushdownCapabilityClient, context.GetEvalCtx())
            .ExprToPB(scalar)
            .unwrap_or_else(|| panic!("{name} must encode to TiPB"));
        assert_eq!(encoded.get_sig(), expected, "{name}");
    }
}

#[test]
fn not_is_null_text_column_encodes_recursively_for_tikv() {
    let mut context = exprstatic::NewExprContext(Vec::new());
    let column = || {
        Box::new(crate::Column::new(
            *crate::types::NewFieldType(crate::mysql::TypeBlob),
            1,
            101,
            0,
        )) as crate::ExprBox
    };
    let predicate = crate::BuildNotNullExpr(&mut context, column());
    let not = predicate
        .as_any()
        .downcast_ref::<crate::ScalarFunction>()
        .expect("IS NOT NULL must keep its outer NOT scalar function");
    assert_eq!(
        not.Function.PbCode(),
        tipb::ScalarFuncSig::UnaryNotInt as i32
    );
    let is_null = not.GetArgs()[0]
        .as_any()
        .downcast_ref::<crate::ScalarFunction>()
        .expect("NOT must wrap an IS NULL scalar function");
    assert_eq!(
        is_null.Function.PbCode(),
        tipb::ScalarFuncSig::StringIsNull as i32
    );

    let encoded = crate::NewPBConverter(&PushdownCapabilityClient, context.GetEvalCtx())
        .ExprToPB(predicate.as_ref())
        .expect("NOT(IS NULL(text column)) must encode to TiPB");
    assert_eq!(encoded.get_sig(), tipb::ScalarFuncSig::UnaryNotInt);
    assert_eq!(
        encoded.get_children()[0].get_sig(),
        tipb::ScalarFuncSig::StringIsNull
    );

    let plus = crate::NewFunctionBase(
        &context,
        crate::ast::Plus,
        *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
        vec![column(), Box::new(crate::NewInt64Const(1))],
    )
    .expect("build text-column arithmetic");
    let predicate = crate::BuildNotNullExpr(&mut context, plus);
    let encoded = crate::NewPBConverter(&PushdownCapabilityClient, context.GetEvalCtx())
        .ExprToPB(predicate.as_ref())
        .expect("NOT(IS NULL(text column + integer)) must encode to TiPB");
    assert_eq!(encoded.get_sig(), tipb::ScalarFuncSig::UnaryNotInt);
    let encoded_is_null = &encoded.get_children()[0];
    assert_eq!(encoded_is_null.get_sig(), tipb::ScalarFuncSig::IntIsNull);
    assert_eq!(
        encoded_is_null.get_children()[0].get_sig(),
        tipb::ScalarFuncSig::PlusInt
    );
}

#[test]
/// 验证 REGEXP_* 工厂在最小/最大参数个数下的默认求值。
fn regexp_factories_evaluate_minimum_and_maximum_arities_with_go_defaults() {
    let context = exprstatic::NewExprContext(Vec::new());
    let string = |value: &str| Box::new(crate::NewStrConst(value)) as crate::ExprBox;
    let integer = |value| Box::new(crate::NewInt64Const(value)) as crate::ExprBox;
    let eval = context.GetEvalCtx();
    let row = crate::chunk::Row::default();

    for (arguments, expected) in [
        (vec![string("abcabc"), string("b")], 2),
        (
            vec![
                string("abcabc"),
                string("b"),
                integer(2),
                integer(2),
                integer(1),
                string(""),
            ],
            6,
        ),
    ] {
        let expression = crate::NewFunctionBase(
            &context,
            crate::ast::functions::RegexpInStr,
            *crate::types::NewFieldType(crate::mysql::TypeLonglong),
            arguments,
        )
        .unwrap();
        assert_eq!(
            expression.EvalInt(eval, row.clone()).unwrap(),
            (expected, false)
        );
    }

    for (arguments, expected) in [
        (vec![string("abcabc"), string("b.")], "bc"),
        (
            vec![
                string("abcabc"),
                string("b."),
                integer(2),
                integer(2),
                string(""),
            ],
            "bc",
        ),
    ] {
        let expression = crate::NewFunctionBase(
            &context,
            crate::ast::functions::RegexpSubstr,
            *crate::types::NewFieldType(crate::mysql::TypeVarchar),
            arguments,
        )
        .unwrap();
        assert_eq!(
            expression.EvalString(eval, row.clone()).unwrap(),
            (expected.to_owned(), false)
        );
    }

    for (arguments, expected) in [
        (vec![string("abcabc"), string("b"), string("X")], "aXcaXc"),
        (
            vec![
                string("abcabc"),
                string("b"),
                string("X"),
                integer(2),
                integer(1),
                string(""),
            ],
            "aXcabc",
        ),
    ] {
        let expression = crate::NewFunctionBase(
            &context,
            crate::ast::functions::RegexpReplace,
            *crate::types::NewFieldType(crate::mysql::TypeVarchar),
            arguments,
        )
        .unwrap();
        assert_eq!(
            expression.EvalString(eval, row.clone()).unwrap(),
            (expected.to_owned(), false)
        );
    }
}

#[test]
/// 验证 WEEKDAY 工厂的日期转换与 NULL 语义。
fn weekday_factory_matches_go_datetime_conversion_and_null_semantics() {
    let context = exprstatic::NewExprContext(Vec::new());
    let eval = context.GetEvalCtx();
    let row = crate::chunk::Row::default();

    for (date, expected) in [
        ("2020-01-01 00:00:00", 2),
        ("2020-01-05 23:59:59", 6),
        ("2020-01-06", 0),
    ] {
        let expression = crate::NewFunctionBase(
            &context,
            crate::ast::functions::Weekday,
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![Box::new(crate::NewStrConst(date))],
        )
        .unwrap_or_else(|error| panic!("build weekday({date}): {error}"));
        assert_eq!(
            expression.EvalInt(eval, row.clone()).unwrap(),
            (expected, false)
        );
        let scalar = expression
            .as_any()
            .downcast_ref::<crate::ScalarFunction>()
            .expect("weekday must remain a scalar function");
        assert_eq!(scalar.GetType(eval).GetFlen(), 1);
        assert_eq!(
            scalar.Function.PbCode(),
            tipb::ScalarFuncSig::WeekDay as i32
        );
    }

    let null_expression = crate::NewFunctionBase(
        &context,
        crate::ast::functions::Weekday,
        *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
        vec![Box::new(crate::NewNull())],
    )
    .unwrap();
    assert_eq!(null_expression.EvalInt(eval, row).unwrap(), (0, true));
}

#[test]
/// 验证 EXTRACT 按 Go 的日期、时长及二义字符串三条签名路径构建和求值。
fn extract_factory_matches_go_temporal_dispatch_and_pb_signatures() {
    let context = exprstatic::NewExprContext(Vec::new());
    let eval = context.GetEvalCtx();
    let row = crate::chunk::Row::default();

    for (unit, value, expected, signature) in [
        (
            "YEAR",
            "2020-01-02 12:34:56",
            2020,
            tipb::ScalarFuncSig::ExtractDatetime,
        ),
        ("HOUR", "12:34:56", 12, tipb::ScalarFuncSig::ExtractDuration),
        (
            "DAY_SECOND",
            "01 02:03:04",
            260_304,
            tipb::ScalarFuncSig::ExtractDatetimeFromString,
        ),
    ] {
        let expression = crate::NewFunctionBase(
            &context,
            crate::ast::Extract,
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![
                Box::new(crate::NewStrConst(unit)),
                Box::new(crate::NewStrConst(value)),
            ],
        )
        .unwrap_or_else(|error| panic!("build extract({unit} from {value}): {error}"));
        assert_eq!(
            expression.EvalInt(eval, row.clone()).unwrap(),
            (expected, false),
            "extract({unit} from {value})"
        );
        let scalar = expression
            .as_any()
            .downcast_ref::<crate::ScalarFunction>()
            .expect("extract must remain a scalar function");
        assert_eq!(scalar.Function.PbCode(), signature as i32, "{unit}");
    }
}

#[test]
/// 验证已迁移的字符串、控制流与 LIKE 工厂运行时结果。
fn migrated_string_control_and_like_factories_match_go_runtime() {
    let context = exprstatic::NewExprContext(Vec::new());
    let eval = context.GetEvalCtx();
    let row = crate::chunk::Row::default();
    let unspecified = || *crate::types::NewFieldType(crate::mysql::TypeUnspecified);

    let char_length = crate::NewFunctionBase(
        &context,
        crate::ast::functions::CharLength,
        unspecified(),
        vec![Box::new(crate::NewStrConst("你好"))],
    )
    .unwrap();
    assert_eq!(char_length.EvalInt(eval, row.clone()).unwrap(), (2, false));
    assert_eq!(
        char_length
            .as_any()
            .downcast_ref::<crate::ScalarFunction>()
            .unwrap()
            .Function
            .PbCode(),
        tipb::ScalarFuncSig::CharLengthUtf8 as i32
    );

    let hex = crate::NewFunctionBase(
        &context,
        crate::ast::functions::Hex,
        unspecified(),
        vec![Box::new(crate::NewStrConst("AZ"))],
    )
    .unwrap();
    assert_eq!(
        hex.EvalString(eval, row.clone()).unwrap(),
        ("415A".to_owned(), false)
    );

    let if_null = crate::NewFunctionBase(
        &context,
        crate::ast::functions::Ifnull,
        unspecified(),
        vec![
            Box::new(crate::NewNull()),
            Box::new(crate::NewStrConst("fallback")),
        ],
    )
    .unwrap();
    assert_eq!(
        if_null.EvalString(eval, row.clone()).unwrap(),
        ("fallback".to_owned(), false)
    );

    let like = crate::NewFunctionBase(
        &context,
        crate::ast::functions::Like,
        unspecified(),
        vec![
            Box::new(crate::NewStrConst("aster-sql")),
            Box::new(crate::NewStrConst("aster%")),
            Box::new(crate::NewInt64Const('\\' as i64)),
        ],
    )
    .unwrap();
    assert_eq!(like.EvalInt(eval, row).unwrap(), (1, false));
    assert_eq!(
        like.as_any()
            .downcast_ref::<crate::ScalarFunction>()
            .unwrap()
            .Function
            .PbCode(),
        tipb::ScalarFuncSig::LikeSig as i32
    );
}

#[test]
/// 验证 PB 签名名不会把驼峰函数名误映射为比较运算符。
fn pb_signature_names_do_not_confuse_camel_case_functions_with_comparisons() {
    assert_eq!(
        crate::PBSignatureFunctionName("GetFormat"),
        Some(crate::ast::functions::GetFormat)
    );
    assert_eq!(
        crate::PBSignatureFunctionName("LeftShift"),
        Some(crate::ast::functions::LeftShift)
    );
    assert_eq!(
        crate::PBSignatureFunctionName("LeastString"),
        Some(crate::ast::functions::Least)
    );
    assert_eq!(
        crate::PBSignatureFunctionName("Length"),
        Some(crate::ast::functions::Length)
    );
    assert_eq!(
        crate::PBSignatureFunctionName("GEInt"),
        Some(crate::ast::functions::GE)
    );
    assert_eq!(
        crate::PBSignatureFunctionName("LEString"),
        Some(crate::ast::functions::LE)
    );
}

/// 供 builtin.go 同名迁移入口复用的完整注册与求值回归集合。
pub(crate) fn run_builtin_registry_parity_suite() {
    builtin_registry_matches_complete_go_function_table();
    builtin_argument_ranges_match_go_unbounded_and_optional_rules();
    builtin_listing_and_operator_display_follow_go_filters();
    tiflash_scalar_factories_build_the_go_pb_signatures();
    comparison_and_logic_factories_install_the_go_pb_signatures();
    not_is_null_text_column_encodes_recursively_for_tikv();
    regexp_factories_evaluate_minimum_and_maximum_arities_with_go_defaults();
    weekday_factory_matches_go_datetime_conversion_and_null_semantics();
    extract_factory_matches_go_temporal_dispatch_and_pb_signatures();
    migrated_string_control_and_like_factories_match_go_runtime();
    pb_signature_names_do_not_confuse_camel_case_functions_with_comparisons();
}

#[test]
fn pg_function_01() {
    let context = exprstatic::NewExprContext(Vec::new());
    let expression = crate::NewFunctionBase(
        &context,
        "abs",
        *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
        vec![Box::new(crate::NewInt64Const(-1))],
    )
    .expect("ABS must construct through the formal registry");
    assert_eq!(
        expression
            .EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap(),
        (1, false)
    );
    assert_eq!(
        expression.GetType(context.GetEvalCtx()).GetType(),
        crate::mysql::TypeLonglong
    );
}

#[test]
fn pg_function_01_exact_signed_signatures() {
    let eval = exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
        crate::ast::SqlDialect::PostgreSQL,
    )]);
    let context =
        exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(eval))]);
    for (kind, maximum, minimum) in [
        (
            crate::mysql::TypeLong,
            i64::from(i32::MAX),
            i64::from(i32::MIN),
        ),
        (crate::mysql::TypeLonglong, i64::MAX, i64::MIN),
    ] {
        for (value, expected) in [
            (Some(-1), Some(1)),
            (Some(0), Some(0)),
            (Some(maximum), Some(maximum)),
            (None, None),
        ] {
            let mut argument = value.map_or_else(crate::NewNull, crate::NewInt64Const);
            argument.RetType = Some(*crate::types::NewFieldType(kind));
            let function = crate::NewFunctionBase(
                &context,
                "abs",
                *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
                vec![Box::new(argument)],
            )
            .unwrap();
            assert_eq!(function.GetType(context.GetEvalCtx()).GetType(), kind);
            let (actual, null) = function
                .EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap();
            assert_eq!((!null).then_some(actual), expected);
        }
        let mut argument = crate::NewInt64Const(minimum);
        argument.RetType = Some(*crate::types::NewFieldType(kind));
        let function = crate::NewFunctionBase(
            &context,
            "abs",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![Box::new(argument)],
        )
        .unwrap();
        assert!(
            function
                .EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
                .is_err()
        );
        // The selected signature owns its overflow domain even if an
        // expression is evaluated with another connection's context.
        let mysql_context = exprstatic::NewExprContext(Vec::new());
        assert!(
            function
                .EvalInt(mysql_context.GetEvalCtx(), crate::chunk::Row::default())
                .is_err()
        );
    }
}

#[test]
fn pg_function_02() {
    let eval = exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
        crate::ast::SqlDialect::PostgreSQL,
    )]);
    let context =
        exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(eval))]);
    let fixture = crate::formal_registry::FunctionClassRegistry::default();
    fixture.Store("numeric_abs", funcs.get("abs").unwrap());
    assert!(
        fixture
            .get_for_dialect("numeric_abs", crate::ast::SqlDialect::PostgreSQL)
            .is_some()
    );
    assert!(
        fixture
            .get_for_dialect("numeric_abs", crate::ast::SqlDialect::MySQL)
            .is_none()
    );
    assert!(
        fixture
            .names_for_dialect(crate::ast::SqlDialect::MySQL)
            .is_empty()
    );
    assert_eq!(
        fixture.names_for_dialect(crate::ast::SqlDialect::PostgreSQL),
        ["numeric_abs"]
    );
    for name in ["ifnull", "date_format", "json_extract", "tidb_version"] {
        assert!(
            funcs
                .get_for_dialect(name, crate::ast::SqlDialect::MySQL)
                .is_some()
        );
        assert!(
            funcs
                .get_for_dialect(name, crate::ast::SqlDialect::PostgreSQL)
                .is_none()
        );
        assert!(
            !funcs
                .names_for_dialect(crate::ast::SqlDialect::PostgreSQL)
                .contains(&name.to_owned())
        );
    }
    for (name, count) in [
        ("ifnull", 2),
        ("date_format", 2),
        ("json_extract", 2),
        ("tidb_version", 0),
    ] {
        let result = crate::NewFunctionBase(
            &context,
            name,
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            (0..count)
                .map(|_| Box::new(crate::NewNull()) as crate::ExprBox)
                .collect(),
        );
        assert!(
            result.is_err(),
            "PostgreSQL must hide {name} before constructing a builtin"
        );
    }
}

#[test]
fn pg_function_03() {
    use crate::ast::SqlDialect;
    use crate::formal_registry::{resolve_function_class, with_function_registry};
    let fixture = std::sync::Arc::new(funcs.snapshot_without_factory("abs"));
    for dialect in [SqlDialect::PostgreSQL, SqlDialect::MySQL] {
        let eval = exprstatic::NewEvalContext(vec![
            exprstatic::WithSQLDialect(dialect),
            exprstatic::WithCurrentDB("test".to_owned()),
        ]);
        let context =
            exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(eval))]);
        let build = |name: &str| {
            crate::NewFunctionBase(
                &context,
                name,
                *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
                vec![Box::new(crate::NewInt64Const(-1))],
            )
        };
        with_function_registry(fixture.clone(), || {
            assert!(resolve_function_class("abs", dialect).is_some());
            assert!(resolve_function_class("astersql_missing_function_03", dialect).is_none());
            let error = build("abs")
                .err()
                .expect("visible ABS without factory must fail")
                .to_string();
            assert_eq!(
                error,
                "builtin function 'abs' is registered but its complete implementation is not linked"
            );
            let unknown = build("astersql_missing_function_03")
                .err()
                .expect("unknown function must fail")
                .to_string();
            assert!(!unknown.contains("not linked"));
            assert_eq!(
                unknown,
                "FUNCTION test.astersql_missing_function_03 does not exist"
            );
            println!("{dialect:?}: missing factory={error}; unknown={unknown}");
            // A nested failure restores the outer private snapshot as well.
            let nested = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                with_function_registry(fixture.clone(), || panic!("fixture unwind"));
            }));
            assert!(nested.is_err());
            assert!(build("abs").is_err());
        });
        assert_eq!(
            build("abs")
                .unwrap()
                .EvalInt(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap(),
            (1, false)
        );
    }
    // Unwinding the outermost scope also restores the ordinary factory.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_function_registry(fixture, || panic!("outer fixture unwind"));
    }));
    let context = exprstatic::NewExprContext(Vec::new());
    assert!(
        crate::NewFunctionBase(
            &context,
            "abs",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![Box::new(crate::NewInt64Const(-1))]
        )
        .is_ok()
    );
}

#[test]
fn pg_function_04() {
    let eval = exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
        crate::ast::SqlDialect::PostgreSQL,
    )]);
    let context =
        exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(eval))]);
    let mut value = crate::NewInt64Const(4);
    value.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLong));
    let result = crate::NewFunctionBase(
        &context,
        "round",
        *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
        vec![Box::new(value.clone()), Box::new(value)],
    );
    assert!(
        result.is_ok(),
        "round(int4,int4) must select numeric: {:?}",
        result.err().map(|e| e.to_string())
    );
    let result = result.unwrap();
    assert_eq!(
        result.GetType(context.GetEvalCtx()).GetType(),
        crate::mysql::TypeNewDecimal
    );
    assert_eq!(
        result
            .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
            .unwrap(),
        ("4.0000".into(), false)
    );
}

#[test]
fn pg_function_04_candidate_order() {
    use crate::formal_registry::{
        PgFunctionCandidate as C, pg_numeric_implicit_source, select_pg_candidate,
    };
    use crate::mysql::{TypeDouble as F, TypeLong as I, TypeNewDecimal as N};
    let exact = C {
        arguments: vec![I, I],
        result: I,
    };
    let numeric = C {
        arguments: vec![I, N],
        result: N,
    };
    let preferred = C {
        arguments: vec![F, I],
        result: F,
    };
    // These are the existing numeric_abs and float8abs implicit input sets;
    // fixtures do not install a process-global registry or invent cast costs.
    let implicit = |source, target| {
        (target == N && pg_numeric_implicit_source(source))
            || (target == F && matches!(source, I | N | F))
    };
    for reverse in [false, true] {
        let mut candidates = vec![numeric.clone(), preferred.clone(), exact.clone()];
        if reverse {
            candidates.reverse();
        }
        assert_eq!(
            select_pg_candidate(&[I, I], &candidates, implicit, |_| b'N', |t| t == F),
            Ok(&exact)
        );
        candidates.retain(|c| c != &exact);
        assert_eq!(
            select_pg_candidate(&[I, I], &candidates, implicit, |_| b'N', |t| t == F),
            Ok(&preferred)
        );
    }
}

#[test]
fn pg_function_05() {
    use crate::formal_registry::{PgFunctionCandidate as C, select_pg_candidate};
    use crate::mysql::{
        TypeDouble as F, TypeLong as I, TypeLonglong as B, TypeNull as U, TypeString as S,
        TypeVarString as T,
    };
    let category = |t| if matches!(t, T | S) { b'S' } else { b'N' };
    let preferred = |t| matches!(t, T | F);
    let implicit = |s, t| s == U || (s == I && t == F);
    let c = |a: Vec<u8>| C {
        result: a[0],
        arguments: a,
    };
    for reverse in [false, true] {
        for (inputs, signatures, expected) in [
            (vec![U], vec![vec![I], vec![T]], vec![T]),
            (vec![U], vec![vec![S], vec![T]], vec![T]),
            (vec![U], vec![vec![I], vec![F]], vec![F]),
            (
                vec![I, U],
                vec![vec![I, I], vec![I, T], vec![F, F]],
                vec![I, T],
            ),
            (vec![I, U], vec![vec![I, I], vec![I, B]], vec![I, I]),
            (vec![I], vec![vec![I], vec![T]], vec![I]),
        ] {
            let mut candidates: Vec<_> = signatures.into_iter().map(c).collect();
            if reverse {
                candidates.reverse();
            }
            let selected = select_pg_candidate(&inputs, &candidates, implicit, category, preferred);
            assert_eq!(
                selected.map(|v| v.arguments.clone()),
                Ok(expected),
                "inputs={inputs:?}, candidates={candidates:?}"
            );
        }
    }
}

#[test]
fn pg_function_05_formal_substr() {
    let eval = exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
        crate::ast::SqlDialect::PostgreSQL,
    )]);
    let context =
        exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(eval))]);
    let mut position = crate::NewInt64Const(3);
    position.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeLong));
    for (value, expected) in [
        (crate::NewStrConst("1234"), ("34".to_string(), false)),
        (
            crate::NewNullWithFieldType(*crate::types::NewFieldType(crate::mysql::TypeNull)),
            (String::new(), true),
        ),
        (
            crate::NewNullWithFieldType(*crate::types::NewFieldType(crate::mysql::TypeVarString)),
            (String::new(), true),
        ),
    ] {
        let result = crate::NewFunctionBase(
            &context,
            "substr",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![Box::new(value), Box::new(position.clone())],
        )
        .unwrap();
        assert_eq!(
            result
                .EvalString(context.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap(),
            expected
        );
    }
    let typed_null =
        crate::NewNullWithFieldType(*crate::types::NewFieldType(crate::mysql::TypeLong));
    assert!(
        crate::NewFunctionBase(
            &context,
            "substr",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![Box::new(typed_null), Box::new(position)]
        )
        .is_err()
    );
}

#[test]
fn pg_function_06() {
    use crate::formal_registry::{PgFunctionCandidate, select_pg_candidate};
    use crate::mysql::{TypeLong as I, TypeLonglong as B, TypeNull as U};
    let before = funcs.names();
    // Private signatures for one schema's f(int4)/f(int8); never register f.
    // Deliberately cross the result identities: return type cannot break a tie.
    for reverse in [false, true] {
        let mut candidates = vec![
            PgFunctionCandidate {
                arguments: vec![I],
                result: B,
            },
            PgFunctionCandidate {
                arguments: vec![B],
                result: I,
            },
        ];
        if reverse {
            candidates.reverse();
        }
        for source in ["unknown literal '1'", "unknown NULL"] {
            assert_eq!(
                select_pg_candidate(
                    &[U],
                    &candidates,
                    |s, t| s == I && t == B,
                    |_| b'N',
                    |_| false
                ),
                Err("42725"),
                "{source}, reverse={reverse}"
            );
        }
        let selected = select_pg_candidate(
            &[I],
            &candidates,
            |s, t| s == I && t == B,
            |_| b'N',
            |_| false,
        )
        .unwrap();
        assert_eq!(selected.arguments, vec![I]);
        assert_eq!(selected.result, B);
    }
    assert_eq!(funcs.names(), before, "private candidates must not escape");
}

#[test]
fn pg_function_07() {
    use crate::formal_registry::{
        PgDeclaredCandidate, PgFunctionCandidate, select_pg_declared_candidate,
    };
    use types_dependency::field::ast::PostgreSQLArrayElement as E;
    let before = funcs.names();
    let declaration = |schema: &str, arguments: Vec<u32>, variadic| PgDeclaredCandidate {
        schema: schema.into(),
        signature: PgFunctionCandidate {
            arguments,
            result: 23,
        },
        variadic,
        default_arguments: 0,
    };
    let variadic = declaration("a", vec![E::Int4.array_oid()], Some(E::Int4));
    let fixed = declaration("a", vec![23, 23], None);
    let select = |inputs: &[u32], candidates: &[PgDeclaredCandidate], path: &[&str], explicit| {
        select_pg_declared_candidate(
            inputs,
            candidates,
            path,
            explicit,
            |s, t| s == 23 && t == 20,
            |_| b'N',
            |_| false,
        )
        .cloned()
    };
    for reverse in [false, true] {
        let mut pair = vec![variadic.clone(), fixed.clone()];
        if reverse {
            pair.reverse();
        }
        // Scalar expansion, including zero variadic elements and a fixed prefix.
        for inputs in [vec![], vec![23], vec![23, 23, 23], vec![0, 23]] {
            assert_eq!(
                select(&inputs, &[variadic.clone()], &["a"], false),
                Ok(variadic.clone())
            );
        }
        assert_eq!(select(&[23, 23], &pair, &["a"], false), Ok(fixed.clone()));
        assert_eq!(
            select(&[E::Int4.array_oid()], &pair, &["a"], true),
            Ok(variadic.clone())
        );
        assert_eq!(select(&[23], &pair, &["a"], true), Err("42883"));
        assert_eq!(
            select(&[E::Int8.array_oid()], &pair, &["a"], true),
            Err("42883")
        );
        let prefix = declaration("a", vec![25, E::Int4.array_oid()], Some(E::Int4));
        assert_eq!(
            select(&[25], &[prefix.clone()], &["a"], false),
            Ok(prefix.clone())
        );
        assert_eq!(select(&[], &[prefix.clone()], &["a"], false), Err("42883"));
        assert_eq!(
            select(&[25, 23, 23], &[prefix.clone()], &["a"], false),
            Ok(prefix.clone())
        );
        assert_eq!(
            select(&[25, E::Int4.array_oid()], &[prefix.clone()], &["a"], true),
            Ok(prefix)
        );
        let earlier = declaration("b", vec![23, 23], None);
        let mut cross = vec![earlier.clone(), variadic.clone()];
        if reverse {
            cross.reverse();
        }
        assert_eq!(select(&[23, 23], &cross, &["b", "a"], false), Ok(earlier));
        assert_eq!(
            select(&[23, 23], &cross, &["a", "b"], false),
            Ok(variadic.clone())
        );
        let wider = declaration("b", vec![20, 20], None);
        let mut different = vec![wider, variadic.clone()];
        if reverse {
            different.reverse();
        }
        assert_eq!(
            select(&[23, 23], &different, &["b", "a"], false),
            Ok(variadic.clone())
        );
        assert_eq!(
            select(&[0, 0], &different, &["b", "a"], false),
            Err("42725")
        );
        assert_eq!(
            select(&[23], &[variadic.clone()], &["hidden"], false),
            Err("42883")
        );
        assert_eq!(
            select(&[E::Int4.array_oid()], &[fixed.clone()], &["a"], true),
            Err("42883")
        );
    }
    let malformed = declaration("a", vec![23], Some(E::Int4));
    assert_eq!(select(&[23], &[malformed], &["a"], false), Err("42883"));
    assert_eq!(
        select(&[23, 23], &[fixed.clone(), fixed.clone()], &["a"], false),
        Err("42725")
    );
    // Ordinary array arguments do not implicitly become variadic scalar elements.
    assert_eq!(
        select(&[E::Int4.array_oid()], &[variadic.clone()], &["a"], false),
        Err("42883")
    );
    assert_eq!(funcs.names(), before);
}

#[test]
fn pg_function_08() {
    use crate::formal_registry::{
        PgDeclaredCandidate, PgFunctionCandidate, select_pg_declared_candidate,
    };
    let before = funcs.names();
    let declaration = |schema: &str, arguments: Vec<u32>, result, defaults| PgDeclaredCandidate {
        schema: schema.into(),
        signature: PgFunctionCandidate { arguments, result },
        variadic: None,
        default_arguments: defaults,
    };
    let a = declaration("a", vec![23, 23], 20, 1);
    let b = declaration("a", vec![23, 20], 23, 1);
    let select = |inputs: &[u32], candidates: &[PgDeclaredCandidate], path: &[&str]| {
        select_pg_declared_candidate(
            inputs,
            candidates,
            path,
            false,
            |s, t| s == 23 && t == 20,
            |_| b'N',
            |_| false,
        )
        .cloned()
    };
    for reverse in [false, true] {
        let mut pair = vec![a.clone(), b.clone()];
        if reverse {
            pair.reverse();
        }
        // Omitted positions leave equal effective signatures in the same schema.
        // Crossed return identities cannot break this ambiguity.
        assert_eq!(select(&[23], &pair, &["a"]), Err("42725"));
        assert_eq!(select(&[0], &pair, &["a"]), Err("42725"));
        assert_eq!(select(&[23, 23], &pair, &["a"]), Ok(a.clone()));
        assert_eq!(select(&[23, 20], &pair, &["a"]), Ok(b.clone()));
        assert_eq!(select(&[], &pair, &["a"]), Err("42883"));
        assert_eq!(select(&[23, 23, 23], &pair, &["a"]), Err("42883"));
        assert_eq!(select(&[25], &pair, &["a"]), Err("42883"));
        assert_eq!(select(&[23], &[a.clone()], &["a"]), Ok(a.clone()));
        assert_eq!(select(&[23], &pair, &["hidden"]), Err("42883"));
        let earlier = declaration("b", vec![23, 20], 25, 1);
        let mut cross = vec![a.clone(), earlier.clone()];
        if reverse {
            cross.reverse();
        }
        assert_eq!(select(&[23], &cross, &["b", "a"]), Ok(earlier.clone()));
        assert_eq!(select(&[23], &cross, &["a", "b"]), Ok(a.clone()));
        assert_eq!(select(&[23], &cross, &["b"]), Ok(earlier));
        // Distinct effective input signatures compete by type, not path rank.
        let wider = declaration("b", vec![20, 20], 25, 1);
        assert_eq!(
            select(&[23], &[wider, a.clone()], &["b", "a"]),
            Ok(a.clone())
        );
        let fixed = declaration("a", vec![23], 25, 0);
        assert_eq!(select(&[23], &[fixed, a.clone()], &["a"]), Err("42725"));
    }
    let all_default = declaration("a", vec![23, 20], 23, 2);
    assert_eq!(select(&[], &[all_default.clone()], &["a"]), Ok(all_default));
    let malformed = declaration("a", vec![23], 23, 2);
    assert_eq!(select(&[23], &[malformed], &["a"]), Err("42883"));
    let required = declaration("a", vec![23, 20], 23, 0);
    assert_eq!(select(&[23], &[required], &["a"]), Err("42883"));
    assert_eq!(
        funcs.names(),
        before,
        "private default candidates must not escape"
    );
}

#[test]
fn pg_function_25_coalesce_factory() {
    use crate::{Constant, Expression, NewFunctionBase, chunk, mysql, types};
    let ctx = exprstatic::NewExprContext(Vec::new());
    let mut integer = *types::NewFieldType(mysql::TypeLonglong);
    integer.SetFlen(22);
    integer.SetDecimal(0);
    integer.AddFlag(mysql::BinaryFlag);
    let arg = |value| Box::new(Constant::with_type(value, integer.clone())) as crate::ExprBox;
    let build = |args| {
        NewFunctionBase(
            &ctx,
            "coalesce",
            *types::NewFieldType(mysql::TypeUnspecified),
            args,
        )
    };
    let overflow = || {
        NewFunctionBase(
            &ctx,
            "abs",
            integer.clone(),
            vec![arg(types::NewIntDatum(i64::MIN))],
        )
        .unwrap()
    };
    let short = build(vec![
        arg(types::Datum::default()),
        arg(types::NewIntDatum(7)),
        overflow(),
    ])
    .unwrap();
    assert_eq!(
        short
            .EvalInt(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap(),
        (7, false)
    );
    let required = build(vec![
        arg(types::Datum::default()),
        overflow(),
        arg(types::NewIntDatum(7)),
    ])
    .unwrap();
    assert!(
        required
            .EvalInt(ctx.GetEvalCtx(), chunk::Row::default())
            .is_err()
    );
    assert!(build(vec![]).is_err());
    let all_null = build(vec![
        arg(types::Datum::default()),
        arg(types::Datum::default()),
    ])
    .unwrap();
    assert!(
        all_null
            .EvalInt(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap()
            .1
    );
    let mut numeric = *types::NewFieldType(mysql::TypeNewDecimal);
    numeric.SetFlen(8);
    numeric.SetDecimal(2);
    numeric.AddFlag(mysql::BinaryFlag);
    let mut decimal = types::MyDecimal::default();
    decimal.FromString(b"2.50").unwrap();
    let mixed = build(vec![
        arg(types::NewIntDatum(7)),
        Box::new(Constant::with_type(
            types::NewDecimalDatum(decimal),
            numeric,
        )),
    ])
    .unwrap();
    assert_eq!(mixed.GetType(ctx.GetEvalCtx()).GetDecimal(), 2);
    assert_eq!(
        mixed
            .EvalDecimal(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap()
            .0
            .ToString(),
        b"7"
    );
    let divide = NewFunctionBase(
        &ctx,
        "/",
        *types::NewFieldType(mysql::TypeUnspecified),
        vec![arg(types::NewIntDatum(10)), arg(types::NewIntDatum(2))],
    )
    .unwrap();
    assert_eq!(divide.GetType(ctx.GetEvalCtx()).GetDecimal(), 4);
    assert_eq!(
        divide
            .EvalDecimal(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap()
            .0
            .String(),
        "5.0000"
    );
    let column = |index| {
        Box::new(crate::Column {
            RetType: Some(integer.Clone()),
            Index: index,
            ..Default::default()
        }) as crate::ExprBox
    };
    let division = NewFunctionBase(
        &ctx,
        "/",
        *types::NewFieldType(mysql::TypeUnspecified),
        vec![column(0), column(1)],
    )
    .unwrap();
    let input =
        chunk::mutrow::MutRowFromDatums(vec![types::NewIntDatum(10), types::NewIntDatum(2)]);
    assert_eq!(
        division
            .EvalDecimal(ctx.GetEvalCtx(), input.ToRow())
            .unwrap()
            .0
            .String(),
        "5.0000"
    );
    let mut text = *types::NewFieldType(mysql::TypeVarchar);
    text.SetCharset("utf8mb4".to_owned());
    text.SetCollate("utf8mb4_bin".to_owned());
    text.SetFlen(2);
    let strings = build(vec![
        Box::new(Constant::with_type(types::Datum::default(), text.clone())),
        Box::new(Constant::with_type(
            types::NewStringDatum(String::new()),
            text,
        )),
    ])
    .unwrap();
    assert_eq!(
        strings
            .EvalString(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap(),
        (String::new(), false)
    );
}

#[test]
fn pg_function_25_nullif_numeric_fields() {
    use crate::{Constant, NewFunctionBase, mysql, types};
    let ctx = exprstatic::NewExprContext(Vec::new());
    let mut integer = *types::NewFieldType(mysql::TypeLong);
    integer.SetFlen(11);
    integer.SetDecimal(0);
    let source = crate::Column {
        RetType: Some(integer.Clone()),
        Index: 0,
        ..Default::default()
    };
    let function = NewFunctionBase(
        &ctx,
        "if",
        integer.clone(),
        vec![
            Box::new(Constant::with_type(
                types::NewIntDatum(1),
                *types::NewFieldType(mysql::TypeLonglong),
            )),
            Box::new(Constant::with_type(
                types::Datum::default(),
                *types::NewFieldType(mysql::TypeNull),
            )),
            Box::new(source),
        ],
    )
    .unwrap();
    assert_ne!(
        function.GetType(ctx.GetEvalCtx()).GetFlag() & mysql::BinaryFlag,
        0,
        "Go IF adds the base numeric BinaryFlag to its inferred value type"
    );
    assert_eq!(
        function.GetType(ctx.GetEvalCtx()).GetType(),
        mysql::TypeLong
    );
    assert_eq!(integer.GetFlag(), 0, "do not mutate the source column");
}

#[test]
fn pg_function_27_json_document_factory() {
    let context = exprstatic::NewExprContext(Vec::new());
    let build = |document: crate::Constant| {
        crate::NewFunctionBase(
            &context,
            "json_extract",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![Box::new(document), Box::new(crate::NewStrConst("$"))],
        )
    };
    let error = build(crate::NewInt64Const(1))
        .err()
        .expect("Go rejects numeric documents")
        .to_string();
    assert!(error.contains("[expression:3146]"), "{error}");
    assert!(build(crate::NewStrConst("1")).is_ok());
    // NewNull() is a typed TINY NULL in this repository; Go rejects that
    // document type as well. An untyped SQL NULL has TypeNull instead.
    assert!(build(crate::NewNull()).is_err());
    assert!(
        build(crate::NewNullWithFieldType(*crate::types::NewFieldType(
            crate::mysql::TypeNull
        )))
        .is_ok()
    );
}

#[test]
fn pg_function_23_formal_factories() {
    use crate::{BuildContext, Expression};
    let mut null_parameter = *crate::types::NewFieldType(crate::mysql::TypeUnspecified);
    crate::types::InferParamTypeFromDatum(&crate::types::Datum::default(), &mut null_parameter);
    assert_eq!(null_parameter.GetType(), crate::mysql::TypeNull);
    assert_eq!(
        null_parameter.GetFlen(),
        0,
        "Go NULL parameter metadata must retain its field width"
    );
    assert_eq!(null_parameter.GetDecimal(), 0);
    let mut marker =
        crate::Constant::with_type(crate::types::Datum::default(), null_parameter.clone());
    marker.ParamMarker = Some(crate::ParamMarker::new(0));
    let mut arguments: Vec<Box<dyn Expression>> = vec![
        Box::new(marker),
        Box::new(crate::Constant::with_type(
            crate::types::NewIntDatum(3),
            *crate::types::NewFieldType(crate::mysql::TypeLonglong),
        )),
    ];
    let mysql_eval = exprstatic::NewEvalContext(Vec::new());
    crate::typeInferForNull(&mysql_eval, &mut arguments);
    assert_eq!(
        arguments[0].GetType(&mysql_eval).GetType(),
        crate::mysql::TypeNull,
        "Go ParamMarker dynamic type survives NULL peer inference"
    );

    let eval = exprstatic::NewEvalContext(vec![exprstatic::WithSQLDialect(
        crate::ast::SqlDialect::PostgreSQL,
    )]);
    let pg = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(eval))]);
    for (value, expected) in [
        (Some("2026-10-29 12:34:56"), Some("2026-10-29 00:00:00")),
        (Some("infinity"), Some("infinity")),
        (None, None),
    ] {
        let mut timestamp = value.map(crate::NewStrConst).unwrap_or_else(crate::NewNull);
        timestamp.RetType = Some(*crate::types::NewFieldType(crate::mysql::TypeDatetime));
        let expression = crate::NewFunctionBase(
            &pg,
            "date_trunc",
            *crate::types::NewFieldType(crate::mysql::TypeDatetime),
            vec![Box::new(crate::NewStrConst("day")), Box::new(timestamp)],
        )
        .unwrap();
        assert_eq!(
            expression.GetType(pg.GetEvalCtx()).GetType(),
            crate::mysql::TypeDatetime
        );
        let scalar = expression
            .as_any()
            .downcast_ref::<crate::ScalarFunction>()
            .unwrap();
        assert!(!scalar.Function.SafeToShareAcrossSession());
        assert_eq!(
            scalar.Function.PbCode(),
            0,
            "this client evidence grants no pushdown signature"
        );
        for expression in [expression.CloneExpr(), expression] {
            let (actual, null) = expression
                .EvalString(pg.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap();
            assert_eq!((!null).then_some(actual.as_str()), expected);
        }
    }
    let mysql = exprstatic::NewExprContext(Vec::new());
    assert!(!crate::formal_registry::FunctionVisibleInDialect(
        "date_trunc",
        crate::ast::SqlDialect::MySQL
    ));
    for (value, fraction, expected) in [(-15, -1, -20), (15, -1, 20), (42, 2, 42)] {
        let expression = crate::NewFunctionBase(
            &mysql,
            "round",
            *crate::types::NewFieldType(crate::mysql::TypeUnspecified),
            vec![
                Box::new(crate::NewInt64Const(value)),
                Box::new(crate::NewInt64Const(fraction)),
            ],
        )
        .unwrap();
        assert_eq!(
            expression
                .EvalInt(mysql.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap(),
            (expected, false)
        );
        assert_eq!(expression.GetType(mysql.GetEvalCtx()).GetDecimal(), 0);
    }
    for (value, expected) in [(2.5_f64, 2.0), (3.5, 4.0), (-2.5, -2.0)] {
        let input = crate::Constant::with_type(
            crate::types::NewDatum(&value),
            *crate::types::NewFieldType(crate::mysql::TypeDouble),
        );
        let expression = crate::NewFunctionBase(
            &mysql,
            "round",
            *crate::types::NewFieldType(crate::mysql::TypeDouble),
            vec![Box::new(input)],
        )
        .unwrap();
        assert_eq!(
            expression
                .EvalReal(mysql.GetEvalCtx(), crate::chunk::Row::default())
                .unwrap(),
            (expected, false)
        );
    }
    let mut value = crate::types::MyDecimal::default();
    value.FromString(b"12.345").unwrap();
    let mut tp = *crate::types::NewFieldType(crate::mysql::TypeNewDecimal);
    tp.SetFlen(6);
    tp.SetDecimal(3);
    let input = crate::Constant::with_type(crate::types::NewDatum(&value), tp);
    let expression = crate::NewFunctionBase(
        &mysql,
        "round",
        *crate::types::NewFieldType(crate::mysql::TypeNewDecimal),
        vec![Box::new(input), Box::new(crate::NewInt64Const(2))],
    )
    .unwrap();
    let (actual, null) = expression
        .EvalDecimal(mysql.GetEvalCtx(), crate::chunk::Row::default())
        .unwrap();
    assert!(!null);
    assert_eq!(actual.ToString(), b"12.35");
    let expression = crate::NewFunctionBase(
        &mysql,
        "round",
        *crate::types::NewFieldType(crate::mysql::TypeLonglong),
        vec![
            Box::new(crate::NewInt64Const(i64::MAX)),
            Box::new(crate::NewInt64Const(-1)),
        ],
    )
    .unwrap();
    assert!(
        expression
            .EvalInt(mysql.GetEvalCtx(), crate::chunk::Row::default())
            .is_err()
    );
}

#[test]
fn pg_function_25_context_display_width() {
    use crate::{Expression, mysql, types};
    let legacy = exprstatic::NewExprContext(Vec::new());
    let modern_eval =
        exprstatic::NewEvalContext(vec![exprstatic::WithDeprecatedIntegerDisplayWidth(Some(
            true,
        ))]);
    let modern = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        modern_eval,
    ))]);
    let mut tp = *types::NewFieldType(mysql::TypeLonglong);
    tp.SetFlen(22);
    tp.SetDecimal(0);
    tp.AddFlag(mysql::BinaryFlag);
    let arg: crate::ExprBox = Box::new(crate::Column {
        OrigName: "p".to_owned(),
        RetType: Some(*types::NewFieldType(mysql::TypeVarString)),
        ..Default::default()
    });
    let cast = crate::BuildCastFunction(&legacy, &arg, &tp);
    assert_eq!(
        cast.StringWithCtx(Some(legacy.GetEvalCtx()), crate::errors::RedactLogDisable),
        "cast(p, bigint(22) BINARY)"
    );
    assert_eq!(
        cast.StringWithCtx(Some(modern.GetEvalCtx()), crate::errors::RedactLogDisable),
        "cast(p, bigint BINARY)"
    );
    assert_eq!(cast.GetType(modern.GetEvalCtx()).GetFlen(), 22);
}

#[test]
fn pg_function_26_formal_factories() {
    use crate::{Constant, Expression, NewFunctionBase, chunk, mysql, types};
    let ctx = exprstatic::NewExprContext(Vec::new());
    let mut integer = *types::NewFieldType(mysql::TypeLonglong);
    integer.SetFlen(22);
    integer.SetDecimal(0);
    let arg = |value| Box::new(Constant::with_type(value, integer.clone())) as crate::ExprBox;
    let overflow = || {
        NewFunctionBase(
            &ctx,
            "abs",
            integer.clone(),
            vec![arg(types::NewIntDatum(i64::MIN))],
        )
        .unwrap()
    };
    let build = |name: &str, args| {
        NewFunctionBase(
            &ctx,
            name,
            *types::NewFieldType(mysql::TypeUnspecified),
            args,
        )
    };
    for name in ["greatest", "least", "concat"] {
        let short = build(name, vec![arg(types::Datum::default()), overflow()]).unwrap();
        assert!(
            short
                .Eval(ctx.GetEvalCtx(), chunk::Row::default())
                .unwrap()
                .IsNull(),
            "{name}: NULL stops later error"
        );
        let required = build(name, vec![overflow(), arg(types::Datum::default())]).unwrap();
        assert!(
            required
                .Eval(ctx.GetEvalCtx(), chunk::Row::default())
                .is_err(),
            "{name}: preceding error propagates"
        );
        assert!(build(name, vec![]).is_err());
    }
    for name in ["greatest", "least"] {
        let values = build(
            name,
            vec![
                arg(types::NewIntDatum(i64::MIN)),
                arg(types::NewIntDatum(i64::MAX)),
            ],
        )
        .unwrap();
        assert_eq!(
            values
                .EvalInt(ctx.GetEvalCtx(), chunk::Row::default())
                .unwrap(),
            (
                if name == "greatest" {
                    i64::MAX
                } else {
                    i64::MIN
                },
                false
            )
        );
        assert!(build(name, vec![arg(types::NewIntDatum(7))]).is_err());
    }
    let mut numeric = *types::NewFieldType(mysql::TypeNewDecimal);
    numeric.SetFlen(4);
    numeric.SetDecimal(1);
    let mut decimal = types::MyDecimal::default();
    decimal.FromString(b"10.5").unwrap();
    let mixed = build(
        "least",
        vec![
            arg(types::NewIntDatum(2)),
            Box::new(Constant::with_type(
                types::NewDecimalDatum(decimal),
                numeric,
            )),
        ],
    )
    .unwrap();
    assert_eq!(
        mixed.GetType(ctx.GetEvalCtx()).GetType(),
        mysql::TypeNewDecimal
    );
    assert_eq!(
        mixed
            .EvalDecimal(ctx.GetEvalCtx(), chunk::Row::default())
            .unwrap()
            .0
            .ToString(),
        b"2"
    );
    // CONCAT packet overflow uses the existing statement warning/error policy.
    let limited_eval = exprstatic::NewEvalContext(vec![exprstatic::WithMaxAllowedPacket(1)]);
    let limited = exprstatic::NewExprContext(vec![exprstatic::WithEvalCtx(std::sync::Arc::new(
        limited_eval,
    ))]);
    let text = |v: &str| {
        Box::new(Constant::with_type(
            types::NewStringDatum(v.to_owned()),
            *types::NewFieldType(mysql::TypeVarString),
        )) as crate::ExprBox
    };
    let concat = NewFunctionBase(
        &limited,
        "concat",
        *types::NewFieldType(mysql::TypeUnspecified),
        vec![text("ab")],
    )
    .unwrap();
    assert!(
        concat
            .EvalString(limited.GetEvalCtx(), chunk::Row::default())
            .is_err()
    );
}

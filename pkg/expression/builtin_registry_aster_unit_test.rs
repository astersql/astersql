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
    const SQL_OPERATOR_ALIASES: &[&str] = &[
        "=", "!=", "<>", "<", "<=", ">", ">=", "+", "-", "*", "/", "%",
    ];
    assert_eq!(funcs.len(), 309 + SQL_OPERATOR_ALIASES.len());
    for alias in SQL_OPERATOR_ALIASES {
        assert!(
            IsFunctionSupported(alias),
            "missing parser operator alias {alias}"
        );
    }
    assert!(IsFunctionSupported("coalesce"));
    assert!(IsFunctionSupported("tidb_decode_sql_digests"));
    assert!(IsFunctionSupported("vec_cosine_distance"));
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

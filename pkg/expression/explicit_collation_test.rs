// Copyright 2026 AsterSQL.
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

// `SetCollationToExpression` 的元数据级单元测试。
//
// 验证字符集不匹配拒绝、列 CAST 不污染原类型、JSON 转 LONGTEXT、
// ENUM/SET 拒绝，以及常量就地更新与旧模式行为。

use crate::*;

/// 空用户变量读取器。
struct EmptyUserVars;

impl exprctx::UserVarsReader for EmptyUserVars {
    fn GetUserVarVal(&self, _name: &str) -> Option<types::Datum> {
        None
    }
    fn GetUserVarType(&self, _name: &str) -> Option<types::FieldType> {
        None
    }
    fn Clone(&self) -> Box<dyn exprctx::UserVarsReader> {
        Box::new(Self)
    }
}

/// 仅满足 trait 的轻量求值上下文；类型/错误路径对本测试未使用。
struct TestEvalContext(EmptyUserVars);

impl contextutil::WarnAppender for TestEvalContext {
    fn AppendWarning(&self, _error: contextutil::errors::SharedError) {}
    fn AppendNote(&self, _error: contextutil::errors::SharedError) {}
}

impl contextutil::WarnHandler for TestEvalContext {
    fn WarningCount(&self) -> usize {
        0
    }
    fn TruncateWarnings(&self, _start: isize) -> Vec<contextutil::SQLWarn> {
        Vec::new()
    }
    fn CopyWarnings(&self, destination: Vec<contextutil::SQLWarn>) -> Vec<contextutil::SQLWarn> {
        destination
    }
}

impl exprctx::ParamValues for TestEvalContext {
    fn GetParamValue(&self, _index: usize) -> Result<types::Datum, exprctx::ParamError> {
        Err(exprctx::ParamError::IndexExceedsParamCount)
    }
}

impl EvalContext for TestEvalContext {
    fn CtxID(&self) -> u64 {
        1
    }
    fn SQLMode(&self) -> mysql::SQLMode {
        mysql::SQLMode::default()
    }
    fn TypeCtx(&self) -> types::Context {
        panic!("not used by metadata-only test")
    }
    fn ErrCtx(&self) -> errctx::Context {
        panic!("not used by metadata-only test")
    }
    fn Location(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }
    fn CurrentTime(
        &self,
    ) -> Result<chrono::DateTime<chrono_tz::Tz>, contextutil::errors::SharedError> {
        panic!("not used by metadata-only test")
    }
    fn CurrentDB(&self) -> String {
        String::new()
    }
    fn GetMaxAllowedPacket(&self) -> u64 {
        64 << 20
    }
    fn GetTiDBRedactLog(&self) -> String {
        "OFF".to_owned()
    }
    fn GetDefaultWeekFormatMode(&self) -> String {
        "0".to_owned()
    }
    fn GetDivPrecisionIncrement(&self) -> i32 {
        4
    }
    fn GetUserVarsReader(&self) -> &dyn exprctx::UserVarsReader {
        &self.0
    }
    fn GetOptionalPropSet(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptionalEvalPropKeySet::default()
    }
    fn GetOptionalPropProvider(
        &self,
        _key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        None
    }
}

/// 构建期上下文：固定 utf8mb4 / utf8mb4_bin 等会话默认值。
struct TestBuildContext(TestEvalContext);

impl BuildContext for TestBuildContext {
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        &self.0
    }
    fn GetCharsetInfo(&self) -> (String, String) {
        ("utf8mb4".to_owned(), "utf8mb4_bin".to_owned())
    }
    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        "utf8mb4_bin".to_owned()
    }
    fn GetBlockEncryptionMode(&self) -> String {
        "aes-128-ecb".to_owned()
    }
    fn GetSysdateIsNow(&self) -> bool {
        false
    }
    fn GetNoopFuncsMode(&self) -> i32 {
        0
    }
    fn Rng(&self) -> &exprctx::mathutil::MysqlRng {
        panic!("not used by metadata-only test")
    }
    fn IsUseCache(&self) -> bool {
        false
    }
    fn SetSkipPlanCache(&self, _reason: &str) {}
    fn AllocPlanColumnID(&self) -> i64 {
        1
    }
    fn IsInNullRejectCheck(&self) -> bool {
        false
    }
    fn IsConstantPropagateCheck(&self) -> bool {
        false
    }
    fn ConnectionID(&self) -> u64 {
        1
    }
    fn IsReadonlyUserVar(&self, _name: &str) -> bool {
        false
    }
}

/// 构造默认测试构建上下文。
fn context() -> TestBuildContext {
    TestBuildContext(TestEvalContext(EmptyUserVars))
}

/// 构造带字符集与排序规则的字段类型。
fn field_type(mysql_type: u8, charset_name: &str, collation: &str) -> types::FieldType {
    let mut result = *types::NewFieldType(mysql_type);
    result.SetCharset(charset_name.to_owned());
    result.SetCollate(collation.to_owned());
    result
}

/// 用给定字段类型构造列表达式。
fn column(field_type: types::FieldType) -> ExprBox {
    Box::new(Column::new(field_type, 1, 1, 0))
}

/// 断言结果为错误并取出 Error。
fn expect_error(result: Result<ExprBox, Error>) -> Error {
    match result {
        Ok(_) => panic!("expected collation error"),
        Err(error) => error,
    }
}

/// 新模式：字符集不匹配与未知排序规则名均报错。
#[test]
fn new_collation_rejects_charset_mismatch_and_unknown_name() {
    let ctx = context();
    let latin = column(field_type(mysql::TypeVarchar, "latin1", "latin1_bin"));
    let error = expect_error(SetCollationToExpression(&ctx, latin, "utf8mb4_bin", true));
    assert!(error.to_string().contains("utf8mb4_bin"));
    assert!(error.to_string().contains("latin1"));

    let text = column(field_type(mysql::TypeVarchar, "utf8mb4", "utf8mb4_bin"));
    let error = expect_error(SetCollationToExpression(
        &ctx,
        text,
        "no_such_collation",
        true,
    ));
    assert!(error.to_string().contains("no_such_collation"));
}

/// 列经 CAST 获得新 collate，原 FieldType 不被修改，coercibility 为显式。
#[test]
fn column_is_cast_without_mutating_its_original_type() {
    let ctx = context();
    let source_type = field_type(mysql::TypeVarchar, "utf8mb4", "utf8mb4_bin");
    let result = SetCollationToExpression(
        &ctx,
        column(source_type.clone()),
        "utf8mb4_unicode_ci",
        true,
    )
    .unwrap();

    assert!(result.as_scalar_function().is_some());
    assert_eq!(
        result.GetType(ctx.GetEvalCtx()).GetCollate(),
        "utf8mb4_unicode_ci"
    );
    assert_eq!(result.Coercibility(), CoercibilityExplicit);
    assert_eq!(source_type.GetCollate(), "utf8mb4_bin");
}

/// JSON 按 utf8mb4 校验并以 CAST 转为 LONGTEXT + 目标 collate。
#[test]
fn json_is_checked_as_utf8mb4_and_cast_to_longtext() {
    let ctx = context();
    let json_type = field_type(mysql::TypeJSON, "binary", "binary");
    let result =
        SetCollationToExpression(&ctx, column(json_type), "utf8mb4_unicode_ci", true).unwrap();

    let result_type = result.GetType(ctx.GetEvalCtx());
    assert_eq!(result_type.GetType(), mysql::TypeLongBlob);
    assert_eq!(result_type.GetCharset(), "utf8mb4");
    assert_eq!(result_type.GetCollate(), "utf8mb4_unicode_ci");
    assert_eq!(result.Coercibility(), CoercibilityExplicit);
}

/// ENUM/SET 列尚不支持 COLLATE。
#[test]
fn enum_and_set_columns_are_rejected() {
    let ctx = context();
    for mysql_type in [mysql::TypeEnum, mysql::TypeSet] {
        let expression = column(field_type(mysql_type, "utf8mb4", "utf8mb4_bin"));
        let error = expect_error(SetCollationToExpression(
            &ctx,
            expression,
            "utf8mb4_bin",
            true,
        ));
        assert!(error.to_string().contains("enum or set"));
    }
}

/// 旧模式跳过名称校验，常量就地改 collate 并同步 CharsetAndCollation。
#[test]
fn constant_is_updated_in_place_and_legacy_mode_keeps_go_behavior() {
    let ctx = context();
    let constant: ExprBox = Box::new(Constant::with_type(
        types::NewStringDatum("value".to_owned()),
        field_type(mysql::TypeVarchar, "utf8mb4", "utf8mb4_bin"),
    ));
    let result = SetCollationToExpression(&ctx, constant, "legacy_custom_ci", false).unwrap();

    assert!(result.as_constant().is_some());
    assert_eq!(
        result.GetType(ctx.GetEvalCtx()).GetCollate(),
        "legacy_custom_ci"
    );
    assert_eq!(result.Coercibility(), CoercibilityExplicit);
    assert_eq!(
        result.CharsetAndCollation(),
        ("utf8mb4".to_owned(), "legacy_custom_ci".to_owned())
    );
}

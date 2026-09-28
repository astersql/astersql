// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// 表列查找、类型转换、默认值与 DESC 描述等行为的单测。

use std::sync::Arc;

use chrono::{TimeZone, Utc};
use chrono_tz::Tz;
use collate_test as collate;
use contextutil_test::{SQLWarn, WarnAppender, WarnHandler};
use exprctx_test::{
    OptionalEvalPropKey, OptionalEvalPropKeySet, OptionalEvalPropProvider, ParamError, ParamValues,
    UserVarsReader,
};
use expression_dependency::{BuildContext, EvalContext};
use mathutil_test as mathutil;
use model_dependency as model;
use parser_mysql_dependency::charset as mysql_charset;
use parser_mysql_dependency::r#const as sqlmode;
use types::mysql;
use types_dependency as types_root;
use types_dependency::datum as types;

use super::column::*;
use crate::{CheckRowConstraint, Constraint};

#[test]
/// Go parity: a nil reconstruction constructor falls back to the stored AST.
fn clonable_expression_without_constructor_returns_internal_node() {
    let internal = parser_ast_dependency::ExprNode::Value("fallback".to_owned());
    let node = NewClonableExprNode(None, internal.clone());

    assert_eq!(node.Clone(), internal);
}

#[derive(Default)]
/// 空用户变量读取器桩。
struct EmptyUserVars;

impl UserVarsReader for EmptyUserVars {
    fn GetUserVarVal(&self, _name: &str) -> Option<types::Datum> {
        None
    }

    fn GetUserVarType(&self, _name: &str) -> Option<types::FieldType> {
        None
    }

    fn Clone(&self) -> Box<dyn UserVarsReader> {
        Box::new(Self)
    }
}

/// 测试用求值上下文，可配置类型上下文、SQL Mode 与警告。
struct TestEvalContext {
    /// 类型转换上下文。
    type_context: types::Context,
    /// 错误/警告上下文。
    error_context: errctx_dependency::errctx::Context,
    /// SQL Mode。
    sql_mode: sqlmode::SQLMode,
    /// 静态警告收集器。
    warnings: contextutil_test::StaticWarnHandler,
    /// 用户变量。
    user_vars: EmptyUserVars,
    /// 固定的“当前时间”。
    now: chrono::DateTime<Tz>,
}

impl WarnAppender for TestEvalContext {
    fn AppendWarning(&self, error: contextutil_test::errors::SharedError) {
        self.warnings.AppendWarning(error);
    }

    fn AppendNote(&self, error: contextutil_test::errors::SharedError) {
        self.warnings.AppendNote(error);
    }
}

impl WarnHandler for TestEvalContext {
    fn WarningCount(&self) -> usize {
        self.warnings.WarningCount()
    }

    fn TruncateWarnings(&self, start: isize) -> Vec<SQLWarn> {
        self.warnings.TruncateWarnings(start)
    }

    fn CopyWarnings(&self, destination: Vec<SQLWarn>) -> Vec<SQLWarn> {
        self.warnings.CopyWarnings(destination)
    }
}

impl ParamValues for TestEvalContext {
    fn GetParamValue(&self, _index: usize) -> Result<types::Datum, ParamError> {
        Err(ParamError::IndexExceedsParamCount)
    }
}

impl EvalContext for TestEvalContext {
    fn CtxID(&self) -> u64 {
        42
    }

    fn SQLMode(&self) -> sqlmode::SQLMode {
        self.sql_mode
    }

    fn TypeCtx(&self) -> types::Context {
        self.type_context.clone()
    }

    fn ErrCtx(&self) -> errctx_dependency::errctx::Context {
        self.error_context.clone()
    }

    fn Location(&self) -> Tz {
        self.type_context.Location()
    }

    fn CurrentTime(&self) -> Result<chrono::DateTime<Tz>, contextutil_test::errors::SharedError> {
        Ok(self.now)
    }

    fn CurrentDB(&self) -> String {
        "test".to_owned()
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

    fn GetUserVarsReader(&self) -> &dyn UserVarsReader {
        &self.user_vars
    }

    fn GetOptionalPropSet(&self) -> OptionalEvalPropKeySet {
        OptionalEvalPropKeySet::default()
    }

    fn GetOptionalPropProvider(
        &self,
        _key: OptionalEvalPropKey,
    ) -> Option<&dyn OptionalEvalPropProvider> {
        None
    }
}

/// 测试用表达式构建上下文。
struct TestBuildContext {
    /// 嵌入的求值上下文。
    eval: TestEvalContext,
    /// 可复现随机数。
    rng: Box<mathutil::MysqlRng>,
}

impl BuildContext for TestBuildContext {
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        &self.eval
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

    fn Rng(&self) -> &mathutil::MysqlRng {
        &self.rng
    }

    fn IsUseCache(&self) -> bool {
        true
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
        42
    }

    fn IsReadonlyUserVar(&self, _name: &str) -> bool {
        false
    }
}

/// 构造可配置严格模式、时区与无默认值错误级别的测试上下文。
fn test_context(
    // 是否严格模式。
    strict: bool,
    location: Tz,
    no_default_level: errctx_dependency::errctx::Level,
) -> TestBuildContext {
    let type_handler: Arc<dyn contextutil_test::WarnAppender + Send + Sync> =
        Arc::new(contextutil_test::ignoreWarn {});
    let type_context = types_root::scalar::NewContext(types::Flags(0), location, type_handler);
    let mut levels =
        [errctx_dependency::errctx::Level::LevelError; errctx_dependency::errctx::errGroupCount];
    levels[errctx_dependency::errctx::ErrGroup::ErrGroupNoDefault as usize] = no_default_level;
    let error_handler: errctx_dependency::errctx::WarnAppenderRef =
        Arc::new(contextutil_test::ignoreWarn {});
    let error_context = errctx_dependency::errctx::NewContextWithLevels(levels, error_handler);
    let now = location.from_utc_datetime(
        &Utc.with_ymd_and_hms(2026, 7, 18, 1, 2, 3)
            .unwrap()
            .naive_utc(),
    );
    TestBuildContext {
        eval: TestEvalContext {
            type_context,
            error_context,
            sql_mode: if strict {
                sqlmode::ModeStrictTransTables
            } else {
                sqlmode::ModeNone
            },
            warnings: contextutil_test::NewStaticWarnHandler(0),
            user_vars: EmptyUserVars,
            now,
        },
        rng: mathutil::NewWithSeed(1),
    }
}

/// 创建 Public 状态的测试列。
fn new_col(name: &str) -> Arc<Column> {
    let mut info = Box::new(model::ColumnInfo::default());
    info.Name = model::ast::NewCIStr(name);
    info.State = model::StatePublic;
    ToColumn(info)
}

/// 取得列元数据的可变引用（要求唯一所有权）。
fn info_mut(column: &mut Arc<Column>) -> &mut model::ColumnInfo {
    Arc::get_mut(column)
        .expect("test column must have unique ownership")
        .ColumnInfo
        .as_mut()
}

/// 断言转换失败并取出 CastError。
fn expect_cast_error(result: CastResult) -> CastError {
    match result {
        Ok(_) => panic!("expected cast error"),
        Err(error) => error,
    }
}

/// 按列排序规则比较两个 Datum 是否相等。
fn assert_datum_equal(
    actual: &types::Datum,
    expected: &types::Datum,
    field_type: &types::FieldType,
) {
    assert_eq!(actual.Kind(), expected.Kind());
    let comparer = collate::GetCollator(field_type.GetCollate());
    let compared = actual
        .Compare(
            types::DefaultStmtNoWarningContext.clone(),
            expected,
            comparer.as_ref(),
        )
        .unwrap();
    assert_eq!(compared, 0);
}

#[derive(Default)]
/// 测试用 handle 标志实现。
struct HandleFlags {
    /// 是否整型主键 handle。
    pk_is_handle: bool,
    /// 是否聚簇索引 common handle。
    common_handle: bool,
}

impl HandleTableInfo for HandleFlags {
    fn PKIsHandle(&self) -> bool {
        self.pk_is_handle
    }

    fn IsCommonHandle(&self) -> bool {
        self.common_handle
    }
}

#[test]
/// 覆盖类型描述字符串、主键 handle 判定与 String() 输出。
pub fn TestString() {
    let mut column = new_col("");
    let info = info_mut(&mut column);
    info.SetType(mysql::TypeTiny);
    info.SetFlen(2);
    info.SetDecimal(1);
    info.SetCharset(mysql::DefaultCharset.to_owned());
    info.SetCollate(mysql::DefaultCollationName.to_owned());
    info.AddFlag(
        mysql::ZerofillFlag
            | mysql::UnsignedFlag
            | mysql::BinaryFlag
            | mysql::AutoIncrementFlag
            | mysql::NotNullFlag,
    );

    assert_eq!(
        column.ToInfo().GetTypeDesc(),
        "tinyint(2) unsigned zerofill"
    );
    let mut table = HandleFlags::default();
    assert!(!column.IsPKHandleColumn(&table));
    table.pk_is_handle = true;
    info_mut(&mut column).AddFlag(mysql::PriKeyFlag);
    assert!(column.IsPKHandleColumn(&table));
    assert!(!column.String().is_empty());

    let info = info_mut(&mut column);
    info.SetType(mysql::TypeEnum);
    info.SetFlag(0);
    info.SetElems(vec!["a".to_owned(), "b".to_owned()]);
    assert_eq!(info.GetTypeDesc(), "enum('a','b')");
    info.SetElems(vec!["'a'".to_owned(), "b".to_owned()]);
    assert_eq!(info.GetTypeDesc(), "enum('''a''','b')");

    info.SetType(mysql::TypeFloat);
    info.SetFlen(8);
    info.SetDecimal(-1);
    assert_eq!(info.GetTypeDesc(), "float");
    info.SetDecimal(1);
    assert_eq!(info.GetTypeDesc(), "float(8,1)");

    info.SetType(mysql::TypeDatetime);
    info.SetDecimal(6);
    assert_eq!(info.GetTypeDesc(), "datetime(6)");
    info.SetDecimal(0);
    assert_eq!(info.GetTypeDesc(), "datetime");
    info.SetDecimal(-1);
    assert_eq!(info.GetTypeDesc(), "datetime");
}

#[test]
/// 覆盖 FindCols / FindOnUpdateCols。
pub fn TestFind() {
    let mut columns = vec![new_col("a"), new_col("b"), new_col("c")];
    let (found, missing) = FindCols(&columns, &["a".to_owned()], true);
    let found = found.unwrap();
    assert_eq!(found.len(), 1);
    assert!(Arc::ptr_eq(&found[0], &columns[0]));
    assert_eq!(missing, "");
    drop(found);

    let (found, missing) = FindCols(&columns, &["d".to_owned()], true);
    assert!(found.is_none());
    assert_eq!(missing, "d");

    info_mut(&mut columns[0]).AddFlag(mysql::OnUpdateNowFlag);
    let updated = FindOnUpdateCols(&columns);
    assert_eq!(updated.len(), 1);
    assert!(Arc::ptr_eq(&updated[0], &columns[0]));
}

/// 对一行按列偏移批量做非空检查。
fn check_not_null(
    columns: &[Arc<Column>],
    row: &[types::Datum],
) -> Result<(), types::errors::Error> {
    for column in columns {
        column.CheckNotNull(&row[column.ToInfo().Offset as usize], 0)?;
    }
    Ok(())
}

#[test]
/// 覆盖 CheckOnce 重复名与 CheckNotNull。
pub fn TestCheck() {
    let mut column = new_col("a");
    info_mut(&mut column).SetFlag(mysql::AutoIncrementFlag);
    let mut columns = vec![Arc::clone(&column), Arc::clone(&column)];
    assert!(CheckOnce(&columns).is_err());
    columns.truncate(1);

    let row = vec![types::Datum::default()];
    assert!(check_not_null(&columns, &row).is_ok());
    drop(column);
    info_mut(&mut columns[0]).AddFlag(mysql::NotNullFlag);
    assert!(check_not_null(&columns, &row).is_err());
    assert!(CheckOnce(&[]).is_ok());
}

#[test]
/// 覆盖 HandleBadNull：可空、严格报错与警告降级。
pub fn TestHandleBadNull() {
    use errctx_dependency::errctx::{ErrGroup, Level};

    let mut column = new_col("a");
    let strict = errctx_dependency::errctx::StrictNoWarningContext.clone();
    let mut datum = types::Datum::default();
    assert!(column.HandleBadNull(strict.clone(), &mut datum, 0).is_ok());
    assert!(datum.IsNull());

    info_mut(&mut column).AddFlag(mysql::NotNullFlag);
    assert!(
        column
            .HandleBadNull(strict.clone(), &mut types::Datum::default(), 0)
            .is_err()
    );
    let warning = strict.WithErrGroupLevel(ErrGroup::ErrGroupBadNull, Level::LevelWarn);
    assert!(
        column
            .HandleBadNull(warning, &mut types::Datum::default(), 0)
            .is_ok()
    );
}

#[test]
/// 覆盖 NewColDesc 的 Null/Key/Extra 与字段名列表。
pub fn TestDesc() {
    let mut column = new_col("a");
    info_mut(&mut column)
        .SetFlag(mysql::AutoIncrementFlag | mysql::NotNullFlag | mysql::PriKeyFlag);
    let desc = NewColDesc(&column);
    assert_eq!(desc.Null, "NO");
    assert_eq!(desc.Key, "PRI");
    assert_eq!(desc.Extra, "auto_increment");

    info_mut(&mut column).SetFlag(mysql::MultipleKeyFlag);
    assert_eq!(NewColDesc(&column).Key, "MUL");

    info_mut(&mut column).SetFlag(mysql::UniqueKeyFlag | mysql::OnUpdateNowFlag);
    assert_eq!(
        NewColDesc(&column).Extra,
        "DEFAULT_GENERATED on update CURRENT_TIMESTAMP"
    );

    let info = info_mut(&mut column);
    info.SetFlag(0);
    info.GeneratedExprString = "test".to_owned();
    info.GeneratedStored = true;
    assert_eq!(NewColDesc(&column).Extra, "STORED GENERATED");
    info_mut(&mut column).GeneratedStored = false;
    assert_eq!(NewColDesc(&column).Extra, "VIRTUAL GENERATED");
    assert_eq!(ColDescFieldNames(false).len(), 6);
    assert_eq!(ColDescFieldNames(true).len(), 9);
}

#[test]
/// 覆盖各 MySQL 类型的零值 Datum。
pub fn TestGetZeroValue() {
    let mut unsigned = *types::NewFieldType(mysql::TypeLonglong);
    unsigned.SetFlag(mysql::UnsignedFlag);
    let mut binary = *types::NewFieldType(mysql::TypeString);
    binary.SetFlen(2);
    binary.SetCharset("binary".to_owned());
    binary.SetCollate("binary".to_owned());
    let mut utf8 = *types::NewFieldType(mysql::TypeString);
    utf8.SetFlen(2);
    utf8.SetCharset("utf8mb4".to_owned());
    utf8.SetCollate("binary".to_owned());

    let cases = vec![
        (*types::NewFieldType(mysql::TypeLong), types::NewIntDatum(0)),
        (unsigned, types::NewUintDatum(0)),
        (
            *types::NewFieldType(mysql::TypeFloat),
            types::NewFloat32Datum(0.0),
        ),
        (
            *types::NewFieldType(mysql::TypeDouble),
            types::NewFloat64Datum(0.0),
        ),
        (
            *types::NewFieldType(mysql::TypeNewDecimal),
            types::NewDecimalDatum(types::MyDecimal::default()),
        ),
        (
            *types::NewFieldType(mysql::TypeVarchar),
            types::NewStringDatum(String::new()),
        ),
        (
            *types::NewFieldType(mysql::TypeBlob),
            types::NewStringDatum(String::new()),
        ),
        (
            *types::NewFieldType(mysql::TypeDuration),
            types::NewDurationDatum(types::ZeroDuration),
        ),
        (
            *types::NewFieldType(mysql::TypeDatetime),
            types::NewTimeDatum(types::NewTime(
                types::CoreTime::default(),
                mysql::TypeDatetime,
                types::DefaultFsp,
            )),
        ),
        (
            *types::NewFieldType(mysql::TypeTimestamp),
            types::NewTimeDatum(types::NewTime(
                types::CoreTime::default(),
                mysql::TypeTimestamp,
                types::DefaultFsp,
            )),
        ),
        (
            *types::NewFieldType(mysql::TypeDate),
            types::NewTimeDatum(types::NewTime(
                types::CoreTime::default(),
                mysql::TypeDate,
                types::DefaultFsp,
            )),
        ),
        (
            *types::NewFieldType(mysql::TypeBit),
            types::NewMysqlBitDatum(types::BinaryLiteral(Vec::new())),
        ),
        (
            *types::NewFieldType(mysql::TypeSet),
            types::NewMysqlSetDatum(types::Set::default(), String::new()),
        ),
        (
            *types::NewFieldType(mysql::TypeEnum),
            types::NewMysqlEnumDatum(types::Enum::default()),
        ),
        (binary, types::NewBytesDatum(vec![0; 2])),
        (utf8, types::NewStringDatum(String::new())),
        (
            *types::NewFieldType(mysql::TypeJSON),
            types::NewJSONDatum(types::BinaryJSON::default()),
        ),
    ];

    for (field_type, expected) in cases {
        let mut info = Box::new(model::ColumnInfo::default());
        info.FieldType = field_type.clone();
        let column = ToColumn(info);
        let actual = GetZeroValue(&column);
        assert_datum_equal(&actual, &expected, &field_type);
    }
}

#[test]
/// 覆盖整数/字符串转换、字符集非法字节与 Incorrect string value。
pub fn TestCastValue() {
    let context = test_context(
        true,
        chrono_tz::UTC,
        errctx_dependency::errctx::Level::LevelError,
    );
    let mut integer = model::ColumnInfo::default();
    integer.FieldType = *types::NewFieldType(mysql::TypeLong);
    integer.State = model::StatePublic;
    integer.SetCharset(mysql_charset::UTF8Charset.to_owned());

    let value = CastValue(
        &context.eval,
        types::Datum::default(),
        &integer,
        false,
        false,
    )
    .unwrap();
    assert_eq!(value.GetInt64(), 0);
    let error = expect_cast_error(CastValue(
        &context.eval,
        types::NewStringDatum("test".to_owned()),
        &integer,
        false,
        false,
    ));
    assert_eq!(error.casted().GetInt64(), 0);

    let mut string = model::ColumnInfo::default();
    string.FieldType = *types::NewFieldType(mysql::TypeString);
    string.State = model::StatePublic;
    assert!(
        CastValue(
            &context.eval,
            types::NewStringDatum("test".to_owned()),
            &string,
            false,
            false,
        )
        .is_ok()
    );

    // utf8mb3 拒绝 4 字节字符；force_ignore_truncate 可放行。
    string.SetCharset(mysql_charset::UTF8Charset.to_owned());
    let utf8mb3 = types::NewBytesDatum(vec![0xf0, 0x9f, 0x8c, 0x80]);
    assert!(CastValue(&context.eval, utf8mb3.clone(), &string, false, false).is_err());
    assert!(CastValue(&context.eval, utf8mb3, &string, false, true).is_ok());

    string.SetCharset(mysql_charset::UTF8MB4Charset.to_owned());
    let invalid_utf8 = types::NewBytesDatum(vec![0xf0, 0x9f, 0x80]);
    assert!(CastValue(&context.eval, invalid_utf8.clone(), &string, false, false).is_err());
    assert!(CastValue(&context.eval, invalid_utf8, &string, false, true).is_ok());

    string.SetCharset("ascii".to_owned());
    let invalid_ascii = types::NewBytesDatum(vec![0x32, 0xf0]);
    assert!(CastValue(&context.eval, invalid_ascii.clone(), &string, false, false).is_err());
    assert!(CastValue(&context.eval, invalid_ascii, &string, false, true).is_ok());

    string.SetCharset("utf8mb4".to_owned());
    string.SetCollate("utf8mb4_general_ci".to_owned());
    let valid = CastValue(
        &context.eval,
        types::NewBinaryLiteralDatum(types::BinaryLiteral(vec![0xE5, 0xA5, 0xBD])),
        &string,
        false,
        false,
    )
    .unwrap();
    assert_eq!(valid.Collation(), "utf8mb4_general_ci");
    for input in [
        types::NewBinaryLiteralDatum(types::BinaryLiteral(vec![0xE5, 0xA5, 0xBD, 0x81])),
        types::NewBytesDatum(vec![0xE5, 0xA5, 0xBD, 0x81]),
    ] {
        let error = expect_cast_error(CastValue(&context.eval, input, &string, false, false));
        assert!(error.to_string().contains("Incorrect string value '\\x81'"));
        assert_eq!(error.casted().Collation(), "utf8mb4_general_ci");
    }
}

/// GetColDefaultValue 用例描述。
struct DefaultCase {
    /// 列元数据。
    info: model::ColumnInfo,
    strict: bool,
    /// 期望默认值。
    expected: types::Datum,
    /// 是否期望报错。
    expect_error: bool,
}

/// 测试桩：仅支持简单常量表达式构建。
fn test_build_simple_expr<'a>(
    _context: &dyn BuildContext,
    expression: &parser_ast_dependency::ExprNode,
    options: Vec<expression_dependency::BuildOption<'a>>,
) -> Result<expression_dependency::ExprBox, types::errors::Error> {
    let mut build_options = expression_dependency::BuildOptions::default();
    for option in options {
        option(&mut build_options);
    }
    match &expression.Kind {
        parser_ast_dependency::ExprKind::Value(value) => match &value.Datum {
            parser_ast_dependency::ValueDatum::Null => {
                Ok(Box::new(expression_dependency::NewNull()))
            }
            parser_ast_dependency::ValueDatum::Bool(value) => Ok(Box::new(
                expression_dependency::NewInt64Const(i64::from(*value)),
            )),
            parser_ast_dependency::ValueDatum::Int64(value) => {
                Ok(Box::new(expression_dependency::NewInt64Const(*value)))
            }
            parser_ast_dependency::ValueDatum::Uint64(value) => Ok(Box::new(
                expression_dependency::NewUInt64ConstWithFieldType(
                    *value,
                    *types::NewFieldType(mysql::TypeLonglong),
                ),
            )),
            _ => Err(types::errors::New(
                "unsupported test constant expression".to_owned(),
            )),
        },
        _ => Err(types::errors::New("unsupported test expression".to_owned())),
    }
}

/// 安装测试用 BuildSimpleExpr。
fn install_test_build_simple_expr() {
    expression_dependency::InstallBuildSimpleExpr(test_build_simple_expr).unwrap();
}

/// 快速构造带 origin/default 的 ColumnInfo。
fn default_info(
    field_type: types::FieldType,
    origin: Option<model::DefaultValue>,
    value: Option<model::DefaultValue>,
) -> model::ColumnInfo {
    model::ColumnInfo {
        FieldType: field_type,
        OriginDefaultValue: origin,
        DefaultValue: value,
        ..Default::default()
    }
}

#[test]
/// 覆盖可空/非空/枚举/时间戳 UTC 转换/表达式默认值与严格模式。
pub fn TestGetDefaultValue() {
    install_test_build_simple_expr();
    let location = chrono_tz::America::Los_Angeles;
    let nullable = *types::NewFieldType(mysql::TypeLonglong);
    let mut not_null = nullable.clone();
    not_null.SetFlag(mysql::NotNullFlag);
    let mut enum_type = *types::NewFieldType(mysql::TypeEnum);
    enum_type.SetFlag(mysql::NotNullFlag);
    enum_type.SetElems(vec!["abc".to_owned(), "def".to_owned()]);
    enum_type.SetCollate(mysql::DefaultCollationName.to_owned());
    let mut timestamp = *types::NewFieldType(mysql::TypeTimestamp);
    timestamp.SetFlag(mysql::TimestampFlag);
    let mut auto_increment = not_null.clone();
    auto_increment.AddFlag(mysql::AutoIncrementFlag);

    let local_time = types::NewTime(
        types::FromDate(2019, 5, 6, 12, 48, 49, 0),
        mysql::TypeTimestamp,
        types::DefaultFsp,
    );
    let mut utc_time = local_time;
    utc_time.ConvertTimeZone(location, chrono_tz::UTC).unwrap();

    let mut versioned = default_info(
        timestamp.clone(),
        Some(model::DefaultValue::String(utc_time.String().into_bytes())),
        Some(model::DefaultValue::String(utc_time.String().into_bytes())),
    );
    versioned.Version = model::ColumnInfoVersion2;
    let mut expression = default_info(
        not_null.clone(),
        None,
        Some(model::DefaultValue::String(b"1".to_vec())),
    );
    expression.DefaultIsExpr = true;

    let cases = vec![
        DefaultCase {
            info: default_info(
                not_null.clone(),
                Some(model::DefaultValue::Float(1.0)),
                Some(model::DefaultValue::Float(1.0)),
            ),
            strict: false,
            expected: types::NewIntDatum(1),
            expect_error: false,
        },
        DefaultCase {
            info: default_info(not_null.clone(), None, None),
            strict: false,
            expected: types::NewIntDatum(0),
            expect_error: false,
        },
        DefaultCase {
            info: default_info(nullable.clone(), None, None),
            strict: false,
            expected: types::Datum::default(),
            expect_error: false,
        },
        DefaultCase {
            info: default_info(enum_type, None, None),
            strict: false,
            expected: types::NewCollateMysqlEnumDatum(
                types::Enum {
                    Name: "abc".to_owned(),
                    Value: 1,
                },
                mysql::DefaultCollationName.to_owned(),
            ),
            expect_error: false,
        },
        DefaultCase {
            info: default_info(
                timestamp.clone(),
                Some(model::DefaultValue::String(b"0000-00-00 00:00:00".to_vec())),
                Some(model::DefaultValue::String(b"0000-00-00 00:00:00".to_vec())),
            ),
            strict: false,
            expected: types::NewTimeDatum(types::NewTime(
                types::CoreTime::default(),
                mysql::TypeTimestamp,
                types::DefaultFsp,
            )),
            expect_error: false,
        },
        DefaultCase {
            info: versioned,
            strict: true,
            expected: types::NewTimeDatum(local_time),
            expect_error: false,
        },
        DefaultCase {
            info: default_info(
                timestamp,
                Some(model::DefaultValue::String(b"not valid date".to_vec())),
                Some(model::DefaultValue::String(b"not valid date".to_vec())),
            ),
            strict: true,
            expected: types::Datum::default(),
            expect_error: true,
        },
        DefaultCase {
            info: default_info(not_null.clone(), None, None),
            strict: true,
            expected: types::Datum::default(),
            expect_error: true,
        },
        DefaultCase {
            info: default_info(auto_increment, None, None),
            strict: true,
            expected: types::NewIntDatum(0),
            expect_error: false,
        },
        DefaultCase {
            info: expression,
            strict: false,
            expected: types::NewIntDatum(1),
            expect_error: false,
        },
    ];

    // 先测当前默认值，再测 origin 默认值（跳过表达式默认）。
    for case in &cases {
        let context = test_context(
            case.strict,
            location,
            if case.strict {
                errctx_dependency::errctx::Level::LevelError
            } else {
                errctx_dependency::errctx::Level::LevelWarn
            },
        );
        match GetColDefaultValue(&context, &ToColumn(Box::new(case.info.clone()))) {
            Ok(actual) => {
                assert!(!case.expect_error);
                assert_datum_equal(&actual, &case.expected, &case.info.FieldType);
            }
            Err(_) => assert!(case.expect_error),
        }
    }

    for case in &cases {
        let context = test_context(
            case.strict,
            location,
            if case.strict {
                errctx_dependency::errctx::Level::LevelError
            } else {
                errctx_dependency::errctx::Level::LevelWarn
            },
        );
        let result = GetColOriginDefaultValue(&context, &ToColumn(Box::new(case.info.clone())));
        if case.info.DefaultIsExpr {
            continue;
        }
        match result {
            Ok(actual) => {
                assert!(!case.expect_error);
                assert_datum_equal(&actual, &case.expected, &case.info.FieldType);
            }
            Err(_) => assert!(case.expect_error),
        }
    }
}

#[test]
/// 行级 CHECK：真值/NULL 通过，假值与非法表达式失败。
fn check_row_constraint_uses_formal_expression_evaluation() {
    install_test_build_simple_expr();
    let context = test_context(
        true,
        chrono_tz::UTC,
        errctx_dependency::errctx::Level::LevelError,
    );
    let table_info = model::TableInfo::default();
    let constraint = |name: &str, expression: &str| {
        Arc::new(Constraint {
            ConstraintInfo: Box::new(model::ConstraintInfo {
                Name: parser_ast_dependency::NewCIStr(name),
                ExprString: expression.to_owned(),
                ..Default::default()
            }),
        })
    };

    assert!(
        CheckRowConstraint(
            &context,
            &[constraint("positive", "1")],
            chunk_dependency::Row::default(),
            &table_info,
        )
        .is_ok()
    );
    assert!(
        CheckRowConstraint(
            &context,
            &[constraint("nullable", "NULL")],
            chunk_dependency::Row::default(),
            &table_info,
        )
        .is_ok()
    );

    let violation = CheckRowConstraint(
        &context,
        &[constraint("must_be_true", "0")],
        chunk_dependency::Row::default(),
        &table_info,
    )
    .unwrap_err();
    assert!(violation.to_string().contains("must_be_true"));

    assert!(
        CheckRowConstraint(
            &context,
            &[constraint("invalid", "1 +")],
            chunk_dependency::Row::default(),
            &table_info,
        )
        .is_err()
    );
}

#[test]
/// 严格模式 CastColumnValueWithStrictMode：无符号、截断与尾空格。
pub fn TestCastValueStrict() {
    let mut unsigned = *types::NewFieldType(mysql::TypeLonglong);
    unsigned.AddFlag(mysql::UnsignedFlag);
    let error = expect_cast_error(CastColumnValueWithStrictMode(
        types::NewIntDatum(-1),
        &unsigned,
    ));
    assert_eq!(error.casted().GetUint64(), 0);

    let signed = *types::NewFieldType(mysql::TypeLonglong);
    assert_eq!(
        CastColumnValueWithStrictMode(types::NewIntDatum(1), &signed)
            .unwrap()
            .GetInt64(),
        1
    );

    let int_type = *types::NewFieldType(mysql::TypeLong);
    let error = expect_cast_error(CastColumnValueWithStrictMode(
        types::NewIntDatum(1_i64 << 40),
        &int_type,
    ));
    assert_eq!(error.casted().GetInt64(), i32::MAX as i64);

    assert_eq!(
        CastColumnValueWithStrictMode(types::NewIntDatum(1_i64 << 16), &signed)
            .unwrap()
            .GetInt64(),
        1_i64 << 16
    );

    let mut char_two = *types::NewFieldType(mysql::TypeString);
    char_two.SetFlen(2);
    let error = expect_cast_error(CastColumnValueWithStrictMode(
        types::NewStringDatum("abcd".to_owned()),
        &char_two,
    ));
    assert_eq!(error.casted().GetString(), "ab");

    assert_eq!(
        CastColumnValueWithStrictMode(types::NewStringDatum("a   ".to_owned()), &char_two,)
            .unwrap()
            .GetString(),
        "a"
    );
}

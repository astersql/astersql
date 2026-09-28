// Copyright 2026 AsterSQL.

// table 列相关能力的 Aster 迁移单元测试。
//
// 覆盖可克隆表达式节点、列身份与标志格式化、列查找辅助、
// 严格模式下的列值转换（cast）及截断/非法字符等错误形态，
// 行为对齐 Go `pkg/table` 中对应列测试。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::column::*;
use model_dependency as model;
use parser_ast_dependency::ExprNode;
use types::mysql;
use types_dependency::datum as types;

/// 测试用类型转换上下文，固定 ConnectionID 并携带 SQL Mode。
#[derive(Clone)]
struct TestCastContext {
    /// Datum 类型转换用的类型上下文。
    type_context: types::Context,
    /// 错误分级上下文（严格/告警等）。
    error_context: errctx_dependency::errctx::Context,
    /// 当前会话 SQL Mode 位图。
    sql_mode: parser_mysql_dependency::r#const::SQLMode,
}

impl CastContext for TestCastContext {
    fn TypeCtx(&self) -> types::Context {
        self.type_context.clone()
    }

    fn ErrCtx(&self) -> errctx_dependency::errctx::Context {
        self.error_context.clone()
    }

    fn SQLMode(&self) -> parser_mysql_dependency::r#const::SQLMode {
        self.sql_mode
    }

    fn ConnectionID(&self) -> u64 {
        42
    }
}

/// 构造带严格无告警错误上下文的转换上下文。
fn strict_cast_context(sql_mode: parser_mysql_dependency::r#const::SQLMode) -> TestCastContext {
    TestCastContext {
        type_context: types::DefaultStmtNoWarningContext
            .clone()
            .WithFlags(types::Flags(0)),
        error_context: errctx_dependency::errctx::StrictNoWarningContext.clone(),
        sql_mode,
    }
}

/// 断言转换结果为错误并返回该错误。
fn expect_cast_error(result: CastResult) -> CastError {
    match result {
        Ok(_) => panic!("expected cast error"),
        Err(error) => error,
    }
}

/// 由列名与字段类型构造测试用 `Column`。
fn column(name: &str, field_type: types::FieldType) -> Arc<Column> {
    let mut info = Box::new(model::ColumnInfo::default());
    info.Name = model::ast::NewCIStr(name);
    info.FieldType = field_type;
    ToColumn(info)
}

/// 可克隆表达式：Clone 走构造器重建 AST，Internal 保留原始节点。
#[test]
fn clonable_expression_reconstructs_and_retains_internal_node() {
    let calls = Arc::new(AtomicUsize::new(0));
    let captured = Arc::clone(&calls);
    let internal = ExprNode::Value("internal".to_owned());
    let node = NewClonableExprNode(
        Some(Arc::new(move || {
            captured.fetch_add(1, Ordering::SeqCst);
            ExprNode::Value("clone".to_owned())
        })),
        internal.clone(),
    );

    assert_eq!(node.Clone(), ExprNode::Value("clone".to_owned()));
    assert_eq!(node.Clone(), ExprNode::Value("clone".to_owned()));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(node.Internal(), internal);
}

/// 缺少重建构造器时 Clone 与 Go 一致，回退到内部 AST。
#[test]
fn clonable_expression_without_constructor_uses_internal_node() {
    let internal = ExprNode::Value("fallback".to_owned());
    let node = NewClonableExprNode(None, internal.clone());
    assert_eq!(node.Clone(), internal);
}

/// 列使用正式 model 身份，String 输出含 AUTO_INCREMENT / NOT NULL 等标志。
#[test]
fn column_uses_formal_model_identity_and_formats_flags() {
    fn accepts_formal_model(_: &model::ColumnInfo) {}

    let mut field_type = *types::NewFieldType(mysql::TypeLonglong);
    field_type.AddFlag(mysql::AutoIncrementFlag | mysql::NotNullFlag);
    let column = column("AccountID", field_type);
    accepts_formal_model(column.ToInfo());

    let rendered = column.String();
    assert!(rendered.starts_with("AccountID "));
    assert!(rendered.contains("AUTO_INCREMENT"));
    assert!(rendered.ends_with("NOT NULL"));
}

/// FindCol / FindCols 等查找辅助：大小写、缺失偏移、额外 handle 列。
#[test]
fn find_helpers_preserve_identity_missing_offsets_and_extra_handle() {
    let first = column("FirstName", *types::NewFieldType(mysql::TypeVarString));
    let mut update_type = *types::NewFieldType(mysql::TypeTimestamp);
    update_type.AddFlag(mysql::OnUpdateNowFlag);
    let updated = column("updated_at", update_type);
    let columns = vec![Arc::clone(&first), Arc::clone(&updated)];

    assert!(Arc::ptr_eq(
        &FindCol(&columns, "firstname").unwrap(),
        &first
    ));
    assert!(FindColLowerCase(&columns, "FirstName").is_none());
    assert!(Arc::ptr_eq(
        &FindColLowerCase(&columns, "firstname").unwrap(),
        &first
    ));

    let requested = vec!["firstname".to_owned(), "missing".to_owned()];
    let (found, missing) = FindColumns(&columns, &requested, false);
    assert!(found.is_none());
    assert_eq!(missing, 1);

    // `_tidb_rowid` 可解析为 ExtraHandle；严格模式下缺失名直接报错。
    let requested = vec!["FirstName".to_owned(), "_tidb_rowid".to_owned()];
    let (found, missing) = FindCols(&columns, &requested, false);
    assert_eq!(missing, "");
    let found = found.unwrap();
    assert!(Arc::ptr_eq(&found[0], &first));
    assert_eq!(found[1].ToInfo().ID, model::ExtraHandleID);
    assert_eq!(found[1].ToInfo().Offset, columns.len() as isize);

    let (found, missing) = FindCols(&columns, &requested, true);
    assert!(found.is_none());
    assert_eq!(missing, "_tidb_rowid");

    let on_update = FindOnUpdateCols(&columns);
    assert_eq!(on_update.len(), 1);
    assert!(Arc::ptr_eq(&on_update[0], &updated));
}

/// 严格模式转换：合法字符串转整型，CHAR 仅裁尾部空格。
#[test]
fn strict_cast_converts_values_and_char_cast_trims_only_spaces() {
    let integer_type = *types::NewFieldType(mysql::TypeLonglong);
    let casted =
        CastColumnValueWithStrictMode(types::NewStringDatum("42".to_owned()), &integer_type)
            .unwrap();
    assert_eq!(casted.GetInt64(), 42);

    let error = CastColumnValueWithStrictMode(
        types::NewStringDatum("not-an-int".to_owned()),
        &integer_type,
    );
    assert!(error.is_err());

    let mut char_type = *types::NewFieldType(mysql::TypeString);
    char_type.SetFlen(16);
    let casted =
        CastColumnValueWithStrictMode(types::NewStringDatum("value   ".to_owned()), &char_type)
            .unwrap();
    assert_eq!(casted.GetString(), "value");
}

/// 非法字符串错误包装：保留未知错误，hex 转义 Incorrect string value。
#[test]
fn incorrect_string_conversion_preserves_unknown_errors_and_escapes_hex() {
    let original = types::errors::New("some other failure");
    assert_eq!(convertToIncorrectStringErr(original.clone(), "c"), original);

    let converted = convertToIncorrectStringErr(
        types::errors::New("invalid character string: F09F92A9"),
        "payload",
    );
    assert_eq!(
        converted.to_string(),
        "Incorrect string value '\\xF0\\x9F\\x92\\xA9' for column 'payload'"
    );
}

/// 严格转换错误保留 Go 侧部分转换值与错误种类。
#[test]
fn strict_cast_error_retains_the_go_partial_value_and_typed_kind() {
    let mut unsigned = *types::NewFieldType(mysql::TypeLonglong);
    unsigned.AddFlag(mysql::UnsignedFlag);
    let failure = expect_cast_error(CastColumnValueWithStrictMode(
        types::NewIntDatum(-1),
        &unsigned,
    ));
    assert_eq!(failure.kind(), CastErrorKind::Truncated);
    assert_eq!(failure.casted().GetUint64(), 0);

    let signed_int = *types::NewFieldType(mysql::TypeLong);
    let failure = expect_cast_error(CastColumnValueWithStrictMode(
        types::NewIntDatum(1_i64 << 40),
        &signed_int,
    ));
    assert_eq!(failure.kind(), CastErrorKind::Truncated);
    assert_eq!(failure.casted().GetInt64(), i32::MAX as i64);

    let mut char_two = *types::NewFieldType(mysql::TypeString);
    char_two.SetFlen(2);
    let failure = expect_cast_error(CastColumnValueWithStrictMode(
        types::NewStringDatum("abcd".to_owned()),
        &char_two,
    ));
    assert_eq!(failure.kind(), CastErrorKind::Truncated);
    assert_eq!(failure.casted().GetString(), "ab");
}

/// returnErr / forceIgnoreTruncate 路径下部分值与严格截断一致。
#[test]
fn return_error_and_force_ignore_truncate_keep_the_same_partial_value() {
    let mut char_two = *types::NewFieldType(mysql::TypeString);
    char_two.SetFlen(2);
    let column = column("payload", char_two);
    let context = strict_cast_context(parser_mysql_dependency::r#const::ModeNone);

    let failure = expect_cast_error(CastValue(
        &context,
        types::NewStringDatum("abcd".to_owned()),
        column.ToInfo(),
        true,
        false,
    ));
    assert_eq!(failure.kind(), CastErrorKind::Truncated);
    assert_eq!(failure.casted().GetString(), "ab");

    let casted = CastValue(
        &context,
        types::NewStringDatum("abcd".to_owned()),
        column.ToInfo(),
        false,
        true,
    )
    .unwrap();
    assert_eq!(casted.GetString(), "ab");
}

/// 严格 + NO_ZERO_DATE 对零时间戳报 WrongDatetime，值为零。
#[test]
fn strict_no_zero_date_reports_typed_error_with_zero_timestamp() {
    use parser_mysql_dependency::r#const::{ModeNoZeroDate, ModeStrictTransTables, SQLMode};

    let timestamp = *types::NewFieldType(mysql::TypeTimestamp);
    let column = column("created_at", timestamp);
    let context = strict_cast_context(SQLMode(ModeNoZeroDate.0 | ModeStrictTransTables.0));
    let failure = expect_cast_error(CastValue(
        &context,
        types::NewStringDatum("0000-00-00 00:00:00".to_owned()),
        column.ToInfo(),
        false,
        false,
    ));

    assert_eq!(failure.kind(), CastErrorKind::WrongDatetime);
    assert!(failure.casted().GetMysqlTime().IsZero());
}

/// 非严格或 insert-ignore 类错误级别下，零时间戳返回 Go 侧零值。
#[test]
fn zero_timestamp_warn_and_ignore_modes_return_the_go_zero_value() {
    use errctx_dependency::errctx::{ErrGroup, Level};
    use parser_mysql_dependency::r#const::{ModeNoZeroDate, ModeStrictTransTables, SQLMode};

    let timestamp = *types::NewFieldType(mysql::TypeTimestamp);
    let column = column("created_at", timestamp);
    let input = || types::NewStringDatum("0000-00-00 00:00:00".to_owned());

    let non_strict = strict_cast_context(ModeNoZeroDate);
    let casted = CastValue(&non_strict, input(), column.ToInfo(), false, false).unwrap();
    assert!(casted.GetMysqlTime().IsZero());

    let mut insert_ignore =
        strict_cast_context(SQLMode(ModeNoZeroDate.0 | ModeStrictTransTables.0));
    insert_ignore.error_context = insert_ignore
        .error_context
        .WithErrGroupLevel(ErrGroup::ErrGroupDupKey, Level::LevelWarn);
    let casted = CastValue(&insert_ignore, input(), column.ToInfo(), false, false).unwrap();
    assert!(casted.GetMysqlTime().IsZero());
}

/// 非法 UTF-8 字符错误保留合法前缀与校对规则。
#[test]
fn invalid_character_error_is_typed_and_retains_the_valid_prefix() {
    let mut utf8mb4 = *types::NewFieldType(mysql::TypeString);
    utf8mb4.SetCharset("utf8mb4".to_owned());
    utf8mb4.SetCollate("utf8mb4_general_ci".to_owned());
    let column = column("payload", utf8mb4);
    let context = strict_cast_context(parser_mysql_dependency::r#const::ModeNone);
    let input = types::NewBytesDatum(vec![0xE5, 0xA5, 0xBD, 0x81]);

    let failure = expect_cast_error(CastValue(
        &context,
        input.clone(),
        column.ToInfo(),
        false,
        false,
    ));
    assert_eq!(failure.kind(), CastErrorKind::InvalidCharacter);
    assert_eq!(failure.casted().GetString(), "好");
    assert_eq!(failure.casted().Collation(), "utf8mb4_general_ci");
    assert!(failure.to_string().contains("\\x81"));

    let ignored = CastValue(&context, input, column.ToInfo(), false, true).unwrap();
    assert_eq!(ignored.GetString(), "好");
    assert_eq!(ignored.Collation(), "utf8mb4_general_ci");
}

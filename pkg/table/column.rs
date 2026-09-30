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

// Copyright 2016 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

// The executable part of TiDB's table-column boundary.
//
// `model::ColumnInfo` remains the canonical metadata and field-type owner;
// `Column` only adds executable generated/default expression state.
//
// 表列的可执行边界：`model::ColumnInfo` 仍是元数据与字段类型的权威来源；
// `Column` 仅附加可执行的生成列（generated column）/默认值表达式状态。

use std::sync::Arc;
use std::{fmt, str};

use chunk_dependency as chunk;
use errctx_dependency::errctx::{Context as ErrorContext, ErrGroup, Level};
use expression_dependency::{BuildContext, EvalContext};
use model_dependency as model;
use parser_ast_dependency::ExprNode;
use parser_mysql_dependency::r#const::{ModeNone, SQLMode};
use types::mysql;
use types_dependency::datum as types;

/// 可重建 AST 表达式节点的构造器类型（线程安全）。
pub type ExprNodeCtor = Arc<dyn Fn() -> ExprNode + Send + Sync>;

/// Metadata and executable type information for one table column.
/// 单列表列的元数据与可执行类型信息。
#[derive(Clone)]
pub struct Column {
    /// 规范列元数据。
    pub ColumnInfo: Box<model::ColumnInfo>,
    /// 生成列表达式（若有）。
    pub GeneratedExpr: Option<Arc<ClonableExprNode>>,
    /// 默认值表达式 AST（若有）。
    pub DefaultExpr: Option<ExprNode>,
}

/// A reconstructable AST node.  `Arc` makes cloning the wrapper explicit while
/// each call to [`Clone`](ClonableExprNode::Clone) still constructs a fresh AST.
/// 可重建的 AST 节点包装：`Arc` 显式克隆包装，`Clone` 仍构造新 AST。
pub struct ClonableExprNode {
    /// 可选重建构造器。
    ctor: Option<ExprNodeCtor>,
    /// 内部持有的表达式节点。
    internal: ExprNode,
}

/// 构造可克隆表达式节点包装。
pub fn NewClonableExprNode(
    ctor: Option<ExprNodeCtor>,
    internal: ExprNode,
) -> Arc<ClonableExprNode> {
    Arc::new(ClonableExprNode { ctor, internal })
}

impl ClonableExprNode {
    /// 通过构造器重建一份新的表达式 AST。
    pub fn Clone(&self) -> ExprNode {
        self.ctor
            .as_ref()
            .map_or_else(|| self.internal.clone(), |ctor| ctor())
    }

    /// 返回内部表达式节点的克隆。
    pub fn Internal(&self) -> ExprNode {
        self.internal.clone()
    }
}

impl Column {
    /// 由列元数据构造可执行列包装。
    pub fn New(column_info: Box<model::ColumnInfo>) -> Arc<Self> {
        Arc::new(Self {
            ColumnInfo: column_info,
            GeneratedExpr: None,
            DefaultExpr: None,
        })
    }

    /// 返回列标志位。
    pub fn GetFlag(&self) -> usize {
        self.ColumnInfo.GetFlag()
    }

    /// 格式化列名、类型与常见标志（自增、非空）。
    pub fn String(&self) -> String {
        let mut parts = vec![
            self.ColumnInfo.Name.O.clone(),
            self.ColumnInfo.FieldType.CompactStr(),
        ];
        if mysql::HasAutoIncrementFlag(self.GetFlag()) {
            parts.push("AUTO_INCREMENT".to_owned());
        }
        if mysql::HasNotNullFlag(self.GetFlag()) {
            parts.push("NOT NULL".to_owned());
        }
        parts.join(" ")
    }

    /// Returns the canonical model object.  Callers must treat it as read-only,
    /// matching the Go API contract.
    /// 返回规范 model 对象；调用方应只读，对齐 Go 契约。
    pub fn ToInfo(&self) -> &model::ColumnInfo {
        &self.ColumnInfo
    }
}

/// 按列名（忽略大小写）查找列。
pub fn FindCol(cols: &[Arc<Column>], name: &str) -> Option<Arc<Column>> {
    cols.iter()
        .find(|column| column.ColumnInfo.Name.O.eq_ignore_ascii_case(name))
        .cloned()
}

/// 按小写列名精确查找列。
pub fn FindColLowerCase(cols: &[Arc<Column>], name: &str) -> Option<Arc<Column>> {
    cols.iter()
        .find(|column| column.ColumnInfo.Name.L == name)
        .cloned()
}

/// Converts canonical model metadata to the table wrapper.
/// 将规范 model 元数据转为表侧列包装。
pub fn ToColumn(column_info: Box<model::ColumnInfo>) -> Arc<Column> {
    Column::New(column_info)
}

/// TiDB 隐式行句柄列名（无显式主键时的 `_tidb_rowid`）。
const EXTRA_HANDLE_NAME: &str = "_tidb_rowid";

/// 构造额外 handle 列，offset 为列偏移。
fn extra_handle_column(offset: usize) -> Arc<Column> {
    let mut info = Box::new(model::NewExtraHandleColInfo());
    info.Offset = offset as isize;
    Column::New(info)
}

/// 按原始大小写不敏感名列表查找列；失败时返回缺失列名。
pub fn FindCols(
    cols: &[Arc<Column>],
    names: &[String],
    pk_is_handle: bool,
) -> (Option<Vec<Arc<Column>>>, String) {
    let mut found = Vec::with_capacity(names.len());
    for name in names {
        if let Some(column) = FindCol(cols, name) {
            found.push(column);
        } else if name == EXTRA_HANDLE_NAME && !pk_is_handle {
            found.push(extra_handle_column(cols.len()));
        } else {
            return (None, name.clone());
        }
    }
    (Some(found), String::new())
}

/// 按小写名列表查找列；失败时返回缺失名下标。
pub fn FindColumns(
    cols: &[Arc<Column>],
    names: &[String],
    pk_is_handle: bool,
) -> (Option<Vec<Arc<Column>>>, i32) {
    let mut found = Vec::with_capacity(names.len());
    for (offset, name) in names.iter().enumerate() {
        if let Some(column) = FindColLowerCase(cols, name) {
            found.push(column);
        } else if name == EXTRA_HANDLE_NAME && !pk_is_handle {
            found.push(extra_handle_column(cols.len()));
        } else {
            return (None, offset as i32);
        }
    }
    (Some(found), -1)
}

/// 筛选带 ON UPDATE CURRENT_TIMESTAMP 标志的列。
pub fn FindOnUpdateCols(cols: &[Arc<Column>]) -> Vec<Arc<Column>> {
    cols.iter()
        .filter(|column| mysql::HasOnUpdateNowFlag(column.GetFlag()))
        .cloned()
        .collect()
}

/// 截断字符串 Datum 尾部空格（CHAR 语义）。
fn truncateTrailingSpaces(value: &mut types::Datum) {
    if value.Kind() == types::KindNull {
        return;
    }
    let mut bytes = value.GetBytes();
    while bytes.last() == Some(&b' ') {
        bytes.pop();
    }
    let collation = value.Collation();
    value.SetString(String::from_utf8_lossy(&bytes).into_owned(), collation);
}

/// Converts the structured invalid-character message emitted by the formal
/// types crate to the table-column error shape.  Unknown errors are preserved.
/// 将类型层非法字符错误转为表列侧 Incorrect string value 形态。
pub(crate) fn convertToIncorrectStringErr(
    // 底层错误。
    error: types::errors::Error,
    column_name: &str,
) -> types::errors::Error {
    let rendered = error.to_string();
    let lower = rendered.to_ascii_lowercase();
    if !lower.contains("invalid character") {
        return error;
    }

    let Some(hex) = rendered
        .rsplit([',', ':'])
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
    else {
        return error;
    };
    let escaped = hex
        .as_bytes()
        .chunks(2)
        .map(|pair| format!("\\x{}", String::from_utf8_lossy(pair)))
        .collect::<String>();
    types::errors::New(format!(
        "Incorrect string value '{}' for column '{}'",
        escaped, column_name
    ))
}

/// 构造对应类型的零时间 Datum。
fn zero_time_datum(field_type: &types::FieldType) -> types::Datum {
    let time = types::NewTime(
        types::CoreTime::default(),
        field_type.GetType(),
        types::DefaultFsp,
    );
    types::NewTimeDatum(time)
}

/// 零时间错误消息中的类型名。
fn zero_time_name(field_type: &types::FieldType) -> &'static str {
    match field_type.GetType() {
        mysql::TypeDate => "date",
        mysql::TypeTimestamp => "timestamp",
        _ => "datetime",
    }
}

/// 构造 Incorrect <date/time> value 错误。
fn wrong_time_value(field_type: &types::FieldType, value: &str) -> types::errors::Error {
    types::errors::New(format!(
        "Incorrect {} value: '{}'",
        zero_time_name(field_type),
        value
    ))
}

/// 向错误上下文追加警告。
fn append_warning(context: &ErrorContext, error: &types::errors::Error) {
    context.AppendWarning(errors_dependency::New(error.to_string()));
}

/// 按 SQL Mode 处理零日期/非法时间：严格模式报错，否则警告并回退零值。
fn handleZeroDatetime(
    error_context: ErrorContext,
    mode: SQLMode,
    field_type: &types::FieldType,
    // 已转换（可能部分成功）的值。
    casted: types::Datum,
    source: &str,
    time_is_invalid: bool,
) -> (types::Datum, bool, Option<types::errors::Error>) {
    let time = casted.GetMysqlTime();
    let ignore_error = error_context.LevelForGroup(ErrGroup::ErrGroupDupKey) != Level::LevelError;
    let zero = zero_time_datum(field_type);

    if time.IsZero() && field_type.GetType() == mysql::TypeTimestamp {
        let error = wrong_time_value(field_type, source);
        if mode.HasStrictMode() && !ignore_error && (time_is_invalid || mode.HasNoZeroDateMode()) {
            return (zero, true, Some(error));
        }
        if time_is_invalid || mode.HasNoZeroDateMode() {
            append_warning(&error_context, &error);
        }
        return (zero, true, None);
    }

    if time_is_invalid && field_type.GetType() == mysql::TypeTimestamp {
        let error = wrong_time_value(field_type, source);
        if mode.HasStrictMode() {
            return (zero, true, Some(error));
        }
        append_warning(&error_context, &error);
        return (zero, true, None);
    }

    if time.IsZero() || time.InvalidZero() {
        if time.IsZero() && !time_is_invalid && !mode.HasNoZeroDateMode() {
            return (zero, true, None);
        }
        if time.InvalidZero() && !time.IsZero() && !mode.HasNoZeroInDateMode() {
            return (casted, true, None);
        }

        let error = wrong_time_value(field_type, source);
        if mode.HasStrictMode() && !ignore_error {
            return (zero, true, Some(error));
        }
        append_warning(&error_context, &error);
        return (zero, true, None);
    }

    (casted, false, None)
}

/// Session-shaped casting context.  The formal `SessionVarsProvider` slice does
/// not yet expose type/error contexts, so consumers provide this narrow view;
/// every formal expression `EvalContext` implements it automatically.
/// 会话形态的类型转换上下文视图。
pub trait CastContext {
    fn TypeCtx(&self) -> types::Context;
    fn ErrCtx(&self) -> ErrorContext;
    fn SQLMode(&self) -> SQLMode;
    fn ConnectionID(&self) -> u64;
}

/// Stable cast categories used at the table boundary.  The lower-level datum
/// facade currently exposes a string error, so table classifies the operation
/// from the source/target types before formatting the public message.
/// 表边界稳定的转换错误分类。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CastErrorKind {
    /// 截断。
    Truncated,
    /// 非法字符。
    InvalidCharacter,
    /// 错误的日期时间。
    WrongDatetime,
    /// 其他。
    Other,
}

/// A failed cast still owns the converted value, matching Go's `(casted, err)`
/// contract while retaining idiomatic `Result` propagation for Rust callers.
/// 失败转换仍持有已转换值，对齐 Go 的 `(casted, err)`。
pub struct CastError {
    casted: types::Datum,
    error: types::errors::Error,
    /// 错误分类。
    kind: CastErrorKind,
}

impl CastError {
    /// 构造转换错误。
    fn new(casted: types::Datum, error: types::errors::Error, kind: CastErrorKind) -> Self {
        Self {
            casted,
            error,
            kind,
        }
    }

    /// 借用已转换值。
    pub fn casted(&self) -> &types::Datum {
        &self.casted
    }

    /// 取出已转换值。
    pub fn into_casted(self) -> types::Datum {
        self.casted
    }

    /// 返回错误分类。
    pub fn kind(&self) -> CastErrorKind {
        self.kind
    }

    /// 借用底层错误。
    pub fn error(&self) -> &types::errors::Error {
        &self.error
    }

    /// 取出底层错误。
    fn into_error(self) -> types::errors::Error {
        self.error
    }
}

impl fmt::Debug for CastError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CastError")
            .field("kind", &self.kind)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl fmt::Display for CastError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.error, formatter)
    }
}

impl std::error::Error for CastError {}

/// 列值转换结果。
pub type CastResult = Result<types::Datum, CastError>;

impl<T: EvalContext + ?Sized> CastContext for T {
    fn TypeCtx(&self) -> types::Context {
        EvalContext::TypeCtx(self)
    }

    fn ErrCtx(&self) -> ErrorContext {
        EvalContext::ErrCtx(self)
    }

    fn SQLMode(&self) -> SQLMode {
        EvalContext::SQLMode(self)
    }

    fn ConnectionID(&self) -> u64 {
        EvalContext::CtxID(self)
    }
}

/// 在 `CastContext` 下将 Datum 转为列类型。
pub fn CastValue(
    context: &dyn CastContext,
    value: types::Datum,
    column: &model::ColumnInfo,
    return_error: bool,
    force_ignore_truncate: bool,
) -> CastResult {
    castColumnValue(
        context.TypeCtx(),
        context.ErrCtx(),
        context.SQLMode(),
        value,
        &column.FieldType,
        &column.Name.O,
        context.ConnectionID(),
        return_error,
        force_ignore_truncate,
    )
}

/// 以严格模式、无警告上下文转换列值。
pub fn CastColumnValueWithStrictMode(
    value: types::Datum,
    field_type: &types::FieldType,
) -> CastResult {
    let type_context = types::DefaultStmtNoWarningContext
        .clone()
        .WithFlags(types::Flags(0));
    castColumnValue(
        type_context,
        errctx_dependency::errctx::StrictNoWarningContext.clone(),
        ModeNone,
        value,
        field_type,
        "",
        0,
        true,
        false,
    )
}

/// 通过 `BuildContext` 的求值上下文转换列值。
pub fn CastColumnValue(
    context: &dyn BuildContext,
    value: types::Datum,
    column: &model::ColumnInfo,
    return_error: bool,
    force_ignore_truncate: bool,
) -> CastResult {
    let eval_context = context.GetEvalCtx();
    let legacy_enum_set = matches!(column.FieldType.GetType(), mysql::TypeEnum | mysql::TypeSet)
        && !context.NewCollationEnabled()
        && expression_dependency::collate::NewCollationEnabled();
    let mut field_type = column.FieldType.clone();
    if legacy_enum_set {
        field_type.SetCollate("binary".to_owned());
    }
    let result = castColumnValue(
        eval_context.TypeCtx(),
        eval_context.ErrCtx(),
        eval_context.SQLMode(),
        value,
        &field_type,
        &column.Name.O,
        context.ConnectionID(),
        return_error,
        force_ignore_truncate,
    );
    match result {
        Ok(mut casted) => {
            if legacy_enum_set {
                casted.SetCollation(column.GetCollate().to_owned());
            }
            Ok(casted)
        }
        Err(mut error) => {
            if legacy_enum_set {
                error.casted.SetCollation(column.GetCollate().to_owned());
            }
            Err(error)
        }
    }
}

/// 核心转换：部分恢复、零日期处理、非法字符与截断策略。
fn castColumnValue(
    type_context: types::Context,
    error_context: ErrorContext,
    sql_mode: SQLMode,
    value: types::Datum,
    field_type: &types::FieldType,
    column_name: &str,
    _connection_id: u64,
    return_error: bool,
    force_ignore_truncate: bool,
) -> CastResult {
    let (mut casted, mut error) = match convert_with_partial(&value, &type_context, field_type) {
        Ok(casted) => (casted, None),
        Err(error) => (error.casted.clone(), Some(error)),
    };

    // 调用方要求直接返回转换错误时立即失败。
    if return_error {
        if let Some(error) = error {
            return Err(error);
        }
    }

    // 截断类错误重写为 Truncated incorrect ... 消息（集合/枚举除外）。
    if error.as_ref().is_some_and(|error| {
        error.kind == CastErrorKind::Truncated
            && field_type.GetType() != mysql::TypeSet
            && field_type.GetType() != mysql::TypeEnum
    }) {
        let source = value.ToString().unwrap_or_else(|_| value.GetString());
        if let Some(error) = error.as_mut() {
            error.error = types::errors::New(format!(
                "Truncated incorrect {} value: '{}'",
                field_type.CompactStr(),
                source
            ));
        }
    } else if !casted.IsNull()
        && matches!(
            field_type.GetType(),
            mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp
        )
    {
        let source = value.ToString().unwrap_or_else(|_| value.GetString());
        let time_is_invalid = error
            .as_ref()
            .is_some_and(|error| error.kind == CastErrorKind::WrongDatetime);
        let (handled, exit, inner_error) = handleZeroDatetime(
            error_context,
            sql_mode,
            field_type,
            casted,
            &source,
            time_is_invalid,
        );
        if exit {
            return inner_error.map_or(Ok(handled.clone()), |error| {
                Err(CastError::new(handled, error, CastErrorKind::WrongDatetime))
            });
        }
        casted = handled;
    } else if error
        .as_ref()
        .is_some_and(|error| error.kind == CastErrorKind::InvalidCharacter)
    {
        if let Some(error) = error.as_mut() {
            error.error =
                incorrect_string_error(&value, field_type, column_name, error.error.clone());
        }
    }

    if let Some(mut truncate_error) = error.take() {
        let shared = errors_dependency::New(truncate_error.error.to_string());
        match type_context.HandleTruncate(casted, shared) {
            Ok(handled) => casted = handled,
            Err(with_value) => {
                casted = with_value.value;
                truncate_error.casted = casted.clone();
                truncate_error.error = types::errors::New(with_value.error.to_string());
                error = Some(truncate_error);
            }
        }
    }

    if !force_ignore_truncate {
        if let Some(error) = error {
            return Err(error);
        }
    }

    // CHAR 非二进制串去掉尾部空格。
    if field_type.GetType() == mysql::TypeString && !types::IsBinaryStr(field_type) {
        truncateTrailingSpaces(&mut casted);
    }
    Ok(casted)
}

/// 根据源值与目标类型推断转换错误种类。
fn cast_error_kind(value: &types::Datum, field_type: &types::FieldType) -> CastErrorKind {
    if matches!(
        field_type.GetType(),
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp
    ) {
        return CastErrorKind::WrongDatetime;
    }
    if matches!(
        field_type.GetType(),
        mysql::TypeString
            | mysql::TypeVarchar
            | mysql::TypeVarString
            | mysql::TypeBlob
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob
    ) && first_invalid_character(value, field_type).is_some()
    {
        return CastErrorKind::InvalidCharacter;
    }
    if matches!(
        field_type.GetType(),
        mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeFloat
            | mysql::TypeDouble
            | mysql::TypeNewDecimal
            | mysql::TypeString
            | mysql::TypeVarchar
            | mysql::TypeVarString
            | mysql::TypeBlob
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob
    ) {
        CastErrorKind::Truncated
    } else {
        CastErrorKind::Other
    }
}

/// 部分恢复时忽略截断与零日期相关错误的标志。
fn recovery_flags(flags: types::Flags) -> types::Flags {
    flags
        .WithIgnoreTruncateErr(true)
        .WithIgnoreZeroDateErr(true)
        .WithIgnoreZeroInDate(true)
        .WithIgnoreInvalidDateErr(true)
}

/// 数值转换失败时尝试恢复为有界整数/无符号值。
fn recover_numeric_partial(
    value: &types::Datum,
    context: &types::Context,
    field_type: &types::FieldType,
) -> Option<types::Datum> {
    let target = field_type.GetType();
    if mysql::HasUnsignedFlag(field_type.GetFlag()) && value.Kind() == types::KindInt64 {
        let result = types::ConvertIntToUint(
            context.Flags(),
            value.GetInt64(),
            types::IntegerUnsignedUpperBound(target),
            target,
        );
        let number = result.unwrap_or_else(|error| error.value);
        return Some(types::NewUintDatum(number));
    }
    if !mysql::HasUnsignedFlag(field_type.GetFlag()) && value.Kind() == types::KindInt64 {
        let result = types::ConvertIntToInt(
            value.GetInt64(),
            types::IntegerSignedLowerBound(target),
            types::IntegerSignedUpperBound(target),
            target,
        );
        let number = result.unwrap_or_else(|error| error.value);
        return Some(types::NewIntDatum(number));
    }
    None
}

/// 字符串转换失败时截取合法字符前缀并限制长度。
fn recover_string_partial(value: &types::Datum, field_type: &types::FieldType) -> types::Datum {
    let bytes = value.GetBytes();
    let prefix = valid_character_prefix_len(&bytes, field_type);
    let mut text = String::from_utf8_lossy(&bytes[..prefix]).into_owned();
    if field_type.GetFlen() >= 0 {
        text = text.chars().take(field_type.GetFlen() as usize).collect();
    }
    let mut casted = types::NewStringDatum(text);
    casted.SetString(casted.GetString(), field_type.GetCollate().to_owned());
    casted
}

/// 按字符集计算合法前缀字节长度。
fn valid_character_prefix_len(bytes: &[u8], field_type: &types::FieldType) -> usize {
    let charset = field_type.GetCharset().to_ascii_lowercase();
    if charset == "ascii" {
        return bytes
            .iter()
            .position(|byte| !byte.is_ascii())
            .unwrap_or(bytes.len());
    }
    if !matches!(charset.as_str(), "utf8" | "utf8mb3" | "utf8mb4") {
        return bytes.len();
    }
    match str::from_utf8(bytes) {
        Ok(text) if charset != "utf8mb4" => text
            .char_indices()
            .find_map(|(index, character)| (character.len_utf8() == 4).then_some(index))
            .unwrap_or(bytes.len()),
        Ok(_) => bytes.len(),
        Err(error) => error.valid_up_to(),
    }
}

/// 转换失败后尽可能恢复部分结果。
fn recover_partial(
    value: &types::Datum,
    context: &types::Context,
    field_type: &types::FieldType,
) -> types::Datum {
    if let Some(partial) = recover_numeric_partial(value, context, field_type) {
        return partial;
    }
    let recovery = context.WithFlags(recovery_flags(context.Flags()));
    if let Ok(partial) = value.ConvertTo(recovery, field_type) {
        return partial;
    }
    if matches!(
        field_type.GetType(),
        mysql::TypeString
            | mysql::TypeVarchar
            | mysql::TypeVarString
            | mysql::TypeBlob
            | mysql::TypeTinyBlob
            | mysql::TypeMediumBlob
            | mysql::TypeLongBlob
    ) {
        return recover_string_partial(value, field_type);
    }
    if matches!(
        field_type.GetType(),
        mysql::TypeDate | mysql::TypeDatetime | mysql::TypeTimestamp
    ) {
        return zero_time_datum(field_type);
    }
    types::Datum::default()
}

/// 先 ConvertTo，失败则带部分结果包装为 CastError。
fn convert_with_partial(
    value: &types::Datum,
    context: &types::Context,
    field_type: &types::FieldType,
) -> CastResult {
    match value.ConvertTo(context.clone(), field_type) {
        Ok(casted) => Ok(casted),
        Err(error) => {
            let kind = cast_error_kind(value, field_type);
            let casted = recover_partial(value, context, field_type);
            Err(CastError::new(casted, error, kind))
        }
    }
}

/// 定位首个非法字符的原始字节。
fn first_invalid_character(value: &types::Datum, field_type: &types::FieldType) -> Option<Vec<u8>> {
    if !matches!(
        value.Kind(),
        types::KindString | types::KindBytes | types::KindBinaryLiteral
    ) {
        return None;
    }
    let bytes = value.GetBytes();
    let charset = field_type.GetCharset().to_ascii_lowercase();
    if charset == "ascii" {
        return bytes
            .iter()
            .position(|byte| !byte.is_ascii())
            .map(|index| vec![bytes[index]]);
    }
    if !matches!(charset.as_str(), "utf8" | "utf8mb3" | "utf8mb4") {
        return None;
    }
    match str::from_utf8(&bytes) {
        Ok(text) if charset != "utf8mb4" => text
            .chars()
            .find(|character| character.len_utf8() == 4)
            .map(|character| {
                let mut buffer = [0; 4];
                character.encode_utf8(&mut buffer).as_bytes().to_vec()
            }),
        Ok(_) => None,
        Err(error) => {
            let start = error.valid_up_to();
            let length = error
                .error_len()
                .unwrap_or(bytes.len().saturating_sub(start));
            Some(bytes[start..start + length].to_vec())
        }
    }
}

/// 格式化为 Incorrect string value 错误。
fn incorrect_string_error(
    value: &types::Datum,
    field_type: &types::FieldType,
    column_name: &str,
    fallback: types::errors::Error,
) -> types::errors::Error {
    let Some(invalid) = first_invalid_character(value, field_type) else {
        return fallback;
    };
    let escaped = invalid
        .iter()
        .map(|byte| format!("\\x{byte:02X}"))
        .collect::<String>();
    types::errors::New(format!(
        "Incorrect string value '{}' for column '{}'",
        escaped, column_name
    ))
}

// ColDesc describes column information like MySQL DESC and SHOW COLUMNS.
/// 类似 MySQL DESC / SHOW COLUMNS 的列描述。
pub struct ColDesc {
    /// 列名。
    pub Field: String,
    /// 类型描述。
    pub Type: String,
    /// 字符集（若适用）。
    pub Charset: Option<String>,
    /// 排序规则（若适用）。
    pub Collation: Option<String>,
    /// 是否可空（YES/NO）。
    pub Null: String,
    /// 键类型（PRI/UNI/MUL）。
    pub Key: String,
    /// 默认值。
    pub DefaultValue: Option<model::DefaultValue>,
    /// 额外信息（auto_increment、生成列等）。
    pub Extra: String,
    /// 权限列表字符串。
    pub Privileges: String,
    /// 列注释。
    pub Comment: String,
}

/// DESC 输出默认权限串。
pub const defaultPrivileges: &str = "select,insert,update,references";

/// 由可执行列构造 ColDesc。
pub fn NewColDesc(column: &Column) -> Box<ColDesc> {
    let info = column.ToInfo();
    let null_flag = if mysql::HasNotNullFlag(column.GetFlag()) {
        "NO"
    } else {
        "YES"
    };
    // 主键 / 唯一键 / 普通索引标记映射到 PRI/UNI/MUL。
    let key_flag = if mysql::HasPriKeyFlag(column.GetFlag()) {
        "PRI"
    } else if mysql::HasUniKeyFlag(column.GetFlag()) {
        "UNI"
    } else if mysql::HasMultipleKeyFlag(column.GetFlag()) {
        "MUL"
    } else {
        ""
    };

    let mut default_value = if mysql::HasNoDefaultValueFlag(column.GetFlag()) {
        None
    } else {
        info.GetDefaultValue()
    };
    if let Some(model::DefaultValue::String(value)) = default_value.as_ref() {
        let text = String::from_utf8_lossy(value);
        if matches!(info.GetType(), mysql::TypeTimestamp | mysql::TypeDatetime)
            && text.eq_ignore_ascii_case("CURRENT_TIMESTAMP")
            && info.GetDecimal() > 0
        {
            default_value = Some(model::DefaultValue::String(
                format!("{}({})", text, info.GetDecimal()).into_bytes(),
            ));
        }
    }

    // Extra 列：自增、ON UPDATE、生成列或表达式默认值。
    let extra = if mysql::HasAutoIncrementFlag(column.GetFlag()) {
        "auto_increment".to_owned()
    } else if mysql::HasOnUpdateNowFlag(column.GetFlag()) {
        format!(
            "DEFAULT_GENERATED on update CURRENT_TIMESTAMP{}",
            OptionalFsp(&info.FieldType)
        )
    } else if info.IsGenerated() {
        if info.GeneratedStored {
            "STORED GENERATED".to_owned()
        } else {
            "VIRTUAL GENERATED".to_owned()
        }
    } else if info.DefaultIsExpr {
        "DEFAULT_GENERATED".to_owned()
    } else {
        String::new()
    };

    let has_charset = model::types::HasCharset(&info.FieldType);
    Box::new(ColDesc {
        Field: info.Name.O.clone(),
        Type: info.GetTypeDesc(),
        Charset: has_charset.then(|| info.GetCharset().to_owned()),
        Collation: has_charset.then(|| info.GetCollate().to_owned()),
        Null: null_flag.to_owned(),
        Key: key_flag.to_owned(),
        DefaultValue: default_value,
        Extra: extra,
        Privileges: defaultPrivileges.to_owned(),
        Comment: info.Comment.clone(),
    })
}

/// DESC 结果列名；full 为真时含 Collation/Privileges/Comment。
pub fn ColDescFieldNames(full: bool) -> Vec<String> {
    let names: &[&str] = if full {
        &[
            "Field",
            "Type",
            "Collation",
            "Null",
            "Key",
            "Default",
            "Extra",
            "Privileges",
            "Comment",
        ]
    } else {
        &["Field", "Type", "Null", "Key", "Default", "Extra"]
    };
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// 检查列名不重复。
pub fn CheckOnce(columns: &[Arc<Column>]) -> Result<(), types::errors::Error> {
    let mut names = std::collections::HashSet::with_capacity(columns.len());
    for column in columns {
        if !names.insert(column.ColumnInfo.Name.L.clone()) {
            return Err(types::errors::New(format!(
                "Duplicate column name '{}'",
                column.ColumnInfo.Name.O
            )));
        }
    }
    Ok(())
}

impl Column {
    /// 非空约束检查；LOAD DATA 可带行号。
    pub fn CheckNotNull(
        &self,
        data: &types::Datum,
        row_count_in_load_data: u64,
    ) -> Result<(), types::errors::Error> {
        if (mysql::HasNotNullFlag(self.GetFlag())
            || mysql::HasPreventNullInsertFlag(self.GetFlag()))
            && data.IsNull()
        {
            if self.ColumnInfo.FieldType.EvalType().IsVectorKind() {
                return Err(types::errors::New(format!(
                    "VECTOR column '{}' cannot be null",
                    self.ColumnInfo.Name.O
                )));
            }
            if row_count_in_load_data > 0 {
                return Err(types::errors::New(format!(
                    "Column '{}' cannot be null at row {}",
                    self.ColumnInfo.Name.O, row_count_in_load_data
                )));
            }
            return Err(types::errors::New(format!(
                "Column '{}' cannot be null",
                self.ColumnInfo.Name.O
            )));
        }
        Ok(())
    }

    /// 处理坏 NULL：按错误级别警告并填零值，或上抛。
    pub fn HandleBadNull(
        &self,
        error_context: ErrorContext,
        datum: &mut types::Datum,
        row_count_in_load_data: u64,
    ) -> Result<(), types::errors::Error> {
        if let Err(error) = self.CheckNotNull(datum, row_count_in_load_data) {
            let code = if row_count_in_load_data > 0 {
                errctx_dependency::errno::ErrWarnNullToNotnull
            } else {
                errctx_dependency::errno::ErrBadNull
            };
            let shared = errors_dependency::SharedError::new(errors_dependency::Normalize(
                error.to_string(),
                &[errors_dependency::MySQLErrorCode(i32::from(code))],
            ));
            if error_context.HandleError(Some(shared)).is_none() {
                *datum = GetZeroValue(self);
                return Ok(());
            }
            return Err(error);
        }
        Ok(())
    }

    /// 是否为整型主键 handle 列。
    pub fn IsPKHandleColumn<T: HandleTableInfo>(&self, table_info: &T) -> bool {
        mysql::HasPriKeyFlag(self.GetFlag()) && table_info.PKIsHandle()
    }

    /// 是否为聚簇索引（common handle）主键列。
    pub fn IsCommonHandleColumn<T: HandleTableInfo>(&self, table_info: &T) -> bool {
        mysql::HasPriKeyFlag(self.GetFlag()) && table_info.IsCommonHandle()
    }
}

/// Narrow view of the two handle flags used by column logic.  It keeps the
/// table package independent while the model table slice grows those fields.
/// 列逻辑所需的两个 handle 标志窄视图。
pub trait HandleTableInfo {
    fn PKIsHandle(&self) -> bool;
    fn IsCommonHandle(&self) -> bool;
}

impl HandleTableInfo for model::TableInfo {
    fn PKIsHandle(&self) -> bool {
        self.PKIsHandle
    }
    fn IsCommonHandle(&self) -> bool {
        self.IsCommonHandle
    }
}

#[allow(non_camel_case_types)]
/// 取原始默认值时的选项（是否严格 SQL Mode）。
pub struct getColOriginDefaultValue {
    /// 是否启用严格 SQL Mode。
    pub StrictSQLMode: bool,
}

/// 将 model 默认值转为 Datum。
fn datum_from_default_value(value: &model::DefaultValue) -> types::Datum {
    match value {
        model::DefaultValue::Bool(value) => types::NewDatum(value),
        model::DefaultValue::Int(value) => types::NewIntDatum(*value),
        model::DefaultValue::Uint(value) => types::NewUintDatum(*value),
        model::DefaultValue::Float(value) => types::NewFloat64Datum(*value),
        model::DefaultValue::String(value) => types::NewBytesDatum(value.clone()),
    }
}

/// 取列的原始（origin）默认值。
pub fn GetColOriginDefaultValue(
    context: &dyn BuildContext,
    column: &Column,
) -> Result<types::Datum, types::errors::Error> {
    getColDefaultValue(
        context,
        column,
        column.ColumnInfo.GetOriginDefaultValue(),
        None,
    )
}

/// 在非严格 SQL Mode 下取原始默认值。
pub fn GetColOriginDefaultValueWithoutStrictSQLMode(
    context: &dyn BuildContext,
    column: &Column,
) -> Result<types::Datum, types::errors::Error> {
    getColDefaultValue(
        context,
        column,
        column.ColumnInfo.GetOriginDefaultValue(),
        Some(getColOriginDefaultValue {
            StrictSQLMode: false,
        }),
    )
}

/// INSERT 时检查无默认值列；按错误级别报错或警告。
pub fn CheckNoDefaultValueForInsert(
    error_context: &ErrorContext,
    column: &Column,
) -> Result<(), types::errors::Error> {
    let info = column.ToInfo();
    if mysql::HasNoDefaultValueFlag(column.GetFlag())
        && !info.DefaultIsExpr
        && info.GetDefaultValue().is_none()
        && info.GetType() != mysql::TypeEnum
    {
        let error = types::errors::New(format!(
            "Field '{}' doesn't have a default value",
            info.Name.O
        ));
        if error_context.LevelForGroup(ErrGroup::ErrGroupNoDefault) == Level::LevelError {
            return Err(error);
        }
        if !mysql::HasNotNullFlag(column.GetFlag()) {
            append_warning(error_context, &error);
        }
    }
    Ok(())
}

/// 取当前默认值；表达式默认值则求值后再转换。
pub fn GetColDefaultValue(
    context: &dyn BuildContext,
    column: &Column,
) -> Result<types::Datum, types::errors::Error> {
    let default_value = column.ColumnInfo.GetDefaultValue();
    if !column.ColumnInfo.DefaultIsExpr {
        return getColDefaultValue(context, column, default_value, None);
    }
    let Some(model::DefaultValue::String(default_expression)) = default_value else {
        return Err(types::errors::New(format!(
            "invalid default expression for '{}'",
            column.ColumnInfo.Name.O
        )));
    };
    getColDefaultExprValue(
        context,
        column,
        &String::from_utf8_lossy(&default_expression),
    )
}

/// Resolve a column being changed by online DDL from its prior column value.
/// When the old row lacks that value, use and cache the target column default.
pub fn GetChangingColVal(
    context: &dyn BuildContext,
    columns: &[Arc<Column>],
    column: &Column,
    row_map: &std::collections::HashMap<i64, types::Datum>,
    default_values: &mut [Option<types::Datum>],
) -> Result<(types::Datum, bool), types::errors::Error> {
    let change = column
        .ColumnInfo
        .ChangeStateInfo
        .as_ref()
        .ok_or_else(|| types::errors::New("column has no change-state information"))?;
    let dependency_offset = usize::try_from(change.DependencyColumnOffset)
        .map_err(|_| types::errors::New("negative dependency column offset"))?;
    let relative = columns
        .get(dependency_offset)
        .ok_or_else(|| types::errors::New("dependency column offset out of range"))?;
    if let Some(value) = row_map.get(&relative.ColumnInfo.ID) {
        return CastColumnValue(context, value.clone(), &column.ColumnInfo, false, false)
            .map(|value| (value, false))
            .map_err(|error| types::errors::New(error.to_string()));
    }
    let offset = usize::try_from(column.ColumnInfo.Offset)
        .map_err(|_| types::errors::New("negative target column offset"))?;
    let cached = default_values
        .get_mut(offset)
        .ok_or_else(|| types::errors::New("target column offset out of range"))?;
    if cached.is_none() {
        *cached = Some(GetColDefaultValue(context, column)?);
    }
    Ok((cached.as_ref().expect("default value cached").clone(), true))
}

/// 对已解析的默认值表达式求值并转换为列类型。
pub fn EvalColDefaultExpr(
    context: &dyn BuildContext,
    column: &Column,
    default_expression: ExprNode,
) -> Result<types::Datum, types::errors::Error> {
    let expression =
        expression_dependency::BuildSimpleExpr(context, &default_expression, Vec::new())?;
    let datum = expression.Eval(context.GetEvalCtx(), chunk_dependency::Row::default())?;
    CastColumnValue(context, datum, column.ToInfo(), false, false).map_err(CastError::into_error)
}

/// 解析并求值字符串形式的默认值表达式。
fn getColDefaultExprValue(
    context: &dyn BuildContext,
    column: &Column,
    default_value: &str,
) -> Result<types::Datum, types::errors::Error> {
    let expression = expression_dependency::ParseSimpleExpr(context, default_value, Vec::new())?;
    let datum = expression.Eval(context.GetEvalCtx(), chunk_dependency::Row::default())?;
    CastColumnValue(context, datum, column.ToInfo(), false, false).map_err(CastError::into_error)
}

/// 从可选默认值计算列 Datum；时间戳可能需 UTC 转换。
fn getColDefaultValue(
    context: &dyn BuildContext,
    column: &Column,
    default_value: Option<model::DefaultValue>,
    args: Option<getColOriginDefaultValue>,
) -> Result<types::Datum, types::errors::Error> {
    let Some(default_value) = default_value else {
        return getColDefaultValueFromNil(context, column, args);
    };

    if !matches!(
        column.ColumnInfo.GetType(),
        mysql::TypeTimestamp | mysql::TypeDate | mysql::TypeDatetime
    ) {
        return CastColumnValue(
            context,
            datum_from_default_value(&default_value),
            column.ToInfo(),
            false,
            false,
        )
        .map_err(CastError::into_error);
    }

    let mut type_context = context.GetEvalCtx().TypeCtx();
    let mut convert_from_utc = false;
    // v1+ 时间戳默认值（非零、非 CURRENT_TIMESTAMP）按 UTC 存储后转到会话时区。
    if column.ColumnInfo.GetType() == mysql::TypeTimestamp {
        if let model::DefaultValue::String(value) = &default_value {
            let value = String::from_utf8_lossy(value);
            if value != "0000-00-00 00:00:00"
                && !value.eq_ignore_ascii_case("CURRENT_TIMESTAMP")
                && column.ColumnInfo.Version >= model::ColumnInfoVersion1
            {
                convert_from_utc = true;
                type_context = type_context.WithLocation(chrono_tz::UTC);
            }
        }
    }

    let mut value = datum_from_default_value(&default_value)
        .ConvertTo(type_context, &column.ColumnInfo.FieldType)?;
    if convert_from_utc {
        let mut time = value.GetMysqlTime();
        time.ConvertTimeZone(chrono_tz::UTC, context.GetEvalCtx().Location())?;
        value.SetMysqlTime(time);
    }
    Ok(value)
}

/// 无显式默认值时：可空则 NULL，枚举取首元素，否则零值或报错。
fn getColDefaultValueFromNil(
    context: &dyn BuildContext,
    column: &Column,
    args: Option<getColOriginDefaultValue>,
) -> Result<types::Datum, types::errors::Error> {
    if !mysql::HasNotNullFlag(column.GetFlag()) {
        return Ok(types::Datum::default());
    }
    if column.ColumnInfo.GetType() == mysql::TypeEnum {
        let value = types::ParseEnumValue(column.ColumnInfo.GetElems(), 1)?;
        return Ok(types::NewCollateMysqlEnumDatum(
            value,
            column.ColumnInfo.GetCollate().to_owned(),
        ));
    }
    if mysql::HasAutoIncrementFlag(column.GetFlag()) {
        return Ok(GetZeroValue(column));
    }

    let eval_context = context.GetEvalCtx();
    let strict_sql_mode = args.map_or_else(
        || eval_context.SQLMode().HasStrictMode(),
        |args| args.StrictSQLMode,
    );
    let error = types::errors::New(format!(
        "Field '{}' doesn't have a default value",
        column.ColumnInfo.Name.O
    ));
    if !strict_sql_mode {
        eval_context.AppendWarning(errors_dependency::New(error.to_string()));
        return Ok(GetZeroValue(column));
    }
    if eval_context
        .ErrCtx()
        .HandleError(Some(errors_dependency::New(error.to_string())))
        .is_none()
    {
        return Ok(GetZeroValue(column));
    }
    Err(error)
}

/// 按列类型构造零值 Datum。
pub fn GetZeroValue(column: &Column) -> types::Datum {
    let field_type = &column.ColumnInfo.FieldType;
    let mut datum = types::Datum::default();
    match field_type.GetType() {
        mysql::TypeTiny
        | mysql::TypeInt24
        | mysql::TypeShort
        | mysql::TypeLong
        | mysql::TypeLonglong => {
            if mysql::HasUnsignedFlag(field_type.GetFlag()) {
                datum.SetUint64(0);
            } else {
                datum.SetInt64(0);
            }
        }
        mysql::TypeYear => datum.SetInt64(0),
        mysql::TypeFloat => datum.SetFloat32(0.0),
        mysql::TypeDouble => datum.SetFloat64(0.0),
        mysql::TypeNewDecimal => {
            datum.SetLength(field_type.GetFlen() as i32);
            datum.SetFrac(field_type.GetDecimal() as i32);
            datum.SetMysqlDecimal(types::MyDecimal::default());
        }
        mysql::TypeString => {
            if field_type.GetFlen() > 0 && field_type.GetCharset() == "binary" {
                datum.SetBytes(vec![0; field_type.GetFlen() as usize]);
            } else {
                datum.SetString(String::new(), field_type.GetCollate().to_owned());
            }
        }
        mysql::TypeVarString
        | mysql::TypeVarchar
        | mysql::TypeBlob
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob => {
            datum.SetString(String::new(), field_type.GetCollate().to_owned());
        }
        mysql::TypeDuration => datum.SetMysqlDuration(types::ZeroDuration),
        mysql::TypeDate | mysql::TypeTimestamp | mysql::TypeDatetime => {
            datum.SetMysqlTime(types::NewTime(
                types::CoreTime::default(),
                field_type.GetType(),
                types::DefaultFsp,
            ));
        }
        mysql::TypeBit => datum.SetMysqlBit(types::BinaryLiteral(Vec::new())),
        mysql::TypeSet => {
            datum.SetMysqlSet(types::Set::default(), field_type.GetCollate().to_owned())
        }
        mysql::TypeEnum => {
            datum.SetMysqlEnum(types::Enum::default(), field_type.GetCollate().to_owned())
        }
        mysql::TypeJSON => datum.SetMysqlJSON(types::BinaryJSON::default()),
        mysql::TypeTiDBVectorFloat32 => datum.SetVectorFloat32(
            types::ParseVectorFloat32("[]").expect("empty vector is always valid"),
        ),
        _ => {}
    }
    datum
}

/// 小数秒精度（fsp）非正时返回空串，否则返回 `(n)`。
pub fn OptionalFsp(field_type: &types::FieldType) -> String {
    let fsp = field_type.GetDecimal();
    if fsp <= 0 {
        String::new()
    } else {
        format!("({fsp})")
    }
}

/// 填充虚拟生成列：求值、转换，并处理无符号负值与非空回退。
pub fn FillVirtualColumnValue(
    virtual_return_types: &[Box<types::FieldType>],
    virtual_column_indexes: &[i32],
    expression_columns: &[expression_dependency::Column],
    columns: &[Arc<Column>],
    context: &dyn BuildContext,
    request: &mut chunk::Chunk,
) -> Result<(), types::errors::Error> {
    if virtual_column_indexes.is_empty() {
        return Ok(());
    }

    let fields = virtual_return_types
        .iter()
        .map(|field_type| (**field_type).clone())
        .collect();
    let mut virtual_columns = chunk::NewChunkWithCapacity(fields, request.Capacity());
    let eval_context = context.GetEvalCtx();
    let type_context = eval_context.TypeCtx();
    for (output_index, column_index) in virtual_column_indexes.iter().enumerate() {
        let column_index = *column_index as usize;
        for row_index in 0..request.NumRows() {
            let source_row = request.GetRow(row_index);
            let source =
                expression_columns[column_index].EvalVirtualColumn(eval_context, source_row)?;
            let mut casted = CastColumnValue(
                context,
                source.clone(),
                columns[column_index].ToInfo(),
                false,
                true,
            )
            .map_err(CastError::into_error)?;

            // 允许负转无符号时，负源值回退为零值。
            if mysql::HasUnsignedFlag(columns[column_index].GetFlag())
                && !casted.IsNull()
                && type_context.Flags().AllowNegativeToUnsigned()
            {
                let negative = match source.Kind() {
                    types::KindInt64 => source.GetInt64() < 0,
                    types::KindFloat32 | types::KindFloat64 => source.GetFloat64().round() < 0.0,
                    types::KindMysqlDecimal => source.GetMysqlDecimal().IsNegative(),
                    _ => false,
                };
                if negative {
                    casted = GetZeroValue(&columns[column_index]);
                }
            }

            if (mysql::HasNotNullFlag(columns[column_index].GetFlag())
                || mysql::HasPreventNullInsertFlag(columns[column_index].GetFlag()))
                && casted.IsNull()
            {
                casted = GetZeroValue(&columns[column_index]);
            }
            virtual_columns.AppendDatum(output_index, &casted);
        }
        request.SetCol(column_index, virtual_columns.Column(output_index).clone());
    }
    Ok(())
}

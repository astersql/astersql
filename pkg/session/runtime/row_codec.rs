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

use super::*;

pub(super) fn relational_handle_key(table_id: i64, logical_handle: i128) -> kv::Key {
    kv::Key(
        astersql_tablecodec::EncodeRowKeyWithHandle(
            table_id,
            Box::new(astersql_tablecodec::kv::IntHandle(
                (logical_handle as u64) as i64,
            )),
        )
        .0,
    )
}

pub(super) fn typed_literal_to_runtime_value(
    expression: &ast::ExprNode,
    column: &astersql_meta_model::ColumnInfo,
    flags: astersql_types::Flags,
) -> SessionResult<Option<String>> {
    typed_literal_to_runtime_value_with_warning(expression, column, flags)
        .map(|(value, _warning)| value)
}

pub(super) fn typed_literal_to_runtime_value_with_warning(
    expression: &ast::ExprNode,
    column: &astersql_meta_model::ColumnInfo,
    flags: astersql_types::Flags,
) -> SessionResult<(Option<String>, bool)> {
    let statement_context = astersql_sessionctx_stmtctx::NewStmtCtx();
    let datum = match &expression.Kind {
        ast::ExprKind::Value(value) => match &value.Datum {
            ast::ValueDatum::Null => return Ok((None, false)),
            ast::ValueDatum::Int64(value) => astersql_types::datum::NewIntDatum(*value),
            ast::ValueDatum::Uint64(value) => astersql_types::datum::NewUintDatum(*value),
            ast::ValueDatum::String(value) | ast::ValueDatum::Decimal(value) => {
                astersql_types::datum::NewStringDatum(value.clone())
            }
            ast::ValueDatum::Bytes(value)
            | ast::ValueDatum::BitLiteral(value)
            | ast::ValueDatum::HexLiteral(value) => {
                astersql_types::datum::NewBytesDatum(value.clone())
            }
            ast::ValueDatum::Bool(value) => astersql_types::datum::NewIntDatum(i64::from(*value)),
            ast::ValueDatum::Float32(bits) => {
                astersql_types::datum::NewFloat32Datum(f32::from_bits(*bits))
            }
            ast::ValueDatum::Float64(bits) => {
                astersql_types::datum::NewFloat64Datum(f64::from_bits(*bits))
            }
        },
        ast::ExprKind::Parentheses(inner) => {
            return typed_literal_to_runtime_value_with_warning(inner, column, flags);
        }
        ast::ExprKind::Unary { Op, .. } if Op == "+" || Op == "-" => {
            let text = crate::dml_runtime::EvalExpr(expression, &HashMap::new(), None)?
                .ok_or_else(|| SessionError::new("numeric literal evaluated to NULL"))?;
            if let Ok(value) = text.parse::<i64>() {
                astersql_types::datum::NewIntDatum(value)
            } else if let Ok(value) = text.parse::<u64>() {
                astersql_types::datum::NewUintDatum(value)
            } else {
                astersql_types::datum::NewStringDatum(text)
            }
        }
        _ => {
            let text = crate::dml_runtime::EvalExpr(expression, &HashMap::new(), None)?;
            return Ok((text, false));
        }
    };
    if is_binary_string_column(column) {
        let mut bytes = datum.GetBytes();
        if column.GetType() == astersql_parser_mysql::r#type::TypeString && column.GetFlen() >= 0 {
            let width = column.GetFlen() as usize;
            if bytes.len() > width {
                return Err(SessionError::new(format!(
                    "Data too long for column '{}'",
                    column.Name.O
                )));
            }
            bytes.resize(width, 0);
        }
        return Ok((
            Some(format!(
                "{BINARY_RUNTIME_PREFIX}{}",
                bytes
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<String>()
            )),
            false,
        ));
    }
    let converted = datum
        .ConvertTo(
            statement_context.TypeCtx().WithFlags(flags),
            &column.FieldType,
        )
        .map_err(|error| {
            if column.GetType() == astersql_parser_mysql::r#type::TypeTiDBVectorFloat32 {
                SessionError::new(error.to_string())
            } else {
                session_error("convert relational column", error)
            }
        })?;
    let conversion_warned = statement_context.WarningCount() > 0;
    datum_to_runtime_value(&converted, Some(column)).map(|value| (value, conversion_warned))
}

pub(super) fn is_typed_literal(expression: &ast::ExprNode) -> bool {
    matches!(expression.Kind, ast::ExprKind::Value(_))
        || matches!(
            &expression.Kind,
            ast::ExprKind::Parentheses(inner) if is_typed_literal(inner)
        )
        || matches!(
            &expression.Kind,
            ast::ExprKind::Unary { Op, V } if (Op == "+" || Op == "-") && is_typed_literal(V)
        )
}

/// Datum 转为运行时可选字符串。
pub(super) const BINARY_RUNTIME_PREFIX: &str = "__astersql_binary_hex__:";

pub(super) fn is_binary_string_column(column: &astersql_meta_model::ColumnInfo) -> bool {
    matches!(
        column.GetType(),
        astersql_parser_mysql::r#type::TypeString
            | astersql_parser_mysql::r#type::TypeVarString
            | astersql_parser_mysql::r#type::TypeVarchar
            | astersql_parser_mysql::r#type::TypeBlob
            | astersql_parser_mysql::r#type::TypeTinyBlob
            | astersql_parser_mysql::r#type::TypeMediumBlob
            | astersql_parser_mysql::r#type::TypeLongBlob
    ) && (column.GetCharset() == "binary"
        || astersql_parser_mysql::r#type::HasBinaryFlag(column.GetFlag()))
}

pub(super) fn binary_runtime_bytes(value: &str) -> Option<Vec<u8>> {
    let hex = value.strip_prefix(BINARY_RUNTIME_PREFIX)?;
    if hex.len() % 2 != 0 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for chunk in hex.as_bytes().chunks(2) {
        let high = (chunk[0] as char).to_digit(16)?;
        let low = (chunk[1] as char).to_digit(16)?;
        bytes.push(((high << 4) | low) as u8);
    }
    Some(bytes)
}

pub(super) fn display_runtime_value(value: String) -> String {
    binary_runtime_bytes(&value)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or(value)
}

pub(super) fn datum_to_runtime_value(
    datum: &astersql_types::datum::Datum,
    column: Option<&astersql_meta_model::ColumnInfo>,
) -> SessionResult<Option<String>> {
    Ok(match datum.Kind() {
        astersql_types::datum::KindNull => None,
        astersql_types::datum::KindInt64 => Some(datum.GetInt64().to_string()),
        astersql_types::datum::KindUint64 => Some(datum.GetUint64().to_string()),
        astersql_types::datum::KindFloat32 => Some(datum.GetFloat32().to_string()),
        astersql_types::datum::KindFloat64 => Some(datum.GetFloat64().to_string()),
        astersql_types::datum::KindMysqlDecimal => Some(datum.GetMysqlDecimal().String()),
        astersql_types::datum::KindMysqlDuration => Some(datum.GetMysqlDuration().to_string()),
        astersql_types::datum::KindMysqlEnum => Some(datum.GetMysqlEnum().String()),
        astersql_types::datum::KindMysqlSet => Some(datum.GetMysqlSet().String()),
        astersql_types::datum::KindMysqlJSON => Some(datum.GetMysqlJSON().String()),
        astersql_types::datum::KindMysqlTime => Some(datum.GetMysqlTime().String()),
        astersql_types::datum::KindVectorFloat32 => Some(datum.GetVectorFloat32().String()),
        astersql_types::datum::KindMysqlBit => {
            // Preserve BIT as a hex-escaped binary literal so ANALYZE can
            // ConvertTo(TypeBit) without losing non-UTF8 payloads.
            let bytes = datum.GetBytes();
            Some(format!(
                "0x{}",
                bytes
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<String>()
            ))
        }
        astersql_types::datum::KindString | astersql_types::datum::KindBytes
            if column.is_some_and(is_binary_string_column) =>
        {
            let bytes = datum.GetBytes();
            if bytes.starts_with(BINARY_RUNTIME_PREFIX.as_bytes())
                && std::str::from_utf8(&bytes)
                    .ok()
                    .is_some_and(|value| binary_runtime_bytes(value).is_some())
            {
                return Ok(Some(
                    String::from_utf8(bytes).expect("validated UTF-8 marker"),
                ));
            }
            Some(format!(
                "{BINARY_RUNTIME_PREFIX}{}",
                bytes
                    .iter()
                    .map(|byte| format!("{byte:02X}"))
                    .collect::<String>()
            ))
        }
        astersql_types::datum::KindString | astersql_types::datum::KindBytes => {
            Some(datum.GetString())
        }
        kind => {
            return Err(SessionError::new(format!(
                "unsupported row datum kind {kind}"
            )));
        }
    })
}

/// Go `table.GetColOriginDefaultValue`: the value seen by rows written before
/// the column was added.
/// 列的 ORIGIN_DEFAULT 运行时值。
pub(super) fn origin_default_runtime_value(
    column: &astersql_meta_model::ColumnInfo,
) -> Option<String> {
    match column.GetOriginDefaultValue()? {
        astersql_meta_model::DefaultValue::Bool(value) => Some(i64::from(value).to_string()),
        astersql_meta_model::DefaultValue::Int(value) => Some(value.to_string()),
        astersql_meta_model::DefaultValue::Uint(value) => Some(value.to_string()),
        astersql_meta_model::DefaultValue::Float(value) => Some(value.to_string()),
        astersql_meta_model::DefaultValue::String(value) => {
            let value = String::from_utf8_lossy(&value).into_owned();
            (!column.DefaultIsExpr || value.as_bytes().first().is_some_and(u8::is_ascii_digit))
                .then_some(value)
        }
    }
}

/// Go `table.GetColDefaultValue`: evaluate the statement-time
/// `CURRENT_TIMESTAMP` default instead of treating its metadata spelling as a
/// temporal literal.
pub(super) fn insert_default_runtime_value(
    column: &astersql_meta_model::ColumnInfo,
) -> Option<String> {
    if column.DefaultIsExpr {
        let astersql_meta_model::DefaultValue::String(value) = column.GetDefaultValue()? else {
            return None;
        };
        let expression = String::from_utf8_lossy(&value);
        let lower = expression.to_ascii_lowercase();
        if lower.starts_with("current_date") {
            return Some(format_system_time(SystemTime::now())[..10].to_owned());
        }
        if lower.starts_with("current_timestamp") || lower.starts_with("now") {
            return Some(format_system_time(SystemTime::now()));
        }
        if lower.contains("vec_from_text")
            && let (Some(start), Some(end)) = (expression.find('['), expression.rfind(']'))
            && start <= end
        {
            return Some(expression[start..=end].to_owned());
        }
        if lower.starts_with("uuid") {
            return Some(uuid::Uuid::new_v4().to_string());
        }
        return Some(expression.into_owned());
    }
    if matches!(
        column.GetType(),
        astersql_parser_mysql::r#type::TypeTimestamp | astersql_parser_mysql::r#type::TypeDatetime
    ) {
        let is_current_timestamp = column.GetDefaultValue().is_some_and(|value| {
            let astersql_meta_model::DefaultValue::String(value) = value else {
                return false;
            };
            let text = String::from_utf8_lossy(&value);
            let normalized = text.trim().to_ascii_lowercase();
            let Some(arguments) = normalized.strip_prefix("current_timestamp") else {
                return false;
            };
            // Earlier Rust DDL padded function defaults as though they were
            // temporal literals. Keep already-persisted catalogs readable;
            // new DDL preserves the function text without this suffix.
            let arguments = arguments
                .split_once(".")
                .filter(|(function, padding)| {
                    function.ends_with(')')
                        && !padding.is_empty()
                        && padding.bytes().all(|byte| byte == b'0')
                })
                .map_or(arguments, |(function, _)| function);
            arguments.is_empty()
                || arguments
                    .strip_prefix('(')
                    .and_then(|arguments| arguments.strip_suffix(')'))
                    .is_some_and(|fsp| {
                        let fsp = fsp.trim();
                        let fsp = fsp
                            .strip_prefix('\'')
                            .and_then(|fsp| fsp.strip_suffix('\''))
                            .unwrap_or(fsp);
                        fsp.is_empty() || fsp.bytes().all(|byte| byte.is_ascii_digit())
                    })
        });
        if is_current_timestamp {
            let now = SystemTime::now();
            let mut value = format_system_time(now);
            let fsp = column.GetDecimal().clamp(0, 6) as usize;
            if fsp > 0 {
                let micros = now
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .subsec_micros();
                let fractional = format!("{micros:06}");
                value.push('.');
                value.push_str(&fractional[..fsp]);
            }
            return Some(value);
        }
    }
    origin_default_runtime_value(column)
}

/// 运行时字符串转回 Datum。
pub(super) fn runtime_value_to_datum(
    value: Option<&String>,
    column: &astersql_meta_model::ColumnInfo,
    flags: astersql_types::Flags,
) -> SessionResult<astersql_types::datum::Datum> {
    let Some(value) = value else {
        return Ok(astersql_types::datum::Datum::default());
    };
    if is_binary_string_column(column) && binary_runtime_bytes(value).is_some() {
        // Store the reversible ASCII marker in rowcodec. Raw non-UTF8 bytes are
        // recovered only at the protocol boundary, which keeps row offsets
        // deterministic while preserving every binary value byte-for-byte.
        return Ok(astersql_types::datum::NewStringDatum(value.clone()));
    }
    if column.GetType() == astersql_parser_mysql::r#type::TypeBit {
        let statement_context = astersql_sessionctx_stmtctx::NewStmtCtx();
        // Values produced by `datum_to_runtime_value` for KindMysqlBit are hex
        // escaped as `0x…`. Decode that form directly so ANALYZE round-trips.
        if let Some(hex) = value
            .strip_prefix("0x")
            .or_else(|| value.strip_prefix("0X"))
        {
            if hex.len() % 2 == 0 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                let mut bytes = Vec::with_capacity(hex.len() / 2);
                for chunk in hex.as_bytes().chunks(2) {
                    let hi = (chunk[0] as char)
                        .to_digit(16)
                        .ok_or_else(|| SessionError::new("invalid BIT hex digit"))?;
                    let lo = (chunk[1] as char)
                        .to_digit(16)
                        .ok_or_else(|| SessionError::new("invalid BIT hex digit"))?;
                    bytes.push(((hi << 4) | lo) as u8);
                }
                return Ok(astersql_types::datum::NewMysqlBitDatum(
                    astersql_types::scalar::BinaryLiteral(bytes),
                ));
            }
        }
        // SQL numeric literals arrive at the row codec as runtime strings. If
        // they are converted from a string Datum, `convertToMysqlBit` treats
        // the UTF-8 bytes themselves as a binary literal (for example `-1`
        // becomes `0x2D31`). Preserve the parser's numeric Datum kind so BIT
        // uses MySQL's signed/unsigned integer conversion rules.
        let source = if let Ok(value) = value.parse::<i64>() {
            astersql_types::datum::NewIntDatum(value)
        } else if let Ok(value) = value.parse::<u64>() {
            astersql_types::datum::NewUintDatum(value)
        } else {
            astersql_types::datum::NewStringDatum(value.clone())
        };
        return source
            .ConvertTo(statement_context.TypeCtx(), &column.FieldType)
            .map_err(|error| session_error("parse BIT column", error));
    }
    if astersql_parser_mysql::util::IsIntegerType(column.GetType()) {
        if astersql_parser_mysql::r#type::HasUnsignedFlag(column.GetFlag()) {
            let parsed = value.parse::<u64>().map_err(|_| {
                SessionError::new(format!("Out of range value for column '{}'", column.Name.O))
            });
            let parsed = parsed?;
            let maximum = match column.GetType() {
                astersql_parser_mysql::r#type::TypeTiny => u64::from(u8::MAX),
                astersql_parser_mysql::r#type::TypeShort => u64::from(u16::MAX),
                astersql_parser_mysql::r#type::TypeInt24 => 0x00ff_ffff,
                astersql_parser_mysql::r#type::TypeLong => u64::from(u32::MAX),
                _ => u64::MAX,
            };
            if parsed > maximum {
                return Err(SessionError::new(format!(
                    "Out of range value for column '{}'",
                    column.Name.O
                )));
            }
            return Ok(astersql_types::datum::NewUintDatum(parsed));
        }
        let parsed = value
            .parse::<i64>()
            .map_err(|error| session_error("parse integer column", error))?;
        let in_range = match column.GetType() {
            astersql_parser_mysql::r#type::TypeTiny => i8::try_from(parsed).is_ok(),
            astersql_parser_mysql::r#type::TypeShort => i16::try_from(parsed).is_ok(),
            astersql_parser_mysql::r#type::TypeInt24 => (-0x80_0000..=0x7f_ffff).contains(&parsed),
            astersql_parser_mysql::r#type::TypeLong => i32::try_from(parsed).is_ok(),
            _ => true,
        };
        if !in_range {
            return Err(SessionError::new(format!(
                "Out of range value for column '{}'",
                column.Name.O
            )));
        }
        return Ok(astersql_types::datum::NewIntDatum(parsed));
    }
    if column.GetType() == astersql_parser_mysql::r#type::TypeJSON {
        let json = astersql_types::json_binary::ParseBinaryJSONFromString(value)
            .map_err(|error| session_error("parse JSON column", error))?;
        return Ok(astersql_types::datum::NewJSONDatum(json));
    }
    if column.GetType() == astersql_parser_mysql::r#type::TypeNewDecimal {
        let mut decimal = astersql_types::decimal::mydecimal::MyDecimal::default();
        decimal
            .FromString(value.as_bytes())
            .map_err(|error| session_error("parse DECIMAL column", error))?;
        return astersql_types::datum::NewDecimalDatum(decimal)
            .ConvertTo(
                astersql_sessionctx_stmtctx::NewStmtCtx()
                    .TypeCtx()
                    .WithFlags(flags),
                &column.FieldType,
            )
            .map_err(|error| session_error("cast DECIMAL column", error));
    }
    if column.GetType() == astersql_parser_mysql::r#type::TypeEnum && value.is_empty() {
        return Ok(astersql_types::datum::NewMysqlEnumDatum(
            astersql_types::datum::Enum {
                Name: String::new(),
                Value: 0,
            },
        ));
    }
    if matches!(
        column.GetType(),
        astersql_parser_mysql::r#type::TypeDate
            | astersql_parser_mysql::r#type::TypeDatetime
            | astersql_parser_mysql::r#type::TypeTimestamp
    ) {
        // Go `ResetContextOfStmt` derives the temporal strictness of a DML
        // statement from `sql_mode`, so a zero month is only rejected while
        // `NO_ZERO_IN_DATE` and strict mode are both on.
        let statement_context = astersql_sessionctx_stmtctx::NewStmtCtx();
        return astersql_types::datum::NewStringDatum(value.clone())
            .ConvertTo(
                statement_context.TypeCtx().WithFlags(flags),
                &column.FieldType,
            )
            .map_err(|error| {
                session_error(&format!("parse temporal column {}", column.Name.O), error)
            });
    }
    astersql_types::datum::NewStringDatum(value.clone())
        .ConvertTo(
            astersql_sessionctx_stmtctx::NewStmtCtx().TypeCtx(),
            &column.FieldType,
        )
        .map_err(|error| session_error("convert relational column", error))
}

pub(super) fn relational_row_handle(
    table: &astersql_meta_model::TableInfo,
    row: &HashMap<String, Option<String>>,
    flags: astersql_types::Flags,
) -> SessionResult<Box<dyn astersql_tablecodec::kv::Handle>> {
    let handle_column = table
        .PKIsHandle
        .then(|| table.GetPkColInfo())
        .flatten()
        .or_else(|| table.GetAutoIncrementColInfo());
    if table.IsCommonHandle {
        let primary_values = table
            .Columns
            .iter()
            .filter(|column| astersql_parser_mysql::r#type::HasPriKeyFlag(column.GetFlag()))
            .map(|column| {
                runtime_value_to_datum(
                    row.get(&column.Name.L).and_then(Option::as_ref),
                    column,
                    flags,
                )
            })
            .collect::<SessionResult<Vec<_>>>()?;
        let encoded = astersql_tablecodec::codec::EncodeKey(
            astersql_tablecodec::time::UTC,
            Vec::new(),
            primary_values,
        )
        .map_err(|error| session_error("encode common relational handle", error))?;
        Ok(Box::new(
            astersql_tablecodec::kv::NewCommonHandle(encoded)
                .map_err(|error| session_error("build common relational handle", error))?,
        ))
    } else {
        let handle_name = handle_column.map_or("_tidb_rowid", |column| column.Name.L.as_str());
        let handle_text = row
            .get(handle_name)
            .and_then(Option::as_ref)
            .ok_or_else(|| SessionError::new("relational DML handle is NULL"))?;
        if handle_column
            .is_some_and(|column| astersql_parser_mysql::r#type::HasUnsignedFlag(column.GetFlag()))
        {
            Ok(Box::new(astersql_tablecodec::kv::IntHandle(
                handle_text
                    .parse::<u64>()
                    .map(|value| value as i64)
                    .map_err(|error| session_error("parse unsigned relational handle", error))?,
            )))
        } else {
            Ok(Box::new(astersql_tablecodec::kv::IntHandle(
                handle_text
                    .parse::<i64>()
                    .map_err(|error| session_error("parse relational handle", error))?,
            )))
        }
    }
}

/// 将关系行编码为表行 KV。
pub(super) fn encode_relational_row(
    table: &astersql_meta_model::TableInfo,
    row: &HashMap<String, Option<String>>,
    flags: astersql_types::Flags,
) -> SessionResult<(kv::Key, Vec<u8>)> {
    encode_relational_row_with_format(table, row, flags, true)
}

/// Encode a relational row using the session-selected row value format.
pub(super) fn encode_relational_row_with_format(
    table: &astersql_meta_model::TableInfo,
    row: &HashMap<String, Option<String>>,
    flags: astersql_types::Flags,
    row_encoder_enabled: bool,
) -> SessionResult<(kv::Key, Vec<u8>)> {
    let handle = relational_row_handle(table, row, flags)?;
    let mut values = Vec::with_capacity(table.Columns.len());
    let mut ids = Vec::with_capacity(table.Columns.len());
    for column in table.Columns.iter().filter(|column| {
        matches!(
            column.State,
            astersql_meta_model::SchemaState::Public
                | astersql_meta_model::SchemaState::WriteOnly
                | astersql_meta_model::SchemaState::WriteReorganization
        )
    }) {
        let value = if let Some(change) = column.ChangeStateInfo.as_ref().filter(|_| {
            matches!(
                column.State,
                astersql_meta_model::SchemaState::WriteOnly
                    | astersql_meta_model::SchemaState::WriteReorganization
            )
        }) {
            let old = table
                .Columns
                .get(change.DependencyColumnOffset as usize)
                .ok_or_else(|| SessionError::new("invalid changing-column dependency offset"))?;
            let datum =
                runtime_value_to_datum(row.get(&old.Name.L).and_then(Option::as_ref), old, flags)?;
            datum
                .ConvertTo(
                    astersql_sessionctx_stmtctx::NewStmtCtx()
                        .TypeCtx()
                        .WithFlags(flags),
                    &column.FieldType,
                )
                .map_err(|e| SessionError::new(e.to_string()))?
        } else {
            runtime_value_to_datum(
                row.get(&column.Name.L).and_then(Option::as_ref),
                column,
                flags,
            )?
        };
        if value.IsNull()
            && column.GetFlag() & astersql_parser_mysql::r#type::PreventNullInsertFlag != 0
        {
            return Err(SessionError::new("[ddl:1138]Invalid use of NULL value"));
        }
        if let Some(field_type) = &column.ChangingFieldType {
            value
                .clone()
                .ConvertTo(
                    astersql_sessionctx_stmtctx::NewStmtCtx()
                        .TypeCtx()
                        .WithFlags(flags),
                    field_type,
                )
                .map_err(|e| SessionError::new(e.to_string()))?;
        }
        values.push(value);
        ids.push(column.ID);
    }
    let value = astersql_tablecodec::EncodeRow(
        Some(astersql_tablecodec::time::UTC),
        values,
        ids,
        Vec::new(),
        None,
        None,
        astersql_tablecodec::rowcodec::Encoder::new(row_encoder_enabled),
    )
    .map_err(|error| session_error("encode relational row", error))?;
    let physical_id = ConcreteSession::row_physical_id(table, row);
    let key = astersql_tablecodec::EncodeRowKeyWithHandle(physical_id, handle);
    Ok((kv::Key(key.0), value))
}

impl ConcreteSession {
    pub(super) fn encode_relational_row_for_write(
        &self,
        table: &astersql_meta_model::TableInfo,
        row: &HashMap<String, Option<String>>,
        flags: astersql_types::Flags,
    ) -> SessionResult<(kv::Key, Vec<u8>)> {
        encode_relational_row_with_format(
            table,
            row,
            flags,
            self.state.borrow().row_encoder_enabled,
        )
    }
}

pub(super) fn relational_index_column<'a>(
    table: &'a astersql_meta_model::TableInfo,
    index_column: &astersql_meta_model::IndexColumn,
) -> SessionResult<&'a astersql_meta_model::ColumnInfo> {
    usize::try_from(index_column.Offset)
        .ok()
        .and_then(|offset| table.Columns.get(offset))
        .or_else(|| {
            table
                .Columns
                .iter()
                .find(|column| column.Name.L == index_column.Name.L)
        })
        .ok_or_else(|| {
            SessionError::new(format!(
                "index references unknown column {}",
                index_column.Name.O
            ))
        })
}

pub(super) fn multi_valued_index_datum(
    value: serde_json::Value,
    column: &astersql_meta_model::ColumnInfo,
) -> SessionResult<astersql_types::datum::Datum> {
    if value.is_null() {
        return Ok(astersql_types::datum::Datum::default());
    }
    let element_type = column.FieldType.ArrayType();
    if astersql_parser_mysql::util::IsIntegerType(element_type.GetType()) {
        if astersql_parser_mysql::r#type::HasUnsignedFlag(element_type.GetFlag()) {
            let value = value
                .as_u64()
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
                .ok_or_else(|| SessionError::new("invalid unsigned multi-valued index element"))?;
            return Ok(astersql_types::datum::NewUintDatum(value));
        }
        let value = value
            .as_i64()
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
            .ok_or_else(|| SessionError::new("invalid signed multi-valued index element"))?;
        return Ok(astersql_types::datum::NewIntDatum(value));
    }
    let mut value = match value {
        serde_json::Value::String(value) => value,
        value => value.to_string(),
    };
    if element_type.GetFlen() >= 0 {
        value = value
            .chars()
            .take(element_type.GetFlen() as usize)
            .collect();
    }
    Ok(astersql_types::datum::NewBytesDatum(value.into_bytes()))
}

pub(super) fn relational_index_value_rows(
    table: &astersql_meta_model::TableInfo,
    index: &astersql_meta_model::IndexInfo,
    row: &HashMap<String, Option<String>>,
    flags: astersql_types::Flags,
) -> SessionResult<Vec<Vec<astersql_types::datum::Datum>>> {
    let mut rows = vec![Vec::with_capacity(index.Columns.len())];
    for index_column in &index.Columns {
        let mut effective = relational_index_column(table, index_column)?.clone();
        if index_column.UseChangingType {
            if let Some(changing) = &effective.ChangingFieldType {
                effective.FieldType = changing.clone();
            }
        }
        let column = &effective;
        if index.MVIndex && column.FieldType.IsArray() {
            let elements = match row.get(&column.Name.L).and_then(Option::as_ref) {
                None => vec![serde_json::Value::Null],
                Some(value) => match serde_json::from_str(value)
                    .map_err(|error| session_error("parse multi-valued index array", error))?
                {
                    serde_json::Value::Array(values) => values,
                    value => vec![value],
                },
            };
            if elements.is_empty() {
                return Ok(Vec::new());
            }
            let elements = elements
                .into_iter()
                .map(|value| multi_valued_index_datum(value, column))
                .collect::<SessionResult<Vec<_>>>()?;
            let mut seen = BTreeSet::new();
            let elements = elements
                .into_iter()
                .filter(|value| {
                    astersql_tablecodec::codec::EncodeKey(
                        astersql_tablecodec::time::UTC,
                        Vec::new(),
                        vec![value.clone()],
                    )
                    .is_ok_and(|encoded| seen.insert(encoded))
                })
                .collect::<Vec<_>>();
            let mut expanded = Vec::with_capacity(rows.len() * elements.len());
            for prefix in &rows {
                for element in &elements {
                    let mut values = prefix.clone();
                    values.push(element.clone());
                    expanded.push(values);
                }
            }
            rows = expanded;
        } else {
            let source = column
                .ChangeStateInfo
                .as_ref()
                .and_then(|info| table.Columns.get(info.DependencyColumnOffset as usize))
                .unwrap_or(column);
            let runtime_value = row.get(&source.Name.L).and_then(Option::as_ref);
            let datum = if is_binary_string_column(column)
                && let Some(bytes) = runtime_value.and_then(|value| binary_runtime_bytes(value))
            {
                astersql_types::datum::NewBytesDatum(bytes)
            } else {
                runtime_value_to_datum(runtime_value, column, flags)?
            };
            for values in &mut rows {
                values.push(datum.clone());
            }
        }
    }
    Ok(rows)
}

pub(super) fn encode_relational_index_value_row(
    table: &astersql_meta_model::TableInfo,
    index: &astersql_meta_model::IndexInfo,
    row: &HashMap<String, Option<String>>,
    flags: astersql_types::Flags,
    indexed_values: Vec<astersql_types::datum::Datum>,
) -> SessionResult<(kv::Key, Vec<u8>)> {
    let mut effective = table.clone();
    for part in &index.Columns {
        if part.UseChangingType {
            let column = &mut effective.Columns[part.Offset as usize];
            if let Some(changing) = &column.ChangingFieldType {
                column.FieldType = changing.clone();
            }
        }
    }
    let table = &effective;
    let handle = relational_row_handle(table, row, flags)?;
    let physical_table_id = ConcreteSession::row_physical_id(table, row);
    let codec_table = astersql_tablecodec::model::TableInfo {
        Columns: table.Columns.clone(),
        Indices: table.Indices.clone(),
        PKIsHandle: table.PKIsHandle,
        IsCommonHandle: table.IsCommonHandle,
        CommonHandleVersion: table.CommonHandleVersion,
        ..Default::default()
    };
    let indexed_values_for_value = indexed_values.clone();
    let (key, distinct) = astersql_tablecodec::GenIndexKey(
        astersql_tablecodec::codec::NewEncoder(astersql_tablecodec::collate::NewCollationEnabled()),
        Some(astersql_tablecodec::time::UTC),
        Box::new(codec_table.clone()),
        Box::new(index.clone()),
        physical_table_id,
        indexed_values,
        Some(handle.Copy()),
        None,
    )
    .map_err(|error| session_error("encode relational index key", error))?;
    let value = astersql_tablecodec::GenIndexValuePortal(
        astersql_tablecodec::collate::NewCollationEnabled(),
        Some(astersql_tablecodec::time::UTC),
        Box::new(codec_table),
        Box::new(index.clone()),
        index.Columns.iter().any(|part| {
            astersql_types::metadata::NeedRestoredData(
                &table.Columns[part.Offset as usize].FieldType,
            )
        }),
        distinct,
        false,
        indexed_values_for_value,
        handle.Copy(),
        0,
        Vec::new(),
        None,
    )
    .map_err(|error| session_error("encode relational index value", error))?;
    Ok((kv::Key(key), value))
}

pub(super) fn encode_relational_index_entries(
    table: &astersql_meta_model::TableInfo,
    row: &HashMap<String, Option<String>>,
    flags: astersql_types::Flags,
) -> SessionResult<Vec<(kv::Key, Vec<u8>)>> {
    let mut entries = Vec::new();
    for index in table.Indices.iter().filter(|index| {
        (index.State == astersql_meta_model::StatePublic
            || ((index.IsChanging() || index.IsRemoving())
                && matches!(
                    index.State,
                    astersql_meta_model::SchemaState::DeleteOnly
                        | astersql_meta_model::SchemaState::WriteOnly
                        | astersql_meta_model::SchemaState::WriteReorganization
                )))
            // A clustered PRIMARY KEY is encoded in the record key itself and
            // has no independent index KV. A non-clustered PRIMARY KEY does,
            // so DML and partition reorganization must maintain it just like
            // every other secondary index.
            && (!index.Primary || !table.HasClusteredIndex())
            && !index.Global
    }) {
        if !index.ConditionExprString.is_empty() {
            let condition = crate::dml_runtime::ParseGeneratedExpr(&index.ConditionExprString)?;
            if !row_matches_simple_where(row, &condition) {
                continue;
            }
        }
        for values in relational_index_value_rows(table, index, row, flags)? {
            entries.push(encode_relational_index_value_row(
                table, index, row, flags, values,
            )?);
        }
    }
    Ok(entries)
}

/// Encode the distinct secondary-unique index entries owned by one row.
///
/// NULL-containing entries are non-distinct in MySQL and therefore cannot be
/// probed as conflicts. Partial and multi-valued indexes reuse the same
/// condition and value expansion as normal index mutation encoding.
pub(super) fn encode_relational_unique_index_entries(
    table: &astersql_meta_model::TableInfo,
    row: &HashMap<String, Option<String>>,
    flags: astersql_types::Flags,
) -> SessionResult<Vec<(kv::Key, Vec<astersql_types::datum::Datum>, String)>> {
    let mut entries = Vec::new();
    for index in table.Indices.iter().filter(|index| {
        index.Unique
            && index.State == astersql_meta_model::StatePublic
            && (!index.Primary || !table.HasClusteredIndex())
            && !index.Global
    }) {
        if !index.ConditionExprString.is_empty() {
            let condition = crate::dml_runtime::ParseGeneratedExpr(&index.ConditionExprString)?;
            if !row_matches_simple_where(row, &condition) {
                continue;
            }
        }
        for values in relational_index_value_rows(table, index, row, flags)? {
            if values.iter().any(astersql_types::datum::Datum::IsNull) {
                continue;
            }
            let (key, _) =
                encode_relational_index_value_row(table, index, row, flags, values.clone())?;
            entries.push((key, values, index.Name.O.clone()));
        }
    }
    Ok(entries)
}

pub(super) fn relational_index_mutations(
    table: &astersql_meta_model::TableInfo,
    old_row: Option<&HashMap<String, Option<String>>>,
    new_row: Option<&HashMap<String, Option<String>>>,
    flags: astersql_types::Flags,
) -> SessionResult<Vec<(kv::Key, Option<Vec<u8>>)>> {
    let mut mutations = BTreeMap::<Vec<u8>, Option<Vec<u8>>>::new();
    for (row, deleting) in old_row
        .into_iter()
        .map(|row| (row, true))
        .chain(new_row.into_iter().map(|row| (row, false)))
    {
        for (key, value) in encode_relational_index_entries(table, row, flags)? {
            let id =
                astersql_tablecodec::DecodeIndexID(astersql_tablecodec::kv::Key(key.0.clone()))
                    .map_err(|e| session_error("decode modifying index", e))?;
            let index = table
                .Indices
                .iter()
                .find(|index| index.ID == id)
                .ok_or_else(|| SessionError::new("index metadata missing"))?;
            if index.State == astersql_meta_model::SchemaState::DeleteOnly && !deleting {
                continue;
            }
            if (index.State != astersql_meta_model::StatePublic
                && index.BackfillState != astersql_meta_model::BackfillStateInapplicable)
                || astersql_tablecodec::IsTempIndexKey(&key.0)
            {
                let distinct = index.Unique
                    && !relational_index_value_rows(table, index, row, flags)?
                        .iter()
                        .flatten()
                        .any(astersql_types::datum::Datum::IsNull);
                let mut temp = key.0.clone();
                astersql_tablecodec::IndexKey2TempIndexKey(&mut temp);
                let merging = matches!(
                    index.BackfillState,
                    astersql_meta_model::BackfillStateReadyToMerge
                        | astersql_meta_model::BackfillStateMerging
                );
                let elem = astersql_tablecodec::TempIndexValueElem {
                    Value: value.clone(),
                    Handle: relational_row_handle(table, row, flags)?,
                    KeyVer: if merging {
                        astersql_tablecodec::TempIndexKeyTypeMerge
                    } else {
                        if index.State == astersql_meta_model::SchemaState::DeleteOnly {
                            astersql_tablecodec::TempIndexKeyTypeDelete
                        } else {
                            astersql_tablecodec::TempIndexKeyTypeBackfill
                        }
                    },
                    Delete: deleting,
                    Distinct: distinct,
                    Global: index.Global,
                };
                let prior = if distinct {
                    mutations.remove(&temp).flatten()
                } else {
                    None
                };
                mutations.insert(temp, Some(elem.Encode(prior)));
                if !merging {
                    continue;
                }
            }
            mutations.insert(key.0, (!deleting).then_some(value));
        }
    }
    Ok(mutations
        .into_iter()
        .map(|(key, value)| (kv::Key(key), value))
        .collect())
}

// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// CREATE TABLE 语句的元数据构建模块。
//
// 本模块负责把解析器产出的 `CREATE TABLE` 抽象语法树（AST）转换为内部的
// 表元数据结构 `model::TableInfo`，是 DDL（数据定义语言）执行流程的核心一环。
// 主要职责包括：
// - 逐列构建 `ColumnInfo`：处理列类型、默认值、注释、生成列（generated column）等选项；
// - 解析表级选项：字符集/排序规则（charset/collation）、AUTO_INCREMENT、
//   SHARD_ROW_ID_BITS（行 ID 分片位数，用于打散写入热点）、放置策略（placement policy）等；
// - 构建索引元数据：主键（聚簇/非聚簇）、唯一索引、普通索引、全文索引、
//   表达式索引（通过隐藏列物化）与多值索引（MV index）；
// - 构建约束与外键：CHECK 约束、FOREIGN KEY 引用动作；
// - 构建分区信息：RANGE/LIST/HASH/KEY 分区定义；
// - 各类合法性校验：重复列名/索引名、AUTO_RANDOM 位数限制、临时表限制等。
//
// 术语说明：
// - 聚簇索引（clustered index）：数据行按主键顺序存储，主键即行的物理定位键；
//   非聚簇则由系统另行分配隐藏的行 ID（row ID）。
// - AUTO_RANDOM：TiDB 特有的主键随机化机制，通过在 BIGINT 主键高位注入
//   分片位（shard bits）来打散写入热点，避免顺序自增主键造成的 Region 写入倾斜
//  （Region 是 TiKV 中数据分片的基本单位）。
// - 本文件是 Go(TiDB) 代码的机械迁移，注释中多处引用对应的 Go 函数名以便对照。

/// Go checkTableInfoValid: full catalog construction checks, then explicit or
/// implicit primary-key visibility. Integer clustered handles bypass the latter.
pub(crate) fn check_table_info_valid(
    table: &mut astersql_meta_model::TableInfo,
) -> Result<(), String> {
    astersql_table_tables::tables::table_from_meta_for_validation(table)?;
    if !table.PKIsHandle && table.GetPrimaryKey().is_some_and(|key| key.Invisible) {
        return Err("[ddl:3522]A primary key index cannot be invisible".into());
    }
    Ok(())
}

use astersql_config_deploymode as deploymode;
use astersql_meta_metabuild as metabuild;
use astersql_meta_model as model;
use astersql_parser as parser;
use astersql_parser_ast as ast;
use astersql_parser_charset as charset;
use astersql_sessionctx_vardef as vardef;
use std::collections::{HashMap, HashSet};

/// 本模块统一使用的构建结果类型，错误类型复用解析器的 Error。
type BuildResult<T> = Result<T, parser::errors::Error>;

/// 构造一个携带指定消息的构建错误。
fn build_error(message: impl Into<String>) -> parser::errors::Error {
    parser::errors::New(message)
}

/// 将无符号整数转换为二进制字面量（BinaryLiteral）字节串。
/// 采用大端序并去掉前导零字节，与 Go 侧 types::NewBinaryLiteralFromUint 行为一致。
fn binary_literal_from_uint(value: u64) -> Vec<u8> {
    // Matches types::NewBinaryLiteralFromUint(value, -1): big-endian bytes with
    // leading zero bytes trimmed (empty for zero).
    let bytes = value.to_be_bytes();
    match bytes.iter().position(|&byte| byte != 0) {
        Some(index) => bytes[index..].to_vec(),
        None => Vec::new(),
    }
}

/// 将 BIT 类型列的默认值归一化为二进制字面量字节串形式。
/// BIT 列的默认值需以字节串保存，以便 ColumnInfo::SetDefaultValue 填充 DefaultValueBit。
fn bit_default_value(value: model::DefaultValue) -> BuildResult<model::DefaultValue> {
    // Go getDefaultValue stores BIT defaults as BinaryLiteral byte strings so
    // ColumnInfo::SetDefaultValue can populate DefaultValueBit.
    match value {
        model::DefaultValue::Int(value) if value >= 0 => Ok(model::DefaultValue::String(
            binary_literal_from_uint(value as u64),
        )),
        model::DefaultValue::Uint(value) => {
            Ok(model::DefaultValue::String(binary_literal_from_uint(value)))
        }
        model::DefaultValue::Bool(value) => Ok(model::DefaultValue::String(
            binary_literal_from_uint(u64::from(value)),
        )),
        model::DefaultValue::String(value) => Ok(model::DefaultValue::String(value)),
        _ => Err(build_error("invalid default value for BIT column")),
    }
}

/// 从字面量表达式中提取列默认值；NULL 字面量返回 None。
/// 非字面量默认值需要 DDL 表达式求值器，这里直接报错。
fn literal_default(expr: &ast::ExprNode) -> BuildResult<Option<model::DefaultValue>> {
    let ast::ExprKind::Value(value) = &expr.Kind else {
        return Err(build_error(
            "non-literal column defaults require the DDL expression evaluator",
        ));
    };
    Ok(match &value.Datum {
        ast::ValueDatum::Null => None,
        ast::ValueDatum::Bool(value) => Some(model::DefaultValue::Bool(*value)),
        ast::ValueDatum::Int64(value) => Some(model::DefaultValue::Int(*value)),
        ast::ValueDatum::Uint64(value) => Some(model::DefaultValue::Uint(*value)),
        ast::ValueDatum::Float32(bits) => {
            Some(model::DefaultValue::Float(f32::from_bits(*bits) as f64))
        }
        ast::ValueDatum::Float64(bits) => Some(model::DefaultValue::Float(f64::from_bits(*bits))),
        ast::ValueDatum::Decimal(value) | ast::ValueDatum::String(value) => {
            Some(model::DefaultValue::String(value.as_bytes().to_vec()))
        }
        ast::ValueDatum::Bytes(value)
        | ast::ValueDatum::BitLiteral(value)
        | ast::ValueDatum::HexLiteral(value) => Some(model::DefaultValue::String(value.clone())),
    })
}

/// 解析列的 DEFAULT 选项表达式，返回 (默认值, 是否为表达式默认值)。
/// 支持三类形式：字面量、带正负号的整数字面量、CURRENT_TIMESTAMP/NOW 函数；
/// 其余表达式以还原后的 SQL 文本保存，并标记 DefaultIsExpr 为 true。
fn column_default(expr: &ast::ExprNode) -> BuildResult<(Option<model::DefaultValue>, bool)> {
    if matches!(expr.Kind, ast::ExprKind::Value(_)) {
        return literal_default(expr).map(|value| (value, false));
    }
    // 处理 "+N" / "-N" 形式的带符号整数默认值。
    if let ast::ExprKind::Unary { Op, V } = &expr.Kind
        && let ast::ExprKind::Value(value) = &V.Kind
    {
        let signed = match (&value.Datum, Op.as_str()) {
            (ast::ValueDatum::Int64(value), "-") => Some(-*value),
            (ast::ValueDatum::Int64(value), "+") => Some(*value),
            _ => None,
        };
        if let Some(value) = signed {
            return Ok((Some(model::DefaultValue::Int(value)), false));
        }
    }
    // CURRENT_TIMESTAMP / NOW 作为默认值时保存其函数文本，不算表达式默认值。
    if let ast::ExprKind::Function { FnName, .. } = &expr.Kind
        && matches!(FnName.L.as_str(), "current_timestamp" | "now")
    {
        return Ok((
            Some(model::DefaultValue::String(
                expression_text(expr)?.into_bytes(),
            )),
            false,
        ));
    }
    let restored = expression_text(expr)?;
    Ok((
        Some(model::DefaultValue::String(restored.into_bytes())),
        true,
    ))
}

/// 校验列默认值与列类型是否兼容：
/// 数值类型列（整数/浮点/定点 DECIMAL）的字符串默认值必须能解析为数字。
fn validate_column_default(
    column_type: u8,
    expression: &ast::ExprNode,
    is_expression: bool,
) -> BuildResult<()> {
    if is_expression {
        return Ok(());
    }
    let ast::ExprKind::Value(value) = &expression.Kind else {
        return Ok(());
    };
    let ast::ValueDatum::String(text) = &value.Datum else {
        return Ok(());
    };
    if matches!(
        column_type,
        model::mysql::TypeTiny
            | model::mysql::TypeShort
            | model::mysql::TypeInt24
            | model::mysql::TypeLong
            | model::mysql::TypeLonglong
            | model::mysql::TypeFloat
            | model::mysql::TypeDouble
            | model::mysql::TypeNewDecimal
    ) && text.parse::<f64>().is_err()
    {
        return Err(build_error("invalid numeric column default"));
    }
    Ok(())
}

/// 提取字面量表达式的文本值，用于列注释（COMMENT）等只接受字面量的场景。
fn literal_text(expr: &ast::ExprNode) -> BuildResult<String> {
    let ast::ExprKind::Value(value) = &expr.Kind else {
        return Err(build_error("column comment must be a literal"));
    };
    Ok(value.text())
}

/// 将表达式 AST 还原为规范化的 SQL 文本，用于保存生成列表达式、
/// CHECK 约束表达式、分区表达式等元数据字段。
/// 只支持元数据中允许出现的表达式形态，其余形态报错。
fn expression_text(expr: &ast::ExprNode) -> BuildResult<String> {
    expression_text_with_column_qualifiers(expr, true)
}

fn expression_text_with_column_qualifiers(
    expr: &ast::ExprNode,
    keep_column_qualifiers: bool,
) -> BuildResult<String> {
    Ok(match &expr.Kind {
        ast::ExprKind::Value(value) => match &value.Datum {
            ast::ValueDatum::String(value) => {
                format!("'{}'", value.replace('\'', "''"))
            }
            ast::ValueDatum::Bytes(value) => {
                let mut encoded = String::with_capacity(value.len() * 2 + 3);
                encoded.push_str("x'");
                for byte in value {
                    use std::fmt::Write;
                    write!(&mut encoded, "{byte:02x}").expect("write byte literal");
                }
                encoded.push('\'');
                encoded
            }
            ast::ValueDatum::BitLiteral(value) => {
                let bits = value
                    .iter()
                    .flat_map(|byte| (0..8).rev().map(move |bit| (byte >> bit) & 1))
                    .map(|bit| char::from(b'0' + bit))
                    .collect::<String>();
                format!("b'{bits}'")
            }
            ast::ValueDatum::HexLiteral(value) => {
                let mut encoded = String::with_capacity(value.len() * 2 + 3);
                encoded.push_str("x'");
                for byte in value {
                    use std::fmt::Write;
                    write!(&mut encoded, "{byte:02x}").expect("write hex literal");
                }
                encoded.push('\'');
                encoded
            }
            _ => value.text(),
        },
        ast::ExprKind::IntroducedValue { Value, Charset, .. } => {
            format!("_{Charset}'{}'", Value.replace('\'', "''"))
        }
        // 列引用按 `库`.`表`.`列` 形式拼接，缺省部分省略。
        ast::ExprKind::Column(column) => {
            let mut parts = Vec::new();
            if keep_column_qualifiers && !column.Schema.O.is_empty() {
                parts.push(format!("`{}`", column.Schema.O));
            }
            if keep_column_qualifiers && !column.Table.O.is_empty() {
                parts.push(format!("`{}`", column.Table.O));
            }
            parts.push(format!("`{}`", column.Name.O));
            parts.join(".")
        }
        ast::ExprKind::Function {
            Schema,
            FnName,
            Args,
        } => {
            let arguments = Args
                .iter()
                .map(|argument| {
                    expression_text_with_column_qualifiers(argument, keep_column_qualifiers)
                })
                .collect::<BuildResult<Vec<_>>>()?
                .join(",");
            if Schema.O.is_empty() {
                format!("{}({arguments})", FnName.O)
            } else {
                format!("`{}`.{}({arguments})", Schema.O, FnName.O)
            }
        }
        ast::ExprKind::Binary { Op, L, R } => {
            format!(
                "{} {} {}",
                expression_text_with_column_qualifiers(L, keep_column_qualifiers)?,
                Op,
                expression_text_with_column_qualifiers(R, keep_column_qualifiers)?
            )
        }
        ast::ExprKind::Unary { Op, V } => format!(
            "{}{}",
            Op,
            expression_text_with_column_qualifiers(V, keep_column_qualifiers)?
        ),
        ast::ExprKind::IsNull { Expr, Not } => {
            format!(
                "{} IS {}NULL",
                expression_text_with_column_qualifiers(Expr, keep_column_qualifiers)?,
                if *Not { "NOT " } else { "" }
            )
        }
        ast::ExprKind::Parentheses(value) => format!(
            "({})",
            expression_text_with_column_qualifiers(value, keep_column_qualifiers)?
        ),
        ast::ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            let mut text = String::from("case");
            if let Some(value) = Value {
                text.push(' ');
                text.push_str(&expression_text_with_column_qualifiers(
                    value,
                    keep_column_qualifiers,
                )?);
            }
            for clause in WhenClauses {
                text.push_str(" when ");
                text.push_str(&expression_text_with_column_qualifiers(
                    &clause.Expr,
                    keep_column_qualifiers,
                )?);
                text.push_str(" then ");
                text.push_str(&expression_text_with_column_qualifiers(
                    &clause.Result,
                    keep_column_qualifiers,
                )?);
            }
            if let Some(value) = ElseClause {
                text.push_str(" else ");
                text.push_str(&expression_text_with_column_qualifiers(
                    value,
                    keep_column_qualifiers,
                )?);
            }
            text.push_str(" end");
            text
        }
        ast::ExprKind::TimeUnit(unit) => format!("{unit:?}").to_uppercase(),
        ast::ExprKind::Cast {
            Expr,
            Tp,
            ExplicitCharSet,
            ..
        } => {
            // CompactStr reports TypeJSON for arrays; restore the cast target so
            // multi-valued indexes keep `char(N) array` / `signed array` text.
            let cast_type = if Tp.IsArray() {
                let mut buffer = Vec::new();
                Tp.FormatAsCastType(&mut buffer, *ExplicitCharSet)
                    .map_err(|error| build_error(error.to_string()))?;
                String::from_utf8(buffer).map_err(|error| build_error(error.to_string()))?
            } else {
                Tp.CompactStr()
            };
            format!(
                "cast({} as {})",
                expression_text_with_column_qualifiers(Expr, keep_column_qualifiers)?,
                cast_type
            )
        }
        ast::ExprKind::MaxValue => "MAXVALUE".to_owned(),
        ast::ExprKind::DefaultValue => "DEFAULT".to_owned(),
        ast::ExprKind::Row(values) => format!(
            "({})",
            values
                .iter()
                .map(|value| expression_text_with_column_qualifiers(value, keep_column_qualifiers))
                .collect::<BuildResult<Vec<_>>>()?
                .join(",")
        ),
        _ => {
            return Err(build_error(
                "expression form is not valid in CREATE TABLE metadata",
            ));
        }
    })
}

/// Validate and restore the restricted predicate accepted by partial indexes.
///
/// TiDB currently permits one comparison between a physical column and a
/// literal, or an `IS [NOT] NULL` test. Predicates involving another column,
/// generated columns, functions, lists, patterns, or subqueries are rejected
/// before index metadata is published.
pub fn BuildPartialIndexCondition(
    expression: &ast::ExprNode,
    table: &model::TableInfo,
) -> Result<String, parser::errors::Error> {
    let physical_column = |expression: &ast::ExprNode| {
        let ast::ExprKind::Column(column) = &expression.Kind else {
            return None;
        };
        table
            .Columns
            .iter()
            .find(|candidate| candidate.Name.L == column.Name.L)
            .filter(|column| column.GeneratedExprString.is_empty())
    };
    let valid = match &expression.Kind {
        ast::ExprKind::Binary { Op, L, R }
            if matches!(Op.as_str(), "=" | "!=" | "<>" | ">" | ">=" | "<" | "<=") =>
        {
            (physical_column(L).is_some() && matches!(R.Kind, ast::ExprKind::Value(_)))
                || (physical_column(R).is_some() && matches!(L.Kind, ast::ExprKind::Value(_)))
        }
        ast::ExprKind::IsNull { Expr, .. } => physical_column(Expr).is_some(),
        _ => false,
    };
    if !valid {
        return Err(build_error(
            "Unsupported DDL operation: invalid partial index predicate",
        ));
    }
    expression_text(expression)
}

/// 由列定义 AST 构建 `ColumnInfo` 列元数据（对应 Go 的 columnDefToCol）。
/// 依次设置类型信息、校验精度约束，再逐个处理列选项
/// （主键、非空、自增、默认值、注释、排序规则、生成列等）。
fn build_column(definition: &ast::ColumnDef, offset: usize) -> BuildResult<model::ColumnInfo> {
    // 列 ID 从 1 开始，Offset 为该列在表中的位置（从 0 开始）。
    let mut column = model::ColumnInfo::New((offset + 1) as i64, definition.Name.Name.clone());
    column.Offset = offset as isize;
    column.FieldType = definition.Tp.clone();
    column.SetFlag(
        column.GetFlag()
            & !(model::mysql::PriKeyFlag
                | model::mysql::UniqueKeyFlag
                | model::mysql::MultipleKeyFlag),
    );
    let (default_flen, default_decimal) =
        model::mysql::GetDefaultFieldLengthAndDecimal(column.GetType());
    if column.GetDecimal() == model::types::UnspecifiedLength {
        column.SetDecimal(default_decimal);
    }
    if column.GetFlen() == model::types::UnspecifiedLength {
        let flen = if model::mysql::HasUnsignedFlag(column.GetFlag())
            && column.GetType() != model::mysql::TypeLonglong
            && model::mysql::IsIntegerType(column.GetType())
        {
            default_flen - 1
        } else {
            default_flen
        };
        column.SetFlen(flen);
    }
    column.State = model::StatePublic;
    column.Version = model::CurrLatestColumnInfoVersion;

    // 时间类型的小数秒精度（fsp）最多 6 位。
    if matches!(
        column.GetType(),
        model::mysql::TypeDuration | model::mysql::TypeDatetime | model::mysql::TypeTimestamp
    ) && column.GetDecimal() > 6
    {
        return Err(build_error(
            "fractional seconds precision must be between 0 and 6",
        ));
    }
    if matches!(
        column.GetType(),
        model::mysql::TypeFloat | model::mysql::TypeDouble
    ) && column.GetFlen() == 0
        && column.GetDecimal() == 0
    {
        return Err(build_error(
            "display width and scale cannot both be zero for FLOAT or DOUBLE",
        ));
    }
    // YEAR 类型隐式为无符号。
    if column.GetType() == model::mysql::TypeYear {
        column.AddFlag(model::mysql::UnsignedFlag);
    }

    let mut has_default_value = false;
    // 逐个处理列选项，把 AST 选项映射到 ColumnInfo 的标志位与字段上。
    for option in &definition.Options {
        match option.Tp {
            ast::ColumnOptionType::None => {}
            ast::ColumnOptionType::PrimaryKey => {
                column.AddFlag(model::mysql::PriKeyFlag | model::mysql::NotNullFlag);
            }
            ast::ColumnOptionType::NotNull => column.AddFlag(model::mysql::NotNullFlag),
            ast::ColumnOptionType::Null => column.DelFlag(model::mysql::NotNullFlag),
            ast::ColumnOptionType::AutoIncrement => {
                column.AddFlag(model::mysql::AutoIncrementFlag | model::mysql::NotNullFlag)
            }
            ast::ColumnOptionType::UniqueKey => column.AddFlag(model::mysql::UniqueKeyFlag),
            ast::ColumnOptionType::DefaultValue => {
                let expression = option
                    .Expr
                    .as_ref()
                    .ok_or_else(|| build_error("column DEFAULT option has no expression"))?;
                let (value, is_expression) = column_default(expression)?;
                validate_column_default(column.GetType(), expression, is_expression)?;
                // BIT 类型的默认值需转换为二进制字面量字节串。
                let value = if column.GetType() == model::mysql::TypeBit {
                    match value {
                        Some(value) => Some(bit_default_value(value)?),
                        None => None,
                    }
                } else {
                    value
                };
                column.DefaultIsExpr = is_expression;
                column
                    .SetOriginDefaultValue(value.clone())
                    .map_err(|error| build_error(error.to_string()))?;
                column
                    .SetDefaultValue(value)
                    .map_err(|error| build_error(error.to_string()))?;
                has_default_value = true;
            }
            ast::ColumnOptionType::OnUpdate => column.AddFlag(model::mysql::OnUpdateNowFlag),
            ast::ColumnOptionType::Comment => {
                column.Comment = match option.Expr.as_ref() {
                    Some(expression) => literal_text(expression)?,
                    None => option.StrValue.clone(),
                };
            }
            ast::ColumnOptionType::Collate => column.SetCollate(option.StrValue.clone()),
            // 生成列（generated column）：值由表达式计算得到，
            // 记录表达式文本、是否落盘存储（STORED）以及依赖的列集合。
            ast::ColumnOptionType::Generated => {
                let expression = option
                    .Expr
                    .as_ref()
                    .ok_or_else(|| build_error("generated column has no expression"))?;
                let mut dependencies = HashSet::new();
                collect_generated_column_dependencies(expression, &mut dependencies)?;
                column.GeneratedExprString =
                    expression_text_with_column_qualifiers(expression, false)?;
                column.GeneratedStored = option.Stored;
                column.Dependences = dependencies.into_iter().map(|name| (name, ())).collect();
            }
            // 外键引用与 CHECK 约束在表级流程中单独处理；AUTO_RANDOM 由
            // set_table_auto_random_bits 统一校验并设置到表元数据。
            ast::ColumnOptionType::Reference | ast::ColumnOptionType::Check => {}
            ast::ColumnOptionType::AutoRandom => {}
            ast::ColumnOptionType::Fulltext
            | ast::ColumnOptionType::ColumnFormat
            | ast::ColumnOptionType::Storage
            | ast::ColumnOptionType::SecondaryEngineAttribute
            | ast::ColumnOptionType::MariaDBRowStart
            | ast::ColumnOptionType::MariaDBRowEnd => {
                return Err(build_error(format!(
                    "column option {:?} is not valid for table metadata construction",
                    option.Tp
                )));
            }
        }
    }
    set_no_default_value_flag(&mut column, has_default_value);
    Ok(column)
}

/// 与 Go `setNoDefaultValueFlag` 一致：无显式默认值的 NOT NULL 普通列
/// 标记为 `NoDefaultValueFlag`，AUTO_INCREMENT 与 TIMESTAMP 例外。
fn set_no_default_value_flag(column: &mut model::ColumnInfo, has_default_value: bool) {
    if !has_default_value
        && model::mysql::HasNotNullFlag(column.GetFlag())
        && !model::mysql::HasAutoIncrementFlag(column.GetFlag())
        && !model::mysql::HasTimestampFlag(column.GetFlag())
    {
        column.AddFlag(model::mysql::NoDefaultValueFlag);
    }
}

/// MySQL `BINARY(n)` 的显式默认值按固定宽度用 NUL 字节补齐。
fn pad_binary_default_value(column: &mut model::ColumnInfo) {
    if column.GetType() != model::mysql::TypeString
        || column.GetCharset() != "binary"
        || column.GetFlen() <= 0
    {
        return;
    }
    let width = column.GetFlen() as usize;
    if let Some(model::DefaultValue::String(value)) = column.DefaultValue.as_mut() {
        value.resize(width, 0);
    }
}

/// DATETIME/TIMESTAMP/TIME 默认值按字段小数秒精度补齐尾部零。
fn normalize_temporal_default_value(column: &mut model::ColumnInfo) {
    let decimal = column.GetDecimal();
    if !matches!(
        column.GetType(),
        model::mysql::TypeDatetime | model::mysql::TypeTimestamp | model::mysql::TypeDuration
    ) || decimal <= 0
        || column.DefaultIsExpr
    {
        return;
    }
    if let Some(model::DefaultValue::String(value)) = column.DefaultValue.as_mut() {
        let normalized = String::from_utf8_lossy(value).trim().to_ascii_lowercase();
        if normalized.starts_with("current_timestamp") || normalized.starts_with("now") {
            return;
        }
        if !value.contains(&b'.') {
            value.push(b'.');
            value.extend(std::iter::repeat_n(b'0', decimal as usize));
        }
    }
}

/// 把索引键列说明转换为 `IndexColumn` 列表，并根据列名查找列偏移。
/// 表达式索引的键在此之前必须已物化为隐藏列，否则报错。
fn index_columns(
    keys: &[ast::IndexPartSpecification],
    offsets: &HashMap<String, usize>,
) -> BuildResult<Vec<model::IndexColumn>> {
    keys.iter()
        .map(|key| {
            let name = key
                .Column
                .as_ref()
                .map(|column| column.Name.clone())
                .ok_or_else(|| build_error("expression index has no materialized hidden column"))?;
            let offset = offsets
                .get(&name.L)
                .copied()
                .ok_or_else(|| build_error(format!("key column '{}' does not exist", name.O)))?;
            Ok(model::IndexColumn {
                Name: name,
                Offset: offset as isize,
                Length: key.Length,
                UseChangingType: false,
            })
        })
        .collect()
}

/// 构造一条索引元数据 `IndexInfo`。
/// 索引类型默认为 Btree；可见性（INVISIBLE）、全局索引（Global，
/// 即分区表上跨分区的索引）与注释来自可选的索引选项。
fn make_index(
    id: i64,
    table_name: &ast::CIStr,
    name: ast::CIStr,
    columns: Vec<model::IndexColumn>,
    primary: bool,
    unique: bool,
    option: Option<&ast::IndexOption>,
) -> model::IndexInfo {
    let index_type = option
        .map(|option| option.Tp)
        .filter(|index_type| *index_type != ast::IndexType::Invalid)
        .unwrap_or(ast::IndexType::Btree);
    let index_type = match index_type {
        ast::IndexType::Invalid => model::ast::IndexType::Invalid,
        ast::IndexType::Btree => model::ast::IndexType::Btree,
        ast::IndexType::Hash => model::ast::IndexType::Hash,
        ast::IndexType::Rtree => model::ast::IndexType::Rtree,
        ast::IndexType::Hypo => model::ast::IndexType::Hypo,
        ast::IndexType::HNSW => model::ast::IndexType::HNSW,
        ast::IndexType::Inverted => model::ast::IndexType::Inverted,
    };
    model::IndexInfo {
        ID: id,
        Name: name,
        Table: table_name.clone(),
        Columns: columns,
        State: model::StatePublic,
        Tp: index_type,
        Unique: unique,
        Primary: primary,
        Invisible: option
            .is_some_and(|option| option.Visibility == ast::IndexVisibility::Invisible),
        Global: option.is_some_and(|option| option.Global),
        Comment: option
            .map(|option| option.Comment.clone())
            .unwrap_or_default(),
        ..Default::default()
    }
}

/// 获取语句中声明的主键类型（CLUSTERED / NONCLUSTERED / 默认）。
/// 先看表级 PRIMARY KEY 约束的选项，再看列级 PRIMARY KEY 选项。
fn primary_key_type(statement: &ast::CreateTableStmt) -> ast::PrimaryKeyType {
    statement
        .Constraints
        .iter()
        .find(|constraint| constraint.Tp == ast::ConstraintType::PrimaryKey)
        .and_then(|constraint| constraint.Option.as_ref())
        .map(|option| option.PrimaryKeyTp)
        .or_else(|| {
            statement.Cols.iter().find_map(|column| {
                column
                    .Options
                    .iter()
                    .find(|option| option.Tp == ast::ColumnOptionType::PrimaryKey)
                    .map(|option| option.PrimaryKeyTp)
            })
        })
        .unwrap_or_default()
}

/// 判断主键是否应作为聚簇索引（clustered index，数据按主键组织存储）。
/// 显式声明优先；默认时依据会话的 clustered_index 配置：
/// ON 恒为聚簇，INT_ONLY 仅当主键是单个整数列时聚簇。
fn should_cluster_primary_key<C: ?Sized + 'static, E: 'static>(
    context: &metabuild::Context<C, E>,
    requested: ast::PrimaryKeyType,
    single_integer_primary_key: bool,
) -> bool {
    match requested {
        ast::PrimaryKeyType::Clustered => true,
        ast::PrimaryKeyType::NonClustered => false,
        ast::PrimaryKeyType::Default => match context.GetClusteredIndexDefMode() {
            // metabuild defaults mirror vardef: 1 = ON, 0 = INT_ONLY.
            1 => true,
            0 => single_integer_primary_key,
            _ => false,
        },
    }
}

/// 由 PARTITION BY 子句构建分区元数据 `PartitionInfo`。
/// 支持 RANGE（LESS THAN 值列表）、LIST（IN 值列表）、HASH/KEY 分区；
/// 校验分区名不重复，并收集每个分区的注释与放置策略。
fn build_partition_info(options: &ast::PartitionOptions) -> BuildResult<model::PartitionInfo> {
    // 子分区（subpartition）只允许出现在 RANGE 或 LIST 分区之下。
    if options.Sub.is_some()
        && matches!(
            options.PartitionMethod.Tp,
            ast::PartitionType::Hash | ast::PartitionType::Key
        )
    {
        return Err(build_error(
            "subpartitioning is only accepted with RANGE or LIST partitioning",
        ));
    }
    let method = &options.PartitionMethod;
    let partition_type = match method.Tp {
        ast::PartitionType::None => model::ast::model::PartitionTypeNone,
        ast::PartitionType::Key => model::ast::model::PartitionTypeKey,
        ast::PartitionType::Hash => model::ast::model::PartitionTypeHash,
        ast::PartitionType::Range => model::ast::model::PartitionTypeRange,
        ast::PartitionType::List => model::ast::model::PartitionTypeList,
        ast::PartitionType::SystemTime => model::ast::model::PartitionTypeSystemTime,
    };
    let mut definitions = Vec::with_capacity(options.Definitions.len());
    let mut names = HashSet::new();
    for definition in &options.Definitions {
        if !names.insert(definition.Name.L.clone()) {
            return Err(build_error(format!(
                "duplicate partition name '{}'",
                definition.Name.O
            )));
        }
        // RANGE 分区记录 LESS THAN 表达式文本；LIST 分区记录 IN 的值行。
        let (less_than, in_values) = match &definition.Clause {
            ast::PartitionDefinitionClause::None => (Vec::new(), Vec::new()),
            ast::PartitionDefinitionClause::LessThan(values) => (
                values
                    .iter()
                    .map(expression_text)
                    .collect::<BuildResult<Vec<_>>>()?,
                Vec::new(),
            ),
            ast::PartitionDefinitionClause::In(values) => (
                Vec::new(),
                values
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(expression_text)
                            .collect::<BuildResult<Vec<_>>>()
                    })
                    .collect::<BuildResult<Vec<_>>>()?,
            ),
            ast::PartitionDefinitionClause::History { .. } => {
                return Err(build_error(
                    "system-time partition history is not valid for this table",
                ));
            }
        };
        let mut comment = String::new();
        let mut placement = None;
        for option in &definition.Options {
            match option.Tp {
                ast::TableOptionType::Comment => comment = option.StrValue.clone(),
                ast::TableOptionType::Policy => {
                    placement = Some(model::PolicyRefInfo {
                        Name: ast::NewCIStr(&option.StrValue),
                        ..Default::default()
                    });
                }
                _ => {}
            }
        }
        definitions.push(model::PartitionDefinition {
            Name: definition.Name.clone(),
            LessThan: less_than,
            InValues: in_values,
            PlacementPolicyRef: placement,
            Comment: comment,
            ..Default::default()
        });
    }
    if !method.Expr.as_ref().is_some_and(|expression| {
        matches!(
            &expression.Kind,
            ast::ExprKind::Function { FnName, .. } if FnName.L == "extract"
        )
    }) {
        validate_range_partition_boundaries(partition_type, &definitions)?;
    }
    Ok(model::PartitionInfo {
        Type: partition_type,
        Expr: method
            .Expr
            .as_ref()
            .map(expression_text)
            .transpose()?
            .unwrap_or_default(),
        Columns: method
            .ColumnNames
            .iter()
            .map(|column| column.Name.clone())
            .collect(),
        Enable: true,
        IsEmptyColumns: method.ColumnNames.is_empty(),
        Num: if method.Num == 0 {
            definitions.len() as u64
        } else {
            method.Num
        },
        Definitions: definitions,
        ..Default::default()
    })
}

/// Build canonical partition metadata for ALTER TABLE ... PARTITION BY.
pub fn BuildPartitionInfo(options: &ast::PartitionOptions) -> BuildResult<model::PartitionInfo> {
    build_partition_info(options)
}

fn validate_extract_partition_expression(
    table: &model::TableInfo,
    partition: &model::PartitionInfo,
) -> BuildResult<()> {
    let expression = partition.Expr.trim().to_ascii_lowercase();
    let Some(arguments) = expression
        .strip_prefix("extract(")
        .and_then(|expression| expression.strip_suffix(')'))
    else {
        return Ok(());
    };
    let Some((unit, column_name)) = arguments
        .split_once(" from ")
        .or_else(|| arguments.split_once(','))
    else {
        return Err(build_error(
            "Constant, random or timezone-dependent expressions in (sub)partitioning function are not allowed",
        ));
    };
    let unit = unit.trim().trim_matches('`').trim_matches('\'');
    let compact_unit = unit.replace('_', "");
    let column_name = column_name.trim().trim_matches('`');
    let Some(column) = table
        .Columns
        .iter()
        .find(|column| column.Name.L == column_name)
    else {
        return Err(build_error(
            "Constant, random or timezone-dependent expressions in (sub)partitioning function are not allowed",
        ));
    };
    let allowed = match column.GetType() {
        model::mysql::TypeDate => matches!(
            compact_unit.as_str(),
            "year" | "quarter" | "yearmonth" | "month" | "day"
        ),
        model::mysql::TypeDatetime => matches!(
            compact_unit.as_str(),
            "year"
                | "quarter"
                | "yearmonth"
                | "month"
                | "day"
                | "dayhour"
                | "dayminute"
                | "daysecond"
                | "daymicrosecond"
                | "hour"
                | "hourminute"
                | "hoursecond"
                | "hourmicrosecond"
                | "minute"
                | "minutesecond"
                | "minutemicrosecond"
                | "second"
                | "secondmicrosecond"
                | "microsecond"
        ),
        model::mysql::TypeDuration => matches!(
            compact_unit.as_str(),
            "hour"
                | "hourminute"
                | "hoursecond"
                | "hourmicrosecond"
                | "minute"
                | "minutesecond"
                | "minutemicrosecond"
                | "second"
                | "secondmicrosecond"
                | "microsecond"
        ),
        _ => false,
    };
    if allowed {
        Ok(())
    } else {
        Err(build_error(
            "Constant, random or timezone-dependent expressions in (sub)partitioning function are not allowed",
        ))
    }
}

/// 校验 CREATE TABLE 的 RANGE 上界严格递增。
///
/// Go 的 `checkPartitionDefinitionConstraints` 在建表阶段就拒绝递减或重复
/// 的 `VALUES LESS THAN`。此前 Rust 只检查了分区名，导致非法定义进入 catalog，
/// 后续 DDL 才暴露错误。这里对可直接解析的整数边界执行同样的建表期校验；
/// 非整数表达式交由表达式校验路径处理，避免把合法的日期/函数边界误判为字符串序。
fn validate_range_partition_boundaries(
    partition_type: model::ast::PartitionType,
    definitions: &[model::PartitionDefinition],
) -> BuildResult<()> {
    if partition_type != model::ast::model::PartitionTypeRange {
        return Ok(());
    }

    let mut previous: Option<Vec<i128>> = None;
    for (index, definition) in definitions.iter().enumerate() {
        if definition.LessThan.is_empty() {
            continue;
        }
        let contains_maxvalue = definition
            .LessThan
            .iter()
            .any(|boundary| boundary.eq_ignore_ascii_case("MAXVALUE"));
        // RANGE COLUMNS compares tuples lexicographically.  A trailing
        // MAXVALUE such as `(30, MAXVALUE)` only closes the sub-range whose
        // first column is 30, so a later `(40, 'm')` boundary remains valid.
        // Only MAXVALUE in the leading position makes the whole range final.
        let is_terminal_maxvalue = definition.LessThan[0].eq_ignore_ascii_case("MAXVALUE");
        if is_terminal_maxvalue && index + 1 != definitions.len() {
            return Err(build_error(
                "VALUES LESS THAN MAXVALUE must be the last partition",
            ));
        }
        let current = definition
            .LessThan
            .iter()
            .map(|boundary| boundary.parse::<i128>())
            .collect::<Result<Vec<_>, _>>();
        if let (Some(previous), Ok(current)) = (&previous, &current) {
            if current <= previous {
                return Err(build_error(
                    "VALUES LESS THAN value must be strictly increasing for each partition",
                ));
            }
        }
        if let Ok(current) = current {
            previous = Some(current);
        } else if !contains_maxvalue {
            // Keep non-numeric expression boundaries on the existing expression
            // validation path; do not compare their source text lexicographically.
            previous = None;
        }
    }
    Ok(())
}

/// 解析表的字符集与排序规则（collation，决定字符串比较与排序的规则）。
/// 优先级：表选项 > 库默认 > 由排序规则反推字符集 > utf8mb4 兜底；
/// 最后校验字符集与排序规则相互匹配。
fn resolve_charset_collation<C: ?Sized + 'static, E: 'static>(
    context: &metabuild::Context<C, E>,
    options: &[ast::TableOption],
    db_charset: &str,
    db_collate: &str,
) -> BuildResult<(String, String)> {
    let mut table_charset = String::new();
    let mut table_collate = String::new();
    for option in options {
        match option.Tp {
            ast::TableOptionType::Charset => table_charset = option.StrValue.to_lowercase(),
            ast::TableOptionType::Collate => table_collate = option.StrValue.to_lowercase(),
            _ => {}
        }
    }

    if table_charset.is_empty() && table_collate.is_empty() {
        table_charset = db_charset.to_lowercase();
        table_collate = db_collate.to_lowercase();
    }
    if table_charset.is_empty() && !table_collate.is_empty() {
        table_charset = charset::charset::GetCollationByName(&table_collate)
            .map_err(|error| build_error(error.to_string()))?
            .CharsetName;
    }
    if table_charset.is_empty() {
        table_charset = "utf8mb4".to_owned();
    }
    let charset_info = charset::charset::GetCharsetInfo(&table_charset)
        .map_err(|error| build_error(error.to_string()))?;
    if table_collate.is_empty() {
        table_collate = if table_charset == "utf8mb4" {
            context.GetDefaultCollationForUTF8MB4()
        } else {
            charset_info.DefaultCollation
        };
    } else {
        let collation = charset::charset::GetCollationByName(&table_collate)
            .map_err(|error| build_error(error.to_string()))?;
        if collation.CharsetName != table_charset {
            return Err(build_error(format!(
                "collation '{}' is not valid for character set '{}'",
                table_collate, table_charset
            )));
        }
    }
    Ok((table_charset, table_collate))
}

/// 判断列类型是否需要字符集信息（字符串/文本/枚举/集合类）；
/// 其余类型统一使用 "binary"。
fn type_needs_charset(column_type: u8) -> bool {
    matches!(
        column_type,
        model::mysql::TypeString
            | model::mysql::TypeVarchar
            | model::mysql::TypeVarString
            | model::mysql::TypeBlob
            | model::mysql::TypeTinyBlob
            | model::mysql::TypeMediumBlob
            | model::mysql::TypeLongBlob
            | model::mysql::TypeEnum
            | model::mysql::TypeSet
    )
}

/// 将外键引用动作（ON DELETE/ON UPDATE 后的 RESTRICT、CASCADE 等）
/// 映射为存入元数据的整数编码。
fn refer_option(option: ast::ReferOptionType) -> i32 {
    match option {
        ast::ReferOptionType::None => 0,
        ast::ReferOptionType::Restrict => 1,
        ast::ReferOptionType::Cascade => 2,
        ast::ReferOptionType::SetNull => 3,
        ast::ReferOptionType::NoAction => 4,
        ast::ReferOptionType::SetDefault => 5,
    }
}

/// 递归收集表达式中引用的所有列名（小写形式），用于依赖分析。
fn collect_expression_columns(expression: &ast::ExprNode, columns: &mut HashSet<String>) {
    match &expression.Kind {
        ast::ExprKind::Column(column) => {
            columns.insert(column.Name.L.clone());
        }
        ast::ExprKind::Function { Args, .. } | ast::ExprKind::Row(Args) => {
            for argument in Args {
                collect_expression_columns(argument, columns);
            }
        }
        ast::ExprKind::Binary { L, R, .. } => {
            collect_expression_columns(L, columns);
            collect_expression_columns(R, columns);
        }
        ast::ExprKind::Unary { V, .. }
        | ast::ExprKind::Parentheses(V)
        | ast::ExprKind::Cast { Expr: V, .. } => {
            collect_expression_columns(V, columns);
        }
        _ => {}
    }
}

/// 递归收集生成列表达式依赖的列名。
fn collect_generated_column_dependencies(
    expression: &ast::ExprNode,
    columns: &mut HashSet<String>,
) -> BuildResult<()> {
    match &expression.Kind {
        ast::ExprKind::Column(column) => {
            columns.insert(column.Name.L.clone());
        }
        ast::ExprKind::Function { Args, .. } | ast::ExprKind::Row(Args) => {
            for argument in Args {
                collect_generated_column_dependencies(argument, columns)?;
            }
        }
        ast::ExprKind::Binary { L, R, .. } => {
            collect_generated_column_dependencies(L, columns)?;
            collect_generated_column_dependencies(R, columns)?;
        }
        ast::ExprKind::Unary { V, .. }
        | ast::ExprKind::Parentheses(V)
        | ast::ExprKind::Cast { Expr: V, .. } => {
            collect_generated_column_dependencies(V, columns)?;
        }
        ast::ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            if let Some(value) = Value {
                collect_generated_column_dependencies(value, columns)?;
            }
            for clause in WhenClauses {
                collect_generated_column_dependencies(&clause.Expr, columns)?;
                collect_generated_column_dependencies(&clause.Result, columns)?;
            }
            if let Some(value) = ElseClause {
                collect_generated_column_dependencies(value, columns)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// 校验生成列表达式里的限定名与正在创建的库表一致。
/// Go `findDependedColumnNames` 允许当前库/表限定，只拒绝不同的非空限定名。
fn validate_generated_column_qualifiers(
    expression: &ast::ExprNode,
    schema_name: &ast::CIStr,
    table_name: &ast::CIStr,
) -> BuildResult<()> {
    match &expression.Kind {
        ast::ExprKind::Column(column) => {
            if !column.Schema.L.is_empty()
                && !schema_name.L.is_empty()
                && column.Schema.L != schema_name.L
            {
                return Err(build_error(format!(
                    "wrong database name '{}'",
                    column.Schema.O
                )));
            }
            if !column.Table.L.is_empty()
                && !table_name.L.is_empty()
                && column.Table.L != table_name.L
            {
                return Err(build_error(format!(
                    "wrong table name '{}'",
                    column.Table.O
                )));
            }
        }
        ast::ExprKind::Function { Args, .. } | ast::ExprKind::Row(Args) => {
            for argument in Args {
                validate_generated_column_qualifiers(argument, schema_name, table_name)?;
            }
        }
        ast::ExprKind::Binary { L, R, .. } => {
            validate_generated_column_qualifiers(L, schema_name, table_name)?;
            validate_generated_column_qualifiers(R, schema_name, table_name)?;
        }
        ast::ExprKind::Unary { V, .. }
        | ast::ExprKind::Parentheses(V)
        | ast::ExprKind::Cast { Expr: V, .. } => {
            validate_generated_column_qualifiers(V, schema_name, table_name)?;
        }
        ast::ExprKind::Case {
            Value,
            WhenClauses,
            ElseClause,
        } => {
            if let Some(value) = Value {
                validate_generated_column_qualifiers(value, schema_name, table_name)?;
            }
            for clause in WhenClauses {
                validate_generated_column_qualifiers(&clause.Expr, schema_name, table_name)?;
                validate_generated_column_qualifiers(&clause.Result, schema_name, table_name)?;
            }
            if let Some(value) = ElseClause {
                validate_generated_column_qualifiers(value, schema_name, table_name)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Infer the hidden column FieldType for an expression-index key part.
/// Multi-valued indexes use `cast(... as ... array)`; the cast target carries
/// the array flag (Go builds the expression then reads `expr.GetType()`).
///
/// 推断表达式索引键部分对应隐藏列的字段类型：
/// - CAST 表达式直接取目标类型（多值索引 `cast(... as ... array)` 由此携带数组标记）；
/// - 括号表达式递归内部；
/// - 其余情况取表达式引用的任一列的类型，找不到则回退到未指定类型。
fn field_type_for_expression_index(
    expression: &ast::ExprNode,
    columns: &[model::ColumnInfo],
    offsets: &HashMap<String, usize>,
) -> model::types::FieldType {
    match &expression.Kind {
        ast::ExprKind::Cast { Tp, .. } => Tp.clone(),
        ast::ExprKind::Parentheses(inner) => {
            field_type_for_expression_index(inner, columns, offsets)
        }
        _ => {
            let mut dependencies = HashSet::new();
            collect_expression_columns(expression, &mut dependencies);
            dependencies
                .iter()
                .find_map(|name| {
                    offsets
                        .get(name)
                        .map(|offset| columns[*offset].FieldType.clone())
                })
                .unwrap_or_else(|| model::types::NewFieldType(model::mysql::TypeUnspecified))
        }
    }
}

/// 将表达式索引的表达式键物化为隐藏虚拟列。
/// 表达式索引（如 `INDEX ((a+1))`）无法直接索引表达式，
/// 需要为每个表达式键生成一个名为 `_V$_<索引名>_<序号>` 的隐藏生成列，
/// 并把键部分改写为对该隐藏列的引用。
fn materialize_expression_index_columns(
    constraint: &mut ast::Constraint,
    columns: &mut Vec<model::ColumnInfo>,
    offsets: &mut HashMap<String, usize>,
) -> BuildResult<()> {
    for (part_offset, part) in constraint.Keys.iter_mut().enumerate() {
        // 仅处理带表达式的键部分；普通列键跳过。
        let Some(expression) = part.Expr.take() else {
            continue;
        };
        let hidden_name = ast::NewCIStr(&format!("_V$_{}_{}", constraint.Name, part_offset));
        if offsets.contains_key(&hidden_name.L) {
            return Err(build_error(format!(
                "hidden expression-index column '{}' already exists",
                hidden_name.O
            )));
        }
        let mut dependencies = HashSet::new();
        collect_expression_columns(&expression, &mut dependencies);
        for dependency in &dependencies {
            if !offsets.contains_key(dependency) {
                return Err(build_error(format!(
                    "expression index references unknown column '{dependency}'"
                )));
            }
        }
        // Capture generated text before mutating FieldType charset for arrays.
        let generated = expression_text(&expression)?;
        let mut field_type = field_type_for_expression_index(&expression, columns, offsets);
        // For an array, the collation is set to "binary". The collation has no
        // effect on the array itself (as it's usually regarded as a JSON), but
        // will influence how TiKV handles the index value.
        // 数组类型（多值索引）的字符集/排序规则固定为 binary：
        // 排序规则对数组本身（视作 JSON）无意义，但会影响 TiKV 对索引值的处理。
        if field_type.IsArray() {
            field_type.SetCharset("binary".to_owned());
            field_type.SetCollate("binary".to_owned());
        }
        let offset = columns.len();
        let mut hidden = model::ColumnInfo::New((offset + 1) as i64, hidden_name.clone());
        hidden.Offset = offset as isize;
        hidden.FieldType = field_type;
        hidden.State = model::StatePublic;
        hidden.Version = model::CurrLatestColumnInfoVersion;
        hidden.Hidden = true;
        hidden.GeneratedExprString = generated;
        hidden.Dependences = dependencies.into_iter().map(|name| (name, ())).collect();
        columns.push(hidden);
        offsets.insert(hidden_name.L.clone(), offset);
        part.Column = Some(ast::ColumnName {
            Name: hidden_name,
            ..Default::default()
        });
        part.Length = model::types::UnspecifiedLength;
    }
    Ok(())
}

/// Materialize expression-index key parts for `ALTER TABLE ADD INDEX`.
///
/// CREATE TABLE uses the same helper internally. Exposing this narrow wrapper
/// keeps ALTER TABLE on the identical hidden-column naming, type inference,
/// dependency validation, and generated-expression path.
pub fn MaterializeExpressionIndexColumns(
    constraint: &mut ast::Constraint,
    columns: &mut Vec<model::ColumnInfo>,
    offsets: &mut HashMap<String, usize>,
) -> BuildResult<()> {
    materialize_expression_index_columns(constraint, columns, offsets)
}

/// 判断列定义中是否包含指定类型的列选项。
fn contains_column_option(definition: &ast::ColumnDef, option_type: ast::ColumnOptionType) -> bool {
    definition
        .Options
        .iter()
        .any(|option| option.Tp == option_type)
}

/// 校验约束名不重复，并为未命名的约束自动生成名字。
/// 外键与其余约束使用独立的命名空间；CHECK 约束按 `<表名>_chk_N` 命名，
/// 索引类约束默认取首列名，冲突时追加 `_2`、`_3` 等后缀。
fn check_constraint_names(
    table_name: &ast::CIStr,
    constraints: &mut [ast::Constraint],
) -> BuildResult<()> {
    // 第一阶段：登记显式命名的约束并检查重名。
    let mut ordinary_names = HashSet::new();
    let mut foreign_key_names = HashSet::new();
    for constraint in constraints.iter() {
        if constraint.Name.is_empty() {
            continue;
        }
        let namespace = if constraint.Tp == ast::ConstraintType::ForeignKey {
            &mut foreign_key_names
        } else {
            &mut ordinary_names
        };
        if !namespace.insert(constraint.Name.to_lowercase()) {
            return Err(build_error(format!(
                "duplicate constraint name '{}'",
                constraint.Name
            )));
        }
    }

    // 第二阶段：为未命名的约束生成不冲突的名字。
    let mut check_sequence = 1;
    for constraint in constraints.iter_mut() {
        if !constraint.Name.is_empty() || constraint.Tp == ast::ConstraintType::ForeignKey {
            continue;
        }
        let base = match constraint.Tp {
            ast::ConstraintType::Check => loop {
                let candidate = format!("{}_chk_{check_sequence}", table_name.L);
                check_sequence += 1;
                if !ordinary_names.contains(&candidate) {
                    break candidate;
                }
            },
            ast::ConstraintType::PrimaryKey | ast::ConstraintType::None => continue,
            _ => constraint
                .Keys
                .first()
                .and_then(|key| key.Column.as_ref().map(|column| column.Name.O.clone()))
                .unwrap_or_else(|| "_V$".to_owned()),
        };
        let mut candidate = base.clone();
        let mut suffix = 2;
        while !ordinary_names.insert(candidate.to_lowercase()) {
            candidate = format!("{base}_{suffix}");
            suffix += 1;
        }
        constraint.Name = candidate;
    }
    Ok(())
}

/// 校验并设置表的 AUTO_RANDOM 位数配置。
/// AUTO_RANDOM 在 BIGINT 主键高位注入随机分片位以打散写入热点，要求：
/// 列必须是 BIGINT 且为聚簇主键的第一列，不能与 AUTO_INCREMENT/DEFAULT 共存；
/// 分片位（shard bits）1..=15、范围位（range bits）32..=64，
/// 且剩余的自增位（range - shard）至少 27 位。
fn set_table_auto_random_bits(
    table: &mut model::TableInfo,
    definitions: &[ast::ColumnDef],
) -> BuildResult<()> {
    for definition in definitions {
        let Some(option) = definition
            .Options
            .iter()
            .find(|option| option.Tp == ast::ColumnOptionType::AutoRandom)
        else {
            continue;
        };
        if definition.Tp.GetType() != model::mysql::TypeLonglong {
            return Err(build_error(
                "AUTO_RANDOM is only supported on BIGINT columns",
            ));
        }
        let first_primary_column = table
            .Indices
            .iter()
            .find(|index| index.Primary)
            .and_then(|index| index.Columns.first())
            .map(|column| column.Name.L.as_str())
            .or_else(|| table.GetPkColInfo().map(|column| column.Name.L.as_str()));
        if !table.HasClusteredIndex()
            || first_primary_column != Some(definition.Name.Name.L.as_str())
        {
            return Err(build_error(
                "AUTO_RANDOM column must be the first column of a clustered primary key",
            ));
        }
        if contains_column_option(definition, ast::ColumnOptionType::AutoIncrement) {
            return Err(build_error(
                "AUTO_RANDOM is incompatible with AUTO_INCREMENT",
            ));
        }
        if contains_column_option(definition, ast::ColumnOptionType::DefaultValue) {
            return Err(build_error("AUTO_RANDOM is incompatible with DEFAULT"));
        }
        let shard_bits = match option.AutoRandOpt.ShardBits {
            model::types::UnspecifiedLength => 5,
            bits if (1..=15).contains(&bits) => bits as u64,
            _ => {
                return Err(build_error(
                    "AUTO_RANDOM shard bits must be between 1 and 15",
                ));
            }
        };
        let range_bits = match option.AutoRandOpt.RangeBits {
            model::types::UnspecifiedLength => 64,
            bits if (32..=64).contains(&bits) => bits as u64,
            _ => {
                return Err(build_error(
                    "AUTO_RANDOM range bits must be between 32 and 64",
                ));
            }
        };
        if range_bits.saturating_sub(shard_bits) < 27 {
            return Err(build_error(
                "AUTO_RANDOM incremental bits must be at least 27",
            ));
        }
        table.AutoRandomBits = shard_bits;
        table.AutoRandomRangeBits = range_bits;
    }
    Ok(())
}

/// Builds table metadata from a parsed CREATE TABLE statement.
///
/// 由解析后的 CREATE TABLE 语句构建完整的表元数据 `TableInfo`。
/// 这是本模块的主入口，整体流程：
/// 1. 解析表级字符集/排序规则并逐列构建 `ColumnInfo`；
/// 2. 物化表达式索引的隐藏列，处理约束命名；
/// 3. 应用表选项（AUTO_INCREMENT、SHARD_ROW_ID_BITS、放置策略、亲和性等）；
/// 4. 校验临时表限制并构建分区信息；
/// 5. 收集列级 CHECK 约束与外键；
/// 6. 确定主键的聚簇方式（PKIsHandle 表示单整数列主键直接作为行句柄；
///    IsCommonHandle 表示多列/非整数聚簇主键）；
/// 7. 构建全部索引（主键、唯一键、普通索引、全文索引、多值索引）；
/// 8. 做最终一致性校验（重名、外键列数、自增列唯一、AUTO_RANDOM 等）。
pub fn BuildTableInfoWithStmt<C: ?Sized + 'static, E: 'static>(
    context: &metabuild::Context<C, E>,
    statement: &ast::CreateTableStmt,
    db_charset: &str,
    db_collate: &str,
    placement_policy_ref: Option<&model::PolicyRefInfo>,
) -> BuildResult<model::TableInfo> {
    // CREATE TABLE LIKE / AS SELECT 需要额外的源表或计划器信息，此处不支持。
    if statement.ReferTable.is_some() {
        return Err(build_error(
            "CREATE TABLE LIKE requires source table metadata",
        ));
    }
    if statement.Select.is_some() {
        return Err(build_error(
            "CREATE TABLE AS SELECT requires planner metadata",
        ));
    }
    let (table_charset, table_collate) =
        resolve_charset_collation(context, &statement.Options, db_charset, db_collate)?;
    let mut seen_columns = HashSet::new();
    let mut columns = Vec::with_capacity(statement.Cols.len());
    for (offset, definition) in statement.Cols.iter().enumerate() {
        if !seen_columns.insert(definition.Name.Name.L.clone()) {
            return Err(build_error(format!(
                "duplicate column '{}'",
                definition.Name.Name.O
            )));
        }
        let mut column = build_column(definition, offset)?;
        // 字符类列继承表的字符集/排序规则，非字符类列固定为 binary。
        if type_needs_charset(column.GetType()) {
            let mut column_charset = column.GetCharset().to_lowercase();
            let mut column_collate = column.GetCollate().to_lowercase();
            if column_charset.is_empty() && column_collate.is_empty() {
                column_charset = table_charset.clone();
                column_collate = table_collate.clone();
            } else {
                if column_charset.is_empty() {
                    column_charset = charset::charset::GetCollationByName(&column_collate)
                        .map_err(|error| build_error(error.to_string()))?
                        .CharsetName;
                }
                if column_collate.is_empty() {
                    column_collate = if column_charset == "utf8mb4" {
                        context.GetDefaultCollationForUTF8MB4()
                    } else {
                        charset::charset::GetDefaultCollation(&column_charset)
                            .map_err(|error| build_error(error.to_string()))?
                    };
                }
                let collation = charset::charset::GetCollationByName(&column_collate)
                    .map_err(|error| build_error(error.to_string()))?;
                if collation.CharsetName != column_charset {
                    return Err(build_error(format!(
                        "collation '{}' is not valid for character set '{}'",
                        column_collate, column_charset
                    )));
                }
            }
            column.SetCharset(column_charset);
            column.SetCollate(column_collate);
        } else {
            column.SetCharset("binary".to_owned());
            column.SetCollate("binary".to_owned());
        }
        pad_binary_default_value(&mut column);
        normalize_temporal_default_value(&mut column);
        columns.push(column);
    }
    // 建立列名（小写）到列偏移的映射，供后续索引/外键按名查列。
    let mut offsets = columns
        .iter()
        .enumerate()
        .map(|(offset, column)| (column.Name.L.clone(), offset))
        .collect::<HashMap<_, _>>();

    // 为索引类约束物化表达式键的隐藏列（跳过主键/外键/CHECK）。
    let mut constraints = statement.Constraints.clone();
    check_constraint_names(&statement.Table.Name, &mut constraints)?;
    for constraint in &mut constraints {
        if matches!(
            constraint.Tp,
            ast::ConstraintType::PrimaryKey
                | ast::ConstraintType::ForeignKey
                | ast::ConstraintType::Check
                | ast::ConstraintType::None
        ) {
            continue;
        }
        materialize_expression_index_columns(constraint, &mut columns, &mut offsets)?;
    }

    // 组装表元数据骨架；TempTableType 区分普通表/全局临时表/本地临时表。
    let mut table = model::TableInfo {
        Name: statement.Table.Name.clone(),
        Charset: table_charset,
        Collate: table_collate,
        Columns: columns,
        State: model::StatePublic,
        Version: model::CurrLatestTableInfoVersion,
        MaxColumnID: offsets.len() as i64,
        PlacementPolicyRef: None,
        TempTableType: match statement.TemporaryKeyword {
            ast::TemporaryKeyword::None => model::TempTableNone,
            ast::TemporaryKeyword::Global => model::TempTableGlobal,
            ast::TemporaryKeyword::Local => model::TempTableLocal,
        },
        ..Default::default()
    };
    // Preserve the complete TTL table option on the canonical metadata object.
    // The parser already retains TTL/TTL_ENABLE/TTL_JOB_INTERVAL, and the Go
    // builder materializes that aggregate before the DDL job is published.
    let mut ttl_info = None;
    let mut ttl_enable = None;
    let mut ttl_job_interval = None;
    for option in &statement.Options {
        match option.Tp {
            ast::TableOptionType::TTL => {
                let column_name = option
                    .ColumnName
                    .as_ref()
                    .map(|column| column.Name.clone())
                    .unwrap_or_default();
                let interval_expression = option
                    .Value
                    .as_ref()
                    .map(expression_text)
                    .transpose()?
                    .unwrap_or_default();
                let interval_time_unit = option.TimeUnitValue.unwrap_or_default() as i32;
                ttl_info = Some(model::TTLInfo {
                    ColumnName: column_name,
                    IntervalExprStr: interval_expression,
                    IntervalTimeUnit: interval_time_unit,
                    Enable: true,
                    JobInterval: model::DefaultTTLJobInterval.to_owned(),
                });
            }
            ast::TableOptionType::TTLEnable => ttl_enable = Some(option.BoolValue),
            ast::TableOptionType::TTLJobInterval => {
                ttl_job_interval = Some(option.StrValue.clone())
            }
            _ => {}
        }
    }
    if let Some(info) = ttl_info.as_mut() {
        if let Some(enable) = ttl_enable {
            info.Enable = enable;
        }
        if let Some(job_interval) = ttl_job_interval {
            info.JobInterval = job_interval;
        }
    } else if ttl_enable.is_some() {
        return Err(build_error("TTL_ENABLE requires a TTL table definition"));
    } else if ttl_job_interval.is_some() {
        return Err(build_error(
            "TTL_JOB_INTERVAL requires a TTL table definition",
        ));
    }
    table.TTLInfo = ttl_info;
    if statement.TemporaryKeyword == ast::TemporaryKeyword::Global && !statement.OnCommitDelete {
        return Err(build_error(
            "GLOBAL TEMPORARY TABLE only supports ON COMMIT DELETE ROWS",
        ));
    }

    // 应用表级选项：字符集、注释、自增起始值、行 ID 分片位数、
    // 预切分 Region 数（PRE_SPLIT_REGIONS，建表时预先切分数据分片以分散写入）等。
    for option in &statement.Options {
        match option.Tp {
            ast::TableOptionType::Charset => table.Charset = option.StrValue.to_lowercase(),
            ast::TableOptionType::Collate => table.Collate = option.StrValue.to_lowercase(),
            ast::TableOptionType::Comment => table.Comment = option.StrValue.clone(),
            ast::TableOptionType::AutoIncrement => table.AutoIncID = option.UintValue as i64,
            ast::TableOptionType::AutoIdCache => {
                table.AutoIDCache = i64::try_from(option.UintValue)
                    .map_err(|_| build_error("table option auto_id_cache overflows int64"))?;
            }
            ast::TableOptionType::AutoRandomBase => table.AutoRandID = option.UintValue as i64,
            ast::TableOptionType::ShardRowID => {
                table.ShardRowIDBits = option.UintValue.min(vardef::MaxShardRowIDBits as u64);
                table.MaxShardRowIDBits = table.ShardRowIDBits;
            }
            ast::TableOptionType::PreSplitRegion => {
                if table.TempTableType != model::TempTableNone {
                    return Err(build_error(
                        "PRE_SPLIT_REGIONS is not valid on temporary tables",
                    ));
                }
                table.PreSplitRegions = option.UintValue;
            }
            ast::TableOptionType::Compression => table.Compression = option.StrValue.clone(),
            ast::TableOptionType::Policy => {
                table.PlacementPolicyRef = Some(model::PolicyRefInfo {
                    Name: ast::NewCIStr(&option.StrValue),
                    ..Default::default()
                });
            }
            ast::TableOptionType::Affinity => {
                table.Affinity = model::NewTableAffinityInfoWithLevel(&option.StrValue)
                    .map_err(|error| build_error(error.to_string()))?;
            }
            ast::TableOptionType::EngineAttribute => {
                return Err(build_error("ENGINE_ATTRIBUTE is not supported"));
            }
            _ => {}
        }
    }
    // 临时表不支持放置策略、亲和性与分区；普通表继承库级放置策略。
    if table.TempTableType != model::TempTableNone {
        if table.PlacementPolicyRef.is_some() {
            return Err(build_error(
                "placement policy is not valid on temporary tables",
            ));
        }
        if table.Affinity.is_some() {
            return Err(build_error("affinity is not valid on temporary tables"));
        }
        if statement.Partition.is_some() {
            return Err(build_error("partitioning is not valid on temporary tables"));
        }
    } else if table.PlacementPolicyRef.is_none() {
        table.PlacementPolicyRef = placement_policy_ref.cloned();
    }
    if let Some(partition) = statement.Partition.as_ref() {
        let partition = build_partition_info(partition)?;
        validate_extract_partition_expression(&table, &partition)?;
        table.Partition = Some(partition);
    }
    // 亲和性（affinity）级别必须与是否分区匹配：
    // 表级亲和性仅用于非分区表，分区级亲和性仅用于分区表。
    if let Some(affinity) = table.Affinity.as_ref() {
        match (affinity.Level.as_str(), table.Partition.is_some()) {
            ("table", true) => {
                return Err(build_error(
                    "table-level affinity is not valid on partitioned tables",
                ));
            }
            ("partition", false) => {
                return Err(build_error(
                    "partition-level affinity is not valid on non-partitioned tables",
                ));
            }
            ("table", false) | ("partition", true) => {}
            (level, _) => {
                return Err(build_error(format!("invalid affinity level '{level}'")));
            }
        }
    }

    // 收集列级 CHECK 约束与列级外键引用（REFERENCES 子句）。
    let mut next_constraint_id = 1_i64;
    for definition in &statement.Cols {
        for option in &definition.Options {
            match option.Tp {
                ast::ColumnOptionType::Check => {
                    let expression = option
                        .Expr
                        .as_ref()
                        .ok_or_else(|| build_error("column check has no expression"))?;
                    let name = if option.ConstraintName.is_empty() {
                        format!("{}_chk_{}", table.Name.L, next_constraint_id)
                    } else {
                        option.ConstraintName.clone()
                    };
                    table.Constraints.push(model::ConstraintInfo {
                        ID: next_constraint_id,
                        Name: ast::NewCIStr(&name),
                        Table: table.Name.clone(),
                        ConstraintCols: vec![definition.Name.Name.clone()],
                        Enforced: option.Enforced,
                        InColumn: true,
                        ExprString: expression_text(expression)?,
                        State: model::StatePublic,
                    });
                    next_constraint_id += 1;
                }
                ast::ColumnOptionType::Reference => {
                    let reference = option
                        .Refer
                        .as_ref()
                        .ok_or_else(|| build_error("column reference has no target"))?;
                    let constraint_name = if option.ConstraintName.is_empty() {
                        format!("{}_ibfk_{}", table.Name.L, table.ForeignKeys.len() + 1)
                    } else {
                        option.ConstraintName.clone()
                    };
                    table.ForeignKeys.push(model::FKInfo {
                        ID: table.ForeignKeys.len() as i64 + 1,
                        Name: ast::NewCIStr(&constraint_name),
                        RefSchema: reference.Table.Schema.clone(),
                        RefTable: reference.Table.Name.clone(),
                        RefCols: reference
                            .IndexPartSpecifications
                            .iter()
                            .filter_map(|key| key.Column.as_ref().map(|column| column.Name.clone()))
                            .collect(),
                        Cols: vec![definition.Name.Name.clone()],
                        OnDelete: refer_option(reference.OnDelete.ReferOpt),
                        OnUpdate: refer_option(reference.OnUpdate.ReferOpt),
                        State: model::StatePublic,
                        ..Default::default()
                    });
                }
                _ => {}
            }
        }
    }

    // 汇总主键列：列级 PRIMARY KEY 选项与表级 PRIMARY KEY 约束二选一，
    // 同时出现或出现多个表级主键约束都视为多重主键错误。
    let mut primary_keys = statement
        .Cols
        .iter()
        .filter(|column| {
            column
                .Options
                .iter()
                .any(|option| option.Tp == ast::ColumnOptionType::PrimaryKey)
        })
        .map(|column| ast::IndexPartSpecification {
            Column: Some(column.Name.clone()),
            Length: model::types::UnspecifiedLength,
            ..Default::default()
        })
        .collect::<Vec<_>>();

    let primary_constraints = constraints
        .iter()
        .filter(|constraint| constraint.Tp == ast::ConstraintType::PrimaryKey)
        .collect::<Vec<_>>();
    if primary_constraints.len() > 1
        || (!primary_keys.is_empty() && !primary_constraints.is_empty())
    {
        return Err(build_error("multiple primary keys defined"));
    }
    if let Some(constraint) = primary_constraints.first() {
        primary_keys = constraint.Keys.clone();
    }
    if context.PrimaryKeyRequired() && primary_keys.is_empty() {
        return Err(build_error("table must have a primary key"));
    }

    // 为主键列打上主键与非空标志，并确定聚簇方式：
    // PKIsHandle：单个整数主键列直接作为行句柄（handle，行的唯一定位键）；
    // IsCommonHandle：多列或非整数的聚簇主键。
    let primary_columns = index_columns(&primary_keys, &offsets)?;
    for key in &primary_columns {
        let column = &mut table.Columns[key.Offset as usize];
        column.AddFlag(model::mysql::PriKeyFlag | model::mysql::NotNullFlag);
        let has_default_value = column.DefaultValue.is_some() || column.DefaultIsExpr;
        set_no_default_value_flag(column, has_default_value);
    }
    let single_integer_primary_key = primary_columns.len() == 1
        && model::mysql::IsIntegerType(table.Columns[primary_columns[0].Offset as usize].GetType());
    let clustered = !primary_columns.is_empty()
        && should_cluster_primary_key(
            context,
            primary_key_type(statement),
            single_integer_primary_key,
        );
    table.PKIsHandle = clustered && single_integer_primary_key;
    table.IsCommonHandle = clustered && !single_integer_primary_key;

    // 除单整数聚簇主键（PKIsHandle）外，主键需要一条名为 PRIMARY 的索引记录。
    let mut next_index_id = 1_i64;
    if !primary_columns.is_empty() && !table.PKIsHandle {
        table.Indices.push(make_index(
            next_index_id,
            &table.Name,
            ast::NewCIStr("PRIMARY"),
            primary_columns,
            true,
            true,
            primary_constraints
                .first()
                .and_then(|constraint| constraint.Option.as_ref()),
        ));
        next_index_id += 1;
    }

    // 为每个带列级 UNIQUE 选项的列生成一条单列唯一索引。
    for (offset, definition) in statement.Cols.iter().enumerate() {
        if definition
            .Options
            .iter()
            .any(|option| option.Tp == ast::ColumnOptionType::UniqueKey)
        {
            let name = definition.Name.Name.clone();
            table.Indices.push(make_index(
                next_index_id,
                &table.Name,
                name.clone(),
                vec![model::IndexColumn {
                    Name: name,
                    Offset: offset as isize,
                    Length: model::types::UnspecifiedLength,
                    UseChangingType: false,
                }],
                false,
                true,
                None,
            ));
            next_index_id += 1;
        }
    }

    // 处理表级约束：索引/唯一索引在此建索引，CHECK 与外键记入相应元数据，
    // 全文索引附带解析器信息，向量与倒排列存索引附带各自的元数据。
    let mut seen_index_names = table
        .Indices
        .iter()
        .map(|index| index.Name.L.clone())
        .collect::<HashSet<_>>();
    for constraint in &constraints {
        if constraint.Tp == ast::ConstraintType::Vector {
            let key = constraint
                .Keys
                .first()
                .ok_or_else(|| build_error("VECTOR INDEX requires a distance expression"))?;
            let (column, function_name) = if let Some(mut expression) = key.Expr.as_ref() {
                while let ast::ExprKind::Parentheses(inner) = &expression.Kind {
                    expression = inner;
                }
                let ast::ExprKind::Function { FnName, Args, .. } = &expression.Kind else {
                    return Err(build_error(
                        "VECTOR INDEX requires a vector distance function",
                    ));
                };
                let column = Args
                    .first()
                    .and_then(|argument| match &argument.Kind {
                        ast::ExprKind::Column(column) => Some(column.Name.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| build_error("VECTOR INDEX requires a vector column"))?;
                (column, FnName.L.clone())
            } else {
                let hidden_name = key
                    .Column
                    .as_ref()
                    .map(|column| &column.Name)
                    .ok_or_else(|| build_error("VECTOR INDEX requires a distance expression"))?;
                let hidden_offset = offsets.get(&hidden_name.L).copied().ok_or_else(|| {
                    build_error("VECTOR INDEX hidden expression column does not exist")
                })?;
                let hidden = &table.Columns[hidden_offset];
                let dependency = hidden
                    .Dependences
                    .keys()
                    .next()
                    .cloned()
                    .ok_or_else(|| build_error("VECTOR INDEX requires a vector column"))?;
                let function_name = if hidden
                    .GeneratedExprString
                    .to_ascii_lowercase()
                    .contains("vec_cosine_distance")
                {
                    "vec_cosine_distance"
                } else if hidden
                    .GeneratedExprString
                    .to_ascii_lowercase()
                    .contains("vec_l2_distance")
                {
                    "vec_l2_distance"
                } else {
                    return Err(build_error("unsupported VECTOR INDEX distance function"));
                };
                (ast::NewCIStr(&dependency), function_name.to_owned())
            };
            let offset = offsets
                .get(&column.L)
                .copied()
                .ok_or_else(|| build_error(format!("key column '{}' does not exist", column.O)))?;
            let column_info = &table.Columns[offset];
            if column_info.GetType() != model::mysql::TypeTiDBVectorFloat32 {
                return Err(build_error(format!(
                    "Unsupported add vector index: only support vector type, but this is type: {}",
                    column_info.FieldType.String()
                )));
            }
            let metric = model::IndexableFnNameToDistanceMetric()
                .get(function_name.as_str())
                .cloned()
                .ok_or_else(|| build_error("unsupported VECTOR INDEX distance function"))?;
            let base_name = if constraint.Name.is_empty() {
                "vector_index".to_owned()
            } else {
                constraint.Name.clone()
            };
            let mut name = ast::NewCIStr(&base_name);
            let mut suffix = 2;
            while !seen_index_names.insert(name.L.clone()) {
                name = ast::NewCIStr(&format!("{base_name}_{suffix}"));
                suffix += 1;
            }
            let mut index = make_index(
                next_index_id,
                &table.Name,
                name,
                vec![model::IndexColumn {
                    Name: column.clone(),
                    Offset: offset as isize,
                    Length: model::types::UnspecifiedLength,
                    UseChangingType: false,
                }],
                false,
                false,
                constraint.Option.as_ref(),
            );
            index.VectorInfo = Some(model::VectorIndexInfo {
                Kind: model::VectorIndexKindHNSW.into(),
                Dimension: column_info.GetFlen().max(0) as u64,
                DistanceMetric: metric,
            });
            table.Indices.push(index);
            next_index_id += 1;
            continue;
        }
        let (unique, full_text) = match constraint.Tp {
            ast::ConstraintType::PrimaryKey => continue,
            ast::ConstraintType::Index => (false, false),
            ast::ConstraintType::Unique => (true, false),
            ast::ConstraintType::None => continue,
            ast::ConstraintType::Check => {
                let expression = constraint
                    .Expr
                    .as_ref()
                    .ok_or_else(|| build_error("check constraint has no expression"))?;
                let name = if constraint.Name.is_empty() {
                    format!("{}_chk_{}", table.Name.L, next_constraint_id)
                } else {
                    constraint.Name.clone()
                };
                table.Constraints.push(model::ConstraintInfo {
                    ID: next_constraint_id,
                    Name: ast::NewCIStr(&name),
                    Table: table.Name.clone(),
                    ConstraintCols: constraint
                        .Keys
                        .iter()
                        .filter_map(|key| key.Column.as_ref().map(|column| column.Name.clone()))
                        .collect(),
                    Enforced: constraint.Enforced,
                    InColumn: false,
                    ExprString: expression_text(expression)?,
                    State: model::StatePublic,
                });
                next_constraint_id += 1;
                continue;
            }
            ast::ConstraintType::ForeignKey => {
                let reference = constraint
                    .Refer
                    .as_ref()
                    .ok_or_else(|| build_error("foreign key has no target"))?;
                let id = table.ForeignKeys.len() as i64 + 1;
                let name = if constraint.Name.is_empty() {
                    format!("{}_ibfk_{id}", table.Name.L)
                } else {
                    constraint.Name.clone()
                };
                table.ForeignKeys.push(model::FKInfo {
                    ID: id,
                    Name: ast::NewCIStr(&name),
                    RefSchema: reference.Table.Schema.clone(),
                    RefTable: reference.Table.Name.clone(),
                    RefCols: reference
                        .IndexPartSpecifications
                        .iter()
                        .filter_map(|key| key.Column.as_ref().map(|column| column.Name.clone()))
                        .collect(),
                    Cols: constraint
                        .Keys
                        .iter()
                        .filter_map(|key| key.Column.as_ref().map(|column| column.Name.clone()))
                        .collect(),
                    OnDelete: refer_option(reference.OnDelete.ReferOpt),
                    OnUpdate: refer_option(reference.OnUpdate.ReferOpt),
                    State: model::StatePublic,
                    ..Default::default()
                });
                continue;
            }
            ast::ConstraintType::Fulltext => {
                if !deploymode::IsStarter() {
                    return Err(build_error(
                        "FULLTEXT index is only supported in starter deployment mode",
                    ));
                }
                if constraint.Keys.len() != 1
                    || constraint.Keys[0].Length != model::types::UnspecifiedLength
                    || constraint.Keys[0].Desc
                {
                    return Err(build_error(
                        "FULLTEXT index requires exactly one whole ascending column",
                    ));
                }
                (false, true)
            }
            ast::ConstraintType::Vector => unreachable!("VECTOR handled above"),
            ast::ConstraintType::Columnar => (false, false),
        };
        let columns = index_columns(&constraint.Keys, &offsets)?;
        let inverted = constraint.Tp == ast::ConstraintType::Columnar;
        if inverted
            && (constraint.Option.as_ref().map(|option| option.Tp)
                != Some(ast::IndexType::Inverted)
                || columns.len() != 1)
        {
            return Err(build_error(
                "INVERTED index requires exactly one columnar index column",
            ));
        }
        if !full_text {
            if let Some(first) = columns.first() {
                let column = &mut table.Columns[first.Offset as usize];
                if unique {
                    if columns.len() > 1 {
                        column.AddFlag(model::mysql::MultipleKeyFlag);
                    } else {
                        column.AddFlag(model::mysql::UniqueKeyFlag);
                    }
                } else {
                    column.AddFlag(model::mysql::MultipleKeyFlag);
                }
            }
        }
        // 全文索引要求其唯一的键列是字符串类型。
        if full_text
            && table.Columns[columns[0].Offset as usize]
                .FieldType
                .EvalType()
                != model::types::ETString
        {
            return Err(build_error("FULLTEXT index requires a string column"));
        }
        let name = if constraint.Name.is_empty() {
            columns
                .first()
                .map(|column| column.Name.clone())
                .ok_or_else(|| build_error("index has no columns"))?
        } else {
            ast::NewCIStr(&constraint.Name)
        };
        if !seen_index_names.insert(name.L.clone()) {
            return Err(build_error(format!("duplicate key name '{}'", name.O)));
        }
        let mut index = make_index(
            next_index_id,
            &table.Name,
            name,
            columns,
            false,
            unique,
            constraint.Option.as_ref(),
        );
        if let Some(condition) = constraint
            .Option
            .as_ref()
            .and_then(|option| option.Condition.as_ref())
        {
            index.ConditionExprString = BuildPartialIndexCondition(condition, &table)?;
        }
        // Mirror Go buildIndexColumns: any array key part marks the index as MV.
        // 任一键列为数组类型即标记为多值索引（MV index，一行可产生多条索引项）。
        index.MVIndex = index.Columns.iter().any(|index_column| {
            table.Columns[index_column.Offset as usize]
                .FieldType
                .IsArray()
        });
        if full_text {
            let parser_type = constraint
                .Option
                .as_ref()
                .filter(|option| !option.ParserName.L.is_empty())
                .map(|option| model::GetFullTextParserTypeBySQLName(&option.ParserName.L))
                .unwrap_or_else(|| model::FullTextParserTypeStandardV1.clone());
            if parser_type == model::FullTextParserTypeInvalid {
                return Err(build_error("invalid FULLTEXT parser"));
            }
            index.FullTextInfo = Some(model::FullTextIndexInfo {
                ParserType: parser_type,
            });
        }
        if inverted {
            let column = &table.Columns[index.Columns[0].Offset as usize];
            index.InvertedInfo = Some(
                model::FieldTypeToInvertedIndexInfo(&column.FieldType, column.ID).ok_or_else(
                    || build_error("INVERTED index does not support this column type"),
                )?,
            );
        }
        table.Indices.push(index);
        next_index_id += 1;
    }

    // 记录各类对象的最大 ID，并校验约束名不与索引名冲突。
    table.MaxIndexID = next_index_id - 1;
    table.MaxForeignKeyID = table.ForeignKeys.len() as i64;
    table.MaxConstraintID = next_constraint_id - 1;
    let mut constraint_names = table
        .Indices
        .iter()
        .map(|index| index.Name.L.clone())
        .collect::<HashSet<_>>();
    for constraint in &table.Constraints {
        if !constraint_names.insert(constraint.Name.L.clone()) {
            return Err(build_error(format!(
                "duplicate constraint name '{}'",
                constraint.Name.O
            )));
        }
    }
    // 校验外键：名字不重复、本表列与被引用列数量一致且本表列存在。
    let mut foreign_key_names = HashSet::new();
    for foreign_key in &table.ForeignKeys {
        if !foreign_key_names.insert(foreign_key.Name.L.clone()) {
            return Err(build_error(format!(
                "duplicate foreign key name '{}'",
                foreign_key.Name.O
            )));
        }
        if foreign_key.Cols.is_empty() || foreign_key.Cols.len() != foreign_key.RefCols.len() {
            return Err(build_error(format!(
                "foreign key '{}' has incompatible column counts",
                foreign_key.Name.O
            )));
        }
        for column in &foreign_key.Cols {
            if !offsets.contains_key(&column.L) {
                return Err(build_error(format!(
                    "foreign key column '{}' does not exist",
                    column.O
                )));
            }
        }
    }
    // 自增列最多一个；TiDB 与 MySQL 兼容整数、FLOAT 和 DOUBLE 自增列。
    let auto_increment_columns = table
        .Columns
        .iter()
        .filter(|column| column.GetFlag() & model::mysql::AutoIncrementFlag != 0)
        .collect::<Vec<_>>();
    if auto_increment_columns.len() > 1 {
        return Err(build_error(
            "incorrect table definition: multiple auto columns",
        ));
    }
    if auto_increment_columns.first().is_some_and(|column| {
        !model::mysql::IsIntegerType(column.GetType())
            && !matches!(
                column.GetType(),
                model::mysql::TypeFloat | model::mysql::TypeDouble
            )
    }) {
        return Err(build_error("incorrect column specifier for auto-increment"));
    }
    set_table_auto_random_bits(&mut table, &statement.Cols)?;
    // 行 ID 分片仅对使用隐藏行 ID 的表有意义：聚簇索引表清零相关配置，
    // 非聚簇普通表在未显式设置时继承会话级默认值。
    if table.HasClusteredIndex() {
        if table.ShardRowIDBits > 0 {
            return Err(build_error(
                "[ddl:8200]Unsupported shard_row_id_bits for table with primary key as row id",
            ));
        }
        table.ShardRowIDBits = 0;
        table.MaxShardRowIDBits = 0;
        if !table.ContainsAutoRandomBits() {
            table.PreSplitRegions = 0;
        }
    } else if table.ShardRowIDBits == 0 && table.TempTableType == model::TempTableNone {
        table.ShardRowIDBits = context.GetShardRowIDBits();
        table.MaxShardRowIDBits = table.ShardRowIDBits;
        table.PreSplitRegions = context.GetPreSplitRegions();
    }
    let sharding_bits = if table.ContainsAutoRandomBits() {
        table.AutoRandomBits
    } else {
        table.ShardRowIDBits
    };
    if table.PreSplitRegions > sharding_bits {
        table.PreSplitRegions = sharding_bits;
    }
    Ok(table)
}

/// Builds checked table metadata using TiDB's default utf8mb4 charset/collation.
///
/// 使用默认的 utf8mb4 字符集构建并校验表元数据，是带校验的便捷入口。
pub fn BuildTableInfoFromAST<C: ?Sized + 'static, E: 'static>(
    context: &metabuild::Context<C, E>,
    statement: &ast::CreateTableStmt,
) -> BuildResult<model::TableInfo> {
    build_table_info_with_check(context, statement, "utf8mb4", "", None)
}

/// Builds the metadata of one `ALTER TABLE ... ADD COLUMN` definition.
///
/// Go routes both CREATE TABLE and ADD COLUMN through `columnDefToCol` and
/// then inherits the table charset/collation, so share the same builder here.
/// The caller owns the column ID because Go allocates it from
/// `TableInfo.MaxColumnID`.
///
/// 由 `ALTER TABLE ... ADD COLUMN` 的列定义构建列元数据。
/// Go 侧建表与加列共用 columnDefToCol 并继承表字符集/排序规则，
/// 此处复用同一构建逻辑；列 ID 由调用方从 TableInfo.MaxColumnID 分配。
pub fn BuildColumnInfoFromAST(
    definition: &ast::ColumnDef,
    offset: usize,
    table_charset: &str,
    table_collate: &str,
) -> BuildResult<model::ColumnInfo> {
    let mut column = build_column(definition, offset)?;
    if type_needs_charset(column.GetType()) {
        if column.GetCharset().is_empty() {
            column.SetCharset(table_charset.to_owned());
        }
        if column.GetCollate().is_empty() {
            column.SetCollate(table_collate.to_owned());
        }
    } else {
        column.SetCharset("binary".to_owned());
        column.SetCollate("binary".to_owned());
    }
    Ok(column)
}

/// 构建表元数据并执行两轮校验（语句相关校验 + 通用限制校验）。
fn build_table_info_with_check<C: ?Sized + 'static, E: 'static>(
    context: &metabuild::Context<C, E>,
    statement: &ast::CreateTableStmt,
    db_charset: &str,
    db_collate: &str,
    placement_policy_ref: Option<&model::PolicyRefInfo>,
) -> BuildResult<model::TableInfo> {
    let table = BuildTableInfoWithStmt(
        context,
        statement,
        db_charset,
        db_collate,
        placement_policy_ref,
    )?;
    check_table_info_valid_with_stmt(context, &table, statement)?;
    check_table_info_valid_extra(&table)?;
    Ok(table)
}

/// 结合原始语句做校验：主键必需性、生成列只能引用位置在其之前的生成列、
/// 分区名不重复。普通列允许后向引用，与 Go `verifyColumnGeneration` 一致。
fn check_table_info_valid_with_stmt<C: ?Sized + 'static, E: 'static>(
    context: &metabuild::Context<C, E>,
    table: &model::TableInfo,
    statement: &ast::CreateTableStmt,
) -> BuildResult<()> {
    if context.PrimaryKeyRequired() && table.GetPkName().L.is_empty() {
        return Err(build_error("table must have a primary key"));
    }
    let column_positions = statement
        .Cols
        .iter()
        .enumerate()
        .map(|(position, column)| (column.Name.Name.L.as_str(), position))
        .collect::<HashMap<_, _>>();
    let generated_columns = statement
        .Cols
        .iter()
        .filter(|column| {
            column
                .Options
                .iter()
                .any(|option| option.Tp == ast::ColumnOptionType::Generated)
        })
        .map(|column| column.Name.Name.L.as_str())
        .collect::<HashSet<_>>();
    for column in &statement.Cols {
        for option in &column.Options {
            if option.Tp != ast::ColumnOptionType::Generated {
                continue;
            }
            let expression = option
                .Expr
                .as_ref()
                .ok_or_else(|| build_error("generated column has no expression"))?;
            validate_generated_column_qualifiers(
                expression,
                &statement.Table.Schema,
                &statement.Table.Name,
            )?;
            let mut dependencies = HashSet::new();
            collect_expression_columns(expression, &mut dependencies);
            let current_position = column_positions[&column.Name.Name.L.as_str()];
            for dependency in dependencies {
                let Some(dependency_position) = column_positions.get(dependency.as_str()) else {
                    return Err(build_error(format!(
                        "generated column '{}' refers to unknown column '{dependency}'",
                        column.Name.Name.O
                    )));
                };
                if generated_columns.contains(dependency.as_str())
                    && *dependency_position >= current_position
                {
                    return Err(build_error(format!(
                        "generated column '{}' refers to a later generated column",
                        column.Name.Name.O
                    )));
                }
            }
        }
    }
    if let Some(partition) = &table.Partition {
        let mut names = HashSet::new();
        for definition in &partition.Definitions {
            if !names.insert(definition.Name.L.as_str()) {
                return Err(build_error(format!(
                    "duplicate partition name '{}'",
                    definition.Name.O
                )));
            }
        }
    }
    Ok(())
}

/// 通用限制校验：表名/列名不超过 64 字符、列数不超过 1017、索引数不超过 64。
fn check_table_info_valid_extra(table: &model::TableInfo) -> BuildResult<()> {
    if table.Name.O.chars().count() > 64 {
        return Err(build_error("table name is too long"));
    }
    if table.Columns.len() > 1017 {
        return Err(build_error("too many columns"));
    }
    if table.Indices.len() > 64 {
        return Err(build_error("too many indexes"));
    }
    for column in &table.Columns {
        if column.Name.O.chars().count() > 64 {
            return Err(build_error(format!(
                "column name '{}' is too long",
                column.Name.O
            )));
        }
    }
    Ok(())
}

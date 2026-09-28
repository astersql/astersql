// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// SQL 解析器驱动的值表达式与参数占位符实现，对齐 Go `parser/types`。
//
// 提供 `ValueExpr`/`ParamMarkerExpr`、字面量 Restore/Format，以及向解析器
// 注册 Decimal/Hex/Bit 构造钩子；Visitor 保留 Go 的 Enter/Leave 替换语义。

use crate::format;
use std::any::Any;
use std::io::{self, Write};
use std::ops::{Deref, DerefMut};
use types_decimal::mydecimal::{DecimalError, MyDecimal};
use types_scalar::{BitLiteral, HexLiteral, NewBitLiteral, NewHexLiteral};

/// Rust has no Go package initialization hook.  This value exposes the same
/// parser-driver registrations to the package integration layer explicitly.
/// 暴露给集成层的解析器驱动注册钩子（对应 Go 包 init 注册）。
pub struct DriverHooks {
    pub new_value_expr: fn(Box<dyn Any>, &str, &str) -> Box<ValueExpr>,
    pub new_param_marker_expr: fn(i32) -> Box<ParamMarkerExpr>,
    pub new_decimal: fn(&str) -> Result<MyDecimal, String>,
    pub new_hex_literal: fn(&str) -> Result<HexLiteral, String>,
    pub new_bit_literal: fn(&str) -> Result<BitLiteral, String>,
}

/// 从字符串构造 MyDecimal；Truncated 视为可接受。
fn new_decimal(value: &str) -> Result<MyDecimal, String> {
    let mut decimal = MyDecimal::default();
    match decimal.FromString(value.as_bytes()) {
        Ok(()) | Err(DecimalError::Truncated) => Ok(decimal),
        Err(error) => Err(error.to_string()),
    }
}

/// 解析 HEX 字面量字符串。
fn new_hex_literal(value: &str) -> Result<HexLiteral, String> {
    NewHexLiteral(value.to_owned()).map_err(|error| error.to_string())
}

/// 解析 BIT 字面量字符串。
fn new_bit_literal(value: &str) -> Result<BitLiteral, String> {
    NewBitLiteral(value.to_owned()).map_err(|error| error.to_string())
}

/// 构造驱动钩子表，对应 Go 包初始化时的注册。
pub fn init() -> DriverHooks {
    DriverHooks {
        new_value_expr: newValueExpr,
        new_param_marker_expr: newParamMarkerExpr,
        new_decimal,
        new_hex_literal,
        new_bit_literal,
    }
}

/// The expression-node portion embedded by Go's `ast.TexprNode`.
#[derive(Clone, Default)]
/// 嵌入 Go `ast.TexprNode` 的表达式节点公共字段（含字段类型）。
pub struct TexprNode {
    pub Type: types::FieldType,
}

/// ValueExpr is the simple value expression.
#[derive(Clone, Default)]
/// 简单值表达式：持有 Datum 与投影偏移。
pub struct ValueExpr {
    pub TexprNode: TexprNode,
    pub Datum: types::Datum,
    pub projectionOffset: i32,
}

impl Deref for ValueExpr {
    type Target = TexprNode;

    fn deref(&self) -> &Self::Target {
        &self.TexprNode
    }
}

impl DerefMut for ValueExpr {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.TexprNode
    }
}

impl ValueExpr {
    /// 写入 Datum，使用默认校对规则。
    /// SetValue implements the value-expression interface.
    pub fn SetValue(&mut self, value: &dyn Any) {
        self.Datum.SetValueWithDefaultCollation(value);
    }

    /// 按 Datum 种类还原为 SQL 字面量文本。
    /// Restore implements the Go SQL-literal restoration branches.
    pub fn Restore(&self, ctx: &mut format::RestoreCtx<'_>) -> io::Result<()> {
        macro_rules! ignore_write_error {
            ($write:expr) => {{
                let _ = $write;
                Ok(())
            }};
        }

        match self.Datum.Kind() {
            types::KindNull => ignore_write_error!(ctx.WriteKeyWord("NULL")),
            types::KindInt64 => {
                if self.Type.GetFlag() & types::mysql::IsBooleanFlag != 0 {
                    ignore_write_error!(ctx.WriteKeyWord(if self.Datum.GetInt64() > 0 {
                        "TRUE"
                    } else {
                        "FALSE"
                    }))
                } else {
                    ignore_write_error!(ctx.WritePlain(&self.Datum.GetInt64().to_string()))
                }
            }
            types::KindUint64 => {
                ignore_write_error!(ctx.WritePlain(&self.Datum.GetUint64().to_string()))
            }
            types::KindFloat32 => {
                ignore_write_error!(ctx.WritePlain(&format_float(self.Datum.GetFloat64(), 32)))
            }
            types::KindFloat64 => {
                ignore_write_error!(ctx.WritePlain(&format_float(self.Datum.GetFloat64(), 64)))
            }
            types::KindString => {
                let charset = self.Type.GetCharset();
                if !charset.is_empty()
                    && !ctx.Flags.HasStringWithoutCharset()
                    && (!ctx.Flags.HasStringWithoutDefaultCharset()
                        || charset != types::mysql::DefaultCharset)
                {
                    let _ = ctx.WritePlain("_");
                    let _ = ctx.WriteKeyWord(charset);
                }
                // 无论 SQL Mode，先双写反斜杠，再交给 RestoreCtx 做引号转义。
                // Go doubles backslashes here regardless of SQL mode before
                // RestoreCtx applies its configured string quoting.
                ignore_write_error!(
                    ctx.WriteString(&self.Datum.GetString().replace('\u{5c}', "\\\\"))
                )
            }
            types::KindBytes => ignore_write_error!(ctx.WriteString(&self.Datum.GetString())),
            types::KindMysqlDecimal => {
                ignore_write_error!(ctx.WritePlain(&self.Datum.GetMysqlDecimal().String()))
            }
            types::KindBinaryLiteral => {
                if self.Type.GetFlag() & types::mysql::UnsignedFlag != 0 {
                    ignore_write_error!(
                        ctx.WritePlain(&format!("x'{}'", hex::encode(self.Datum.GetBytes())))
                    )
                } else {
                    ignore_write_error!(
                        ctx.WritePlain(&self.Datum.GetBinaryLiteral().ToBitLiteralString(true))
                    )
                }
            }
            types::KindMysqlDuration => {
                ignore_write_error!(ctx.WritePlain(&format!("'{}'", self.Datum.GetMysqlDuration())))
            }
            types::KindMysqlTime => {
                ignore_write_error!(ctx.WritePlain(&format!("'{}'", self.Datum.GetMysqlTime())))
            }
            types::KindMysqlEnum
            | types::KindMysqlBit
            | types::KindMysqlSet
            | types::KindInterface
            | types::KindMinNotNull
            | types::KindMaxValue
            | types::KindRaw
            | types::KindMysqlJSON
            | types::KindVectorFloat32 => Err(io::Error::other("Not implemented")),
            _ => Err(io::Error::other("can't format to string")),
        }
    }

    pub fn RestoreToString(&self, flags: format::RestoreFlags) -> io::Result<String> {
        let mut output = Vec::new();
        {
            let mut ctx = format::NewRestoreCtx(flags, &mut output);
            self.Restore(&mut ctx)?;
        }
        Ok(String::from_utf8(output).expect("restored SQL is UTF-8"))
    }

    /// 取 Datum 的字符串表示。
    /// GetDatumString implements the value-expression interface.
    pub fn GetDatumString(&self) -> String {
        self.Datum.GetString()
    }

    /// 紧凑字面量输出；对齐 Go 无错误返回契约。
    /// Format writes the compact literal representation.  The Go method has no
    /// error return, so writer failures are intentionally not surfaced.
    pub fn Format(&self, writer: &mut dyn Write) {
        let text = match self.Datum.Kind() {
            types::KindNull => "NULL".to_owned(),
            types::KindInt64 if self.Type.GetFlag() & types::mysql::IsBooleanFlag != 0 => {
                if self.Datum.GetInt64() > 0 {
                    "TRUE".to_owned()
                } else {
                    "FALSE".to_owned()
                }
            }
            types::KindInt64 => self.Datum.GetInt64().to_string(),
            types::KindUint64 => self.Datum.GetUint64().to_string(),
            types::KindFloat32 => format_float(self.Datum.GetFloat64(), 32),
            types::KindFloat64 => format_float(self.Datum.GetFloat64(), 64),
            types::KindString | types::KindBytes => WrapInSingleQuotes(&self.Datum.GetString()),
            types::KindMysqlDecimal => self.Datum.GetMysqlDecimal().String(),
            types::KindBinaryLiteral => {
                if self.Type.GetFlag() & types::mysql::UnsignedFlag != 0 {
                    format!("x'{}'", hex::encode(self.Datum.GetBytes()))
                } else {
                    self.Datum.GetBinaryLiteral().ToBitLiteralString(true)
                }
            }
            _ => panic!("Can't format to string"),
        };
        let _ = writer.write_all(text.as_bytes());
    }

    /// 设置投影列偏移。
    pub fn SetProjectionOffset(&mut self, offset: i32) {
        self.projectionOffset = offset;
    }

    /// 读取投影列偏移。
    pub fn GetProjectionOffset(&self) -> i32 {
        self.projectionOffset
    }

    /// 访问者模式：保留 Enter 替换、跳过子节点与 Leave 顺序。
    /// Accept preserves Enter replacement, skip-children, type assertion, and
    /// Leave ordering from the Go visitor protocol.
    pub fn Accept(self: Box<Self>, visitor: &mut dyn Visitor) -> (Box<dyn Any>, bool) {
        let (node, skip_children) = visitor.Enter(self);
        if skip_children {
            return visitor.Leave(node);
        }
        let node = match node.downcast::<ValueExpr>() {
            Ok(node) => node,
            Err(_) => panic!("visitor Enter returned a non-ValueExpr node"),
        };
        visitor.Leave(node)
    }
}

/// Visitor is the object-safe Rust form of `ast.Visitor` used by this driver.
/// 本驱动使用的 object-safe Visitor，对应 `ast.Visitor`。
pub trait Visitor {
    fn Enter(&mut self, node: Box<dyn Any>) -> (Box<dyn Any>, bool);
    fn Leave(&mut self, node: Box<dyn Any>) -> (Box<dyn Any>, bool);
}

/// WrapInSingleQuotes escapes single quotes and backslashes and adds quotes.
/// 转义反斜杠与单引号后加单引号包裹。
pub fn WrapInSingleQuotes(input: &str) -> String {
    let escaped = input.replace('\u{5c}', "\\\\").replace('\'', "''");
    format!("'{escaped}'")
}

/// UnwrapFromSingleQuotes reverses WrapInSingleQuotes and also accepts plain
/// strings, matching the Go helper's sequential replacement order.
/// 逆转 WrapInSingleQuotes；非引号串原样返回。
pub fn UnwrapFromSingleQuotes(input: &str) -> String {
    if input.len() < 2 || !input.starts_with('\'') || !input.ends_with('\'') {
        return input.to_owned();
    }
    input[1..input.len() - 1]
        .replace("\\\\", "\\")
        .replace("''", "'")
}

/// newValueExpr creates a ValueExpr and applies the default field type before
/// setting Datum, preserving the Go collation invariant and call order.
/// 创建 ValueExpr：先推断默认字段类型再写入 Datum，保持 Go 调用顺序。
pub fn newValueExpr(value: Box<dyn Any>, charset: &str, collate: &str) -> Box<ValueExpr> {
    let value = match value.downcast::<ValueExpr>() {
        Ok(expression) => return expression,
        Err(value) => value,
    };

    let mut expression = Box::new(ValueExpr::default());
    let inferred_value = if value.is::<()>() {
        None
    } else {
        Some(value.as_ref())
    };
    types_field::DefaultTypeForValue(inferred_value, &mut expression.Type, charset, collate);
    expression
        .Datum
        .SetValue(value.as_ref(), &expression.TexprNode.Type);
    expression.projectionOffset = -1;
    expression
}

/// ParamMarkerExpr holds a place for another expression while parsing a
/// prepared statement.
#[derive(Clone, Default)]
/// 预处理语句中的 `?` 参数占位符表达式。
pub struct ParamMarkerExpr {
    pub ValueExpr: ValueExpr,
    pub Offset: i32,
    pub Order: i32,
    pub InExecute: bool,
    pub UseAsValueInGbyByClause: bool,
}

impl Deref for ParamMarkerExpr {
    type Target = ValueExpr;

    fn deref(&self) -> &Self::Target {
        &self.ValueExpr
    }
}

impl DerefMut for ParamMarkerExpr {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ValueExpr
    }
}

impl ParamMarkerExpr {
    pub fn Restore(&self, ctx: &mut format::RestoreCtx<'_>) -> io::Result<()> {
        // Go RestoreCtx.WritePlain has no error return; keep the same contract.
        let _ = ctx.WritePlain("?");
        Ok(())
    }

    pub fn RestoreToString(&self, flags: format::RestoreFlags) -> io::Result<String> {
        let mut output = Vec::new();
        {
            let mut ctx = format::NewRestoreCtx(flags, &mut output);
            self.Restore(&mut ctx)?;
        }
        Ok(String::from_utf8(output).expect("restored SQL is UTF-8"))
    }

    pub fn Format(&self, _writer: &mut dyn Write) {
        panic!("Not implemented")
    }

    pub fn Accept(self: Box<Self>, visitor: &mut dyn Visitor) -> (Box<dyn Any>, bool) {
        let (node, skip_children) = visitor.Enter(self);
        if skip_children {
            return visitor.Leave(node);
        }
        let node = match node.downcast::<ParamMarkerExpr>() {
            Ok(node) => node,
            Err(_) => panic!("visitor Enter returned a non-ParamMarkerExpr node"),
        };
        visitor.Leave(node)
    }

    /// 设置参数在预处理语句中的顺序号。
    pub fn SetOrder(&mut self, order: i32) {
        self.Order = order;
    }
}

/// 对接 AST 层 ValueExpr trait。
impl parser_ast::expressions::ValueExpr for ParamMarkerExpr {
    fn set_value(&mut self, value: Box<dyn Any>) {
        self.SetValue(value.as_ref());
    }

    fn get_value(&self) -> &dyn Any {
        &self.Datum
    }

    fn projection_offset(&self) -> i32 {
        self.projectionOffset
    }

    fn set_projection_offset(&mut self, offset: i32) {
        self.projectionOffset = offset;
    }
}

/// 对接 AST 层 ParamMarkerExpr trait。
impl parser_ast::expressions::ParamMarkerExpr for ParamMarkerExpr {
    fn set_order(&mut self, order: i32) {
        self.SetOrder(order);
    }

    fn order(&self) -> i32 {
        self.Order
    }

    fn offset(&self) -> i32 {
        self.Offset
    }

    fn in_execute(&self) -> bool {
        self.InExecute
    }

    fn clone_box(&self) -> Box<dyn parser_ast::expressions::ParamMarkerExpr> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn accept(&mut self, visitor: &mut dyn parser_ast::expressions::Visitor) -> bool {
        if visitor.enter_param_marker(self) {
            return visitor.leave_param_marker(self);
        }
        visitor.leave_param_marker(self)
    }
}

/// 按源码偏移构造参数占位符。
pub fn newParamMarkerExpr(offset: i32) -> Box<ParamMarkerExpr> {
    Box::new(ParamMarkerExpr {
        Offset: offset,
        ..ParamMarkerExpr::default()
    })
}

/// 按 Go strconv 风格输出科学计数法（含 NaN/+Inf/-Inf）。
fn format_float(value: f64, bits: u32) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "+Inf".to_owned()
        } else {
            "-Inf".to_owned()
        };
    }
    let rendered = if bits == 32 {
        format!("{:e}", value as f32)
    } else {
        format!("{value:e}")
    };
    let (mantissa, exponent) = rendered
        .split_once('e')
        .expect("scientific format has an exponent");
    let exponent: i32 = exponent.parse().expect("scientific exponent is numeric");
    format!("{mantissa}e{exponent:+03}")
}

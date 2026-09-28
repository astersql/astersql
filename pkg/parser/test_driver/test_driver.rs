// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 解析器集成测试驱动：ValueExpr / ParamMarkerExpr 与最小 Visitor。
//
// 对照 Go `test_driver.go`：为 AST 集成提供字面值表达式节点，支持按 Datum kind
// 还原 SQL 文本、Format 输出，以及投影偏移与 Accept 访问者模式钩子。
// 在完整 AST Node trait 接线完成前，用 `Any` 保持与 Go 接口相近的形状。

use std::any::Any;
use std::io::{self, Write};

use crate::*;

// Reuse the repository's generated Go `strconv.IsPrint` repertoire so Format
// preserves `strconv.Quote` for both ASCII controls and non-printable Unicode.
include!("../../util/plancodec/go_quote_printable.rs");

/// Rust has no Go-style package `init`.  Keeping an explicit entry point lets
/// the parser integration task preserve the original registration boundary.
/// 显式初始化入口；Rust 无包级 init，保留与 Go 注册边界对应的空函数。
pub fn init_test_driver() {}

/// Object-safe visitor surface preserving Go's replacement node and skip flag.
/// 测试驱动访问者接口：Enter/Leave 返回替换节点与是否跳过子节点。
pub trait Visitor {
    fn Enter(&mut self, node: Box<dyn Any>) -> (Box<dyn Any>, bool);
    fn Leave(&mut self, node: Box<dyn Any>) -> (Box<dyn Any>, bool);
}

/// 字面值表达式节点：内嵌 Datum、字段类型与投影偏移。
#[derive(Default)]
pub struct ValueExpr {
    pub datum: Datum,
    pub Type: types::FieldType,
    projection_offset: i32,
}

impl ValueExpr {
    /// 由动态值构造 ValueExpr，并按 DefaultTypeForValue 填充 FieldType。
    pub fn new(value: Box<dyn Any>, charset_name: &str, collate: &str) -> Self {
        // Go newValueExpr 收到 *ValueExpr 时直接复用原节点。
        if value.is::<Self>() {
            return *value
                .downcast::<Self>()
                .expect("ValueExpr type was checked above");
        }
        let mut expression = Self::default();
        DefaultTypeForValue(&*value, &mut expression.Type, charset_name, collate);
        expression.datum.SetValue(value);
        expression.projection_offset = -1;
        expression
    }

    /// 按 Datum kind 将字面值写回 SQL 文本（Restore 上下文）。
    pub fn Restore(&self, ctx: &mut format::RestoreCtx<'_>) -> io::Result<()> {
        match self.datum.Kind() {
            KindNull => ctx.WriteKeyWord("NULL"),
            KindInt64 => {
                // 布尔标志位时输出 TRUE/FALSE，而不是整数。
                if self.Type.GetFlag() & mysql::IsBooleanFlag != 0 {
                    ctx.WriteKeyWord(if self.datum.GetInt64() > 0 {
                        "TRUE"
                    } else {
                        "FALSE"
                    })
                } else {
                    ctx.WritePlain(&self.datum.GetInt64().to_string())
                }
            }
            KindUint64 => ctx.WritePlain(&self.datum.GetUint64().to_string()),
            KindFloat32 => ctx.WritePlain(&format_float(self.datum.GetFloat64(), 32)),
            KindFloat64 => ctx.WritePlain(&format_float(self.datum.GetFloat64(), 64)),
            KindString => {
                let charset_name = self.Type.GetCharset();
                // 非默认字符集时写 _CHARSET 前缀，除非 RestoreFlags 要求省略。
                if !charset_name.is_empty()
                    && !ctx.Flags.HasStringWithoutCharset()
                    && (!ctx.Flags.HasStringWithoutDefaultCharset()
                        || charset_name != mysql::DefaultCharset)
                {
                    ctx.WritePlain("_")?;
                    ctx.WriteKeyWord(charset_name)?;
                }
                ctx.WriteString(&self.datum.GetString())
            }
            KindBytes => ctx.WriteString(&self.datum.GetString()),
            KindMysqlDecimal => ctx.WritePlain(&self.datum.GetMysqlDecimal().String()),
            KindBinaryLiteral => {
                let charset_name = self.Type.GetCharset();
                // 二进制字面量：Unsigned 走十六进制 x'...'，否则走 b'...' 位串。
                if !charset_name.is_empty()
                    && charset_name != mysql::DefaultCharset
                    && !ctx.Flags.HasStringWithoutCharset()
                    && charset_name != charset::CharsetBin
                {
                    ctx.WritePlain("_")?;
                    ctx.WriteKeyWord(&format!("{charset_name} "))?;
                }
                if self.Type.GetFlag() & mysql::UnsignedFlag != 0 {
                    ctx.WritePlain(&format!("x'{}'", hex::encode(self.datum.GetBytes())))
                } else {
                    ctx.WritePlain(&self.datum.GetBinaryLiteral().ToBitLiteralString(true))
                }
            }
            KindMysqlDuration | KindMysqlEnum | KindMysqlBit | KindMysqlSet | KindMysqlTime
            | KindInterface | KindMinNotNull | KindMaxValue | KindRaw | KindMysqlJSON => {
                Err(io::Error::other("not implemented"))
            }
            _ => Err(io::Error::other("can't format to string")),
        }
    }

    /// 使用给定 RestoreFlags 还原为 UTF-8 字符串。
    pub fn RestoreToString(&self, flags: format::RestoreFlags) -> io::Result<String> {
        let mut output = Vec::new();
        {
            let mut ctx = format::NewRestoreCtx(flags, &mut output);
            self.Restore(&mut ctx)?;
        }
        Ok(String::from_utf8(output).expect("restore output is valid UTF-8"))
    }

    /// 返回 Datum 的字符串视图。
    pub fn GetDatumString(&self) -> String {
        self.datum.GetString()
    }

    /// 将字面值格式化为调试/展示文本并写入 writer（未实现 kind 会 panic）。
    pub fn Format(&self, writer: &mut dyn Write) {
        let text = match self.datum.Kind() {
            KindNull => "NULL".to_owned(),
            KindInt64 if self.Type.GetFlag() & mysql::IsBooleanFlag != 0 => {
                if self.datum.GetInt64() > 0 {
                    "TRUE".to_owned()
                } else {
                    "FALSE".to_owned()
                }
            }
            KindInt64 => self.datum.GetInt64().to_string(),
            KindUint64 => self.datum.GetUint64().to_string(),
            KindFloat32 => format_float(self.datum.GetFloat64(), 32),
            KindFloat64 => format_float(self.datum.GetFloat64(), 64),
            KindString | KindBytes => quote_go_string(&self.datum.GetString()),
            KindMysqlDecimal => self.datum.GetMysqlDecimal().String(),
            KindBinaryLiteral if self.Type.GetFlag() & mysql::UnsignedFlag != 0 => {
                format!("x'{}'", hex::encode(self.datum.GetBytes()))
            }
            KindBinaryLiteral => self.datum.GetBinaryLiteral().ToBitLiteralString(true),
            _ => panic!("Can't format to string"),
        };
        let _ = writer.write_all(text.as_bytes());
    }

    /// 设置投影偏移；-1 表示尚未参与投影映射。
    pub fn SetProjectionOffset(&mut self, offset: i32) {
        self.projection_offset = offset;
    }
    /// 读取投影偏移。
    pub fn GetProjectionOffset(&self) -> i32 {
        self.projection_offset
    }

    /// 接受 Visitor：Enter 若跳过子节点则直接 Leave。
    pub fn Accept(self: Box<Self>, visitor: &mut dyn Visitor) -> (Box<dyn Any>, bool) {
        let (node, skip_children) = visitor.Enter(self);
        if skip_children {
            return visitor.Leave(node);
        }
        let node = node
            .downcast::<ValueExpr>()
            .unwrap_or_else(|_| panic!("visitor Enter returned a non-ValueExpr node"));
        visitor.Leave(node)
    }
}

/// Go 风格构造函数别名，委托给 `ValueExpr::new`。
pub fn newValueExpr(value: Box<dyn Any>, charset_name: &str, collate: &str) -> ValueExpr {
    ValueExpr::new(value, charset_name, collate)
}

/// 预处理参数占位符表达式（对应 SQL 中的 `?`）。
#[derive(Default)]
pub struct ParamMarkerExpr {
    pub value_expr: ValueExpr,
    pub Offset: i32,
    pub Order: i32,
    pub InExecute: bool,
}

impl ParamMarkerExpr {
    /// 以源码偏移构造参数占位符。
    pub fn new(offset: i32) -> Self {
        Self {
            Offset: offset,
            ..Self::default()
        }
    }

    /// 还原为字面量 `?`。
    pub fn Restore(&self, ctx: &mut format::RestoreCtx<'_>) -> io::Result<()> {
        ctx.WritePlain("?")
    }

    /// 使用 RestoreFlags 还原为字符串 `"?"`。
    pub fn RestoreToString(&self, flags: format::RestoreFlags) -> io::Result<String> {
        let mut output = Vec::new();
        {
            let mut ctx = format::NewRestoreCtx(flags, &mut output);
            self.Restore(&mut ctx)?;
        }
        Ok(String::from_utf8(output).expect("restore output is valid UTF-8"))
    }

    /// Format 在测试驱动中尚未实现，调用即 panic。
    pub fn Format(&self, _writer: &mut dyn Write) {
        panic!("Not implemented")
    }
    /// 接受 Visitor，语义同 ValueExpr::Accept。
    pub fn Accept(self: Box<Self>, visitor: &mut dyn Visitor) -> (Box<dyn Any>, bool) {
        let (node, skip_children) = visitor.Enter(self);
        if skip_children {
            return visitor.Leave(node);
        }
        let node = node
            .downcast::<ParamMarkerExpr>()
            .unwrap_or_else(|_| panic!("visitor Enter returned a non-ParamMarkerExpr node"));
        visitor.Leave(node)
    }
    /// 设置参数在预处理语句中的顺序号。
    pub fn SetOrder(&mut self, order: i32) {
        self.Order = order;
    }
}

/// Go 风格构造函数别名，委托给 `ParamMarkerExpr::new`。
pub fn newParamMarkerExpr(offset: i32) -> ParamMarkerExpr {
    ParamMarkerExpr::new(offset)
}

/// 按 32/64 位浮点科学计数法格式化，指数至少三位并带符号（对齐 Go 测试输出）。
fn format_float(value: f64, bits: u32) -> String {
    let value = if bits == 32 {
        (value as f32) as f64
    } else {
        value
    };
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == f64::INFINITY {
        return "+Inf".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-Inf".to_owned();
    }
    let rendered = format!("{value:e}");
    let (mantissa, exponent) = rendered
        .split_once('e')
        .expect("scientific format has exponent");
    let exponent: i32 = exponent.parse().expect("scientific exponent is numeric");
    format!("{mantissa}e{exponent:+03}")
}

fn quote_go_string(value: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in value.chars() {
        match ch {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\x07' => quoted.push_str("\\a"),
            '\x08' => quoted.push_str("\\b"),
            '\x0c' => quoted.push_str("\\f"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            '\x0b' => quoted.push_str("\\v"),
            _ => {
                let code = ch as u32;
                let index = GO_PRINTABLE_RANGES.partition_point(|&(start, _)| start <= code);
                let printable = index > 0 && code <= GO_PRINTABLE_RANGES[index - 1].1;
                if printable {
                    quoted.push(ch);
                } else if code < 0x20 || ch == '\x7f' {
                    quoted.push_str(&format!("\\x{code:02x}"));
                } else if code < 0x10000 {
                    quoted.push_str(&format!("\\u{code:04x}"));
                } else {
                    quoted.push_str(&format!("\\U{code:08x}"));
                }
            }
        }
    }
    quoted.push('"');
    quoted
}

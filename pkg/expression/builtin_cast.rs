// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 类型转换（CAST）内置函数实现，对应 Go `builtin_cast.go`。
//
// Go 版按源/目标求值类型矩阵分派众多签名；本模块在统一的类型化求值器中保留相同语义：
// 构建表达式时固定目标元数据，求值时执行转换、警告与 NULL 处理。
// CAST 将值从一种 SQL 类型转为另一种（如字符串转 DECIMAL、整数转时间）。

// Executable Rust counterpart of `builtin_cast.go`.
//
// TiDB's Go implementation dispatches one signature for every source/target
// evaluation-type pair. Rust keeps the same matrix in one typed evaluator:
// metadata is fixed while building an expression, and conversion, warning and
// NULL semantics are applied while evaluating it. This makes the module usable
// without inventing stand-ins for TiDB's session, chunk or protobuf packages.

use chrono::{Datelike, Duration as ChronoDuration, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde_json::{Number as JsonNumber, Value as JsonValue};
use std::fmt;
use std::str::FromStr;
use thiserror::Error;

/// 未指定长度/精度占位。
pub const UNSPECIFIED_LENGTH: i32 = -1;
/// DECIMAL 最大精度。
pub const MAX_DECIMAL_WIDTH: i32 = 65;
/// 时间类型最大小数秒精度（fractional seconds precision）。
pub const MAX_FSP: u32 = 6;
/// DATE 显示宽度。
pub const MAX_DATE_WIDTH: i32 = 10;
/// 无小数秒时 DATETIME 显示宽度。
pub const MAX_DATETIME_WIDTH_NO_FSP: i32 = 19;
/// 无小数秒时 TIME/DURATION 显示宽度。
pub const MAX_DURATION_WIDTH_NO_FSP: i32 = 10;
/// LONGBLOB 最大宽度。
pub const MAX_LONG_BLOB_WIDTH: i32 = 4_294_967_295_u32 as i32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 抽象字段种类（求值用），如 Int/Real/Decimal/时间/JSON/向量/数组。
pub enum FieldKind {
    Int,
    Real,
    Decimal,
    String,
    Date,
    DateTime,
    Timestamp,
    Duration,
    Json,
    VectorFloat32,
    Array,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// MySQL 线协议/元数据层类型码的本地枚举。
pub enum MysqlType {
    Tiny,
    Short,
    Int24,
    Long,
    LongLong,
    Year,
    Bit,
    Float,
    Double,
    String,
    VarString,
    TinyBlob,
    Blob,
    MediumBlob,
    LongBlob,
    Date,
    DateTime,
    Timestamp,
    Duration,
    Json,
    VectorFloat32,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 完整字段类型：种类、MySQL 类型、flen/decimal、标志、字符集与数组元素类型。
pub struct FieldType {
    pub kind: FieldKind,
    pub mysql_type: MysqlType,
    pub flen: i32,
    pub decimal: i32,
    pub unsigned: bool,
    pub binary: bool,
    pub not_null: bool,
    pub parse_to_json: bool,
    pub hybrid: bool,
    pub charset: String,
    pub collation: String,
    pub element_type: Option<Box<FieldType>>,
}

impl FieldType {
    /// 按 `FieldKind` 构造带默认 MySQL 类型映射的 `FieldType`。
    pub fn new(kind: FieldKind) -> Self {
        let mysql_type = match kind {
            FieldKind::Int => MysqlType::LongLong,
            FieldKind::Real => MysqlType::Double,
            FieldKind::Decimal => MysqlType::Other,
            FieldKind::String => MysqlType::VarString,
            FieldKind::Date => MysqlType::Date,
            FieldKind::DateTime => MysqlType::DateTime,
            FieldKind::Timestamp => MysqlType::Timestamp,
            FieldKind::Duration => MysqlType::Duration,
            FieldKind::Json => MysqlType::Json,
            FieldKind::VectorFloat32 => MysqlType::VectorFloat32,
            FieldKind::Array => MysqlType::Other,
        };
        Self {
            kind,
            mysql_type,
            flen: UNSPECIFIED_LENGTH,
            decimal: UNSPECIFIED_LENGTH,
            unsigned: false,
            binary: false,
            not_null: false,
            parse_to_json: false,
            hybrid: false,
            charset: "utf8mb4".to_owned(),
            collation: "utf8mb4_bin".to_owned(),
            element_type: None,
        }
    }

    pub fn array(element_type: FieldType) -> Self {
        let mut result = Self::new(FieldKind::Array);
        result.element_type = Some(Box::new(element_type));
        result
    }

    pub fn with_mysql_type(mut self, mysql_type: MysqlType) -> Self {
        self.mysql_type = mysql_type;
        self
    }
    pub fn with_flen(mut self, flen: i32) -> Self {
        self.flen = flen;
        self
    }
    pub fn with_decimal(mut self, decimal: i32) -> Self {
        self.decimal = decimal;
        self
    }
    pub fn with_unsigned(mut self, unsigned: bool) -> Self {
        self.unsigned = unsigned;
        self
    }
    pub fn with_binary(mut self, binary: bool) -> Self {
        self.binary = binary;
        if binary {
            self.charset = "binary".to_owned();
            self.collation = "binary".to_owned();
            self.mysql_type = MysqlType::String;
        }
        self
    }
    pub fn with_not_null(mut self, not_null: bool) -> Self {
        self.not_null = not_null;
        self
    }
    pub fn with_parse_to_json(mut self, parse: bool) -> Self {
        self.parse_to_json = parse;
        self
    }
    pub fn with_hybrid(mut self, hybrid: bool) -> Self {
        self.hybrid = hybrid;
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
/// DATE/DATETIME/TIMESTAMP 的本地时间值表示。
pub enum MysqlTime {
    Date(NaiveDate),
    DateTime(NaiveDateTime, u32),
    Timestamp(NaiveDateTime, u32),
}

impl MysqlTime {
    fn date(&self) -> NaiveDate {
        match self {
            Self::Date(value) => *value,
            Self::DateTime(value, _) | Self::Timestamp(value, _) => value.date(),
        }
    }

    fn time(&self) -> NaiveTime {
        match self {
            Self::Date(_) => NaiveTime::MIN,
            Self::DateTime(value, _) | Self::Timestamp(value, _) => value.time(),
        }
    }

    fn fsp(&self) -> u32 {
        match self {
            Self::Date(_) => 0,
            Self::DateTime(_, fsp) | Self::Timestamp(_, fsp) => *fsp,
        }
    }

    fn numeric_string(&self) -> String {
        let date = self.date();
        let time = self.time();
        let mut value = format!(
            "{:04}{:02}{:02}{:02}{:02}{:02}",
            date.year(),
            date.month(),
            date.day(),
            time.hour(),
            time.minute(),
            time.second()
        );
        if self.fsp() > 0 {
            value.push('.');
            let micros = format!("{:06}", time.nanosecond() / 1_000);
            value.push_str(&micros[..self.fsp() as usize]);
        }
        value
    }
}

impl fmt::Display for MysqlTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Date(value) => write!(formatter, "{}", value.format("%Y-%m-%d")),
            Self::DateTime(value, fsp) | Self::Timestamp(value, fsp) => {
                write!(formatter, "{}", value.format("%Y-%m-%d %H:%M:%S"))?;
                if *fsp > 0 {
                    let micros = format!("{:06}", value.nanosecond() / 1_000);
                    write!(formatter, ".{}", &micros[..*fsp as usize])?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// TIME 时长类型（可超过 24 小时，带符号与 FSP）。
pub struct MysqlDuration {
    pub negative: bool,
    pub hours: u32,
    pub minutes: u32,
    pub seconds: u32,
    pub micros: u32,
    pub fsp: u32,
}

impl MysqlDuration {
    /// 按 `FieldKind` 构造带默认 MySQL 类型映射的 `FieldType`。
    pub fn new(
        negative: bool,
        hours: u32,
        minutes: u32,
        seconds: u32,
        micros: u32,
        fsp: u32,
    ) -> Result<Self, CastError> {
        if hours > 838 || minutes > 59 || seconds > 59 || micros > 999_999 || fsp > MAX_FSP {
            return Err(CastError::InvalidDuration(format!(
                "{}{:02}:{:02}:{:02}.{:06}",
                if negative { "-" } else { "" },
                hours,
                minutes,
                seconds,
                micros
            )));
        }
        Ok(Self {
            negative,
            hours,
            minutes,
            seconds,
            micros,
            fsp,
        })
    }

    fn signed_micros(&self) -> i64 {
        let absolute = (((self.hours as i64 * 60 + self.minutes as i64) * 60
            + self.seconds as i64)
            * 1_000_000)
            + self.micros as i64;
        if self.negative { -absolute } else { absolute }
    }

    fn numeric_string(&self) -> String {
        let mut result = format!(
            "{}{}{:02}{:02}",
            if self.negative { "-" } else { "" },
            self.hours,
            self.minutes,
            self.seconds
        );
        if self.fsp > 0 {
            let micros = format!("{:06}", self.micros);
            result.push('.');
            result.push_str(&micros[..self.fsp as usize]);
        }
        result
    }
}

impl fmt::Display for MysqlDuration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}{:02}:{:02}:{:02}",
            if self.negative { "-" } else { "" },
            self.hours,
            self.minutes,
            self.seconds
        )?;
        if self.fsp > 0 {
            let micros = format!("{:06}", self.micros);
            write!(formatter, ".{}", &micros[..self.fsp as usize])?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq)]
/// CAST 求值过程中的运行时值联合体。
pub enum Value {
    Null,
    Int(i64),
    UInt(u64),
    Real(f64),
    Decimal(Decimal),
    String(String),
    Bytes(Vec<u8>),
    Time(MysqlTime),
    Duration(MysqlDuration),
    Json(JsonValue),
    VectorFloat32(Vec<f32>),
    Array(Vec<Value>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 可下推 CAST 的控制流函数种类（IF/CASE/IFNULL 等）。
pub enum ControlKind {
    If,
    Case,
    Elt,
}

#[derive(Clone, Debug, PartialEq)]
/// 表达式内部节点：常量、CAST、控制流。
enum ExprNode {
    Constant(Value),
    Cast {
        source: Box<Expression>,
        in_union: bool,
    },
    Control {
        kind: ControlKind,
        args: Vec<Expression>,
    },
}

#[derive(Clone, Debug, PartialEq)]
/// 带结果类型的可求值表达式树。
pub struct Expression {
    pub field_type: FieldType,
    pub target: FieldType,
    node: ExprNode,
}

impl Expression {
    pub fn constant(value: Value, field_type: FieldType) -> Self {
        Self {
            target: field_type.clone(),
            field_type,
            node: ExprNode::Constant(value),
        }
    }

    pub fn control(kind: ControlKind, args: Vec<Self>, field_type: FieldType) -> Self {
        Self {
            target: field_type.clone(),
            field_type,
            node: ExprNode::Control { kind, args },
        }
    }

    pub fn eval(&self, ctx: &mut CastContext) -> Result<Value, CastError> {
        match &self.node {
            ExprNode::Constant(value) => Ok(value.clone()),
            ExprNode::Cast { source, in_union } => {
                let value = source.eval(ctx)?;
                cast_value(value, &source.field_type, &self.target, *in_union, ctx)
            }
            ExprNode::Control { kind, args } => eval_control(*kind, args, ctx),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 转换过程警告（截断、越界、无效值等）。
pub enum CastWarning {
    Truncated,
    Overflow,
    CastNegativeAsUnsigned,
    CastAsSignedOverflow,
    AllowedPacketOverflow,
}

#[derive(Clone, Debug)]
/// CAST 求值上下文：收集警告、UNION 场景标志等。
pub struct CastContext {
    pub warnings: Vec<CastWarning>,
    pub current_date: NaiveDate,
    pub max_allowed_packet: u64,
}

impl Default for CastContext {
    fn default() -> Self {
        Self::new(NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch"))
    }
}

impl CastContext {
    /// 按 `FieldKind` 构造带默认 MySQL 类型映射的 `FieldType`。
    pub fn new(current_date: NaiveDate) -> Self {
        Self {
            warnings: Vec::new(),
            current_date,
            max_allowed_packet: 64 * 1024 * 1024,
        }
    }

    fn warn(&mut self, warning: CastWarning) {
        self.warnings.push(warning);
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
/// 致命转换错误。
pub enum CastError {
    #[error("cannot cast from {from:?} to {to:?}")]
    Unsupported { from: FieldKind, to: FieldKind },
    #[error("invalid integer: {0}")]
    InvalidInteger(String),
    #[error("invalid decimal: {0}")]
    InvalidDecimal(String),
    #[error("invalid time: {0}")]
    InvalidTime(String),
    #[error("invalid duration: {0}")]
    InvalidDuration(String),
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    #[error("invalid vector: {0}")]
    InvalidVector(String),
    #[error("CAST AS ARRAY requires a JSON array source and an element type")]
    InvalidArray,
}

/// 求值 IF/CASE 等控制流节点。
fn eval_control(
    kind: ControlKind,
    args: &[Expression],
    ctx: &mut CastContext,
) -> Result<Value, CastError> {
    match kind {
        ControlKind::If if args.len() == 3 => {
            if truthy(&args[0].eval(ctx)?) {
                args[1].eval(ctx)
            } else {
                args[2].eval(ctx)
            }
        }
        ControlKind::Case => {
            let pair_end = args.len() - (args.len() % 2);
            for pair in args[..pair_end].chunks_exact(2) {
                if truthy(&pair[0].eval(ctx)?) {
                    return pair[1].eval(ctx);
                }
            }
            if args.len() % 2 == 1 {
                args.last().expect("odd case has else").eval(ctx)
            } else {
                Ok(Value::Null)
            }
        }
        ControlKind::Elt if !args.is_empty() => {
            let index = value_to_i64(args[0].eval(ctx)?, false, false, ctx)?;
            if index <= 0 || index as usize >= args.len() {
                Ok(Value::Null)
            } else {
                args[index as usize].eval(ctx)
            }
        }
        _ => Ok(Value::Null),
    }
}

/// 判断值在 SQL 布尔上下文中是否为真。
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Int(value) => *value != 0,
        Value::UInt(value) => *value != 0,
        Value::Real(value) => *value != 0.0,
        Value::Decimal(value) => !value.is_zero(),
        Value::String(value) => value.parse::<f64>().is_ok_and(|value| value != 0.0),
        Value::Bytes(value) => !value.is_empty(),
        _ => true,
    }
}

/// 构建普通 CAST 表达式。
// 构建普通 CAST 节点：求值时再按目标类型做实际转换。
pub fn BuildCastFunction(expr: Expression, target: FieldType) -> Expression {
    BuildCastFunctionWithCheck(expr, target, false, false)
        .expect("BuildCastFunction receives a supported target type")
}

/// 为 UNION 类型对齐构建 CAST（允许更宽松的截断语义）。
pub fn BuildCastFunction4Union(expr: Expression, target: FieldType) -> Expression {
    BuildCastFunctionWithCheck(expr, target, true, false)
        .expect("BuildCastFunction4Union receives a supported target type")
}

/// 构建仅改变字符集/排序规则的 CAST。
pub fn BuildCastCollationFunction(
    expr: Expression,
    charset: impl Into<String>,
    collation: impl Into<String>,
    enum_or_set_real_type_is_string: bool,
) -> Expression {
    let mut target = expr.field_type.clone();
    target.charset = charset.into();
    target.collation = collation.into();
    if enum_or_set_real_type_is_string && target.hybrid {
        target.kind = FieldKind::String;
    }
    BuildCastFunction(expr, target)
}

/// 构建 CAST，并在需要时做额外合法性检查。
pub fn BuildCastFunctionWithCheck(
    expr: Expression,
    mut target: FieldType,
    in_union: bool,
    _is_explicit_charset: bool,
) -> Result<Expression, CastError> {
    if !expr.field_type.not_null {
        target.not_null = false;
    }
    if target.kind == FieldKind::Array
        && (expr.field_type.kind != FieldKind::Json || target.element_type.is_none())
    {
        return Err(CastError::InvalidArray);
    }
    let expr = TryPushCastIntoControlFunctionForHybridType(expr, &target);
    Ok(Expression {
        field_type: target.clone(),
        target,
        node: ExprNode::Cast {
            source: Box::new(expr),
            in_union,
        },
    })
}

/// 尝试把 CAST 下推进控制流函数参数，以优化混合类型分支。
// 混合类型（ENUM/SET 等）在 IF/CASE 分支中预先下推 CAST，避免分支结果类型抖动。
pub fn TryPushCastIntoControlFunctionForHybridType(
    mut expr: Expression,
    target: &FieldType,
) -> Expression {
    if !matches!(target.kind, FieldKind::Int | FieldKind::Real) {
        return expr;
    }
    let ExprNode::Control { kind, args } = &mut expr.node else {
        return expr;
    };
    let wrap = |arg: Expression| {
        if arg.field_type.hybrid && arg.field_type.mysql_type != MysqlType::Bit {
            BuildCastFunction(arg, target.clone())
        } else {
            arg
        }
    };
    match kind {
        ControlKind::If if args.len() == 3 => {
            args[1] = wrap(args[1].clone());
            args[2] = wrap(args[2].clone());
        }
        ControlKind::Case => {
            let pair_end = args.len() - (args.len() % 2);
            for index in (1..pair_end).step_by(2) {
                args[index] = wrap(args[index].clone());
            }
            if args.len() % 2 == 1 {
                let last = args.len() - 1;
                args[last] = wrap(args[last].clone());
            }
        }
        ControlKind::Elt => {
            for arg in args.iter_mut().skip(1) {
                *arg = wrap(arg.clone());
            }
        }
        _ => {}
    }
    expr.field_type = target.clone();
    expr.target = target.clone();
    expr
}

/// 按目标 `FieldType` 将 `Value` 转换为目标类型。
// CAST 核心分派：按目标 FieldKind 转入对应转换函数；NULL 直接透传。
fn cast_value(
    value: Value,
    source: &FieldType,
    target: &FieldType,
    in_union: bool,
    ctx: &mut CastContext,
) -> Result<Value, CastError> {
    if value == Value::Null {
        return Ok(Value::Null);
    }
    match target.kind {
        FieldKind::Int => to_int(value, source, target, in_union, ctx),
        FieldKind::Real => to_real(value, source, target, in_union, ctx),
        FieldKind::Decimal => to_decimal(value, source, target, in_union, ctx),
        FieldKind::String => to_string(value, source, target, ctx),
        FieldKind::Date | FieldKind::DateTime | FieldKind::Timestamp => to_time(value, target, ctx),
        FieldKind::Duration => to_duration(value, target, ctx),
        FieldKind::Json => to_json(value, source, target),
        FieldKind::VectorFloat32 => to_vector(value),
        FieldKind::Array => to_array(value, target, ctx),
    }
}

/// 转换到整数类型。
fn to_int(
    value: Value,
    _source: &FieldType,
    target: &FieldType,
    in_union: bool,
    ctx: &mut CastContext,
) -> Result<Value, CastError> {
    if target.unsigned {
        let result = value_to_u64(value, in_union, ctx)?;
        Ok(Value::UInt(result))
    } else {
        let result = value_to_i64(value, false, in_union, ctx)?;
        Ok(Value::Int(result))
    }
}

/// 提取有符号 i64。
fn value_to_i64(
    value: Value,
    unsigned: bool,
    in_union: bool,
    ctx: &mut CastContext,
) -> Result<i64, CastError> {
    if unsigned {
        return Ok(value_to_u64(value, in_union, ctx)? as i64);
    }
    match value {
        Value::Int(value) => Ok(value),
        Value::UInt(value) => {
            if value > i64::MAX as u64 {
                ctx.warn(CastWarning::CastAsSignedOverflow);
            }
            Ok(value as i64)
        }
        Value::Real(value) => float_to_i64(value, ctx),
        Value::Decimal(value) => decimal_to_i64(value, ctx),
        Value::String(value) => parse_signed_integer(&value, ctx),
        Value::Bytes(value) => parse_signed_integer(&String::from_utf8_lossy(&value), ctx),
        Value::Time(value) => parse_signed_integer(&value.numeric_string(), ctx),
        Value::Duration(value) => parse_signed_integer(&value.numeric_string(), ctx),
        Value::Json(value) => json_to_i64(value, ctx),
        other => Err(CastError::Unsupported {
            from: kind_of(&other),
            to: FieldKind::Int,
        }),
    }
}

/// 提取无符号 u64；UNION 场景有特殊截断规则。
// 转无符号 64 位；UNION 对齐时对越界更宽松（截断并记警告而非失败）。
fn value_to_u64(value: Value, in_union: bool, ctx: &mut CastContext) -> Result<u64, CastError> {
    match value {
        Value::UInt(value) => Ok(value),
        Value::Int(value) if value < 0 => {
            if in_union {
                Ok(0)
            } else {
                ctx.warn(CastWarning::CastNegativeAsUnsigned);
                Ok(value as u64)
            }
        }
        Value::Int(value) => Ok(value as u64),
        Value::Real(value) if value < 0.0 => {
            if in_union {
                Ok(0)
            } else {
                ctx.warn(CastWarning::CastNegativeAsUnsigned);
                Ok((value.round() as i64) as u64)
            }
        }
        Value::Real(value) if value.is_finite() && value <= u64::MAX as f64 => {
            Ok(value.round() as u64)
        }
        Value::Real(_) => {
            ctx.warn(CastWarning::Overflow);
            Ok(u64::MAX)
        }
        Value::Decimal(value) if value.is_sign_negative() => {
            if in_union {
                Ok(0)
            } else {
                ctx.warn(CastWarning::CastNegativeAsUnsigned);
                Ok(value.round().to_i64().unwrap_or(i64::MIN) as u64)
            }
        }
        Value::Decimal(value) => match value.round().to_u64() {
            Some(value) => Ok(value),
            None => {
                ctx.warn(CastWarning::Overflow);
                Ok(u64::MAX)
            }
        },
        Value::String(value) => parse_unsigned_integer(&value, in_union, ctx),
        Value::Bytes(value) => {
            parse_unsigned_integer(&String::from_utf8_lossy(&value), in_union, ctx)
        }
        Value::Time(value) => parse_unsigned_integer(&value.numeric_string(), in_union, ctx),
        Value::Duration(value) => parse_unsigned_integer(&value.numeric_string(), in_union, ctx),
        Value::Json(value) => json_to_u64(value, in_union, ctx),
        other => Err(CastError::Unsupported {
            from: kind_of(&other),
            to: FieldKind::Int,
        }),
    }
}

/// 浮点转有符号整数，越界记警告或错误。
fn float_to_i64(value: f64, ctx: &mut CastContext) -> Result<i64, CastError> {
    if !value.is_finite() {
        ctx.warn(CastWarning::Overflow);
        return Ok(if value.is_sign_negative() {
            i64::MIN
        } else {
            i64::MAX
        });
    }
    let rounded = value.round();
    if rounded > i64::MAX as f64 {
        ctx.warn(CastWarning::Overflow);
        Ok(i64::MAX)
    } else if rounded < i64::MIN as f64 {
        ctx.warn(CastWarning::Overflow);
        Ok(i64::MIN)
    } else {
        Ok(rounded as i64)
    }
}

/// DECIMAL 转有符号整数。
fn decimal_to_i64(value: Decimal, ctx: &mut CastContext) -> Result<i64, CastError> {
    match value.round().to_i64() {
        Some(value) => Ok(value),
        None => {
            ctx.warn(CastWarning::Overflow);
            Ok(if value.is_sign_negative() {
                i64::MIN
            } else {
                i64::MAX
            })
        }
    }
}

/// 从字符串取出可解析的整数前缀。
fn integer_prefix(value: &str) -> (&str, bool) {
    let trimmed = value.trim_start();
    let bytes = trimmed.as_bytes();
    let mut end = usize::from(
        bytes
            .first()
            .is_some_and(|byte| *byte == b'+' || *byte == b'-'),
    );
    let digit_start = end;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
    }
    (&trimmed[..end], end != trimmed.len() || end == digit_start)
}

/// 解析有符号整数字符串。
fn parse_signed_integer(value: &str, ctx: &mut CastContext) -> Result<i64, CastError> {
    let (prefix, truncated) = integer_prefix(value);
    if prefix.is_empty() || prefix == "+" || prefix == "-" {
        ctx.warn(CastWarning::Truncated);
        return Ok(0);
    }
    if truncated {
        ctx.warn(CastWarning::Truncated);
    }
    let parsed = prefix.parse::<i128>().unwrap_or_else(|_| {
        if prefix.starts_with('-') {
            i128::MIN
        } else {
            i128::MAX
        }
    });
    if parsed > i64::MAX as i128 {
        ctx.warn(CastWarning::Truncated);
        Ok(if parsed <= u64::MAX as i128 {
            parsed as u64 as i64
        } else {
            -1
        })
    } else if parsed < i64::MIN as i128 {
        ctx.warn(CastWarning::Truncated);
        Ok(i64::MIN)
    } else {
        Ok(parsed as i64)
    }
}

/// 解析无符号整数字符串。
fn parse_unsigned_integer(
    value: &str,
    in_union: bool,
    ctx: &mut CastContext,
) -> Result<u64, CastError> {
    let (prefix, truncated) = integer_prefix(value);
    if prefix.is_empty() || prefix == "+" || prefix == "-" {
        ctx.warn(CastWarning::Truncated);
        return Ok(0);
    }
    if truncated {
        ctx.warn(CastWarning::Truncated);
    }
    let parsed = prefix.parse::<i128>().unwrap_or_else(|_| {
        if prefix.starts_with('-') {
            i128::MIN
        } else {
            i128::MAX
        }
    });
    if parsed < 0 {
        if in_union {
            return Ok(0);
        }
        ctx.warn(CastWarning::CastNegativeAsUnsigned);
        return Ok((parsed as i64) as u64);
    }
    if parsed > u64::MAX as i128 {
        ctx.warn(CastWarning::Truncated);
        Ok(u64::MAX)
    } else {
        Ok(parsed as u64)
    }
}

/// 转换到浮点。
fn to_real(
    value: Value,
    source: &FieldType,
    target: &FieldType,
    in_union: bool,
    ctx: &mut CastContext,
) -> Result<Value, CastError> {
    let mut result = match value {
        Value::Int(value) if source.unsigned => value as u64 as f64,
        Value::Int(value) => value as f64,
        Value::UInt(value) => value as f64,
        Value::Real(value) => value,
        Value::Decimal(value) => value
            .to_f64()
            .ok_or_else(|| CastError::InvalidDecimal(value.to_string()))?,
        Value::String(value) => parse_float(&value, ctx),
        Value::Bytes(value) => parse_float(&String::from_utf8_lossy(&value), ctx),
        Value::Time(value) => parse_float(&value.numeric_string(), ctx),
        Value::Duration(value) => parse_float(&value.numeric_string(), ctx),
        Value::Json(value) => json_to_f64(value, ctx)?,
        other => {
            return Err(CastError::Unsupported {
                from: kind_of(&other),
                to: FieldKind::Real,
            });
        }
    };
    if in_union && target.unsigned && result < 0.0 {
        result = 0.0;
    }
    Ok(Value::Real(result))
}

/// 解析浮点字符串。
fn parse_float(value: &str, ctx: &mut CastContext) -> f64 {
    let trimmed = value.trim_start();
    let mut end = 0;
    let mut seen_digit = false;
    let mut seen_dot = false;
    let mut seen_exp = false;
    for (index, byte) in trimmed.bytes().enumerate() {
        let valid = if byte.is_ascii_digit() {
            seen_digit = true;
            true
        } else if (byte == b'+' || byte == b'-')
            && (index == 0
                || trimmed
                    .as_bytes()
                    .get(index - 1)
                    .is_some_and(|b| *b == b'e' || *b == b'E'))
        {
            true
        } else if byte == b'.' && !seen_dot && !seen_exp {
            seen_dot = true;
            true
        } else if (byte == b'e' || byte == b'E') && seen_digit && !seen_exp {
            seen_exp = true;
            seen_digit = false;
            true
        } else {
            false
        };
        if !valid {
            break;
        }
        end = index + 1;
    }
    if end != trimmed.len() || !seen_digit {
        ctx.warn(CastWarning::Truncated);
    }
    trimmed[..end].parse::<f64>().unwrap_or_else(|_| {
        ctx.warn(CastWarning::Overflow);
        if trimmed.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        }
    })
}

/// 转换到 DECIMAL。
fn to_decimal(
    value: Value,
    source: &FieldType,
    target: &FieldType,
    in_union: bool,
    ctx: &mut CastContext,
) -> Result<Value, CastError> {
    let mut result = match value {
        Value::Int(value) if source.unsigned => Decimal::from(value as u64),
        Value::Int(value) => Decimal::from(value),
        Value::UInt(value) => Decimal::from(value),
        Value::Real(value) => Decimal::from_f64_retain(value)
            .ok_or_else(|| CastError::InvalidDecimal(value.to_string()))?,
        Value::Decimal(value) => value,
        Value::String(value) => parse_decimal(&value, ctx),
        Value::Bytes(value) => parse_decimal(&String::from_utf8_lossy(&value), ctx),
        Value::Time(value) => parse_decimal(&value.numeric_string(), ctx),
        Value::Duration(value) => parse_decimal(&value.numeric_string(), ctx),
        Value::Json(value) => json_to_decimal(value, ctx)?,
        other => {
            return Err(CastError::Unsupported {
                from: kind_of(&other),
                to: FieldKind::Decimal,
            });
        }
    };
    if in_union && target.unsigned && result.is_sign_negative() {
        result = Decimal::ZERO;
    }
    result = produce_decimal(result, target, ctx);
    Ok(Value::Decimal(result))
}

/// 解析 DECIMAL 字符串。
fn parse_decimal(value: &str, ctx: &mut CastContext) -> Decimal {
    let trimmed = value.trim();
    if let Ok(value) = Decimal::from_str(trimmed) {
        return value;
    }
    let numeric = parse_float(trimmed, ctx);
    Decimal::from_f64_retain(numeric).unwrap_or(Decimal::ZERO)
}

/// 按目标 flen/scale 调整 DECIMAL（舍入/截断）。
// 按目标精度/小数位舍入或截断 DECIMAL，超宽时记警告。
fn produce_decimal(mut value: Decimal, target: &FieldType, ctx: &mut CastContext) -> Decimal {
    let scale = target.decimal.max(0) as u32;
    if target.decimal != UNSPECIFIED_LENGTH {
        value = value.round_dp_with_strategy(scale, RoundingStrategy::MidpointAwayFromZero);
    }
    if target.flen == UNSPECIFIED_LENGTH {
        return value;
    }
    let integer_digits = (target.flen - target.decimal.max(0)).max(0) as u32;
    let mut maximum = if integer_digits == 0 {
        Decimal::ZERO
    } else {
        Decimal::from(10_i64.pow(integer_digits.min(18))) - Decimal::new(1, scale)
    };
    if integer_digits > 18 {
        maximum = Decimal::MAX;
    }
    if value.abs() > maximum {
        ctx.warn(CastWarning::Overflow);
        if value.is_sign_negative() && !target.unsigned {
            -maximum
        } else {
            maximum
        }
    } else if target.unsigned && value.is_sign_negative() {
        ctx.warn(CastWarning::Overflow);
        Decimal::ZERO
    } else {
        value
    }
}

/// 转换到字符串类型。
fn to_string(
    value: Value,
    source: &FieldType,
    target: &FieldType,
    ctx: &mut CastContext,
) -> Result<Value, CastError> {
    let string = match value {
        Value::Int(value) if source.unsigned => (value as u64).to_string(),
        Value::Int(value) if source.mysql_type == MysqlType::Year && value == 0 => {
            "0000".to_owned()
        }
        Value::Int(value) => value.to_string(),
        Value::UInt(value) => value.to_string(),
        Value::Real(value) => format_float(value),
        Value::Decimal(value) => value.normalize().to_string(),
        Value::String(value) => value,
        Value::Bytes(value) => String::from_utf8_lossy(&value).into_owned(),
        Value::Time(value) => value.to_string(),
        Value::Duration(value) => value.to_string(),
        Value::Json(value) => value.to_string(),
        Value::VectorFloat32(value) => format_vector(&value),
        other => {
            return Err(CastError::Unsupported {
                from: kind_of(&other),
                to: FieldKind::String,
            });
        }
    };
    apply_string_type(string, target, ctx)
}

/// 格式化浮点为字符串。
fn format_float(value: f64) -> String {
    if value.fract() == 0.0 && value.is_finite() {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

/// 按目标字符串类型截断/填充并设置字符集。
fn apply_string_type(
    mut value: String,
    target: &FieldType,
    ctx: &mut CastContext,
) -> Result<Value, CastError> {
    if target.binary {
        let mut bytes = value.into_bytes();
        if target.flen >= 0 {
            let flen = target.flen as usize;
            if bytes.len() > flen {
                bytes.truncate(flen);
                ctx.warn(CastWarning::Truncated);
            } else if bytes.len() < flen {
                if flen as u64 > ctx.max_allowed_packet {
                    ctx.warn(CastWarning::AllowedPacketOverflow);
                    return Ok(Value::Null);
                }
                bytes.resize(flen, 0);
            }
        }
        return Ok(Value::Bytes(bytes));
    }
    if target.flen >= 0 {
        let flen = target.flen as usize;
        let char_count = value.chars().count();
        if char_count > flen {
            value = value.chars().take(flen).collect();
            ctx.warn(CastWarning::Truncated);
        }
    }
    Ok(Value::String(value))
}

/// 转换到 DATE/DATETIME/TIMESTAMP。
fn to_time(value: Value, target: &FieldType, ctx: &mut CastContext) -> Result<Value, CastError> {
    let parsed = match value {
        Value::Time(value) => value,
        Value::Duration(value) => duration_to_time(&value, ctx.current_date)?,
        Value::Int(value) => parse_time_string(&value.to_string(), target, ctx)?,
        Value::UInt(value) => parse_time_string(&value.to_string(), target, ctx)?,
        Value::Real(value) => parse_time_string(&format_float(value), target, ctx)?,
        Value::Decimal(value) => parse_time_string(&value.to_string(), target, ctx)?,
        Value::String(value) => parse_time_string(&value, target, ctx)?,
        Value::Bytes(value) => parse_time_string(&String::from_utf8_lossy(&value), target, ctx)?,
        Value::Json(JsonValue::String(value)) => parse_time_string(&value, target, ctx)?,
        other => {
            return Err(CastError::Unsupported {
                from: kind_of(&other),
                to: target.kind,
            });
        }
    };
    Ok(Value::Time(coerce_time(parsed, target)?))
}

/// 解析时间字符串。
fn parse_time_string(
    value: &str,
    target: &FieldType,
    ctx: &mut CastContext,
) -> Result<MysqlTime, CastError> {
    let trimmed = value.trim().trim_matches('"');
    let fsp = target.decimal.clamp(0, MAX_FSP as i32) as u32;
    let normalized = if trimmed.contains('-') {
        trimmed.to_owned()
    } else {
        let (whole, fraction) = trimmed.split_once('.').unwrap_or((trimmed, ""));
        let digits: String = whole.chars().filter(char::is_ascii_digit).collect();
        if digits.len() == 8 {
            format!("{}-{}-{}", &digits[..4], &digits[4..6], &digits[6..8])
        } else if digits.len() == 14 {
            format!(
                "{}-{}-{} {}:{}:{}.{}",
                &digits[..4],
                &digits[4..6],
                &digits[6..8],
                &digits[8..10],
                &digits[10..12],
                &digits[12..14],
                fraction
            )
        } else {
            return Err(CastError::InvalidTime(value.to_owned()));
        }
    };
    let formats = ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S", "%Y-%m-%d"];
    for format in formats {
        if format == "%Y-%m-%d" {
            if let Ok(date) = NaiveDate::parse_from_str(normalized.trim_end_matches('.'), format) {
                return coerce_time(MysqlTime::Date(date), target);
            }
        } else if let Ok(value) =
            NaiveDateTime::parse_from_str(normalized.trim_end_matches('.'), format)
        {
            let rounded = round_datetime(value, fsp)?;
            return Ok(MysqlTime::DateTime(rounded, fsp));
        }
    }
    ctx.warn(CastWarning::Truncated);
    Err(CastError::InvalidTime(value.to_owned()))
}

/// 将时间值强制到目标时间种类。
fn coerce_time(value: MysqlTime, target: &FieldType) -> Result<MysqlTime, CastError> {
    if target.kind == FieldKind::Date {
        return Ok(MysqlTime::Date(value.date()));
    }
    let fsp = target.decimal.clamp(0, MAX_FSP as i32) as u32;
    let datetime = NaiveDateTime::new(value.date(), value.time());
    let datetime = round_datetime(datetime, fsp)?;
    Ok(if target.kind == FieldKind::Timestamp {
        MysqlTime::Timestamp(datetime, fsp)
    } else {
        MysqlTime::DateTime(datetime, fsp)
    })
}

/// 按 FSP 对 datetime 做舍入。
fn round_datetime(value: NaiveDateTime, fsp: u32) -> Result<NaiveDateTime, CastError> {
    let factor = 10_u32.pow(6 - fsp.min(6));
    let micros = value.nanosecond() / 1_000;
    let rounded = ((micros + factor / 2) / factor) * factor;
    let base = value
        .with_nanosecond(0)
        .ok_or_else(|| CastError::InvalidTime(value.to_string()))?;
    Ok(base + ChronoDuration::microseconds(rounded as i64))
}

/// 将 TIME 与基准日期合成 DATETIME。
fn duration_to_time(value: &MysqlDuration, date: NaiveDate) -> Result<MysqlTime, CastError> {
    let midnight = NaiveDateTime::new(date, NaiveTime::MIN);
    Ok(MysqlTime::DateTime(
        midnight + ChronoDuration::microseconds(value.signed_micros()),
        value.fsp,
    ))
}

/// 转换到 TIME/DURATION。
fn to_duration(
    value: Value,
    target: &FieldType,
    ctx: &mut CastContext,
) -> Result<Value, CastError> {
    let fsp = target.decimal.clamp(0, MAX_FSP as i32) as u32;
    let duration = match value {
        Value::Duration(value) => round_duration(value, fsp)?,
        Value::Time(value) => {
            let time = value.time();
            round_duration(
                MysqlDuration::new(
                    false,
                    time.hour(),
                    time.minute(),
                    time.second(),
                    time.nanosecond() / 1_000,
                    value.fsp(),
                )?,
                fsp,
            )?
        }
        Value::Int(value) => parse_duration(&value.to_string(), fsp)?,
        Value::UInt(value) => parse_duration(&value.to_string(), fsp)?,
        Value::Real(value) => parse_duration(&format_float(value), fsp)?,
        Value::Decimal(value) => parse_duration(&value.to_string(), fsp)?,
        Value::String(value) => parse_duration(&value, fsp)?,
        Value::Bytes(value) => parse_duration(&String::from_utf8_lossy(&value), fsp)?,
        Value::Json(JsonValue::String(value)) => parse_duration(&value, fsp)?,
        other => {
            return Err(CastError::Unsupported {
                from: kind_of(&other),
                to: FieldKind::Duration,
            });
        }
    };
    let _ = ctx;
    Ok(Value::Duration(duration))
}

/// 解析 duration 字符串。
fn parse_duration(value: &str, fsp: u32) -> Result<MysqlDuration, CastError> {
    let trimmed = value.trim().trim_matches('"');
    let negative = trimmed.starts_with('-');
    let unsigned = trimmed.trim_start_matches(['+', '-']);
    let (whole, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let (hours, minutes, seconds) = if whole.contains(':') {
        let parts: Vec<_> = whole.split(':').collect();
        if parts.len() != 3 {
            return Err(CastError::InvalidDuration(value.to_owned()));
        }
        (
            parse_u32(parts[0], value)?,
            parse_u32(parts[1], value)?,
            parse_u32(parts[2], value)?,
        )
    } else {
        let number = parse_u32(whole, value)?;
        (number / 10_000, (number / 100) % 100, number % 100)
    };
    let mut micro_string: String = fraction.chars().take(7).collect();
    while micro_string.len() < 7 {
        micro_string.push('0');
    }
    let seven_digits = micro_string.parse::<u32>().unwrap_or(0);
    let mut micros = (seven_digits + 5) / 10;
    let factor = 10_u32.pow(6 - fsp.min(6));
    micros = ((micros + factor / 2) / factor) * factor;
    let mut total_seconds = (hours * 60 + minutes) * 60 + seconds;
    if micros >= 1_000_000 {
        total_seconds += 1;
        micros -= 1_000_000;
    }
    MysqlDuration::new(
        negative,
        total_seconds / 3_600,
        (total_seconds / 60) % 60,
        total_seconds % 60,
        micros,
        fsp,
    )
}

/// 按 FSP 舍入 duration。
fn round_duration(value: MysqlDuration, fsp: u32) -> Result<MysqlDuration, CastError> {
    parse_duration(&value.numeric_string(), fsp)
}

/// 解析无符号 32 位整数片段。
fn parse_u32(value: &str, original: &str) -> Result<u32, CastError> {
    value
        .parse::<u32>()
        .map_err(|_| CastError::InvalidDuration(original.to_owned()))
}

/// 转换到 JSON。
fn to_json(value: Value, source: &FieldType, target: &FieldType) -> Result<Value, CastError> {
    let json = match value {
        Value::Json(value) => value,
        Value::Int(value) if source.unsigned => JsonValue::Number(JsonNumber::from(value as u64)),
        Value::Int(value) => JsonValue::Number(JsonNumber::from(value)),
        Value::UInt(value) => JsonValue::Number(JsonNumber::from(value)),
        Value::Real(value) => JsonNumber::from_f64(value)
            .map(JsonValue::Number)
            .ok_or_else(|| CastError::InvalidJson(value.to_string()))?,
        Value::Decimal(value) => JsonNumber::from_f64(
            value
                .to_f64()
                .ok_or_else(|| CastError::InvalidDecimal(value.to_string()))?,
        )
        .map(JsonValue::Number)
        .ok_or_else(|| CastError::InvalidJson(value.to_string()))?,
        Value::String(value) if target.parse_to_json => {
            serde_json::from_str(&value).map_err(|_| CastError::InvalidJson(value))?
        }
        Value::String(value) => JsonValue::String(value),
        Value::Bytes(value) if target.parse_to_json => serde_json::from_slice(&value)
            .map_err(|_| CastError::InvalidJson(String::from_utf8_lossy(&value).into_owned()))?,
        Value::Bytes(value) => JsonValue::String(String::from_utf8_lossy(&value).into_owned()),
        Value::Time(value) => JsonValue::String(value.to_string()),
        Value::Duration(value) => JsonValue::String(value.to_string()),
        other => {
            return Err(CastError::Unsupported {
                from: kind_of(&other),
                to: FieldKind::Json,
            });
        }
    };
    Ok(Value::Json(json))
}

/// 转换到 VectorFloat32。
fn to_vector(value: Value) -> Result<Value, CastError> {
    match value {
        Value::VectorFloat32(value) => Ok(Value::VectorFloat32(value)),
        Value::String(value) => parse_vector(&value).map(Value::VectorFloat32),
        Value::Bytes(value) => {
            parse_vector(&String::from_utf8_lossy(&value)).map(Value::VectorFloat32)
        }
        other => Err(CastError::Unsupported {
            from: kind_of(&other),
            to: FieldKind::VectorFloat32,
        }),
    }
}

/// 解析向量字面量字符串。
fn parse_vector(value: &str) -> Result<Vec<f32>, CastError> {
    let json: JsonValue =
        serde_json::from_str(value).map_err(|_| CastError::InvalidVector(value.to_owned()))?;
    let JsonValue::Array(values) = json else {
        return Err(CastError::InvalidVector(value.to_owned()));
    };
    values
        .into_iter()
        .map(|value| {
            value
                .as_f64()
                .map(|value| value as f32)
                .ok_or_else(|| CastError::InvalidVector(value.to_string()))
        })
        .collect()
}

/// 格式化向量为字符串。
fn format_vector(value: &[f32]) -> String {
    format!(
        "[{}]",
        value
            .iter()
            .map(|value| format_float(*value as f64))
            .collect::<Vec<_>>()
            .join(",")
    )
}

/// 转换到数组类型。
fn to_array(value: Value, target: &FieldType, ctx: &mut CastContext) -> Result<Value, CastError> {
    let Value::Json(JsonValue::Array(values)) = value else {
        return Err(CastError::InvalidArray);
    };
    let element = target
        .element_type
        .as_deref()
        .ok_or(CastError::InvalidArray)?;
    values
        .into_iter()
        .map(|value| {
            if value.is_null() {
                Ok(Value::Null)
            } else {
                cast_value(
                    Value::Json(value),
                    &FieldType::new(FieldKind::Json),
                    element,
                    false,
                    ctx,
                )
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

/// JSON 值转有符号整数。
fn json_to_i64(value: JsonValue, ctx: &mut CastContext) -> Result<i64, CastError> {
    match value {
        JsonValue::Null => Ok(0),
        JsonValue::Bool(value) => Ok(i64::from(value)),
        JsonValue::Number(value) => value
            .as_i64()
            .or_else(|| value.as_u64().map(|value| value as i64))
            .or_else(|| value.as_f64().map(|value| value.round() as i64))
            .ok_or_else(|| CastError::InvalidInteger(value.to_string())),
        JsonValue::String(value) => parse_signed_integer(&value, ctx),
        value => {
            ctx.warn(CastWarning::Truncated);
            Err(CastError::InvalidInteger(value.to_string()))
        }
    }
}

/// JSON 值转无符号整数。
fn json_to_u64(value: JsonValue, in_union: bool, ctx: &mut CastContext) -> Result<u64, CastError> {
    match value {
        JsonValue::Null => Ok(0),
        JsonValue::Bool(value) => Ok(u64::from(value)),
        JsonValue::Number(value) if value.is_u64() => Ok(value.as_u64().expect("checked")),
        JsonValue::Number(value) if value.is_i64() => {
            value_to_u64(Value::Int(value.as_i64().expect("checked")), in_union, ctx)
        }
        JsonValue::Number(value) => Ok(value.as_f64().unwrap_or(0.0).round().max(0.0) as u64),
        JsonValue::String(value) => parse_unsigned_integer(&value, in_union, ctx),
        value => {
            ctx.warn(CastWarning::Truncated);
            Err(CastError::InvalidInteger(value.to_string()))
        }
    }
}

/// JSON 值转浮点。
fn json_to_f64(value: JsonValue, ctx: &mut CastContext) -> Result<f64, CastError> {
    match value {
        JsonValue::Null => Ok(0.0),
        JsonValue::Bool(value) => Ok(if value { 1.0 } else { 0.0 }),
        JsonValue::Number(value) => value
            .as_f64()
            .ok_or_else(|| CastError::InvalidDecimal(value.to_string())),
        JsonValue::String(value) => Ok(parse_float(&value, ctx)),
        value => Err(CastError::InvalidDecimal(value.to_string())),
    }
}

/// JSON 值转 DECIMAL。
fn json_to_decimal(value: JsonValue, ctx: &mut CastContext) -> Result<Decimal, CastError> {
    match value {
        JsonValue::Null => Ok(Decimal::ZERO),
        JsonValue::Bool(value) => Ok(Decimal::from(i64::from(value))),
        JsonValue::Number(value) => Decimal::from_str(&value.to_string())
            .map_err(|_| CastError::InvalidDecimal(value.to_string())),
        JsonValue::String(value) => Ok(parse_decimal(&value, ctx)),
        value => Err(CastError::InvalidDecimal(value.to_string())),
    }
}

/// 由运行时值推断 `FieldKind`。
fn kind_of(value: &Value) -> FieldKind {
    match value {
        Value::Null | Value::Int(_) | Value::UInt(_) => FieldKind::Int,
        Value::Real(_) => FieldKind::Real,
        Value::Decimal(_) => FieldKind::Decimal,
        Value::String(_) | Value::Bytes(_) => FieldKind::String,
        Value::Time(MysqlTime::Date(_)) => FieldKind::Date,
        Value::Time(MysqlTime::DateTime(_, _)) => FieldKind::DateTime,
        Value::Time(MysqlTime::Timestamp(_, _)) => FieldKind::Timestamp,
        Value::Duration(_) => FieldKind::Duration,
        Value::Json(_) => FieldKind::Json,
        Value::VectorFloat32(_) => FieldKind::VectorFloat32,
        Value::Array(_) => FieldKind::Array,
    }
}

/// 按目标类型把 JSON 转为对应 SQL 值。
pub fn ConvertJSON2Tp(
    value: JsonValue,
    target: &FieldType,
    ctx: &mut CastContext,
) -> Result<Value, CastError> {
    cast_value(
        Value::Json(value),
        &FieldType::new(FieldKind::Json),
        target,
        false,
        ctx,
    )
}

/// 判断表达式是否可隐式按整数求值。
pub fn CanImplicitEvalInt(expr: &Expression) -> bool {
    expr.field_type.hybrid && expr.field_type.kind == FieldKind::Int
}

/// 判断表达式是否可隐式按浮点求值。
pub fn CanImplicitEvalReal(expr: &Expression) -> bool {
    expr.field_type.hybrid && expr.field_type.kind == FieldKind::Real
}

/// 如需要则包装一层转整数的 CAST。
pub fn WrapWithCastAsInt(expr: Expression, target_type: Option<&FieldType>) -> Expression {
    if expr.field_type.kind == FieldKind::Int {
        return expr;
    }
    let mut target = FieldType::new(FieldKind::Int)
        .with_flen(expr.field_type.flen)
        .with_not_null(expr.field_type.not_null);
    target.unsigned = target_type.map_or(expr.field_type.unsigned, |target| target.unsigned);
    BuildCastFunction(expr, target)
}

/// 如需要则包装转浮点的 CAST。
pub fn WrapWithCastAsReal(expr: Expression) -> Expression {
    if expr.field_type.kind == FieldKind::Real {
        return expr;
    }
    let target = FieldType::new(FieldKind::Real)
        .with_flen(22)
        .with_not_null(expr.field_type.not_null);
    BuildCastFunction(expr, target)
}

/// 如需要则包装转 DECIMAL 的 CAST。
pub fn WrapWithCastAsDecimal(expr: Expression) -> Expression {
    if expr.field_type.kind == FieldKind::Decimal {
        return expr;
    }
    let mut target = FieldType::new(FieldKind::Decimal)
        .with_flen(expr.field_type.flen.clamp(0, MAX_DECIMAL_WIDTH))
        .with_decimal(expr.field_type.decimal)
        .with_unsigned(expr.field_type.unsigned)
        .with_not_null(expr.field_type.not_null);
    if expr.field_type.kind == FieldKind::Int {
        target.flen = minimalDecimalLenForHoldingInteger(expr.field_type.mysql_type);
        target.decimal = 0;
    }
    BuildCastFunction(expr, target)
}

/// 如需要则包装转字符串的 CAST。
pub fn WrapWithCastAsString(expr: Expression) -> Expression {
    if expr.field_type.kind == FieldKind::String {
        return expr;
    }
    let mut target = FieldType::new(FieldKind::String);
    target.flen = match expr.field_type.kind {
        FieldKind::Int => 20,
        FieldKind::Decimal if expr.field_type.flen != UNSPECIFIED_LENGTH => {
            expr.field_type.flen + 3
        }
        FieldKind::Real => UNSPECIFIED_LENGTH,
        _ => expr.field_type.flen,
    };
    BuildCastFunction(expr, target)
}

/// 如需要则包装转时间的 CAST。
pub fn WrapWithCastAsTime(expr: Expression, mut target: FieldType) -> Expression {
    if expr.field_type.kind == target.kind
        || (matches!(expr.field_type.kind, FieldKind::Date | FieldKind::Timestamp)
            && target.kind == FieldKind::DateTime)
    {
        return expr;
    }
    target.decimal = match expr.field_type.kind {
        FieldKind::Int => 0,
        FieldKind::String | FieldKind::Real | FieldKind::Json => MAX_FSP as i32,
        _ => expr.field_type.decimal.min(MAX_FSP as i32),
    };
    target.flen = if target.kind == FieldKind::Date {
        MAX_DATE_WIDTH
    } else {
        MAX_DATETIME_WIDTH_NO_FSP
            + if target.decimal > 0 {
                1 + target.decimal
            } else {
                0
            }
    };
    BuildCastFunction(expr, target)
}

/// 如需要则包装转 TIME 的 CAST。
pub fn WrapWithCastAsDuration(expr: Expression) -> Expression {
    if expr.field_type.kind == FieldKind::Duration {
        return expr;
    }
    let decimal = if matches!(
        expr.field_type.kind,
        FieldKind::Date | FieldKind::DateTime | FieldKind::Timestamp
    ) {
        expr.field_type.decimal
    } else {
        MAX_FSP as i32
    };
    let target = FieldType::new(FieldKind::Duration)
        .with_decimal(decimal)
        .with_flen(MAX_DURATION_WIDTH_NO_FSP + if decimal > 0 { decimal + 1 } else { 0 });
    BuildCastFunction(expr, target)
}

/// 如需要则包装转 JSON 的 CAST。
pub fn WrapWithCastAsJSON(expr: Expression) -> Expression {
    if expr.field_type.kind == FieldKind::Json && !expr.field_type.parse_to_json {
        return expr;
    }
    BuildCastFunction(expr, FieldType::new(FieldKind::Json).with_flen(12_582_912))
}

/// 如需要则包装转向量的 CAST。
pub fn WrapWithCastAsVectorFloat32(expr: Expression) -> Expression {
    if expr.field_type.kind == FieldKind::VectorFloat32 {
        return expr;
    }
    BuildCastFunction(expr, FieldType::new(FieldKind::VectorFloat32))
}

/// 按参数类型调整 CAST 到字符串的返回字段类型。
pub fn adjustRetFtForCastString(ret: &mut FieldType, arg: &FieldType) {
    if ret.mysql_type == MysqlType::String || ret.flen != UNSPECIFIED_LENGTH {
        return;
    }
    ret.flen = match arg.kind {
        FieldKind::Int => match arg.mysql_type {
            MysqlType::Tiny => {
                if arg.unsigned {
                    3
                } else {
                    4
                }
            }
            MysqlType::Short => {
                if arg.unsigned {
                    5
                } else {
                    6
                }
            }
            MysqlType::Int24 => {
                if arg.unsigned {
                    8
                } else {
                    9
                }
            }
            MysqlType::Long => {
                if arg.unsigned {
                    10
                } else {
                    11
                }
            }
            MysqlType::LongLong => 20,
            MysqlType::Year => 4,
            MysqlType::Bit => arg.flen,
            _ => ret.flen,
        },
        FieldKind::Real => {
            if arg.mysql_type == MysqlType::Float {
                87
            } else {
                370
            }
        }
        FieldKind::Decimal => decimalPrecisionToLength(arg),
        FieldKind::Date => MAX_DATE_WIDTH,
        FieldKind::DateTime | FieldKind::Timestamp => {
            MAX_DATETIME_WIDTH_NO_FSP + if arg.decimal > 0 { arg.decimal + 1 } else { 0 }
        }
        FieldKind::Duration => {
            MAX_DURATION_WIDTH_NO_FSP + if arg.decimal > 0 { arg.decimal + 1 } else { 0 }
        }
        FieldKind::Json => i32::MAX,
        FieldKind::String => match arg.mysql_type {
            MysqlType::TinyBlob => 255,
            MysqlType::Blob => 65_535 * 4,
            MysqlType::MediumBlob => 16_777_215 * 4,
            MysqlType::LongBlob => i32::MAX,
            _ => arg.flen.max(ret.flen),
        },
        FieldKind::VectorFloat32 | FieldKind::Array => ret.flen,
    };
}

/// 能容纳给定整数 MySQL 类型的最小 DECIMAL 长度。
pub fn minimalDecimalLenForHoldingInteger(mysql_type: MysqlType) -> i32 {
    match mysql_type {
        MysqlType::Tiny => 3,
        MysqlType::Short => 5,
        MysqlType::Int24 => 8,
        MysqlType::Long => 10,
        MysqlType::LongLong => 20,
        MysqlType::Year => 4,
        _ => 20,
    }
}

/// 为 DOUBLE 结果设置 flen/decimal。
pub fn setDataTypeDouble(src_decimal: i32) -> (i32, i32) {
    let decimal = -1;
    (floatLength(src_decimal, decimal), decimal)
}

/// 计算浮点显示长度。
pub fn floatLength(src_decimal: i32, decimal_parameter: i32) -> i32 {
    const DBL_DIG: i32 = 15;
    if src_decimal != -1 {
        DBL_DIG + 2 + decimal_parameter
    } else {
        DBL_DIG + 8
    }
}

/// 由 DECIMAL 字段推算显示长度。
pub fn decimalPrecisionToLength(field_type: &FieldType) -> i32 {
    let precision = field_type.flen;
    let scale = field_type.decimal;
    if precision == UNSPECIFIED_LENGTH || scale == UNSPECIFIED_LENGTH {
        return UNSPECIFIED_LENGTH;
    }
    let mut result = precision;
    if scale > 0 {
        result += 1;
    }
    if !field_type.unsigned && precision > 0 {
        result += 1;
    }
    if result == 0 { 1 } else { result }
}

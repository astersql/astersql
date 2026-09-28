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

// Native Rust implementation of the scalar comparison behavior in
// `builtin_compare.go`.
//
// TiDB's Go implementation represents every evaluation type/operator pair by
// a separate signature struct. This module keeps the same dispatch matrix in
// [`CompareType`] and [`Op`], while sharing the type-independent evaluation
// code. NULL handling, mixed signed/unsigned integers, constant refinement,
// INTERVAL search, COALESCE short-circuiting, and GREATEST/LEAST coercion all
// follow the source Go control flow.

//
// 标量比较内核：类型决议、COALESCE/GREATEST/LEAST/INTERVAL、NULL 与有无符号混比、
// 以及面向整列比较的常量精化（ceil/floor、无符号折叠）。控制流对齐 `builtin_compare.go`。

use std::cmp::Ordering;
use std::error::Error;
use std::fmt;

use chrono::{NaiveDate, NaiveDateTime, Timelike};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::Value as JsonValue;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 比较用求值类型（含 VectorFloat32）。
pub enum EvalType {
    Int,
    Real,
    Decimal,
    String,
    Datetime,
    Timestamp,
    Duration,
    Json,
    VectorFloat32,
}

impl EvalType {
    /// 字符串族（含时间/JSON/向量在 Go 字符串比较分支中的归类）。
    fn is_string_kind(self) -> bool {
        matches!(
            self,
            Self::String
                | Self::Datetime
                | Self::Timestamp
                | Self::Duration
                | Self::Json
                | Self::VectorFloat32
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 参与比较类型决议的 MySQL 字段类型子集。
pub enum MysqlType {
    Unspecified,
    Null,
    Bit,
    LongLong,
    Double,
    NewDecimal,
    Varchar,
    Date,
    Datetime,
    Timestamp,
    Duration,
    Year,
    Json,
    VectorFloat32,
    Enum,
    Set,
}

impl MysqlType {
    /// DATE/DATETIME/TIMESTAMP。
    fn is_time(self) -> bool {
        matches!(self, Self::Date | Self::Datetime | Self::Timestamp)
    }

    /// 含 Duration 的时态类型。
    fn is_temporal(self) -> bool {
        self.is_time() || self == Self::Duration
    }

    /// 带日期分量的时态类型（不含纯 Duration）。
    fn is_temporal_with_date(self) -> bool {
        self.is_time()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 字段类型元数据：符号、可空、hybrid（ENUM/SET）、flen/decimal。
pub struct FieldType {
    pub tp: MysqlType,
    pub unsigned: bool,
    pub not_null: bool,
    pub hybrid: bool,
    pub flen: i32,
    pub decimal: i32,
}

impl FieldType {
    /// 按类型构造默认 FieldType（ENUM/SET 自动标 hybrid）。
    pub fn new(tp: MysqlType) -> Self {
        Self {
            tp,
            unsigned: false,
            not_null: false,
            hybrid: matches!(tp, MysqlType::Enum | MysqlType::Set),
            flen: 0,
            decimal: 0,
        }
    }

    /// 标记无符号。
    pub fn unsigned(mut self) -> Self {
        self.unsigned = true;
        self
    }

    /// 标记 NOT NULL。
    pub fn not_null(mut self) -> Self {
        self.not_null = true;
        self
    }

    /// 设置显示宽度与小数位。
    pub fn with_flen_decimal(mut self, flen: i32, decimal: i32) -> Self {
        self.flen = flen;
        self.decimal = decimal;
        self
    }

    /// 映射到比较用 EvalType。
    pub fn eval_type(&self) -> EvalType {
        match self.tp {
            MysqlType::Bit | MysqlType::LongLong | MysqlType::Year => EvalType::Int,
            MysqlType::Double => EvalType::Real,
            MysqlType::NewDecimal => EvalType::Decimal,
            MysqlType::Date | MysqlType::Datetime => EvalType::Datetime,
            MysqlType::Timestamp => EvalType::Timestamp,
            MysqlType::Duration => EvalType::Duration,
            MysqlType::Json => EvalType::Json,
            MysqlType::VectorFloat32 => EvalType::VectorFloat32,
            MysqlType::Unspecified
            | MysqlType::Null
            | MysqlType::Varchar
            | MysqlType::Enum
            | MysqlType::Set => EvalType::String,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 表达式形态：常量、列、关联列或其它。
pub enum ExpressionKind {
    Constant,
    Column,
    CorrelatedColumn,
    Other,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 类型决议所需的表达式元信息。
pub struct ExpressionMeta {
    pub field_type: FieldType,
    pub kind: ExpressionKind,
    pub binary_literal: bool,
}

impl ExpressionMeta {
    /// 常量表达式元信息。
    pub fn constant(field_type: FieldType) -> Self {
        Self {
            field_type,
            kind: ExpressionKind::Constant,
            binary_literal: false,
        }
    }

    /// 列表达式元信息。
    pub fn column(field_type: FieldType) -> Self {
        Self {
            field_type,
            kind: ExpressionKind::Column,
            binary_literal: false,
        }
    }

    /// 关联列元信息。
    pub fn correlated_column(field_type: FieldType) -> Self {
        Self {
            field_type,
            kind: ExpressionKind::CorrelatedColumn,
            binary_literal: false,
        }
    }

    /// 标记二进制字面量（影响 BETWEEN 整型决议）。
    pub fn binary_literal(mut self) -> Self {
        self.binary_literal = true;
        self
    }

    /// 是否为常量。
    fn is_constant(&self) -> bool {
        self.kind == ExpressionKind::Constant
    }

    /// 是否为时态列；Go 不把关联列纳入此特殊转换。
    fn is_temporal_column(&self) -> bool {
        self.kind == ExpressionKind::Column && self.field_type.tp.is_temporal()
    }
}

/// 基础比较类型决议，顺序对齐 Go `getBaseCmpType`。
/// Direct translation of Go's `getBaseCmpType` decision order.
pub fn get_base_cmp_type(
    mut lhs: EvalType,
    mut rhs: EvalType,
    lhs_field: Option<&FieldType>,
    rhs_field: Option<&FieldType>,
) -> EvalType {
    if let (Some(left), Some(right)) = (lhs_field, rhs_field)
        && (left.tp == MysqlType::Unspecified || right.tp == MysqlType::Unspecified)
    {
        if left.tp == right.tp {
            return EvalType::String;
        }
        if left.tp == MysqlType::Unspecified {
            lhs = rhs;
        } else {
            rhs = lhs;
        }
    }

    let lhs_hybrid = lhs_field.is_some_and(|field| field.hybrid);
    let rhs_hybrid = rhs_field.is_some_and(|field| field.hybrid);
    // 两侧均为字符串族 → 字符串比较。
    if lhs.is_string_kind() && rhs.is_string_kind() {
        EvalType::String
    } else if (lhs == EvalType::Int || lhs_hybrid) && (rhs == EvalType::Int || rhs_hybrid) {
        EvalType::Int
    } else if matches!(
        (lhs, rhs),
        (EvalType::Decimal, EvalType::String) | (EvalType::String, EvalType::Decimal)
    ) {
        EvalType::Real
    } else if matches!(lhs, EvalType::Int | EvalType::Decimal) || lhs_hybrid {
        if matches!(rhs, EvalType::Int | EvalType::Decimal) || rhs_hybrid {
            EvalType::Decimal
        } else if lhs_field.zip(rhs_field).is_some_and(|(left, right)| {
            (left.tp.is_temporal_with_date() && right.tp == MysqlType::Year)
                || (left.tp == MysqlType::Year && right.tp.is_temporal_with_date())
        }) {
            EvalType::Datetime
        } else {
            EvalType::Real
        }
    } else if lhs_field.zip(rhs_field).is_some_and(|(left, right)| {
        (left.tp.is_temporal_with_date() && right.tp == MysqlType::Year)
            || (left.tp == MysqlType::Year && right.tp.is_temporal_with_date())
    }) {
        EvalType::Datetime
    } else {
        EvalType::Real
    }
}

/// 精确比较类型：保留 DECIMAL/字符串常量精度，以及时态列与常量例外。
/// More precise comparison type selection, including the precision-preserving
/// decimal/string constant and temporal-column exceptions from Go.
pub fn get_accurate_cmp_type(lhs: &ExpressionMeta, rhs: &ExpressionMeta) -> EvalType {
    let lhs_eval = lhs.field_type.eval_type();
    let rhs_eval = rhs.field_type.eval_type();
    let mut cmp_type = get_base_cmp_type(
        lhs_eval,
        rhs_eval,
        Some(&lhs.field_type),
        Some(&rhs.field_type),
    );

    if lhs_eval == EvalType::VectorFloat32 || rhs_eval == EvalType::VectorFloat32 {
        cmp_type = EvalType::VectorFloat32;
    } else if lhs.field_type.tp == MysqlType::Json || rhs.field_type.tp == MysqlType::Json {
        cmp_type = EvalType::Json;
    } else if cmp_type == EvalType::String
        && (lhs.field_type.tp.is_time() || rhs.field_type.tp.is_time())
    {
        cmp_type = if lhs.field_type.tp == rhs.field_type.tp {
            lhs_eval
        } else {
            EvalType::Datetime
        };
    } else if lhs.field_type.tp == MysqlType::Duration && rhs.field_type.tp == MysqlType::Duration {
        cmp_type = EvalType::Duration;
    } else if matches!(cmp_type, EvalType::Real | EvalType::String) {
        let decimal_and_string_constant = (lhs_eval == EvalType::Decimal
            && !lhs.is_constant()
            && rhs_eval.is_string_kind()
            && rhs.is_constant())
            || (rhs_eval == EvalType::Decimal
                && !rhs.is_constant()
                && lhs_eval.is_string_kind()
                && lhs.is_constant());
        // DECIMAL 列 vs 字符串常量：保留 DECIMAL 精度，避免先转 Real 丢精度。
        if decimal_and_string_constant {
            cmp_type = EvalType::Decimal;
        } else if (lhs.is_temporal_column() && rhs.is_constant())
            || (rhs.is_temporal_column() && lhs.is_constant())
        {
            let temporal = if lhs.is_temporal_column() { lhs } else { rhs };
            if temporal.field_type.tp == MysqlType::Duration {
                cmp_type = EvalType::Duration;
            }
        }
    }
    cmp_type
}

/// BETWEEN 三参数的比较类型决议。
pub fn resolve_type_for_between(args: &[ExpressionMeta; 3]) -> EvalType {
    let mut cmp_type = args[0].field_type.eval_type();
    for arg in &args[1..] {
        cmp_type = get_base_cmp_type(cmp_type, arg.field_type.eval_type(), None, None);
    }
    if cmp_type == EvalType::String {
        if args[0].field_type.tp == MysqlType::Duration {
            cmp_type = EvalType::Duration;
        } else if args.iter().any(|arg| arg.field_type.tp.is_temporal()) {
            cmp_type = EvalType::Datetime;
        }
    }
    if args
        .iter()
        .all(|arg| arg.field_type.eval_type() == EvalType::Int || arg.binary_literal)
    {
        return EvalType::Int;
    }
    cmp_type
}

/// GREATEST/LEAST 结果 flen/decimal 取各参数最大值。
pub fn fix_flen_and_decimal_for_greatest_and_least(args: &[FieldType]) -> (i32, i32) {
    args.iter().fold((0, 0), |(flen, decimal), arg| {
        (flen.max(arg.flen), decimal.max(arg.decimal))
    })
}

#[derive(Clone, Debug, PartialEq)]
/// 比较用运行时值；Error 变体用于 COALESCE 短路测错。
pub enum Datum {
    Null,
    Int(i64),
    UInt(u64),
    Real(f64),
    Decimal(Decimal),
    String(String),
    Date(NaiveDate),
    DateTime(NaiveDateTime),
    Duration(i64),
    Json(JsonValue),
    VectorFloat32(Vec<f32>),
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 比较求值错误。
pub struct CompareError(String);

impl CompareError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for CompareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CompareError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 字符串比较排序规则（binary / utf8mb4_bin / general_ci）。
pub enum Collation {
    Binary,
    Utf8Mb4Bin,
    Utf8Mb4GeneralCi,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// GREATEST/LEAST 时态字符串归一模式。
pub enum TemporalMode {
    Direct,
    AsDate,
    AsDatetime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 实际执行比较时使用的统一类型。
pub enum CompareType {
    Int,
    Real,
    Decimal,
    String,
    Time,
    Duration,
    Json,
    VectorFloat32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 比较算子，含 NULL-safe 相等（<=>）。
pub enum Op {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
    NullEq,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 有符号/无符号整型值，避免过早收窄 u64。
pub enum IntValue {
    Signed(i64),
    Unsigned(u64),
}

/// Ordering 映射为 -1/0/1。
fn ordering_value(ordering: Ordering) -> i64 {
    match ordering {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// 有无符号混比：负的有符号值小于任意无符号值。
pub fn compare_int_values(lhs: IntValue, rhs: IntValue) -> Ordering {
    match (lhs, rhs) {
        (IntValue::Signed(left), IntValue::Signed(right)) => left.cmp(&right),
        (IntValue::Unsigned(left), IntValue::Unsigned(right)) => left.cmp(&right),
        (IntValue::Signed(left), IntValue::Unsigned(right)) => {
            // 有符号负数小于任意无符号值（含大于 i64::MAX 的位型）。
            if left < 0 {
                Ordering::Less
            } else {
                (left as u64).cmp(&right)
            }
        }
        (IntValue::Unsigned(left), IntValue::Signed(right)) => {
            if right < 0 {
                Ordering::Greater
            } else {
                left.cmp(&(right as u64))
            }
        }
    }
}

/// 按排序规则比较字符串（ci 去尾空格并小写）。
fn compare_strings(lhs: &str, rhs: &str, collation: Collation) -> Ordering {
    match collation {
        Collation::Binary | Collation::Utf8Mb4Bin => lhs.cmp(rhs),
        Collation::Utf8Mb4GeneralCi => lhs
            .trim_end_matches(' ')
            .to_lowercase()
            .cmp(&rhs.trim_end_matches(' ').to_lowercase()),
    }
}

/// NULL 与非 NULL 的次序（两 NULL 相等）。
fn compare_null(lhs_is_null: bool, rhs_is_null: bool) -> i64 {
    match (lhs_is_null, rhs_is_null) {
        (true, true) => 0,
        (true, false) => -1,
        (false, true) => 1,
        (false, false) => unreachable!("compare_null requires a NULL operand"),
    }
}

/// Datum 对应的比较类型；Null/Error 无类型。
fn datum_compare_type(datum: &Datum) -> Option<CompareType> {
    match datum {
        Datum::Null | Datum::Error(_) => None,
        Datum::Int(_) | Datum::UInt(_) => Some(CompareType::Int),
        Datum::Real(_) => Some(CompareType::Real),
        Datum::Decimal(_) => Some(CompareType::Decimal),
        Datum::String(_) => Some(CompareType::String),
        Datum::Date(_) | Datum::DateTime(_) => Some(CompareType::Time),
        Datum::Duration(_) => Some(CompareType::Duration),
        Datum::Json(_) => Some(CompareType::Json),
        Datum::VectorFloat32(_) => Some(CompareType::VectorFloat32),
    }
}

/// 多参数聚合比较类型（字符串/向量/JSON/时态优先，有无符号混用升 DECIMAL）。
fn aggregate_compare_type(args: &[Datum]) -> CompareType {
    let types: Vec<CompareType> = args.iter().filter_map(datum_compare_type).collect();
    if types.iter().any(|tp| *tp == CompareType::String) {
        return CompareType::String;
    }
    if types.iter().any(|tp| *tp == CompareType::VectorFloat32) {
        return CompareType::VectorFloat32;
    }
    if types.iter().any(|tp| *tp == CompareType::Json) {
        return CompareType::Json;
    }
    if types.iter().any(|tp| *tp == CompareType::Time) {
        return CompareType::Time;
    }
    if types.iter().any(|tp| *tp == CompareType::Duration) {
        return CompareType::Duration;
    }
    if types.iter().any(|tp| *tp == CompareType::Real) {
        return CompareType::Real;
    }
    let mixed_sign = args.iter().any(|arg| matches!(arg, Datum::Int(_)))
        && args.iter().any(|arg| matches!(arg, Datum::UInt(_)));
    if mixed_sign || types.iter().any(|tp| *tp == CompareType::Decimal) {
        CompareType::Decimal
    } else {
        CompareType::Int
    }
}

/// Real 转字符串：有限整数不带小数点。
fn real_to_string(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

/// DateTime 格式化（无小数秒时省略）。
fn datetime_to_string(value: &NaiveDateTime) -> String {
    if value.nanosecond() == 0 {
        value.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        value.format("%Y-%m-%d %H:%M:%S%.f").to_string()
    }
}

/// 将 Datum 转为目标比较类型；传播 Error，保留 Null。
fn cast_datum(datum: &Datum, target: CompareType) -> Result<Datum, CompareError> {
    if matches!(datum, Datum::Null) {
        return Ok(Datum::Null);
    }
    if let Datum::Error(message) = datum {
        return Err(CompareError::new(message.clone()));
    }
    match target {
        CompareType::Int => {
            match datum {
                Datum::Int(value) => Ok(Datum::Int(*value)),
                Datum::UInt(value) => Ok(Datum::UInt(*value)),
                Datum::Real(value) => Ok(Datum::Int(*value as i64)),
                Datum::Decimal(value) => value.trunc().to_i64().map(Datum::Int).ok_or_else(|| {
                    CompareError::new("decimal is outside the signed integer range")
                }),
                Datum::String(value) => Ok(Datum::Int(value.parse::<i64>().unwrap_or(0))),
                _ => Err(CompareError::new("value cannot be evaluated as integer")),
            }
        }
        CompareType::Real => match datum {
            Datum::Int(value) => Ok(Datum::Real(*value as f64)),
            Datum::UInt(value) => Ok(Datum::Real(*value as f64)),
            Datum::Real(value) => Ok(Datum::Real(*value)),
            Datum::Decimal(value) => value
                .to_f64()
                .map(Datum::Real)
                .ok_or_else(|| CompareError::new("decimal cannot be represented as real")),
            Datum::String(value) => Ok(Datum::Real(value.parse::<f64>().unwrap_or(0.0))),
            _ => Err(CompareError::new("value cannot be evaluated as real")),
        },
        CompareType::Decimal => match datum {
            Datum::Int(value) => Ok(Datum::Decimal(Decimal::from(*value))),
            Datum::UInt(value) => Ok(Datum::Decimal(Decimal::from(*value))),
            Datum::Real(value) => Decimal::from_f64_retain(*value)
                .map(Datum::Decimal)
                .ok_or_else(|| CompareError::new("real cannot be represented as decimal")),
            Datum::Decimal(value) => Ok(Datum::Decimal(*value)),
            Datum::String(value) => Ok(Datum::Decimal(value.parse().unwrap_or(Decimal::ZERO))),
            _ => Err(CompareError::new("value cannot be evaluated as decimal")),
        },
        CompareType::String => match datum {
            Datum::Int(value) => Ok(Datum::String(value.to_string())),
            Datum::UInt(value) => Ok(Datum::String(value.to_string())),
            Datum::Real(value) => Ok(Datum::String(real_to_string(*value))),
            Datum::Decimal(value) => Ok(Datum::String(value.normalize().to_string())),
            Datum::String(value) => Ok(Datum::String(value.clone())),
            Datum::Date(value) => Ok(Datum::String(value.format("%Y-%m-%d").to_string())),
            Datum::DateTime(value) => Ok(Datum::String(datetime_to_string(value))),
            Datum::Duration(value) => Ok(Datum::String(value.to_string())),
            Datum::Json(value) => Ok(Datum::String(value.to_string())),
            Datum::VectorFloat32(value) => Ok(Datum::String(format!("{value:?}"))),
            Datum::Null | Datum::Error(_) => unreachable!(),
        },
        CompareType::Time => match datum {
            Datum::Date(value) => Ok(Datum::DateTime(value.and_hms_opt(0, 0, 0).unwrap())),
            Datum::DateTime(value) => Ok(Datum::DateTime(*value)),
            Datum::String(value) => parse_datetime(value).map(Datum::DateTime),
            _ => Err(CompareError::new("value cannot be evaluated as time")),
        },
        CompareType::Duration => match datum {
            Datum::Duration(value) => Ok(Datum::Duration(*value)),
            Datum::Int(value) => Ok(Datum::Duration(*value)),
            Datum::String(value) => value
                .parse::<i64>()
                .map(Datum::Duration)
                .map_err(|_| CompareError::new("value cannot be evaluated as duration")),
            _ => Err(CompareError::new("value cannot be evaluated as duration")),
        },
        CompareType::Json => match datum {
            Datum::Json(value) => Ok(Datum::Json(value.clone())),
            Datum::Int(value) => Ok(Datum::Json(JsonValue::from(*value))),
            Datum::UInt(value) => Ok(Datum::Json(JsonValue::from(*value))),
            Datum::Real(value) => Ok(Datum::Json(JsonValue::from(*value))),
            Datum::Decimal(value) => Ok(Datum::Json(JsonValue::from(value.to_string()))),
            Datum::String(value) => Ok(Datum::Json(JsonValue::from(value.clone()))),
            _ => Err(CompareError::new("value cannot be evaluated as JSON")),
        },
        CompareType::VectorFloat32 => match datum {
            Datum::VectorFloat32(value) => Ok(Datum::VectorFloat32(value.clone())),
            _ => Err(CompareError::new(
                "value cannot be evaluated as vector<float32>",
            )),
        },
    }
}

/// 解析日期时间字符串（含纯日期）。
fn parse_datetime(value: &str) -> Result<NaiveDateTime, CompareError> {
    for format in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(result) = NaiveDateTime::parse_from_str(value, format) {
            return Ok(result);
        }
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .map(|date| date.and_hms_opt(0, 0, 0).unwrap())
        .map_err(|_| CompareError::new(format!("invalid datetime: {value}")))
}

/// 按时态模式归一为可比较字符串。
fn normalize_temporal(datum: &Datum, mode: TemporalMode) -> Result<String, CompareError> {
    let raw = match cast_datum(datum, CompareType::String)? {
        Datum::String(value) => value,
        _ => unreachable!(),
    };
    match mode {
        TemporalMode::Direct => Ok(raw),
        TemporalMode::AsDate => {
            let parsed = NaiveDate::parse_from_str(&raw, "%Y-%m-%d")
                .or_else(|_| parse_datetime(&raw).map(|value| value.date()));
            Ok(parsed
                .map(|date| date.format("%Y-%m-%d").to_string())
                .unwrap_or(raw))
        }
        TemporalMode::AsDatetime => Ok(parse_datetime(&raw)
            .map(|value| datetime_to_string(&value))
            .unwrap_or(raw)),
    }
}

/// COALESCE：从左到右求值，遇首个非 NULL 即返回（先铸到聚合类型）；遇 Error 立即失败。
/// COALESCE evaluates left-to-right and stops at the first non-NULL value.
/// Its result is cast to the aggregate type before returning, matching the Go
/// signature construction that casts every argument to the inferred type.
pub fn coalesce(args: &[Datum]) -> Result<Datum, CompareError> {
    let result_type = aggregate_compare_type(args);
    for arg in args {
        match arg {
            Datum::Null => continue,
            Datum::Error(message) => return Err(CompareError::new(message.clone())),
            value => return cast_datum(value, result_type),
        }
    }
    Ok(Datum::Null)
}

/// GREATEST/LEAST 核心：任一侧 NULL 则结果 NULL；时态模式先归一字符串再比。
fn extremum(
    args: &[Datum],
    greatest: bool,
    temporal_mode: TemporalMode,
    collation: Collation,
) -> Result<Datum, CompareError> {
    if args.is_empty() {
        return Err(CompareError::new(
            "GREATEST/LEAST requires at least one argument",
        ));
    }
    // Go 按参数顺序求值：在 NULL 之前遇到的错误必须传播，NULL 之后则不再求值。
    for arg in args {
        match arg {
            Datum::Null => return Ok(Datum::Null),
            Datum::Error(message) => return Err(CompareError::new(message.clone())),
            _ => {}
        }
    }
    if temporal_mode != TemporalMode::Direct {
        let mut selected: Option<String> = None;
        for arg in args {
            let value = normalize_temporal(arg, temporal_mode)?;
            if selected.as_ref().is_none_or(|current| {
                let ordering = value.cmp(current);
                if greatest {
                    ordering.is_gt()
                } else {
                    ordering.is_lt()
                }
            }) {
                selected = Some(value);
            }
        }
        return Ok(Datum::String(selected.unwrap()));
    }

    let compare_type = aggregate_compare_type(args);
    let mut selected = cast_datum(&args[0], compare_type)?;
    for arg in &args[1..] {
        let value = cast_datum(arg, compare_type)?;
        let ordering = compare_non_null(&value, &selected, compare_type, collation)?;
        if (greatest && ordering.is_gt()) || (!greatest && ordering.is_lt()) {
            selected = value;
        }
    }
    Ok(selected)
}

/// 返回参数中的最大值。
pub fn greatest(
    args: &[Datum],
    temporal_mode: TemporalMode,
    collation: Collation,
) -> Result<Datum, CompareError> {
    extremum(args, true, temporal_mode, collation)
}

/// 返回参数中的最小值。
pub fn least(
    args: &[Datum],
    temporal_mode: TemporalMode,
    collation: Collation,
) -> Result<Datum, CompareError> {
    extremum(args, false, temporal_mode, collation)
}

/// MySQL INTERVAL uses a linear scan whenever any boundary is nullable; only
/// an all-not-NULL sorted boundary list takes the binary-search path.
/// INTERVAL(整型)：返回 target 落入的边界下标；target 为 NULL 时返回 -1。
pub fn interval_int(target: Option<IntValue>, boundaries: &[Option<IntValue>]) -> i64 {
    let Some(target) = target else {
        return -1;
    };
    if boundaries.iter().any(Option::is_none) {
        return boundaries
            .iter()
            .position(|boundary| {
                boundary.is_some_and(|value| compare_int_values(target, value).is_lt())
            })
            .unwrap_or(boundaries.len()) as i64;
    }
    boundaries.partition_point(|boundary| {
        let value = boundary.expect("all boundaries were checked as non-NULL");
        !compare_int_values(target, value).is_lt()
    }) as i64
}

/// INTERVAL(实数)：语义同 interval_int。
pub fn interval_real(target: Option<f64>, boundaries: &[Option<f64>]) -> i64 {
    let Some(target) = target else {
        return -1;
    };
    if boundaries.iter().any(Option::is_none) {
        return boundaries
            .iter()
            .position(|boundary| boundary.is_some_and(|value| target < value))
            .unwrap_or(boundaries.len()) as i64;
    }
    boundaries.partition_point(|boundary| boundary.is_none_or(|value| target >= value)) as i64
}

/// JSON 值比较（对齐 Go JSON 比较次序）。
fn compare_json(lhs: &JsonValue, rhs: &JsonValue) -> Ordering {
    fn rank(value: &JsonValue) -> u8 {
        match value {
            JsonValue::Null => 0,
            JsonValue::Bool(_) => 1,
            JsonValue::Number(_) => 2,
            JsonValue::String(_) => 3,
            JsonValue::Array(_) => 4,
            JsonValue::Object(_) => 5,
        }
    }
    let rank_ordering = rank(lhs).cmp(&rank(rhs));
    if !rank_ordering.is_eq() {
        return rank_ordering;
    }
    match (lhs, rhs) {
        (JsonValue::Null, JsonValue::Null) => Ordering::Equal,
        (JsonValue::Bool(left), JsonValue::Bool(right)) => left.cmp(right),
        (JsonValue::Number(left), JsonValue::Number(right)) => left
            .as_f64()
            .partial_cmp(&right.as_f64())
            .unwrap_or(Ordering::Equal),
        (JsonValue::String(left), JsonValue::String(right)) => left.cmp(right),
        (JsonValue::Array(left), JsonValue::Array(right)) => {
            for (left, right) in left.iter().zip(right) {
                let ordering = compare_json(left, right);
                if !ordering.is_eq() {
                    return ordering;
                }
            }
            left.len().cmp(&right.len())
        }
        (JsonValue::Object(left), JsonValue::Object(right)) => {
            let mut left_items: Vec<_> = left.iter().collect();
            let mut right_items: Vec<_> = right.iter().collect();
            left_items.sort_by_key(|(key, _)| *key);
            right_items.sort_by_key(|(key, _)| *key);
            for ((left_key, left_value), (right_key, right_value)) in
                left_items.iter().zip(&right_items)
            {
                let ordering = left_key.cmp(right_key);
                if !ordering.is_eq() {
                    return ordering;
                }
                let ordering = compare_json(left_value, right_value);
                if !ordering.is_eq() {
                    return ordering;
                }
            }
            left_items.len().cmp(&right_items.len())
        }
        _ => unreachable!("equal JSON ranks have matching variants"),
    }
}

/// 向量逐维比较。
fn compare_vector(lhs: &[f32], rhs: &[f32]) -> Ordering {
    for (left, right) in lhs.iter().zip(rhs) {
        let ordering = left.total_cmp(right);
        if !ordering.is_eq() {
            return ordering;
        }
    }
    lhs.len().cmp(&rhs.len())
}

/// 两非 NULL Datum 在指定 CompareType 下比较。
fn compare_non_null(
    lhs: &Datum,
    rhs: &Datum,
    compare_type: CompareType,
    collation: Collation,
) -> Result<Ordering, CompareError> {
    match (compare_type, lhs, rhs) {
        (CompareType::Int, Datum::Int(left), Datum::Int(right)) => Ok(left.cmp(right)),
        (CompareType::Int, Datum::UInt(left), Datum::UInt(right)) => Ok(left.cmp(right)),
        (CompareType::Int, Datum::Int(left), Datum::UInt(right)) => Ok(compare_int_values(
            IntValue::Signed(*left),
            IntValue::Unsigned(*right),
        )),
        (CompareType::Int, Datum::UInt(left), Datum::Int(right)) => Ok(compare_int_values(
            IntValue::Unsigned(*left),
            IntValue::Signed(*right),
        )),
        (CompareType::Real, Datum::Real(left), Datum::Real(right)) => {
            Ok(left.partial_cmp(right).unwrap_or(Ordering::Equal))
        }
        (CompareType::Decimal, Datum::Decimal(left), Datum::Decimal(right)) => Ok(left.cmp(right)),
        (CompareType::String, Datum::String(left), Datum::String(right)) => {
            Ok(compare_strings(left, right, collation))
        }
        (CompareType::Time, Datum::DateTime(left), Datum::DateTime(right)) => Ok(left.cmp(right)),
        (CompareType::Duration, Datum::Duration(left), Datum::Duration(right)) => {
            Ok(left.cmp(right))
        }
        (CompareType::Json, Datum::Json(left), Datum::Json(right)) => Ok(compare_json(left, right)),
        (CompareType::VectorFloat32, Datum::VectorFloat32(left), Datum::VectorFloat32(right)) => {
            Ok(compare_vector(left, right))
        }
        _ => Err(CompareError::new(format!(
            "values do not match requested comparison type {compare_type:?}"
        ))),
    }
}

/// Evaluates the complete Go operator matrix. Ordinary comparisons return
/// `None` for SQL NULL. `<=>` always returns a non-NULL boolean.
/// 比较两 Datum：普通算子遇 NULL 得 None；NullEq 将两 NULL 视为相等。
pub fn compare_datums(
    lhs: &Datum,
    rhs: &Datum,
    compare_type: CompareType,
    op: Op,
    collation: Collation,
) -> Result<Option<bool>, CompareError> {
    if let Datum::Error(message) = lhs {
        return Err(CompareError::new(message.clone()));
    }
    if let Datum::Error(message) = rhs {
        return Err(CompareError::new(message.clone()));
    }
    let lhs_is_null = matches!(lhs, Datum::Null);
    let rhs_is_null = matches!(rhs, Datum::Null);
    if lhs_is_null || rhs_is_null {
        let value = compare_null(lhs_is_null, rhs_is_null);
        return if op == Op::NullEq {
            Ok(Some(value == 0))
        } else {
            Ok(None)
        };
    }

    let lhs = cast_datum(lhs, compare_type)?;
    let rhs = cast_datum(rhs, compare_type)?;
    let ordering = compare_non_null(&lhs, &rhs, compare_type, collation)?;
    let result = match op {
        Op::Lt => ordering.is_lt(),
        Op::Le => !ordering.is_gt(),
        Op::Gt => ordering.is_gt(),
        Op::Ge => !ordering.is_lt(),
        Op::Eq | Op::NullEq => ordering.is_eq(),
        Op::Ne => !ordering.is_eq(),
    };
    Ok(Some(result))
}

#[derive(Clone, Debug, PartialEq)]
/// 精化后的比较常量形态。
pub enum RefinedConstant {
    Null,
    Int(i64),
    UInt(u64),
    Real(f64),
    Decimal(Decimal),
    String(String),
}

/// 将精化常量视为 Decimal（若可能）。
fn constant_decimal(value: &RefinedConstant) -> Option<Decimal> {
    match value {
        RefinedConstant::Null => None,
        RefinedConstant::Int(value) => Some(Decimal::from(*value)),
        RefinedConstant::UInt(value) => Some(Decimal::from(*value)),
        RefinedConstant::Real(value) => Decimal::from_f64_retain(*value),
        RefinedConstant::Decimal(value) => Some(*value),
        RefinedConstant::String(value) => Some(value.parse().unwrap_or(Decimal::ZERO)),
    }
}

/// YEAR 两位数扩展（如 2→2002）。
fn adjust_year(year: i64) -> Option<i64> {
    let adjusted = match year {
        0 => 0,
        1..=69 => year + 2000,
        70..=99 => year + 1900,
        value => value,
    };
    (adjusted == 0 || (1901..=2155).contains(&adjusted)).then_some(adjusted)
}

/// 将 Decimal 转为目标整型常量。
fn convert_integral(decimal: Decimal, target: &FieldType) -> Option<RefinedConstant> {
    if target.tp == MysqlType::Year {
        return decimal
            .to_i64()
            .and_then(adjust_year)
            .map(RefinedConstant::Int);
    }
    if target.unsigned {
        decimal.to_u64().map(RefinedConstant::UInt)
    } else {
        decimal.to_i64().map(RefinedConstant::Int)
    }
}

/// Refines a non-integer constant against an integer expression exactly as the
/// Go range-building path: LT/GE use ceil, LE/GT use floor, while fractional
/// EQ/NULL-safe-EQ constants are marked exceptional (always false).
/// 列与常量比较前的常量精化：按算子对小数 ceil/floor；Eq 可保留 Real 例外。
pub fn refine_compared_constant(
    target: &FieldType,
    constant: RefinedConstant,
    op: Op,
) -> (RefinedConstant, bool) {
    if constant == RefinedConstant::Null {
        return (constant, false);
    }
    let Some(decimal) = constant_decimal(&constant) else {
        return (constant, false);
    };
    let truncated = decimal.trunc();
    if decimal == truncated {
        return convert_integral(truncated, target)
            .map(|value| (value, false))
            .unwrap_or_else(|| {
                let infinity = if decimal.is_sign_negative() {
                    if target.unsigned {
                        RefinedConstant::UInt(0)
                    } else {
                        RefinedConstant::Int(i64::MIN)
                    }
                } else if target.unsigned {
                    RefinedConstant::UInt(u64::MAX)
                } else {
                    RefinedConstant::Int(i64::MAX)
                };
                (infinity, true)
            });
    }

    match op {
        Op::Lt | Op::Ge => convert_integral(decimal.ceil(), target)
            .map(|value| (value, false))
            .unwrap_or((constant, true)),
        Op::Le | Op::Gt => convert_integral(decimal.floor(), target)
            .map(|value| (value, false))
            .unwrap_or((constant, true)),
        Op::Eq | Op::NullEq => (constant, true),
        Op::Ne => (constant, false),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 无符号列与常量比较的折叠结果。
pub enum UnsignedRefinement {
    Keep,
    Always(bool),
}

/// Result-level form of Go's `refineArgsByUnsignedFlag` for an unsigned column
/// on the left and a signed constant on the right. Nullable columns are never
/// folded because SQL NULL must remain observable.
/// NOT NULL 无符号列对负常量可折叠为恒真/恒假；可空列则 Keep。
pub fn refine_unsigned_comparison(
    column_type: &FieldType,
    constant: IntValue,
    op: Op,
) -> UnsignedRefinement {
    if !column_type.unsigned || !column_type.not_null {
        return UnsignedRefinement::Keep;
    }
    let IntValue::Signed(value) = constant else {
        return UnsignedRefinement::Keep;
    };
    if value > 0 {
        return UnsignedRefinement::Keep;
    }
    if value == 0 && matches!(op, Op::Le | Op::Gt | Op::NullEq | Op::Eq | Op::Ne) {
        return UnsignedRefinement::Keep;
    }
    let result = match op {
        Op::Lt | Op::Le | Op::Eq | Op::NullEq => false,
        Op::Gt | Op::Ge | Op::Ne => true,
    };
    UnsignedRefinement::Always(result)
}

/// Kept public for callers that need the Go helper's three-way result rather
/// than a boolean operator result.
/// 导出 NULL 次序比较。
pub fn compare_null_order(lhs_is_null: bool, rhs_is_null: bool) -> i64 {
    compare_null(lhs_is_null, rhs_is_null)
}

/// 导出 Ordering→-1/0/1。
pub fn compare_ordering_value(ordering: Ordering) -> i64 {
    ordering_value(ordering)
}

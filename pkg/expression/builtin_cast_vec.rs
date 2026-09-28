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

// Native vectorized CAST behavior corresponding to `builtin_cast_vec.go`.
//
// Go's expression layer stores each physical type in a `chunk.Column`.  The
// Rust port uses `Vec<Option<ScalarValue>>`: `Option` is the NULL bitmap and
// the vector owns its values, so temporary column pooling and `defer`-based
// returns are unnecessary.  The 49 source/target combinations and their
// per-row control flow remain explicit below.

//
// 向量化 CAST：对列中每一行执行与 `builtin_cast_vec.go` 相同的 49 种源/目标组合。
// `Vec<Option<ScalarValue>>` 用 Option 表示 NULL 位图；非法值在 warning 模式下记入警告并产出 NULL。

use std::fmt;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use rust_decimal::prelude::ToPrimitive;

use crate::mysql;
use types_dependency::datum::{
    BasicTimeContext, BinaryJSON, Context as TypeContext, CoreTime, CreateBinaryJSON, Duration,
    FieldType, FromDate, IsBinaryStr, JsonDuration, JsonTime, MaxFsp, ModeHalfUp, MyDecimal,
    NewFieldType, ParseBinaryJSONFromString, ParseDuration, ParseTime, ParseTimeFromFloatString,
    ParseTimeFromNum, ProduceDecWithSpecifiedTp, ProduceFloatWithSpecifiedTp,
    ProduceStrWithSpecifiedTp, StrToFloat, StrToInt, StrToUint, Time,
};
use types_dependency::json_binary::{
    JSONLiteralFalse, JSONTypeCodeDate, JSONTypeCodeDatetime, JSONTypeCodeDuration,
    JSONTypeCodeFloat64, JSONTypeCodeInt64, JSONTypeCodeLiteral, JSONTypeCodeString,
    JSONTypeCodeTimestamp, JSONTypeCodeUint64, Opaque,
};
use types_dependency::time::ParseTimeFromYear;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
/// 向量化求值类型（对应 Go 的 types.EvalType 子集）。
pub enum EvalKind {
    Int,
    Real,
    Decimal,
    String,
    Time,
    Duration,
    Json,
}

#[derive(Clone, Debug, PartialEq)]
/// 单行标量值，与 EvalKind 一一对应。
pub enum ScalarValue {
    Int(i64),
    Real(f64),
    Decimal(MyDecimal),
    String(String),
    Time(Time),
    Duration(Duration),
    Json(BinaryJSON),
}

impl ScalarValue {
    /// 返回该标量所属的求值类型。
    pub fn kind(&self) -> EvalKind {
        match self {
            Self::Int(_) => EvalKind::Int,
            Self::Real(_) => EvalKind::Real,
            Self::Decimal(_) => EvalKind::Decimal,
            Self::String(_) => EvalKind::String,
            Self::Time(_) => EvalKind::Time,
            Self::Duration(_) => EvalKind::Duration,
            Self::Json(_) => EvalKind::Json,
        }
    }

    /// 若为 Time 则取出，否则 None。
    pub fn as_time(&self) -> Option<Time> {
        match self {
            Self::Time(value) => Some(*value),
            _ => None,
        }
    }
    /// 若为 Duration 则取出，否则 None。
    pub fn as_duration(&self) -> Option<Duration> {
        match self {
            Self::Duration(value) => Some(*value),
            _ => None,
        }
    }
    /// 若为 Decimal 则取出引用，否则 None。
    pub fn as_decimal(&self) -> Option<&MyDecimal> {
        match self {
            Self::Decimal(value) => Some(value),
            _ => None,
        }
    }
    /// 若为 String 则取出引用，否则 None。
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
    /// 若为 Json 则取出引用，否则 None。
    pub fn as_json(&self) -> Option<&BinaryJSON> {
        match self {
            Self::Json(value) => Some(value),
            _ => None,
        }
    }

    /// 将标量格式化为便于断言的数值/文本字符串。
    pub fn numeric_string(&self) -> String {
        match self {
            Self::Int(value) => value.to_string(),
            Self::Real(value) => value.to_string(),
            Self::Decimal(value) => value.String(),
            Self::Time(value) => value.ToNumber().normalize().to_string(),
            Self::Duration(value) => value.ToNumber().normalize().to_string(),
            Self::String(value) => value.clone(),
            Self::Json(value) => value.String(),
        }
    }
}

#[derive(Clone, Debug)]
/// CAST 规格：源/目标类型及无符号、UNION、布尔、YEAR、parse_to_json 等标志。
pub struct CastSpec {
    pub source: EvalKind,
    pub target_kind: EvalKind,
    pub target: FieldType,
    pub in_union: bool,
    pub target_unsigned: bool,
    pub source_unsigned: bool,
    pub source_boolean: bool,
    pub source_year: bool,
    pub source_float32: bool,
    pub source_binary: bool,
    pub source_binary_type: u8,
    pub source_flen: isize,
    pub parse_to_json: bool,
}

impl CastSpec {
    /// 构造默认 FieldType 的 CAST 规格。
    pub fn new(source: EvalKind, target: EvalKind) -> Self {
        let target_type = match target {
            EvalKind::Int => mysql::TypeLonglong,
            EvalKind::Real => mysql::TypeDouble,
            EvalKind::Decimal => mysql::TypeNewDecimal,
            EvalKind::String => mysql::TypeVarString,
            EvalKind::Time => mysql::TypeDatetime,
            EvalKind::Duration => mysql::TypeDuration,
            EvalKind::Json => mysql::TypeJSON,
        };
        Self {
            source,
            target_kind: target,
            target: *NewFieldType(target_type),
            in_union: false,
            target_unsigned: false,
            source_unsigned: false,
            source_boolean: false,
            source_year: false,
            source_float32: false,
            source_binary: false,
            source_binary_type: mysql::TypeVarString,
            source_flen: -1,
            parse_to_json: false,
        }
    }

    /// 目标为带精度/标度的 DECIMAL。
    pub fn decimal(source: EvalKind, precision: isize, scale: isize) -> Self {
        let mut result = Self::new(source, EvalKind::Decimal);
        result.target.SetFlen(precision);
        result.target.SetDecimal(scale);
        result
    }

    /// 目标为带 flen 的字符串。
    pub fn string(source: EvalKind, flen: isize) -> Self {
        let mut result = Self::new(source, EvalKind::String);
        result.target.SetFlen(flen);
        result
    }

    /// 目标为带小数秒精度（fsp）的日期时间。
    pub fn datetime(source: EvalKind, fsp: isize) -> Self {
        let mut result = Self::new(source, EvalKind::Time);
        result.target.SetDecimal(fsp);
        result
    }

    /// 目标为带 fsp 的时长。
    pub fn duration(source: EvalKind, fsp: isize) -> Self {
        let mut result = Self::new(source, EvalKind::Duration);
        result.target.SetDecimal(fsp);
        result
    }

    /// 覆盖目标 MySQL 类型码（如 DATE vs DATETIME）。
    pub fn target_type(mut self, tp: u8) -> Self {
        self.target.SetType(tp);
        self
    }
}

#[derive(Clone)]
/// CAST 求值上下文：类型/时间子上下文、非法值策略、警告列表与“当前时间”。
pub struct CastContext {
    type_context: TypeContext,
    time_context: BasicTimeContext,
    invalid_as_null: bool,
    warnings: Vec<String>,
    now: DateTime<Tz>,
}

impl CastContext {
    /// warning 模式：非法值记警告并转为 NULL。
    pub fn warning() -> Self {
        let type_context = types_dependency::scalar::DefaultStmtNoWarningContext.clone();
        let time_context = types_dependency::datum::time_context(&type_context);
        Self {
            type_context,
            time_context,
            invalid_as_null: true,
            warnings: Vec::new(),
            now: Utc::now().with_timezone(&chrono_tz::UTC),
        }
    }

    /// 严格模式：非法值直接返回错误。
    pub fn strict() -> Self {
        let mut result = Self::warning();
        result.invalid_as_null = false;
        result
    }

    /// 指定求值所用的“当前时间”。
    pub fn with_now(mut self, now: DateTime<Tz>) -> Self {
        self.now = now;
        self
    }

    /// 已累计的警告消息。
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// 按 invalid_as_null 将错误转为 NULL+警告或 CastError。
    fn invalid(&mut self, error: impl ToString) -> Result<Option<ScalarValue>, CastError> {
        let message = error.to_string();
        if self.invalid_as_null {
            self.warnings.push(message);
            Ok(None)
        } else {
            Err(CastError(message))
        }
    }
}

impl Default for CastContext {
    fn default() -> Self {
        Self::warning()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// CAST 失败错误。
pub struct CastError(pub String);

impl fmt::Display for CastError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for CastError {}

/// 将任意错误信息包装为 CastError。
fn error(error: impl ToString) -> CastError {
    CastError(error.to_string())
}

/// Go 向量化 CAST 支持的全部 (源, 目标) 签名。
const SUPPORTED_CASTS: [(EvalKind, EvalKind); 49] = [
    (EvalKind::Int, EvalKind::Duration),
    (EvalKind::Int, EvalKind::Int),
    (EvalKind::Int, EvalKind::Real),
    (EvalKind::Real, EvalKind::Real),
    (EvalKind::Time, EvalKind::Json),
    (EvalKind::Real, EvalKind::String),
    (EvalKind::Decimal, EvalKind::String),
    (EvalKind::Time, EvalKind::Decimal),
    (EvalKind::Duration, EvalKind::Int),
    (EvalKind::Int, EvalKind::Time),
    (EvalKind::Real, EvalKind::Json),
    (EvalKind::Json, EvalKind::Real),
    (EvalKind::Json, EvalKind::Time),
    (EvalKind::Real, EvalKind::Time),
    (EvalKind::Decimal, EvalKind::Decimal),
    (EvalKind::Duration, EvalKind::Time),
    (EvalKind::Int, EvalKind::String),
    (EvalKind::Real, EvalKind::Int),
    (EvalKind::Time, EvalKind::Real),
    (EvalKind::String, EvalKind::Json),
    (EvalKind::Real, EvalKind::Decimal),
    (EvalKind::String, EvalKind::Int),
    (EvalKind::String, EvalKind::Duration),
    (EvalKind::Duration, EvalKind::Decimal),
    (EvalKind::Int, EvalKind::Decimal),
    (EvalKind::Int, EvalKind::Json),
    (EvalKind::Json, EvalKind::Json),
    (EvalKind::Json, EvalKind::String),
    (EvalKind::Duration, EvalKind::Real),
    (EvalKind::Json, EvalKind::Int),
    (EvalKind::Real, EvalKind::Duration),
    (EvalKind::Time, EvalKind::Duration),
    (EvalKind::Duration, EvalKind::Duration),
    (EvalKind::Duration, EvalKind::String),
    (EvalKind::Decimal, EvalKind::Real),
    (EvalKind::Decimal, EvalKind::Time),
    (EvalKind::Time, EvalKind::Int),
    (EvalKind::Time, EvalKind::Time),
    (EvalKind::Time, EvalKind::String),
    (EvalKind::Json, EvalKind::Decimal),
    (EvalKind::String, EvalKind::Real),
    (EvalKind::String, EvalKind::Decimal),
    (EvalKind::String, EvalKind::Time),
    (EvalKind::Decimal, EvalKind::Int),
    (EvalKind::Decimal, EvalKind::Duration),
    (EvalKind::String, EvalKind::String),
    (EvalKind::Json, EvalKind::Duration),
    (EvalKind::Decimal, EvalKind::Json),
    (EvalKind::Duration, EvalKind::Json),
];

/// 返回支持的向量化 CAST 签名表。
pub fn supported_casts() -> &'static [(EvalKind, EvalKind)] {
    &SUPPORTED_CASTS
}

/// 对整列执行 CAST：校验签名与源类型，NULL 行直接传播。
pub fn cast_column(
    ctx: &mut CastContext,
    spec: &CastSpec,
    input: &[Option<ScalarValue>],
) -> Result<Vec<Option<ScalarValue>>, CastError> {
    // 仅允许 Go 已向量化的 49 种签名。
    if !SUPPORTED_CASTS.contains(&(spec.source, spec.target_kind)) {
        return Err(CastError(format!(
            "unsupported cast {:?} -> {:?}",
            spec.source, spec.target_kind
        )));
    }
    let mut output = Vec::with_capacity(input.len());
    for item in input {
        match item {
            // NULL 位图：空输入直接产出 NULL，不调用 cast_one。
            None => output.push(None),
            Some(value) if value.kind() != spec.source => {
                return Err(CastError(format!(
                    "source column declared {:?}, found {:?}",
                    spec.source,
                    value.kind()
                )));
            }
            Some(value) => output.push(cast_one(ctx, spec, value)?),
        }
    }
    Ok(output)
}

/// 单行按源标量种类分派到各 cast_* 实现。
fn cast_one(
    ctx: &mut CastContext,
    spec: &CastSpec,
    value: &ScalarValue,
) -> Result<Option<ScalarValue>, CastError> {
    match (value, spec.target_kind) {
        (ScalarValue::Int(value), target) => cast_int(ctx, spec, *value, target),
        (ScalarValue::Real(value), target) => cast_real(ctx, spec, *value, target),
        (ScalarValue::Decimal(value), target) => cast_decimal(ctx, spec, value, target),
        (ScalarValue::String(value), target) => cast_string(ctx, spec, value, target),
        (ScalarValue::Time(value), target) => cast_time(ctx, spec, *value, target),
        (ScalarValue::Duration(value), target) => cast_duration(ctx, spec, *value, target),
        (ScalarValue::Json(value), target) => cast_json(ctx, spec, value, target),
    }
}

/// 按 target_unsigned 合成目标 FieldType 标志位。
fn target_field(spec: &CastSpec) -> FieldType {
    let mut target = spec.target.clone();
    let mut flags = target.GetFlag();
    if spec.target_unsigned {
        flags |= mysql::UnsignedFlag;
    } else {
        flags &= !mysql::UnsignedFlag;
    }
    target.SetFlag(flags);
    target
}

/// 取出并钳制目标小数秒精度。
fn target_fsp(spec: &CastSpec) -> i32 {
    spec.target.GetDecimal().clamp(0, MaxFsp as isize) as i32
}

/// 从字符串解析 MyDecimal。
fn decimal_from_str(value: &str) -> Result<MyDecimal, CastError> {
    let mut decimal = MyDecimal::default();
    decimal.FromString(value.as_bytes()).map_err(error)?;
    Ok(decimal)
}

/// 按目标精度产出 DECIMAL（对齐 ProduceDecWithSpecifiedTp）。
fn produce_decimal(
    ctx: &CastContext,
    spec: &CastSpec,
    value: MyDecimal,
) -> Result<MyDecimal, CastError> {
    let target = target_field(spec);
    let (result, failure) = ProduceDecWithSpecifiedTp(ctx.type_context.clone(), value, &target);
    if let Some(failure) = failure {
        return Err(error(failure));
    }
    result.ok_or_else(|| CastError("decimal conversion returned no value".into()))
}

/// 按目标 flen/字符集产出字符串。
fn produce_string(
    ctx: &CastContext,
    spec: &CastSpec,
    value: String,
    pad_binary: bool,
) -> Result<String, CastError> {
    let target = target_field(spec);
    let (mut result, failure) =
        ProduceStrWithSpecifiedTp(value, &target, ctx.type_context.clone(), false);
    if let Some(failure) = failure {
        return Err(error(failure));
    }
    if pad_binary
        && target.GetType() == mysql::TypeString
        && IsBinaryStr(&target)
        && target.GetFlen() > result.len() as isize
    {
        result.push_str(&"\0".repeat(target.GetFlen() as usize - result.len()));
    }
    Ok(result)
}

/// 按目标浮点宽度产出 Real。
fn produce_real(spec: &CastSpec, value: f64) -> Result<f64, CastError> {
    let target = target_field(spec);
    let (result, failure) = ProduceFloatWithSpecifiedTp(value, &target);
    if let Some(failure) = failure {
        return Err(error(failure));
    }
    Ok(result)
}

/// 解析时长文本；截断或非法时按上下文策略处理。
fn parse_duration_value(
    ctx: &mut CastContext,
    text: &str,
    fsp: i32,
) -> Result<Option<ScalarValue>, CastError> {
    match ParseDuration(&ctx.time_context, text, fsp) {
        Ok((_, true)) => Ok(None),
        Ok((duration, false)) => Ok(Some(ScalarValue::Duration(duration))),
        Err(failure) => ctx.invalid(failure),
    }
}

/// 解析时间文本（可走浮点字符串路径）；DATE 目标清零时分秒。
fn parse_time_value(
    ctx: &mut CastContext,
    spec: &CastSpec,
    text: &str,
    float: bool,
) -> Result<Option<ScalarValue>, CastError> {
    let tp = spec.target.GetType();
    let fsp = target_fsp(spec);
    let parsed = if float {
        ParseTimeFromFloatString(&ctx.time_context, text, tp, fsp)
    } else {
        ParseTime(&ctx.time_context, text, tp, fsp)
    };
    match parsed {
        Ok(mut value) => {
            if tp == mysql::TypeDate {
                value.SetCoreTime(FromDate(
                    value.Year(),
                    value.Month(),
                    value.Day(),
                    0,
                    0,
                    0,
                    0,
                ));
                value.SetType(tp);
            }
            Ok(Some(ScalarValue::Time(value)))
        }
        Err(failure) => ctx.invalid(failure),
    }
}

/// Real→Time：将浮点格式化为 Go 可解析的日期时间数字串（短整数左侧补零）。
fn real_time_text(value: f64) -> String {
    let text = value.to_string();
    let (integer, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let normalized =
            // 3/4 位整数按 Go 规则左侧补零到 6 位，便于解析为 YYMMDD。
        if matches!(integer.len(), 3 | 4) && integer.bytes().all(|byte| byte.is_ascii_digit()) {
            format!("{integer:0>6}")
        } else {
            integer.to_owned()
        };
    if fraction.is_empty() {
        normalized
    } else {
        format!("{normalized}.{fraction}")
    }
}

/// Int 源到各目标类型的 CAST（含 UNION 无符号钳制、YEAR、布尔 JSON）。
fn cast_int(
    ctx: &mut CastContext,
    spec: &CastSpec,
    value: i64,
    target: EvalKind,
) -> Result<Option<ScalarValue>, CastError> {
    let result = match target {
        // UNION 无符号目标：负数钳为 0，对齐 Go inUnion 语义。
        EvalKind::Int => ScalarValue::Int(if spec.in_union && spec.target_unsigned && value < 0 {
            0
        } else {
            value
        }),
        EvalKind::Real => {
            let value = if !spec.target_unsigned && !spec.source_unsigned {
                value as f64
            } else if spec.in_union && !spec.source_unsigned && value < 0 {
                0.0
            } else {
                (value as u64) as f64
            };
            ScalarValue::Real(value)
        }
        EvalKind::String => {
            let mut text = if spec.source_unsigned {
                (value as u64).to_string()
            } else {
                value.to_string()
            };
            if spec.source_year && text == "0" {
                text = "0000".into();
            }
            ScalarValue::String(produce_string(ctx, spec, text, true)?)
        }
        EvalKind::Decimal => {
            let mut decimal = MyDecimal::default();
            if !spec.target_unsigned && !spec.source_unsigned {
                decimal.FromInt(value);
            } else if spec.in_union && !spec.source_unsigned && value < 0 {
                decimal.FromUint(0);
            } else {
                decimal.FromUint(value as u64);
            }
            ScalarValue::Decimal(produce_decimal(ctx, spec, decimal)?)
        }
        EvalKind::Json => {
            let json = if spec.source_boolean {
                CreateBinaryJSON(value != 0)
            } else if spec.source_unsigned || spec.source_year {
                CreateBinaryJSON(value as u64)
            } else {
                CreateBinaryJSON(value)
            };
            ScalarValue::Json(json)
        }
        EvalKind::Duration => {
            return parse_duration_value(ctx, &value.to_string(), target_fsp(spec));
        }
        EvalKind::Time => {
            let parsed = if spec.source_year {
                ParseTimeFromYear(value)
            } else {
                ParseTimeFromNum(
                    &ctx.time_context,
                    value,
                    spec.target.GetType(),
                    target_fsp(spec),
                )
            };
            return match parsed {
                Ok(mut value) => {
                    if spec.target.GetType() == mysql::TypeDate {
                        value.SetCoreTime(FromDate(
                            value.Year(),
                            value.Month(),
                            value.Day(),
                            0,
                            0,
                            0,
                            0,
                        ));
                    }
                    Ok(Some(ScalarValue::Time(value)))
                }
                Err(failure) => ctx.invalid(failure),
            };
        }
    };
    Ok(Some(result))
}

/// Real 源到各目标类型的 CAST。
fn cast_real(
    ctx: &mut CastContext,
    spec: &CastSpec,
    value: f64,
    target: EvalKind,
) -> Result<Option<ScalarValue>, CastError> {
    let result = match target {
        EvalKind::Real => {
            let value = if spec.in_union && spec.target_unsigned && value < 0.0 {
                0.0
            } else {
                value
            };
            ScalarValue::Real(produce_real(spec, value)?)
        }
        EvalKind::String => {
            let text = if spec.source_float32 {
                (value as f32).to_string()
            } else {
                value.to_string()
            };
            ScalarValue::String(produce_string(ctx, spec, text, true)?)
        }
        EvalKind::Json => ScalarValue::Json(CreateBinaryJSON(value)),
        EvalKind::Time => {
            if value == 0.0 {
                let mut zero = types_dependency::time::ZeroTime;
                zero.SetType(spec.target.GetType());
                zero.SetFsp(target_fsp(spec));
                return Ok(Some(ScalarValue::Time(zero)));
            }
            return parse_time_value(ctx, spec, &real_time_text(value), true);
        }
        EvalKind::Int => {
            if spec.target_unsigned {
                if spec.in_union && value < 0.0 {
                    ScalarValue::Int(0)
                } else {
                    let converted = types_dependency::scalar::ConvertFloatToUint(
                        ctx.type_context.Flags(),
                        value,
                        u64::MAX,
                        mysql::TypeLonglong,
                    )
                    .map_err(error)?;
                    ScalarValue::Int(converted as i64)
                }
            } else {
                ScalarValue::Int(
                    types_dependency::scalar::ConvertFloatToInt(
                        value,
                        i64::MIN,
                        i64::MAX,
                        mysql::TypeLonglong,
                    )
                    .map_err(error)?,
                )
            }
        }
        EvalKind::Decimal => {
            let mut decimal = MyDecimal::default();
            if !(spec.in_union && spec.target_unsigned && value < 0.0) {
                decimal.FromFloat64(value).map_err(error)?;
            }
            ScalarValue::Decimal(produce_decimal(ctx, spec, decimal)?)
        }
        EvalKind::Duration => {
            return parse_duration_value(ctx, &value.to_string(), target_fsp(spec));
        }
    };
    Ok(Some(result))
}

/// Decimal 源到各目标类型的 CAST。
fn cast_decimal(
    ctx: &mut CastContext,
    spec: &CastSpec,
    value: &MyDecimal,
    target: EvalKind,
) -> Result<Option<ScalarValue>, CastError> {
    let result = match target {
        EvalKind::String => ScalarValue::String(produce_string(ctx, spec, value.String(), true)?),
        EvalKind::Decimal => {
            let decimal = if spec.in_union && spec.target_unsigned && value.IsNegative() {
                MyDecimal::default()
            } else {
                value.clone()
            };
            ScalarValue::Decimal(produce_decimal(ctx, spec, decimal)?)
        }
        EvalKind::Real => {
            let real = if spec.in_union && spec.target_unsigned && value.IsNegative() {
                0.0
            } else {
                value.ToFloat64().map_err(error)?
            };
            ScalarValue::Real(real)
        }
        EvalKind::Time => return parse_time_value(ctx, spec, &value.String(), true),
        EvalKind::Int => {
            let mut rounded = MyDecimal::default();
            value.Round(&mut rounded, 0, ModeHalfUp).map_err(error)?;
            if spec.target_unsigned {
                if spec.in_union && rounded.IsNegative() {
                    ScalarValue::Int(0)
                } else {
                    let (converted, status) = rounded.ToUint();
                    status.map_err(error)?;
                    ScalarValue::Int(converted as i64)
                }
            } else {
                let (converted, status) = rounded.ToInt();
                status.map_err(error)?;
                ScalarValue::Int(converted)
            }
        }
        EvalKind::Duration => return parse_duration_value(ctx, &value.String(), target_fsp(spec)),
        EvalKind::Json => ScalarValue::Json(CreateBinaryJSON(value.ToFloat64().map_err(error)?)),
    };
    Ok(Some(result))
}

/// String 源到各目标类型的 CAST（含 parse_to_json）。
fn cast_string(
    ctx: &mut CastContext,
    spec: &CastSpec,
    value: &str,
    target: EvalKind,
) -> Result<Option<ScalarValue>, CastError> {
    let result = match target {
        EvalKind::Json => {
            let json = if spec.source_binary {
                let mut bytes = value.as_bytes().to_vec();
                if spec.source_binary_type == mysql::TypeString && spec.source_flen > 0 {
                    bytes.resize(spec.source_flen as usize, 0);
                }
                CreateBinaryJSON(Opaque {
                    TypeCode: spec.source_binary_type,
                    Buf: bytes,
                })
            } else if spec.parse_to_json {
                ParseBinaryJSONFromString(value).map_err(error)?
            } else {
                CreateBinaryJSON(value)
            };
            ScalarValue::Json(json)
        }
        EvalKind::Int => {
            let text = value.trim();
            let negative = text.len() > 1 && text.starts_with('-');
            let converted = if negative && spec.in_union && spec.target_unsigned {
                0
            } else if negative {
                StrToInt(ctx.type_context.clone(), text, true).map_err(error)?
            } else {
                StrToUint(ctx.type_context.clone(), text, true).map_err(error)? as i64
            };
            ScalarValue::Int(converted)
        }
        EvalKind::Duration => return parse_duration_value(ctx, value, target_fsp(spec)),
        EvalKind::Real => {
            let mut converted = StrToFloat(ctx.type_context.clone(), value, true).map_err(error)?;
            if spec.in_union && spec.target_unsigned && converted < 0.0 {
                converted = 0.0;
            }
            ScalarValue::Real(produce_real(spec, converted)?)
        }
        EvalKind::Decimal => {
            let text = value.trim();
            let decimal = if spec.in_union && spec.target_unsigned && text.starts_with('-') {
                MyDecimal::default()
            } else {
                decimal_from_str(text)?
            };
            ScalarValue::Decimal(produce_decimal(ctx, spec, decimal)?)
        }
        EvalKind::Time => return parse_time_value(ctx, spec, value, false),
        EvalKind::String => ScalarValue::String(produce_string(ctx, spec, value.to_owned(), true)?),
    };
    Ok(Some(result))
}

/// 将 Time 编码为 BinaryJSON。
fn json_from_time(value: Time) -> BinaryJSON {
    let type_code = match value.Type() {
        mysql::TypeDate => types_dependency::datum::JSONTypeCodeDate,
        mysql::TypeTimestamp => types_dependency::datum::JSONTypeCodeTimestamp,
        _ => types_dependency::datum::JSONTypeCodeDatetime,
    };
    CreateBinaryJSON(JsonTime {
        CoreTime: value.coreTime.0,
        TypeCode: type_code,
        Fsp: value.Fsp() as u8,
    })
}

/// Time 源到各目标类型的 CAST。
fn cast_time(
    ctx: &mut CastContext,
    spec: &CastSpec,
    value: Time,
    target: EvalKind,
) -> Result<Option<ScalarValue>, CastError> {
    let result = match target {
        EvalKind::Json => {
            let mut value = value;
            if matches!(value.Type(), mysql::TypeDatetime | mysql::TypeTimestamp) {
                value.SetFsp(MaxFsp);
            }
            ScalarValue::Json(json_from_time(value))
        }
        EvalKind::Decimal => ScalarValue::Decimal(produce_decimal(
            ctx,
            spec,
            decimal_from_str(&value.ToNumber().to_string())?,
        )?),
        EvalKind::Real => ScalarValue::Real(
            value
                .ToNumber()
                .to_f64()
                .ok_or_else(|| CastError("time number overflow".into()))?,
        ),
        EvalKind::Duration => {
            let duration = value
                .ConvertToDuration()
                .map_err(error)?
                .RoundFrac(target_fsp(spec), ctx.type_context.Location())
                .map_err(error)?;
            ScalarValue::Duration(duration)
        }
        EvalKind::Int => {
            let rounded = value.RoundFrac(&ctx.time_context, 0).map_err(error)?;
            ScalarValue::Int(
                rounded
                    .ToNumber()
                    .to_i64()
                    .ok_or_else(|| CastError("time integer overflow".into()))?,
            )
        }
        EvalKind::Time => {
            let mut converted = match value.Convert(&ctx.time_context, spec.target.GetType()) {
                Ok(value) => value,
                Err(failure) => return ctx.invalid(failure),
            };
            converted = converted
                .RoundFrac(&ctx.time_context, target_fsp(spec))
                .map_err(error)?;
            if spec.target.GetType() == mysql::TypeDate {
                converted.SetCoreTime(FromDate(
                    converted.Year(),
                    converted.Month(),
                    converted.Day(),
                    0,
                    0,
                    0,
                    0,
                ));
                converted.SetType(mysql::TypeDate);
            }
            ScalarValue::Time(converted)
        }
        EvalKind::String => ScalarValue::String(produce_string(ctx, spec, value.String(), true)?),
    };
    Ok(Some(result))
}

/// Duration 源到各目标类型的 CAST。
fn cast_duration(
    ctx: &mut CastContext,
    spec: &CastSpec,
    value: Duration,
    target: EvalKind,
) -> Result<Option<ScalarValue>, CastError> {
    let result = match target {
        EvalKind::Int => {
            let converted = if spec.target.GetType() == mysql::TypeYear {
                value
                    .ConvertToYearFromNow(&ctx.time_context, ctx.now)
                    .map_err(error)?
            } else {
                value
                    .RoundFrac(0, ctx.type_context.Location())
                    .map_err(error)?
                    .ToNumber()
                    .to_i64()
                    .ok_or_else(|| CastError("duration integer overflow".into()))?
            };
            ScalarValue::Int(converted)
        }
        EvalKind::Time => {
            let mut converted = match value.ConvertToTimeWithTimestamp(
                &ctx.time_context,
                spec.target.GetType(),
                ctx.now,
            ) {
                Ok(value) => value,
                Err(failure) => return ctx.invalid(failure),
            };
            converted = converted
                .RoundFrac(&ctx.time_context, target_fsp(spec))
                .map_err(error)?;
            ScalarValue::Time(converted)
        }
        EvalKind::Decimal => ScalarValue::Decimal(produce_decimal(
            ctx,
            spec,
            decimal_from_str(&value.ToNumber().to_string())?,
        )?),
        EvalKind::Real => ScalarValue::Real(
            value
                .ToNumber()
                .to_f64()
                .ok_or_else(|| CastError("duration real overflow".into()))?,
        ),
        EvalKind::Duration => ScalarValue::Duration(
            value
                .RoundFrac(target_fsp(spec), ctx.type_context.Location())
                .map_err(error)?,
        ),
        EvalKind::String => ScalarValue::String(produce_string(ctx, spec, value.String(), true)?),
        EvalKind::Json => ScalarValue::Json(CreateBinaryJSON(JsonDuration {
            Duration: value.Duration,
            Fsp: MaxFsp as u32,
        })),
    };
    Ok(Some(result))
}

/// JSON 取值转为 Real。
fn json_to_real(ctx: &CastContext, value: &BinaryJSON) -> Result<f64, CastError> {
    match value.TypeCode {
        JSONTypeCodeLiteral => Ok(if value.Value.first() == Some(&JSONLiteralFalse) {
            0.0
        } else {
            1.0
        }),
        JSONTypeCodeInt64 => Ok(value.GetInt64() as f64),
        JSONTypeCodeUint64 => Ok(value.GetUint64() as f64),
        JSONTypeCodeFloat64 => Ok(value.GetFloat64()),
        JSONTypeCodeString => {
            let bytes = value.GetString();
            StrToFloat(
                ctx.type_context.clone(),
                &String::from_utf8_lossy(&bytes),
                false,
            )
            .map_err(error)
        }
        _ => Err(CastError(format!(
            "truncated incorrect FLOAT value: {}",
            value.String()
        ))),
    }
}

/// JSON 取值转为 Int（尊重无符号等标志）。
fn json_to_int(ctx: &CastContext, spec: &CastSpec, value: &BinaryJSON) -> Result<i64, CastError> {
    let result = match value.TypeCode {
        JSONTypeCodeLiteral => {
            if value.Value.first() == Some(&JSONLiteralFalse) {
                0
            } else {
                1
            }
        }
        JSONTypeCodeInt64 => value.GetInt64(),
        JSONTypeCodeUint64 => value.GetUint64() as i64,
        JSONTypeCodeFloat64 => {
            if spec.target_unsigned {
                types_dependency::scalar::ConvertFloatToUint(
                    ctx.type_context.Flags(),
                    value.GetFloat64(),
                    u64::MAX,
                    mysql::TypeLonglong,
                )
                .map_err(error)? as i64
            } else {
                types_dependency::scalar::ConvertFloatToInt(
                    value.GetFloat64(),
                    i64::MIN,
                    i64::MAX,
                    mysql::TypeLonglong,
                )
                .map_err(error)?
            }
        }
        JSONTypeCodeString => {
            let bytes = value.GetString();
            let text = String::from_utf8_lossy(&bytes);
            if text.starts_with('-') {
                StrToInt(ctx.type_context.clone(), &text, false).map_err(error)?
            } else {
                StrToUint(ctx.type_context.clone(), &text, false).map_err(error)? as i64
            }
        }
        _ => {
            return Err(CastError(format!(
                "truncated incorrect INTEGER value: {}",
                value.String()
            )));
        }
    };
    Ok(result)
}

/// JSON 取值转为 Decimal。
fn json_to_decimal(value: &BinaryJSON) -> Result<MyDecimal, CastError> {
    match value.TypeCode {
        JSONTypeCodeLiteral => Ok(if value.Value.first() == Some(&JSONLiteralFalse) {
            decimal_from_str("0")?
        } else {
            decimal_from_str("1")?
        }),
        JSONTypeCodeInt64 => {
            let mut result = MyDecimal::default();
            result.FromInt(value.GetInt64());
            Ok(result)
        }
        JSONTypeCodeUint64 => {
            let mut result = MyDecimal::default();
            result.FromUint(value.GetUint64());
            Ok(result)
        }
        JSONTypeCodeFloat64 => {
            let mut result = MyDecimal::default();
            result.FromFloat64(value.GetFloat64()).map_err(error)?;
            Ok(result)
        }
        JSONTypeCodeString => {
            let bytes = value.GetString();
            decimal_from_str(&String::from_utf8_lossy(&bytes))
        }
        _ => Err(CastError(format!(
            "truncated incorrect DECIMAL value: {}",
            value.String()
        ))),
    }
}

/// 从 JSON 构造 Time。
fn time_from_json(value: &BinaryJSON, target_type: u8, fsp: i32) -> Time {
    let json_time = value.GetTimeWithFsp(fsp as u8);
    let mut result = Time {
        coreTime: CoreTime(json_time.CoreTime),
    };
    result.SetType(target_type);
    result.SetFsp(fsp);
    result
}

/// 从 JSON 构造 Duration。
fn duration_from_json(value: &BinaryJSON) -> Duration {
    let duration = value.GetDuration();
    Duration {
        Duration: duration.Duration,
        Fsp: duration.Fsp as i32,
    }
}

/// Json 源到各目标类型的 CAST。
fn cast_json(
    ctx: &mut CastContext,
    spec: &CastSpec,
    value: &BinaryJSON,
    target: EvalKind,
) -> Result<Option<ScalarValue>, CastError> {
    let result = match target {
        EvalKind::Real => ScalarValue::Real(json_to_real(ctx, value)?),
        EvalKind::Time => match value.TypeCode {
            JSONTypeCodeDate | JSONTypeCodeDatetime | JSONTypeCodeTimestamp => {
                let mut time = time_from_json(value, spec.target.GetType(), target_fsp(spec));
                if spec.target.GetType() == mysql::TypeDate {
                    time.SetCoreTime(FromDate(time.Year(), time.Month(), time.Day(), 0, 0, 0, 0));
                }
                ScalarValue::Time(time)
            }
            JSONTypeCodeDuration => {
                let duration = duration_from_json(value);
                let time = match duration.ConvertToTimeWithTimestamp(
                    &ctx.time_context,
                    spec.target.GetType(),
                    ctx.now,
                ) {
                    Ok(time) => time
                        .RoundFrac(&ctx.time_context, target_fsp(spec))
                        .map_err(error)?,
                    Err(failure) => return ctx.invalid(failure),
                };
                ScalarValue::Time(time)
            }
            JSONTypeCodeString => {
                let bytes = value.GetString();
                let text = String::from_utf8(bytes).map_err(error)?;
                return parse_time_value(ctx, spec, &text, false);
            }
            _ => {
                return ctx.invalid(format!(
                    "truncated incorrect time value: {}",
                    value.String()
                ));
            }
        },
        EvalKind::Json => ScalarValue::Json(value.clone()),
        // Go's JSON→String vectorized signature does not call padZeroForBinaryType.
        EvalKind::String => ScalarValue::String(produce_string(ctx, spec, value.String(), false)?),
        EvalKind::Int => ScalarValue::Int(json_to_int(ctx, spec, value)?),
        EvalKind::Decimal => {
            ScalarValue::Decimal(produce_decimal(ctx, spec, json_to_decimal(value)?)?)
        }
        EvalKind::Duration => match value.TypeCode {
            JSONTypeCodeDate | JSONTypeCodeDatetime | JSONTypeCodeTimestamp => {
                let time = time_from_json(value, mysql::TypeDatetime, target_fsp(spec));
                ScalarValue::Duration(
                    time.ConvertToDuration()
                        .map_err(error)?
                        .RoundFrac(target_fsp(spec), ctx.type_context.Location())
                        .map_err(error)?,
                )
            }
            JSONTypeCodeDuration => ScalarValue::Duration(duration_from_json(value)),
            JSONTypeCodeString => {
                let bytes = value.GetString();
                let text = String::from_utf8(bytes).map_err(error)?;
                return parse_duration_value(ctx, &text, target_fsp(spec));
            }
            _ => {
                return ctx.invalid(format!(
                    "truncated incorrect duration value: {}",
                    value.String()
                ));
            }
        },
    };
    Ok(Some(result))
}

/// 测试辅助：从字符串构造 Decimal。
pub fn decimal(value: &str) -> MyDecimal {
    decimal_from_str(value).expect("valid decimal literal")
}

/// 测试辅助：从字符串解析 BinaryJSON。
pub fn json(value: &str) -> BinaryJSON {
    ParseBinaryJSONFromString(value).expect("valid JSON literal")
}

/// 测试辅助：解析 Time。
pub fn time(value: &str, fsp: i32) -> Time {
    let context = CastContext::strict();
    ParseTime(&context.time_context, value, mysql::TypeDatetime, fsp)
        .expect("valid datetime literal")
}

/// 测试辅助：解析 Duration。
pub fn duration(value: &str, fsp: i32) -> Duration {
    let context = CastContext::strict();
    ParseDuration(&context.time_context, value, fsp)
        .expect("valid duration literal")
        .0
}

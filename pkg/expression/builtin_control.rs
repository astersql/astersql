// Copyright 2015 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Rust implementation of TiDB's scalar CASE, IF and IFNULL control flow.
//
// The Go implementation has a signature for every TiDB evaluation type.  Rust
// keeps those values in [`SqlValue`], but preserves the same evaluation order:
// conditions are evaluated from left to right, inactive result expressions are
// never observed, NULL conditions are false, and an error from an evaluated
// expression is returned immediately.

//
// 标量 CASE / IF / IFNULL 控制流：对应 TiDB Go 实现。
// 条件自左向右求值；未选中的结果表达式不求值；NULL 条件视为假；
// 已求值表达式出错则立即返回。SQL NULL 与三值逻辑在此模块中显式处理。

use std::fmt;

/// 未指定显示宽度 / 小数位数时的哨兵值（-1）。
pub const UNSPECIFIED_LENGTH: i32 = -1;
/// 实数类型默认最大显示宽度。
pub const MAX_REAL_WIDTH: i32 = 23;
/// 字段 NOT NULL 标志位。
pub const NOT_NULL_FLAG: u64 = 1;
/// 无符号整数标志位。
pub const UNSIGNED_FLAG: u64 = 1 << 5;
/// 二进制字符串 / BINARY 字符集标志位。
pub const BINARY_FLAG: u64 = 1 << 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 表达式求值类型（EvalType）：控制函数结果归约所用的类型族。
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
    /// 是否为字符串求值类型。
    pub fn is_string_kind(self) -> bool {
        matches!(self, Self::String)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// MySQL 字段类型种类（与 `mysql.Type*` 对应的简化枚举）。
pub enum FieldKind {
    Null,
    Tiny,
    Short,
    Int24,
    Long,
    Longlong,
    Float,
    Double,
    NewDecimal,
    Varchar,
    VarString,
    String,
    Date,
    Datetime,
    Timestamp,
    Duration,
    Json,
    Enum,
    Set,
    VectorFloat32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 字段类型描述：种类、显示宽度 flen、小数位、标志与字符集/校对。
pub struct FieldType {
    pub kind: FieldKind,
    pub flen: i32,
    pub decimal: i32,
    pub flags: u64,
    pub charset: String,
    pub collation: String,
}

impl FieldType {
    /// 按种类构造带默认 flen/decimal/charset 的字段类型。
    pub fn new(kind: FieldKind) -> Self {
        let (flen, decimal, charset, collation) = match kind {
            FieldKind::Null => (0, 0, "binary", "binary"),
            FieldKind::Tiny => (4, 0, "binary", "binary"),
            FieldKind::Short => (6, 0, "binary", "binary"),
            FieldKind::Int24 => (9, 0, "binary", "binary"),
            FieldKind::Long => (11, 0, "binary", "binary"),
            FieldKind::Longlong => (20, 0, "binary", "binary"),
            FieldKind::Float | FieldKind::Double => {
                (MAX_REAL_WIDTH, UNSPECIFIED_LENGTH, "binary", "binary")
            }
            FieldKind::NewDecimal => (MAX_REAL_WIDTH, 0, "binary", "binary"),
            FieldKind::Varchar
            | FieldKind::VarString
            | FieldKind::String
            | FieldKind::Enum
            | FieldKind::Set => (
                UNSPECIFIED_LENGTH,
                UNSPECIFIED_LENGTH,
                "utf8mb4",
                "utf8mb4_bin",
            ),
            FieldKind::Date => (10, 0, "binary", "binary"),
            FieldKind::Datetime | FieldKind::Timestamp => (19, 0, "binary", "binary"),
            FieldKind::Duration => (10, 0, "binary", "binary"),
            FieldKind::Json => (UNSPECIFIED_LENGTH, 0, "binary", "binary"),
            FieldKind::VectorFloat32 => (UNSPECIFIED_LENGTH, 0, "binary", "binary"),
        };
        Self {
            kind,
            flen,
            decimal,
            flags: 0,
            charset: charset.into(),
            collation: collation.into(),
        }
    }

    /// 设置显示宽度 flen。
    pub fn with_flen(mut self, flen: i32) -> Self {
        self.flen = flen;
        self
    }

    /// 设置小数位数。
    pub fn with_decimal(mut self, decimal: i32) -> Self {
        self.decimal = decimal;
        self
    }

    /// 设置类型标志位。
    pub fn with_flags(mut self, flags: u64) -> Self {
        self.flags = flags;
        self
    }

    /// 设置字符集与校对规则（collation）。
    pub fn with_charset(mut self, charset: &str, collation: &str) -> Self {
        self.charset = charset.into();
        self.collation = collation.into();
        self
    }

    /// 映射到表达式求值类型 EvalType。
    pub fn eval_type(&self) -> EvalType {
        match self.kind {
            FieldKind::Null
            | FieldKind::Tiny
            | FieldKind::Short
            | FieldKind::Int24
            | FieldKind::Long
            | FieldKind::Longlong => EvalType::Int,
            FieldKind::Float | FieldKind::Double => EvalType::Real,
            FieldKind::NewDecimal => EvalType::Decimal,
            FieldKind::Varchar
            | FieldKind::VarString
            | FieldKind::String
            | FieldKind::Enum
            | FieldKind::Set => EvalType::String,
            FieldKind::Date | FieldKind::Datetime => EvalType::Datetime,
            FieldKind::Timestamp => EvalType::Timestamp,
            FieldKind::Duration => EvalType::Duration,
            FieldKind::Json => EvalType::Json,
            FieldKind::VectorFloat32 => EvalType::VectorFloat32,
        }
    }

    /// 是否为 binary 字符集的字符串。
    pub fn is_binary_string(&self) -> bool {
        self.eval_type() == EvalType::String && self.charset == "binary"
    }

    /// 是否为非 binary 字符集的字符串。
    pub fn is_non_binary_string(&self) -> bool {
        self.eval_type() == EvalType::String && self.charset != "binary"
    }
}

#[derive(Clone, Debug, PartialEq)]
/// SQL 标量值载体；`Null` 表示 SQL NULL，`Error` 表示求值错误占位。
pub enum SqlValue {
    Null,
    Int(i64),
    UInt(u64),
    Real(f64),
    Decimal(String),
    String(String),
    Binary(Vec<u8>),
    Time(String),
    Duration(i64),
    Json(String),
    VectorFloat32(Vec<f32>),
    Set(String),
    Error(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 控制流求值错误。
pub struct EvalError(pub String);

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for EvalError {}

/// 若值为 Error 则转为 Err，否则克隆返回。
fn evaluated(value: &SqlValue) -> Result<SqlValue, EvalError> {
    match value {
        SqlValue::Error(error) => Err(EvalError(error.clone())),
        value => Ok(value.clone()),
    }
}

/// 从字符串提取前缀数值（用于真值判定），非法则视为 0。
fn numeric_prefix(value: &str) -> f64 {
    let value = value.trim_start();
    let mut index = 0;
    let mut valid_end = 0;
    let mut digits = false;
    let mut dot = false;
    let mut exponent_index = None;
    let bytes = value.as_bytes();
    if matches!(bytes.first(), Some(b'+' | b'-')) {
        index = 1;
    }
    // 扫描数字、可选小数点与科学计数法指数部分。
    while index < bytes.len() {
        match bytes[index] {
            b'0'..=b'9' => {
                digits = true;
                index += 1;
                valid_end = index;
            }
            b'.' if !dot && exponent_index.is_none() => {
                dot = true;
                index += 1;
                if digits {
                    valid_end = index;
                }
            }
            b'e' | b'E' if digits && exponent_index.is_none() => {
                exponent_index = Some(index);
                index += 1;
                // TiDB accepts a trailing exponent marker as the prefix before `e`.
                if index == bytes.len() {
                    break;
                }
                if matches!(bytes[index], b'+' | b'-') {
                    index += 1;
                }
            }
            _ => break,
        }
    }
    if !digits {
        0.0
    } else {
        value[..valid_end].parse().unwrap_or(0.0)
    }
}

/// 将 SQL 值转为布尔真值：NULL 为假；数值非零为真；字符串取数值前缀。
pub fn is_true(value: &SqlValue) -> Result<bool, EvalError> {
    Ok(match value {
        SqlValue::Null => false,
        SqlValue::Int(value) => *value != 0,
        SqlValue::UInt(value) => *value != 0,
        SqlValue::Real(value) => *value != 0.0,
        SqlValue::Decimal(value) | SqlValue::String(value) | SqlValue::Set(value) => {
            numeric_prefix(value) != 0.0
        }
        SqlValue::Binary(value) => numeric_prefix(&String::from_utf8_lossy(value)) != 0.0,
        SqlValue::Time(value) => numeric_prefix(&value.replace(['-', ':', ' '], "")) != 0.0,
        SqlValue::Duration(value) => *value != 0,
        SqlValue::Json(value) => {
            if value == "null" || value == "false" {
                false
            } else if value == "true" {
                true
            } else {
                numeric_prefix(value) != 0.0
            }
        }
        SqlValue::VectorFloat32(value) => value.iter().any(|item| *item != 0.0),
        SqlValue::Error(error) => return Err(EvalError(error.clone())),
    })
}

/// Evaluates `(WHEN, THEN)* [, ELSE]` exactly in source order.
/// 按源顺序求值 `(WHEN, THEN)* [, ELSE]`：首个真 WHEN 返回对应 THEN。
pub fn eval_case_when(args: &[SqlValue]) -> Result<SqlValue, EvalError> {
    if args.len() < 2 {
        return Err(EvalError(
            "CASE requires at least a WHEN and THEN argument".into(),
        ));
    }
    // 成对扫描 WHEN/THEN；奇数个参数时末项为 ELSE。
    let pair_limit = args.len() - (args.len() % 2);
    let mut index = 0;
    while index < pair_limit {
        if is_true(&args[index])? {
            return evaluated(&args[index + 1]);
        }
        index += 2;
    }
    if args.len() % 2 == 1 {
        evaluated(&args[args.len() - 1])
    } else {
        Ok(SqlValue::Null)
    }
}

/// IF(cond, a, b)：条件为真取 a，否则取 b；未选中分支不求值。
pub fn eval_if(
    condition: &SqlValue,
    when_true: &SqlValue,
    when_false: &SqlValue,
) -> Result<SqlValue, EvalError> {
    if is_true(condition)? {
        evaluated(when_true)
    } else {
        evaluated(when_false)
    }
}

/// IFNULL(a, b)：a 非 NULL 返回 a，否则求值并返回 b。
pub fn eval_if_null(first: &SqlValue, fallback: &SqlValue) -> Result<SqlValue, EvalError> {
    match first {
        SqlValue::Error(error) => Err(EvalError(error.clone())),
        SqlValue::Null => evaluated(fallback),
        value => Ok(value.clone()),
    }
}

/// 按行批量求值 CASE WHEN。
pub fn eval_case_when_rows(rows: &[Vec<SqlValue>]) -> Result<Vec<SqlValue>, EvalError> {
    rows.iter().map(|row| eval_case_when(row)).collect()
}

/// 按行批量求值 IF；三列行数须一致。
pub fn eval_if_rows(
    conditions: &[SqlValue],
    when_true: &[SqlValue],
    when_false: &[SqlValue],
) -> Result<Vec<SqlValue>, EvalError> {
    if conditions.len() != when_true.len() || conditions.len() != when_false.len() {
        return Err(EvalError(
            "IF vector arguments have different row counts".into(),
        ));
    }
    conditions
        .iter()
        .zip(when_true)
        .zip(when_false)
        .map(|((condition, yes), no)| eval_if(condition, yes, no))
        .collect()
}

/// 按行批量求值 IFNULL；两列行数须一致。
pub fn eval_if_null_rows(
    first: &[SqlValue],
    fallback: &[SqlValue],
) -> Result<Vec<SqlValue>, EvalError> {
    if first.len() != fallback.len() {
        return Err(EvalError(
            "IFNULL vector arguments have different row counts".into(),
        ));
    }
    first
        .iter()
        .zip(fallback)
        .map(|(first, fallback)| eval_if_null(first, fallback))
        .collect()
}

/// 取两侧显示宽度较大者；任一侧未指定则回退到 MAX_REAL_WIDTH。
pub fn max_len(lhs: i32, rhs: i32) -> i32 {
    if lhs < 0 || rhs < 0 {
        MAX_REAL_WIDTH
    } else {
        lhs.max(rhs)
    }
}

/// 整数种类的默认显示宽度。
fn integer_display_width(kind: FieldKind) -> Option<i32> {
    match kind {
        FieldKind::Tiny => Some(4),
        FieldKind::Short => Some(6),
        FieldKind::Int24 => Some(9),
        FieldKind::Long => Some(11),
        FieldKind::Longlong => Some(20),
        _ => None,
    }
}

/// 按参数字段类型推导结果 flen（显示宽度）。
pub fn set_flen_from_args(eval_type: EvalType, result: &mut FieldType, args: &[FieldType]) {
    // DECIMAL/INT：按整数部分宽度 + 小数位 + 符号位估算 flen。
    if matches!(eval_type, EvalType::Decimal | EvalType::Int) {
        let mut max_arg_flen = 0;
        for arg in args {
            let sign_len = i32::from(arg.flags & UNSIGNED_FLAG == 0);
            let mut flen = arg.flen - sign_len;
            if arg.decimal != UNSPECIFIED_LENGTH {
                flen -= arg.decimal;
            }
            max_arg_flen = max_len(max_arg_flen, flen);
        }
        result.flen = (max_arg_flen + result.decimal + 1).min(65);
    } else if eval_type == EvalType::String {
        let mut max_arg_flen = 0;
        for arg in args {
            if let Some(width) = integer_display_width(arg.kind) {
                max_arg_flen = max_len(width, max_arg_flen);
            } else if arg.flen == UNSPECIFIED_LENGTH {
                result.flen = UNSPECIFIED_LENGTH;
                return;
            } else {
                max_arg_flen = max_len(arg.flen, max_arg_flen);
            }
        }
        result.flen = max_arg_flen;
    } else {
        result.flen = args
            .iter()
            .fold(0, |current, arg| max_len(arg.flen, current));
    }
}

/// 按参数字段类型推导结果 decimal（小数位数）；整型固定为 0。
pub fn set_decimal_from_args(eval_type: EvalType, result: &mut FieldType, args: &[FieldType]) {
    if eval_type == EvalType::Int {
        result.decimal = 0;
        return;
    }
    let mut max_decimal = 0;
    for arg in args {
        if arg.decimal == UNSPECIFIED_LENGTH {
            result.decimal = UNSPECIFIED_LENGTH;
            return;
        }
        max_decimal = max_decimal.max(arg.decimal);
    }
    result.decimal = max_decimal.min(30);
}

/// 聚合多参数的求值类型：混类型按 String > Real > Decimal > Int 归约。
fn aggregate_eval_type(fields: &[FieldType]) -> Result<EvalType, EvalError> {
    let first = fields[0].eval_type();
    if fields.iter().all(|field| field.eval_type() == first) {
        return Ok(first);
    }
    let has = |candidate| fields.iter().any(|field| field.eval_type() == candidate);
    if has(EvalType::VectorFloat32) {
        return Err(EvalError(
            "VECTOR FLOAT32 cannot be mixed with another control result type".into(),
        ));
    }
    if has(EvalType::String)
        || has(EvalType::Json)
        || has(EvalType::Datetime)
        || has(EvalType::Timestamp)
        || has(EvalType::Duration)
    {
        Ok(EvalType::String)
    } else if has(EvalType::Real) {
        Ok(EvalType::Real)
    } else if has(EvalType::Decimal) {
        Ok(EvalType::Decimal)
    } else {
        Ok(EvalType::Int)
    }
}

/// 按归约后的 EvalType 选择结果 FieldKind。
fn aggregate_kind(eval_type: EvalType, fields: &[FieldType]) -> FieldKind {
    let first = fields[0].kind;
    if fields.iter().all(|field| field.kind == first) {
        return first;
    }
    match eval_type {
        EvalType::Int => FieldKind::Longlong,
        EvalType::Real => FieldKind::Double,
        EvalType::Decimal => FieldKind::NewDecimal,
        EvalType::String => FieldKind::Varchar,
        EvalType::Datetime => FieldKind::Datetime,
        EvalType::Timestamp => FieldKind::Timestamp,
        EvalType::Duration => FieldKind::Duration,
        EvalType::Json => FieldKind::Json,
        EvalType::VectorFloat32 => FieldKind::VectorFloat32,
    }
}

/// 聚合标志：NOT NULL / UNSIGNED 需全员具备；BINARY 任一侧即可。
fn aggregate_flags(fields: &[FieldType]) -> u64 {
    let mut flags = 0;
    if fields.iter().all(|field| field.flags & NOT_NULL_FLAG != 0) {
        flags |= NOT_NULL_FLAG;
    }
    if fields.iter().all(|field| field.flags & UNSIGNED_FLAG != 0) {
        flags |= UNSIGNED_FLAG;
    }
    if fields.iter().any(|field| field.flags & BINARY_FLAG != 0) {
        flags |= BINARY_FLAG;
    }
    flags
}

/// 将结果设为 binary 字符集并置 BINARY 标志。
fn set_binary_charset(result: &mut FieldType) {
    result.charset = "binary".into();
    result.collation = "binary".into();
    result.flags |= BINARY_FLAG;
}

/// 按控制函数名与参数字符集推导结果 charset / collation。
fn derive_charset(
    func_name: &str,
    eval_type: EvalType,
    result: &mut FieldType,
    args: &[FieldType],
) {
    let any_binary = args.iter().any(FieldType::is_binary_string);
    let any_non_binary = args.iter().any(FieldType::is_non_binary_string);
    let any_non_string = args
        .iter()
        .any(|field| field.eval_type() != EvalType::String);

    match func_name.to_ascii_lowercase().as_str() {
        // 控制类函数：binary 或非字符串 → binary；否则继承非 binary 参数字符集。
        "if" | "ifnull" | "lead" | "lag" | "case" | "coalesce" => {
            if any_binary || !eval_type.is_string_kind() {
                set_binary_charset(result);
            } else if any_non_binary {
                if let Some(field) = args.iter().find(|field| field.is_non_binary_string()) {
                    result.charset.clone_from(&field.charset);
                    result.collation.clone_from(&field.collation);
                }
                result.flags &= !BINARY_FLAG;
                if any_non_string {
                    result.flags |= BINARY_FLAG;
                }
            } else {
                result.charset = "utf8mb4".into();
                result.collation = "utf8mb4_bin".into();
                result.flags &= !BINARY_FLAG;
            }
        }
        _ => {}
    }
}

/// Mirrors `InferType4ControlFuncs` for IF, IFNULL, CASE, COALESCE, LEAD and LAG.
/// 对应 Go `InferType4ControlFuncs`：为 IF/IFNULL/CASE/COALESCE/LEAD/LAG 推导结果类型。
pub fn infer_type_for_control(func_name: &str, args: &[FieldType]) -> Result<FieldType, EvalError> {
    if args.is_empty() {
        return Err(EvalError(
            "control type inference requires at least one argument".into(),
        ));
    }
    // 分离 NULL 类型参数与非 NULL 参数。
    let (nulls, non_nulls): (Vec<_>, Vec<_>) = args
        .iter()
        .cloned()
        .partition(|field| field.kind == FieldKind::Null);
    if non_nulls.is_empty() {
        let mut result = nulls[0].clone();
        result.kind = FieldKind::Null;
        result.flen = 0;
        result.decimal = 0;
        result.flags &= !NOT_NULL_FLAG;
        set_binary_charset(&mut result);
        return Ok(result);
    }

    let mut result = if non_nulls.len() == 1 {
        non_nulls[0].clone()
    } else {
        let eval_type = aggregate_eval_type(&non_nulls)?;
        let mut result = FieldType::new(aggregate_kind(eval_type, &non_nulls));
        result.flags = aggregate_flags(&non_nulls);
        set_decimal_from_args(eval_type, &mut result, &non_nulls);
        derive_charset(func_name, eval_type, &mut result, args);
        set_flen_from_args(eval_type, &mut result, &non_nulls);
        result
    };

    // 存在 NULL 参数则结果不可为 NOT NULL。
    if !nulls.is_empty() {
        result.flags &= !NOT_NULL_FLAG;
    }
    match result.eval_type() {
        EvalType::Int => result.decimal = 0,
        EvalType::String => result.decimal = UNSPECIFIED_LENGTH,
        _ => {}
    }
    // ENUM/SET 在控制函数中提升为整数或 VARCHAR。
    if matches!(result.kind, FieldKind::Enum | FieldKind::Set) {
        result.kind = match result.eval_type() {
            EvalType::Int => FieldKind::Longlong,
            _ => FieldKind::Varchar,
        };
    }
    // DATETIME/TIMESTAMP 显示宽度含可选小数秒。
    if matches!(result.kind, FieldKind::Datetime | FieldKind::Timestamp) {
        result.flen = 19
            + if result.decimal > 0 {
                result.decimal + 1
            } else {
                0
            };
    }
    Ok(result)
}

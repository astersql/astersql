// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// Rust implementation of the scalar behaviors in `builtin_other.go`.
//
// The package integration task supplies TiDB's shared expression interfaces.
// This vertical file keeps the Go control-flow decisions in an independently
// runnable form: IN constant folding and SQL NULL semantics, user variables,
// VALUES(), BIT_COUNT(), ROW(), and GET_PARAM().

// 其他杂项标量内置函数（对应 Go `builtin_other.go`）。
// 独立可运行地保留 Go 控制流：IN 常量折叠与 SQL NULL 语义、用户变量、
// VALUES()、BIT_COUNT()、ROW()、GET_PARAM()。

use std::collections::{HashMap, HashSet};

use crate::collate;
use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};

/// 本模块结果别名。
pub type Result<T> = std::result::Result<T, OtherError>;

#[derive(Clone, Debug, PartialEq, thiserror::Error)]
/// IN/VALUES/用户变量/参数查找等路径上的错误。
pub enum OtherError {
    #[error("Incorrect parameter count in the call to native function '{function}'")]
    ArgumentCount { function: &'static str },
    #[error("{0} is not supported for IN()")]
    UnsupportedInType(String),
    #[error("{0} is not supported for VALUES()")]
    UnsupportedValuesType(String),
    #[error("cannot convert {value:?} to {target:?}")]
    Conversion { value: Value, target: EvalType },
    #[error("column index {index} is outside row length {len}")]
    ColumnIndex { index: usize, len: usize },
    #[error("Session current insert values len {len} and column's offset {offset} don't match")]
    ValuesOffset { len: usize, offset: usize },
    #[error("Session current insert values is too long")]
    InsertValueTooLong,
    #[error("parameter index {index} exceeds parameter count {count}")]
    ParamIndex { index: i64, count: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
/// 表达式求值类型（用于 IN/VALUES 分派与类型转换）。
pub enum EvalType {
    Int,
    String,
    Real,
    Decimal,
    Time,
    Duration,
    Json,
    VectorFloat32,
}

impl std::fmt::Display for EvalType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 影响 hybrid/浮点精度等行为的 MySQL 列类型标记。
pub enum MysqlType {
    #[default]
    Unspecified,
    Bit,
    Float,
    Enum,
    Set,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 字段类型：求值类型、MySQL 类型与校对规则（collation）。
pub struct FieldType {
    eval_type: EvalType,
    mysql_type: MysqlType,
    collation: String,
}

impl FieldType {
    /// 构造默认 binary collation 的字段类型。
    pub fn new(eval_type: EvalType) -> Self {
        Self {
            eval_type,
            mysql_type: MysqlType::Unspecified,
            collation: "binary".to_owned(),
        }
    }

    /// BIT 列字段类型。
    pub fn bit() -> Self {
        Self {
            eval_type: EvalType::Int,
            mysql_type: MysqlType::Bit,
            collation: "binary".to_owned(),
        }
    }

    /// FLOAT（单精度）字段类型。
    pub fn float32() -> Self {
        Self {
            eval_type: EvalType::Real,
            mysql_type: MysqlType::Float,
            collation: "binary".to_owned(),
        }
    }

    /// 指定 collation 的字符串字段。
    pub fn string(collation: impl Into<String>) -> Self {
        Self {
            eval_type: EvalType::String,
            mysql_type: MysqlType::Unspecified,
            collation: collation.into(),
        }
    }

    /// Bit/Enum/Set 等 hybrid 字符串字段。
    pub fn hybrid_string(mysql_type: MysqlType, collation: impl Into<String>) -> Self {
        debug_assert!(matches!(
            mysql_type,
            MysqlType::Bit | MysqlType::Enum | MysqlType::Set
        ));
        Self {
            eval_type: EvalType::String,
            mysql_type,
            collation: collation.into(),
        }
    }

    /// 求值类型。
    pub fn eval_type(&self) -> EvalType {
        self.eval_type
    }

    /// MySQL 列类型标记。
    pub fn mysql_type(&self) -> MysqlType {
        self.mysql_type
    }

    /// 校对规则名。
    pub fn collation(&self) -> &str {
        &self.collation
    }

    /// 是否为 Bit/Enum/Set hybrid 类型。
    pub fn is_hybrid(&self) -> bool {
        matches!(
            self.mysql_type,
            MysqlType::Bit | MysqlType::Enum | MysqlType::Set
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
/// SQL 值的统一表示；`Null` 表示 SQL NULL。
pub enum Value {
    Null,
    Int(i64),
    UInt(u64),
    Real(f64),
    Decimal(Decimal),
    String(String),
    Bytes(Vec<u8>),
    BinaryLiteral(Vec<u8>),
    Time(i64),
    Duration(i64),
    Json(serde_json::Value),
    VectorFloat32(Vec<f32>),
}

impl Value {
    /// 是否为 SQL NULL。
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// NULL 无类型；其余变体映射到对应 EvalType。
    pub fn eval_type(&self) -> Option<EvalType> {
        match self {
            Self::Null => None,
            Self::Int(_) | Self::UInt(_) | Self::BinaryLiteral(_) => Some(EvalType::Int),
            Self::Real(_) => Some(EvalType::Real),
            Self::Decimal(_) => Some(EvalType::Decimal),
            Self::String(_) | Self::Bytes(_) => Some(EvalType::String),
            Self::Time(_) => Some(EvalType::Time),
            Self::Duration(_) => Some(EvalType::Duration),
            Self::Json(_) => Some(EvalType::Json),
            Self::VectorFloat32(_) => Some(EvalType::VectorFloat32),
        }
    }

    /// 转为 MySQL 文本表示（用于字符串比较/GET_PARAM 等）。
    fn to_mysql_string(&self) -> Result<String> {
        match self {
            Self::Null => Ok(String::new()),
            Self::Int(value) => Ok(value.to_string()),
            Self::UInt(value) => Ok(value.to_string()),
            Self::Real(value) => Ok(value.to_string()),
            Self::Decimal(value) => Ok(value.to_string()),
            Self::String(value) => Ok(value.clone()),
            Self::Bytes(value) | Self::BinaryLiteral(value) => {
                Ok(String::from_utf8_lossy(value).into_owned())
            }
            Self::Time(value) | Self::Duration(value) => Ok(value.to_string()),
            Self::Json(value) => Ok(value.to_string()),
            Self::VectorFloat32(values) => Ok(format!(
                "[{}]",
                values
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )),
        }
    }

    /// 转换到目标求值类型；不兼容则报 Conversion 错误。
    fn convert_to(&self, target: EvalType) -> Result<Self> {
        if self.is_null() {
            return Ok(Self::Null);
        }
        match target {
            EvalType::String => Ok(Self::String(self.to_mysql_string()?)),
            EvalType::Int => {
                let (value, unsigned) = int_repr(self)?;
                if unsigned {
                    Ok(Self::UInt(value as u64))
                } else {
                    Ok(Self::Int(value))
                }
            }
            EvalType::Real => Ok(Self::Real(real_value(self)?)),
            EvalType::Decimal => Ok(Self::Decimal(decimal_value(self)?)),
            EvalType::Time => match self {
                Self::Time(value) => Ok(Self::Time(*value)),
                _ => Err(conversion_error(self, target)),
            },
            EvalType::Duration => match self {
                Self::Duration(value) => Ok(Self::Duration(*value)),
                _ => Err(conversion_error(self, target)),
            },
            EvalType::Json => match self {
                Self::Json(value) => Ok(Self::Json(value.clone())),
                _ => Err(conversion_error(self, target)),
            },
            EvalType::VectorFloat32 => match self {
                Self::VectorFloat32(value) => Ok(Self::VectorFloat32(value.clone())),
                _ => Err(conversion_error(self, target)),
            },
        }
    }
}

/// 构造类型转换错误。
fn conversion_error(value: &Value, target: EvalType) -> OtherError {
    OtherError::Conversion {
        value: value.clone(),
        target,
    }
}

#[derive(Clone, Debug, PartialEq)]
/// IN 参数表达式：纯常量、仅上下文常量、或列引用。
pub enum Expr {
    Constant(Value),
    Context(Value),
    Column(usize),
}

impl Expr {
    /// 严格常量参数。
    pub fn constant(value: Value) -> Self {
        Self::Constant(value)
    }

    /// ConstOnlyInContext：可在构建期求值但仍作运行时参数。
    pub fn context(value: Value) -> Self {
        Self::Context(value)
    }

    /// 行内列下标引用。
    pub fn column(index: usize) -> Self {
        Self::Column(index)
    }

    /// 对给定行求值。
    fn eval(&self, row: &[Value]) -> Result<Value> {
        match self {
            Self::Constant(value) | Self::Context(value) => Ok(value.clone()),
            Self::Column(index) => row.get(*index).cloned().ok_or(OtherError::ColumnIndex {
                index: *index,
                len: row.len(),
            }),
        }
    }

    /// 是否为可参与常量哈希折叠的严格常量。
    fn is_strict_constant(&self) -> bool {
        matches!(self, Self::Constant(_))
    }
}

#[derive(Clone, Debug)]
/// IN 列表常量哈希缓存，按求值类型分桶。
enum ConstantCache {
    Int(HashMap<i64, bool>),
    String(HashSet<Vec<u8>>),
    Real(HashSet<u64>),
    Decimal(HashSet<Decimal>),
    Time(HashSet<i64>),
    Duration(HashSet<i64>),
    None,
}

impl ConstantCache {
    /// 按类型创建空缓存；Json/Vector 不支持常量哈希。
    fn for_type(eval_type: EvalType) -> Self {
        match eval_type {
            EvalType::Int => Self::Int(HashMap::new()),
            EvalType::String => Self::String(HashSet::new()),
            EvalType::Real => Self::Real(HashSet::new()),
            EvalType::Decimal => Self::Decimal(HashSet::new()),
            EvalType::Time => Self::Time(HashSet::new()),
            EvalType::Duration => Self::Duration(HashSet::new()),
            EvalType::Json | EvalType::VectorFloat32 => Self::None,
        }
    }

    /// 已缓存常量个数。
    fn len(&self) -> usize {
        match self {
            Self::Int(values) => values.len(),
            Self::String(values) => values.len(),
            Self::Real(values) => values.len(),
            Self::Decimal(values) => values.len(),
            Self::Time(values) => values.len(),
            Self::Duration(values) => values.len(),
            Self::None => 0,
        }
    }

    /// 该类型是否支持常量哈希加速。
    fn supports_constants(&self) -> bool {
        !matches!(self, Self::None)
    }

    /// 插入常量；返回是否为新键（NaN 视为“可插入但不入集”的特殊情况由调用方处理）。
    fn insert(&mut self, value: &Value, field_type: &FieldType) -> Result<bool> {
        match self {
            Self::Int(values) => {
                let (value, unsigned) = int_repr(value)?;
                if let std::collections::hash_map::Entry::Vacant(entry) = values.entry(value) {
                    entry.insert(unsigned);
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            Self::String(values) => Ok(values.insert(collation_key(value, field_type)?)),
            Self::Real(values) => {
                let value = real_value(value)?;
                if value.is_nan() {
                    return Ok(true);
                }
                Ok(values.insert(canonical_float_bits(value)))
            }
            Self::Decimal(values) => Ok(values.insert(decimal_value(value)?)),
            Self::Time(values) => match value {
                Value::Time(value) => Ok(values.insert(*value)),
                _ => Err(conversion_error(value, EvalType::Time)),
            },
            Self::Duration(values) => match value {
                Value::Duration(value) => Ok(values.insert(*value)),
                _ => Err(conversion_error(value, EvalType::Duration)),
            },
            Self::None => Ok(true),
        }
    }

    /// 查询针值是否命中常量缓存。
    fn contains(&self, value: &Value, field_type: &FieldType) -> Result<bool> {
        match self {
            Self::Int(values) => {
                let (needle, needle_unsigned) = int_repr(value)?;
                Ok(values.get(&needle).is_some_and(|candidate_unsigned| {
                    integers_equal(needle, needle_unsigned, needle, *candidate_unsigned)
                }))
            }
            Self::String(values) => Ok(values.contains(&collation_key(value, field_type)?)),
            Self::Real(values) => {
                let value = real_value(value)?;
                Ok(!value.is_nan() && values.contains(&canonical_float_bits(value)))
            }
            Self::Decimal(values) => Ok(values.contains(&decimal_value(value)?)),
            Self::Time(values) => match value {
                Value::Time(value) => Ok(values.contains(value)),
                _ => Err(conversion_error(value, EvalType::Time)),
            },
            Self::Duration(values) => match value {
                Value::Duration(value) => Ok(values.contains(value)),
                _ => Err(conversion_error(value, EvalType::Duration)),
            },
            Self::None => Ok(false),
        }
    }
}

#[derive(Clone, Debug)]
/// IN 谓词：常量折叠缓存 + 非常量参数列表 + NULL 语义。
pub struct InPredicate {
    field_type: FieldType,
    args: Vec<Expr>,
    non_constant_args: Vec<usize>,
    has_null: bool,
    cache: ConstantCache,
    skip_plan_cache_reason: Option<String>,
}

impl InPredicate {
    /// Mirrors `inFunctionClass.verifyArgs` and
    /// `builtinInXXXSig.buildHashMapForConstArgs`.
    /// 对齐 `inFunctionClass.verifyArgs` 与 `buildHashMapForConstArgs`：
    /// 校验参数个数、过滤 BIT 负常量、折叠可哈希常量并记录非常量下标。
    pub fn build(
        field_type: FieldType,
        args: Vec<Expr>,
        plan_cache_sensitive: bool,
    ) -> Result<Self> {
        if args.len() < 2 {
            return Err(OtherError::ArgumentCount { function: "in" });
        }

        let mut validated = Vec::with_capacity(args.len());
        let mut skip_plan_cache_reason = None;
        for (index, arg) in args.into_iter().enumerate() {
            let is_negative_bit_constant = index != 0
                && field_type.mysql_type == MysqlType::Bit
                && matches!(&arg, Expr::Constant(Value::Int(value)) if *value < 0);
            if is_negative_bit_constant {
                if plan_cache_sensitive {
                    let Expr::Constant(Value::Int(value)) = arg else {
                        unreachable!();
                    };
                    skip_plan_cache_reason = Some(format!("Bit Column in ({value})"));
                }
                continue;
            }
            validated.push(arg);
        }
        if validated.len() < 2 {
            return Err(OtherError::ArgumentCount { function: "in" });
        }

        let mut cache = ConstantCache::for_type(field_type.eval_type);
        if !cache.supports_constants() {
            return Ok(Self {
                field_type,
                args: validated,
                non_constant_args: Vec::new(),
                has_null: false,
                cache,
                skip_plan_cache_reason,
            });
        }

        let mut compact = vec![validated[0].clone()];
        let mut non_constant_args = Vec::new();
        let mut has_null = false;
        for arg in validated.into_iter().skip(1) {
            if arg.is_strict_constant() {
                let value = arg.eval(&[])?;
                if value.is_null() {
                    if !has_null {
                        has_null = true;
                        compact.push(arg);
                    }
                    continue;
                }
                if cache.insert(&value, &field_type)? {
                    compact.push(arg);
                }
                continue;
            }

            // ConstOnlyInContext is evaluated once here in Go to reject a
            // wrongly typed cached plan, but it remains a runtime argument.
            // Go 在此求值一次以拒绝错误类型的缓存计划，但仍保留为运行时参数。
            if matches!(&arg, Expr::Context(_)) {
                let value = arg.eval(&[])?;
                validate_value_for_type(&value, field_type.eval_type)?;
            }
            compact.push(arg);
            non_constant_args.push(compact.len() - 1);
        }

        Ok(Self {
            field_type,
            args: compact,
            non_constant_args,
            has_null,
            cache,
            skip_plan_cache_reason,
        })
    }

    /// 求值 IN：命中为 true；未命中且曾见 NULL 为 NULL；否则 false。
    pub fn eval(&self, row: &[Value]) -> Result<Option<bool>> {
        let needle = self.args[0].eval(row)?;
        if needle.is_null() {
            return Ok(None);
        }

        if self.cache.contains(&needle, &self.field_type)? {
            return Ok(Some(true));
        }

        let cache_active = self.cache.len() != 0;
        let mut has_null = cache_active && self.has_null;
        if cache_active {
            for index in &self.non_constant_args {
                let candidate = self.args[*index].eval(row)?;
                if candidate.is_null() {
                    has_null = true;
                    continue;
                }
                if values_equal(&needle, &candidate, &self.field_type)? {
                    return Ok(Some(true));
                }
            }
        } else {
            for candidate in self.args.iter().skip(1) {
                let candidate = candidate.eval(row)?;
                if candidate.is_null() {
                    has_null = true;
                    continue;
                }
                if values_equal(&needle, &candidate, &self.field_type)? {
                    return Ok(Some(true));
                }
            }
        }

        Ok(if has_null { None } else { Some(false) })
    }

    /// 已折叠常量个数。
    pub fn cached_constants(&self) -> usize {
        self.cache.len()
    }

    /// 非常量候选参数个数。
    pub fn non_constant_args(&self) -> usize {
        self.non_constant_args.len()
    }

    /// 常量列表中是否含 NULL。
    pub fn has_null_constant(&self) -> bool {
        self.has_null
    }

    /// 压缩后的参数个数（含针值）。
    pub fn argument_count(&self) -> usize {
        self.args.len()
    }

    /// 若因 BIT 负常量跳过计划缓存，返回原因文案。
    pub fn skip_plan_cache_reason(&self) -> Option<&str> {
        self.skip_plan_cache_reason.as_deref()
    }
}

/// 便捷入口：全部为常量时构造并求值 IN。
pub fn eval_in(
    field_type: FieldType,
    needle: Value,
    candidates: Vec<Value>,
) -> Result<Option<bool>> {
    let mut args = Vec::with_capacity(candidates.len() + 1);
    args.push(Expr::constant(needle));
    args.extend(candidates.into_iter().map(Expr::constant));
    InPredicate::build(field_type, args, false)?.eval(&[])
}

/// 校验值是否可转换为目标求值类型。
fn validate_value_for_type(value: &Value, eval_type: EvalType) -> Result<()> {
    if value.is_null() {
        return Ok(());
    }
    match eval_type {
        EvalType::Int => int_repr(value).map(|_| ()),
        EvalType::String => value.to_mysql_string().map(|_| ()),
        EvalType::Real => real_value(value).map(|_| ()),
        EvalType::Decimal => decimal_value(value).map(|_| ()),
        EvalType::Time if matches!(value, Value::Time(_)) => Ok(()),
        EvalType::Duration if matches!(value, Value::Duration(_)) => Ok(()),
        EvalType::Json if matches!(value, Value::Json(_)) => Ok(()),
        EvalType::VectorFloat32 if matches!(value, Value::VectorFloat32(_)) => Ok(()),
        _ => Err(conversion_error(value, eval_type)),
    }
}

/// 按字段类型比较两值是否相等（含有符号/无符号整数规则）。
fn values_equal(left: &Value, right: &Value, field_type: &FieldType) -> Result<bool> {
    match field_type.eval_type {
        EvalType::Int => {
            let (left, left_unsigned) = int_repr(left)?;
            let (right, right_unsigned) = int_repr(right)?;
            Ok(integers_equal(left, left_unsigned, right, right_unsigned))
        }
        EvalType::String => {
            Ok(collation_key(left, field_type)? == collation_key(right, field_type)?)
        }
        EvalType::Real => Ok(real_value(left)? == real_value(right)?),
        EvalType::Decimal => Ok(decimal_value(left)? == decimal_value(right)?),
        EvalType::Time => match (left, right) {
            (Value::Time(left), Value::Time(right)) => Ok(left == right),
            _ => Err(conversion_error(right, EvalType::Time)),
        },
        EvalType::Duration => match (left, right) {
            (Value::Duration(left), Value::Duration(right)) => Ok(left == right),
            _ => Err(conversion_error(right, EvalType::Duration)),
        },
        EvalType::Json => match (left, right) {
            (Value::Json(left), Value::Json(right)) => Ok(json_values_equal(left, right)),
            _ => Err(conversion_error(right, EvalType::Json)),
        },
        EvalType::VectorFloat32 => match (left, right) {
            (Value::VectorFloat32(left), Value::VectorFloat32(right)) => Ok(left == right),
            _ => Err(conversion_error(right, EvalType::VectorFloat32)),
        },
    }
}

/// Mirrors `types.CompareBinaryJSON(...) == 0` for the JSON shapes represented
/// by `serde_json::Value`. In particular, TiDB compares integer and floating
/// encodings numerically and applies its `1e-8` tolerance only when the two
/// numeric encodings differ.
fn json_values_equal(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    use serde_json::Value as JsonValue;

    match (left, right) {
        (JsonValue::Number(left), JsonValue::Number(right)) => {
            if left.is_f64() && right.is_f64() {
                return left.as_f64() == right.as_f64();
            }
            if left.is_f64() || right.is_f64() {
                let (Some(left), Some(right)) = (left.as_f64(), right.as_f64()) else {
                    return false;
                };
                return (left - right).abs() < 1e-8;
            }
            match (left.as_i64(), right.as_i64()) {
                (Some(left), Some(right)) => left == right,
                _ => left.as_u64() == right.as_u64(),
            }
        }
        (JsonValue::Array(left), JsonValue::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_values_equal(left, right))
        }
        (JsonValue::Object(left), JsonValue::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| json_values_equal(left, right))
                })
        }
        _ => left == right,
    }
}

/// 有符号与无符号混合比较：负有符号与无符号永不相等。
fn integers_equal(left: i64, left_unsigned: bool, right: i64, right_unsigned: bool) -> bool {
    match (left_unsigned, right_unsigned) {
        (true, true) | (false, false) => left == right,
        (false, true) => left >= 0 && left == right,
        (true, false) => right >= 0 && left == right,
    }
}

/// 将值规范为 (i64 位型, 是否无符号)。
fn int_repr(value: &Value) -> Result<(i64, bool)> {
    match value {
        Value::Int(value) => Ok((*value, false)),
        Value::UInt(value) => Ok((*value as i64, true)),
        Value::Real(value) if value.is_finite() => Ok((*value as i64, false)),
        Value::Decimal(decimal) => decimal
            .trunc()
            .to_i64()
            .map(|value| (value, false))
            .ok_or_else(|| conversion_error(value, EvalType::Int)),
        Value::String(value) => Ok((parse_mysql_int(value), false)),
        Value::Bytes(value) => Ok((parse_mysql_int(&String::from_utf8_lossy(value)), false)),
        Value::BinaryLiteral(value) if value.len() <= 8 => {
            Ok((binary_literal_to_u64(value) as i64, true))
        }
        _ => Err(conversion_error(value, EvalType::Int)),
    }
}

/// MySQL 风格文本转整数：失败则回退解析浮点再截断，再失败为 0。
fn parse_mysql_int(value: &str) -> i64 {
    let value = value.trim();
    value
        .parse::<i64>()
        .or_else(|_| value.parse::<f64>().map(|number| number as i64))
        .unwrap_or(0)
}

/// 转为 f64。
fn real_value(value: &Value) -> Result<f64> {
    match value {
        Value::Int(value) => Ok(*value as f64),
        Value::UInt(value) => Ok(*value as f64),
        Value::Real(value) => Ok(*value),
        Value::Decimal(decimal) => decimal
            .to_f64()
            .ok_or_else(|| conversion_error(value, EvalType::Real)),
        Value::String(value) => value
            .trim()
            .parse::<f64>()
            .map_err(|_| conversion_error(&Value::String(value.clone()), EvalType::Real)),
        _ => Err(conversion_error(value, EvalType::Real)),
    }
}

/// 转为 Decimal。
fn decimal_value(value: &Value) -> Result<Decimal> {
    match value {
        Value::Int(value) => Ok(Decimal::from(*value)),
        Value::UInt(value) => Ok(Decimal::from(*value)),
        Value::Real(real) => Decimal::from_f64(*real)
            .ok_or_else(|| conversion_error(&Value::Real(*real), EvalType::Decimal)),
        Value::Decimal(value) => Ok(*value),
        Value::String(value) => value
            .parse::<Decimal>()
            .map_err(|_| conversion_error(&Value::String(value.clone()), EvalType::Decimal)),
        _ => Err(conversion_error(value, EvalType::Decimal)),
    }
}

/// 二进制字面量按大端折叠为 u64。
fn binary_literal_to_u64(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0_u64, |value, byte| (value << 8) | u64::from(*byte))
}

/// 规范浮点位型：+0/-0 统一为 +0 的 bits，便于哈希集合。
fn canonical_float_bits(value: f64) -> u64 {
    if value == 0.0 {
        0.0_f64.to_bits()
    } else {
        value.to_bits()
    }
}

/// 按字段 collation 生成字符串比较键。
fn collation_key(value: &Value, field_type: &FieldType) -> Result<Vec<u8>> {
    let value = match value {
        Value::String(value) => value.clone(),
        Value::Bytes(value) => String::from_utf8_lossy(value).into_owned(),
        _ => return Err(conversion_error(value, EvalType::String)),
    };
    Ok(collate::GetCollator(&field_type.collation).Key(&value))
}

#[derive(Clone, Debug, Default)]
/// 会话侧状态：用户变量、当前 INSERT 行值、计划缓存参数。
pub struct Session {
    user_vars: HashMap<String, Value>,
    user_var_types: HashMap<String, EvalType>,
    readonly_user_vars: HashSet<String>,
    pub curr_insert_values: Vec<Value>,
    pub plan_cache_params: Vec<Value>,
}

impl Session {
    /// Mirrors SET_VAR: NULL returns immediately and does not mutate the map;
    /// all stored values are owned so a later row-buffer reuse cannot alter it.
    /// 对齐 SET_VAR：NULL 立即返回且不改 map；存入的是自有值，避免行缓冲复用污染。
    pub fn set_user_var(&mut self, name: &str, value: Value) -> Result<Value> {
        if value.is_null() {
            return Ok(Value::Null);
        }
        let name = name.to_lowercase();
        if let Some(eval_type) = value.eval_type() {
            self.user_var_types.insert(name.clone(), eval_type);
        }
        self.user_vars.insert(name, value.clone());
        Ok(value)
    }

    /// 按名读取用户变量并转换到目标类型；不存在返回 None。
    pub fn get_user_var(&self, name: &str, target: EvalType) -> Result<Option<Value>> {
        self.user_vars
            .get(&name.to_lowercase())
            .map(|value| value.convert_to(target))
            .transpose()
    }

    /// 是否存在该用户变量。
    pub fn contains_user_var(&self, name: &str) -> bool {
        self.user_vars.contains_key(&name.to_lowercase())
    }

    /// 用户变量记录的求值类型。
    pub fn get_user_var_type(&self, name: &str) -> Option<EvalType> {
        self.user_var_types.get(&name.to_lowercase()).copied()
    }

    /// 标记用户变量只读（可折叠为常量）。
    pub fn mark_readonly(&mut self, name: &str) {
        self.readonly_user_vars.insert(name.to_lowercase());
    }

    /// 是否只读用户变量。
    pub fn is_readonly(&self, name: &str) -> bool {
        self.readonly_user_vars.contains(&name.to_lowercase())
    }
}

#[derive(Clone, Debug, PartialEq)]
/// GET_VAR：只读变量折叠为常量，否则运行时按名查找。
pub enum GetVarExpr {
    Constant(Value),
    Runtime { name: String, target: EvalType },
}

impl GetVarExpr {
    /// Mirrors BuildGetVarFunction and convertReadonlyVarToConst.
    /// 对齐 BuildGetVarFunction / convertReadonlyVarToConst。
    pub fn build(session: &Session, name: &str, target: EvalType) -> Result<Self> {
        let normalized = name.to_lowercase();
        if session.is_readonly(&normalized) {
            if let Some(Value::BinaryLiteral(value)) = session.user_vars.get(&normalized) {
                return Ok(Self::Constant(Value::BinaryLiteral(value.clone())));
            }
            return Ok(Self::Constant(
                session
                    .get_user_var(&normalized, target)?
                    .unwrap_or(Value::Null),
            ));
        }
        Ok(Self::Runtime {
            name: normalized,
            target,
        })
    }

    /// 求值 GET_VAR。
    pub fn eval(&self, session: &Session) -> Result<Value> {
        match self {
            Self::Constant(value) => Ok(value.clone()),
            Self::Runtime { name, target } => {
                Ok(session.get_user_var(name, *target)?.unwrap_or(Value::Null))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// VALUES(col)：读取当前 INSERT 行中指定偏移列的值。
pub struct ValuesFunction {
    offset: usize,
    field_type: FieldType,
}

impl ValuesFunction {
    /// Mirrors valuesFunctionClass.verifyArgs: VALUES() has no scalar args;
    /// the selected column offset and type are carried by the function class.
    /// 对齐 valuesFunctionClass.verifyArgs：无标量参数；列偏移与类型由函数类携带。
    pub fn build(offset: usize, field_type: FieldType, argument_count: usize) -> Result<Self> {
        if argument_count != 0 {
            return Err(OtherError::ArgumentCount { function: "values" });
        }
        Ok(Self { offset, field_type })
    }

    /// 直接构造（跳过参数个数校验）。
    pub fn new(offset: usize, field_type: FieldType) -> Self {
        Self { offset, field_type }
    }

    /// 从 `curr_insert_values` 取列并按字段类型规范化。
    pub fn eval(&self, session: &Session) -> Result<Value> {
        let row = &session.curr_insert_values;
        if row.is_empty() {
            return Ok(Value::Null);
        }
        let value = row.get(self.offset).ok_or(OtherError::ValuesOffset {
            len: row.len(),
            offset: self.offset,
        })?;
        if value.is_null() {
            return Ok(Value::Null);
        }

        match self.field_type.eval_type {
            EvalType::Int => match value {
                Value::BinaryLiteral(bytes) | Value::Bytes(bytes) => {
                    if bytes.len() > 8 {
                        return Err(OtherError::InsertValueTooLong);
                    }
                    Ok(Value::Int(binary_literal_to_u64(bytes) as i64))
                }
                Value::Int(value) => Ok(Value::Int(*value)),
                Value::UInt(value) => Ok(Value::Int(*value as i64)),
                _ => Err(conversion_error(value, EvalType::Int)),
            },
            EvalType::Real => {
                let value = real_value(value)?;
                if self.field_type.mysql_type == MysqlType::Float {
                    Ok(Value::Real(f64::from(value as f32)))
                } else {
                    Ok(Value::Real(value))
                }
            }
            EvalType::Decimal => Ok(Value::Decimal(decimal_value(value)?)),
            EvalType::String if self.field_type.is_hybrid() => {
                Ok(Value::String(value.to_mysql_string()?))
            }
            EvalType::String => match value {
                Value::String(value) => Ok(Value::String(value.clone())),
                Value::Bytes(value) | Value::BinaryLiteral(value) => {
                    Ok(Value::String(String::from_utf8_lossy(value).into_owned()))
                }
                _ => Err(conversion_error(value, EvalType::String)),
            },
            EvalType::Time => match value {
                Value::Time(value) => Ok(Value::Time(*value)),
                _ => Err(conversion_error(value, EvalType::Time)),
            },
            EvalType::Duration => match value {
                Value::Duration(value) => Ok(Value::Duration(*value)),
                _ => Err(conversion_error(value, EvalType::Duration)),
            },
            EvalType::Json => match value {
                Value::Json(value) => Ok(Value::Json(value.clone())),
                _ => Err(conversion_error(value, EvalType::Json)),
            },
            EvalType::VectorFloat32 => match value {
                Value::VectorFloat32(value) => Ok(Value::VectorFloat32(value.clone())),
                _ => Err(conversion_error(value, EvalType::VectorFloat32)),
            },
        }
    }
}

/// Implements BIT_COUNT(N) after TiDB's integer coercion. Signed values use
/// their two's-complement u64 representation, matching Go's bitCount helper.
/// BIT_COUNT(N)：整数强制转换后统计置位；有符号按补码 u64 计位，对齐 Go。
pub fn bit_count(value: &Value) -> Result<Option<i64>> {
    if value.is_null() {
        return Ok(None);
    }
    let bits = match value {
        Value::UInt(value) => *value,
        Value::Real(value)
            if !value.is_finite() || *value > i64::MAX as f64 || *value < i64::MIN as f64 =>
        {
            return Ok(Some(64));
        }
        Value::Decimal(value) if value.trunc().to_i64().is_none() => return Ok(Some(64)),
        Value::String(value) if mysql_text_overflows_i64(value) => {
            return Ok(Some(64));
        }
        Value::Bytes(value) if mysql_text_overflows_i64(&String::from_utf8_lossy(value)) => {
            return Ok(Some(64));
        }
        Value::BinaryLiteral(bytes) if bytes.len() <= 8 => binary_literal_to_u64(bytes),
        _ => int_repr(value)?.0 as u64,
    };
    Ok(Some(i64::from(bits.count_ones())))
}

/// 文本无法落入 i64 且浮点值超出 i64 范围时视为溢出。
fn mysql_text_overflows_i64(value: &str) -> bool {
    let text = value.trim();
    text.parse::<i64>().is_err()
        && text
            .parse::<f64>()
            .is_ok_and(|number| number > i64::MAX as f64 || number < i64::MIN as f64)
}

/// Implements GET_PARAM's typed-index lookup and string result.
/// GET_PARAM：按索引取计划缓存参数并转为字符串；越界报错，NULL 返回 None。
pub fn get_param(session: &Session, index: i64) -> Result<Option<String>> {
    let value = usize::try_from(index)
        .ok()
        .and_then(|index| session.plan_cache_params.get(index))
        .ok_or(OtherError::ParamIndex {
            index,
            count: session.plan_cache_params.len(),
        })?;
    if value.is_null() {
        return Ok(None);
    }
    Ok(Some(value.to_mysql_string()?))
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// ROW(...)：重写阶段展平，运行时不应被求值。
pub struct RowFunction {
    argument_types: Vec<FieldType>,
}

impl RowFunction {
    /// 构造 ROW 函数元数据。
    pub fn new(argument_types: Vec<FieldType>) -> Self {
        Self { argument_types }
    }

    /// 各参数字段类型。
    pub fn argument_types(&self) -> &[FieldType] {
        &self.argument_types
    }

    /// ROW is flattened during expression rewrite and must not be evaluated.
    /// ROW 在表达式重写时已展平，求值路径应 panic。
    pub fn eval(&self) -> ! {
        panic!("builtinRowSig.evalString() should never be called.")
    }
}

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

// `builtin_other` 的向量化求值内核。
//
// 提供 `EvalContext`（计划参数、用户变量、collation）、`VectorExpression` trait，
// 以及列/字面量表达式、VALUES/ROW、BIT_COUNT、GET_PARAM 与 SET/GET 用户变量等
// 向量签名，对应 Go `builtin_other_vec.go` 中非生成部分。

use std::collections::HashMap;

use crate::{chunk, types};

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
/// 向量化求值错误：溢出、参数越界、未实现或不支持的类型路径等。
pub enum EvalError {
    #[error("integer overflow")]
    Overflow,
    #[error("Param index exceed param counts")]
    ParamIndexExceeds,
    #[error("not implemented")]
    NotImplemented,
    #[error("expression does not support {0} vector evaluation")]
    Unsupported(&'static str),
    #[error("{0}")]
    Message(String),
}

/// 向量化求值结果别名。
pub type EvalResult<T> = Result<T, EvalError>;

/// 将外部 Display 错误包装为 `EvalError::Message`。
fn external_error(error: impl std::fmt::Display) -> EvalError {
    EvalError::Message(error.to_string())
}

/// 向量内建共用的求值上下文：计划参数、用户变量、collation 与类型转换状态。
/// Evaluation state used by the vector builtins in this file group. It keeps
/// the same parameter, user-variable, collation, and conversion state touched
/// by the corresponding Go methods.
pub struct EvalContext {
    parameters: Vec<types::Datum>,
    user_vars: HashMap<String, types::Datum>,
    collation: String,
    type_context: types::Context,
}

impl EvalContext {
    /// 以计划缓存参数列表构造上下文，默认 binary collation 与无告警类型上下文。
    pub fn new(parameters: Vec<types::Datum>) -> Self {
        Self {
            parameters,
            user_vars: HashMap::new(),
            collation: "utf8mb4_bin".to_owned(),
            type_context: (*types::DefaultStmtNoWarningContext).clone(),
        }
    }

    /// 覆盖字符串用户变量写入时使用的 collation。
    pub fn with_collation(mut self, collation: impl Into<String>) -> Self {
        self.collation = collation.into();
        self
    }

    /// 按索引取计划参数；负索引或越界返回 `ParamIndexExceeds`。
    fn parameter(&self, index: i64) -> EvalResult<&types::Datum> {
        if index < 0 {
            return Err(EvalError::ParamIndexExceeds);
        }
        self.parameters
            .get(index as usize)
            .ok_or(EvalError::ParamIndexExceeds)
    }

    /// 写入用户变量（调用方应已把名称转为小写）。
    fn set_user_var(&mut self, name: String, value: types::Datum) {
        self.user_vars.insert(name, value);
    }

    /// 按小写名称查询用户变量。
    pub fn user_var(&self, name: &str) -> Option<&types::Datum> {
        self.user_vars.get(&name.to_lowercase())
    }
}

/// 逻辑行号映射到物理行（考虑 Chunk 选择向量 Sel）。
fn physical_row(input: &chunk::Chunk, row: usize) -> usize {
    input.Sel().map_or(row, |selection| selection[row])
}

/// 重置结果列为定长 i64，预留 `capacity` 行。
pub(crate) fn reset_int_column(result: &mut chunk::Column, capacity: usize) {
    *result = *chunk::newFixedLenColumn(chunk::sizeInt64, capacity);
}

/// 重置结果列为定长 f64。
fn reset_real_column(result: &mut chunk::Column, capacity: usize) {
    *result = *chunk::newFixedLenColumn(chunk::sizeFloat64, capacity);
}

/// 重置结果列为定长 MyDecimal。
fn reset_decimal_column(result: &mut chunk::Column, capacity: usize) {
    *result = *chunk::newFixedLenColumn(chunk::sizeMyDecimal, capacity);
}

/// 重置结果列为定长 Time。
fn reset_time_column(result: &mut chunk::Column, capacity: usize) {
    *result = *chunk::newFixedLenColumn(chunk::sizeTime, capacity);
}

/// 重置结果列为变长字符串。
fn reset_string_column(result: &mut chunk::Column, capacity: usize) {
    *result = *chunk::newVarLenColumn(capacity);
}

/// 重置结果列为变长 JSON。
fn reset_json_column(result: &mut chunk::Column, capacity: usize) {
    *result = *chunk::newVarLenColumn(capacity);
}

/// 将可空 i64 切片写入结果列。
pub(crate) fn write_int_options(result: &mut chunk::Column, values: &[Option<i64>]) {
    reset_int_column(result, values.len());
    for value in values {
        match value {
            Some(value) => result.AppendInt64(*value),
            None => result.AppendNull(),
        }
    }
}

/// 向量表达式边界：不支持的类型方法显式失败，避免静默走错表示。
/// The production vector-expression boundary used by all signatures in this
/// group. Unsupported type methods fail explicitly instead of silently
/// coercing a value through an unrelated representation.
pub trait VectorExpression: Send + Sync {
    fn vec_eval_int(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> EvalResult<()> {
        Err(EvalError::Unsupported("int"))
    }

    fn vec_eval_string(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> EvalResult<()> {
        Err(EvalError::Unsupported("string"))
    }

    fn vec_eval_real(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> EvalResult<()> {
        Err(EvalError::Unsupported("real"))
    }

    fn vec_eval_decimal(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> EvalResult<()> {
        Err(EvalError::Unsupported("decimal"))
    }

    fn vec_eval_time(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> EvalResult<()> {
        Err(EvalError::Unsupported("time"))
    }

    fn vec_eval_duration(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> EvalResult<()> {
        Err(EvalError::Unsupported("duration"))
    }

    fn vec_eval_json(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> EvalResult<()> {
        Err(EvalError::Unsupported("json"))
    }

    fn eval_int_row(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _row: usize,
    ) -> EvalResult<Option<i64>> {
        Err(EvalError::Unsupported("scalar int"))
    }

    fn is_unsigned(&self) -> bool {
        false
    }
}

/// 从输入 Chunk 按列下标投影的向量表达式。
pub struct ColumnExpression {
    index: usize,
    unsigned: bool,
}

impl ColumnExpression {
    /// 引用输入第 `index` 列，默认有符号。
    pub fn new(index: usize) -> Self {
        Self {
            index,
            unsigned: false,
        }
    }

    /// 标记该列是否为无符号整数（影响 IN 比较）。
    pub fn unsigned(mut self, unsigned: bool) -> Self {
        self.unsigned = unsigned;
        self
    }

    /// 取得源列；下标越界则报错。
    fn source<'a>(&self, input: &'a chunk::Chunk) -> EvalResult<&'a chunk::Column> {
        input
            .columns
            .get(self.index)
            .ok_or_else(|| EvalError::Message(format!("column {} is out of range", self.index)))
    }
}

impl VectorExpression for ColumnExpression {
    fn vec_eval_int(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let source = self.source(input)?;
        reset_int_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            let row = physical_row(input, row);
            if source.IsNull(row) {
                result.AppendNull();
            } else {
                result.AppendInt64(source.GetInt64(row));
            }
        }
        Ok(())
    }

    fn vec_eval_string(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let source = self.source(input)?;
        reset_string_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            let row = physical_row(input, row);
            if source.IsNull(row) {
                result.AppendNull();
            } else {
                result.AppendString(&source.GetString(row));
            }
        }
        Ok(())
    }

    fn vec_eval_real(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let source = self.source(input)?;
        reset_real_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            let row = physical_row(input, row);
            if source.IsNull(row) {
                result.AppendNull();
            } else {
                result.AppendFloat64(source.GetFloat64(row));
            }
        }
        Ok(())
    }

    fn vec_eval_decimal(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let source = self.source(input)?;
        reset_decimal_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            let row = physical_row(input, row);
            if source.IsNull(row) {
                result.AppendNull();
            } else {
                result.AppendMyDecimal(&source.GetDecimal(row));
            }
        }
        Ok(())
    }

    fn vec_eval_time(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let source = self.source(input)?;
        reset_time_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            let row = physical_row(input, row);
            if source.IsNull(row) {
                result.AppendNull();
            } else {
                result.AppendTime(source.GetTime(row));
            }
        }
        Ok(())
    }

    fn vec_eval_duration(
        &self,
        context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        self.vec_eval_int(context, input, result)
    }

    fn vec_eval_json(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let source = self.source(input)?;
        reset_json_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            let row = physical_row(input, row);
            if source.IsNull(row) {
                result.AppendNull();
            } else {
                result.AppendJSON(source.GetJSON(row));
            }
        }
        Ok(())
    }

    fn eval_int_row(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        row: usize,
    ) -> EvalResult<Option<i64>> {
        let source = self.source(input)?;
        let row = physical_row(input, row);
        Ok((!source.IsNull(row)).then(|| source.GetInt64(row)))
    }

    fn is_unsigned(&self) -> bool {
        self.unsigned
    }
}

#[derive(Clone)]
/// 字面量取值族，对应各 EvalType 的常量向量广播。
pub enum LiteralValue {
    Null,
    Int(i64, bool),
    String(String),
    Real(f64),
    Decimal(types::MyDecimal),
    Time(types::Time),
    Duration(i64),
    Json(types::BinaryJSON),
}

/// 将同一字面量广播到 Chunk 每一行的表达式。
pub struct LiteralExpression(pub LiteralValue);

impl LiteralExpression {
    fn rows(input: &chunk::Chunk) -> usize {
        input.NumRows()
    }
}

impl VectorExpression for LiteralExpression {
    fn vec_eval_int(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        reset_int_column(result, Self::rows(input));
        for _ in 0..Self::rows(input) {
            match self.0 {
                LiteralValue::Null => result.AppendNull(),
                LiteralValue::Int(value, _) => result.AppendInt64(value),
                _ => return Err(EvalError::Unsupported("int literal")),
            }
        }
        Ok(())
    }

    fn vec_eval_string(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        reset_string_column(result, Self::rows(input));
        for _ in 0..Self::rows(input) {
            match &self.0 {
                LiteralValue::Null => result.AppendNull(),
                LiteralValue::String(value) => result.AppendString(value),
                _ => return Err(EvalError::Unsupported("string literal")),
            }
        }
        Ok(())
    }

    fn vec_eval_real(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        reset_real_column(result, Self::rows(input));
        for _ in 0..Self::rows(input) {
            match self.0 {
                LiteralValue::Null => result.AppendNull(),
                LiteralValue::Real(value) => result.AppendFloat64(value),
                _ => return Err(EvalError::Unsupported("real literal")),
            }
        }
        Ok(())
    }

    fn vec_eval_decimal(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        reset_decimal_column(result, Self::rows(input));
        for _ in 0..Self::rows(input) {
            match &self.0 {
                LiteralValue::Null => result.AppendNull(),
                LiteralValue::Decimal(value) => result.AppendMyDecimal(value),
                _ => return Err(EvalError::Unsupported("decimal literal")),
            }
        }
        Ok(())
    }

    fn vec_eval_time(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        reset_time_column(result, Self::rows(input));
        for _ in 0..Self::rows(input) {
            match self.0 {
                LiteralValue::Null => result.AppendNull(),
                LiteralValue::Time(value) => result.AppendTime(value),
                _ => return Err(EvalError::Unsupported("time literal")),
            }
        }
        Ok(())
    }

    fn vec_eval_duration(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        reset_int_column(result, Self::rows(input));
        for _ in 0..Self::rows(input) {
            match self.0 {
                LiteralValue::Null => result.AppendNull(),
                LiteralValue::Duration(value) => result.AppendInt64(value),
                _ => return Err(EvalError::Unsupported("duration literal")),
            }
        }
        Ok(())
    }

    fn vec_eval_json(
        &self,
        _context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        reset_json_column(result, Self::rows(input));
        for _ in 0..Self::rows(input) {
            match &self.0 {
                LiteralValue::Null => result.AppendNull(),
                LiteralValue::Json(value) => result.AppendJSON(value.clone()),
                _ => return Err(EvalError::Unsupported("json literal")),
            }
        }
        Ok(())
    }

    fn eval_int_row(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _row: usize,
    ) -> EvalResult<Option<i64>> {
        match self.0 {
            LiteralValue::Null => Ok(None),
            LiteralValue::Int(value, _) => Ok(Some(value)),
            _ => Err(EvalError::Unsupported("scalar int literal")),
        }
    }

    fn is_unsigned(&self) -> bool {
        matches!(self.0, LiteralValue::Int(_, true))
    }
}

/// 生成 VALUES 各返回类型的占位向量签名。
macro_rules! values_signature {
    ($name:ident, $eval:ident) => {
        #[derive(Clone, Copy, Debug, Default)]
        /// VALUES 向量签名占位：标记不可向量化，求值返回 NotImplemented。
        pub struct $name;

        impl $name {
            pub fn vectorized(&self) -> bool {
                false
            }

            pub fn $eval(
                &self,
                _context: &EvalContext,
                _input: &chunk::Chunk,
                _result: &mut chunk::Column,
            ) -> EvalResult<()> {
                Err(EvalError::NotImplemented)
            }
        }
    };
}

values_signature!(BuiltinValuesIntSig, vec_eval_int);
values_signature!(BuiltinValuesDurationSig, vec_eval_duration);
values_signature!(BuiltinValuesRealSig, vec_eval_real);
values_signature!(BuiltinValuesStringSig, vec_eval_string);
values_signature!(BuiltinValuesTimeSig, vec_eval_time);
values_signature!(BuiltinValuesJsonSig, vec_eval_json);
values_signature!(BuiltinValuesDecimalSig, vec_eval_decimal);

/// 与 Go 命名对齐的 JSON VALUES 别名。
pub type BuiltinValuesJSONSig = BuiltinValuesJsonSig;

#[derive(Clone, Copy, Debug, Default)]
/// ROW 签名：声明可向量化，但字符串向量求值按 Go 契约直接 panic。
pub struct BuiltinRowSig;

impl BuiltinRowSig {
    pub fn vectorized(&self) -> bool {
        true
    }

    pub fn vec_eval_string(
        &self,
        _context: &EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> EvalResult<()> {
        panic!("builtinRowSig.vecEvalString() should never be called.")
    }
}

/// 计算二的补码 int64 中置位个数；负数用 wrapping 算术对齐 Go。
/// bitCount returns the number of set bits in the two's-complement int64,
/// retaining Go's wrapping arithmetic for negative inputs.
pub fn bit_count(mut value: i64) -> i64 {
    value = value.wrapping_sub((value >> 1) & 0x5555_5555_5555_5555);
    value = (value & 0x3333_3333_3333_3333).wrapping_add((value >> 2) & 0x3333_3333_3333_3333);
    value = (value & 0x0f0f_0f0f_0f0f_0f0f).wrapping_add((value >> 4) & 0x0f0f_0f0f_0f0f_0f0f);
    value = value.wrapping_add(value >> 8);
    value = value.wrapping_add(value >> 16);
    value = value.wrapping_add(value >> 32);
    value & 0x7f
}

/// 向量化 BIT_COUNT：参数溢出时回退到逐行 `eval_int_row`。
pub struct BuiltinBitCountSig {
    arg: Box<dyn VectorExpression>,
}

impl BuiltinBitCountSig {
    pub fn new(arg: Box<dyn VectorExpression>) -> Self {
        Self { arg }
    }

    pub fn vectorized(&self) -> bool {
        true
    }

    pub fn vec_eval_int(
        &self,
        context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let mut values = chunk::Column::default();
        match self.arg.vec_eval_int(context, input, &mut values) {
            Ok(()) => {
                let output = (0..input.NumRows())
                    .map(|row| (!values.IsNull(row)).then(|| bit_count(values.GetInt64(row))))
                    .collect::<Vec<_>>();
                write_int_options(result, &output);
                Ok(())
            }
            // 向量路径溢出时，按行标量求值再做 bit_count，对齐 Go 回退语义。
            Err(EvalError::Overflow) => {
                let mut output = Vec::with_capacity(input.NumRows());
                for row in 0..input.NumRows() {
                    output.push(self.arg.eval_int_row(context, input, row)?.map(bit_count));
                }
                write_int_options(result, &output);
                Ok(())
            }
            Err(error) => Err(error),
        }
    }
}

/// 向量化 GET_PARAM：按索引取计划参数并转为字符串。
pub struct BuiltinGetParamStringSig {
    index: Box<dyn VectorExpression>,
}

impl BuiltinGetParamStringSig {
    pub fn new(index: Box<dyn VectorExpression>) -> Self {
        Self { index }
    }

    pub fn vectorized(&self) -> bool {
        true
    }

    pub fn vec_eval_string(
        &self,
        context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let mut indexes = chunk::Column::default();
        self.index.vec_eval_int(context, input, &mut indexes)?;
        reset_string_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            if indexes.IsNull(row) {
                result.AppendNull();
                continue;
            }
            let value = context.parameter(indexes.GetInt64(row))?;
            match value.ToString() {
                Ok(value) => result.AppendString(&value),
                Err(_) => result.AppendNull(),
            }
        }
        Ok(())
    }
}

/// 生成「变量名 + 值」二元 SET 用户变量签名骨架。
macro_rules! binary_signature {
    ($name:ident) => {
        pub struct $name {
            name: Box<dyn VectorExpression>,
            value: Box<dyn VectorExpression>,
        }

        impl $name {
            pub fn new(name: Box<dyn VectorExpression>, value: Box<dyn VectorExpression>) -> Self {
                Self { name, value }
            }

            pub fn vectorized(&self) -> bool {
                true
            }
        }
    };
}

binary_signature!(BuiltinSetStringVarSig);
binary_signature!(BuiltinSetIntVarSig);
binary_signature!(BuiltinSetRealVarSig);
binary_signature!(BuiltinSetDecimalVarSig);

/// 按行写入字符串用户变量（名称小写），并回传赋值结果列。
impl BuiltinSetStringVarSig {
    pub fn vec_eval_string(
        &self,
        context: &mut EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let mut names = chunk::Column::default();
        let mut values = chunk::Column::default();
        self.name.vec_eval_string(context, input, &mut names)?;
        self.value.vec_eval_string(context, input, &mut values)?;
        reset_string_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            if names.IsNull(row) || values.IsNull(row) {
                result.AppendNull();
                continue;
            }
            let name = names.GetString(row).to_lowercase();
            let value = values.GetString(row);
            let datum = types::NewCollationStringDatum(value.clone(), context.collation.clone());
            context.set_user_var(name, datum);
            result.AppendString(&value);
        }
        Ok(())
    }
}

/// 按行写入整数用户变量。
impl BuiltinSetIntVarSig {
    pub fn vec_eval_int(
        &self,
        context: &mut EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let mut names = chunk::Column::default();
        let mut values = chunk::Column::default();
        self.name.vec_eval_string(context, input, &mut names)?;
        self.value.vec_eval_int(context, input, &mut values)?;
        reset_int_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            if names.IsNull(row) || values.IsNull(row) {
                result.AppendNull();
                continue;
            }
            let name = names.GetString(row).to_lowercase();
            let value = values.GetInt64(row);
            context.set_user_var(name, types::NewIntDatum(value));
            result.AppendInt64(value);
        }
        Ok(())
    }
}

/// 按行写入实数用户变量。
impl BuiltinSetRealVarSig {
    pub fn vec_eval_real(
        &self,
        context: &mut EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let mut names = chunk::Column::default();
        let mut values = chunk::Column::default();
        self.name.vec_eval_string(context, input, &mut names)?;
        self.value.vec_eval_real(context, input, &mut values)?;
        reset_real_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            if names.IsNull(row) || values.IsNull(row) {
                result.AppendNull();
                continue;
            }
            let name = names.GetString(row).to_lowercase();
            let value = values.GetFloat64(row);
            context.set_user_var(name, types::NewFloat64Datum(value));
            result.AppendFloat64(value);
        }
        Ok(())
    }
}

/// 按行写入 Decimal 用户变量。
impl BuiltinSetDecimalVarSig {
    pub fn vec_eval_decimal(
        &self,
        context: &mut EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let mut names = chunk::Column::default();
        let mut values = chunk::Column::default();
        self.name.vec_eval_string(context, input, &mut names)?;
        self.value.vec_eval_decimal(context, input, &mut values)?;
        reset_decimal_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            if names.IsNull(row) || values.IsNull(row) {
                result.AppendNull();
                continue;
            }
            let name = names.GetString(row).to_lowercase();
            let value = values.GetDecimal(row);
            context.set_user_var(name, types::NewDecimalDatum(value.clone()));
            result.AppendMyDecimal(&value);
        }
        Ok(())
    }
}

/// 生成「仅变量名」一元 GET 用户变量签名骨架。
macro_rules! unary_signature {
    ($name:ident) => {
        pub struct $name {
            name: Box<dyn VectorExpression>,
        }

        impl $name {
            pub fn new(name: Box<dyn VectorExpression>) -> Self {
                Self { name }
            }

            pub fn vectorized(&self) -> bool {
                true
            }
        }
    };
}

unary_signature!(BuiltinGetStringVarSig);
unary_signature!(BuiltinGetIntVarSig);
unary_signature!(BuiltinGetRealVarSig);
unary_signature!(BuiltinGetDecimalVarSig);

/// 按变量名（小写）读取字符串用户变量；缺失则为 NULL。
impl BuiltinGetStringVarSig {
    pub fn vec_eval_string(
        &self,
        context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let mut names = chunk::Column::default();
        self.name.vec_eval_string(context, input, &mut names)?;
        reset_string_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            if names.IsNull(row) {
                result.AppendNull();
                continue;
            }
            let name = names.GetString(row).to_lowercase();
            let Some(value) = context.user_vars.get(&name) else {
                result.AppendNull();
                continue;
            };
            result.AppendString(&value.ToString().map_err(external_error)?);
        }
        Ok(())
    }
}

/// 按变量名读取整数用户变量。
impl BuiltinGetIntVarSig {
    pub fn vec_eval_int(
        &self,
        context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let mut names = chunk::Column::default();
        self.name.vec_eval_string(context, input, &mut names)?;
        reset_int_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            if names.IsNull(row) {
                result.AppendNull();
                continue;
            }
            let name = names.GetString(row).to_lowercase();
            match context.user_vars.get(&name) {
                Some(value) => result.AppendInt64(value.GetInt64()),
                None => result.AppendNull(),
            }
        }
        Ok(())
    }
}

/// 按变量名读取实数用户变量，经类型上下文做 ToFloat64。
impl BuiltinGetRealVarSig {
    pub fn vec_eval_real(
        &self,
        context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let mut names = chunk::Column::default();
        self.name.vec_eval_string(context, input, &mut names)?;
        reset_real_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            if names.IsNull(row) {
                result.AppendNull();
                continue;
            }
            let name = names.GetString(row).to_lowercase();
            match context.user_vars.get(&name) {
                Some(value) => result.AppendFloat64(
                    value
                        .ToFloat64(context.type_context.clone())
                        .map_err(external_error)?,
                ),
                None => result.AppendNull(),
            }
        }
        Ok(())
    }
}

/// 按变量名读取 Decimal 用户变量，经类型上下文做 ToDecimal。
impl BuiltinGetDecimalVarSig {
    pub fn vec_eval_decimal(
        &self,
        context: &EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> EvalResult<()> {
        let mut names = chunk::Column::default();
        self.name.vec_eval_string(context, input, &mut names)?;
        reset_decimal_column(result, input.NumRows());
        for row in 0..input.NumRows() {
            if names.IsNull(row) {
                result.AppendNull();
                continue;
            }
            let name = names.GetString(row).to_lowercase();
            match context.user_vars.get(&name) {
                Some(value) => result.AppendMyDecimal(
                    &value
                        .ToDecimal(context.type_context.clone())
                        .map_err(external_error)?,
                ),
                None => result.AppendNull(),
            }
        }
        Ok(())
    }
}

// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Per-evaluation storage for optional, constant, and vector builtin parameters.
//
// 内置函数参数的求值期存储，对应 Go 的 `funcParam`。
// 支持三种来源：缺省未提供、常量（含 NULL）、列向量；向量化求值按行下标取参。

use thiserror::Error;

/// 参数求值输入来源，在构建 `FuncParam` 前由表达式框架填充。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamSource<T> {
    /// 可选参数未出现在调用中。
    NotProvided,
    /// 常量实参；`None` 表示 SQL NULL。
    Constant(Option<T>),
    /// 非常量表达式已按批求值为列。
    Column(Vec<T>),
    /// 参数求值阶段已失败，携带错误信息。
    EvalError(String),
}

/// 参数构建与按行取值时可能返回的错误。
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ParamError {
    #[error("parameter evaluation failed: {0}")]
    Evaluation(String),
    #[error("parameter row {index} is outside column length {length}")]
    RowOutOfBounds { index: usize, length: usize },
}

/// 内部存储：常量/缺省用单值，列参数用向量。
#[derive(Debug, Clone, PartialEq, Eq)]
enum ParamStorage<T> {
    Default(T),
    Column(Vec<T>),
}

/// `FuncParam` mirrors Go's `funcParam`: constants and omitted arguments use a
/// default value, while non-constant expressions retain their evaluated column.
///
/// 求值期参数句柄：常量与缺省走 `Default`，列参数走 `Column`，按行 `get` 取值。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuncParam<T> {
    storage: ParamStorage<T>,
}

impl<T> FuncParam<T> {
    /// 按行下标取参数引用；常量对任意合法行返回同一默认值。
    pub fn get(&self, row: usize) -> Result<&T, ParamError> {
        match &self.storage {
            ParamStorage::Default(value) => Ok(value),
            ParamStorage::Column(values) => values.get(row).ok_or(ParamError::RowOutOfBounds {
                index: row,
                length: values.len(),
            }),
        }
    }

    /// 是否为列存储（非常量参数）。
    pub fn is_column(&self) -> bool {
        matches!(self.storage, ParamStorage::Column(_))
    }

    /// 若为列参数则返回底层切片，否则 `None`。
    pub fn column(&self) -> Option<&[T]> {
        match &self.storage {
            ParamStorage::Column(values) => Some(values),
            ParamStorage::Default(_) => None,
        }
    }
}

/// 构建结果：成功得到可取值的 `FuncParam`，或常量 NULL（调用方短路）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildParam<T> {
    Value(FuncParam<T>),
    ConstNull,
}

impl<T> BuildParam<T> {
    /// 非 NULL 时返回内部 `FuncParam`。
    pub fn value(&self) -> Option<&FuncParam<T>> {
        match self {
            Self::Value(value) => Some(value),
            Self::ConstNull => None,
        }
    }
}

/// 将 `ParamSource` 规约为 `BuildParam`；`omitted_default` 用于缺省参数。
fn build_param<T>(source: ParamSource<T>, omitted_default: T) -> Result<BuildParam<T>, ParamError> {
    match source {
        // 未提供：用调用方给定的缺省值包成 Default。
        ParamSource::NotProvided => Ok(BuildParam::Value(FuncParam {
            storage: ParamStorage::Default(omitted_default),
        })),
        ParamSource::Constant(Some(value)) => Ok(BuildParam::Value(FuncParam {
            storage: ParamStorage::Default(value),
        })),
        // 常量 NULL：不进入 FuncParam，由上层按 NULL 语义处理。
        ParamSource::Constant(None) => Ok(BuildParam::ConstNull),
        ParamSource::Column(values) => Ok(BuildParam::Value(FuncParam {
            storage: ParamStorage::Column(values),
        })),
        ParamSource::EvalError(error) => Err(ParamError::Evaluation(error)),
    }
}

/// Builds a string parameter. Omitted strings use the same empty default as Go.
///
/// 构建字符串参数；缺省时默认值为空串，与 Go 一致。
pub fn build_string_param(source: ParamSource<String>) -> Result<BuildParam<String>, ParamError> {
    build_param(source, String::new())
}

/// Builds an integer parameter with the caller-provided omitted default.
///
/// 构建整数参数；缺省值由调用方传入（不同内置函数缺省不同）。
pub fn build_int_param(
    source: ParamSource<i64>,
    default_int_value: i64,
) -> Result<BuildParam<i64>, ParamError> {
    build_param(source, default_int_value)
}

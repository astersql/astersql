// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// ILIKE 向量化求值路径。
//
// 对应 Go `builtin_ilike_vec.go`：字符串参数的常量/列表示、escape 必须为常量，
// 以及按行合并 NULL 位图的 `vec_eval_int`。

use crate::builtin_ilike_kernel::{ExpressionError, IlikeSig};
use stringutil_dependency as stringutil;

/// A string argument in the vectorized evaluator. This mirrors Go's funcParam:
/// constants carry one value while columns carry one optional value per row.
///
/// 向量化求值中的字符串实参：常量一份值，列则每行一个可选值。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StringParam {
    Constant(Option<String>),
    Column(Vec<Option<String>>),
}

impl StringParam {
    /// 是否为常量实参（可触发 pattern 缓存）。
    pub fn is_constant(&self) -> bool {
        matches!(self, Self::Constant(_))
    }

    /// Go's `buildStringParam` reports a strict constant NULL before the
    /// evaluator inspects later arguments, including escape.
    fn is_constant_null(&self) -> bool {
        matches!(self, Self::Constant(None))
    }

    /// 按行取字符串；常量忽略行号，列越界视为 NULL。
    fn value(&self, row: usize) -> Option<&str> {
        match self {
            Self::Constant(value) => value.as_deref(),
            Self::Column(values) => values.get(row).and_then(Option::as_deref),
        }
    }

    /// 列长度必须与批大小一致，否则报参数错误。
    fn validate_len(&self, row_count: usize) -> Result<(), ExpressionError> {
        if let Self::Column(values) = self
            && values.len() != row_count
        {
            return Err(ExpressionError::InvalidArgument(format!(
                "column has {} rows, expected {row_count}",
                values.len()
            )));
        }
        Ok(())
    }
}

/// Go requires the third ILIKE argument to be ConstStrict in vector mode.
///
/// 向量模式下第三参 escape 必须是严格常量；`Column` 变体仅用于报错。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EscapeParam {
    Constant(Option<i64>),
    Column,
}

/// Lower ASCII letters in every non-NULL string without touching non-ASCII
/// bytes. A copied vector should be passed here so upstream columns stay intact.
///
/// 就地折叠列中每个非 NULL 字符串的 ASCII 字母；调用方应传入副本以免污染上游列。
pub fn LowerAlphaASCII(column: &mut [Option<String>]) {
    for value in column.iter_mut().flatten() {
        let mut bytes = value.as_bytes().to_vec();
        stringutil::string_util::LowerOneString(&mut bytes);
        *value = String::from_utf8(bytes).expect("ASCII folding preserves UTF-8");
    }
}

/// Lower an ILIKE pattern column while protecting the configured escape byte.
/// The returned byte is the actual escape used after folding, matching Go's
/// lower-case-escape-to-upper-case rule.
///
/// 折叠 pattern 列并保护 escape 字节；返回折叠后实际使用的 escape。
pub fn LowerAlphaASCIIExcludeEscapeChar(column: &mut [Option<String>], excluded_char: i64) -> i64 {
    let mut actual = excluded_char as u8;
    for value in column.iter_mut().flatten() {
        let mut bytes = value.as_bytes().to_vec();
        actual = stringutil::string_util::LowerOneStringExcludeEscapeChar(
            &mut bytes,
            excluded_char as u8,
        );
        *value = String::from_utf8(bytes).expect("ASCII folding preserves UTF-8");
    }
    i64::from(actual)
}

impl IlikeSig {
    /// 声明 ILIKE 支持向量化求值。
    pub fn vectorized(&self) -> bool {
        true
    }

    /// Evaluate all four Go vector paths: vec/vec, const/vec, vec/const with
    /// memorization, and const/const. NULL bitmaps are merged row by row.
    ///
    /// 覆盖 Go 四条向量路径；escape 为列则报错，为 NULL 则整列 NULL。
    pub fn vec_eval_int(
        &self,
        expression: &StringParam,
        pattern: &StringParam,
        escape: EscapeParam,
        row_count: usize,
    ) -> Result<Vec<Option<i64>>, ExpressionError> {
        // `buildStringParam` returns immediately for either strict constant
        // NULL in Go, so this must precede escape validation as well.
        if expression.is_constant_null() || pattern.is_constant_null() {
            return Ok(vec![None; row_count]);
        }
        expression.validate_len(row_count)?;
        pattern.validate_len(row_count)?;
        // escape 必须常量；NULL escape → 整批 NULL。
        let escape = match escape {
            EscapeParam::Column => return Err(ExpressionError::EscapeMustBeConstant),
            EscapeParam::Constant(None) => return Ok(vec![None; row_count]),
            EscapeParam::Constant(Some(escape)) => escape,
        };

        // 常量 pattern 可缓存编译结果。
        let cacheable = pattern.is_constant();
        let mut result = Vec::with_capacity(row_count);
        for row in 0..row_count {
            let (Some(value), Some(pattern)) = (expression.value(row), pattern.value(row)) else {
                result.push(None);
                continue;
            };
            result.push(Some(i64::from(
                self.matches(value, pattern, escape, cacheable),
            )));
        }
        Ok(result)
    }
}

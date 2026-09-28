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

// 类型系统错误码的单元测试。
//
// 遍历 `types` 包中常见的类型转换/截断/溢出等 terror 错误，
// 校验各自都能正确映射为 SQL 错误码（`ToSQLError`）。

use crate::metadata::*;

/// 校验类型相关 terror 错误均能转换为带正确 Code 的 SQLError。
#[test]
fn test_error() {
    let errors = [
        &**ErrInvalidDefault,
        &**ErrDataTooLong,
        &**ErrIllegalValueForType,
        &**ErrTruncated,
        &**ErrOverflow,
        &**ErrDivByZero,
        &**ErrTooBigDisplayWidth,
        &**ErrTooBigFieldLength,
        &**ErrTooBigSet,
        &**ErrTooBigScale,
        &**ErrTooBigPrecision,
        &**ErrBadNumber,
        &**ErrInvalidFieldSize,
        &**ErrMBiggerThanD,
        &**ErrWarnDataOutOfRange,
        &**ErrDuplicatedValueInType,
        &**ErrDatetimeFunctionOverflow,
        &**ErrCastAsSignedOverflow,
        &**ErrCastNegIntAsUnsigned,
        &**ErrInvalidYearFormat,
        &**ErrTruncatedWrongVal,
        &**ErrInvalidWeekModeFormat,
        &**ErrWrongValue,
    ];
    for error in errors {
        let sql_error = terror::ToSQLError(error);
        assert_eq!(sql_error.Code, error.Code() as u16, "error: {error:?}");
    }
}

/// Go 通过赋值直接复用 parser/types 的包级错误；Rust 也必须保留同一实例。
#[test]
fn invalid_default_reuses_parser_types_error() {
    assert!(std::ptr::eq(
        &**ErrInvalidDefault,
        &**parser_types::types::ErrInvalidDefault,
    ));
}

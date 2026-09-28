// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 预处理语句（prepared statement）参数值访问。
//
// 对应 Go `param.go`：表达式求值时按位置读取 `?` 占位符绑定值。
// 索引越界统一映射为 `ParamError`，与 Go 哨兵错误语义一致。

use std::fmt;

use crate::types;

/// 对应 Go ErrParamIndexExceedParamCounts。
pub const ERR_PARAM_INDEX_EXCEED_PARAM_COUNTS: &str = "Param index exceed param counts";

/// 参数访问错误；目前仅索引超出绑定参数个数一种。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParamError {
    /// 请求的参数下标 ≥ 已绑定参数个数。
    IndexExceedsParamCount,
}

impl fmt::Display for ParamError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(ERR_PARAM_INDEX_EXCEED_PARAM_COUNTS)
    }
}

impl std::error::Error for ParamError {}

/// 与 Go 包级错误变量同名的常量别名，便于迁移对照。
pub const ErrParamIndexExceedParamCounts: ParamError = ParamError::IndexExceedsParamCount;

/// 对应 Go ParamValues：通过位置只读取得参数值。
pub trait ParamValues {
    /// 按 0 起始下标取参数 Datum；越界返回 `ParamError`。
    fn GetParamValue(&self, idx: usize) -> Result<types::Datum, ParamError>;
}

/// 不含任何参数的实现；任意索引都返回同一越界错误。
pub struct EmptyParamValues;

impl ParamValues for EmptyParamValues {
    fn GetParamValue(&self, _idx: usize) -> Result<types::Datum, ParamError> {
        Err(ErrParamIndexExceedParamCounts)
    }
}

/// 对应 Go 的包级 EmptyParamValues 接口值。
pub static EMPTY_PARAM_VALUES: EmptyParamValues = EmptyParamValues;

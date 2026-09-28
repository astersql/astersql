// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 聚合工具：DISTINCT 去重检查器，以及 SUM/AVG 用的 `calculateSum` 类型提升。

use crate::*;

use std::sync::Arc;

use expression::exprctx::EvalContext;

/// Tracks the encoded argument tuples already observed by a DISTINCT
/// aggregate. Buffers are retained across calls just as in the Go version.
/// DISTINCT 聚合用的去重器：用编码后的参数元组在 MVMap 中判重。
pub struct distinctChecker {
    existing_keys: mvmap::MVMap,
    key: Vec<u8>,
    ctx: Arc<dyn EvalContext>,
}

/// 基于求值上下文创建 DISTINCT 检查器。
pub fn createDistinctChecker(ctx: Arc<dyn EvalContext>) -> distinctChecker {
    distinctChecker {
        existing_keys: mvmap::NewMVMap(),
        key: Vec::new(),
        ctx,
    }
}

impl distinctChecker {
    /// 若参数组合首次出现返回 true 并记录；重复则返回 false。
    pub fn Check(&mut self, values: Vec<types::Datum>) -> Result<bool, crate::Error> {
        self.key.clear();
        // 将 Datum 列表编码为可比较的字节键。
        match codec::EncodeValue(self.ctx.Location(), std::mem::take(&mut self.key), values) {
            Ok(key) => self.key = key,
            Err(error) => {
                if let Some(error) = self.ctx.ErrCtx().HandleError(Some(error)) {
                    return Err(expression::errors::New(error.to_string()));
                }
            }
        }
        if !self.existing_keys.Get(&self.key, Vec::new()).is_empty() {
            return Ok(false);
        }
        self.existing_keys.Put(&self.key, &[]);
        Ok(true)
    }
}

/// Adds one aggregate input using MySQL's SUM/AVG coercion rules: integer and
/// decimal inputs accumulate as DECIMAL; all other non-null values use DOUBLE.
/// 按 MySQL SUM/AVG 规则把 `value` 累加到 `sum`：整数/Decimal→DECIMAL，其余→DOUBLE。
pub fn calculateSum(
    ctx: types::Context,
    sum: types::Datum,
    value: types::Datum,
) -> Result<types::Datum, crate::Error> {
    let data = match value.Kind() {
        types::KindNull => types::Datum::default(),
        types::KindInt64 | types::KindUint64 => types::NewDecimalDatum(value.ToDecimal(ctx)?),
        types::KindMysqlDecimal => value,
        _ => types::NewFloat64Datum(value.ToFloat64(ctx)?),
    };

    if data.IsNull() {
        return Ok(sum);
    }
    match sum.Kind() {
        types::KindNull => Ok(data),
        types::KindFloat64 | types::KindMysqlDecimal => types::ComputePlus(sum, data)
            .map_err(|error| expression::errors::New(error.to_string())),
        kind => Err(expression::errors::New(format!(
            "invalid value {kind:?} for aggregate"
        ))),
    }
}

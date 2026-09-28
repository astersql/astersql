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

// Datum 加法求值：按 Kind 分派 int/uint/float/decimal 运算。
//
// 对齐 Go `datum_eval.go` 的 `ComputePlus`；仅支持同类或整数混合加法，
// 其他类型组合返回非法二元操作错误。溢出映射为 SharedError。

// 本文件对照 pkg/types/datum_eval.go，实现 Datum 加法求值的类型分派。

use super::overflow::{AddInt64, AddUint64, OverflowError};
use crate::errors;

// ComputePlus computes the result of a+b.
// Go 只处理 int64/uint64/float64/decimal 的同类或整数混合加法，其他组合走 InvOp2。
/// 计算 a+b：覆盖 int64/uint64/float64/decimal 及有符号与无符号混合。
pub fn ComputePlus(a: Datum, b: Datum) -> Result<Datum, errors::SharedError> {
    let mut d = Datum::default();
    match a.Kind() {
        KindInt64 => match b.Kind() {
            KindInt64 => {
                let r = AddInt64(a.GetInt64(), b.GetInt64()).map_err(trace_overflow)?;
                d.SetInt64(r);
                return Ok(d);
            }
            KindUint64 => {
                let r = add_integer(b.GetUint64(), a.GetInt64())?;
                d.SetUint64(r);
                return Ok(d);
            }
            _ => {}
        },
        KindUint64 => match b.Kind() {
            KindInt64 => {
                let r = add_integer(a.GetUint64(), b.GetInt64())?;
                d.SetUint64(r);
                return Ok(d);
            }
            KindUint64 => {
                let r = AddUint64(a.GetUint64(), b.GetUint64()).map_err(trace_overflow)?;
                d.SetUint64(r);
                return Ok(d);
            }
            _ => {}
        },
        KindFloat64 => {
            if b.Kind() == KindFloat64 {
                let r = a.GetFloat64() + b.GetFloat64();
                d.SetFloat64(r);
                return Ok(d);
            }
        }
        KindMysqlDecimal => {
            if b.Kind() == KindMysqlDecimal {
                let mut r = MyDecimal::default();
                DecimalAdd(&a.GetMysqlDecimal(), &b.GetMysqlDecimal(), &mut r)?;
                d.SetMysqlDecimal(r);
                d.SetFrac(std::cmp::max(a.Frac(), b.Frac()));
                return Ok(d);
            }
        }
        _ => {}
    }

    // Go 在未覆盖的类型组合上调用 InvOp2 生成二元操作非法错误。
    Err(errors::New(format!(
        "Invalid operation: {} + {} (mismatched kinds {} and {})",
        a.GetValue(),
        b.GetValue(),
        a.Kind(),
        b.Kind()
    )))
}

/// 将溢出错误包装为 SharedError。
fn trace_overflow(err: OverflowError) -> errors::SharedError {
    errors::New(err.to_string())
}

// add_integer 对应 Go AddInteger，使用 unsigned_abs 正确覆盖 MinInt64。
/// 无符号与有符号整数相加；负量用 unsigned_abs，正确处理 MinInt64。
fn add_integer(a: u64, b: i64) -> Result<u64, errors::SharedError> {
    if b >= 0 {
        return AddUint64(a, b as u64).map_err(trace_overflow);
    }

    // 负加数：若绝对值大于无符号操作数则溢出 BIGINT UNSIGNED
    let magnitude = b.unsigned_abs();
    if magnitude > a {
        return Err(errors::New(format!(
            "value ({a}, {b}) overflows BIGINT UNSIGNED"
        )));
    }
    Ok(a - magnitude)
}

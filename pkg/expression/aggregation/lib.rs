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
// aggregation 模块入口：聚合与窗口聚合相关子模块导出。
//
// 子模块覆盖运行时聚合器、AggFuncDesc/WindowFuncDesc、EXPLAIN 与 tipb 下推转换。
// `types` 门面把 datum/decimal/field/overflow 拼成与 Go `types` 包兼容的统一入口。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

mod agg_to_pb;
mod aggregation;
mod avg;
mod base_func;
mod bit_and;
mod bit_or;
mod bit_xor;
mod concat;
mod count;
mod descriptor;
mod explain;
mod first_row;
mod max_min;
mod max_min_count;
mod sum;
mod sum_int;
mod util;
mod window_func;

pub use agg_to_pb::*;
pub use aggregation::*;
pub use avg::*;
pub use base_func::*;
pub use bit_and::*;
pub use bit_or::*;
pub use bit_xor::*;
pub use concat::*;
pub use count::*;
pub use descriptor::*;
pub use explain::*;
pub use first_row::*;
pub use max_min::*;
pub use max_min_count::*;
pub use sum::*;
pub use sum_int::*;
pub use util::*;
pub use window_func::*;

pub use expression::{ast, exprctx, mysql};
pub use planner_util as plannerutil;
pub use planner_util as util_dependency;

/// Aggregation uses the formal datum, decimal, field and overflow compile
/// units as one Go-compatible `types` surface.
/// 聚合用的类型门面：统一导出 Datum、Decimal、求值类型与整数溢出加法。
pub mod types {
    pub use datum_dependency::*;
    pub use decimal_dependency::mydecimal::{DecimalDiv, NewDecFromInt, NewDecFromStringForTest};
    pub use field_dependency::{
        ETDatetime, ETDecimal, ETDuration, ETInt, ETJson, ETReal, ETString, ETTimestamp,
        ETVectorFloat32, EvalType,
    };
    pub use file_dependency::overflow::{AddInt64, AddUint64};

    /// 将字段类型标记为 binary 字符集/排序规则（如位运算聚合结果）。
    pub fn SetBinChsClnFlag(field_type: &mut FieldType) {
        field_type.SetCharset("binary".to_owned());
        field_type.SetCollate("binary".to_owned());
        field_type.AddFlag(crate::mysql::BinaryFlag);
    }

    /// 同种 Datum 相加：浮点、DECIMAL、有符号/无符号整数；混合类型报错。
    pub fn ComputePlus(left: Datum, right: Datum) -> Result<Datum, errors::Error> {
        match (left.Kind(), right.Kind()) {
            (KindFloat64, KindFloat64) => {
                Ok(NewFloat64Datum(left.GetFloat64() + right.GetFloat64()))
            }
            (KindMysqlDecimal, KindMysqlDecimal) => {
                let mut result = MyDecimal::default();
                // DECIMAL 加法写入 result，再包装为 Datum。
                DecimalAdd(
                    &left.GetMysqlDecimal(),
                    &right.GetMysqlDecimal(),
                    &mut result,
                )?;
                let mut datum = NewDecimalDatum(result);
                datum.SetFrac(std::cmp::max(left.Frac(), right.Frac()));
                Ok(datum)
            }
            (KindInt64, KindInt64) => AddInt64(left.GetInt64(), right.GetInt64())
                .map(NewIntDatum)
                .map_err(|error| errors::New(error.to_string())),
            (KindInt64, KindUint64) => {
                add_integer(right.GetUint64(), left.GetInt64()).map(NewUintDatum)
            }
            (KindUint64, KindInt64) => {
                add_integer(left.GetUint64(), right.GetInt64()).map(NewUintDatum)
            }
            (KindUint64, KindUint64) => AddUint64(left.GetUint64(), right.GetUint64())
                .map(NewUintDatum)
                .map_err(|error| errors::New(error.to_string())),
            _ => Err(errors::New("invalid mixed datum addition")),
        }
    }

    /// Go `AddInteger`: add a signed delta to an unsigned integer with
    /// BIGINT UNSIGNED overflow/underflow checks.
    fn add_integer(unsigned: u64, signed: i64) -> Result<u64, errors::Error> {
        if signed >= 0 {
            return AddUint64(unsigned, signed as u64)
                .map_err(|error| errors::New(error.to_string()));
        }

        unsigned
            .checked_sub(signed.unsigned_abs())
            .ok_or_else(|| errors::New("BIGINT UNSIGNED value is out of range"))
    }
}

/// 本 crate 错误类型别名，与 expression 包共用。
pub type Error = expression::Error;

#[cfg(test)]
#[path = "agg_to_pb_test.rs"]
mod agg_to_pb_test;
#[cfg(test)]
#[path = "aggregation_aster_unit_test.rs"]
mod aggregation_aster_unit_test;
#[cfg(test)]
#[path = "aggregation_test.rs"]
mod aggregation_test;
#[cfg(test)]
#[path = "avg_test.rs"]
mod avg_test;
#[cfg(test)]
#[path = "base_func_test.rs"]
mod base_func_test;
#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
#[cfg(test)]
#[path = "bit_xor_test.rs"]
mod bit_xor_test;
#[cfg(test)]
#[path = "concat_test.rs"]
mod concat_test;
#[cfg(test)]
#[path = "count_test.rs"]
mod count_test;
#[cfg(test)]
#[path = "first_row_test.rs"]
mod first_row_test;
#[cfg(test)]
#[path = "go_merge_44_test.rs"]
mod go_merge_44_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "util_test.rs"]
mod util_test;
#[cfg(test)]
#[path = "window_func_test.rs"]
mod window_func_test;

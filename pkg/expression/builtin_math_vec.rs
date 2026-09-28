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
// 数学内建函数的向量化（列式）求值实现，对应 Go `builtin_math_vec.go`。
//
// 对 Chunk 中每一行批量计算，写入 Column；处理 NULL 合并、定义域告警与溢出。
// 多数签名由宏生成结构体，再实现 `vecEval*`；CONV 保留实现但 `vectorized()` 为 false。

use std::f64::consts::PI;

use types_dependency::decimal::mydecimal::{
    DecimalAdd, DecimalError, DecimalSub, ModeHalfUp, ModeTruncate, MyDecimal,
};

use crate::legacy_vectorized_runtime::{
    Chunk, Column, EvalContext, EvalError, ExprRef, MathBase, Result,
};
use types_dependency::decimal::mydecimal::{NewDecFromInt, NewDecFromUint};

/// 为可向量化的数学签名生成统一包装：持有 MathBase，并声明 vectorized=true。
macro_rules! define_signature {
    ($($name:ident),+ $(,)?) => {$(
        #[derive(Clone)]
        pub struct $name {
            base: MathBase,
        }

        impl $name {
            pub fn new(args: Vec<ExprRef>) -> Self {
                Self { base: MathBase::new(args) }
            }

            pub fn from_base(base: MathBase) -> Self {
                Self { base }
            }

            pub fn vectorized(&self) -> bool {
                true
            }
        }
    )+};
}

define_signature!(
    builtinLog1ArgSig,
    builtinLog2Sig,
    builtinLog10Sig,
    builtinSqrtSig,
    builtinAcosSig,
    builtinAsinSig,
    builtinAtan1ArgSig,
    builtinAtan2ArgsSig,
    builtinCosSig,
    builtinCotSig,
    builtinDegreesSig,
    builtinExpSig,
    builtinRadiansSig,
    builtinSinSig,
    builtinTanSig,
    builtinAbsDecSig,
    builtinRoundDecSig,
    builtinPowSig,
    builtinFloorRealSig,
    builtinLog2ArgsSig,
    builtinCeilRealSig,
    builtinRoundRealSig,
    builtinRoundWithFracRealSig,
    builtinTruncateRealSig,
    builtinAbsRealSig,
    builtinAbsIntSig,
    builtinRoundIntSig,
    builtinRoundWithFracIntSig,
    builtinCRC32Sig,
    builtinPISig,
    builtinRandSig,
    builtinRandWithSeedFirstGenSig,
    builtinCeilIntToDecSig,
    builtinTruncateIntSig,
    builtinTruncateUintSig,
    builtinCeilDecToDecSig,
    builtinFloorDecToDecSig,
    builtinTruncateDecimalSig,
    builtinRoundWithFracDecSig,
    builtinFloorIntToDecSig,
    builtinSignSig,
    builtinAbsUIntSig,
    builtinCeilDecToIntSig,
    builtinCeilIntToIntSig,
    builtinFloorIntToIntSig,
    builtinFloorDecToIntSig,
);

/// 一元实数运算种类，供共享的 `unary_real` 内核按行分发。
#[derive(Clone, Copy)]
enum UnaryRealOp {
    Ln,
    Log2,
    Log10,
    Sqrt,
    Acos,
    Asin,
    Atan,
    Cos,
    Cot,
    Degrees,
    Exp,
    Radians,
    Sin,
    Tan,
    Floor,
    Ceil,
    Round,
    Abs,
}

/// 按小数位四舍五入（ties to even）；极端位数用 Inf/0 作为缩放因子。
fn round_float(value: f64, decimals: i64) -> f64 {
    let shift = if decimals > i64::from(i32::MAX) {
        f64::INFINITY
    } else if decimals < i64::from(i32::MIN) {
        0.0
    } else {
        10_f64.powi(decimals as i32)
    };
    let shifted = value * shift;
    if shifted.is_infinite() {
        return value;
    }
    let rounded = shifted.round_ties_even() / shift;
    if rounded.is_nan() { 0.0 } else { rounded }
}

/// 按小数位向零截断；保留 NaN，极端负位数得到 0。
fn truncate_float(value: f64, decimals: i64) -> f64 {
    let shift = if decimals > i64::from(i32::MAX) {
        f64::INFINITY
    } else if decimals < i64::from(i32::MIN) {
        0.0
    } else {
        10_f64.powi(decimals as i32)
    };
    let shifted = value * shift;
    if shifted.is_infinite() || shifted.is_nan() {
        return value;
    }
    if shift == 0.0 {
        return if value.is_nan() { value } else { 0.0 };
    }
    shifted.trunc() / shift
}

/// 一元实数向量化内核：先 VecEvalReal，再按 op 原地改写或置 NULL/报错。
fn unary_real(
    base: &MathBase,
    ctx: &EvalContext,
    input: &Chunk,
    result: &mut Column,
    op: UnaryRealOp,
) -> Result<()> {
    base.args[0].VecEvalReal(ctx, input, result)?;
    for row in 0..result.Float64s().len() {
        if result.IsNull(row) {
            continue;
        }
        let value = result.Float64s()[row];
        let evaluated = match op {
            // 对数定义域：非正数 → NULL + 告警（与 Go 一致）。
            UnaryRealOp::Ln | UnaryRealOp::Log2 | UnaryRealOp::Log10 if value <= 0.0 => {
                ctx.append_warning("invalid argument for logarithm");
                None
            }
            UnaryRealOp::Sqrt if value < 0.0 => None,
            // Match Go's `value < -1 || value > 1` exactly: NaN satisfies
            // neither comparison and therefore remains a non-NULL NaN.
            UnaryRealOp::Acos | UnaryRealOp::Asin if value < -1.0 || value > 1.0 => None,
            UnaryRealOp::Ln => Some(value.ln()),
            UnaryRealOp::Log2 => Some(value.log2()),
            UnaryRealOp::Log10 => Some(value.log10()),
            UnaryRealOp::Sqrt => Some(value.sqrt()),
            UnaryRealOp::Acos => Some(value.acos()),
            UnaryRealOp::Asin => Some(value.asin()),
            UnaryRealOp::Atan => Some(value.atan()),
            UnaryRealOp::Cos => Some(value.cos()),
            UnaryRealOp::Degrees => Some(value * 180.0 / PI),
            UnaryRealOp::Radians => Some(value * (PI / 180.0)),
            UnaryRealOp::Sin => Some(value.sin()),
            UnaryRealOp::Tan => Some(value.tan()),
            UnaryRealOp::Floor => Some(value.floor()),
            UnaryRealOp::Ceil => Some(value.ceil()),
            UnaryRealOp::Round => Some(round_float(value, 0)),
            UnaryRealOp::Abs => Some(value.abs()),
            UnaryRealOp::Exp => {
                let output = value.exp();
                if output.is_infinite() || output.is_nan() {
                    return Err(EvalError::DoubleOverflow(format!("exp({value})")));
                }
                Some(output)
            }
            UnaryRealOp::Cot => {
                let tangent = value.tan();
                if tangent == 0.0 {
                    return Err(EvalError::DoubleOverflow(format!("cot({value})")));
                }
                let output = 1.0 / tangent;
                // Go leaves the original cell untouched for a non-zero tangent
                // whose reciprocal is Inf/NaN, then continues.
                if output.is_infinite() || output.is_nan() {
                    continue;
                }
                Some(output)
            }
        };
        match evaluated {
            Some(value) => result.Float64sMut()[row] = value,
            None => result.SetNull(row, true),
        }
    }
    Ok(())
}

/// 将签名类型绑定到具体 UnaryRealOp。
macro_rules! unary_real_impl {
    ($name:ident, $op:ident) => {
        impl $name {
            pub fn vecEvalReal(
                &self,
                ctx: &EvalContext,
                input: &Chunk,
                result: &mut Column,
            ) -> Result<()> {
                unary_real(&self.base, ctx, input, result, UnaryRealOp::$op)
            }
        }
    };
}

unary_real_impl!(builtinLog1ArgSig, Ln);
unary_real_impl!(builtinLog2Sig, Log2);
unary_real_impl!(builtinLog10Sig, Log10);
unary_real_impl!(builtinSqrtSig, Sqrt);
unary_real_impl!(builtinAcosSig, Acos);
unary_real_impl!(builtinAsinSig, Asin);
unary_real_impl!(builtinAtan1ArgSig, Atan);
unary_real_impl!(builtinCosSig, Cos);
unary_real_impl!(builtinCotSig, Cot);
unary_real_impl!(builtinDegreesSig, Degrees);
unary_real_impl!(builtinExpSig, Exp);
unary_real_impl!(builtinRadiansSig, Radians);
unary_real_impl!(builtinSinSig, Sin);
unary_real_impl!(builtinTanSig, Tan);
unary_real_impl!(builtinFloorRealSig, Floor);
unary_real_impl!(builtinCeilRealSig, Ceil);
unary_real_impl!(builtinRoundRealSig, Round);
unary_real_impl!(builtinAbsRealSig, Abs);

impl builtinAtan2ArgsSig {
    /// 向量化 ATAN2：合并两侧 NULL 后按行计算 atan2。
    pub fn vecEvalReal(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        self.base.args[0].VecEvalReal(ctx, input, result)?;
        let mut second = Column::default();
        self.base.args[1].VecEvalReal(ctx, input, &mut second)?;
        result.MergeNulls(&[&second]);
        for row in 0..input.NumRows() {
            if !result.IsNull(row) {
                result.Float64sMut()[row] = result.Float64s()[row].atan2(second.Float64s()[row]);
            }
        }
        Ok(())
    }
}

impl builtinAbsDecSig {
    /// 向量化 DECIMAL ABS：仅对负值做 `0 - x`。
    pub fn vecEvalDecimal(
        &self,
        ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        self.base.args[0].VecEvalDecimal(ctx, input, result)?;
        let zero = MyDecimal::default();
        for row in 0..input.NumRows() {
            if result.IsNull(row) || !result.Decimals()[row].IsNegative() {
                continue;
            }
            let value = result.Decimals()[row].clone();
            let mut output = MyDecimal::default();
            DecimalSub(&zero, &value, &mut output)?;
            result.DecimalsMut()[row] = output;
        }
        Ok(())
    }
}

impl builtinRoundDecSig {
    /// 向量化 ROUND(DECIMAL) 到 0 位，半入模式。
    pub fn vecEvalDecimal(
        &self,
        ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        self.base.args[0].VecEvalDecimal(ctx, input, result)?;
        for row in 0..input.NumRows() {
            if result.IsNull(row) {
                continue;
            }
            let mut output = MyDecimal::default();
            result.Decimals()[row].Round(&mut output, 0, ModeHalfUp)?;
            result.DecimalsMut()[row] = output;
        }
        Ok(())
    }
}

impl builtinPowSig {
    /// 向量化 POW；任一非有限结果立即返回 DOUBLE 溢出。
    pub fn vecEvalReal(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        let mut first = Column::default();
        self.base.args[0].VecEvalReal(ctx, input, &mut first)?;
        self.base.args[1].VecEvalReal(ctx, input, result)?;
        result.MergeNulls(&[&first]);
        for row in 0..input.NumRows() {
            if result.IsNull(row) {
                continue;
            }
            let x = first.Float64s()[row];
            let y = result.Float64s()[row];
            let output = x.powf(y);
            if !output.is_finite() {
                return Err(EvalError::DoubleOverflow(format!("pow({x}, {y})")));
            }
            result.Float64sMut()[row] = output;
        }
        Ok(())
    }
}

impl builtinLog2ArgsSig {
    /// 向量化双参数 LOG；非法底/真数置 NULL 并告警，但仍写入 ln 比值（与 Go 顺序一致处保留）。
    pub fn vecEvalReal(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        self.base.args[0].VecEvalReal(ctx, input, result)?;
        let mut argument = Column::default();
        self.base.args[1].VecEvalReal(ctx, input, &mut argument)?;
        result.MergeNulls(&[&argument]);
        for row in 0..input.NumRows() {
            if result.IsNull(row) {
                continue;
            }
            let base = result.Float64s()[row];
            let value = argument.Float64s()[row];
            if base <= 0.0 || base == 1.0 || value <= 0.0 {
                ctx.append_warning("invalid argument for logarithm");
                result.SetNull(row, true);
            }
            result.Float64sMut()[row] = value.ln() / base.ln();
        }
        Ok(())
    }
}

impl builtinRoundWithFracRealSig {
    /// 向量化 ROUND(REAL, d)。
    pub fn vecEvalReal(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        self.base.args[0].VecEvalReal(ctx, input, result)?;
        let mut fraction = Column::default();
        self.base.args[1].VecEvalInt(ctx, input, &mut fraction)?;
        result.MergeNulls(&[&fraction]);
        for row in 0..input.NumRows() {
            if !result.IsNull(row) {
                result.Float64sMut()[row] =
                    round_float(result.Float64s()[row], fraction.Int64s()[row]);
            }
        }
        Ok(())
    }
}

impl builtinTruncateRealSig {
    /// 向量化 TRUNCATE(REAL, d)。
    pub fn vecEvalReal(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        self.base.args[0].VecEvalReal(ctx, input, result)?;
        let mut fraction = Column::default();
        self.base.args[1].VecEvalInt(ctx, input, &mut fraction)?;
        result.MergeNulls(&[&fraction]);
        for row in 0..input.NumRows() {
            if !result.IsNull(row) {
                result.Float64sMut()[row] =
                    truncate_float(result.Float64s()[row], fraction.Int64s()[row]);
            }
        }
        Ok(())
    }
}

impl builtinAbsIntSig {
    /// 向量化 ABS(INT)；遇到 i64::MIN 报 BIGINT 溢出。
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        self.base.args[0].VecEvalInt(ctx, input, result)?;
        for row in 0..input.NumRows() {
            if result.IsNull(row) {
                continue;
            }
            let value = result.Int64s()[row];
            if value == i64::MIN {
                return Err(EvalError::BigIntOverflow(format!("abs({value})")));
            }
            if value < 0 {
                result.Int64sMut()[row] = -value;
            }
        }
        Ok(())
    }
}

impl builtinRoundIntSig {
    /// ROUND(整数) 无小数位：直接透传整数列。
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        self.base.args[0].VecEvalInt(ctx, input, result)
    }
}

impl builtinRoundWithFracIntSig {
    /// 向量化 ROUND(INT, d)，经 f64 舍入。
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        self.base.args[0].VecEvalInt(ctx, input, result)?;
        let mut fraction = Column::default();
        self.base.args[1].VecEvalInt(ctx, input, &mut fraction)?;
        result.MergeNulls(&[&fraction]);
        for row in 0..input.NumRows() {
            if !result.IsNull(row) {
                result.Int64sMut()[row] =
                    round_float(result.Int64s()[row] as f64, fraction.Int64s()[row]) as i64;
            }
        }
        Ok(())
    }
}

impl builtinCRC32Sig {
    /// 向量化 CRC32：对字符串字节计算校验和。
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        let mut values = Column::default();
        self.base.args[0].VecEvalString(ctx, input, &mut values)?;
        result.ResizeInt64(input.NumRows(), false);
        result.MergeNulls(&[&values]);
        for row in 0..input.NumRows() {
            if !values.IsNull(row) {
                result.Int64sMut()[row] = i64::from(crc32fast::hash(values.GetBytes(row)));
            }
        }
        Ok(())
    }
}

impl builtinPISig {
    /// 向量化 PI：整列填充圆周率常量。
    pub fn vecEvalReal(
        &self,
        _ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        result.ResizeFloat64(input.NumRows(), false);
        result.Float64sMut().fill(PI);
        Ok(())
    }
}

impl builtinRandSig {
    /// 使用固定种子构造会话式 RAND 签名（测试/确定性路径）。
    pub fn with_seed(seed: i64) -> Self {
        Self::from_base(MathBase::with_seed(Vec::new(), seed))
    }

    /// 向量化 RAND：按行推进共享 MysqlRng。
    pub fn vecEvalReal(
        &self,
        _ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        result.ResizeFloat64(input.NumRows(), false);
        for value in result.Float64sMut() {
            *value = self.base.mysql_rng.Gen();
        }
        Ok(())
    }
}

impl builtinRandWithSeedFirstGenSig {
    /// 每行用该行种子新建 RNG 并取首值；种子 NULL 视为 0。
    pub fn vecEvalReal(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        let mut seeds = Column::default();
        self.base.args[0].VecEvalInt(ctx, input, &mut seeds)?;
        result.ResizeFloat64(input.NumRows(), false);
        for row in 0..input.NumRows() {
            let seed = if seeds.IsNull(row) {
                0
            } else {
                seeds.Int64s()[row]
            };
            result.Float64sMut()[row] = mathutil::NewWithSeed(seed).Gen();
        }
        Ok(())
    }
}

/// 将整数列提升为 DECIMAL 列，尊重参数 unsigned 标志。
fn int_to_decimal(
    base: &MathBase,
    ctx: &EvalContext,
    input: &Chunk,
    result: &mut Column,
) -> Result<()> {
    let mut values = Column::default();
    base.args[0].VecEvalInt(ctx, input, &mut values)?;
    result.ResizeDecimal(input.NumRows(), false);
    result.MergeNulls(&[&values]);
    let unsigned = base.args[0].GetType(ctx).unsigned;
    for row in 0..input.NumRows() {
        if result.IsNull(row) {
            continue;
        }
        let value = values.Int64s()[row];
        result.DecimalsMut()[row] = if unsigned || value >= 0 {
            NewDecFromUint(value as u64)
        } else {
            NewDecFromInt(value)
        };
    }
    Ok(())
}

impl builtinCeilIntToDecSig {
    pub fn vecEvalDecimal(
        &self,
        ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        int_to_decimal(&self.base, ctx, input, result)
    }
}

impl builtinFloorIntToDecSig {
    pub fn vecEvalDecimal(
        &self,
        ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        int_to_decimal(&self.base, ctx, input, result)
    }
}

/// 整数 TRUNCATE 共享实现；小数位参数为 unsigned 时直接返回。
fn truncate_integer(
    base: &MathBase,
    ctx: &EvalContext,
    input: &Chunk,
    result: &mut Column,
    value_unsigned: bool,
) -> Result<()> {
    base.args[0].VecEvalInt(ctx, input, result)?;
    let mut fraction = Column::default();
    base.args[1].VecEvalInt(ctx, input, &mut fraction)?;
    result.MergeNulls(&[&fraction]);
    // 无符号小数位 ⇒ 视为非负，截断无意义，保持原值。
    if base.args[1].GetType(ctx).unsigned {
        return Ok(());
    }
    for row in 0..input.NumRows() {
        if result.IsNull(row) || fraction.Int64s()[row] >= 0 {
            continue;
        }
        let frac = fraction.Int64s()[row];
        if frac == i64::MIN {
            result.Int64sMut()[row] = 0;
            continue;
        }
        let places = (-frac) as u32;
        if value_unsigned {
            if places >= 20 {
                result.Int64sMut()[row] = 0;
            } else {
                let shift = 10_u64.pow(places);
                let value = result.Int64s()[row] as u64;
                result.Int64sMut()[row] = (value / shift * shift) as i64;
            }
        } else if places >= 19 {
            result.Int64sMut()[row] = 0;
        } else {
            let shift = 10_i64.pow(places);
            let value = result.Int64s()[row];
            result.Int64sMut()[row] = value / shift * shift;
        }
    }
    Ok(())
}

impl builtinTruncateIntSig {
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        truncate_integer(&self.base, ctx, input, result, false)
    }
}

impl builtinTruncateUintSig {
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        truncate_integer(&self.base, ctx, input, result, true)
    }
}

impl builtinCeilDecToDecSig {
    /// 向量化 CEIL(DECIMAL→DECIMAL)：正数有小数则截断后加一。
    pub fn vecEvalDecimal(
        &self,
        ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        self.base.args[0].VecEvalDecimal(ctx, input, result)?;
        for row in 0..input.NumRows() {
            if result.IsNull(row) {
                continue;
            }
            let value = result.Decimals()[row].clone();
            let mut rounded = MyDecimal::default();
            value.Round(&mut rounded, 0, ModeTruncate)?;
            // 向正无穷：非负且截断改变了值时 +1。
            if !value.IsNegative() && rounded.Compare(&value) != 0 {
                let before = rounded.clone();
                DecimalAdd(&before, &NewDecFromInt(1), &mut rounded)?;
            }
            result.DecimalsMut()[row] = rounded;
        }
        Ok(())
    }
}

impl builtinFloorDecToDecSig {
    /// 向量化 FLOOR(DECIMAL→DECIMAL)：负数有小数则截断后减一。
    pub fn vecEvalDecimal(
        &self,
        ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        self.base.args[0].VecEvalDecimal(ctx, input, result)?;
        for row in 0..input.NumRows() {
            if result.IsNull(row) {
                continue;
            }
            let value = result.Decimals()[row].clone();
            let mut rounded = MyDecimal::default();
            value.Round(&mut rounded, 0, ModeTruncate)?;
            // 向负无穷：负值且截断改变了值时 -1。
            if value.IsNegative() && rounded.Compare(&value) != 0 {
                let before = rounded.clone();
                DecimalSub(&before, &NewDecFromInt(1), &mut rounded)?;
            }
            result.DecimalsMut()[row] = rounded;
        }
        Ok(())
    }
}

impl builtinTruncateDecimalSig {
    /// 携带返回类型 decimal 位数，限制 TRUNCATE 有效位数。
    pub fn with_ret_decimal(args: Vec<ExprRef>, decimal: i32) -> Self {
        Self::from_base(MathBase::new(args).with_ret_decimal(decimal))
    }

    /// 向量化 TRUNCATE(DECIMAL, d)。
    pub fn vecEvalDecimal(
        &self,
        ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        self.base.args[0].VecEvalDecimal(ctx, input, result)?;
        let mut fraction = Column::default();
        self.base.args[1].VecEvalInt(ctx, input, &mut fraction)?;
        result.MergeNulls(&[&fraction]);
        for row in 0..input.NumRows() {
            if result.IsNull(row) {
                continue;
            }
            let digits = fraction.Int64s()[row].min(i64::from(self.base.ret_decimal));
            let mut output = MyDecimal::default();
            result.Decimals()[row].Round(&mut output, digits as isize, ModeTruncate)?;
            result.DecimalsMut()[row] = output;
        }
        Ok(())
    }
}

impl builtinRoundWithFracDecSig {
    /// 携带返回类型 decimal 位数。
    pub fn with_ret_decimal(args: Vec<ExprRef>, decimal: i32) -> Self {
        Self::from_base(MathBase::new(args).with_ret_decimal(decimal))
    }

    /// 向量化 ROUND(DECIMAL, d)，半入。
    pub fn vecEvalDecimal(
        &self,
        ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        self.base.args[0].VecEvalDecimal(ctx, input, result)?;
        let mut fraction = Column::default();
        self.base.args[1].VecEvalInt(ctx, input, &mut fraction)?;
        result.MergeNulls(&[&fraction]);
        for row in 0..input.NumRows() {
            if result.IsNull(row) {
                continue;
            }
            let digits = fraction.Int64s()[row].min(i64::from(self.base.ret_decimal));
            let mut output = MyDecimal::default();
            result.Decimals()[row].Round(&mut output, digits as isize, ModeHalfUp)?;
            result.DecimalsMut()[row] = output;
        }
        Ok(())
    }
}

impl builtinSignSig {
    /// 向量化 SIGN：由实数列得到 -1/0/1。
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        let mut values = Column::default();
        self.base.args[0].VecEvalReal(ctx, input, &mut values)?;
        result.ResizeInt64(input.NumRows(), false);
        result.MergeNulls(&[&values]);
        for row in 0..input.NumRows() {
            if !result.IsNull(row) {
                result.Int64sMut()[row] = if values.Float64s()[row] > 0.0 {
                    1
                } else if values.Float64s()[row] < 0.0 {
                    -1
                } else {
                    0
                };
            }
        }
        Ok(())
    }
}

/// CONV 签名：实现仍可按列求值，但对外声明不可向量化（与 Go 临时禁用一致）。
pub struct builtinConvSig {
    base: MathBase,
}

impl builtinConvSig {
    pub fn new(args: Vec<ExprRef>) -> Self {
        Self {
            base: MathBase::new(args),
        }
    }

    pub fn vectorized(&self) -> bool {
        // Go intentionally keeps this disabled until hybrid-type vector
        // matching is fixed, while retaining the implementation below.
        false
    }

    pub fn vecEvalString(
        &self,
        ctx: &EvalContext,
        input: &Chunk,
        result: &mut Column,
    ) -> Result<()> {
        let mut text = Column::default();
        let mut from_base = Column::default();
        let mut to_base = Column::default();
        self.base.args[0].VecEvalString(ctx, input, &mut text)?;
        self.base.args[1].VecEvalInt(ctx, input, &mut from_base)?;
        self.base.args[2].VecEvalInt(ctx, input, &mut to_base)?;
        result.ReserveString(input.NumRows());
        for row in 0..input.NumRows() {
            if text.IsNull(row) || from_base.IsNull(row) || to_base.IsNull(row) {
                result.AppendNull();
                continue;
            }
            match conv(
                text.GetString(row),
                from_base.Int64s()[row],
                to_base.Int64s()[row],
            )? {
                Some(value) => result.AppendString(value),
                None => result.AppendNull(),
            }
        }
        Ok(())
    }
}

/// 截取合法进制前缀，供向量化 CONV 使用。
fn valid_prefix(text: &str, base: u32) -> &str {
    let bytes = text.as_bytes();
    let mut valid_len = 0;
    for (index, byte) in bytes.iter().copied().enumerate() {
        if index == 0 && matches!(byte, b'+' | b'-') {
            continue;
        }
        if (byte as char).to_digit(base).is_none() {
            break;
        }
        valid_len = index + 1;
    }
    if valid_len > 1 && text.starts_with('+') {
        &text[1..valid_len]
    } else {
        &text[..valid_len]
    }
}

/// 无符号整数 → 目标进制字符串。
fn format_radix(mut value: u64, base: u32) -> String {
    const DIGITS: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    if value == 0 {
        return "0".into();
    }
    let mut output = Vec::new();
    while value != 0 {
        output.push(DIGITS[(value % u64::from(base)) as usize]);
        value /= u64::from(base);
    }
    output.reverse();
    String::from_utf8(output).expect("radix digits are ASCII")
}

/// CONV 核心：处理有符号基数、前缀解析与二进制补码语义。
fn conv(text: &str, mut from_base: i64, mut to_base: i64) -> Result<Option<String>> {
    let mut signed = false;
    let mut ignore_sign = false;
    if from_base < 0 {
        let Some(absolute) = from_base.checked_abs() else {
            return Ok(None);
        };
        from_base = absolute;
        signed = true;
    }
    if to_base < 0 {
        let Some(absolute) = to_base.checked_abs() else {
            return Ok(None);
        };
        to_base = absolute;
        ignore_sign = true;
    }
    if !(2..=36).contains(&from_base) || !(2..=36).contains(&to_base) {
        return Ok(None);
    }

    let prefix = valid_prefix(text.trim(), from_base as u32);
    if prefix.is_empty() {
        return Ok(Some("0".into()));
    }
    let (negative_input, digits) = match prefix.strip_prefix('-') {
        Some(digits) => (true, digits),
        None => (false, prefix),
    };
    let mut value = u64::from_str_radix(digits, from_base as u32)
        .map_err(|_| EvalError::BigIntOverflow(digits.into()))?;
    if signed {
        if negative_input && value > (1_u64 << 63) {
            value = 1_u64 << 63;
        }
        if !negative_input && value > i64::MAX as u64 {
            value = i64::MAX as u64;
        }
    }
    if negative_input {
        value = value.wrapping_neg();
    }
    let negative = (value as i64) < 0;
    if ignore_sign && negative {
        value = value.wrapping_neg();
    }
    let mut output = format_radix(value, to_base as u32);
    if negative && ignore_sign {
        output.insert(0, '-');
    }
    Ok(Some(output))
}

impl builtinAbsUIntSig {
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        self.base.args[0].VecEvalInt(ctx, input, result)
    }
}

/// DECIMAL→整数的 CEIL/FLOOR 共享内核；`ceil` 控制截断进位方向。
fn decimal_to_int(
    base: &MathBase,
    ctx: &EvalContext,
    input: &Chunk,
    result: &mut Column,
    ceil: bool,
) -> Result<()> {
    let mut values = Column::default();
    base.args[0].VecEvalDecimal(ctx, input, &mut values)?;
    result.ResizeInt64(input.NumRows(), false);
    result.MergeNulls(&[&values]);
    for row in 0..input.NumRows() {
        if result.IsNull(row) {
            continue;
        }
        let decimal = &values.Decimals()[row];
        let (mut value, status) = decimal.ToInt();
        match status {
            Ok(()) => {}
            Err(DecimalError::Truncated) => {
                if ceil && !decimal.IsNegative() {
                    value += 1;
                } else if !ceil && decimal.IsNegative() {
                    value -= 1;
                }
            }
            Err(error) => return Err(error.into()),
        }
        result.Int64sMut()[row] = value;
    }
    Ok(())
}

impl builtinCeilDecToIntSig {
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        decimal_to_int(&self.base, ctx, input, result, true)
    }
}

impl builtinFloorDecToIntSig {
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        decimal_to_int(&self.base, ctx, input, result, false)
    }
}

impl builtinCeilIntToIntSig {
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        self.base.args[0].VecEvalInt(ctx, input, result)
    }
}

impl builtinFloorIntToIntSig {
    pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()> {
        self.base.args[0].VecEvalInt(ctx, input, result)
    }
}

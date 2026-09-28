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

// 向量（VectorFloat32）内置函数的向量化（按列）求值实现。
//
// 对应 Go `builtin_vec_vec.go`。对 Chunk 中整列批量计算维度、距离、范数与文本互转；
// NULL 或 NaN 结果写入结果列的 NULL 位。标量路径见 `builtin_vec`。

use crate::builtin_vec_kernel::*;
use crate::*;

/// 向量化求值左/右向量参数，写入临时列。
fn evaluateVectorArgument(
    argument: &ExprBox,
    ctx: &dyn EvalContext,
    input: &chunk::Chunk,
) -> Result<chunk::Column, Error> {
    let mut column = chunk::Column::default();
    argument.VecEvalVectorFloat32(ctx, input, &mut column)?;
    Ok(column)
}

/// 双向量距离的列式求值：任一侧 NULL 或距离为 NaN 则追加 NULL。
fn vectorizedDistance(
    base: &formal_registry::RegistryBuiltinBase,
    ctx: &dyn EvalContext,
    input: &chunk::Chunk,
    result: &mut chunk::Column,
    distance: impl Fn(
        &types::VectorFloat32,
        &types::VectorFloat32,
    ) -> Result<f64, contextutil::errors::SharedError>,
) -> Result<(), Error> {
    let left = evaluateVectorArgument(&base.args[0], ctx, input)?;
    let right = evaluateVectorArgument(&base.args[1], ctx, input)?;
    result.ResizeFloat64(0, false);
    for index in 0..input.NumRows() {
        // 任一侧为 SQL NULL 则结果为 NULL，与标量路径一致。
        if left.IsNull(index) || right.IsNull(index) {
            result.AppendNull();
            continue;
        }
        let value = distance(
            &left.GetVectorFloat32(index),
            &right.GetVectorFloat32(index),
        )?;
        // NaN 距离（如零向量余弦）按 NULL 写出。
        if value.is_nan() {
            result.AppendNull();
        } else {
            result.AppendFloat64(value);
        }
    }
    Ok(())
}

impl builtinVecDimsSig {
    /// 向量化 VEC_DIMS：按行写出维度；输入 NULL 则输出 NULL。
    pub fn vecEvalInt(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        let vectors = evaluateVectorArgument(&self.baseBuiltinFunc.args[0], ctx, input)?;
        result.ResizeInt64(0, false);
        for index in 0..input.NumRows() {
            if vectors.IsNull(index) {
                result.AppendNull();
            } else {
                result.AppendInt64(vectors.GetVectorFloat32(index).Len() as i64);
            }
        }
        Ok(())
    }
}

macro_rules! vectorized_distance {
    ($signature:ident, $method:ident) => {
        impl $signature {
            pub fn vecEvalReal(
                &self,
                ctx: &dyn EvalContext,
                input: &chunk::Chunk,
                result: &mut chunk::Column,
            ) -> Result<(), Error> {
                vectorizedDistance(&self.baseBuiltinFunc, ctx, input, result, |left, right| {
                    left.$method(right)
                })
            }
        }
    };
}

vectorized_distance!(builtinVecL1DistanceSig, L1Distance);
vectorized_distance!(builtinVecL2DistanceSig, L2Distance);
vectorized_distance!(builtinVecNegativeInnerProductSig, NegativeInnerProduct);
vectorized_distance!(builtinVecCosineDistanceSig, CosineDistance);

impl builtinVecL2NormSig {
    /// 向量化距离/范数：按行计算实数结果，NULL/NaN 传播为 NULL。
    pub fn vecEvalReal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        let vectors = evaluateVectorArgument(&self.baseBuiltinFunc.args[0], ctx, input)?;
        result.ResizeFloat64(0, false);
        for index in 0..input.NumRows() {
            if vectors.IsNull(index) {
                result.AppendNull();
                continue;
            }
            let value = vectors.GetVectorFloat32(index).L2Norm();
            if value.is_nan() {
                result.AppendNull();
            } else {
                result.AppendFloat64(value);
            }
        }
        Ok(())
    }
}

impl builtinVecFromTextSig {
    /// 向量化 VEC_FROM_TEXT：解析字符串列并校验维度是否适配返回列。
    pub fn vecEvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        let mut strings = chunk::Column::default();
        self.baseBuiltinFunc.args[0].VecEvalString(ctx, input, &mut strings)?;
        result.ReserveVectorFloat32(input.NumRows());
        for index in 0..input.NumRows() {
            if strings.IsNull(index) {
                result.AppendNull();
                continue;
            }
            let vector = types::ParseVectorFloat32(&strings.GetString(index))?;
            vector.CheckDimsFitColumn(self.baseBuiltinFunc.return_type.GetFlen() as i32)?;
            result.AppendVectorFloat32(vector);
        }
        Ok(())
    }
}

impl builtinVecAsTextSig {
    /// 向量化 VEC_AS_TEXT：将向量列格式化为字符串列。
    pub fn vecEvalString(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        let vectors = evaluateVectorArgument(&self.baseBuiltinFunc.args[0], ctx, input)?;
        result.ReserveString(input.NumRows());
        for index in 0..input.NumRows() {
            if vectors.IsNull(index) {
                result.AppendNull();
            } else {
                result.AppendString(&vectors.GetVectorFloat32(index).String());
            }
        }
        Ok(())
    }
}

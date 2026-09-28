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

// 向量（VectorFloat32）标量内置函数：维度、距离、范数与文本互转。
//
// 对应 Go `builtin_vec.go`。提供 VEC_DIMS、L1/L2/余弦距离、负内积、L2 范数、
// VEC_FROM_TEXT / VEC_AS_TEXT 的标量求值与函数类注册；向量化路径见 `builtin_vec_vec`。
// 这里的「向量」指 Embedding 浮点向量类型，而非执行引擎的列式向量化。

use crate::*;

/// 向量内置函数共用的注册基座类型别名。
type VectorBuiltinBase = formal_registry::RegistryBuiltinBase;

/// 为向量函数参数/返回值构造 FieldType；字符串用 utf8mb4，其余用 binary。
fn vectorFieldType(tp: u8) -> types::FieldType {
    let mut field_type = *types::NewFieldType(tp);
    if tp == mysql::TypeVarString {
        field_type.SetCharset(charset::CharsetUTF8MB4.to_owned());
        field_type.SetCollate(charset::CollationUTF8MB4.to_owned());
    } else {
        field_type.SetCharset(charset::CharsetBin.to_owned());
        field_type.SetCollate(charset::CollationBin.to_owned());
    }
    field_type
}

/// 校验参数个数，必要时插入 CAST，并设置 tipb 下推编码（pb_code）。
fn buildVectorBase(
    ctx: &dyn BuildContext,
    args: Vec<ExprBox>,
    argument_types: &[u8],
    return_type: u8,
    pb_code: i32,
) -> Result<VectorBuiltinBase, Error> {
    if args.len() != argument_types.len() {
        return Err(errors::New("unexpected length of vector builtin arguments"));
    }
    // 按声明类型对每个参数做求值类型对齐；不一致则包一层 CAST。
    let args = args
        .into_iter()
        .zip(argument_types)
        .map(|(argument, tp)| {
            let target = vectorFieldType(*tp);
            if argument.GetType(ctx.GetEvalCtx()).EvalType() == target.EvalType() {
                argument
            } else {
                BuildCastFunction(ctx, &argument, &target)
            }
        })
        .collect();
    let mut base = VectorBuiltinBase::new_recursive(args, vectorFieldType(return_type));
    base.pb_code = pb_code;
    Ok(base)
}

macro_rules! vector_function_class {
    ($class:ident, $signature:ident, $return_type:expr, [$($argument_type:expr),*], $pb_code:expr) => {
        pub struct $class {
            pub baseFunctionClass: baseFunctionClass,
        }

        impl $class {
            pub fn getFunction(
                &self,
                ctx: &dyn BuildContext,
                args: Vec<ExprBox>,
            ) -> Result<Box<dyn builtinFunc>, Error> {
                self.baseFunctionClass.verifyArgs(&args)?;
                Ok(Box::new($signature {
                    baseBuiltinFunc: buildVectorBase(
                        ctx,
                        args,
                        &[$($argument_type),*],
                        $return_type,
                        $pb_code,
                    )?,
                }))
            }
        }

        impl functionClass for $class {
            fn getFunction(
                &self,
                ctx: &dyn BuildContext,
                args: Vec<ExprBox>,
            ) -> Result<Box<dyn builtinFunc>, Error> {
                <$class>::getFunction(self, ctx, args)
            }
            fn verifyArgsByCount(&self, count: usize) -> Result<(), Error> {
                self.baseFunctionClass.verifyArgsByCount(count)
            }
            fn getDisplayName(&self) -> &str {
                &self.baseFunctionClass.funcName
            }
        }
    };
}

macro_rules! vector_builtin_common {
    ($signature:ident, $scalar:ident, $vector:ident, $value:ty) => {
        impl CollationInfo for $signature {
            fn HasCoercibility(&self) -> bool {
                self.baseBuiltinFunc.HasCoercibility()
            }
            fn Coercibility(&self) -> Coercibility {
                self.baseBuiltinFunc.Coercibility()
            }
            fn SetCoercibility(&self, value: Coercibility) {
                self.baseBuiltinFunc.SetCoercibility(value)
            }
            fn Repertoire(&self) -> Repertoire {
                self.baseBuiltinFunc.Repertoire()
            }
            fn SetRepertoire(&mut self, value: Repertoire) {
                self.baseBuiltinFunc.SetRepertoire(value)
            }
            fn CharsetAndCollation(&self) -> (String, String) {
                self.baseBuiltinFunc.CharsetAndCollation()
            }
            fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
                self.baseBuiltinFunc
                    .SetCharsetAndCollation(charset, collation)
            }
            fn IsExplicitCharset(&self) -> bool {
                self.baseBuiltinFunc.IsExplicitCharset()
            }
            fn SetExplicitCharset(&mut self, explicit: bool) {
                self.baseBuiltinFunc.SetExplicitCharset(explicit)
            }
        }

        impl builtinFunc for $signature {
            fn $scalar(
                &self,
                ctx: &dyn EvalContext,
                row: chunk::Row,
            ) -> Result<($value, bool), Error> {
                <$signature>::$scalar(self, ctx, row)
            }
            fn $vector(
                &self,
                ctx: &dyn EvalContext,
                input: &chunk::Chunk,
                result: &mut chunk::Column,
            ) -> Result<(), Error> {
                <$signature>::$vector(self, ctx, input, result)
            }
            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
            fn SafeToShareAcrossSession(&self) -> bool {
                self.baseBuiltinFunc.SafeToShareAcrossSession()
            }
            fn getArgs(&self) -> &[ExprBox] {
                &self.baseBuiltinFunc.args
            }
            fn getArgsMut(&mut self) -> &mut [ExprBox] {
                &mut self.baseBuiltinFunc.args
            }
            fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
                other
                    .as_any()
                    .downcast_ref::<Self>()
                    .is_some_and(|right| self.baseBuiltinFunc.equal(ctx, &right.baseBuiltinFunc))
            }
            fn getRetTp(&self) -> &types::FieldType {
                &self.baseBuiltinFunc.return_type
            }
            fn setPbCode(&mut self, code: i32) {
                self.baseBuiltinFunc.pb_code = code
            }
            fn PbCode(&self) -> i32 {
                self.baseBuiltinFunc.pb_code
            }
            fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
                self.baseBuiltinFunc.collator = collator
            }
            fn collator(&self) -> &dyn collate::Collator {
                self.baseBuiltinFunc.collator.as_ref()
            }
            fn Clone(&self) -> Box<dyn builtinFunc> {
                Box::new(self.clone())
            }
            fn MemoryUsage(&self) -> i64 {
                self.baseBuiltinFunc.memory_usage()
            }
            fn vectorized(&self) -> bool {
                true
            }
        }
    };
}

/// VEC_DIMS：返回向量维度（元素个数）。
#[derive(Clone)]
pub struct builtinVecDimsSig {
    pub(crate) baseBuiltinFunc: VectorBuiltinBase,
}
/// VEC_L1_DISTANCE：曼哈顿距离。
#[derive(Clone)]
pub struct builtinVecL1DistanceSig {
    pub(crate) baseBuiltinFunc: VectorBuiltinBase,
}
/// VEC_L2_DISTANCE：欧氏距离。
#[derive(Clone)]
pub struct builtinVecL2DistanceSig {
    pub(crate) baseBuiltinFunc: VectorBuiltinBase,
}
/// VEC_NEGATIVE_INNER_PRODUCT：负内积（常用于近似最近邻排序）。
#[derive(Clone)]
pub struct builtinVecNegativeInnerProductSig {
    pub(crate) baseBuiltinFunc: VectorBuiltinBase,
}
/// VEC_COSINE_DISTANCE：余弦距离；零向量产生 NaN→NULL。
#[derive(Clone)]
pub struct builtinVecCosineDistanceSig {
    pub(crate) baseBuiltinFunc: VectorBuiltinBase,
}
/// VEC_L2_NORM：L2 范数。
#[derive(Clone)]
pub struct builtinVecL2NormSig {
    pub(crate) baseBuiltinFunc: VectorBuiltinBase,
}
/// VEC_FROM_TEXT：解析文本为 VectorFloat32，并校验列维度。
#[derive(Clone)]
pub struct builtinVecFromTextSig {
    pub(crate) baseBuiltinFunc: VectorBuiltinBase,
}
/// VEC_AS_TEXT：将向量格式化为文本。
#[derive(Clone)]
pub struct builtinVecAsTextSig {
    pub(crate) baseBuiltinFunc: VectorBuiltinBase,
}

impl builtinVecDimsSig {
    pub fn evalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
        let (vector, null) = self.baseBuiltinFunc.args[0].EvalVectorFloat32(ctx, row)?;
        Ok((if null { 0 } else { vector.Len() as i64 }, null))
    }
}

/// 双向量距离类标量求值：任一侧 NULL 则结果 NULL；NaN 距离也视为 NULL。
fn vectorDistance(
    base: &VectorBuiltinBase,
    ctx: &dyn EvalContext,
    row: chunk::Row,
    distance: impl FnOnce(
        &types::VectorFloat32,
        &types::VectorFloat32,
    ) -> Result<f64, contextutil::errors::SharedError>,
) -> Result<(f64, bool), Error> {
    let (left, null) = base.args[0].EvalVectorFloat32(ctx, row.clone())?;
    if null {
        return Ok((0.0, true));
    }
    let (right, null) = base.args[1].EvalVectorFloat32(ctx, row)?;
    if null {
        return Ok((0.0, true));
    }
    let value = distance(&left, &right)?;
    // MySQL/TiDB：距离为 NaN 时按 NULL 返回（is_null=true）。
    Ok((if value.is_nan() { 0.0 } else { value }, value.is_nan()))
}

impl builtinVecL1DistanceSig {
    pub fn evalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(f64, bool), Error> {
        vectorDistance(&self.baseBuiltinFunc, ctx, row, |left, right| {
            left.L1Distance(right)
        })
    }
}
impl builtinVecL2DistanceSig {
    pub fn evalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(f64, bool), Error> {
        vectorDistance(&self.baseBuiltinFunc, ctx, row, |left, right| {
            left.L2Distance(right)
        })
    }
}
impl builtinVecNegativeInnerProductSig {
    pub fn evalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(f64, bool), Error> {
        vectorDistance(&self.baseBuiltinFunc, ctx, row, |left, right| {
            left.NegativeInnerProduct(right)
        })
    }
}
impl builtinVecCosineDistanceSig {
    pub fn evalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(f64, bool), Error> {
        vectorDistance(&self.baseBuiltinFunc, ctx, row, |left, right| {
            left.CosineDistance(right)
        })
    }
}
impl builtinVecL2NormSig {
    pub fn evalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(f64, bool), Error> {
        let (vector, null) = self.baseBuiltinFunc.args[0].EvalVectorFloat32(ctx, row)?;
        if null {
            return Ok((0.0, true));
        }
        let value = vector.L2Norm();
        Ok((if value.is_nan() { 0.0 } else { value }, value.is_nan()))
    }
}
impl builtinVecFromTextSig {
    pub fn evalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        let (text, null) = self.baseBuiltinFunc.args[0].EvalString(ctx, row)?;
        if null {
            return Ok((types::ZeroVectorFloat32(), true));
        }
        let vector = types::ParseVectorFloat32(&text)?;
        vector.CheckDimsFitColumn(self.baseBuiltinFunc.return_type.GetFlen() as i32)?;
        Ok((vector, false))
    }
}
impl builtinVecAsTextSig {
    pub fn evalString(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(String, bool), Error> {
        let (vector, null) = self.baseBuiltinFunc.args[0].EvalVectorFloat32(ctx, row)?;
        Ok((if null { String::new() } else { vector.String() }, null))
    }
}

vector_builtin_common!(builtinVecDimsSig, evalInt, vecEvalInt, i64);
macro_rules! real_vector_builtin {
    ($signature:ident) => {
        vector_builtin_common!($signature, evalReal, vecEvalReal, f64);
    };
}
real_vector_builtin!(builtinVecL1DistanceSig);
real_vector_builtin!(builtinVecL2DistanceSig);
real_vector_builtin!(builtinVecNegativeInnerProductSig);
real_vector_builtin!(builtinVecCosineDistanceSig);
real_vector_builtin!(builtinVecL2NormSig);
vector_builtin_common!(
    builtinVecFromTextSig,
    evalVectorFloat32,
    vecEvalVectorFloat32,
    types::VectorFloat32
);
vector_builtin_common!(builtinVecAsTextSig, evalString, vecEvalString, String);

vector_function_class!(
    vecDimsFunctionClass,
    builtinVecDimsSig,
    mysql::TypeLonglong,
    [mysql::TypeTiDBVectorFloat32],
    tipb::ScalarFuncSig::VecDimsSig as i32
);
vector_function_class!(
    vecL1DistanceFunctionClass,
    builtinVecL1DistanceSig,
    mysql::TypeDouble,
    [mysql::TypeTiDBVectorFloat32, mysql::TypeTiDBVectorFloat32],
    tipb::ScalarFuncSig::VecL1DistanceSig as i32
);
vector_function_class!(
    vecL2DistanceFunctionClass,
    builtinVecL2DistanceSig,
    mysql::TypeDouble,
    [mysql::TypeTiDBVectorFloat32, mysql::TypeTiDBVectorFloat32],
    tipb::ScalarFuncSig::VecL2DistanceSig as i32
);
vector_function_class!(
    vecNegativeInnerProductFunctionClass,
    builtinVecNegativeInnerProductSig,
    mysql::TypeDouble,
    [mysql::TypeTiDBVectorFloat32, mysql::TypeTiDBVectorFloat32],
    tipb::ScalarFuncSig::VecNegativeInnerProductSig as i32
);
vector_function_class!(
    vecCosineDistanceFunctionClass,
    builtinVecCosineDistanceSig,
    mysql::TypeDouble,
    [mysql::TypeTiDBVectorFloat32, mysql::TypeTiDBVectorFloat32],
    tipb::ScalarFuncSig::VecCosineDistanceSig as i32
);
vector_function_class!(
    vecL2NormFunctionClass,
    builtinVecL2NormSig,
    mysql::TypeDouble,
    [mysql::TypeTiDBVectorFloat32],
    tipb::ScalarFuncSig::VecL2NormSig as i32
);
vector_function_class!(
    vecFromTextFunctionClass,
    builtinVecFromTextSig,
    mysql::TypeTiDBVectorFloat32,
    [mysql::TypeVarString],
    0
);
vector_function_class!(
    vecAsTextFunctionClass,
    builtinVecAsTextSig,
    mysql::TypeVarString,
    [mysql::TypeTiDBVectorFloat32],
    tipb::ScalarFuncSig::VecAsTextSig as i32
);

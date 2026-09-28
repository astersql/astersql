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

// 向量内置函数标量与向量化路径的 Aster 集成单元测试（expression group 31）。
//
// 对照 Go 数值：验证维度/距离/范数/文本互转、NULL 与 NaN、维度不匹配错误，
// 以及函数类参数个数校验与 tipb 下推编码赋值。

use crate::builtin_vec_kernel::*;
use crate::*;

/// 空用户变量读取器，满足 EvalContext 依赖但不提供变量。
struct EmptyUserVars;

impl exprctx::UserVarsReader for EmptyUserVars {
    fn GetUserVarVal(&self, _name: &str) -> Option<types::Datum> {
        None
    }

    fn GetUserVarType(&self, _name: &str) -> Option<types::FieldType> {
        None
    }

    fn Clone(&self) -> Box<dyn exprctx::UserVarsReader> {
        Box::new(Self)
    }
}

/// 测试用求值上下文：仅实现向量常量路径所需最小接口。
struct TestEvalContext(EmptyUserVars);

impl contextutil::WarnAppender for TestEvalContext {
    fn AppendWarning(&self, _error: contextutil::errors::SharedError) {}
    fn AppendNote(&self, _error: contextutil::errors::SharedError) {}
}

impl contextutil::WarnHandler for TestEvalContext {
    fn WarningCount(&self) -> usize {
        0
    }

    fn TruncateWarnings(&self, _start: isize) -> Vec<contextutil::SQLWarn> {
        Vec::new()
    }

    fn CopyWarnings(&self, destination: Vec<contextutil::SQLWarn>) -> Vec<contextutil::SQLWarn> {
        destination
    }
}

impl exprctx::ParamValues for TestEvalContext {
    fn GetParamValue(&self, _index: usize) -> Result<types::Datum, exprctx::ParamError> {
        Err(exprctx::ParamError::IndexExceedsParamCount)
    }
}

impl EvalContext for TestEvalContext {
    fn CtxID(&self) -> u64 {
        1
    }

    fn SQLMode(&self) -> mysql::SQLMode {
        mysql::SQLMode::default()
    }

    fn TypeCtx(&self) -> types::Context {
        panic!("not used by vector constant evaluation")
    }

    fn ErrCtx(&self) -> errctx::Context {
        panic!("not used by vector constant evaluation")
    }

    fn Location(&self) -> chrono_tz::Tz {
        chrono_tz::UTC
    }

    fn CurrentTime(
        &self,
    ) -> Result<chrono::DateTime<chrono_tz::Tz>, contextutil::errors::SharedError> {
        panic!("not used by vector constant evaluation")
    }

    fn CurrentDB(&self) -> String {
        String::new()
    }

    fn GetMaxAllowedPacket(&self) -> u64 {
        64 << 20
    }

    fn GetTiDBRedactLog(&self) -> String {
        "OFF".to_owned()
    }

    fn GetDefaultWeekFormatMode(&self) -> String {
        "0".to_owned()
    }

    fn GetDivPrecisionIncrement(&self) -> i32 {
        4
    }

    fn GetUserVarsReader(&self) -> &dyn exprctx::UserVarsReader {
        &self.0
    }

    fn GetOptionalPropSet(&self) -> exprctx::OptionalEvalPropKeySet {
        exprctx::OptionalEvalPropKeySet::default()
    }

    fn GetOptionalPropProvider(
        &self,
        _key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        None
    }
}

/// 测试用函数构建上下文：用于 `getFunction` 绑定签名。
struct TestBuildContext(TestEvalContext);

impl BuildContext for TestBuildContext {
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        &self.0
    }

    fn GetCharsetInfo(&self) -> (String, String) {
        ("utf8mb4".to_owned(), "utf8mb4_bin".to_owned())
    }

    fn GetDefaultCollationForUTF8MB4(&self) -> String {
        "utf8mb4_bin".to_owned()
    }

    fn GetBlockEncryptionMode(&self) -> String {
        "aes-128-ecb".to_owned()
    }

    fn GetSysdateIsNow(&self) -> bool {
        false
    }

    fn GetNoopFuncsMode(&self) -> i32 {
        0
    }

    fn Rng(&self) -> &exprctx::mathutil::MysqlRng {
        panic!("not used by vector constant evaluation")
    }

    fn IsUseCache(&self) -> bool {
        false
    }

    fn SetSkipPlanCache(&self, _reason: &str) {}

    fn AllocPlanColumnID(&self) -> i64 {
        1
    }

    fn IsInNullRejectCheck(&self) -> bool {
        false
    }

    fn IsConstantPropagateCheck(&self) -> bool {
        false
    }

    fn ConnectionID(&self) -> u64 {
        1
    }

    fn IsReadonlyUserVar(&self, _name: &str) -> bool {
        false
    }
}

/// 将文本解析为 VectorFloat32 常量表达式。
fn vector(value: &str) -> ExprBox {
    let value = types::ParseVectorFloat32(value).unwrap();
    Box::new(Constant::with_type(
        types::NewVectorFloat32Datum(value),
        *types::NewFieldType(mysql::TypeTiDBVectorFloat32),
    ))
}

/// 构造类型为 VectorFloat32 的 NULL 常量。
fn null_vector() -> ExprBox {
    Box::new(NewNullWithFieldType(*types::NewFieldType(
        mysql::TypeTiDBVectorFloat32,
    )))
}

/// 构造类型为字符串的 NULL 常量。
fn null_string() -> ExprBox {
    Box::new(NewNullWithFieldType(*types::NewFieldType(
        mysql::TypeVarString,
    )))
}

/// 构造仅含虚拟行数的空 Chunk，用于常量参数的向量化广播。
fn input(rows: usize) -> Box<chunk::Chunk> {
    let mut input = chunk::NewChunkWithCapacity(Vec::<types::FieldType>::new(), rows);
    input.SetNumVirtualRows(rows);
    input
}

/// 组装向量内置函数基座。
fn base(args: Vec<ExprBox>, return_type: u8) -> formal_registry::RegistryBuiltinBase {
    formal_registry::RegistryBuiltinBase::new_recursive(args, *types::NewFieldType(return_type))
}

/// 组装双向量距离类函数基座（返回 DOUBLE）。
fn distance_base(left: ExprBox, right: ExprBox) -> formal_registry::RegistryBuiltinBase {
    base(vec![left, right], mysql::TypeDouble)
}

/// 标量路径：距离/范数数值、NULL、维度不匹配与零向量 NaN→NULL。
#[test]
fn scalar_vector_metrics_match_go_values_null_errors_and_nan() {
    let ctx = TestEvalContext(EmptyUserVars);
    let row = chunk::Row::default();

    let dims = builtinVecDimsSig {
        baseBuiltinFunc: base(vec![vector("[1,2]")], mysql::TypeLonglong),
    };
    assert_eq!(dims.evalInt(&ctx, row.clone()).unwrap(), (2, false));

    let null_dims = builtinVecDimsSig {
        baseBuiltinFunc: base(vec![null_vector()], mysql::TypeLonglong),
    };
    assert_eq!(null_dims.evalInt(&ctx, row.clone()).unwrap(), (0, true));

    let l1 = builtinVecL1DistanceSig {
        baseBuiltinFunc: distance_base(vector("[1,2]"), vector("[4,0]")),
    };
    assert_eq!(l1.evalReal(&ctx, row.clone()).unwrap(), (5.0, false));

    let l2 = builtinVecL2DistanceSig {
        baseBuiltinFunc: distance_base(vector("[1,2]"), vector("[4,0]")),
    };
    let (distance, is_null) = l2.evalReal(&ctx, row.clone()).unwrap();
    assert!(!is_null);
    assert!((distance - 13.0_f64.sqrt()).abs() < 1e-12);

    let negative_inner = builtinVecNegativeInnerProductSig {
        baseBuiltinFunc: distance_base(vector("[1,2]"), vector("[4,0]")),
    };
    assert_eq!(
        negative_inner.evalReal(&ctx, row.clone()).unwrap(),
        (-4.0, false)
    );

    let cosine = builtinVecCosineDistanceSig {
        baseBuiltinFunc: distance_base(vector("[1,2]"), vector("[4,0]")),
    };
    let (distance, is_null) = cosine.evalReal(&ctx, row.clone()).unwrap();
    assert!(!is_null);
    assert!((distance - (1.0 - 1.0 / 5.0_f64.sqrt())).abs() < 1e-12);

    let norm = builtinVecL2NormSig {
        baseBuiltinFunc: base(vec![vector("[1,2]")], mysql::TypeDouble),
    };
    let (value, is_null) = norm.evalReal(&ctx, row.clone()).unwrap();
    assert!(!is_null);
    assert!((value - 5.0_f64.sqrt()).abs() < 1e-12);

    let null_norm = builtinVecL2NormSig {
        baseBuiltinFunc: base(vec![null_vector()], mysql::TypeDouble),
    };
    assert_eq!(null_norm.evalReal(&ctx, row.clone()).unwrap(), (0.0, true));

    let null_l1 = builtinVecL1DistanceSig {
        baseBuiltinFunc: distance_base(null_vector(), vector("[4,0]")),
    };
    assert_eq!(null_l1.evalReal(&ctx, row.clone()).unwrap(), (0.0, true));

    let mismatch = builtinVecL2DistanceSig {
        baseBuiltinFunc: distance_base(vector("[1,2]"), vector("[4]")),
    };
    assert!(mismatch.evalReal(&ctx, row.clone()).is_err());

    let zero_cosine = builtinVecCosineDistanceSig {
        baseBuiltinFunc: distance_base(vector("[0,0]"), vector("[4,0]")),
    };
    assert_eq!(zero_cosine.evalReal(&ctx, row).unwrap(), (0.0, true));
}

/// 标量 VEC_FROM_TEXT/AS_TEXT：维度校验、非法文本与格式化。
#[test]
fn scalar_text_conversion_checks_dimensions_and_format() {
    let ctx = TestEvalContext(EmptyUserVars);
    let row = chunk::Row::default();
    let mut from_text_base = base(
        vec![Box::new(NewStrConst("[1.5,2]"))],
        mysql::TypeTiDBVectorFloat32,
    );
    from_text_base.return_type.SetFlen(2);
    let from_text = builtinVecFromTextSig {
        baseBuiltinFunc: from_text_base,
    };
    let (value, is_null) = from_text.evalVectorFloat32(&ctx, row.clone()).unwrap();
    assert!(!is_null);
    assert_eq!(value.String(), "[1.5,2]");

    let mut wrong_dims_base = base(
        vec![Box::new(NewStrConst("[1.5,2]"))],
        mysql::TypeTiDBVectorFloat32,
    );
    wrong_dims_base.return_type.SetFlen(1);
    let wrong_dims = builtinVecFromTextSig {
        baseBuiltinFunc: wrong_dims_base,
    };
    assert!(wrong_dims.evalVectorFloat32(&ctx, row.clone()).is_err());

    let invalid = builtinVecFromTextSig {
        baseBuiltinFunc: base(
            vec![Box::new(NewStrConst("not-a-vector"))],
            mysql::TypeTiDBVectorFloat32,
        ),
    };
    assert!(invalid.evalVectorFloat32(&ctx, row.clone()).is_err());

    let null_from_text = builtinVecFromTextSig {
        baseBuiltinFunc: base(vec![null_string()], mysql::TypeTiDBVectorFloat32),
    };
    assert!(
        null_from_text
            .evalVectorFloat32(&ctx, row.clone())
            .unwrap()
            .1
    );

    let as_text = builtinVecAsTextSig {
        baseBuiltinFunc: base(vec![vector("[1.5,2]")], mysql::TypeVarString),
    };
    assert_eq!(
        as_text.evalString(&ctx, row.clone()).unwrap(),
        ("[1.5,2]".to_owned(), false)
    );

    let null_as_text = builtinVecAsTextSig {
        baseBuiltinFunc: base(vec![null_vector()], mysql::TypeVarString),
    };
    assert_eq!(
        null_as_text.evalString(&ctx, row).unwrap(),
        (String::new(), true)
    );
}

/// 向量化路径与标量在 NULL/NaN/文本行为上保持一致。
#[test]
fn vectorized_paths_match_scalar_null_nan_and_text_behavior() {
    let ctx = TestEvalContext(EmptyUserVars);
    let input = input(3);

    let dims = builtinVecDimsSig {
        baseBuiltinFunc: base(vec![vector("[1,2]")], mysql::TypeLonglong),
    };
    let mut integers = chunk::Column::default();
    dims.vecEvalInt(&ctx, &input, &mut integers).unwrap();
    assert_eq!(integers.Rows(), 3);
    assert_eq!(integers.GetInt64(0), 2);
    assert_eq!(integers.GetInt64(2), 2);

    let l1 = builtinVecL1DistanceSig {
        baseBuiltinFunc: distance_base(vector("[1,2]"), vector("[4,0]")),
    };
    let mut reals = chunk::Column::default();
    l1.vecEvalReal(&ctx, &input, &mut reals).unwrap();
    assert_eq!(reals.Rows(), 3);
    assert_eq!(reals.GetFloat64(0), 5.0);
    assert_eq!(reals.GetFloat64(2), 5.0);

    let l2 = builtinVecL2DistanceSig {
        baseBuiltinFunc: distance_base(vector("[1,2]"), vector("[4,0]")),
    };
    l2.vecEvalReal(&ctx, &input, &mut reals).unwrap();
    assert!((reals.GetFloat64(1) - 13.0_f64.sqrt()).abs() < 1e-12);

    let negative_inner = builtinVecNegativeInnerProductSig {
        baseBuiltinFunc: distance_base(vector("[1,2]"), vector("[4,0]")),
    };
    negative_inner
        .vecEvalReal(&ctx, &input, &mut reals)
        .unwrap();
    assert_eq!(reals.GetFloat64(1), -4.0);

    let norm = builtinVecL2NormSig {
        baseBuiltinFunc: base(vec![vector("[3,4]")], mysql::TypeDouble),
    };
    norm.vecEvalReal(&ctx, &input, &mut reals).unwrap();
    assert_eq!(reals.GetFloat64(1), 5.0);

    let null_l1 = builtinVecL1DistanceSig {
        baseBuiltinFunc: distance_base(null_vector(), vector("[4,0]")),
    };
    null_l1.vecEvalReal(&ctx, &input, &mut reals).unwrap();
    assert!((0..3).all(|index| reals.IsNull(index)));

    let zero_cosine = builtinVecCosineDistanceSig {
        baseBuiltinFunc: distance_base(vector("[0,0]"), vector("[4,0]")),
    };
    zero_cosine.vecEvalReal(&ctx, &input, &mut reals).unwrap();
    assert!((0..3).all(|index| reals.IsNull(index)));

    let mismatch = builtinVecL2DistanceSig {
        baseBuiltinFunc: distance_base(vector("[1,2]"), vector("[4]")),
    };
    assert!(mismatch.vecEvalReal(&ctx, &input, &mut reals).is_err());

    let mut from_text_base = base(
        vec![Box::new(NewStrConst("[3,4]"))],
        mysql::TypeTiDBVectorFloat32,
    );
    from_text_base.return_type.SetFlen(2);
    let from_text = builtinVecFromTextSig {
        baseBuiltinFunc: from_text_base,
    };
    let mut vectors = chunk::Column::default();
    from_text
        .vecEvalVectorFloat32(&ctx, &input, &mut vectors)
        .unwrap();
    assert_eq!(vectors.Rows(), 3);
    assert_eq!(vectors.GetVectorFloat32(2).String(), "[3,4]");

    let as_text = builtinVecAsTextSig {
        baseBuiltinFunc: base(vec![vector("[3,4]")], mysql::TypeVarString),
    };
    let mut strings = chunk::Column::default();
    as_text.vecEvalString(&ctx, &input, &mut strings).unwrap();
    assert_eq!(strings.Rows(), 3);
    assert_eq!(strings.GetString(2), "[3,4]");

    let null_from_text = builtinVecFromTextSig {
        baseBuiltinFunc: base(vec![null_string()], mysql::TypeTiDBVectorFloat32),
    };
    null_from_text
        .vecEvalVectorFloat32(&ctx, &input, &mut vectors)
        .unwrap();
    assert!((0..3).all(|index| vectors.IsNull(index)));

    let null_as_text = builtinVecAsTextSig {
        baseBuiltinFunc: base(vec![null_vector()], mysql::TypeVarString),
    };
    null_as_text
        .vecEvalString(&ctx, &input, &mut strings)
        .unwrap();
    assert!((0..3).all(|index| strings.IsNull(index)));
}

/// 函数类校验参数个数，并设置正确的 tipb ScalarFuncSig。
#[test]
fn function_classes_validate_counts_and_assign_pushdown_codes() {
    let ctx = TestBuildContext(TestEvalContext(EmptyUserVars));
    let class =
        |name: &str, count: usize| baseFunctionClass::new(name.to_owned(), count, count as isize);

    let dims = vecDimsFunctionClass {
        baseFunctionClass: class("vec_dims", 1),
    };
    assert!(dims.getFunction(&ctx, Vec::new()).is_err());
    assert_eq!(
        dims.getFunction(&ctx, vec![vector("[1,2]")])
            .unwrap()
            .PbCode(),
        tipb::ScalarFuncSig::VecDimsSig as i32
    );

    let cases: Vec<(Box<dyn functionClass>, i32)> = vec![
        (
            Box::new(vecL1DistanceFunctionClass {
                baseFunctionClass: class("vec_l1_distance", 2),
            }),
            tipb::ScalarFuncSig::VecL1DistanceSig as i32,
        ),
        (
            Box::new(vecL2DistanceFunctionClass {
                baseFunctionClass: class("vec_l2_distance", 2),
            }),
            tipb::ScalarFuncSig::VecL2DistanceSig as i32,
        ),
        (
            Box::new(vecNegativeInnerProductFunctionClass {
                baseFunctionClass: class("vec_negative_inner_product", 2),
            }),
            tipb::ScalarFuncSig::VecNegativeInnerProductSig as i32,
        ),
        (
            Box::new(vecCosineDistanceFunctionClass {
                baseFunctionClass: class("vec_cosine_distance", 2),
            }),
            tipb::ScalarFuncSig::VecCosineDistanceSig as i32,
        ),
    ];
    for (class, code) in cases {
        assert!(class.getFunction(&ctx, vec![vector("[1,2]")]).is_err());
        assert_eq!(
            class
                .getFunction(&ctx, vec![vector("[1,2]"), vector("[4,0]")])
                .unwrap()
                .PbCode(),
            code
        );
    }

    let norm = vecL2NormFunctionClass {
        baseFunctionClass: class("vec_l2_norm", 1),
    };
    assert!(norm.getFunction(&ctx, Vec::new()).is_err());
    assert_eq!(
        norm.getFunction(&ctx, vec![vector("[1,2]")])
            .unwrap()
            .PbCode(),
        tipb::ScalarFuncSig::VecL2NormSig as i32
    );

    let from_text = vecFromTextFunctionClass {
        baseFunctionClass: class("vec_from_text", 1),
    };
    assert!(from_text.getFunction(&ctx, Vec::new()).is_err());
    assert_eq!(
        from_text
            .getFunction(&ctx, vec![Box::new(NewStrConst("[1,2]"))])
            .unwrap()
            .PbCode(),
        0
    );

    let as_text = vecAsTextFunctionClass {
        baseFunctionClass: class("vec_as_text", 1),
    };
    assert!(as_text.getFunction(&ctx, Vec::new()).is_err());
    assert_eq!(
        as_text
            .getFunction(&ctx, vec![vector("[1,2]")])
            .unwrap()
            .PbCode(),
        tipb::ScalarFuncSig::VecAsTextSig as i32
    );
}

/// 供向量函数 Go 同名迁移入口复用的完整回归集合。
pub(crate) fn run_vector_builtin_parity_suite() {
    scalar_vector_metrics_match_go_values_null_errors_and_nan();
    scalar_text_conversion_checks_dimensions_and_format();
    vectorized_paths_match_scalar_null_nan_and_text_behavior();
    function_classes_validate_counts_and_assign_pushdown_codes();
}

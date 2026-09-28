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
// 数学内建函数向量化求值的单元测试。
//
// 验证对数定义域告警、ROUND/TRUNCATE、ABS 溢出，以及 CRC32/PI/SIGN/RAND
// 在列式（Chunk/Column）路径上的结果与 NULL 传播。

use crate::expression_group_15::*;
use crate::legacy_vectorized_runtime::{Chunk, Column, EvalContext, EvalError, LiteralExpression};

/// 向量化 LOG：正数求 ln，非正数置 NULL 并追加告警，输入 NULL 保持 NULL。
#[test]
fn test_vectorized_builtin_math_domain_and_nulls() {
    let logarithm = builtinLog1ArgSig::new(vec![LiteralExpression::reals(vec![
        Some(1.0),
        Some(0.0),
        Some(-1.0),
        None,
    ])]);
    let context = EvalContext::default();
    let mut result = Column::default();
    logarithm
        .vecEvalReal(&context, &Chunk::new(4), &mut result)
        .unwrap();

    assert_eq!(result.Float64s()[0], 0.0);
    assert!(result.IsNull(1));
    assert!(result.IsNull(2));
    assert!(result.IsNull(3));
    assert_eq!(context.warnings().len(), 2);

    // Go only rejects values strictly outside [-1, 1]. IEEE NaN makes both
    // comparisons false, so ACOS/ASIN must preserve a non-NULL NaN result.
    let acos = builtinAcosSig::new(vec![LiteralExpression::reals(vec![Some(f64::NAN)])]);
    acos.vecEvalReal(&context, &Chunk::new(1), &mut result)
        .unwrap();
    assert!(!result.IsNull(0));
    assert!(result.Float64s()[0].is_nan());

    let asin = builtinAsinSig::new(vec![LiteralExpression::reals(vec![Some(f64::NAN)])]);
    asin.vecEvalReal(&context, &Chunk::new(1), &mut result)
        .unwrap();
    assert!(!result.IsNull(0));
    assert!(result.Float64s()[0].is_nan());
}

/// 向量化 ROUND/TRUNCATE 小数位，以及 ABS(i64::MIN) 触发 BIGINT 溢出。
#[test]
fn test_vectorized_builtin_math_round_truncate_and_overflow() {
    let round = builtinRoundWithFracRealSig::new(vec![
        LiteralExpression::reals(vec![Some(12.55), Some(-12.55)]),
        LiteralExpression::ints(vec![Some(1), Some(1)]),
    ]);
    let mut result = Column::default();
    round
        .vecEvalReal(&EvalContext::default(), &Chunk::new(2), &mut result)
        .unwrap();
    assert_eq!(result.Float64s(), &[12.6, -12.6]);

    let truncate = builtinTruncateRealSig::new(vec![
        LiteralExpression::reals(vec![Some(123.456), Some(-123.456)]),
        LiteralExpression::ints(vec![Some(2), Some(-1)]),
    ]);
    truncate
        .vecEvalReal(&EvalContext::default(), &Chunk::new(2), &mut result)
        .unwrap();
    assert_eq!(result.Float64s(), &[123.45, -120.0]);

    let absolute = builtinAbsIntSig::new(vec![LiteralExpression::ints(vec![Some(i64::MIN)])]);
    assert!(matches!(
        absolute.vecEvalInt(&EvalContext::default(), &Chunk::new(1), &mut result),
        Err(EvalError::BigIntOverflow(_)),
    ));
}

/// 向量化 CRC32/PI/SIGN/RAND：常量填充、NULL 传播与随机数落在 [0,1)。
#[test]
fn test_vectorized_builtin_math_crc_pi_rand_and_sign() {
    let mut result = Column::default();
    let crc = builtinCRC32Sig::new(vec![LiteralExpression::strings(vec![Some("TiDB"), None])]);
    crc.vecEvalInt(&EvalContext::default(), &Chunk::new(2), &mut result)
        .unwrap();
    assert_eq!(result.Int64s()[0], 787_095_035);
    assert!(result.IsNull(1));

    let pi = builtinPISig::new(Vec::new());
    pi.vecEvalReal(&EvalContext::default(), &Chunk::new(2), &mut result)
        .unwrap();
    assert_eq!(result.Float64s(), &[std::f64::consts::PI; 2]);

    let sign = builtinSignSig::new(vec![LiteralExpression::reals(vec![
        Some(-3.0),
        Some(0.0),
        Some(2.0),
    ])]);
    sign.vecEvalInt(&EvalContext::default(), &Chunk::new(3), &mut result)
        .unwrap();
    assert_eq!(result.Int64s(), &[-1, 0, 1]);

    // 固定种子保证可复现；连续两行随机值应不同且落在半开区间 [0,1)。
    let random = builtinRandSig::with_seed(7);
    random
        .vecEvalReal(&EvalContext::default(), &Chunk::new(3), &mut result)
        .unwrap();
    assert!(
        result
            .Float64s()
            .iter()
            .all(|value| (0.0..1.0).contains(value))
    );
    assert_ne!(result.Float64s()[0], result.Float64s()[1]);
}

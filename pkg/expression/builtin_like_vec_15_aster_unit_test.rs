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
// LIKE 向量化与数学内建函数的 Aster 集成单元测试（expression group 15）。
//
// 对齐 Go 表驱动用例：验证 LIKE 标量/向量语义、对数与反三角函数定义域、
// 幂/余切溢出、四舍五入与截断、CRC32/PI/随机数/符号，以及 CONV 进制转换。

use crate::builtin_cast_vec_kernel::decimal;
use crate::expression_group_15::*;
use crate::legacy_vectorized_runtime::{
    Chunk, Column, EvalContext, EvalError, LiteralExpression, Row,
};

/// 辅助：对实数向量化签名跑一轮求值，返回上下文（含告警）与输出列。
fn run_real<T>(
    sig: &T,
    rows: usize,
    eval: impl Fn(&T, &EvalContext, &Chunk, &mut Column) -> crate::legacy_vectorized_runtime::Result<()>,
) -> (EvalContext, Column) {
    let ctx = EvalContext::default();
    let mut output = Column::default();
    eval(sig, &ctx, &Chunk::new(rows), &mut output).unwrap();
    (ctx, output)
}

/// 标量 LIKE：表驱动匹配结果、返回长度、pb 编码，以及模式缓存初始化/Clone 隔离。
#[test]
fn like_scalar_matches_go_table_and_constructs_signature_metadata() {
    let cases = [
        ("a", "", 0),
        ("a", "a", 1),
        ("a", "b", 0),
        ("aA", "Aa", 0),
        ("aAb", "Aa%", 0),
        ("aAb", "aA_", 1),
        ("baab", "b_%b", 1),
        ("baab", "b%_b", 1),
        ("bab", "b_%b", 1),
        ("bab", "b%_b", 1),
        ("bb", "b_%b", 0),
        ("bb", "b%_b", 0),
        ("baabccc", "b_%b%", 1),
        ("a", r"\a", 1),
    ];
    for (input, pattern, expected) in cases {
        // 通过函数类构造签名，模拟优化器绑定内建函数时的路径。
        let class = likeFunctionClass::new("like");
        let sig = class
            .getFunction(vec![
                LiteralExpression::constant_string(Some(input)),
                LiteralExpression::constant_string(Some(pattern)),
                LiteralExpression::constant_int(Some(i64::from(b'\\'))),
            ])
            .unwrap();
        assert_eq!(sig.return_flen(), 1);
        assert_eq!(sig.pb_code(), ScalarFuncSig::LikeSig);
        assert_eq!(
            sig.evalInt(&EvalContext::default(), Row(0)).unwrap(),
            Some(expected)
        );
        assert!(sig.cache_initialized());
        // Clone 不得拷贝运行时模式缓存，与 Go 会话共享约束一致。
        assert!(
            !sig.Clone().cache_initialized(),
            "Clone must not copy the Go runtime cache"
        );
    }
}

/// 向量化 LIKE：按行传播 NULL，并为非常量模式逐行重编译通配符。
#[test]
fn like_vector_propagates_nulls_and_recompiles_per_row_patterns() {
    let sig = builtinLikeSig::new(vec![
        LiteralExpression::strings(vec![Some("abc"), Some("a_c"), None, Some("zzz")]),
        LiteralExpression::strings(vec![Some("a%"), Some(r"a\_c"), Some("%"), None]),
        LiteralExpression::ints(vec![Some(i64::from(b'\\')); 4]),
    ])
    .unwrap();
    let mut output = Column::default();
    sig.vecEvalInt(&EvalContext::default(), &Chunk::new(4), &mut output)
        .unwrap();
    assert_eq!(output.Int64s(), &[1, 1, 0, 0]);
    assert!(!output.IsNull(0));
    assert!(!output.IsNull(1));
    assert!(output.IsNull(2));
    assert!(output.IsNull(3));
    assert!(sig.vectorized());
}

/// 对数/反余弦：定义域外返回 NULL，并对非法对数参数追加告警（warning）。
#[test]
fn logarithms_and_inverse_trig_preserve_domain_nulls_and_warnings() {
    let log = builtinLog1ArgSig::new(vec![LiteralExpression::reals(vec![
        Some(1.0),
        Some(0.0),
        Some(-1.0),
        None,
    ])]);
    let (ctx, output) = run_real(&log, 4, builtinLog1ArgSig::vecEvalReal);
    assert_eq!(output.Float64s()[0], 0.0);
    assert!(output.IsNull(1) && output.IsNull(2) && output.IsNull(3));
    assert_eq!(ctx.warnings().len(), 2);

    let acos = builtinAcosSig::new(vec![LiteralExpression::reals(vec![
        Some(-1.0),
        Some(0.0),
        Some(1.0),
        Some(1.1),
    ])]);
    let (_, output) = run_real(&acos, 4, builtinAcosSig::vecEvalReal);
    assert!((output.Float64s()[0] - std::f64::consts::PI).abs() < 1e-12);
    assert_eq!(output.Float64s()[1], std::f64::consts::FRAC_PI_2);
    assert_eq!(output.Float64s()[2], 0.0);
    assert!(output.IsNull(3));
}

/// 二元 atan2 的 NULL 传播，以及 POW/COT 的 DOUBLE 溢出错误路径。
#[test]
fn binary_math_and_overflow_paths_match_go() {
    let atan = builtinAtan2ArgsSig::new(vec![
        LiteralExpression::reals(vec![Some(1.0), None]),
        LiteralExpression::reals(vec![Some(1.0), Some(1.0)]),
    ]);
    let (_, output) = run_real(&atan, 2, builtinAtan2ArgsSig::vecEvalReal);
    assert!((output.Float64s()[0] - std::f64::consts::FRAC_PI_4).abs() < 1e-12);
    assert!(output.IsNull(1));

    let pow = builtinPowSig::new(vec![
        LiteralExpression::reals(vec![Some(10.0)]),
        LiteralExpression::reals(vec![Some(400.0)]),
    ]);
    let mut output = Column::default();
    assert!(matches!(
        pow.vecEvalReal(&EvalContext::default(), &Chunk::new(1), &mut output),
        Err(EvalError::DoubleOverflow(_))
    ));

    let cot = builtinCotSig::new(vec![LiteralExpression::reals(vec![Some(0.0)])]);
    assert!(matches!(
        cot.vecEvalReal(&EvalContext::default(), &Chunk::new(1), &mut output),
        Err(EvalError::DoubleOverflow(_))
    ));
}

/// ROUND/TRUNCATE 小数位与符号边界，以及 ABS(i64::MIN) 的 BIGINT 溢出。
#[test]
fn rounding_truncation_and_integer_overflow_follow_go_edges() {
    let round = builtinRoundWithFracRealSig::new(vec![
        LiteralExpression::reals(vec![Some(12.55), Some(-12.55)]),
        LiteralExpression::ints(vec![Some(1), Some(1)]),
    ]);
    let (_, output) = run_real(&round, 2, builtinRoundWithFracRealSig::vecEvalReal);
    assert_eq!(output.Float64s(), &[12.6, -12.6]);

    let truncate = builtinTruncateRealSig::new(vec![
        LiteralExpression::reals(vec![Some(123.456), Some(-123.456)]),
        LiteralExpression::ints(vec![Some(2), Some(-1)]),
    ]);
    let (_, output) = run_real(&truncate, 2, builtinTruncateRealSig::vecEvalReal);
    assert_eq!(output.Float64s(), &[123.45, -120.0]);

    let abs = builtinAbsIntSig::new(vec![LiteralExpression::ints(vec![Some(i64::MIN)])]);
    let mut output = Column::default();
    assert!(matches!(
        abs.vecEvalInt(&EvalContext::default(), &Chunk::new(1), &mut output),
        Err(EvalError::BigIntOverflow(_))
    ));
}

/// DECIMAL 上的 ABS/CEIL/FLOOR 与向整数转换的截断方向，对齐 MySQL/Go 语义。
#[test]
fn decimal_abs_round_ceil_floor_and_int_conversion_keep_go_semantics() {
    let abs = builtinAbsDecSig::new(vec![LiteralExpression::decimals(vec![
        Some(decimal("-1.25")),
        None,
    ])]);
    let mut output = Column::default();
    abs.vecEvalDecimal(&EvalContext::default(), &Chunk::new(2), &mut output)
        .unwrap();
    assert_eq!(output.Decimals()[0].String(), "1.25");
    assert!(output.IsNull(1));

    let ceil = builtinCeilDecToDecSig::new(vec![LiteralExpression::decimals(vec![
        Some(decimal("1.01")),
        Some(decimal("-1.01")),
    ])]);
    ceil.vecEvalDecimal(&EvalContext::default(), &Chunk::new(2), &mut output)
        .unwrap();
    assert_eq!(output.Decimals()[0].String(), "2");
    assert_eq!(output.Decimals()[1].String(), "-1");

    let floor = builtinFloorDecToIntSig::new(vec![LiteralExpression::decimals(vec![
        Some(decimal("1.99")),
        Some(decimal("-1.01")),
    ])]);
    floor
        .vecEvalInt(&EvalContext::default(), &Chunk::new(2), &mut output)
        .unwrap();
    assert_eq!(output.Int64s(), &[1, -2]);
}

/// CRC32、常量 PI、带种子随机数首值，以及 SIGN 的三态结果。
#[test]
fn crc_pi_seeded_rand_and_sign_cover_generated_and_constant_results() {
    let crc = builtinCRC32Sig::new(vec![LiteralExpression::strings(vec![Some("TiDB"), None])]);
    let mut output = Column::default();
    crc.vecEvalInt(&EvalContext::default(), &Chunk::new(2), &mut output)
        .unwrap();
    assert_eq!(output.Int64s()[0], 787_095_035);
    assert!(output.IsNull(1));

    let pi = builtinPISig::new(vec![]);
    pi.vecEvalReal(&EvalContext::default(), &Chunk::new(2), &mut output)
        .unwrap();
    assert_eq!(output.Float64s(), &[std::f64::consts::PI; 2]);

    let rand =
        builtinRandWithSeedFirstGenSig::new(vec![LiteralExpression::ints(vec![Some(1), None])]);
    rand.vecEvalReal(&EvalContext::default(), &Chunk::new(2), &mut output)
        .unwrap();
    assert_eq!(output.Float64s()[0], mathutil::NewWithSeed(1).Gen());
    assert_eq!(output.Float64s()[1], mathutil::NewWithSeed(0).Gen());

    let sign = builtinSignSig::new(vec![LiteralExpression::reals(vec![
        Some(-3.0),
        Some(0.0),
        Some(2.0),
    ])]);
    sign.vecEvalInt(&EvalContext::default(), &Chunk::new(3), &mut output)
        .unwrap();
    assert_eq!(output.Int64s(), &[-1, 0, 1]);
}

/// 整数 TRUNCATE：负小数位截断；无符号小数位参数时直接短路返回原值。
#[test]
fn integer_truncate_obeys_signed_fraction_and_unsigned_short_circuit() {
    let signed = builtinTruncateIntSig::new(vec![
        LiteralExpression::ints(vec![Some(12_345), Some(-12_345)]),
        LiteralExpression::ints(vec![Some(-2), Some(-3)]),
    ]);
    let mut output = Column::default();
    signed
        .vecEvalInt(&EvalContext::default(), &Chunk::new(2), &mut output)
        .unwrap();
    assert_eq!(output.Int64s(), &[12_300, -12_000]);

    let unsigned_frac = builtinTruncateIntSig::new(vec![
        LiteralExpression::ints(vec![Some(12_345)]),
        LiteralExpression::uints(vec![Some(u64::MAX)]),
    ]);
    unsigned_frac
        .vecEvalInt(&EvalContext::default(), &Chunk::new(1), &mut output)
        .unwrap();
    assert_eq!(output.Int64s(), &[12_345]);
}

/// CONV 虽标记不可向量化，但仍实现与 Go 一致的进制转换与非法进制 NULL。
#[test]
fn conv_retains_disabled_vectorized_flag_but_implements_go_conversion() {
    let conv = builtinConvSig::new(vec![
        LiteralExpression::strings(vec![
            Some("a"),
            Some("-10"),
            Some("xyz"),
            Some("1"),
            Some("1"),
        ]),
        LiteralExpression::ints(vec![Some(16), Some(10), Some(10), Some(1), Some(i64::MIN)]),
        LiteralExpression::ints(vec![Some(2), Some(-16), Some(2), Some(10), Some(10)]),
    ]);
    assert!(!conv.vectorized());
    let mut output = Column::default();
    conv.vecEvalString(&EvalContext::default(), &Chunk::new(5), &mut output)
        .unwrap();
    assert_eq!(output.GetString(0), "1010");
    assert_eq!(output.GetString(1), "-A");
    assert_eq!(output.GetString(2), "0");
    assert!(output.IsNull(3));
    assert!(output.IsNull(4));
}

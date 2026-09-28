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

// 生成式时间向量化签名（ADDTIME/SUBTIME/TIMEDIFF 等）的 Aster 单元测试。
//
// 验证 NULL/零时间传播、字符串解析告警、二进制短路、日期与时长分支分流，
// 以及全部生成签名均声明 `vectorized() == true`。

use crate::builtin_time_vec_generated_kernel::*;
use types_time::{Duration, FromDate, NewTime, Time};

/// 构造 DATETIME 类型的 `Time` 测试值。
fn time(year: i32, month: i32, day: i32, hour: i32, minute: i32, second: i32) -> Time {
    NewTime(
        FromDate(year, month, day, hour, minute, second, 0),
        mysql::TypeDatetime,
        0,
    )
}

/// 构造带分数秒精度（FSP）的 `Duration` 测试值。
fn duration(hour: i32, minute: i32, second: i32, fsp: i32) -> Duration {
    Duration::from_parts(hour, minute, second, 0, fsp)
}

/// 把常量列包装为字面量表达式。
fn expression(values: Column, field_type: FieldType) -> Rc<dyn Expression> {
    Rc::new(LiteralExpression::new(values, field_type))
}

/// 组装双参数内置函数基座（参数表达式 + 结果 FSP）。
fn base(lhs: Column, rhs: Column, rhs_type: FieldType, result_fsp: i32) -> BuiltinBase {
    BuiltinBase::new(
        expression(lhs, FieldType::default()),
        expression(rhs, rhs_type),
        FieldType::new(result_fsp),
    )
}

/// DATETIME±DURATION：有效行计算，零时间与 NULL 行输出 NULL。
#[test]
fn datetime_add_and_sub_preserve_null_zero_and_row_results() {
    let ctx = EvalContext::default();
    let input = Chunk::new(3);
    let lhs = Column::Times(vec![
        Some(time(2026, 7, 15, 10, 30, 0)),
        Some(Time::default()),
        None,
    ]);
    let rhs = Column::Durations(vec![
        Some(duration(1, 15, 0, 0).Duration),
        Some(duration(1, 0, 0, 0).Duration),
        Some(duration(1, 0, 0, 0).Duration),
    ]);

    let mut added = Column::default();
    builtinAddDatetimeAndDurationSig::new(base(lhs.clone(), rhs.clone(), FieldType::new(0), 0))
        .vecEvalTime(&ctx, &input, &mut added)
        .unwrap();
    assert_eq!(
        added,
        Column::Times(vec![Some(time(2026, 7, 15, 11, 45, 0)), None, None])
    );

    let mut subtracted = Column::default();
    builtinSubDatetimeAndDurationSig::new(base(lhs, rhs, FieldType::new(0), 0))
        .vecEvalTime(&ctx, &input, &mut subtracted)
        .unwrap();
    assert_eq!(
        subtracted,
        Column::Times(vec![Some(time(2026, 7, 15, 9, 15, 0)), None, None])
    );
}

/// DURATION±STRING：非法时长字符串产生警告并输出 NULL。
#[test]
fn duration_string_paths_match_go_warning_and_null_rules() {
    let ctx = EvalContext::default();
    let input = Chunk::new(3);
    let lhs = Column::Durations(vec![
        Some(duration(2, 0, 0, 0).Duration),
        Some(duration(2, 0, 0, 0).Duration),
        None,
    ]);
    let rhs = Column::Strings(vec![
        Some("00:30:00".into()),
        Some("999:99:99".into()),
        Some("00:30:00".into()),
    ]);

    let mut result = Column::default();
    builtinSubDurationAndStringSig::new(base(lhs, rhs, FieldType::new(0), 0))
        .vecEvalDuration(&ctx, &input, &mut result)
        .unwrap();

    assert_eq!(
        result,
        Column::Durations(vec![Some(duration(1, 30, 0, 0).Duration), None, None])
    );
    assert_eq!(ctx.warnings().len(), 1);
}

/// 求值即失败的表达式，用于断言短路时不会触达右操作数。
struct ErrorExpression;

impl Expression for ErrorExpression {
    fn VecEvalString(
        &self,
        _ctx: &EvalContext,
        _input: &Chunk,
        _result: &mut Column,
    ) -> EvalResult {
        Err(EvalError::Message("must not be evaluated".into()))
    }

    fn GetType(&self, _ctx: &EvalContext) -> FieldType {
        FieldType::binary(0)
    }
}

/// STRING±STRING：左操作数为二进制/不可解析时短路，不求值右操作数。
#[test]
fn binary_second_string_short_circuits_before_rhs_evaluation() {
    let signature = builtinAddStringAndStringSig::new(BuiltinBase::new(
        expression(
            Column::Strings(vec![Some("12:00:00".into()), None]),
            FieldType::default(),
        ),
        Rc::new(ErrorExpression),
        FieldType::new(0),
    ));
    let mut result = Column::default();

    signature
        .vecEvalString(&EvalContext::default(), &Chunk::new(2), &mut result)
        .unwrap();
    assert_eq!(result, Column::Strings(vec![None, None]));
}

/// 字符串结果按时长 vs 日期时间分支格式化；非法串告警。
#[test]
fn string_and_date_results_follow_duration_vs_datetime_branches() {
    let ctx = EvalContext::default();
    let input = Chunk::new(3);
    let mut strings = Column::default();
    builtinAddStringAndDurationSig::new(base(
        Column::Strings(vec![
            Some("12:00:00".into()),
            Some("2026-07-15 23:30:00".into()),
            Some("not-a-time".into()),
        ]),
        Column::Durations(vec![
            Some(duration(1, 0, 0, 0).Duration),
            Some(duration(1, 0, 0, 0).Duration),
            Some(duration(1, 0, 0, 0).Duration),
        ]),
        FieldType::new(0),
        0,
    ))
    .vecEvalString(&ctx, &input, &mut strings)
    .unwrap();
    assert_eq!(
        strings,
        Column::Strings(vec![
            Some("13:00:00".into()),
            Some("2026-07-16 00:30:00".into()),
            None,
        ])
    );
    assert_eq!(ctx.warnings().len(), 1);

    let mut dates = Column::default();
    builtinSubDateAndStringSig::new(base(
        Column::Times(vec![Some(time(2026, 7, 15, 0, 0, 0)), None]),
        Column::Strings(vec![Some("01:00:00".into()), Some("01:00:00".into())]),
        FieldType::new(0),
        0,
    ))
    .vecEvalString(&ctx, &Chunk::new(2), &mut dates)
    .unwrap();
    assert_eq!(
        dates,
        Column::Strings(vec![Some("2026-07-14 23:00:00".into()), None])
    );
}

/// 常 NULL 签名按输入行数填充对应类型的全 NULL 列。
#[test]
fn null_signatures_fill_the_requested_shape() {
    let empty = || {
        base(
            Column::Times(vec![]),
            Column::Times(vec![]),
            FieldType::default(),
            0,
        )
    };
    let ctx = EvalContext::default();
    let input = Chunk::new(3);

    let mut times = Column::default();
    builtinAddTimeDateTimeNullSig::new(empty())
        .vecEvalTime(&ctx, &input, &mut times)
        .unwrap();
    assert_eq!(times, Column::Times(vec![None, None, None]));

    let mut strings = Column::default();
    builtinSubTimeStringNullSig::new(empty())
        .vecEvalString(&ctx, &input, &mut strings)
        .unwrap();
    assert_eq!(strings, Column::Strings(vec![None, None, None]));

    let mut durations = Column::default();
    builtinNullTimeDiffSig::new(empty())
        .vecEvalDuration(&ctx, &input, &mut durations)
        .unwrap();
    assert_eq!(durations, Column::Durations(vec![None, None, None]));
}

/// TIMEDIFF：同类可减，混合类型结果为 NULL。
#[test]
fn timediff_accepts_matching_kinds_and_nulls_mixed_kinds() {
    let ctx = EvalContext::default();
    let mut result = Column::default();
    builtinTimeTimeTimeDiffSig::new(base(
        Column::Times(vec![Some(time(2026, 7, 15, 12, 0, 0)), None]),
        Column::Times(vec![
            Some(time(2026, 7, 15, 10, 30, 0)),
            Some(time(2026, 7, 15, 10, 30, 0)),
        ]),
        FieldType::new(0),
        0,
    ))
    .vecEvalDuration(&ctx, &Chunk::new(2), &mut result)
    .unwrap();
    assert_eq!(
        result,
        Column::Durations(vec![Some(duration(1, 30, 0, 0).Duration), None])
    );

    let mut strings = Column::default();
    builtinStringStringTimeDiffSig::new(base(
        Column::Strings(vec![
            Some("12:00:00".into()),
            Some("2026-07-15 12:00:00".into()),
        ]),
        Column::Strings(vec![Some("10:30:00".into()), Some("10:30:00".into())]),
        FieldType::new(0),
        0,
    ))
    .vecEvalDuration(&ctx, &Chunk::new(2), &mut strings)
    .unwrap();
    assert_eq!(
        strings,
        Column::Durations(vec![Some(duration(1, 30, 0, 0).Duration), None])
    );
}

/// 全部生成式 ADDTIME/SUBTIME/TIMEDIFF 签名均标记为可向量化。
#[test]
fn every_generated_signature_reports_vectorized() {
    let empty = || {
        base(
            Column::Times(vec![]),
            Column::Times(vec![]),
            FieldType::default(),
            0,
        )
    };

    macro_rules! assert_vectorized {
        ($($signature:ident),+ $(,)?) => {
            $(assert!($signature::new(empty()).vectorized());)+
        };
    }

    assert_vectorized!(
        builtinAddDatetimeAndDurationSig,
        builtinAddDatetimeAndStringSig,
        builtinAddDurationAndDurationSig,
        builtinAddDurationAndStringSig,
        builtinAddStringAndDurationSig,
        builtinAddStringAndStringSig,
        builtinAddDateAndDurationSig,
        builtinAddDateAndStringSig,
        builtinAddTimeDateTimeNullSig,
        builtinAddTimeStringNullSig,
        builtinAddTimeDurationNullSig,
        builtinSubDatetimeAndDurationSig,
        builtinSubDatetimeAndStringSig,
        builtinSubDurationAndDurationSig,
        builtinSubDurationAndStringSig,
        builtinSubStringAndDurationSig,
        builtinSubStringAndStringSig,
        builtinSubDateAndDurationSig,
        builtinSubDateAndStringSig,
        builtinSubTimeDateTimeNullSig,
        builtinSubTimeStringNullSig,
        builtinSubTimeDurationNullSig,
        builtinNullTimeDiffSig,
        builtinTimeStringTimeDiffSig,
        builtinDurationStringTimeDiffSig,
        builtinDurationDurationTimeDiffSig,
        builtinStringTimeTimeDiffSig,
        builtinStringDurationTimeDiffSig,
        builtinStringStringTimeDiffSig,
        builtinTimeTimeTimeDiffSig,
    );
}

/// 供生成式时间向量化 Go 同名入口复用的完整回归集合。
pub(crate) fn run_generated_time_vector_parity_suite() {
    datetime_add_and_sub_preserve_null_zero_and_row_results();
    duration_string_paths_match_go_warning_and_null_rules();
    binary_second_string_short_circuits_before_rhs_evaluation();
    string_and_date_results_follow_duration_vs_datetime_branches();
    null_signatures_fill_the_requested_shape();
    timediff_accepts_matching_kinds_and_nulls_mixed_kinds();
    every_generated_signature_reports_vectorized();
}

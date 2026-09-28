// Copyright 2026 AsterSQL.

// 时间向量化内核（`builtin_time_vec`）的 Aster 单元测试。
//
// 覆盖日历抽取、零日期模式、名称/末日/构造日期、GET_FORMAT、会计期运算、
// TIME 分量与钳位、TSO/时区/有界陈旧读，以及通用 `vec_map` 委托语义。

use crate::expression_builtin_time_vec::*;
use chrono::{TimeZone, Utc};

/// 构造合法 `MysqlTime` 测试夹具（微秒为 0）。
fn t(year: i32, month: u32, day: u32, hour: u32, minute: u32, second: u32) -> MysqlTime {
    MysqlTime::new(year, month, day, hour, minute, second, 0).unwrap()
}

/// 日历抽取函数保持 NULL 传播，且 WEEKDAY/DAYOFWEEK 索引与 MySQL 一致。
#[test]
fn calendar_extractors_preserve_nulls_and_mysql_indexes() {
    let input = NullableVec::new(vec![
        Some(t(2024, 2, 29, 12, 34, 56)),
        None,
        Some(t(2023, 12, 31, 0, 0, 0)),
    ]);

    assert_eq!(
        vec_month(&input).into_inner(),
        vec![Some(2), None, Some(12)]
    );
    assert_eq!(
        vec_year(&input).into_inner(),
        vec![Some(2024), None, Some(2023)]
    );
    assert_eq!(
        vec_day_of_month(&input).into_inner(),
        vec![Some(29), None, Some(31)]
    );
    assert_eq!(
        vec_quarter(&input).into_inner(),
        vec![Some(1), None, Some(4)]
    );
    let mut ctx = EvalContext::default();
    assert_eq!(
        vec_weekday(&input, &mut ctx).unwrap().into_inner(),
        vec![Some(3), None, Some(6)]
    );
    assert_eq!(
        vec_day_of_week(&input, &mut ctx).unwrap().into_inner(),
        vec![Some(5), None, Some(1)]
    );
}

/// DATE 在 no_zero_* 下把非法行变 NULL 并记警告；严格模式则直接报错。
#[test]
fn date_obeys_no_zero_modes_without_losing_non_strict_rows() {
    let input = NullableVec::new(vec![
        Some(MysqlTime::zero()),
        Some(MysqlTime::from_parts_unchecked(2024, 0, 8, 9, 10, 11, 0)),
        Some(t(2024, 7, 8, 9, 10, 11)),
    ]);
    let mut ctx = EvalContext {
        no_zero_date: true,
        no_zero_in_date: true,
        strict: false,
        warnings: vec![],
    };
    let out = vec_date(&input, &mut ctx).unwrap();
    assert_eq!(out.values()[0], None);
    assert_eq!(out.values()[1], None);
    assert_eq!(
        out.values()[2],
        Some(MysqlTime::new(2024, 7, 8, 0, 0, 0, 0).unwrap())
    );
    assert_eq!(ctx.warnings.len(), 2);

    let mut strict = EvalContext {
        strict: true,
        no_zero_date: true,
        ..EvalContext::default()
    };
    assert!(matches!(
        vec_date(
            &NullableVec::new(vec![Some(MysqlTime::zero())]),
            &mut strict
        ),
        Err(TimeVecError::InvalidTime(_))
    ));
}

/// DAYNAME/MONTHNAME/LAST_DAY/MAKEDATE 边界（闰年、两位数年份）对齐 Go。
#[test]
fn names_last_day_and_make_date_follow_go_edges() {
    let input = NullableVec::new(vec![Some(t(2024, 2, 10, 1, 2, 3)), None]);
    let mut ctx = EvalContext::default();
    assert_eq!(
        vec_day_name(&input, &mut ctx).unwrap().into_inner(),
        vec![Some("Saturday".into()), None]
    );
    assert_eq!(
        vec_month_name(&input, &mut ctx).unwrap().into_inner(),
        vec![Some("February".into()), None]
    );
    assert_eq!(
        vec_last_day(&input, &mut ctx).unwrap().values()[0],
        Some(t(2024, 2, 29, 0, 0, 0))
    );

    let years = NullableVec::new(vec![Some(69), Some(70), Some(2024), Some(-1)]);
    let days = NullableVec::new(vec![Some(1), Some(1), Some(60), Some(1)]);
    assert_eq!(
        vec_make_date(&years, &days).unwrap().into_inner(),
        vec![
            Some(t(2069, 1, 1, 0, 0, 0)),
            Some(t(1970, 1, 1, 0, 0, 0)),
            Some(t(2024, 2, 29, 0, 0, 0)),
            None,
        ]
    );
}

/// GET_FORMAT 地区名大小写不敏感，未知地区返回空串。
#[test]
fn get_format_is_location_case_insensitive() {
    assert_eq!(get_format("DATE", "usa"), "%m.%d.%Y");
    assert_eq!(get_format("DATETIME", "EuR"), "%Y-%m-%d %H.%i.%s");
    assert_eq!(get_format("TIME", "internal"), "%H%i%s");
    assert_eq!(get_format("DATE", "unknown"), "");
}

/// PERIOD_ADD/DIFF 遵循 MySQL 会计期规则，非法期报错，NULL 传播。
#[test]
fn period_arithmetic_matches_mysql_rules_and_null_ordering() {
    let periods = NullableVec::new(vec![Some(202401), Some(6912), None]);
    let offsets = NullableVec::new(vec![Some(1), Some(1), Some(99)]);
    assert_eq!(
        vec_period_add(&periods, &offsets).unwrap().into_inner(),
        vec![Some(202402), Some(207001), None]
    );
    assert_eq!(
        vec_period_diff(
            &NullableVec::new(vec![Some(202402)]),
            &NullableVec::new(vec![Some(202312)])
        )
        .unwrap()
        .into_inner(),
        vec![Some(2)]
    );
    assert!(
        vec_period_add(
            &NullableVec::new(vec![Some(202413)]),
            &NullableVec::new(vec![Some(1)])
        )
        .is_err()
    );
}

/// TIME 分量保留符号/小数；SEC_TO_TIME 超上限钳位并告警。
#[test]
fn duration_functions_keep_sign_fraction_and_mysql_clamp() {
    let input = NullableVec::new(vec![Some(MysqlDuration::from_micros(-3_723_456_789)), None]);
    assert_eq!(vec_hour(&input).into_inner(), vec![Some(1), None]);
    assert_eq!(vec_minute(&input).into_inner(), vec![Some(2), None]);
    assert_eq!(vec_second(&input).into_inner(), vec![Some(3), None]);
    assert_eq!(
        vec_microsecond(&input).into_inner(),
        vec![Some(456_789), None]
    );
    assert_eq!(
        vec_time_to_sec(&input).into_inner(),
        vec![Some(-3723), None]
    );

    let mut ctx = EvalContext::default();
    let out = vec_sec_to_time(&NullableVec::new(vec![Some(3_020_400.5)]), 6, &mut ctx).unwrap();
    assert_eq!(
        out.values()[0],
        Some(MysqlDuration::from_micros(3_020_399_000_000))
    );
    assert_eq!(ctx.warnings.len(), 1);
}

/// MAKETIME 校验分秒范围，并对超大小时钳位到 MySQL 上限。
#[test]
fn make_time_validates_minute_second_and_unsigned_hour() {
    let mut ctx = EvalContext::default();
    let out = vec_make_time(
        &NullableVec::new(vec![Some(-12), Some(5), Some(839)]),
        &NullableVec::new(vec![Some(30), Some(60), Some(1)]),
        &NullableVec::new(vec![Some(1.25), Some(1.0), Some(2.0)]),
        false,
        6,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(
        out.values()[0],
        Some(MysqlDuration::from_micros(-45_001_250_000))
    );
    assert_eq!(out.values()[1], None);
    assert_eq!(
        out.values()[2],
        Some(MysqlDuration::from_micros(3_020_399_000_000))
    );
    assert_eq!(ctx.warnings.len(), 0);

    let unsigned = vec_make_time(
        &NullableVec::new(vec![Some(-1)]),
        &NullableVec::new(vec![Some(0)]),
        &NullableVec::new(vec![Some(0.0)]),
        true,
        0,
        &mut ctx,
    )
    .unwrap();
    assert_eq!(
        unsigned.into_inner(),
        vec![Some(MysqlDuration::from_micros(3_020_399_000_000))]
    );
}

/// STATEMENT_TIMESTAMP、TSO 解析与有界陈旧读的向量化行为。
#[test]
fn timestamps_tso_timezone_and_bounded_staleness_are_vectorized() {
    let instant = Utc.with_ymd_and_hms(2024, 1, 2, 3, 4, 5).unwrap();
    let current = vec_statement_timestamp(3, instant, "Asia/Shanghai", 0).unwrap();
    assert_eq!(current.values(), &[Some(t(2024, 1, 2, 11, 4, 5)); 3]);

    let tso = ((instant.timestamp_millis() as u64) << 18) | 7;
    assert_eq!(
        vec_parse_tso(
            &NullableVec::new(vec![Some(tso as i64), Some(0), Some(-1), None]),
            "UTC"
        )
        .unwrap()
        .into_inner(),
        vec![Some(t(2024, 1, 2, 3, 4, 5)), None, None, None]
    );

    let min = NullableVec::new(vec![
        Some(t(2024, 1, 1, 0, 0, 0)),
        Some(t(2024, 1, 3, 0, 0, 0)),
    ]);
    let max = NullableVec::new(vec![
        Some(t(2024, 1, 4, 0, 0, 0)),
        Some(t(2024, 1, 2, 0, 0, 0)),
    ]);
    let safe = t(2024, 1, 2, 0, 0, 0);
    assert_eq!(
        vec_bounded_staleness(&min, &max, safe)
            .unwrap()
            .into_inner(),
        vec![Some(safe), None]
    );
    assert_eq!(
        vec_bounded_staleness(&min, &max, t(2023, 12, 31, 0, 0, 0))
            .unwrap()
            .into_inner(),
        vec![Some(t(2024, 1, 1, 0, 0, 0)), None]
    );
    assert_eq!(
        vec_bounded_staleness(&min, &max, t(2024, 1, 5, 0, 0, 0))
            .unwrap()
            .into_inner(),
        vec![Some(t(2024, 1, 4, 0, 0, 0)), None]
    );
}

/// `vec_map` 保持行序与 NULL，并正确上抛映射错误。
#[test]
fn generic_vector_delegation_preserves_row_order_nulls_and_errors() {
    let input = NullableVec::new(vec![Some(2_i64), None, Some(4)]);
    let out = vec_map(&input, |value| Ok(value * 3)).unwrap();
    assert_eq!(out.into_inner(), vec![Some(6), None, Some(12)]);

    let err = vec_map(&input, |value| {
        if *value == 4 {
            Err(TimeVecError::Overflow("boom".into()))
        } else {
            Ok(*value)
        }
    });
    assert!(matches!(err, Err(TimeVecError::Overflow(message)) if message == "boom"));
}

/// 供时间向量化 Go 同名迁移入口复用的完整回归集合。
pub(crate) fn run_time_vector_parity_suite() {
    calendar_extractors_preserve_nulls_and_mysql_indexes();
    date_obeys_no_zero_modes_without_losing_non_strict_rows();
    names_last_day_and_make_date_follow_go_edges();
    get_format_is_location_case_insensitive();
    period_arithmetic_matches_mysql_rules_and_null_ordering();
    duration_functions_keep_sign_fraction_and_mysql_clamp();
    make_time_validates_minute_second_and_unsigned_hour();
    timestamps_tso_timezone_and_bounded_staleness_are_vectorized();
    generic_vector_delegation_preserves_row_order_nulls_and_errors();
}

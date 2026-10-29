// Copyright 2026 AsterSQL.
use super::pg_temporal::{PgDate, PgTemporalError};
#[test]
fn calendar_dates_validate_before_encoding() {
    for (text, days) in [
        ("2024-02-29", 8825i32),
        ("2000-02-29", 59),
        ("2000-01-01", 0),
        ("1999-12-31", -1),
    ] {
        let date = text.parse::<PgDate>().unwrap();
        assert_eq!(date.to_binary(), days.to_be_bytes());
        assert_eq!(PgDate::from_binary(&days.to_be_bytes()).unwrap(), date);
        assert_eq!(date.to_string(), text);
    }
    for (year, month, day) in [
        (2023, 2, 29),
        (1900, 2, 29),
        (2024, 4, 31),
        (2024, 0, 1),
        (2024, 13, 1),
        (2024, 1, 0),
        (2024, 1, 32),
        (0, 0, 0),
    ] {
        assert_eq!(
            PgDate::from_ymd(year, month, day),
            Err(PgTemporalError::Range)
        );
    }
}

#[test]
fn gregorian_era_roundtrip_matches_independent_calendar() {
    use chrono::{Datelike, NaiveDate};
    let epoch = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
    // A full Gregorian era includes non-leap centuries and the divisible-by-400 leap year.
    for days in -146097i32..=146097 {
        let expected = epoch + chrono::Duration::days(i64::from(days));
        let date = PgDate::from_ymd(expected.year(), expected.month(), expected.day()).unwrap();
        assert_eq!(date.to_binary(), days.to_be_bytes());
        assert_eq!(
            PgDate::from_binary(&days.to_be_bytes())
                .unwrap()
                .to_string(),
            expected.format("%Y-%m-%d").to_string()
        );
    }
}

#[test]
fn finite_day_representation_preserves_calendar_eras() {
    use chrono::{Datelike, NaiveDate};
    let epoch = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();
    for (year, month, day) in [
        (-400, 2, 28),
        (-1, 2, 29),
        (-1, 12, 31),
        (1, 1, 1),
        (100, 3, 1),
    ] {
        let astronomical = if year < 0 { year + 1 } else { year };
        let independent = NaiveDate::from_ymd_opt(astronomical, month, day).unwrap();
        let date = PgDate::from_ymd(year, month, day).unwrap();
        assert_eq!(
            date.to_binary(),
            (independent.signed_duration_since(epoch).num_days() as i32).to_be_bytes()
        );
        assert_eq!(date.to_string().parse::<PgDate>().unwrap(), date);
        assert_eq!(PgDate::from_binary(&date.to_binary()).unwrap(), date);
        assert_eq!(independent.month(), month);
    }
}

#[test]
fn special_dates_and_numeric_field_orders() {
    use super::pg_temporal::{PgDateContext, PgDateOrder};
    let today = PgDate::from_ymd(2000, 1, 1).unwrap();
    for (order, expected) in [
        (PgDateOrder::Mdy, "2003-01-02"),
        (PgDateOrder::Dmy, "2003-02-01"),
        (PgDateOrder::Ymd, "2001-02-03"),
    ] {
        let context = PgDateContext {
            order,
            today,
            time_micros: 0,
        };
        assert_eq!(
            PgDate::parse_with_context("01/02/03", context)
                .unwrap()
                .to_string(),
            expected
        );
        for (input, expected) in [
            ("now", "2000-01-01"),
            ("today", "2000-01-01"),
            ("yesterday", "1999-12-31"),
            ("tomorrow", "2000-01-02"),
            ("epoch", "1970-01-01"),
            ("infinity", "infinity"),
            ("-infinity", "-infinity"),
            ("2024-02-29", "2024-02-29"),
        ] {
            let date = PgDate::parse_with_context(input, context).unwrap();
            assert_eq!(date.to_string(), expected);
            assert_eq!(PgDate::from_binary(&date.to_binary()).unwrap(), date);
        }
        for bad in ["allballs", "", "garbage"] {
            assert_eq!(
                PgDate::parse_with_context(bad, context),
                Err(PgTemporalError::Syntax)
            );
        }
        for bad in ["0000/01/01", "0000-01-01"] {
            assert_eq!(
                PgDate::parse_with_context(bad, context),
                Err(PgTemporalError::Range)
            );
        }
    }
    for (input, bytes) in [
        ("-infinity", i32::MIN.to_be_bytes()),
        ("infinity", i32::MAX.to_be_bytes()),
    ] {
        assert_eq!(input.parse::<PgDate>().unwrap().to_binary(), bytes);
    }
    for (today, word) in [
        (PgDate::from_ymd(-4714, 11, 24).unwrap(), "yesterday"),
        (PgDate::from_ymd(5874897, 12, 31).unwrap(), "tomorrow"),
    ] {
        assert_eq!(
            PgDate::parse_with_context(
                word,
                PgDateContext {
                    order: PgDateOrder::Mdy,
                    today,
                    time_micros: 0,
                }
            ),
            Err(PgTemporalError::Range)
        );
    }
}

#[test]
fn pg_temporal_time_day_canonical_domain() {
    use super::pg_temporal::PgTime;
    for (text, micros) in [
        ("00:00:00", 0),
        ("12:34:56.123456", 45296123456),
        ("23:59:59.999999", 86399999999),
        ("24:00:00", 86400000000),
    ] {
        let time = text.parse::<PgTime>().unwrap();
        assert_eq!(time.micros(), micros);
        assert_eq!(time.to_binary(), micros.to_be_bytes());
        assert_eq!(PgTime::from_binary(&time.to_binary()).unwrap(), time);
        assert_eq!(time.to_string(), text);
    }
    assert_ne!(
        PgTime::from_micros(0).unwrap(),
        PgTime::from_micros(PgTime::END_OF_DAY).unwrap()
    );
    for value in [i64::MIN, -1, 86400000001, i64::MAX] {
        assert_eq!(PgTime::from_micros(value), Err(PgTemporalError::Range));
        assert_eq!(
            PgTime::from_binary(&value.to_be_bytes()),
            Err(PgTemporalError::Range)
        );
    }
    for text in ["24:00:00.000001", "25:00:00", "12:60:00", "12:00:61"] {
        assert_eq!(text.parse::<PgTime>(), Err(PgTemporalError::Range));
    }
    assert_eq!(
        PgTime::from_binary(&[0; 7]),
        Err(PgTemporalError::Truncated)
    );
    assert_eq!(PgTime::from_binary(&[0; 9]), Err(PgTemporalError::Trailing));
}

#[test]
fn pg_temporal_time_precision_rounding() {
    use super::pg_temporal::PgTime;
    for (text, expected) in [
        ("12:34:56.1234567", "12:34:56.123457"),
        ("12:34:56.1234565", "12:34:56.123456"),
        ("12:34:56.1234575", "12:34:56.123458"),
        ("23:59:59.9999999", "24:00:00"),
    ] {
        assert_eq!(text.parse::<PgTime>().unwrap().to_string(), expected);
    }
    for p in 0..=6 {
        let value = "23:59:59.999999".parse::<PgTime>().unwrap();
        assert_eq!(
            value.round_precision(p).micros(),
            if p == 6 { 86399999999 } else { 86400000000 }
        );
        let zero = PgTime::from_micros(0).unwrap();
        assert_eq!(zero.round_precision(p), zero);
    }
    assert_eq!(
        "24:00:00.0000001".parse::<PgTime>().unwrap().to_string(),
        "24:00:00"
    );
    assert!("24:00:00.0000009".parse::<PgTime>().is_err());
}

#[test]
fn pg_temporal_time_text_zone() {
    use super::pg_temporal::PgTime;
    for (text, expected) in [
        ("04:05", "04:05:00"),
        ("040506", "04:05:06"),
        ("0405", "04:05:00"),
        ("04:05 PM", "16:05:00"),
        ("12:00 am", "00:00:00"),
        ("12:00 PM", "12:00:00"),
        ("ALLBALLS", "00:00:00"),
        ("04:05:06+08:00", "04:05:06"),
        ("04:05:06-08:00", "04:05:06"),
        ("04:05:06+0800", "04:05:06"),
        ("04:05:06+08", "04:05:06"),
        ("04:05:06-15:59:59", "04:05:06"),
        ("24:00", "24:00:00"),
        ("040506.1234567", "04:05:06.123457"),
    ] {
        let time = text.parse::<PgTime>().unwrap();
        assert_eq!(time.to_string(), expected, "{text}");
        assert_eq!(PgTime::from_binary(&time.to_binary()).unwrap(), time);
    }
    for text in ["13:05 PM", "24:01", "04:60", "246000"] {
        assert_eq!(
            text.parse::<PgTime>(),
            Err(PgTemporalError::Range),
            "{text}"
        );
    }
    for text in [
        "",
        "garbage",
        "04:05 XM",
        "04:05:06+xx",
        "04050",
        "04:05:06+08:00junk",
    ] {
        assert_eq!(
            text.parse::<PgTime>(),
            Err(PgTemporalError::Syntax),
            "{text}"
        );
    }
}

#[test]
fn pg_temporal_time_numeric_zone_validation() {
    use super::pg_temporal::PgTime;
    for text in ["04:05:06+16:00", "04:05:06-08:60", "04:05:06+08:00:60"] {
        let error = text.parse::<PgTime>().unwrap_err();
        assert_eq!(error, PgTemporalError::ZoneRange);
        assert_eq!(error.sqlstate(), "22009");
    }
}

#[test]
fn pg_temporal_time_duration_bridge() {
    use super::pg_temporal::PgTime;
    use super::time::{Duration, ParseDuration, StrictContext};
    for (text, fsp, micros) in [
        ("00:00:00", 0, 0i64),
        ("12:34:56.123456", 6, 45_296_123_456),
        ("23:59:59.999999", 6, 86_399_999_999),
        ("24:00:00", 0, 86_400_000_000),
    ] {
        let pg: PgTime = text.parse().unwrap();
        let native = pg.to_mysql_duration(fsp).unwrap();
        assert_eq!(native.Duration, micros * 1_000);
        assert_eq!(native.Fsp, fsp);
        assert_eq!(native.String(), text);
        assert_eq!(PgTime::from_mysql_duration(native).unwrap(), pg);
        assert_eq!(
            ParseDuration(&StrictContext, text, fsp).unwrap(),
            (native, false)
        );
    }
    for text in ["-00:00:01", "25:00:00", "838:59:59"] {
        let native = ParseDuration(&StrictContext, text, 0).unwrap().0;
        assert_eq!(native.String(), text);
        assert_eq!(
            PgTime::from_mysql_duration(native),
            Err(PgTemporalError::DurationBridge)
        );
    }
    for native in [
        Duration {
            Duration: 1,
            Fsp: 6,
        },
        Duration {
            Duration: 1_000,
            Fsp: 0,
        },
        Duration {
            Duration: 0,
            Fsp: 7,
        },
        Duration {
            Duration: 0,
            Fsp: -1,
        },
    ] {
        assert_eq!(
            PgTime::from_mysql_duration(native),
            Err(PgTemporalError::DurationBridge)
        );
    }
    let pg: PgTime = "12:34:56.123456".parse().unwrap();
    for fsp in [-1, 0, 3, 5, 7] {
        assert_eq!(
            pg.to_mysql_duration(fsp),
            Err(PgTemporalError::DurationBridge)
        );
    }
}

#[test]
fn pg_timestamp_epoch_precision_and_domain() {
    use super::pg_temporal::{PgTemporalError, PgTimestamp};
    for (text, micros) in [
        ("1999-12-31 23:59:59.999999", -1i64),
        ("2000-01-01 00:00:00", 0),
        ("2000-01-01 00:00:00.000001", 1),
        ("2000-01-01 00:00:00.1234567", 123457),
        ("4714-11-24 00:00:00 BC", PgTimestamp::MIN),
        ("294276-12-31 23:59:59.999999", PgTimestamp::END - 1),
        ("-infinity", i64::MIN),
        ("infinity", i64::MAX),
    ] {
        let value: PgTimestamp = text.parse().unwrap();
        assert_eq!(value.micros(), micros, "{text}");
        assert_eq!(PgTimestamp::from_binary(&value.to_binary()).unwrap(), value);
        assert_eq!(value.to_string().parse::<PgTimestamp>().unwrap(), value);
    }
    for (micros, expected) in [
        (-500000, -1000000),
        (500000, 1000000),
        (-499999, 0),
        (499999, 0),
    ] {
        assert_eq!(
            PgTimestamp::from_micros(micros)
                .unwrap()
                .round_precision(0)
                .unwrap()
                .micros(),
            expected
        );
    }
    assert_eq!(
        PgTimestamp::from_micros(PgTimestamp::MIN - 1),
        Err(PgTemporalError::Range)
    );
    assert_eq!(
        PgTimestamp::from_micros(PgTimestamp::END),
        Err(PgTemporalError::Range)
    );
    assert_eq!(
        PgTimestamp::from_micros(PgTimestamp::END - 1)
            .unwrap()
            .round_precision(0),
        Err(PgTemporalError::Range)
    );
    assert_eq!(
        PgTimestamp::from_binary(&[0; 7]),
        Err(PgTemporalError::Truncated)
    );
    assert_eq!(
        PgTimestamp::from_binary(&[0; 9]),
        Err(PgTemporalError::Trailing)
    );
}

#[test]
fn pg_timestamp_zone_identity() {
    use crate::pg_temporal::{PgTemporalError, PgTimestamp};
    for suffix in [
        "+02",
        "-0500",
        "+15:59:59",
        "Z",
        " UTC",
        " America/New_York",
        " America/Port-au-Prince",
        " Etc/GMT+5",
        " +02",
    ] {
        let value: PgTimestamp = format!("2024-03-10 02:30:00{suffix}").parse().unwrap();
        assert_eq!(value.to_string(), "2024-03-10 02:30:00");
        assert_eq!(value.micros(), 763353000000000);
    }
    for (suffix, expected) in [
        ("+16", PgTemporalError::ZoneRange),
        ("+02:60", PgTemporalError::ZoneRange),
        ("+bad", PgTemporalError::Syntax),
        (" Invalid/Zone", PgTemporalError::Syntax),
    ] {
        assert_eq!(
            format!("2024-03-10 02:30:00{suffix}").parse::<PgTimestamp>(),
            Err(expected)
        );
    }
}

#[test]
fn pg_timestamp_special_context() {
    use super::pg_temporal::{PgDate, PgDateContext, PgDateOrder, PgTimestamp};
    let context = PgDateContext {
        order: PgDateOrder::Ymd,
        today: PgDate::from_ymd(2000, 1, 1).unwrap(),
        time_micros: 25200123456,
    };
    for (word, expected) in [
        ("now", 25200123456),
        (" TODAY ", 0),
        ("yesterday", -86400000000),
        ("Tomorrow", 86400000000),
        ("epoch", -946684800000000),
        ("infinity", i64::MAX),
        ("-infinity", i64::MIN),
    ] {
        let value = PgTimestamp::parse_with_context(word, context).unwrap();
        assert_eq!(value.micros(), expected);
        assert_eq!(PgTimestamp::from_binary(&value.to_binary()).unwrap(), value);
    }
}

#[test]
fn pg_timestamptz_instant_identity() {
    use super::pg_temporal::PgTimestampTz;
    let epoch: PgTimestampTz = "2000-01-01 00:00:00+00".parse().unwrap();
    assert_eq!(epoch, "2000-01-01 08:00:00+08".parse().unwrap());
    for text in [
        "2000-01-01 08:00:00+08:00",
        "2000-01-01 08:00:00+0800",
        "2000-01-01 08:00:00+8",
    ] {
        assert_eq!(text.parse::<PgTimestampTz>().unwrap(), epoch);
    }
    assert_eq!(epoch.to_binary(), [0; 8]);
    assert_eq!(
        "2000-01-01 00:00:00+08"
            .parse::<PgTimestampTz>()
            .unwrap()
            .micros(),
        -28_800_000_000
    );
    assert_eq!(PgTimestampTz::from_binary(&[0; 8]).unwrap(), epoch);
    assert_eq!(
        epoch.format_with_offset(19800).unwrap(),
        "2000-01-01 05:30:00+05:30"
    );
    for (bytes, state) in [(vec![0; 7], "08P01"), (vec![0; 9], "22P03")] {
        assert_eq!(
            PgTimestampTz::from_binary(&bytes).unwrap_err().sqlstate(),
            state
        );
    }
    for text in [
        "",
        "garbage",
        "2000-02-30 00:00:00+00",
        "0000-01-01 00:00:00+00",
    ] {
        assert!(text.parse::<PgTimestampTz>().is_err());
    }
}

#[test]
fn pg_timestamptz_normalized_range() {
    use super::pg_temporal::{PgTimestamp, PgTimestampTz};
    for (text, micros, offset, display) in [
        (
            "294277-01-01 00:59:59.999999+01",
            PgTimestamp::END - 1,
            3600,
            "294277-01-01 00:59:59.999999+01",
        ),
        (
            "4714-11-23 23:00:00-01 BC",
            PgTimestamp::MIN,
            -3600,
            "4714-11-23 23:00:00-01 BC",
        ),
    ] {
        let value: PgTimestampTz = text.parse().unwrap();
        assert_eq!(value.micros(), micros);
        assert_eq!(value.format_with_offset(offset).unwrap(), display);
        assert_eq!(
            PgTimestampTz::from_binary(&value.to_binary()).unwrap(),
            value
        );
    }
    for text in [
        "4714-11-24 00:00:00+00:00:01 BC",
        "294276-12-31 23:59:59.999999-00:00:01",
    ] {
        assert_eq!(
            text.parse::<PgTimestampTz>().unwrap_err().sqlstate(),
            "22008"
        );
    }
    // The no-zone type and DATE retain their existing finite domains.
    assert!(
        "4714-11-23 BC"
            .parse::<super::pg_temporal::PgDate>()
            .is_err()
    );
    assert!("294277-01-01 00:00:00".parse::<PgTimestamp>().is_err());
}

#[test]
fn pg_timestamptz_invalid_zone_and_payload() {
    use super::pg_temporal::PgTimestampTz;
    for offset in [None, Some(0), Some(28800)] {
        for text in [
            "2000-01-01 00:00:00 Aster/Unknown",
            "2000-01-01 00:00:00 Aster/Unknown-Zone",
            "2000-01-01 00:00:00 Unknown",
            "0001-01-01 00:00:00 Aster/Unknown BC",
        ] {
            assert_eq!(
                PgTimestampTz::parse_with_offset(text, offset)
                    .unwrap_err()
                    .sqlstate(),
                "22023"
            );
        }
    }
    for (text, state) in [
        ("", "22007"),
        ("garbage", "22007"),
        ("2026-01-02 12:34:56+00suffix", "22007"),
        ("2026-01-02 12:34:56+00 garbage", "22007"),
        ("2000-02-30 00:00:00+00", "22008"),
        ("4714-11-24 00:00:00+00:00:01 BC", "22008"),
        ("294276-12-31 23:59:59.999999-00:00:01", "22008"),
    ] {
        assert_eq!(
            PgTimestampTz::parse_with_offset(text, Some(0))
                .unwrap_err()
                .sqlstate(),
            state
        );
    }
    assert_eq!(
        PgTimestampTz::from_binary(&[0; 7]).unwrap_err().sqlstate(),
        "08P01"
    );
    assert_eq!(
        PgTimestampTz::from_binary(&[0; 9]).unwrap_err().sqlstate(),
        "22P03"
    );
}

#[test]
fn pg_interval_components_preserve_dimensions() {
    use super::pg_temporal::PgInterval;
    for (text, components) in [
        ("1 month", (1, 0, 0)),
        ("30 days", (0, 30, 0)),
        ("1 day", (0, 1, 0)),
        ("24 hours", (0, 0, 86_400_000_000)),
        ("1.5 months", (1, 15, 0)),
        ("-1.5 months", (-1, -15, 0)),
        ("0.5 days", (0, 0, 43_200_000_000)),
        (
            "1 year 2 months 3 days 04:05:06.123456",
            (14, 3, 14_706_123_456),
        ),
    ] {
        let value = text.parse::<PgInterval>().unwrap();
        assert_eq!(value.components(), components, "{text}");
        assert_eq!(PgInterval::from_binary(&value.to_binary()).unwrap(), value);
        assert_eq!(value.to_string().parse::<PgInterval>().unwrap(), value);
    }
    for parts in [
        (i32::MIN, 0, 0),
        (i32::MAX, 0, 0),
        (0, i32::MIN, 0),
        (0, i32::MAX, 0),
        (0, 0, i64::MIN),
        (0, 0, i64::MAX),
        (i32::MIN, i32::MIN, i64::MIN),
        (i32::MAX, i32::MAX, i64::MAX),
    ] {
        let value = PgInterval::from_components(parts.0, parts.1, parts.2);
        assert_eq!(PgInterval::from_binary(&value.to_binary()).unwrap(), value);
        assert_eq!(
            value.to_string().parse::<PgInterval>().unwrap(),
            value,
            "{parts:?}"
        );
    }
    for bad in [
        "",
        "garbage",
        "1 day 2 days",
        "1:60:00",
        "1:00:60",
        "1:-1:00",
        "1:-999999999999999999999999999999999999:00",
        "1 monthssss",
    ] {
        assert_eq!(
            bad.parse::<PgInterval>().unwrap_err().sqlstate(),
            "22007",
            "{bad}"
        );
    }
    for bad in [
        "2147483648 months",
        "-1 second 9223372036854775808 microseconds",
        "-1 week 2147483648 days",
        "-0.5 years 2147483648 months",
        "-1 second 2562047788:00:54.775808",
        "-2147483649 days",
        "9223372036854775808 microseconds",
    ] {
        assert_eq!(bad.parse::<PgInterval>().unwrap_err().sqlstate(), "22015");
    }
    assert_eq!(
        PgInterval::from_binary(&[0; 15]).unwrap_err().sqlstate(),
        "08P01"
    );
    assert_eq!(
        PgInterval::from_binary(&[0; 17]).unwrap_err().sqlstate(),
        "22P03"
    );
}

#[test]
fn pg_interval_signs_style() {
    use super::pg_temporal::{PgInterval, PgIntervalStyle as S};
    for (style, cases) in [
        (
            S::Postgres,
            [
                "-1 mons +2 days -03:04:05.123456",
                "-1 mons -2 days -03:04:05.123456",
                "00:00:00",
            ],
        ),
        (
            S::PostgresVerbose,
            [
                "@ 1 mon -2 days 3 hours 4 mins 5.123456 secs ago",
                "@ 1 mon 2 days 3 hours 4 mins 5.123456 secs ago",
                "@ 0",
            ],
        ),
        (
            S::SqlStandard,
            ["-0-1 +2 -3:04:05.123456", "-0-1 -2 -3:04:05.123456", "0"],
        ),
        (
            S::Iso8601,
            [
                "P-1M2DT-3H-4M-5.123456S",
                "P-1M-2DT-3H-4M-5.123456S",
                "PT0S",
            ],
        ),
    ] {
        for (text, parts) in
            cases
                .into_iter()
                .zip([(-1, 2, -11045123456), (-1, -2, -11045123456), (0, 0, 0)])
        {
            let v = PgInterval::parse_with_style(text, style).unwrap();
            assert_eq!(v.components(), parts, "{text}");
            assert_eq!(v.format_with_style(style), text);
        }
        for parts in [
            (i32::MIN, 0, 0),
            (0, i32::MIN, 0),
            (0, 0, i64::MIN),
            (i32::MAX, i32::MIN, i64::MAX),
            (0, 2, 0),
            (1, 2, 11045000000),
            (0, -2, -11045000000),
            (0, 1, -1),
        ] {
            let v = PgInterval::from_components(parts.0, parts.1, parts.2);
            let text = v.format_with_style(style);
            assert_eq!(
                PgInterval::parse_with_style(&text, style)
                    .unwrap_or_else(|e| panic!("{style:?} {text}: {e:?}"))
                    .components(),
                parts,
                "{style:?} {text}"
            );
        }
    }
    for (style, t) in [(S::Postgres, 7384000000), (S::SqlStandard, -7384000000)] {
        assert_eq!(
            PgInterval::parse_with_style("-1 2:03:04", style)
                .unwrap()
                .components(),
            (0, -1, t)
        );
    }
    for (parts, expected) in [
        ((1, 2, 11045000000), "+0-1 +2 +3:04:05"),
        ((0, 1, 0), "1 0:00:00"),
        ((0, -1, 0), "-1 0:00:00"),
        ((1, -2, 0), "+0-1 -2 +0:00:00"),
        ((0, -2, -10800000000), "-2 3:00:00"),
    ] {
        assert_eq!(
            PgInterval::from_components(parts.0, parts.1, parts.2)
                .format_with_style(S::SqlStandard),
            expected
        );
    }
    for bad in ["P", "PT", "P1DT", "P1Q", "@ bad ago", "1-", "1 day 2 days"] {
        assert_eq!(
            PgInterval::parse_with_style(bad, S::Postgres)
                .unwrap_err()
                .sqlstate(),
            "22007",
            "{bad}"
        );
    }
}

#[test]
fn pg_interval_typmod_fields_precision() {
    use super::pg_temporal::PgInterval;
    for (micros, p, rounded) in [
        (1234500, 3, 1235000),
        (-1234500, 3, -1235000),
        (500000, 0, 1000000),
        (-500000, 0, -1000000),
        (59999999, 0, 60000000),
        (1234567, 6, 1234567),
    ] {
        assert_eq!(
            PgInterval::from_components(14, 1, micros)
                .apply_typmod(-1, p)
                .unwrap()
                .components(),
            (14, 1, rounded)
        );
    }
    assert_eq!(
        PgInterval::from_components(14, 1, 7384000000)
            .apply_typmod(3072, -1)
            .unwrap()
            .components(),
        (14, 1, 7380000000)
    );
    assert_eq!(
        PgInterval::from_components(-14, 1, 1)
            .apply_typmod(4, -1)
            .unwrap()
            .components(),
        (-12, 0, 0)
    );
    assert_eq!(
        PgInterval::from_components(0, 0, i64::MAX).apply_typmod(-1, 0),
        Err(PgTemporalError::Range)
    );
    for (mask, expected) in [
        (4, (12, 0, 0)),
        (2, (14, 0, 0)),
        (6, (14, 0, 0)),
        (8, (14, 2, 0)),
        (1024, (14, 2, 7200000000)),
        (1032, (14, 2, 7200000000)),
        (2048, (14, 2, 7380000000)),
        (3072, (14, 2, 7380000000)),
        (3080, (14, 2, 7380000000)),
        (4096, (14, 2, 7384567890)),
        (6144, (14, 2, 7384567890)),
        (7168, (14, 2, 7384567890)),
        (7176, (14, 2, 7384567890)),
    ] {
        assert_eq!(
            PgInterval::from_components(14, 2, 7384567890)
                .apply_typmod(mask, -1)
                .unwrap()
                .components(),
            expected
        );
    }
    for special in ["infinity", "-infinity"] {
        let value = special.parse::<PgInterval>().unwrap();
        assert_eq!(value.apply_typmod(4, 0).unwrap(), value);
    }
}

#[test]
fn pg_interval_range_special_errors() {
    use crate::pg_temporal::{PgInterval, PgTemporalError};
    for text in [
        "2147483648 months",
        "-1 second 9223372036854775808 microseconds",
        "-1 week 2147483648 days",
        "-0.5 years 2147483648 months",
        "-1 second 2562047788:00:54.775808",
        "2147483648 months -1 year",
        "2147483648 years",
        "2147483647 days 1 week",
        "9223372036854775807 microseconds 1 second",
        "9223372036854775808 microseconds",
    ] {
        assert_eq!(
            text.parse::<PgInterval>(),
            Err(PgTemporalError::IntervalOverflow),
            "{text}"
        );
    }
    for text in [
        "2147483647 months 1 year",
        "-2147483648 months -1 year",
        "178956971 years",
    ] {
        assert_eq!(
            text.parse::<PgInterval>(),
            Err(PgTemporalError::Range),
            "{text}"
        );
    }
    assert_eq!(
        "1.5 years".parse::<PgInterval>().unwrap().components(),
        (18, 0, 0)
    );
    assert_eq!(
        "2147483647 days 24 hours"
            .parse::<PgInterval>()
            .unwrap()
            .components(),
        (0, i32::MAX, 86_400_000_000)
    );
    for t in [i64::MIN, i64::MAX] {
        let finite = PgInterval::from_components(0, 0, t);
        assert_eq!(
            PgInterval::from_binary(&finite.to_binary()).unwrap(),
            finite
        );
        assert_eq!(finite.apply_typmod(-1, 0), Err(PgTemporalError::Range));
    }
}

#[test]
fn pg_temporal_iv_bridge_duration_limits() {
    use super::pg_temporal::PgInterval;
    use super::time::{Duration, MaxTime, MinTime, ParseDuration, StrictContext};
    for text in [
        "-51:00:00",
        "00:00:00",
        "838:59:59",
        "-838:59:59",
        "00:00:00.000001",
    ] {
        let duration = ParseDuration(&StrictContext, text, 6).unwrap().0;
        let pg = PgInterval::from_mysql_duration(duration).unwrap();
        assert_eq!(pg.components(), (0, 0, duration.Duration / 1_000));
        assert_eq!(pg.to_mysql_duration(6).unwrap(), duration);
    }
    for duration in [
        Duration {
            Duration: MaxTime + 1_000,
            Fsp: 6,
        },
        Duration {
            Duration: MinTime - 1_000,
            Fsp: 6,
        },
        Duration {
            Duration: i64::MIN,
            Fsp: 6,
        },
        Duration {
            Duration: i64::MAX,
            Fsp: 6,
        },
        Duration {
            Duration: 1,
            Fsp: 6,
        },
        Duration {
            Duration: -1,
            Fsp: 6,
        },
        Duration {
            Duration: 1_000,
            Fsp: 0,
        },
        Duration {
            Duration: 0,
            Fsp: -1,
        },
        Duration {
            Duration: 0,
            Fsp: 7,
        },
    ] {
        let error = PgInterval::from_mysql_duration(duration).unwrap_err();
        assert_eq!(error, PgTemporalError::DurationBridge);
        assert_eq!(error.sqlstate(), "0A000");
    }
    for pg in [
        PgInterval::from_components(1, 0, 0),
        PgInterval::from_components(0, 1, 0),
        PgInterval::from_components(-1, 30, 0),
        PgInterval::from_components(0, -1, 86_400_000_000),
        PgInterval::from_components(0, 0, MaxTime / 1_000 + 1),
        PgInterval::from_components(0, 0, MinTime / 1_000 - 1),
        PgInterval::from_components(0, 0, i64::MAX),
        PgInterval::from_components(0, 0, i64::MIN),
    ] {
        assert_eq!(
            pg.to_mysql_duration(6),
            Err(PgTemporalError::DurationBridge)
        );
    }
    for fsp in [-1, 0, 3, 5, 7] {
        for micros in [-1, 1] {
            assert_eq!(
                PgInterval::from_components(0, 0, micros).to_mysql_duration(fsp),
                Err(PgTemporalError::DurationBridge)
            );
        }
    }
    for fsp in 0..=6 {
        let micros = -10i64.pow(6 - fsp as u32);
        let pg = PgInterval::from_components(0, 0, micros);
        let duration = pg.to_mysql_duration(fsp).unwrap();
        assert_eq!(duration.Fsp, fsp);
        assert_eq!(duration.Duration, micros * 1_000);
        assert_eq!(PgInterval::from_mysql_duration(duration).unwrap(), pg);
    }
}

#[test]
fn pg_interval_duplicate_field_overflow_precedes_syntax() {
    use super::pg_temporal::PgInterval;
    for text in [
        "2147483647 months 1 month",
        "1 month 2147483647 months",
        "2147483647 days 1 day",
        "-2147483648 days -1 day",
        "9223372036854775807 microseconds 1 microsecond",
    ] {
        assert_eq!(
            text.parse::<PgInterval>().unwrap_err().sqlstate(),
            "22015",
            "{text}"
        );
    }
    for text in [
        "2 months 1 month",
        "2147483646 months 1 month",
        "2147483647 months -1 month",
        "2147483647 days 0 days",
    ] {
        assert_eq!(
            text.parse::<PgInterval>().unwrap_err().sqlstate(),
            "22007",
            "{text}"
        );
    }
}

#[test]
fn pg05_array_time_scalar_named_zone_diagnostics() {
    use super::pg_temporal::PgTime;
    // PostgreSQL 18.6 ParseDateTime/DecodeTimeOnly distinguishes date-shaped
    // zone names from ordinary words and malformed numeric displacements.
    for (text, state) in [
        ("not-a-time", "22023"),
        ("not-an-hour", "22023"),
        ("bad-clock", "22023"),
        ("Missing/Zone", "22023"),
        ("missing.zone", "22023"),
        ("12:00:00 Missing/Zone", "22023"),
        ("12:00:00 not-a-time", "22023"),
        ("garbage", "22007"),
        ("not_a_time", "22007"),
        ("America/New_York", "22007"),
        ("america/new_york", "22007"),
        ("12:00:00 America/New_York", "22007"),
        ("not-a-time 12:00:00", "22007"),
        ("04:05 XM", "22007"),
        ("04:05:06+xx", "22007"),
        ("04:05:06+08:00junk", "22007"),
        ("12:00:00-foo", "22007"),
        ("12:00:00+16", "22009"),
        ("12:00:00+08:60", "22009"),
        ("25:00:00", "22008"),
    ] {
        assert_eq!(
            text.parse::<PgTime>().unwrap_err().sqlstate(),
            state,
            "{text}"
        );
    }
    for text in ["00:00:00", "24:00:00", "12:00:00+08", "12:00:00-08:00"] {
        assert!(text.parse::<PgTime>().is_ok(), "{text}");
    }
}

#[test]
fn pg05_array_interval_scalar_style_signed_zero_groups() {
    use crate::pg_temporal::{PgInterval, PgIntervalStyle as S};
    // Exact PostgreSQL 18.6 observations for the sign/zero Cartesian product.
    // Other styles must retain the same independent calendar/time components.
    for (parts, style, expected) in [
        (
            (-14, -2, -7384000000),
            S::Postgres,
            "-1 years -2 mons -2 days -02:03:04",
        ),
        ((-14, -2, 0), S::Postgres, "-1 years -2 mons -2 days"),
        (
            (-14, -2, 7384000000),
            S::Postgres,
            "-1 years -2 mons -2 days +02:03:04",
        ),
        (
            (-14, 0, -7384000000),
            S::Postgres,
            "-1 years -2 mons -02:03:04",
        ),
        ((-14, 0, 0), S::Postgres, "-1 years -2 mons"),
        (
            (-14, 0, 7384000000),
            S::Postgres,
            "-1 years -2 mons +02:03:04",
        ),
        (
            (-14, 2, -7384000000),
            S::Postgres,
            "-1 years -2 mons +2 days -02:03:04",
        ),
        ((-14, 2, 0), S::Postgres, "-1 years -2 mons +2 days"),
        (
            (-14, 2, 7384000000),
            S::Postgres,
            "-1 years -2 mons +2 days 02:03:04",
        ),
        ((0, -2, -7384000000), S::Postgres, "-2 days -02:03:04"),
        ((0, -2, 0), S::Postgres, "-2 days"),
        ((0, -2, 7384000000), S::Postgres, "-2 days +02:03:04"),
        ((0, 0, -7384000000), S::Postgres, "-02:03:04"),
        ((0, 0, 0), S::Postgres, "00:00:00"),
        ((0, 0, 7384000000), S::Postgres, "02:03:04"),
        ((0, 2, -7384000000), S::Postgres, "2 days -02:03:04"),
        ((0, 2, 0), S::Postgres, "2 days"),
        ((0, 2, 7384000000), S::Postgres, "2 days 02:03:04"),
        (
            (14, -2, -7384000000),
            S::Postgres,
            "1 year 2 mons -2 days -02:03:04",
        ),
        ((14, -2, 0), S::Postgres, "1 year 2 mons -2 days"),
        (
            (14, -2, 7384000000),
            S::Postgres,
            "1 year 2 mons -2 days +02:03:04",
        ),
        ((14, 0, -7384000000), S::Postgres, "1 year 2 mons -02:03:04"),
        ((14, 0, 0), S::Postgres, "1 year 2 mons"),
        ((14, 0, 7384000000), S::Postgres, "1 year 2 mons 02:03:04"),
        (
            (14, 2, -7384000000),
            S::Postgres,
            "1 year 2 mons 2 days -02:03:04",
        ),
        ((14, 2, 0), S::Postgres, "1 year 2 mons 2 days"),
        (
            (14, 2, 7384000000),
            S::Postgres,
            "1 year 2 mons 2 days 02:03:04",
        ),
        (
            (-14, -2, -7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons 2 days 2 hours 3 mins 4 secs ago",
        ),
        (
            (-14, -2, 0),
            S::PostgresVerbose,
            "@ 1 year 2 mons 2 days ago",
        ),
        (
            (-14, -2, 7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons 2 days -2 hours -3 mins -4 secs ago",
        ),
        (
            (-14, 0, -7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons 2 hours 3 mins 4 secs ago",
        ),
        ((-14, 0, 0), S::PostgresVerbose, "@ 1 year 2 mons ago"),
        (
            (-14, 0, 7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons -2 hours -3 mins -4 secs ago",
        ),
        (
            (-14, 2, -7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons -2 days 2 hours 3 mins 4 secs ago",
        ),
        (
            (-14, 2, 0),
            S::PostgresVerbose,
            "@ 1 year 2 mons -2 days ago",
        ),
        (
            (-14, 2, 7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons -2 days -2 hours -3 mins -4 secs ago",
        ),
        (
            (0, -2, -7384000000),
            S::PostgresVerbose,
            "@ 2 days 2 hours 3 mins 4 secs ago",
        ),
        ((0, -2, 0), S::PostgresVerbose, "@ 2 days ago"),
        (
            (0, -2, 7384000000),
            S::PostgresVerbose,
            "@ 2 days -2 hours -3 mins -4 secs ago",
        ),
        (
            (0, 0, -7384000000),
            S::PostgresVerbose,
            "@ 2 hours 3 mins 4 secs ago",
        ),
        ((0, 0, 0), S::PostgresVerbose, "@ 0"),
        (
            (0, 0, 7384000000),
            S::PostgresVerbose,
            "@ 2 hours 3 mins 4 secs",
        ),
        (
            (0, 2, -7384000000),
            S::PostgresVerbose,
            "@ 2 days -2 hours -3 mins -4 secs",
        ),
        ((0, 2, 0), S::PostgresVerbose, "@ 2 days"),
        (
            (0, 2, 7384000000),
            S::PostgresVerbose,
            "@ 2 days 2 hours 3 mins 4 secs",
        ),
        (
            (14, -2, -7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons -2 days -2 hours -3 mins -4 secs",
        ),
        ((14, -2, 0), S::PostgresVerbose, "@ 1 year 2 mons -2 days"),
        (
            (14, -2, 7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons -2 days 2 hours 3 mins 4 secs",
        ),
        (
            (14, 0, -7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons -2 hours -3 mins -4 secs",
        ),
        ((14, 0, 0), S::PostgresVerbose, "@ 1 year 2 mons"),
        (
            (14, 0, 7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons 2 hours 3 mins 4 secs",
        ),
        (
            (14, 2, -7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons 2 days -2 hours -3 mins -4 secs",
        ),
        ((14, 2, 0), S::PostgresVerbose, "@ 1 year 2 mons 2 days"),
        (
            (14, 2, 7384000000),
            S::PostgresVerbose,
            "@ 1 year 2 mons 2 days 2 hours 3 mins 4 secs",
        ),
        ((-14, -2, -7384000000), S::SqlStandard, "-1-2 -2 -2:03:04"),
        ((-14, -2, 0), S::SqlStandard, "-1-2 -2 +0:00:00"),
        ((-14, -2, 7384000000), S::SqlStandard, "-1-2 -2 +2:03:04"),
        ((-14, 0, -7384000000), S::SqlStandard, "-1-2 +0 -2:03:04"),
        ((-14, 0, 0), S::SqlStandard, "-1-2"),
        ((-14, 0, 7384000000), S::SqlStandard, "-1-2 +0 +2:03:04"),
        ((-14, 2, -7384000000), S::SqlStandard, "-1-2 +2 -2:03:04"),
        ((-14, 2, 0), S::SqlStandard, "-1-2 +2 +0:00:00"),
        ((-14, 2, 7384000000), S::SqlStandard, "-1-2 +2 +2:03:04"),
        ((0, -2, -7384000000), S::SqlStandard, "-2 2:03:04"),
        ((0, -2, 0), S::SqlStandard, "-2 0:00:00"),
        ((0, -2, 7384000000), S::SqlStandard, "+0-0 -2 +2:03:04"),
        ((0, 0, -7384000000), S::SqlStandard, "-2:03:04"),
        ((0, 0, 0), S::SqlStandard, "0"),
        ((0, 0, 7384000000), S::SqlStandard, "2:03:04"),
        ((0, 2, -7384000000), S::SqlStandard, "+0-0 +2 -2:03:04"),
        ((0, 2, 0), S::SqlStandard, "2 0:00:00"),
        ((0, 2, 7384000000), S::SqlStandard, "2 2:03:04"),
        ((14, -2, -7384000000), S::SqlStandard, "+1-2 -2 -2:03:04"),
        ((14, -2, 0), S::SqlStandard, "+1-2 -2 +0:00:00"),
        ((14, -2, 7384000000), S::SqlStandard, "+1-2 -2 +2:03:04"),
        ((14, 0, -7384000000), S::SqlStandard, "+1-2 +0 -2:03:04"),
        ((14, 0, 0), S::SqlStandard, "1-2"),
        ((14, 0, 7384000000), S::SqlStandard, "+1-2 +0 +2:03:04"),
        ((14, 2, -7384000000), S::SqlStandard, "+1-2 +2 -2:03:04"),
        ((14, 2, 0), S::SqlStandard, "+1-2 +2 +0:00:00"),
        ((14, 2, 7384000000), S::SqlStandard, "+1-2 +2 +2:03:04"),
        ((-14, -2, -7384000000), S::Iso8601, "P-1Y-2M-2DT-2H-3M-4S"),
        ((-14, -2, 0), S::Iso8601, "P-1Y-2M-2D"),
        ((-14, -2, 7384000000), S::Iso8601, "P-1Y-2M-2DT2H3M4S"),
        ((-14, 0, -7384000000), S::Iso8601, "P-1Y-2MT-2H-3M-4S"),
        ((-14, 0, 0), S::Iso8601, "P-1Y-2M"),
        ((-14, 0, 7384000000), S::Iso8601, "P-1Y-2MT2H3M4S"),
        ((-14, 2, -7384000000), S::Iso8601, "P-1Y-2M2DT-2H-3M-4S"),
        ((-14, 2, 0), S::Iso8601, "P-1Y-2M2D"),
        ((-14, 2, 7384000000), S::Iso8601, "P-1Y-2M2DT2H3M4S"),
        ((0, -2, -7384000000), S::Iso8601, "P-2DT-2H-3M-4S"),
        ((0, -2, 0), S::Iso8601, "P-2D"),
        ((0, -2, 7384000000), S::Iso8601, "P-2DT2H3M4S"),
        ((0, 0, -7384000000), S::Iso8601, "PT-2H-3M-4S"),
        ((0, 0, 0), S::Iso8601, "PT0S"),
        ((0, 0, 7384000000), S::Iso8601, "PT2H3M4S"),
        ((0, 2, -7384000000), S::Iso8601, "P2DT-2H-3M-4S"),
        ((0, 2, 0), S::Iso8601, "P2D"),
        ((0, 2, 7384000000), S::Iso8601, "P2DT2H3M4S"),
        ((14, -2, -7384000000), S::Iso8601, "P1Y2M-2DT-2H-3M-4S"),
        ((14, -2, 0), S::Iso8601, "P1Y2M-2D"),
        ((14, -2, 7384000000), S::Iso8601, "P1Y2M-2DT2H3M4S"),
        ((14, 0, -7384000000), S::Iso8601, "P1Y2MT-2H-3M-4S"),
        ((14, 0, 0), S::Iso8601, "P1Y2M"),
        ((14, 0, 7384000000), S::Iso8601, "P1Y2MT2H3M4S"),
        ((14, 2, -7384000000), S::Iso8601, "P1Y2M2DT-2H-3M-4S"),
        ((14, 2, 0), S::Iso8601, "P1Y2M2D"),
        ((14, 2, 7384000000), S::Iso8601, "P1Y2M2DT2H3M4S"),
    ] {
        let value = PgInterval::from_components(parts.0, parts.1, parts.2);
        let text = value.format_with_style(style);
        assert_eq!(text, expected, "{parts:?} {style:?}");
        assert_eq!(PgInterval::parse_with_style(&text, style).unwrap(), value);
    }
}

#[test]
fn pg_function_23_timestamp_truncation() {
    use super::pg_temporal::PgTimestamp;
    for (unit, input, expected) in [
        ("day", "2026-10-29 12:34:56", "2026-10-29 00:00:00"),
        ("week", "2021-01-01 12:00:00", "2020-12-28 00:00:00"),
        (
            "second",
            "1999-12-31 23:59:59.999999",
            "1999-12-31 23:59:59",
        ),
        (
            "milliseconds",
            "1999-12-31 23:59:59.999999",
            "1999-12-31 23:59:59.999",
        ),
        ("month", "2024-02-29 23:59:59", "2024-02-01 00:00:00"),
        ("quarter", "2026-10-29 12:34:56", "2026-10-01 00:00:00"),
        ("century", "2000-01-01 12:00:00", "1901-01-01 00:00:00"),
        ("millennium", "2001-01-01 12:00:00", "2001-01-01 00:00:00"),
        ("year", "0001-12-31 12:34:56 BC", "0001-01-01 00:00:00 BC"),
        (
            "century",
            "0001-12-31 12:34:56 BC",
            "0100-01-01 00:00:00 BC",
        ),
        ("DAY", "infinity", "infinity"),
        ("day", "-infinity", "-infinity"),
    ] {
        assert_eq!(
            input
                .parse::<PgTimestamp>()
                .unwrap()
                .truncate(unit)
                .unwrap()
                .to_string(),
            expected
        );
    }
    let infinite = "infinity".parse::<PgTimestamp>().unwrap();
    assert_eq!(
        infinite.truncate("nonsense"),
        Err(PgTemporalError::TruncationUnit)
    );
    assert_eq!(
        infinite.truncate("epoch"),
        Err(PgTemporalError::TruncationUnit)
    );
}

#[test]
fn pg08_02_zone_buckets() {
    use crate::pg_temporal::PgTimestampTz;
    assert_eq!(
        "2024-01-02 01:30 NoSuchZone"
            .parse::<PgTimestampTz>()
            .unwrap_err(),
        crate::pg_temporal::PgTemporalError::Syntax
    );
    assert_eq!(
        "2024-01-02 01:30 NoSuch/Zone"
            .parse::<PgTimestampTz>()
            .unwrap_err(),
        crate::pg_temporal::PgTemporalError::UnknownZone
    );
    let ny = "America/New_York".parse().unwrap();
    let trunc = |text: &str, unit| {
        text.parse::<PgTimestampTz>()
            .unwrap()
            .truncate_in_zone(unit, ny, None)
            .unwrap()
    };
    assert_eq!(
        trunc("2024-03-11 16:00:00+00", "day").micros()
            - trunc("2024-03-10 16:00:00+00", "day").micros(),
        23 * 3_600_000_000
    );
    assert_eq!(
        trunc("2024-11-04 16:00:00+00", "day").micros()
            - trunc("2024-11-03 16:00:00+00", "day").micros(),
        25 * 3_600_000_000
    );
    for (input, expected) in [
        ("2024-11-03 05:30:00+00", "2024-11-03 05:00:00+00"),
        ("2024-11-03 06:30:00+00", "2024-11-03 06:00:00+00"),
    ] {
        assert_eq!(trunc(input, "hour").to_string(), expected);
    }
    assert_eq!(
        "2024-01-02 01:30:00+00"
            .parse::<PgTimestampTz>()
            .unwrap()
            .truncate_in_zone("day", ny, chrono::FixedOffset::east_opt(19800))
            .unwrap()
            .to_string(),
        "2024-01-01 18:30:00+00"
    );
}

#[test]
fn pg08_03_local_timestamp() {
    use super::pg_temporal::PgTimestampTz;
    for (id, wall, zone, fixed, expected) in [
        (
            "T-utc",
            "2024-01-02 01:30:00",
            chrono_tz::UTC,
            None,
            "2024-01-02 01:30:00+00",
        ),
        (
            "T-gap",
            "2024-03-10 02:30:00",
            chrono_tz::America::New_York,
            None,
            "2024-03-10 07:30:00+00",
        ),
        (
            "T-overlap",
            "2024-11-03 01:30:00",
            chrono_tz::America::New_York,
            None,
            "2024-11-03 06:30:00+00",
        ),
        (
            "T-fixed",
            "2024-01-02 01:30:00",
            chrono_tz::UTC,
            chrono::FixedOffset::east_opt(19800),
            "2024-01-01 20:00:00+00",
        ),
        (
            "T-infinity",
            "infinity",
            chrono_tz::America::New_York,
            None,
            "infinity",
        ),
    ] {
        assert_eq!(
            PgTimestampTz::from_local_timestamp(wall.parse().unwrap(), zone, fixed)
                .unwrap()
                .to_string(),
            expected,
            "{id}"
        );
    }
    println!("pg08_03_local_timestamp: 5 scenarios passed");
}

#[test]
fn pg08_04_interval_components() {
    use crate::pg_temporal::{PgInterval, PgTemporalError};
    for (unit, input, expected) in [
        ("year", (27, 4, 18_367_123_456), (24, 0, 0)),
        ("quarter", (-14, -4, -18_367_123_456), (-12, 0, 0)),
        ("century", (-1213, 4, 1), (-1200, 0, 0)),
        ("millennium", (12013, 4, 1), (12000, 0, 0)),
        ("hour", (27, 4, 97_567_123_456), (27, 4, 97_200_000_000)),
        (
            "milliseconds",
            (-27, -4, -18_367_123_456),
            (-27, -4, -18_367_123_000),
        ),
        (
            "microseconds",
            (i32::MIN, 0, i64::MIN),
            (i32::MIN, 0, i64::MIN),
        ),
    ] {
        let value = PgInterval::from_components(input.0, input.1, input.2);
        assert_eq!(value.truncate(unit).unwrap().components(), expected);
    }
    for text in ["infinity", "-infinity"] {
        let value: PgInterval = text.parse().unwrap();
        assert_eq!(value.truncate("hour").unwrap(), value);
        assert_eq!(value.truncate("week").unwrap_err().sqlstate(), "0A000");
    }
    assert_eq!(
        PgInterval::from_components(1, 2, 3).truncate("nonsense"),
        Err(PgTemporalError::TruncationUnit)
    );
}

#[test]
fn pg08_05_timestamp_fields() {
    use super::pg_temporal::PgTimestamp;
    for (source, unit, expected) in [
        ("1969-12-31 23:59:59.999999", "second", Some("59.999999")),
        ("2000-01-01 00:00:00.000001", "microseconds", Some("1")),
        ("2021-01-01 00:00:00", "week", Some("53")),
        ("2019-12-30 00:00:00", "isoyear", Some("2020")),
        ("0001-01-01 00:00:00 BC", "year", Some("-1")),
        ("infinity", "year", Some("Infinity")),
        ("-infinity", "year", Some("-Infinity")),
        ("infinity", "second", None),
    ] {
        assert_eq!(
            source
                .parse::<PgTimestamp>()
                .unwrap()
                .extract_numeric(unit)
                .unwrap()
                .map(|n| n.to_string())
                .as_deref(),
            expected
        );
    }
    for unit in ["timezone_hour", "timezone_minute"] {
        assert!(
            "infinity"
                .parse::<PgTimestamp>()
                .unwrap()
                .extract_numeric(unit)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            "2024-01-02 00:00:40.123456"
                .parse::<PgTimestamp>()
                .unwrap()
                .extract_numeric(unit)
                .unwrap_err()
                .sqlstate(),
            "0A000"
        );
    }
    let finite = "2024-01-02 00:00:40.123456".parse::<PgTimestamp>().unwrap();
    assert_eq!(
        finite.extract_numeric("timezone").unwrap_err().sqlstate(),
        "0A000"
    );
    assert_eq!(
        finite
            .extract_numeric("aster_bad_unit")
            .unwrap_err()
            .sqlstate(),
        "22023"
    );
}

#[test]
fn pg08_06_zone_fields() {
    use crate::pg_temporal::PgTimestampTz;
    let value: PgTimestampTz = "2024-01-02 01:30:40.123456+00".parse().unwrap();
    let offset = chrono::FixedOffset::west_opt(19800).unwrap();
    for (field, expected) in [
        ("hour", "20"),
        ("timezone", "-19800"),
        ("timezone_hour", "-5"),
        ("timezone_minute", "-30"),
        ("second", "40.123456"),
        ("microseconds", "40123456"),
    ] {
        assert_eq!(
            value
                .extract_numeric_in_zone(field, chrono_tz::UTC, Some(offset))
                .unwrap()
                .unwrap()
                .to_string(),
            expected
        );
    }
    for source in ["infinity", "-infinity"] {
        let value: PgTimestampTz = source.parse().unwrap();
        assert!(
            value
                .extract_numeric_in_zone("timezone", chrono_tz::America::New_York, None)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            value
                .extract_numeric_in_zone("aster_bad_unit", chrono_tz::UTC, None)
                .unwrap_err()
                .sqlstate(),
            "22023"
        );
    }
}

#[test]
fn pg08_07_components() {
    use crate::pg_temporal::PgInterval;
    for (source, field, expected) in [
        (
            "27 months 4 days 05:06:07.123456",
            "second",
            Some("7.123456"),
        ),
        ("-0.123456 seconds", "second", Some("-0.123456")),
        ("-0.123456 seconds", "milliseconds", Some("-123.456")),
        ("-80 minutes", "minute", Some("-20")),
        (
            "27 months 4 days 05:06:07.123456",
            "epoch",
            Some("71255167.123456"),
        ),
        ("-15 days", "week", Some("-2")),
        ("-27 months", "quarter", Some("-2")),
        ("-24 months", "quarter", Some("-1")),
        ("0", "quarter", Some("1")),
        ("24001 months", "decade", Some("200")),
        ("24001 months", "century", Some("20")),
        ("24001 months", "millennium", Some("2")),
        ("infinity", "hour", Some("Infinity")),
        ("-infinity", "day", Some("-Infinity")),
        ("infinity", "month", None),
        ("infinity", "week", None),
    ] {
        let interval: PgInterval = source.parse().unwrap();
        assert_eq!(
            interval
                .extract_numeric(field)
                .unwrap()
                .map(|n| n.to_string())
                .as_deref(),
            expected,
            "{source} {field}"
        );
    }
    for source in ["0", "infinity", "-infinity"] {
        let interval: PgInterval = source.parse().unwrap();
        for field in [
            "timezone",
            "timezone_hour",
            "timezone_minute",
            "dow",
            "doy",
            "isodow",
            "isoyear",
            "julian",
        ] {
            assert_eq!(
                interval.extract_numeric(field).unwrap_err().sqlstate(),
                "0A000"
            );
        }
        assert_eq!(
            interval
                .extract_numeric("aster_bad_unit")
                .unwrap_err()
                .sqlstate(),
            "22023"
        );
    }
    let extreme = PgInterval::from_components(i32::MIN, i32::MAX, i64::MIN);
    assert_eq!(
        extreme
            .extract_numeric("quarter")
            .unwrap()
            .unwrap()
            .to_string(),
        "-3"
    );
    assert!(extreme.extract_numeric("epoch").unwrap().is_some());
    println!("pg08_07_components: 45 assertions passed");
}

#[test]
fn pg08_08_float_fields() {
    use super::pg_temporal::PgTimestamp;
    // These fields intentionally round after float8 arithmetic, rather than
    // converting the exact decimal EXTRACT result directly to float8.
    let timestamp: PgTimestamp = "2000-01-01 00:00:01.003691".parse().unwrap();
    assert_eq!(
        timestamp
            .extract_float8("second")
            .unwrap()
            .unwrap()
            .to_bits(),
        (1.0f64 + 3691.0 / 1_000_000.0).to_bits()
    );
    assert_ne!(timestamp.extract_float8("second").unwrap(), Some(1.003691));
    assert_eq!(
        timestamp.extract_float8("milliseconds").unwrap(),
        Some(1003.691)
    );
    assert_eq!(
        timestamp.extract_float8("microseconds").unwrap(),
        Some(1003691.0)
    );
    for (source, expected) in [
        ("1969-12-31 23:59:59.999999", -0.000001),
        ("1970-01-01 00:00:00", 0.0),
        ("2000-01-01 00:00:00", 946684800.0),
    ] {
        assert_eq!(
            source
                .parse::<PgTimestamp>()
                .unwrap()
                .extract_float8("epoch")
                .unwrap(),
            Some(expected)
        );
    }
    for source in ["infinity", "-infinity"] {
        let timestamp: PgTimestamp = source.parse().unwrap();
        assert_eq!(timestamp.extract_float8("second").unwrap(), None);
        assert!(
            timestamp
                .extract_float8("year")
                .unwrap()
                .unwrap()
                .is_infinite()
        );
        assert_eq!(
            timestamp
                .extract_float8("aster_bad_unit")
                .unwrap_err()
                .sqlstate(),
            "22023"
        );
    }
    assert_eq!(
        timestamp.extract_float8("timezone").unwrap_err().sqlstate(),
        "0A000"
    );
}

#[test]
fn pg08_09_zoned_float_fields() {
    use super::pg_temporal::PgTimestampTz;
    let instant: PgTimestampTz = "2000-01-01 00:00:01.003691+00".parse().unwrap();
    for (offset, hour, minute) in [(20700, 5.0, 45.0), (-12600, -3.0, -30.0)] {
        let fixed = Some(chrono::FixedOffset::east_opt(offset).unwrap());
        assert_eq!(
            instant
                .extract_float8_in_zone("timezone", chrono_tz::UTC, fixed)
                .unwrap(),
            Some(offset as f64)
        );
        assert_eq!(
            instant
                .extract_float8_in_zone("timezone_hour", chrono_tz::UTC, fixed)
                .unwrap(),
            Some(hour)
        );
        assert_eq!(
            instant
                .extract_float8_in_zone("timezone_minute", chrono_tz::UTC, fixed)
                .unwrap(),
            Some(minute)
        );
        assert_eq!(
            instant
                .extract_float8_in_zone("second", chrono_tz::UTC, fixed)
                .unwrap()
                .unwrap()
                .to_bits(),
            (1.0f64 + 3691.0 / 1_000_000.0).to_bits()
        );
        assert_eq!(
            instant
                .extract_float8_in_zone("epoch", chrono_tz::UTC, fixed)
                .unwrap(),
            instant
                .extract_float8_in_zone("epoch", chrono_tz::UTC, None)
                .unwrap()
        );
    }
    for (source, expected) in [
        ("2024-11-03 05:30:00+00", -14400.0),
        ("2024-11-03 06:30:00+00", -18000.0),
    ] {
        let instant: PgTimestampTz = source.parse().unwrap();
        assert_eq!(
            instant
                .extract_float8_in_zone("timezone", chrono_tz::America::New_York, None)
                .unwrap(),
            Some(expected)
        );
    }
    for source in ["infinity", "-infinity"] {
        let instant: PgTimestampTz = source.parse().unwrap();
        assert_eq!(
            instant
                .extract_float8_in_zone("timezone", chrono_tz::America::New_York, None)
                .unwrap(),
            None
        );
        assert_eq!(
            instant
                .extract_float8_in_zone("aster_bad_unit", chrono_tz::America::New_York, None)
                .unwrap_err()
                .sqlstate(),
            "22023"
        );
    }
}

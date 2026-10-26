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
        let context = PgDateContext { order, today };
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
                    today
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

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

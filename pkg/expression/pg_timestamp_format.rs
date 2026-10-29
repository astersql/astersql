// Copyright 2026 AsterSQL.
//! The selected numeric timestamp templates, independently of MySQL DATE_FORMAT.
use types_dependency::pg_temporal::{PgInterval, PgTemporalError, PgTimestamp, PgTimestampTz};

pub(crate) fn format_timestamp(
    timestamp: PgTimestamp,
    template: &str,
) -> Result<Option<String>, PgTemporalError> {
    format_wall(timestamp, template, 0)
}

/// Resolve the wall clock and its offset at Execute, including DST overlaps.
/// The same template scanner handles timestamp and timestamptz.
pub(crate) fn format_timestamptz(
    timestamp: PgTimestampTz,
    template: &str,
    zone: chrono_tz::Tz,
    fixed: Option<chrono::FixedOffset>,
) -> Result<Option<String>, PgTemporalError> {
    if template.is_empty() || matches!(timestamp.micros(), i64::MIN | i64::MAX) {
        return Ok(None);
    }
    let offset: i32 = timestamp
        .extract_numeric_in_zone("timezone", zone, fixed)?
        .ok_or(PgTemporalError::Range)?
        .to_string()
        .parse()
        .map_err(|_| PgTemporalError::Range)?;
    let wall = PgTimestamp::from_micros(
        timestamp
            .micros()
            .checked_add(i64::from(offset) * 1_000_000)
            .ok_or(PgTemporalError::Range)?,
    )?;
    format_wall(wall, template, offset)
}

fn format_wall(
    timestamp: PgTimestamp,
    template: &str,
    offset: i32,
) -> Result<Option<String>, PgTemporalError> {
    if template.is_empty() || matches!(timestamp.micros(), i64::MIN | i64::MAX) {
        return Ok(None);
    }
    format_fields(Some(timestamp), None, template, offset)
}

/// Keep the stored month/day/time components independent. In particular hours
/// must not wrap at midnight or absorb days (PostgreSQL interval_to_char).
pub(crate) fn format_interval(
    interval: PgInterval,
    template: &str,
) -> Result<Option<String>, PgTemporalError> {
    if template.is_empty()
        || matches!(
            interval.components(),
            (i32::MAX, i32::MAX, i64::MAX) | (i32::MIN, i32::MIN, i64::MIN)
        )
    {
        return Ok(None);
    }
    format_fields(None, Some(interval), template, 0)
}

fn format_fields(
    timestamp: Option<PgTimestamp>,
    interval: Option<PgInterval>,
    template: &str,
    offset: i32,
) -> Result<Option<String>, PgTemporalError> {
    let field = |name: &str| -> Result<i64, PgTemporalError> {
        if let Some(interval) = interval {
            let (months, days, time) = interval.components();
            return match name {
                "year" => Ok(i64::from(months / 12)),
                "month" => Ok(i64::from(months % 12)),
                "day" => Ok(i64::from(days)),
                "hour" => Ok(time / 3_600_000_000),
                "minute" => Ok(time / 60_000_000 % 60),
                "second" => Ok(time / 1_000_000 % 60),
                "microsecond" => Ok(time % 1_000_000),
                _ => Err(PgTemporalError::Unsupported),
            };
        }
        let timestamp = timestamp.expect("timestamp or interval source");
        match name {
            "second" => Ok(timestamp.micros().rem_euclid(60_000_000) / 1_000_000),
            "microsecond" => Ok(timestamp.micros().rem_euclid(1_000_000)),
            _ => timestamp
                .extract_numeric(name)?
                .ok_or(PgTemporalError::Range)?
                .to_string()
                .parse()
                .map_err(|_| PgTemporalError::Range),
        }
    };
    let mut remaining = template;
    let mut result = String::new();
    while !remaining.is_empty() {
        // Quoted text is literal. A backslash escapes the next character,
        // including a quote, without making the literal a template again.
        if let Some(rest) = remaining.strip_prefix('"') {
            remaining = rest;
            while let Some(ch) = remaining.chars().next() {
                remaining = &remaining[ch.len_utf8()..];
                if ch == '"' {
                    break;
                }
                if ch == '\\' {
                    if let Some(escaped) = remaining.chars().next() {
                        result.push(escaped);
                        remaining = &remaining[escaped.len_utf8()..];
                    }
                } else {
                    result.push(ch);
                }
            }
            continue;
        }
        let fill = remaining.starts_with("FM") || remaining.starts_with("fm");
        if fill {
            remaining = &remaining[2..];
        }
        // Longest tokens first; '%' is ordinary literal text in PostgreSQL.
        let token = [
            "HH24", "IYYY", "YYYY", "TZH", "TZM", "OF", "IW", "MM", "DD", "MI", "SS", "US", "Y",
        ]
        .into_iter()
        .find(|token| {
            remaining.starts_with(token) || remaining.starts_with(&token.to_ascii_lowercase())
        });
        let Some(token) = token else {
            if let Some(ch) = remaining.chars().next() {
                result.push(ch);
                remaining = &remaining[ch.len_utf8()..];
            }
            continue;
        };
        remaining = &remaining[token.len()..];
        if matches!(token, "TZH" | "TZM" | "OF") {
            if interval.is_some() {
                return Err(PgTemporalError::IntervalFormat);
            }
            let magnitude = offset.unsigned_abs();
            let hour = magnitude / 3600;
            let minute = magnitude % 3600 / 60;
            let sign = if offset < 0 { '-' } else { '+' };
            let width = if fill { 0 } else { 2 };
            match token {
                "TZH" => result.push_str(&format!("{sign}{hour:02}")),
                "TZM" => result.push_str(&format!("{minute:02}")),
                "OF" => {
                    result.push_str(&format!("{sign}{hour:0width$}"));
                    if magnitude % 3600 != 0 {
                        result.push_str(&format!(":{minute:02}"));
                    }
                }
                _ => unreachable!(),
            }
            continue;
        }
        let (value, width) = match token {
            "YYYY" => (
                if interval.is_some() {
                    field("year")?
                } else {
                    field("year")?.abs()
                },
                4,
            ),
            "IYYY" => (field("isoyear")?.abs(), 4),
            "Y" => (
                if interval.is_some() {
                    field("year")? % 10
                } else {
                    field("year")?.abs() % 10
                },
                1,
            ),
            "IW" => (field("week")?, 2),
            "MM" => (field("month")?, 2),
            "DD" => (field("day")?, 2),
            "HH24" => (field("hour")?, 2),
            "MI" => (field("minute")?, 2),
            "SS" => (field("second")?, 2),
            "US" => (field("microsecond")?, 6),
            _ => unreachable!(),
        };
        // Fractional precision is fixed even when FM suppresses date padding.
        let width = if fill && token != "US" {
            0
        } else {
            width + usize::from(value < 0 && token != "US")
        };
        result.push_str(&format!("{value:0width$}"));
        if remaining.starts_with("TH") || remaining.starts_with("th") {
            let suffix = if (11..=13).contains(&(value % 100)) {
                "th"
            } else {
                match value % 10 {
                    1 => "st",
                    2 => "nd",
                    3 => "rd",
                    _ => "th",
                }
            };
            if remaining.starts_with("TH") {
                result.push_str(&suffix.to_ascii_uppercase());
            } else {
                result.push_str(suffix);
            }
            remaining = &remaining[2..];
        }
    }
    Ok(Some(result))
}

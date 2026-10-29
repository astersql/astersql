// Copyright 2026 AsterSQL.
//! The selected numeric timestamp templates, independently of MySQL DATE_FORMAT.
use types_dependency::pg_temporal::{PgTemporalError, PgTimestamp};

pub(crate) fn format_timestamp(
    timestamp: PgTimestamp,
    template: &str,
) -> Result<Option<String>, PgTemporalError> {
    if template.is_empty() || matches!(timestamp.micros(), i64::MIN | i64::MAX) {
        return Ok(None);
    }
    let field = |name: &str| -> Result<i64, PgTemporalError> {
        timestamp
            .extract_numeric(name)?
            .ok_or(PgTemporalError::Range)?
            .to_string()
            .parse()
            .map_err(|_| PgTemporalError::Range)
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
            "HH24", "IYYY", "YYYY", "IW", "MM", "DD", "MI", "SS", "US", "Y",
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
        let (value, width) = match token {
            "YYYY" => (field("year")?.abs(), 4),
            "IYYY" => (field("isoyear")?.abs(), 4),
            "Y" => (field("year")?.abs() % 10, 1),
            "IW" => (field("week")?, 2),
            "MM" => (field("month")?, 2),
            "DD" => (field("day")?, 2),
            "HH24" => (field("hour")?, 2),
            "MI" => (field("minute")?, 2),
            "SS" => (timestamp.micros().rem_euclid(60_000_000) / 1_000_000, 2),
            "US" => (timestamp.micros().rem_euclid(1_000_000), 6),
            _ => unreachable!(),
        };
        // Fractional precision is fixed even when FM suppresses date padding.
        let width = if fill && token != "US" { 0 } else { width };
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

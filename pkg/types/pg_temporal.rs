// Copyright 2026 AsterSQL.
//! PostgreSQL calendar dates, independent of MySQL's packed temporal values.
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgTemporalError {
    #[error("invalid date input syntax")]
    Syntax,
    #[error("date field out of range")]
    Range,
    #[error("date spelling is not yet supported")]
    Unsupported,
    #[error("insufficient data in date parameter")]
    Truncated,
    #[error("incorrect binary date parameter length")]
    Trailing,
}
impl PgTemporalError {
    pub fn sqlstate(self) -> &'static str {
        match self {
            Self::Syntax => "22007",
            Self::Range => "22008",
            Self::Unsupported => "0A000",
            Self::Truncated => "08P01",
            Self::Trailing => "22P03",
        }
    }
}

/// A validated finite Gregorian day, counted from 2000-01-01. Keeping days
/// directly avoids MySQL bit fields and calendar-library year limitations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PgDate(i32);
impl PgDate {
    /// Years use SQL numbering: 1 is 1 AD, -1 is 1 BC, and zero is invalid.
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Result<Self, PgTemporalError> {
        if year == 0 || month == 0 || month > 12 || day == 0 || day > 31 {
            return Err(PgTemporalError::Range);
        }
        let year = i64::from(year) + i64::from(year < 0);
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days_in_month = match month {
            2 => {
                if leap {
                    29
                } else {
                    28
                }
            }
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if day > days_in_month {
            return Err(PgTemporalError::Range);
        }
        // March-based Gregorian eras have 146097 days per 400 years. Euclidean
        // division keeps the same arithmetic valid before the astronomical year 0.
        let year = year - i64::from(month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year - era * 400;
        let month = i64::from(month) + if month > 2 { -3 } else { 9 };
        let day_of_year = (153 * month + 2) / 5 + i64::from(day) - 1;
        let days = era * 146097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100
            + day_of_year
            - 730425;
        if !(-2451545..=2145031948).contains(&days) {
            return Err(PgTemporalError::Range);
        }
        Ok(Self(days as i32))
    }
    pub fn from_binary(bytes: &[u8]) -> Result<Self, PgTemporalError> {
        if bytes.len() < 4 {
            return Err(PgTemporalError::Truncated);
        }
        if bytes.len() > 4 {
            return Err(PgTemporalError::Trailing);
        }
        let days = i32::from_be_bytes(bytes.try_into().unwrap());
        if days == i32::MIN || days == i32::MAX {
            return Err(PgTemporalError::Unsupported);
        }
        if !(-2451545..=2145031948).contains(&days) {
            return Err(PgTemporalError::Range);
        }
        Ok(Self(days))
    }
    pub fn to_binary(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }
    fn ymd(self) -> (i64, i64, i64) {
        let days = i64::from(self.0) + 730425;
        let era = days.div_euclid(146097);
        let day_of_era = days - era * 146097;
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
        let year = year_of_era + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * month + 2) / 5 + 1;
        let month = month + if month < 10 { 3 } else { -9 };
        (year + i64::from(month <= 2), month, day)
    }
}
impl std::str::FromStr for PgDate {
    type Err = PgTemporalError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let text = text.trim();
        if matches!(
            text.to_ascii_lowercase().as_str(),
            "infinity" | "-infinity" | "epoch" | "now" | "today" | "yesterday" | "tomorrow"
        ) {
            return Err(PgTemporalError::Unsupported);
        }
        let bc = text.to_ascii_uppercase().ends_with(" BC");
        let text = if bc { &text[..text.len() - 3] } else { text };
        let parts = text.split('-').collect::<Vec<_>>();
        if parts.len() != 3
            || parts
                .iter()
                .any(|part| part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err(PgTemporalError::Syntax);
        }
        let year = parts[0]
            .parse::<i32>()
            .map_err(|_| PgTemporalError::Range)?;
        let month = parts[1]
            .parse::<u32>()
            .map_err(|_| PgTemporalError::Range)?;
        let day = parts[2]
            .parse::<u32>()
            .map_err(|_| PgTemporalError::Range)?;
        Self::from_ymd(if bc { -year } else { year }, month, day)
    }
}
impl fmt::Display for PgDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (year, month, day) = self.ymd();
        if year <= 0 {
            write!(f, "{:04}-{month:02}-{day:02} BC", 1 - year)
        } else {
            write!(f, "{year:04}-{month:02}-{day:02}")
        }
    }
}

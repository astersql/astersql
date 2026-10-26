// Copyright 2026 AsterSQL.
//! PostgreSQL calendar dates and day times, independent of MySQL temporal values.
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgTemporalError {
    #[error("invalid date input syntax")]
    Syntax,
    #[error("date field out of range")]
    Range,
    #[error("time zone displacement out of range")]
    ZoneRange,
    #[error("date spelling is not yet supported")]
    Unsupported,
    #[error("time and duration cannot be bridged without loss")]
    DurationBridge,
    #[error("native TIMESTAMP requires storage and session timezone conversion provenance")]
    TimestampBridge,
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
            Self::ZoneRange => "22009",
            Self::Unsupported | Self::DurationBridge | Self::TimestampBridge => "0A000",
            Self::Truncated => "08P01",
            Self::Trailing => "22P03",
        }
    }
}

/// Input field order for ISO DateStyle. Output remains unambiguous ISO.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PgDateOrder {
    #[default]
    Mdy,
    Dmy,
    Ymd,
}
impl PgDateOrder {
    pub fn name(self) -> &'static str {
        match self {
            Self::Mdy => "MDY",
            Self::Dmy => "DMY",
            Self::Ymd => "YMD",
        }
    }
}

/// Immutable context sampled at a real Parse, Bind or SQL statement boundary.
#[derive(Clone, Copy, Debug)]
pub struct PgDateContext {
    pub order: PgDateOrder,
    pub today: PgDate,
    /// Local time within the transaction day, sampled with the same timezone.
    pub time_micros: i64,
}

/// A validated Gregorian day or an infinity sentinel, counted from 2000-01-01. Keeping days
/// directly avoids MySQL bit fields and calendar-library year limitations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PgDate(i32);
impl PgDate {
    /// Years use SQL numbering: 1 is 1 AD, -1 is 1 BC, and zero is invalid.
    pub fn from_ymd(year: i32, month: u32, day: u32) -> Result<Self, PgTemporalError> {
        Self::calendar_day(year, month, day, true)
    }
    // Zone normalization may move the adjacent calendar day back into the
    // instantaneous domain. Public DATE construction still checks its domain.
    fn calendar_day(
        year: i32,
        month: u32,
        day: u32,
        check_domain: bool,
    ) -> Result<Self, PgTemporalError> {
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
        if (check_domain && !(-2451545..=2145031948).contains(&days))
            || i32::try_from(days).is_err()
        {
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
            return Ok(Self(days));
        }
        if !(-2451545..=2145031948).contains(&days) {
            return Err(PgTemporalError::Range);
        }
        Ok(Self(days))
    }
    pub fn to_binary(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }
    /// Interpret relative words only with the caller's transaction/zone context.
    pub fn parse_with_context(text: &str, context: PgDateContext) -> Result<Self, PgTemporalError> {
        let text = text.trim();
        let offset = match text.to_ascii_lowercase().as_str() {
            "today" | "now" => Some(0),
            "yesterday" => Some(-1),
            "tomorrow" => Some(1),
            _ => None,
        };
        if let Some(offset) = offset {
            let days = context
                .today
                .0
                .checked_add(offset)
                .ok_or(PgTemporalError::Range)?;
            if !(-2451545..=2145031948).contains(&days) {
                return Err(PgTemporalError::Range);
            }
            return Ok(Self(days));
        }
        if matches!(
            text.to_ascii_lowercase().as_str(),
            "infinity" | "-infinity" | "epoch"
        ) {
            return text.parse();
        }
        // ISO year-first spellings are independent of field order. Numeric
        // slash-separated dates use DateStyle, never MySQL's loose parser.
        if text.contains('/')
            || (text.contains('-') && text.split('-').next().is_some_and(|p| p.len() < 3))
        {
            let bc = text.to_ascii_uppercase().ends_with(" BC");
            let date = if bc { &text[..text.len() - 3] } else { text };
            let parts = date.split(['/', '-']).collect::<Vec<_>>();
            if parts.len() != 3
                || parts
                    .iter()
                    .any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()))
            {
                return Err(PgTemporalError::Syntax);
            }
            let nums = parts
                .iter()
                .map(|p| p.parse::<i32>().map_err(|_| PgTemporalError::Range))
                .collect::<Result<Vec<_>, _>>()?;
            let year_first = parts[0].len() >= 3 || context.order == PgDateOrder::Ymd;
            let year_digits = if year_first {
                parts[0].len()
            } else {
                parts[2].len()
            };
            let (mut year, month, day) = if year_first {
                (nums[0], nums[1], nums[2])
            } else if context.order == PgDateOrder::Dmy {
                (nums[2], nums[1], nums[0])
            } else {
                (nums[2], nums[0], nums[1])
            };
            if !bc && year_digits <= 2 && year < 100 {
                year += if year < 70 { 2000 } else { 1900 };
            }
            return Self::from_ymd(if bc { -year } else { year }, month as u32, day as u32);
        }
        text.parse()
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
        Self::parse_calendar(text, true)
    }
}
impl PgDate {
    fn parse_calendar(text: &str, check_domain: bool) -> Result<Self, PgTemporalError> {
        let text = text.trim();
        match text.to_ascii_lowercase().as_str() {
            "infinity" => return Ok(Self(i32::MAX)),
            "-infinity" => return Ok(Self(i32::MIN)),
            "epoch" => return Self::from_ymd(1970, 1, 1),
            "now" | "today" | "yesterday" | "tomorrow" => return Err(PgTemporalError::Unsupported),
            _ => {}
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
        Self::calendar_day(if bc { -year } else { year }, month, day, check_domain)
    }
}
impl fmt::Display for PgDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == i32::MIN {
            return f.write_str("-infinity");
        }
        if self.0 == i32::MAX {
            return f.write_str("infinity");
        }
        let (year, month, day) = self.ymd();
        if year <= 0 {
            write!(f, "{:04}-{month:02}-{day:02} BC", 1 - year)
        } else {
            write!(f, "{year:04}-{month:02}-{day:02}")
        }
    }
}

/// PostgreSQL time without time zone: microseconds since midnight, including
/// the distinct end-of-day value 24:00:00. This is not a MySQL duration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PgTime(i64);
impl PgTime {
    pub const END_OF_DAY: i64 = 86_400_000_000;

    pub fn from_micros(micros: i64) -> Result<Self, PgTemporalError> {
        if !(0..=Self::END_OF_DAY).contains(&micros) {
            return Err(PgTemporalError::Range);
        }
        Ok(Self(micros))
    }
    /// Apply a declared precision after input has been rounded to microseconds.
    /// The validated nonnegative domain permits PostgreSQL's integer half-up rule.
    pub fn round_precision(self, precision: isize) -> Self {
        if !(0..=6).contains(&precision) {
            return self;
        }
        let scale = 10i64.pow(6 - precision as u32);
        Self(((self.0 + scale / 2) / scale) * scale)
    }
    pub fn micros(self) -> i64 {
        self.0
    }
    /// Explicit, lossless bridge from a typed MySQL TIME duration. Native
    /// duration legality remains MySQL's responsibility; non-daily values have
    /// no PG time equivalent, even though both types count fractions of seconds.
    pub fn from_mysql_duration(value: crate::time::Duration) -> Result<Self, PgTemporalError> {
        if !(0..=6).contains(&value.Fsp)
            || value.Duration % 1_000 != 0
            || value.Duration < 0
            || value.Duration > Self::END_OF_DAY * 1_000
            || value.Duration % 10i64.pow(9 - value.Fsp as u32) != 0
        {
            return Err(PgTemporalError::DurationBridge);
        }
        Self::from_micros(value.Duration / 1_000)
    }
    /// Explicit reverse bridge. Reject a target FSP that would discard digits;
    /// the closed daily endpoint remains a 24-hour duration, never midnight.
    pub fn to_mysql_duration(self, fsp: i32) -> Result<crate::time::Duration, PgTemporalError> {
        if !(0..=6).contains(&fsp) || self.0 % 10i64.pow(6 - fsp as u32) != 0 {
            return Err(PgTemporalError::DurationBridge);
        }
        Ok(crate::time::Duration {
            Duration: self.0 * 1_000,
            Fsp: fsp,
        })
    }
    pub fn from_binary(bytes: &[u8]) -> Result<Self, PgTemporalError> {
        if bytes.len() < 8 {
            return Err(PgTemporalError::Truncated);
        }
        if bytes.len() > 8 {
            return Err(PgTemporalError::Trailing);
        }
        Self::from_micros(i64::from_be_bytes(bytes.try_into().unwrap()))
    }
    pub fn to_binary(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }
}
// Shared numeric-zone validation; callers discard the displacement only after
// checking its complete spelling and PostgreSQL's +/-15:59:59 limit.
fn validate_numeric_zone(offset: &str) -> Result<(), PgTemporalError> {
    numeric_zone_seconds(offset).map(|_| ())
}
fn numeric_zone_seconds(offset: &str) -> Result<i32, PgTemporalError> {
    let syntax = PgTemporalError::Syntax;
    let number = |field: &str| -> Result<i64, PgTemporalError> {
        if field.is_empty() || !field.bytes().all(|b| b.is_ascii_digit()) {
            return Err(syntax);
        }
        field.parse().map_err(|_| PgTemporalError::Range)
    };
    let (hours, minutes, seconds) = if offset.contains(':') {
        let mut fields = offset.split(':');
        let hours = number(fields.next().ok_or(syntax)?)?;
        let minutes = number(fields.next().ok_or(syntax)?)?;
        let seconds = fields.next().map(number).transpose()?.unwrap_or(0);
        if fields.next().is_some() {
            return Err(syntax);
        }
        (hours, minutes, seconds)
    } else {
        if !offset.bytes().all(|b| b.is_ascii_digit()) {
            return Err(syntax);
        }
        match offset.len() {
            1 | 2 => (number(offset)?, 0, 0),
            4 => (number(&offset[..2])?, number(&offset[2..])?, 0),
            6 => (
                number(&offset[..2])?,
                number(&offset[2..4])?,
                number(&offset[4..])?,
            ),
            _ => return Err(syntax),
        }
    };
    if hours > 15 || minutes > 59 || seconds > 59 {
        return Err(PgTemporalError::ZoneRange);
    }
    Ok((hours * 3600 + minutes * 60 + seconds) as i32)
}

impl std::str::FromStr for PgTime {
    type Err = PgTemporalError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let syntax = PgTemporalError::Syntax;
        let number = |field: &str| -> Result<i64, PgTemporalError> {
            if field.is_empty() || !field.bytes().all(|b| b.is_ascii_digit()) {
                return Err(syntax);
            }
            field.parse::<i64>().map_err(|_| PgTemporalError::Range)
        };
        let mut text = text.trim();
        if text.eq_ignore_ascii_case("allballs") {
            return Self::from_micros(0);
        }
        // A numeric displacement is validated even though time without time
        // zone does not retain it. Never apply it to the wall-clock fields.
        if let Some(at) = text.find(['+', '-']).filter(|at| *at > 0) {
            validate_numeric_zone(text[at + 1..].trim())?;
            text = text[..at].trim_end();
        }
        let meridian = text.get(text.len().saturating_sub(2)..).and_then(|suffix| {
            if suffix.eq_ignore_ascii_case("am") {
                Some(false)
            } else if suffix.eq_ignore_ascii_case("pm") {
                Some(true)
            } else {
                None
            }
        });
        if meridian.is_some() {
            text = text[..text.len() - 2].trim_end();
        }
        let (clock, fraction) = text.split_once('.').unwrap_or((text, ""));
        let (mut hour, minute, second) = if clock.contains(':') {
            let mut fields = clock.split(':');
            let hour = number(fields.next().ok_or(syntax)?)?;
            let minute = number(fields.next().ok_or(syntax)?)?;
            let seconds = fields.next();
            if fields.next().is_some() || (seconds.is_none() && text.contains('.')) {
                return Err(syntax);
            }
            (hour, minute, seconds.map(number).transpose()?.unwrap_or(0))
        } else {
            if !clock.bytes().all(|b| b.is_ascii_digit()) {
                return Err(syntax);
            }
            match clock.len() {
                4 if !text.contains('.') => (number(&clock[..2])?, number(&clock[2..])?, 0),
                6 => (
                    number(&clock[..2])?,
                    number(&clock[2..4])?,
                    number(&clock[4..])?,
                ),
                _ => return Err(syntax),
            }
        };
        if !fraction.bytes().all(|b| b.is_ascii_digit()) {
            return Err(syntax);
        }
        if let Some(pm) = meridian {
            if hour > 12 {
                return Err(PgTemporalError::Range);
            }
            if hour == 12 {
                hour = 0;
            }
            if pm {
                hour += 12;
            }
        }
        if hour > 24 || minute > 59 || second > 59 {
            return Err(PgTemporalError::Range);
        }
        // Match PG ParseFractionalSecond: convert to double, then rint at
        // microsecond resolution (ties to even). Typmod rounding is separate.
        let micros = if fraction.is_empty() {
            0
        } else {
            (format!("0.{fraction}").parse::<f64>().map_err(|_| syntax)? * 1_000_000.0)
                .round_ties_even() as i64
        };
        // Check the input domain after fractional microsecond conversion and
        // before applying a target typmod, as PG DecodeTimeOnly does.
        Self::from_micros(((hour * 60 + minute) * 60 + second) * 1_000_000 + micros)
    }
}
impl fmt::Display for PgTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self.0 / 1_000_000;
        write!(
            f,
            "{:02}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )?;
        let fraction = self.0 % 1_000_000;
        if fraction != 0 {
            write!(f, ".{}", format!("{fraction:06}").trim_end_matches('0'))?;
        }
        Ok(())
    }
}

/// PostgreSQL timestamp without time zone, counted in microseconds from
/// 2000-01-01. Calendar conversion never consults a timezone or a MySQL bit field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PgTimestamp(i64);
impl PgTimestamp {
    /// Resolve relative input once at its Parse/Bind/statement boundary.
    pub fn parse_with_context(text: &str, context: PgDateContext) -> Result<Self, PgTemporalError> {
        let word = text.trim().to_ascii_lowercase();
        if matches!(word.as_str(), "now" | "today" | "yesterday" | "tomorrow") {
            let date = PgDate::parse_with_context(&word, context)?;
            let time = if word == "now" {
                context.time_micros
            } else {
                0
            };
            if !(0..PgTime::END_OF_DAY).contains(&time) {
                return Err(PgTemporalError::Range);
            }
            return Self::from_micros(
                i64::from(date.0)
                    .checked_mul(PgTime::END_OF_DAY)
                    .and_then(|v| v.checked_add(time))
                    .ok_or(PgTemporalError::Range)?,
            );
        }
        text.parse()
    }

    pub const MIN: i64 = -211_813_488_000_000_000;
    pub const END: i64 = 9_223_371_331_200_000_000;

    pub fn from_micros(micros: i64) -> Result<Self, PgTemporalError> {
        if micros != i64::MIN && micros != i64::MAX && !(Self::MIN..Self::END).contains(&micros) {
            return Err(PgTemporalError::Range);
        }
        Ok(Self(micros))
    }
    pub fn micros(self) -> i64 {
        self.0
    }
    pub fn from_binary(bytes: &[u8]) -> Result<Self, PgTemporalError> {
        if bytes.len() < 8 {
            return Err(PgTemporalError::Truncated);
        }
        if bytes.len() > 8 {
            return Err(PgTemporalError::Trailing);
        }
        Self::from_micros(i64::from_be_bytes(bytes.try_into().unwrap()))
    }
    pub fn to_binary(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }
    /// PostgreSQL typmod rounding is relative to the epoch, with ties away
    /// from zero. Widen before adding the half-unit to avoid i64 overflow.
    pub fn round_precision(self, precision: isize) -> Result<Self, PgTemporalError> {
        if !(0..=6).contains(&precision) || matches!(self.0, i64::MIN | i64::MAX) {
            return Ok(self);
        }
        let scale = 10i128.pow(6 - precision as u32);
        let value = i128::from(self.0);
        let rounded = value.signum() * ((value.abs() + scale / 2) / scale) * scale;
        Self::from_micros(i64::try_from(rounded).map_err(|_| PgTemporalError::Range)?)
    }
}
impl std::str::FromStr for PgTimestamp {
    type Err = PgTemporalError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_micros(Self::parse_wall(text)?.micros())
    }
}
impl PgTimestamp {
    // Private intermediate only: validate the instant after applying the zone.
    fn parse_wall(text: &str) -> Result<Self, PgTemporalError> {
        let text = text.trim();
        match text.to_ascii_lowercase().as_str() {
            "infinity" => return Ok(Self(i64::MAX)),
            "-infinity" => return Ok(Self(i64::MIN)),
            "epoch" => return "1970-01-01 00:00:00".parse(),
            "now" | "today" | "yesterday" | "tomorrow" => return Err(PgTemporalError::Unsupported),
            _ => {}
        }
        let bc = text.to_ascii_uppercase().ends_with(" BC");
        let text = if bc { &text[..text.len() - 3] } else { text };
        let (date, clock) = text.split_once([' ', 'T']).ok_or(PgTemporalError::Syntax)?;
        let date: PgDate = if bc {
            PgDate::parse_calendar(&format!("{date} BC"), false)?
        } else {
            PgDate::parse_calendar(date, false)?
        };
        if matches!(date.0, i32::MIN | i32::MAX) {
            return Err(PgTemporalError::Syntax);
        }
        // A timestamp is already typed as a wall clock. Validate its zone
        // without resolving a local instant: DST gaps must remain unchanged.
        let (clock, zone) = if let Some((clock, zone)) = clock.rsplit_once(' ') {
            (clock.trim_end(), Some(zone))
        } else if let Some(at) = clock.find(['+', '-']).filter(|at| *at > 0) {
            (&clock[..at], Some(&clock[at..]))
        } else if let Some(clock) = clock.strip_suffix(['Z', 'z']) {
            (clock, Some("UTC"))
        } else {
            (clock, None)
        };
        if let Some(zone) = zone {
            if zone.starts_with(['+', '-']) {
                validate_numeric_zone(&zone[1..])?;
            } else if !(zone.eq_ignore_ascii_case("UTC")
                || zone.eq_ignore_ascii_case("GMT")
                || zone.parse::<chrono_tz::Tz>().is_ok())
            {
                return Err(PgTemporalError::Syntax);
            }
        }
        // Keep the timestamp input fractional conversion separate from typmod:
        // PG first rounds fractional seconds to microseconds, then adjusts p.
        let (clock, fraction) = clock.split_once('.').unwrap_or((clock, ""));
        let fields = clock.split(':').collect::<Vec<_>>();
        if fields.len() != 3
            || fields
                .iter()
                .any(|v| v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()))
            || !fraction.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(PgTemporalError::Syntax);
        }
        let fields = fields
            .iter()
            .map(|v| v.parse::<i64>().map_err(|_| PgTemporalError::Range))
            .collect::<Result<Vec<_>, _>>()?;
        let (hour, minute, second) = (fields[0], fields[1], fields[2]);
        if hour > 24
            || minute > 59
            || second > 60
            || (hour == 24 && (minute != 0 || second != 0 || fraction.bytes().any(|b| b != b'0')))
        {
            return Err(PgTemporalError::Range);
        }
        let micros = if fraction.is_empty() {
            0
        } else {
            (format!("0.{fraction}")
                .parse::<f64>()
                .map_err(|_| PgTemporalError::Syntax)?
                * 1_000_000.0)
                .round_ties_even() as i64
        };
        let total = i128::from(date.0) * i128::from(PgTime::END_OF_DAY)
            + i128::from((hour * 3600 + minute * 60 + second) * 1_000_000 + micros);
        Ok(Self(
            i64::try_from(total).map_err(|_| PgTemporalError::Range)?,
        ))
    }
}
impl fmt::Display for PgTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0 == i64::MIN {
            return f.write_str("-infinity");
        }
        if self.0 == i64::MAX {
            return f.write_str("infinity");
        }
        let date = PgDate(self.0.div_euclid(PgTime::END_OF_DAY) as i32);
        let clock = PgTime(self.0.rem_euclid(PgTime::END_OF_DAY));
        let (year, month, day) = date.ymd();
        if year <= 0 {
            write!(f, "{:04}-{month:02}-{day:02} {clock} BC", 1 - year)
        } else {
            write!(f, "{year:04}-{month:02}-{day:02} {clock}")
        }
    }
}

/// PostgreSQL timestamptz identity: signed UTC microseconds from 2000-01-01.
/// The input zone is deliberately absent from the stored value.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PgTimestampTz(i64);
impl PgTimestampTz {
    pub fn from_micros(micros: i64) -> Result<Self, PgTemporalError> {
        PgTimestamp::from_micros(micros).map(|v| Self(v.micros()))
    }
    pub fn micros(self) -> i64 {
        self.0
    }
    pub fn from_binary(bytes: &[u8]) -> Result<Self, PgTemporalError> {
        PgTimestamp::from_binary(bytes).map(|v| Self(v.micros()))
    }
    pub fn to_binary(self) -> [u8; 8] {
        self.0.to_be_bytes()
    }
    pub fn round_precision(self, precision: isize) -> Result<Self, PgTemporalError> {
        PgTimestamp(self.0)
            .round_precision(precision)
            .map(|v| Self(v.micros()))
    }
    /// An explicit numeric displacement takes precedence over the caller's
    /// session displacement. With no displacement, the caller must supply one.
    pub fn parse_with_offset(
        text: &str,
        session_offset: Option<i32>,
    ) -> Result<Self, PgTemporalError> {
        let text = text.trim();
        if matches!(
            text.to_ascii_lowercase().as_str(),
            "infinity" | "-infinity" | "epoch"
        ) {
            return text.parse::<PgTimestamp>().map(|v| Self(v.micros()));
        }
        let bc = text.to_ascii_uppercase().ends_with(" BC");
        let body = if bc { &text[..text.len() - 3] } else { text };
        let clock_at = body.find([' ', 'T']).ok_or(PgTemporalError::Syntax)?;
        let zone_at = body[clock_at + 1..]
            .find(['+', '-'])
            .map(|at| at + clock_at + 1);
        let (wall, offset) = if let Some(at) = zone_at {
            let zone = &body[at..];
            let seconds = numeric_zone_seconds(&zone[1..])?;
            (
                &body[..at],
                if zone.starts_with('-') {
                    -seconds
                } else {
                    seconds
                },
            )
        } else if let Some(wall) = body
            .strip_suffix(['Z', 'z'])
            .or_else(|| body.strip_suffix(" UTC"))
        {
            (wall, 0)
        } else {
            (body, session_offset.ok_or(PgTemporalError::Unsupported)?)
        };
        let wall = if bc {
            format!("{wall} BC")
        } else {
            wall.to_owned()
        };
        let wall = PgTimestamp::parse_wall(&wall)?;
        Self::from_micros(
            wall.micros()
                .checked_sub(i64::from(offset) * 1_000_000)
                .ok_or(PgTemporalError::Range)?,
        )
    }
    /// Canonical transport is UTC; the SQL consumer supplies the session offset
    /// for display, independently of the original input spelling.
    pub fn format_with_offset(self, offset: i32) -> Result<String, PgTemporalError> {
        if matches!(self.0, i64::MIN | i64::MAX) {
            return Ok(PgTimestamp(self.0).to_string());
        }
        let local = self
            .0
            .checked_add(i64::from(offset) * 1_000_000)
            .ok_or(PgTemporalError::Range)?;
        let wall = PgTimestamp(local).to_string();
        let (wall, era) = wall
            .strip_suffix(" BC")
            .map_or((wall.as_str(), ""), |s| (s, " BC"));
        let magnitude = offset.unsigned_abs();
        let mut zone = format!(
            "{}{:02}",
            if offset < 0 { '-' } else { '+' },
            magnitude / 3600
        );
        if magnitude % 3600 != 0 {
            zone.push_str(&format!(":{:02}", magnitude % 3600 / 60));
        }
        if magnitude % 60 != 0 {
            zone.push_str(&format!(":{:02}", magnitude % 60));
        }
        Ok(format!("{wall}{zone}{era}"))
    }
}
impl std::str::FromStr for PgTimestampTz {
    type Err = PgTemporalError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse_with_offset(text, None)
    }
}
impl fmt::Display for PgTimestampTz {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format_with_offset(0).map_err(|_| fmt::Error)?)
    }
}

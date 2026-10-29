// Copyright 2026 AsterSQL.
//! PostgreSQL calendar dates and day times, independent of MySQL temporal values.
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgTemporalError {
    #[error("invalid date input syntax")]
    Syntax,
    #[error("timestamp truncation unit not recognized")]
    TruncationUnit,
    #[error("timestamp extraction unit not recognized")]
    ExtractionUnit,
    #[error("unit \"{0}\" not supported for type timestamp without time zone")]
    ExtractionTimezone(&'static str),
    #[error("invalid interval type modifier")]
    IntervalModifier,
    #[error("invalid format specification for an interval value")]
    IntervalFormat,
    #[error("interval field overflow")]
    IntervalOverflow,
    #[error("unit \"week\" not supported for type interval")]
    IntervalWeek,
    #[error("unit \"{0}\" not supported for type interval")]
    IntervalExtraction(&'static str),
    #[error("interval extraction unit not recognized")]
    IntervalExtractionUnit,
    #[error("date field out of range")]
    Range,
    #[error("time zone displacement out of range")]
    ZoneRange,
    #[error("time zone not recognized")]
    UnknownZone,
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
            Self::Syntax | Self::IntervalFormat => "22007",
            Self::IntervalModifier
            | Self::TruncationUnit
            | Self::ExtractionUnit
            | Self::IntervalExtractionUnit => "22023",
            Self::Range => "22008",
            Self::IntervalOverflow => "22015",
            Self::ZoneRange => "22009",
            Self::UnknownZone => "22023",
            Self::Unsupported
            | Self::ExtractionTimezone(_)
            | Self::IntervalExtraction(_)
            | Self::DurationBridge
            | Self::TimestampBridge
            | Self::IntervalWeek => "0A000",
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
        // ParseDateTime classifies an alphabetic token followed by '-', '/'
        // or '.' as a date-shaped named zone, before DecodeTimeOnly validates
        // the clock. Do not send these names through numeric displacement
        // validation: unknown names have 22023, ordinary words have 22007.
        // This correction only classifies nonnumeric names; it does not add
        // named-zone or POSIX-zone evaluation to time without time zone.
        let named_zone_token = |word: &str| {
            let letters = word.bytes().take_while(u8::is_ascii_alphabetic).count();
            letters > 0
                && matches!(word.as_bytes().get(letters), Some(b'-' | b'/' | b'.'))
                && word
                    .bytes()
                    .all(|c| c.is_ascii_alphabetic() || b"-/._:".contains(&c))
        };
        let (clock, zone) = text.rsplit_once(char::is_whitespace).unwrap_or(("", text));
        if named_zone_token(zone)
            // A clock cannot contain another named-zone token. Besides keeping
            // date-like prefixes as syntax errors, this bounds the validation
            // call below even for arbitrarily many invalid input fields.
            && !clock.split_whitespace().any(named_zone_token)
            && !chrono_tz::TZ_VARIANTS
                .iter()
                .any(|tz| tz.name().eq_ignore_ascii_case(zone))
            && (clock.is_empty() || clock.parse::<Self>().is_ok())
        {
            return Err(PgTemporalError::UnknownZone);
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
    /// PostgreSQL timestamp_trunc: calendar truncation uses the existing
    /// proleptic Gregorian date kernel; sub-day units floor before the epoch.
    /// Infinity is preserved only after the unit has been validated.
    pub fn truncate(self, unit: &str) -> Result<Self, PgTemporalError> {
        let unit = unit.to_ascii_lowercase();
        let quantum = match unit.as_str() {
            "microsecond" | "microseconds" | "usec" | "usecs" => Some(1),
            "millisecond" | "milliseconds" | "msec" | "msecs" => Some(1_000),
            "second" | "seconds" | "sec" | "secs" => Some(1_000_000),
            "minute" | "minutes" | "min" | "mins" => Some(60_000_000),
            "hour" | "hours" | "hr" | "hrs" => Some(3_600_000_000),
            "day" | "days" => Some(PgTime::END_OF_DAY),
            "week" | "weeks" | "month" | "months" | "quarter" | "year" | "years" | "decade"
            | "decades" | "century" | "centuries" | "millennium" | "millennia" | "millenniums" => {
                None
            }
            "dow" | "doy" | "isodow" | "isoyear" | "timezone" | "timezone_hour"
            | "timezone_minute" | "julian" => return Err(PgTemporalError::Unsupported),
            "epoch" => return Err(PgTemporalError::TruncationUnit),
            _ => return Err(PgTemporalError::TruncationUnit),
        };
        if self.0 == i64::MIN || self.0 == i64::MAX {
            return Ok(self);
        }
        if let Some(quantum) = quantum {
            return Self::from_micros(self.0.div_euclid(quantum) * quantum);
        }
        let days = self.0.div_euclid(PgTime::END_OF_DAY);
        if matches!(unit.as_str(), "week" | "weeks") {
            return Self::from_micros((days - (days + 5).rem_euclid(7)) * PgTime::END_OF_DAY);
        }
        let (mut year, mut month, _) = PgDate(days as i32).ymd();
        match unit.as_str() {
            "millennium" | "millennia" | "millenniums" => {
                year = (year - 1).div_euclid(1000) * 1000 + 1;
                month = 1;
            }
            "century" | "centuries" => {
                year = (year - 1).div_euclid(100) * 100 + 1;
                month = 1;
            }
            "decade" | "decades" => {
                year = year.div_euclid(10) * 10;
                month = 1;
            }
            "year" | "years" => month = 1,
            "quarter" => month = (month - 1) / 3 * 3 + 1,
            _ => {}
        }
        let date = PgDate::from_ymd(
            if year <= 0 {
                (year - 1) as i32
            } else {
                year as i32
            },
            month as u32,
            1,
        )?;
        Self::from_micros(i64::from(date.0) * PgTime::END_OF_DAY)
    }

    /// Extract calendar fields using integer microseconds, retaining numeric scale.
    /// The field is validated before infinity dispatch; inapplicable fields on
    /// infinity are SQL NULL, whereas monotonic fields preserve its sign.
    pub fn extract_numeric(
        self,
        unit: &str,
    ) -> Result<Option<crate::pg_numeric::PgNumeric>, PgTemporalError> {
        let unit = unit.to_ascii_lowercase();
        let unit = match unit.as_str() {
            "second" | "seconds" => "second",
            "microsecond" | "microseconds" => "microseconds",
            "year" | "years" => "year",
            "month" | "months" => "month",
            "day" | "days" => "day",
            "week" | "weeks" => "week",
            "millisecond" | "milliseconds" => "milliseconds",
            "minute" | "minutes" => "minute",
            "hour" | "hours" => "hour",
            "quarter" => "quarter",
            "decade" | "decades" => "decade",
            "century" | "centuries" => "century",
            "millennium" | "millennia" | "millenniums" => "millennium",
            "isoyear" | "dow" | "isodow" | "doy" | "epoch" | "julian" | "timezone"
            | "timezone_hour" | "timezone_minute" => unit.as_str(),
            _ => return Err(PgTemporalError::ExtractionUnit),
        };
        let text = if matches!(self.0, i64::MIN | i64::MAX) {
            if !matches!(
                unit,
                "year" | "isoyear" | "decade" | "century" | "millennium" | "epoch" | "julian"
            ) {
                return Ok(None);
            }
            if self.0 == i64::MIN {
                "-Infinity".to_owned()
            } else {
                "Infinity".to_owned()
            }
        } else {
            let days = self.0.div_euclid(PgTime::END_OF_DAY);
            let (year, month, day) = PgDate(days as i32).ymd();
            let micros = self.0.rem_euclid(60_000_000);
            match unit {
                "second" => format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000),
                "microseconds" => micros.to_string(),
                "milliseconds" => format!("{}.{:03}", micros / 1_000, micros % 1_000),
                "minute" => (self.0.rem_euclid(3_600_000_000) / 60_000_000).to_string(),
                "hour" => (self.0.rem_euclid(PgTime::END_OF_DAY) / 3_600_000_000).to_string(),
                "quarter" => ((month - 1) / 3 + 1).to_string(),
                "decade" => year.div_euclid(10).to_string(),
                "century" => (if year > 0 {
                    (year + 99) / 100
                } else {
                    -(100 - year) / 100
                })
                .to_string(),
                "millennium" => (if year > 0 {
                    (year + 999) / 1000
                } else {
                    -(1000 - year) / 1000
                })
                .to_string(),
                "dow" => ((days + 6).rem_euclid(7)).to_string(),
                "isodow" => ((days + 5).rem_euclid(7) + 1).to_string(),
                "doy" => (days
                    - i64::from(PgDate::from_ymd((year - i64::from(year <= 0)) as i32, 1, 1)?.0)
                    + 1)
                .to_string(),
                "epoch" => {
                    let micros = i128::from(self.0) + 946_684_800_000_000;
                    format!(
                        "{}{}.{:06}",
                        if micros < 0 { "-" } else { "" },
                        micros.abs() / 1_000_000,
                        micros.abs() % 1_000_000
                    )
                }
                "julian" => {
                    let numerator = i128::from(self.0.rem_euclid(PgTime::END_OF_DAY));
                    // numeric_div selects at least 16 significant decimal digits
                    // from base-10000 weights, then rounds the quotient. The
                    // fixed divisor is 864 * 10000^2; widening avoids overflow.
                    let mut first = numerator;
                    let mut weight = 0i32;
                    while first >= 10_000 {
                        first /= 10_000;
                        weight += 1;
                    }
                    let quotient_weight = weight - 2 - i32::from(first <= 864);
                    let scale = (16 - quotient_weight * 4) as usize;
                    let factor = 10i128.pow(scale as u32);
                    let fraction = (numerator * factor + i128::from(PgTime::END_OF_DAY) / 2)
                        / i128::from(PgTime::END_OF_DAY);
                    format!("{}.{:0scale$}", days + 2_451_545, fraction)
                }
                "timezone" => return Err(PgTemporalError::ExtractionTimezone("timezone")),
                "timezone_hour" => {
                    return Err(PgTemporalError::ExtractionTimezone("timezone_hour"));
                }
                "timezone_minute" => {
                    return Err(PgTemporalError::ExtractionTimezone("timezone_minute"));
                }
                "year" => (year - i64::from(year <= 0)).to_string(),
                "month" => month.to_string(),
                "day" => day.to_string(),
                "week" | "isoyear" => {
                    // ISO year is the calendar year of this week's Thursday.
                    let thursday = days + 3 - (days + 5).rem_euclid(7);
                    let (iso_year, _, _) = PgDate(thursday as i32).ymd();
                    if unit == "isoyear" {
                        (iso_year - i64::from(iso_year <= 0)).to_string()
                    } else {
                        let jan4 =
                            PgDate::from_ymd((iso_year - i64::from(iso_year <= 0)) as i32, 1, 4)?.0
                                as i64;
                        (1 + (days - (jan4 - (jan4 + 5).rem_euclid(7))) / 7).to_string()
                    }
                }
                _ => unreachable!(),
            }
        };
        text.parse().map(Some).map_err(|_| PgTemporalError::Range)
    }

    /// date_part shares EXTRACT's field validation and calendar arithmetic.
    /// Fractional fields retain PostgreSQL's float8 operation order, which can
    /// round differently from parsing the exact numeric EXTRACT result.
    pub fn extract_float8(self, unit: &str) -> Result<Option<f64>, PgTemporalError> {
        let Some(numeric) = self.extract_numeric(unit)? else {
            return Ok(None);
        };
        if !matches!(self.0, i64::MIN | i64::MAX) {
            let micros = self.0.rem_euclid(60_000_000);
            let seconds = (micros / 1_000_000) as f64;
            let fraction = (micros % 1_000_000) as f64;
            let value = match unit.to_ascii_lowercase().as_str() {
                "second" | "seconds" => seconds + fraction / 1_000_000.0,
                "millisecond" | "milliseconds" => seconds * 1000.0 + fraction / 1000.0,
                "epoch" => {
                    self.0.checked_add(946_684_800_000_000).map_or_else(
                        || self.0 as f64 + 946_684_800_000_000.0,
                        |micros| micros as f64,
                    ) / 1_000_000.0
                }
                "julian" => {
                    let day = self.0.div_euclid(PgTime::END_OF_DAY) + 2_451_545;
                    let whole_seconds = self.0.rem_euclid(PgTime::END_OF_DAY) / 1_000_000;
                    day as f64 + (whole_seconds as f64 + fraction / 1_000_000.0) / 86400.0
                }
                _ => {
                    return numeric
                        .to_string()
                        .parse()
                        .map(Some)
                        .map_err(|_| PgTemporalError::Range);
                }
            };
            return Ok(Some(value));
        }
        numeric
            .to_string()
            .parse()
            .map(Some)
            .map_err(|_| PgTemporalError::Range)
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
        // A date without a clock denotes midnight, just as an explicit
        // timestamp cast in PostgreSQL. Keep calendar/range validation below.
        let (date, clock) = text.split_once([' ', 'T']).unwrap_or((text, "00:00:00"));
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
    /// Extract from the execution-zone wall clock; epoch remains an instant.
    pub fn extract_numeric_in_zone(
        self,
        unit: &str,
        zone: chrono_tz::Tz,
        fixed: Option<chrono::FixedOffset>,
    ) -> Result<Option<crate::pg_numeric::PgNumeric>, PgTemporalError> {
        // Infinity and epoch do not need transition data or a local calendar.
        if matches!(self.0, i64::MIN | i64::MAX) || unit.eq_ignore_ascii_case("epoch") {
            return PgTimestamp(self.0).extract_numeric(unit);
        }
        let offset = self.execution_offset(zone, fixed)?;
        let zone_field = match unit.to_ascii_lowercase().as_str() {
            "timezone" => Some(offset),
            "timezone_hour" => Some(offset / 3600),
            "timezone_minute" => Some((offset % 3600) / 60),
            _ => None,
        };
        if let Some(value) = zone_field {
            return value
                .to_string()
                .parse()
                .map(Some)
                .map_err(|_| PgTemporalError::Range);
        }
        PgTimestamp(
            self.0
                .checked_add(i64::from(offset) * 1_000_000)
                .ok_or(PgTemporalError::Range)?,
        )
        .extract_numeric(unit)
    }

    fn execution_offset(
        self,
        zone: chrono_tz::Tz,
        fixed: Option<chrono::FixedOffset>,
    ) -> Result<i32, PgTemporalError> {
        use chrono::Offset;
        Ok(if let Some(fixed) = fixed {
            fixed.local_minus_utc()
        } else if zone == chrono_tz::UTC {
            0
        } else {
            self.0
                .checked_add(946_684_800_000_000)
                .and_then(chrono::DateTime::from_timestamp_micros)
                .ok_or(PgTemporalError::Range)?
                .with_timezone(&zone)
                .offset()
                .fix()
                .local_minus_utc()
        })
    }

    /// date_part uses the execution wall clock, except epoch which is an instant.
    /// Preserve float8 arithmetic for fractional fields rather than casting numeric.
    pub fn extract_float8_in_zone(
        self,
        unit: &str,
        zone: chrono_tz::Tz,
        fixed: Option<chrono::FixedOffset>,
    ) -> Result<Option<f64>, PgTemporalError> {
        if matches!(self.0, i64::MIN | i64::MAX) || unit.eq_ignore_ascii_case("epoch") {
            return PgTimestamp(self.0).extract_float8(unit);
        }
        let offset = self.execution_offset(zone, fixed)?;
        let zone_field = match unit.to_ascii_lowercase().as_str() {
            "timezone" => Some(offset),
            "timezone_hour" => Some(offset / 3600),
            "timezone_minute" => Some((offset % 3600) / 60),
            _ => None,
        };
        if let Some(value) = zone_field {
            return Ok(Some(f64::from(value)));
        }
        PgTimestamp(
            self.0
                .checked_add(i64::from(offset) * 1_000_000)
                .ok_or(PgTemporalError::Range)?,
        )
        .extract_float8(unit)
    }

    /// Truncate the civil time in the execution zone, then restore the instant.
    /// Like PostgreSQL, day and larger buckets resolve their new UTC offset;
    /// sub-day buckets retain the input offset through an autumn overlap.
    pub fn truncate_in_zone(
        self,
        unit: &str,
        zone: chrono_tz::Tz,
        fixed: Option<chrono::FixedOffset>,
    ) -> Result<Self, PgTemporalError> {
        use chrono::{Offset, TimeZone};
        // Validate the unit even for infinity, before consulting transition data.
        PgTimestamp(0).truncate(unit)?;
        if matches!(self.0, i64::MIN | i64::MAX) {
            return Ok(self);
        }
        let datetime = |micros: i64| {
            micros
                .checked_add(946_684_800_000_000)
                .and_then(chrono::DateTime::from_timestamp_micros)
                .ok_or(PgTemporalError::Range)
        };
        let offset = if let Some(fixed) = fixed {
            fixed.local_minus_utc()
        } else if zone == chrono_tz::UTC {
            0
        } else {
            datetime(self.0)?
                .with_timezone(&zone)
                .offset()
                .fix()
                .local_minus_utc()
        };
        let wall = PgTimestamp(
            self.0
                .checked_add(i64::from(offset) * 1_000_000)
                .ok_or(PgTemporalError::Range)?,
        )
        .truncate(unit)?;
        let subday = matches!(
            unit.to_ascii_lowercase().as_str(),
            "microsecond"
                | "microseconds"
                | "usec"
                | "usecs"
                | "millisecond"
                | "milliseconds"
                | "msec"
                | "msecs"
                | "second"
                | "seconds"
                | "sec"
                | "secs"
                | "minute"
                | "minutes"
                | "min"
                | "mins"
                | "hour"
                | "hours"
                | "hr"
                | "hrs"
        );
        let offset = if fixed.is_some() || zone == chrono_tz::UTC || subday {
            offset
        } else {
            let local = datetime(wall.0)?.naive_utc();
            match zone.from_local_datetime(&local) {
                chrono::LocalResult::Single(value) => value.offset().fix().local_minus_utc(),
                chrono::LocalResult::Ambiguous(_, after) => after.offset().fix().local_minus_utc(),
                chrono::LocalResult::None => chrono_tz::GapInfo::new(&local, &zone)
                    .and_then(|gap| gap.begin)
                    .map(|(_, before)| before.fix().local_minus_utc())
                    .ok_or(PgTemporalError::Range)?,
            }
        };
        Self::from_micros(
            wall.0
                .checked_sub(i64::from(offset) * 1_000_000)
                .ok_or(PgTemporalError::Range)?,
        )
    }

    /// Interpret an already typed local timestamp in the execution timezone.
    /// Shared by explicit casts and implicit function argument conversion.
    pub fn from_local_timestamp(
        wall: PgTimestamp,
        zone: chrono_tz::Tz,
        fixed: Option<chrono::FixedOffset>,
    ) -> Result<Self, PgTemporalError> {
        use chrono::{Offset, TimeZone};
        if matches!(wall.micros(), i64::MIN | i64::MAX) {
            return Self::from_micros(wall.micros());
        }
        let offset = if let Some(fixed) = fixed {
            fixed.local_minus_utc()
        } else if zone == chrono_tz::UTC {
            0
        } else {
            let unix = wall
                .micros()
                .checked_add(946_684_800_000_000)
                .ok_or(PgTemporalError::Range)?;
            let local = chrono::DateTime::from_timestamp_micros(unix)
                .ok_or(PgTemporalError::Unsupported)?
                .naive_utc();
            // PostgreSQL chooses the pre-gap or post-overlap offset.
            match zone.from_local_datetime(&local) {
                chrono::LocalResult::Single(value) => value.offset().fix().local_minus_utc(),
                chrono::LocalResult::Ambiguous(_, after) => after.offset().fix().local_minus_utc(),
                chrono::LocalResult::None => chrono_tz::GapInfo::new(&local, &zone)
                    .and_then(|gap| gap.begin)
                    .map(|(_, before)| before.fix().local_minus_utc())
                    .ok_or(PgTemporalError::Unsupported)?,
            }
        };
        Self::from_micros(
            wall.micros()
                .checked_sub(i64::from(offset) * 1_000_000)
                .ok_or(PgTemporalError::Range)?,
        )
    }

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
        // Reject unknown zone names before requesting session context.
        // Otherwise the wall-clock fallback would misclassify them as
        // datetime syntax errors (or accept them in the session zone).
        if let Some((clock, zone)) = body[clock_at + 1..].rsplit_once(' ') {
            if zone.starts_with(|ch: char| ch.is_ascii_alphabetic())
                && !zone.eq_ignore_ascii_case("UTC")
                && !zone.eq_ignore_ascii_case("GMT")
                && zone.parse::<chrono_tz::Tz>().is_err()
            {
                // Unknown abbreviations and trailing words are syntax errors.
                // An unrecognized IANA-style name has the zone diagnostic.
                return Err(if clock.contains(['+', '-']) || !zone.contains('/') {
                    PgTemporalError::Syntax
                } else {
                    PgTemporalError::UnknownZone
                });
            }
        }
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

/// PostgreSQL interval keeps calendar months, calendar days and elapsed microseconds
/// independent. In particular, 24 hours never carries into the day component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PgInterval {
    months: i32,
    days: i32,
    time_microseconds: i64,
}
impl PgInterval {
    /// Explicit lossless bridge from a typed MySQL TIME duration. A caller must
    /// choose this mapping; ordinary TIME results retain their native identity.
    /// Elapsed hours stay in the time component, including negative/long TIME.
    pub fn from_mysql_duration(value: crate::time::Duration) -> Result<Self, PgTemporalError> {
        if !(0..=6).contains(&value.Fsp)
            || !(crate::time::MinTime..=crate::time::MaxTime).contains(&value.Duration)
            || value.Duration % 10i64.pow(9 - value.Fsp as u32) != 0
        {
            return Err(PgTemporalError::DurationBridge);
        }
        Ok(Self::from_components(0, 0, value.Duration / 1_000))
    }
    /// Explicit reverse bridge. Calendar months/days are never folded into
    /// elapsed hours, and neither target range nor FSP may lose information.
    pub fn to_mysql_duration(self, fsp: i32) -> Result<crate::time::Duration, PgTemporalError> {
        if self.months != 0
            || self.days != 0
            || !(0..=6).contains(&fsp)
            || !(crate::time::MinTime / 1_000..=crate::time::MaxTime / 1_000)
                .contains(&self.time_microseconds)
            || self.time_microseconds % 10i64.pow(6 - fsp as u32) != 0
        {
            return Err(PgTemporalError::DurationBridge);
        }
        Ok(crate::time::Duration {
            Duration: self.time_microseconds * 1_000,
            Fsp: fsp,
        })
    }
    pub const fn from_components(months: i32, days: i32, time_microseconds: i64) -> Self {
        Self {
            months,
            days,
            time_microseconds,
        }
    }
    pub const fn components(self) -> (i32, i32, i64) {
        (self.months, self.days, self.time_microseconds)
    }
    /// Extract the signed calendar and elapsed-time components without normalizing
    /// months into days or hours into days. Decimal scaling uses integer arithmetic.
    pub fn extract_numeric(
        self,
        unit: &str,
    ) -> Result<Option<crate::pg_numeric::PgNumeric>, PgTemporalError> {
        let lower = unit.to_ascii_lowercase();
        let unit = match lower.as_str() {
            "microsecond" | "microseconds" | "usec" | "usecs" => "microseconds",
            "millisecond" | "milliseconds" | "msec" | "msecs" => "milliseconds",
            "second" | "seconds" | "sec" | "secs" => "second",
            "minute" | "minutes" | "min" | "mins" => "minute",
            "hour" | "hours" | "hr" | "hrs" => "hour",
            "day" | "days" => "day",
            "week" | "weeks" => "week",
            "month" | "months" | "mon" | "mons" => "month",
            "year" | "years" | "yr" | "yrs" => "year",
            "quarter" => "quarter",
            "decade" | "decades" => "decade",
            "century" | "centuries" => "century",
            "millennium" | "millennia" | "millenniums" => "millennium",
            "epoch" => "epoch",
            "timezone" => return Err(PgTemporalError::IntervalExtraction("timezone")),
            "timezone_hour" => return Err(PgTemporalError::IntervalExtraction("timezone_hour")),
            "timezone_minute" => {
                return Err(PgTemporalError::IntervalExtraction("timezone_minute"));
            }
            "dow" => return Err(PgTemporalError::IntervalExtraction("dow")),
            "doy" => return Err(PgTemporalError::IntervalExtraction("doy")),
            "isodow" => return Err(PgTemporalError::IntervalExtraction("isodow")),
            "isoyear" => return Err(PgTemporalError::IntervalExtraction("isoyear")),
            "julian" => return Err(PgTemporalError::IntervalExtraction("julian")),
            _ => return Err(PgTemporalError::IntervalExtractionUnit),
        };
        let text = if matches!(
            self.components(),
            (i32::MAX, i32::MAX, i64::MAX) | (i32::MIN, i32::MIN, i64::MIN)
        ) {
            if matches!(
                unit,
                "microseconds"
                    | "milliseconds"
                    | "second"
                    | "minute"
                    | "week"
                    | "month"
                    | "quarter"
            ) {
                return Ok(None);
            }
            if self.months < 0 {
                "-Infinity".to_owned()
            } else {
                "Infinity".to_owned()
            }
        } else {
            let months = i128::from(self.months);
            let days = i128::from(self.days);
            let time = i128::from(self.time_microseconds);
            let (value, scale) = match unit {
                "microseconds" => (time % 60_000_000, 0),
                "milliseconds" => (time % 60_000_000, 3),
                "second" => (time % 60_000_000, 6),
                "minute" => (time / 60_000_000 % 60, 0),
                "hour" => (time / 3_600_000_000, 0),
                "day" => (days, 0),
                "week" => (days / 7, 0),
                "month" => (months % 12, 0),
                "year" => (months / 12, 0),
                "quarter" => (
                    (months.abs() % 12 / 3 + 1) * if months < 0 { -1 } else { 1 },
                    0,
                ),
                "decade" => (months / 120, 0),
                "century" => (months / 1200, 0),
                "millennium" => (months / 12000, 0),
                // PG defines epoch using 365.25-day years and 30-day residual
                // months; this does not alter the interval's stored components.
                "epoch" => (
                    ((1461 * (months / 12) + 120 * (months % 12) + 4 * days) * 21600) * 1_000_000
                        + time,
                    6,
                ),
                _ => unreachable!(),
            };
            if scale == 0 {
                value.to_string()
            } else {
                let divisor = 10i128.pow(scale as u32);
                format!(
                    "{}{}.{:0width$}",
                    if value < 0 { "-" } else { "" },
                    value.abs() / divisor,
                    value.abs() % divisor,
                    width = scale
                )
            }
        };
        text.parse().map(Some).map_err(|_| PgTemporalError::Range)
    }

    /// Preserve EXTRACT's signed components and validation while applying the
    /// float8 arithmetic order used by PostgreSQL's interval_part_common.
    pub fn extract_float8(self, unit: &str) -> Result<Option<f64>, PgTemporalError> {
        let Some(numeric) = self.extract_numeric(unit)? else {
            return Ok(None);
        };
        if !matches!(
            self.components(),
            (i32::MAX, i32::MAX, i64::MAX) | (i32::MIN, i32::MIN, i64::MIN)
        ) {
            let micros = self.time_microseconds % 60_000_000;
            let seconds = (micros / 1_000_000) as f64;
            let fraction = (micros % 1_000_000) as f64;
            let value = match unit.to_ascii_lowercase().as_str() {
                "second" | "seconds" | "sec" | "secs" => Some(seconds + fraction / 1_000_000.0),
                "millisecond" | "milliseconds" | "msec" | "msecs" => {
                    Some(seconds * 1000.0 + fraction / 1000.0)
                }
                "epoch" => {
                    // Add each stored group separately; casting the exact sum
                    // would change rounding for large or mixed-sign intervals.
                    let mut result = self.time_microseconds as f64 / 1_000_000.0;
                    result += (365.25 * 86400.0) * f64::from(self.months / 12);
                    result += (30.0 * 86400.0) * f64::from(self.months % 12);
                    result += 86400.0 * f64::from(self.days);
                    Some(result)
                }
                _ => None,
            };
            if let Some(value) = value {
                return Ok(Some(value));
            }
        }
        numeric
            .to_string()
            .parse()
            .map(Some)
            .map_err(|_| PgTemporalError::Range)
    }

    /// Truncate interval fields independently, using signed division toward zero.
    /// Calendar months and days never absorb elapsed hours. Unlike timestamps,
    /// century/millennium buckets start at zero and quarters do not add a month.
    pub fn truncate(self, unit: &str) -> Result<Self, PgTemporalError> {
        let (months, time, clear_days) = match unit.to_ascii_lowercase().as_str() {
            "microsecond" | "microseconds" | "usec" | "usecs" => (0, 1, false),
            "millisecond" | "milliseconds" | "msec" | "msecs" => (0, 1_000, false),
            "second" | "seconds" | "sec" | "secs" => (0, 1_000_000, false),
            "minute" | "minutes" | "min" | "mins" => (0, 60_000_000, false),
            "hour" | "hours" | "hr" | "hrs" => (0, 3_600_000_000, false),
            "day" | "days" => (0, 0, false),
            "month" | "months" | "mon" | "mons" => (1, 0, true),
            "quarter" => (3, 0, true),
            "year" | "years" | "yr" | "yrs" => (12, 0, true),
            "decade" | "decades" => (120, 0, true),
            "century" | "centuries" => (1_200, 0, true),
            "millennium" | "millennia" | "millenniums" => (12_000, 0, true),
            "week" | "weeks" => return Err(PgTemporalError::IntervalWeek),
            "dow" | "doy" | "isodow" | "isoyear" | "timezone" | "timezone_hour"
            | "timezone_minute" | "julian" => {
                return Err(PgTemporalError::Unsupported);
            }
            _ => return Err(PgTemporalError::TruncationUnit),
        };
        if matches!(
            self.components(),
            (i32::MAX, i32::MAX, i64::MAX) | (i32::MIN, i32::MIN, i64::MIN)
        ) {
            return Ok(self);
        }
        Ok(Self::from_components(
            if months == 0 {
                self.months
            } else {
                self.months / months * months
            },
            if clear_days { 0 } else { self.days },
            if time == 0 {
                0
            } else {
                self.time_microseconds / time * time
            },
        ))
    }

    /// Apply PG's trailing-field truncation, then round fractional seconds.
    /// Months and days remain independent even when only time fields are named.
    pub fn apply_typmod(self, fields: isize, precision: isize) -> Result<Self, PgTemporalError> {
        if !(-1..=6).contains(&precision) {
            return Err(PgTemporalError::IntervalModifier);
        }
        let mut value = self;
        if (self.months, self.days, self.time_microseconds) == (i32::MAX, i32::MAX, i64::MAX)
            || (self.months, self.days, self.time_microseconds) == (i32::MIN, i32::MIN, i64::MIN)
        {
            return Ok(self);
        }
        let unit = match fields {
            -1 | 0x7fff | 4096 | 6144 | 7168 | 7176 => 1,
            4 => {
                value.months = value.months / 12 * 12;
                value.days = 0;
                value.time_microseconds = 0;
                1
            }
            2 | 6 => {
                value.days = 0;
                value.time_microseconds = 0;
                1
            }
            8 => {
                value.time_microseconds = 0;
                1
            }
            1024 | 1032 => 3_600_000_000,
            2048 | 3072 | 3080 => 60_000_000,
            _ => return Err(PgTemporalError::IntervalModifier),
        };
        value.time_microseconds = value.time_microseconds / unit * unit;
        if precision >= 0 && precision < 6 {
            let scale = 10i128.pow(6 - precision as u32);
            let micros = i128::from(value.time_microseconds);
            let rounded = (micros.abs() + scale / 2) / scale * scale * micros.signum();
            value.time_microseconds = i64::try_from(rounded).map_err(|_| PgTemporalError::Range)?;
        }
        Ok(value)
    }
    pub fn from_binary(bytes: &[u8]) -> Result<Self, PgTemporalError> {
        if bytes.len() < 16 {
            return Err(PgTemporalError::Truncated);
        }
        if bytes.len() > 16 {
            return Err(PgTemporalError::Trailing);
        }
        Ok(Self::from_components(
            i32::from_be_bytes(bytes[12..16].try_into().unwrap()),
            i32::from_be_bytes(bytes[8..12].try_into().unwrap()),
            i64::from_be_bytes(bytes[..8].try_into().unwrap()),
        ))
    }
    pub fn to_binary(self) -> [u8; 16] {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&self.time_microseconds.to_be_bytes());
        bytes[8..12].copy_from_slice(&self.days.to_be_bytes());
        bytes[12..].copy_from_slice(&self.months.to_be_bytes());
        bytes
    }
}
// Exact decimal input avoids rounding large integer months through floating point.
fn interval_decimal(text: &str) -> Result<(i128, i128), PgTemporalError> {
    let negative = text.starts_with('-');
    let text = text.strip_prefix(['-', '+']).unwrap_or(text);
    let mut parts = text.split('.');
    let integer = parts.next().unwrap_or("");
    let fraction = parts.next().unwrap_or("");
    if parts.next().is_some()
        || (integer.is_empty() && fraction.is_empty())
        || !integer
            .bytes()
            .chain(fraction.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(PgTemporalError::Syntax);
    }
    let scale = 10i128
        .checked_pow(
            fraction
                .len()
                .try_into()
                .map_err(|_| PgTemporalError::IntervalOverflow)?,
        )
        .ok_or(PgTemporalError::IntervalOverflow)?;
    let digits = format!("{integer}{fraction}")
        .parse::<i128>()
        .map_err(|_| PgTemporalError::IntervalOverflow)?;
    Ok((if negative { -digits } else { digits }, scale))
}
fn interval_round(n: i128, d: i128) -> i128 {
    let q = n / d;
    let remainder = n % d;
    q + if remainder.abs() > d / 2 || (d % 2 == 0 && remainder.abs() == d / 2 && q % 2 != 0) {
        n.signum()
    } else {
        0
    }
}
impl PgInterval {
    fn parse_postgres(input: &str) -> Result<Self, PgTemporalError> {
        let input = input.trim();
        if input.eq_ignore_ascii_case("infinity") {
            return Ok(Self::from_components(i32::MAX, i32::MAX, i64::MAX));
        }
        if input.eq_ignore_ascii_case("-infinity") {
            return Ok(Self::from_components(i32::MIN, i32::MIN, i64::MIN));
        }
        if input == "0" {
            return Ok(Self::from_components(0, 0, 0));
        }
        let mut fields = input.split_whitespace().peekable();
        let (mut years, mut months, mut days, mut micros) = (0i128, 0i128, 0i128, 0i128);
        let mut seen = std::collections::HashSet::new();
        while let Some(number) = fields.next() {
            let overflow = || PgTemporalError::IntervalOverflow;
            if number.contains(':') {
                if !seen.insert("time") {
                    return Err(PgTemporalError::Syntax);
                }
                let negative = number.starts_with('-');
                let clock = number.strip_prefix(['-', '+']).unwrap_or(number);
                let parts = clock.split(':').collect::<Vec<_>>();
                if parts.len() != 3 {
                    return Err(PgTemporalError::Syntax);
                }
                let (hours, hs) = interval_decimal(parts[0])?;
                let (minutes, ms) = interval_decimal(parts[1])?;
                let (seconds, ss) = interval_decimal(parts[2])?;
                if hs != 1
                    || ms != 1
                    || hours < 0
                    || !(0..60).contains(&minutes)
                    || seconds < 0
                    || seconds / ss >= 60
                {
                    return Err(PgTemporalError::Syntax);
                }
                let time = hours
                    .checked_mul(3_600_000_000)
                    .and_then(|h| h.checked_add(minutes * 60_000_000))
                    .and_then(|v| {
                        v.checked_add(interval_round(seconds.checked_mul(1_000_000)?, ss))
                    })
                    .ok_or_else(overflow)?;
                let time = if negative { -time } else { time };
                i64::try_from(time).map_err(|_| overflow())?;
                micros = micros.checked_add(time).ok_or_else(overflow)?;
                i64::try_from(micros).map_err(|_| overflow())?;
                continue;
            }
            let unit = fields
                .next()
                .ok_or(PgTemporalError::Syntax)?
                .to_ascii_lowercase();
            let unit = unit.strip_suffix('s').unwrap_or(&unit);
            let (mut n, d) = interval_decimal(number)?;
            let category = match unit {
                "year" | "yr" => "year",
                "month" | "mon" => "month",
                "week" => "week",
                "day" => "day",
                "hour" | "hr" => "hour",
                "minute" | "min" => "minute",
                "second" | "sec" => "second",
                "millisecond" => "millisecond",
                "microsecond" => "microsecond",
                _ => return Err(PgTemporalError::Syntax),
            };
            let duplicate = !seen.insert(category);
            match category {
                "year" => {
                    // PG decodes years separately from months, checking each
                    // field before the final year-to-month conversion. A valid
                    // year field can therefore overflow only at final assembly.
                    years = n / d;
                    i32::try_from(years).map_err(|_| overflow())?;
                    months = months
                        .checked_add((n % d).checked_mul(12).ok_or_else(overflow)? / d)
                        .ok_or_else(overflow)?;
                }
                "month" => {
                    i32::try_from(n / d).map_err(|_| overflow())?;
                    months = months.checked_add(n / d).ok_or_else(overflow)?;
                    n = (n % d).checked_mul(30).ok_or_else(overflow)?;
                    i32::try_from(n / d).map_err(|_| overflow())?;
                    days = days.checked_add(n / d).ok_or_else(overflow)?;
                    micros = micros
                        .checked_add(interval_round(
                            (n % d).checked_mul(86_400_000_000).ok_or_else(overflow)?,
                            d,
                        ))
                        .ok_or_else(overflow)?;
                }
                "week" | "day" => {
                    n = n
                        .checked_mul(if category == "week" { 7 } else { 1 })
                        .ok_or_else(overflow)?;
                    i32::try_from(n / d).map_err(|_| overflow())?;
                    days = days.checked_add(n / d).ok_or_else(overflow)?;
                    micros = micros
                        .checked_add(interval_round(
                            (n % d).checked_mul(86_400_000_000).ok_or_else(overflow)?,
                            d,
                        ))
                        .ok_or_else(overflow)?;
                }
                _ => {
                    let factor = match category {
                        "hour" => 3_600_000_000,
                        "minute" => 60_000_000,
                        "second" => 1_000_000,
                        "millisecond" => 1000,
                        _ => 1,
                    };
                    let time = interval_round(n.checked_mul(factor).ok_or_else(overflow)?, d);
                    i64::try_from(time).map_err(|_| overflow())?;
                    micros = micros.checked_add(time).ok_or_else(overflow)?;
                }
            }
            // Do not allow an out-of-range parsed field to be cancelled by
            // another unit. Parsing/accumulation errors are 22015; only final
            // year/month assembly and typmod rounding use 22008.
            i32::try_from(months).map_err(|_| overflow())?;
            i32::try_from(days).map_err(|_| overflow())?;
            i64::try_from(micros).map_err(|_| overflow())?;
            // PG checks field accumulation before rejecting a repeated unit:
            // an overflowing duplicate is 22015, an in-range duplicate 22007.
            if duplicate {
                return Err(PgTemporalError::Syntax);
            }
        }
        if seen.is_empty() {
            return Err(PgTemporalError::Syntax);
        }
        Ok(Self::from_components(
            (years * 12 + months)
                .try_into()
                .map_err(|_| PgTemporalError::Range)?,
            days.try_into()
                .map_err(|_| PgTemporalError::IntervalOverflow)?,
            micros
                .try_into()
                .map_err(|_| PgTemporalError::IntervalOverflow)?,
        ))
    }
}
impl fmt::Display for PgInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.components() == (i32::MAX, i32::MAX, i64::MAX) {
            return f.write_str("infinity");
        }
        if self.components() == (i32::MIN, i32::MIN, i64::MIN) {
            return f.write_str("-infinity");
        }
        let mut parts = Vec::new();
        let mut negative = false;
        for (n, unit) in [
            (self.months / 12, "year"),
            (self.months % 12, "mon"),
            (self.days, "day"),
        ] {
            if n != 0 {
                parts.push(format!(
                    "{}{n} {unit}{}",
                    if negative && n > 0 { "+" } else { "" },
                    if n == 1 { "" } else { "s" }
                ));
                // PostgreSQL signs the next field from the preceding emitted field,
                // rather than retaining every earlier negative component.
                negative = n < 0;
            }
        }
        if self.time_microseconds != 0 || parts.is_empty() {
            let n = self.time_microseconds.unsigned_abs();
            let fraction = n % 1_000_000;
            let suffix = if fraction == 0 {
                String::new()
            } else {
                format!(".{fraction:06}").trim_end_matches('0').to_owned()
            };
            parts.push(format!(
                "{}{:02}:{:02}:{:02}{suffix}",
                if self.time_microseconds < 0 {
                    "-"
                } else if negative {
                    "+"
                } else {
                    ""
                },
                n / 3_600_000_000,
                n / 60_000_000 % 60,
                n / 1_000_000 % 60
            ));
        }
        f.write_str(&parts.join(" "))
    }
}

/// PostgreSQL's four interval input/output conventions; independent of DateStyle.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PgIntervalStyle {
    #[default]
    Postgres,
    PostgresVerbose,
    SqlStandard,
    Iso8601,
}
impl PgIntervalStyle {
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "postgres" => Some(Self::Postgres),
            "postgres_verbose" => Some(Self::PostgresVerbose),
            "sql_standard" => Some(Self::SqlStandard),
            "iso_8601" => Some(Self::Iso8601),
            _ => None,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::PostgresVerbose => "postgres_verbose",
            Self::SqlStandard => "sql_standard",
            Self::Iso8601 => "iso_8601",
        }
    }
}
/// Changes applied by the owning session, including its startup reset value.
#[derive(Clone, Copy, Debug)]
pub enum PgIntervalSetting {
    Set { style: PgIntervalStyle, local: bool },
    Reset,
    ResetLocal,
    Startup(PgIntervalStyle),
}
impl PgInterval {
    pub fn parse_with_style(input: &str, style: PgIntervalStyle) -> Result<Self, PgTemporalError> {
        let mut input = input.trim();
        let ago = input.ends_with(" ago");
        if ago {
            input = input[..input.len() - 4].trim_end();
        }
        input = input.strip_prefix('@').unwrap_or(input).trim();
        let mut normalized = String::new();
        if let Some(iso) = input.strip_prefix('P') {
            let mut time = false;
            let mut start = 0;
            for (i, c) in iso.char_indices() {
                if c == 'T' {
                    if i != start || time {
                        return Err(PgTemporalError::Syntax);
                    }
                    time = true;
                    start = i + 1;
                    continue;
                }
                if c.is_ascii_alphabetic() {
                    let unit = match (c, time) {
                        ('Y', false) => "years",
                        ('M', false) => "months",
                        ('W', false) => "weeks",
                        ('D', false) => "days",
                        ('H', true) => "hours",
                        ('M', true) => "minutes",
                        ('S', true) => "seconds",
                        _ => return Err(PgTemporalError::Syntax),
                    };
                    if start == i {
                        return Err(PgTemporalError::Syntax);
                    }
                    normalized.push_str(&format!("{} {unit} ", &iso[start..i]));
                    start = i + 1;
                }
            }
            if start != iso.len() || normalized.is_empty() || iso.ends_with('T') {
                return Err(PgTemporalError::Syntax);
            }
        } else {
            let fields = input.split_whitespace().collect::<Vec<_>>();
            // In SQL standard input a single leading minus covers unsigned
            // fields. Any subsequent explicit sign restores independent signs.
            let negate = style == PgIntervalStyle::SqlStandard
                && input.starts_with('-')
                && !fields
                    .iter()
                    .skip(1)
                    .any(|field| field.starts_with(['+', '-']));
            for (i, field) in fields.iter().enumerate() {
                let number = field.strip_prefix(['+', '-']).unwrap_or(field);
                if let Some((years, months)) = number.split_once('-') {
                    if years.is_empty() || months.is_empty() || months.contains('-') {
                        return Err(PgTemporalError::Syntax);
                    }
                    let sign = if field.starts_with('-') { "-" } else { "+" };
                    normalized.push_str(&format!("{sign}{years} years {sign}{months} months "));
                } else {
                    let unsigned = !field.starts_with(['+', '-']);
                    if negate
                        && i > 0
                        && unsigned
                        && field.as_bytes().first().is_some_and(u8::is_ascii_digit)
                    {
                        normalized.push('-');
                    }
                    normalized.push_str(field);
                    // A bare number before a clock (or the final SQL-standard
                    // day field) is a calendar day, never elapsed hours.
                    if !field.contains(':')
                        && number.parse::<i128>().is_ok()
                        && (fields.get(i + 1).is_some_and(|s| s.contains(':'))
                            || (style == PgIntervalStyle::SqlStandard
                                && fields.len() > 1
                                && i + 1 == fields.len()))
                    {
                        normalized.push_str(" days");
                    }
                    normalized.push(' ');
                }
            }
        }
        if ago {
            normalized = normalized
                .split_whitespace()
                .map(|field| {
                    if let Some(n) = field.strip_prefix('-') {
                        n.to_owned()
                    } else if field
                        .as_bytes()
                        .first()
                        .is_some_and(|b| b.is_ascii_digit() || *b == b'+')
                    {
                        format!("-{}", field.strip_prefix('+').unwrap_or(field))
                    } else {
                        field.to_owned()
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
        }
        Self::parse_postgres(normalized.trim())
    }

    pub fn format_with_style(self, style: PgIntervalStyle) -> String {
        if style == PgIntervalStyle::Postgres
            || self.components() == (i32::MAX, i32::MAX, i64::MAX)
            || self.components() == (i32::MIN, i32::MIN, i64::MIN)
        {
            return self.to_string();
        }
        let (m, d, t) = (
            i64::from(self.months),
            i64::from(self.days),
            i128::from(self.time_microseconds),
        );
        let seconds = |v: i128| {
            let n = v.abs();
            let frac = n % 1_000_000;
            format!(
                "{}{}{}",
                if v < 0 { "-" } else { "" },
                n / 1_000_000,
                if frac == 0 {
                    String::new()
                } else {
                    format!(".{frac:06}").trim_end_matches('0').to_owned()
                }
            )
        };
        match style {
            PgIntervalStyle::Iso8601 => {
                let mut s = String::from("P");
                for (n, u) in [(m / 12, "Y"), (m % 12, "M"), (d, "D")] {
                    if n != 0 {
                        s.push_str(&format!("{n}{u}"));
                    }
                }
                if t != 0 {
                    s.push('T');
                    for (n, u) in [(t / 3_600_000_000, "H"), (t / 60_000_000 % 60, "M")] {
                        if n != 0 {
                            s.push_str(&format!("{n}{u}"));
                        }
                    }
                    if t % 60_000_000 != 0 {
                        s.push_str(&format!("{}S", seconds(t % 60_000_000)));
                    }
                }
                if s == "P" {
                    s.push_str("T0S");
                }
                s
            }
            PgIntervalStyle::PostgresVerbose => {
                let negative = if m != 0 {
                    m < 0
                } else if d != 0 {
                    d < 0
                } else {
                    t < 0
                };
                let sign = if negative { -1 } else { 1 };
                let (m, d, t) = (m * sign, d * sign, t * i128::from(sign));
                let mut parts = Vec::new();
                for (n, u) in [
                    (i128::from(m / 12), "year"),
                    (i128::from(m % 12), "mon"),
                    (i128::from(d), "day"),
                    (t / 3_600_000_000, "hour"),
                    (t / 60_000_000 % 60, "min"),
                ] {
                    if n != 0 {
                        parts.push(format!("{n} {u}{}", if n.abs() == 1 { "" } else { "s" }));
                    }
                }
                if t % 60_000_000 != 0 {
                    let n = t % 60_000_000;
                    parts.push(format!(
                        "{} sec{}",
                        seconds(n),
                        if n.abs() == 1_000_000 { "" } else { "s" }
                    ));
                }
                if parts.is_empty() {
                    parts.push("0".into());
                }
                format!(
                    "@ {}{}",
                    parts.join(" "),
                    if negative { " ago" } else { "" }
                )
            }
            PgIntervalStyle::SqlStandard => {
                let mixed = (m < 0 || d < 0 || t < 0) && (m > 0 || d > 0 || t > 0);

                let signed = mixed || (m != 0 && (d != 0 || t != 0));
                let mut parts = Vec::new();
                // Mixed signs retain all SQL-standard groups, including zero years/months.
                if m != 0 || mixed {
                    parts.push(format!(
                        "{}{}-{}",
                        if m < 0 {
                            "-"
                        } else if signed {
                            "+"
                        } else {
                            ""
                        },
                        m.abs() / 12,
                        m.abs() % 12
                    ));
                }
                if d != 0 || (m != 0 && t != 0) {
                    parts.push(format!("{}{d}", if signed && d >= 0 { "+" } else { "" }));
                }
                if t != 0 || d != 0 {
                    let n = t.abs();
                    parts.push(format!(
                        "{}{}:{:02}:{}{}",
                        if t < 0 && (signed || parts.is_empty()) {
                            "-"
                        } else if signed {
                            "+"
                        } else {
                            ""
                        },
                        n / 3_600_000_000,
                        n / 60_000_000 % 60,
                        if n / 1_000_000 % 60 < 10 { "0" } else { "" },
                        seconds(n % 60_000_000)
                    ));
                }
                if parts.is_empty() {
                    return "0".into();
                }
                // SQL standard uses one leading minus for a wholly negative
                // day/time interval, and signed fields for mixed categories.
                parts.join(" ")
            }
            PgIntervalStyle::Postgres => unreachable!(),
        }
    }
}
impl std::str::FromStr for PgInterval {
    type Err = PgTemporalError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse_with_style(input, PgIntervalStyle::Postgres)
    }
}

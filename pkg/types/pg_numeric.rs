// Copyright 2026 AsterSQL.

//! Finite PostgreSQL numeric values, independent of MySQL's decimal capacity.
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgNumericError {
    #[error("invalid input syntax for type numeric")]
    InvalidSyntax,
    #[error("value overflows numeric format")]
    OutOfRange,
    #[error("numeric special values are not supported yet")]
    Unsupported,
}
impl PgNumericError {
    pub fn sqlstate(self) -> &'static str {
        match self {
            Self::InvalidSyntax => "22P02",
            Self::OutOfRange => "22003",
            Self::Unsupported => "0A000",
        }
    }
}

/// Normalized decimal coefficient and display scale. No floating point or
/// MySQL decimal conversion participates in construction or formatting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PgNumeric {
    negative: bool,
    coefficient: String,
    scale: usize,
}
impl FromStr for PgNumeric {
    type Err = PgNumericError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        use PgNumericError::*;
        let text = input.trim_matches(|c: char| c.is_ascii_whitespace());
        if [
            "nan",
            "infinity",
            "+infinity",
            "-infinity",
            "inf",
            "+inf",
            "-inf",
        ]
        .iter()
        .any(|v| text.eq_ignore_ascii_case(v))
        {
            return Err(Unsupported);
        }
        let negative = text.starts_with('-');
        let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
        let (mantissa, exponent) = match unsigned.find(['e', 'E']) {
            Some(i) => (&unsigned[..i], Some(&unsigned[i + 1..])),
            None => (unsigned, None),
        };
        let mut coefficient = String::with_capacity(mantissa.len());
        let mut fractional = None;
        for byte in mantissa.bytes() {
            match byte {
                b'0'..=b'9' => {
                    coefficient.push(byte as char);
                    if let Some(n) = &mut fractional {
                        *n += 1;
                    }
                }
                b'.' if fractional.is_none() => fractional = Some(0i64),
                _ => return Err(InvalidSyntax),
            }
        }
        if coefficient.is_empty() {
            return Err(InvalidSyntax);
        }
        let exponent = if let Some(e) = exponent {
            let digits = e.strip_prefix(['+', '-']).unwrap_or(e);
            if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
                return Err(InvalidSyntax);
            }
            e.parse::<i64>().map_err(|_| OutOfRange)?
        } else {
            0
        };
        let scale = fractional
            .unwrap_or(0)
            .checked_sub(exponent)
            .ok_or(OutOfRange)?;
        // Bound expansion before allocation; trim leading zeros without losing scale.
        let significant = coefficient.trim_start_matches('0');
        let zero = significant.is_empty();
        let integer_digits = (significant.len() as i64)
            .checked_sub(scale)
            .ok_or(OutOfRange)?;
        if (!zero && integer_digits > 131072) || scale > 16383 {
            return Err(OutOfRange);
        }
        coefficient = if zero { "0".into() } else { significant.into() };
        if scale < 0 && !zero {
            coefficient.extend(std::iter::repeat_n(
                '0',
                usize::try_from(-scale).map_err(|_| OutOfRange)?,
            ));
        }
        Ok(Self {
            negative: negative && !zero,
            coefficient,
            scale: scale.max(0) as usize,
        })
    }
}
impl fmt::Display for PgNumeric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.negative {
            f.write_str("-")?;
        }
        if self.scale == 0 {
            return f.write_str(&self.coefficient);
        }
        if self.coefficient.len() <= self.scale {
            f.write_str("0.")?;
            for _ in self.coefficient.len()..self.scale {
                f.write_str("0")?;
            }
            f.write_str(&self.coefficient)
        } else {
            let split = self.coefficient.len() - self.scale;
            write!(
                f,
                "{}.{}",
                &self.coefficient[..split],
                &self.coefficient[split..]
            )
        }
    }
}

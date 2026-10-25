// Copyright 2026 AsterSQL.

//! PostgreSQL numeric values, independent of MySQL's decimal capacity.
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgNumericError {
    #[error("invalid input syntax for type numeric")]
    InvalidSyntax,
    #[error("invalid numeric precision or scale")]
    InvalidModifier,
    #[error("value overflows numeric format")]
    OutOfRange,
    #[error("numeric special values are not supported yet")]
    Unsupported,
}
impl PgNumericError {
    pub fn sqlstate(self) -> &'static str {
        match self {
            Self::InvalidSyntax => "22P02",
            Self::InvalidModifier => "22023",
            Self::OutOfRange => "22003",
            Self::Unsupported => "0A000",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NumericKind {
    Finite,
    NaN,
    Infinity,
    NegativeInfinity,
}

/// Tagged numeric identity with normalized finite coefficient and display scale.
/// No floating point or MySQL decimal conversion participates in construction or formatting.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PgNumeric {
    kind: NumericKind,
    negative: bool,
    coefficient: String,
    scale: usize,
}
impl PgNumeric {
    /// Round ties away from zero before checking the signed SQL integer domain.
    pub fn to_signed(self, bits: u32) -> Result<i64, PgNumericError> {
        if self.kind != NumericKind::Finite || !matches!(bits, 16 | 32 | 64) {
            return Err(PgNumericError::OutOfRange);
        }
        // Anything exceeding this precision also exceeds every signed target.
        let rounded = self.apply_modifier(1000, 0)?;
        let value = rounded
            .to_string()
            .parse::<i64>()
            .map_err(|_| PgNumericError::OutOfRange)?;
        let limit = 1i128 << (bits - 1);
        if (value as i128) < -limit || (value as i128) >= limit {
            return Err(PgNumericError::OutOfRange);
        }
        Ok(value)
    }

    /// Validate declaration independently of values, including NULL and empty scans.
    pub fn validate_modifier(precision: isize, scale: isize) -> Result<(), PgNumericError> {
        if (precision == -1 && scale == -1)
            || ((1..=1000).contains(&precision) && (-1000..=1000).contains(&scale))
        {
            Ok(())
        } else {
            Err(PgNumericError::InvalidModifier)
        }
    }

    /// Apply the declared scale with ties away from zero, then check precision.
    pub fn apply_modifier(
        mut self,
        precision: isize,
        scale: isize,
    ) -> Result<Self, PgNumericError> {
        Self::validate_modifier(precision, scale)?;
        if precision == -1 {
            return Ok(self);
        }
        match self.kind {
            NumericKind::NaN => return Ok(self),
            NumericKind::Infinity | NumericKind::NegativeInfinity => {
                return Err(PgNumericError::OutOfRange);
            }
            NumericKind::Finite => {}
        }
        let cut = self.scale as isize - scale;
        if cut > 0 {
            let keep = self.coefficient.len() as isize - cut;
            let round_up = keep >= 0 && self.coefficient.as_bytes()[keep as usize] >= b'5';
            let mut digits = if keep > 0 {
                self.coefficient.as_bytes()[..keep as usize].to_vec()
            } else {
                vec![b'0']
            };
            if round_up {
                let mut carry = true;
                for digit in digits.iter_mut().rev() {
                    if *digit < b'9' {
                        *digit += 1;
                        carry = false;
                        break;
                    }
                    *digit = b'0';
                }
                if carry {
                    digits.insert(0, b'1');
                }
            }
            self.coefficient = String::from_utf8(digits).expect("decimal digits");
        } else {
            self.coefficient
                .extend(std::iter::repeat_n('0', (-cut) as usize));
        }
        if scale < 0 {
            self.coefficient
                .extend(std::iter::repeat_n('0', (-scale) as usize));
        }
        self.scale = scale.max(0) as usize;
        self.coefficient = match self.coefficient.trim_start_matches('0') {
            "" => "0".into(),
            digits => digits.into(),
        };
        if self.coefficient == "0" {
            self.negative = false;
        } else if self.coefficient.len() as isize - self.scale as isize > precision - scale {
            return Err(PgNumericError::OutOfRange);
        }
        Ok(self)
    }
}
impl FromStr for PgNumeric {
    type Err = PgNumericError;
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        use PgNumericError::*;
        let text = input.trim_matches(|c: char| c.is_ascii_whitespace());
        let unsigned_special = text.strip_prefix(['+', '-']).unwrap_or(text);
        let kind = if text.eq_ignore_ascii_case("nan") {
            NumericKind::NaN
        } else if unsigned_special.eq_ignore_ascii_case("inf")
            || unsigned_special.eq_ignore_ascii_case("infinity")
        {
            if text.starts_with('-') {
                NumericKind::NegativeInfinity
            } else {
                NumericKind::Infinity
            }
        } else {
            NumericKind::Finite
        };
        if kind != NumericKind::Finite {
            return Ok(Self {
                kind,
                negative: false,
                coefficient: "0".into(),
                scale: 0,
            });
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
            kind: NumericKind::Finite,
            negative: negative && !zero,
            coefficient,
            scale: scale.max(0) as usize,
        })
    }
}
impl fmt::Display for PgNumeric {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind {
            NumericKind::NaN => return f.write_str("NaN"),
            NumericKind::Infinity => return f.write_str("Infinity"),
            NumericKind::NegativeInfinity => return f.write_str("-Infinity"),
            NumericKind::Finite => {}
        }
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

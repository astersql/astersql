// Copyright 2026 AsterSQL.

//! PostgreSQL numeric values, independent of MySQL's decimal capacity.
use std::{fmt, str::FromStr};

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum PgNumericError {
    #[error("cannot use \"V\" and decimal point together")]
    FormatShiftPoint,
    #[error("multiple decimal points")]
    FormatMultiplePoints,
    #[error("invalid input syntax for type numeric")]
    InvalidSyntax,
    #[error("invalid numeric precision or scale")]
    InvalidModifier,
    #[error("value overflows numeric format")]
    OutOfRange,
    #[error("numeric special values are not supported yet")]
    Unsupported,
    #[error("insufficient data left in numeric parameter")]
    Truncated,
    #[error("invalid binary numeric value")]
    InvalidBinary,
}
impl PgNumericError {
    pub fn sqlstate(self) -> &'static str {
        match self {
            Self::InvalidSyntax => "22P02",
            Self::FormatShiftPoint | Self::FormatMultiplePoints => "42601",
            Self::InvalidModifier => "22023",
            Self::OutOfRange => "22003",
            Self::Unsupported => "0A000",
            Self::Truncated => "08P01",
            Self::InvalidBinary => "22P03",
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
    /// PostgreSQL numeric_recv: base-10000 digits, with truncation to wire dscale.
    /// Validate every digit even for special values before constructing the value.
    pub fn from_binary(bytes: &[u8]) -> Result<Self, PgNumericError> {
        use PgNumericError::{InvalidBinary, Truncated};
        let mut at = 0;
        let mut word = || {
            let pair = bytes.get(at..at + 2).ok_or(Truncated)?;
            at += 2;
            Ok::<_, PgNumericError>(u16::from_be_bytes(pair.try_into().unwrap()))
        };
        let count = word()? as usize;
        let weight = word()? as i16 as i32;
        let sign = word()?;
        if !matches!(sign, 0 | 0x4000 | 0xc000 | 0xd000 | 0xf000) {
            return Err(InvalidBinary);
        }
        let scale = word()? as usize;
        if scale > 0x3fff {
            return Err(InvalidBinary);
        }
        let mut digits = Vec::new();
        for _ in 0..count {
            let digit = word()?;
            if digit >= 10000 {
                return Err(InvalidBinary);
            }
            digits.push(digit);
        }
        if at != bytes.len() {
            return Err(InvalidBinary);
        }
        if sign >= 0xc000 {
            return match sign {
                0xc000 => "NaN",
                0xd000 => "Infinity",
                _ => "-Infinity",
            }
            .parse();
        }
        // Allocation is bounded by int16 weight and the validated 14-bit scale.
        // Hidden fractional digits are truncated, never rounded by numeric_recv.
        let group = |power: i32| -> u16 {
            usize::try_from(weight - power)
                .ok()
                .and_then(|i| digits.get(i))
                .copied()
                .unwrap_or(0)
        };
        let mut text = if sign == 0x4000 {
            String::from("-")
        } else {
            String::new()
        };
        if weight < 0 {
            text.push('0');
        } else {
            for power in (0..=weight).rev() {
                use std::fmt::Write;
                write!(text, "{:04}", group(power)).unwrap();
            }
        }
        if scale > 0 {
            text.push('.');
            for i in 0..scale.div_ceil(4) {
                use std::fmt::Write;
                write!(text, "{:04}", group(-1 - i as i32)).unwrap();
            }
            text.truncate(text.len() - (4 - scale % 4) % 4);
        }
        text.parse()
    }

    /// Canonical numeric_send representation, retaining display scale and specials.
    pub fn to_binary(&self) -> Vec<u8> {
        let sign = match self.kind {
            NumericKind::NaN => 0xc000u16,
            NumericKind::Infinity => 0xd000,
            NumericKind::NegativeInfinity => 0xf000,
            NumericKind::Finite if self.negative => 0x4000,
            NumericKind::Finite => 0,
        };
        if self.kind != NumericKind::Finite {
            // PG18's short special header exposes dscale 32 for +/-Infinity.
            let scale = if self.kind == NumericKind::NaN {
                0u16
            } else {
                32
            };
            return [
                0u16.to_be_bytes(),
                0u16.to_be_bytes(),
                sign.to_be_bytes(),
                scale.to_be_bytes(),
            ]
            .concat();
        }
        let text = self.to_string();
        let value = text.strip_prefix('-').unwrap_or(&text);
        let (integer, fraction) = value.split_once('.').unwrap_or((value, ""));
        let integer_groups = integer.len().div_ceil(4);
        let mut padded = String::with_capacity(integer_groups * 4 + fraction.len().div_ceil(4) * 4);
        padded.extend(std::iter::repeat_n('0', (4 - integer.len() % 4) % 4));
        padded.push_str(integer);
        padded.push_str(fraction);
        padded.extend(std::iter::repeat_n('0', (4 - fraction.len() % 4) % 4));
        let groups: Vec<u16> = padded
            .as_bytes()
            .chunks(4)
            .map(|b| std::str::from_utf8(b).unwrap().parse().unwrap())
            .collect();
        let first = groups.iter().position(|&v| v != 0).unwrap_or(groups.len());
        let end = groups
            .iter()
            .rposition(|&v| v != 0)
            .map_or(first, |i| i + 1);
        let digits = &groups[first..end];
        let weight = if digits.is_empty() {
            0
        } else {
            (integer_groups as i32 - first as i32 - 1) as i16
        };
        let mut out = Vec::with_capacity(8 + digits.len() * 2);
        out.extend_from_slice(&(digits.len() as u16).to_be_bytes());
        out.extend_from_slice(&weight.to_be_bytes());
        out.extend_from_slice(&sign.to_be_bytes());
        out.extend_from_slice(&(self.scale as u16).to_be_bytes());
        for digit in digits {
            out.extend_from_slice(&digit.to_be_bytes());
        }
        out
    }

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
        self = self.round_scale(scale as i32);
        if self.coefficient != "0"
            && self.coefficient.len() as isize - self.scale as isize > precision - scale
        {
            return Err(PgNumericError::OutOfRange);
        }
        Ok(self)
    }

    /// numeric ROUND has no declaration precision check. Clamp before arithmetic
    /// just as PostgreSQL does, retaining the full numeric display-scale domain.
    pub fn round_scale(mut self, scale: i32) -> Self {
        if self.kind != NumericKind::Finite {
            return self;
        }
        let scale = scale.clamp(-131073, 16383) as isize;
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
        }
        self
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

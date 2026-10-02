// Copyright 2026 AsterSQL.

//! Numeric and size grammar shared by Go-compatible DDL resource inputs.
//! Hexadecimal floats are rounded from integer bits, including subnormals;
//! accumulating their mantissa in a float would introduce double rounding.

fn number_error(text: &str, range: bool) -> String {
    format!(
        "strconv.ParseFloat: parsing {text:?}: {}",
        if range {
            "value out of range"
        } else {
            "invalid syntax"
        }
    )
}

fn digits(
    bytes: &[u8],
    cursor: &mut usize,
    hex: bool,
    prefix_underscore: bool,
) -> Result<usize, ()> {
    let start = *cursor;
    let valid = |byte: u8| {
        if hex {
            byte.is_ascii_hexdigit()
        } else {
            byte.is_ascii_digit()
        }
    };
    let mut count = 0;
    while let Some(&byte) = bytes.get(*cursor) {
        if valid(byte) {
            count += 1;
            *cursor += 1;
        } else if byte == b'_' {
            let previous = (*cursor > start && valid(bytes[*cursor - 1]))
                || (prefix_underscore && *cursor == start);
            if !previous || !bytes.get(*cursor + 1).is_some_and(|byte| valid(*byte)) {
                return Err(());
            }
            *cursor += 1;
        } else {
            break;
        }
    }
    Ok(count)
}

fn rounded_shift(value: u128, shift: i64, sticky: bool) -> u128 {
    if shift <= 0 {
        return value << (-shift) as u32;
    }
    if shift > 128 {
        return 0;
    }
    let (kept, remainder, halfway) = if shift == 128 {
        (0, value, 1_u128 << 127)
    } else {
        (
            value >> shift,
            value & ((1_u128 << shift) - 1),
            1_u128 << (shift - 1),
        )
    };
    kept + u128::from(remainder > halfway || (remainder == halfway && (sticky || kept & 1 != 0)))
}

fn hexadecimal(text: &str, negative: bool) -> Result<f64, ()> {
    let exponent_at = text.find(['p', 'P']).ok_or(())?;
    let exponent = &text[exponent_at + 1..];
    let exponent = exponent.parse::<i64>().unwrap_or_else(|_| {
        if exponent.starts_with('-') {
            i64::MIN
        } else {
            i64::MAX
        }
    });
    let mut mantissa = 0_u128;
    let mut fractional = false;
    let mut fractional_digits = 0_i64;
    let mut dropped_bits = 0_i64;
    let mut sticky = false;
    for byte in text[2..exponent_at].bytes() {
        if byte == b'.' {
            fractional = true;
            continue;
        }
        if fractional {
            fractional_digits += 1;
        }
        let digit = (byte as char).to_digit(16).ok_or(())? as u128;
        if mantissa <= u128::MAX >> 4 {
            mantissa = (mantissa << 4) | digit;
        } else {
            dropped_bits += 4;
            sticky |= digit != 0;
        }
    }
    let sign = u64::from(negative) << 63;
    if mantissa == 0 {
        return Ok(f64::from_bits(sign));
    }
    let bits = 128 - i64::from(mantissa.leading_zeros());
    let scale = exponent
        .saturating_sub(fractional_digits.saturating_mul(4))
        .saturating_add(dropped_bits);
    let mut binary_exponent = scale.saturating_add(bits - 1);
    if binary_exponent > 1023 {
        return Err(());
    }
    if binary_exponent < -1022 {
        let significand = rounded_shift(mantissa, -scale.saturating_add(1074), sticky);
        return Ok(f64::from_bits(sign | significand as u64));
    }
    let mut significand = rounded_shift(mantissa, bits - 53, sticky) as u64;
    if significand == 1_u64 << 53 {
        significand >>= 1;
        binary_exponent += 1;
    }
    if binary_exponent > 1023 {
        return Err(());
    }
    Ok(f64::from_bits(
        sign | ((binary_exponent + 1023) as u64) << 52 | (significand & ((1_u64 << 52) - 1)),
    ))
}

/// Parse the complete Go strconv.ParseFloat grammar at float64 precision.
pub fn ParseGoFloat64(text: &str) -> Result<f64, String> {
    if text.eq_ignore_ascii_case("nan") {
        return Ok(f64::from_bits(0x7ff8_0000_0000_0001));
    }
    let (negative, unsigned) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if unsigned.eq_ignore_ascii_case("inf") || unsigned.eq_ignore_ascii_case("infinity") {
        return Ok(if negative {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        });
    }
    let syntax = || number_error(text, false);
    if unsigned.is_empty() || !unsigned.is_ascii() {
        return Err(syntax());
    }
    let hex = unsigned.starts_with("0x") || unsigned.starts_with("0X");
    let bytes = unsigned.as_bytes();
    let mut cursor = if hex { 2 } else { 0 };
    let mut count = digits(bytes, &mut cursor, hex, hex).map_err(|_| syntax())?;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        count += digits(bytes, &mut cursor, hex, false).map_err(|_| syntax())?;
    }
    if count == 0 {
        return Err(syntax());
    }
    let has_exponent = bytes.get(cursor).is_some_and(|byte| {
        if hex {
            matches!(byte, b'p' | b'P')
        } else {
            matches!(byte, b'e' | b'E')
        }
    });
    if has_exponent {
        cursor += 1;
        if bytes
            .get(cursor)
            .is_some_and(|byte| matches!(byte, b'+' | b'-'))
        {
            cursor += 1;
        }
        if digits(bytes, &mut cursor, false, false).map_err(|_| syntax())? == 0 {
            return Err(syntax());
        }
    } else if hex {
        return Err(syntax());
    }
    if cursor != bytes.len() {
        return Err(syntax());
    }
    let normalized = unsigned.replace('_', "");
    if hex {
        return hexadecimal(&normalized, negative).map_err(|_| number_error(text, true));
    }
    let value = normalized.parse::<f64>().map_err(|_| syntax())?;
    if value.is_infinite() {
        return Err(number_error(text, true));
    }
    Ok(if negative { -value } else { value })
}

/// docker/go-units v0.5.0 size grammar; binary selects RAMInBytes rather than
/// FromHumanSize. Both use the same suffix spelling and last-separator rule.
pub fn ParseGoSize(text: &str, binary: bool) -> Result<i64, String> {
    let invalid = || format!("invalid size: '{text}'");
    let separator = text
        .rfind(|c: char| c.is_ascii_digit() || c == '.' || c == ' ')
        .ok_or_else(invalid)?;
    let (number, suffix) = if text.as_bytes()[separator] == b' ' {
        (&text[..separator], &text[separator + 1..])
    } else {
        (&text[..separator + 1], &text[separator + 1..])
    };
    let mut size = ParseGoFloat64(number)?;
    if size < 0.0 {
        return Err(invalid());
    }
    if suffix.len() > 3 {
        return Err(format!("invalid suffix: '{suffix}'"));
    }
    let suffix = suffix.to_ascii_lowercase();
    if !suffix.is_empty() && suffix != "b" {
        let exponent = match suffix.as_bytes()[0] {
            b'k' => 1,
            b'm' => 2,
            b'g' => 3,
            b't' => 4,
            b'p' => 5,
            _ => return Err(format!("invalid suffix: '{suffix}'")),
        };
        if !matches!(&suffix[1..], "" | "b" | "ib") {
            return Err(format!("invalid suffix: '{suffix}'"));
        }
        size *= (if binary { 1024_f64 } else { 1000_f64 }).powi(exponent);
    }
    // Go's conversion on the supported 64-bit hosts yields MinInt64 for NaN,
    // infinity or overflow; Rust's saturating float cast would change it.
    Ok(if !size.is_finite() || size >= 9223372036854775808.0 {
        i64::MIN
    } else {
        size as i64
    })
}

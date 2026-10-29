// Copyright 2026 AsterSQL.
//! Selected PostgreSQL numeric templates. Decimal arithmetic stays in PgNumeric;
//! literal whitespace is distinct from padding suppressed by FM.
use types_dependency::pg_numeric::{PgNumeric, PgNumericError};

#[derive(Clone, Copy)]
enum Token {
    Digit(bool),
    Point,
    Shift,
    Literal(char),
}

pub(crate) fn format_numeric(value: PgNumeric, template: &str) -> Result<String, PgNumericError> {
    let mut tokens = Vec::new();
    let mut rest = template;
    let mut fill = false;
    let mut point = false;
    let mut shift = false;
    let mut integer_count = 0usize;
    let mut fraction_count = 0usize;
    let mut shift_count = 0;
    let mut required_fraction = 0;
    let mut first_zero = None;
    while let Some(ch) = rest.chars().next() {
        if rest.starts_with("FM") || rest.starts_with("fm") {
            fill = true;
            rest = &rest[2..];
            continue;
        }
        rest = &rest[ch.len_utf8()..];
        if ch == '"' {
            while let Some(literal) = rest.chars().next() {
                rest = &rest[literal.len_utf8()..];
                if literal == '"' {
                    break;
                }
                if literal == '\\' {
                    if let Some(escaped) = rest.chars().next() {
                        rest = &rest[escaped.len_utf8()..];
                        tokens.push(Token::Literal(escaped));
                    }
                } else {
                    tokens.push(Token::Literal(literal));
                }
            }
            continue;
        }
        match ch {
            '9' | '0' => {
                if point {
                    fraction_count += 1;
                    if ch == '0' {
                        required_fraction = fraction_count;
                    }
                } else {
                    if ch == '0' && first_zero.is_none() {
                        first_zero = Some(integer_count);
                    }
                    integer_count += 1;
                    if shift {
                        shift_count += 1;
                    }
                }
                tokens.push(Token::Digit(ch == '0'));
            }
            '.' => {
                if shift {
                    return Err(PgNumericError::FormatShiftPoint);
                }
                if point {
                    return Err(PgNumericError::FormatMultiplePoints);
                }
                point = true;
                tokens.push(Token::Point);
            }
            'V' | 'v' => {
                if point {
                    return Err(PgNumericError::FormatShiftPoint);
                }
                shift = true;
                tokens.push(Token::Shift);
            }
            _ => tokens.push(Token::Literal(ch)),
        }
    }
    // PostgreSQL treats numeric's empty template as an empty string, unlike
    // timestamp formatting. No trim() may run on the template or the result.
    if integer_count + fraction_count == 0 {
        return Ok(tokens
            .iter()
            .filter_map(|t| {
                if let Token::Literal(c) = t {
                    Some(*c)
                } else {
                    None
                }
            })
            .collect());
    }
    let original = value.to_string();
    let negative = original.starts_with('-');
    let value = if shift_count != 0 {
        format!("{original}e{shift_count}").parse::<PgNumeric>()?
    } else {
        value
    };
    if fraction_count > 16383 {
        return Err(PgNumericError::OutOfRange);
    }
    let rounded = value.round_scale(fraction_count as i32).to_string();
    let unsigned = rounded.strip_prefix('-').unwrap_or(&rounded);
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let special = matches!(unsigned, "NaN" | "Infinity");
    let overflow = special || integer.len() > integer_count;
    let mut fraction_end = fraction_count as usize;
    if fill && !overflow {
        while fraction_end > required_fraction as usize
            && fraction.as_bytes().get(fraction_end - 1) == Some(&b'0')
        {
            fraction_end -= 1;
        }
    }
    // An optional integer zero before a fraction is blank, but an all-integer
    // template must still print zero. The sign follows blank integer padding.
    let integer = if integer == "0" && point && first_zero.is_none() {
        ""
    } else {
        integer
    };
    let padding = integer_count.saturating_sub(integer.len());
    let mut integer_at = 0;
    let mut fraction_at = 0;
    let mut after_point = false;
    let mut signed = false;
    let mut result = String::new();
    for token in tokens {
        match token {
            Token::Literal(c) => result.push(c),
            Token::Shift => {}
            Token::Point => {
                if !signed {
                    if negative {
                        result.push('-');
                    } else if !fill {
                        result.push(' ');
                    }
                    signed = true;
                }
                result.push('.');
                after_point = true;
            }
            Token::Digit(required) => {
                let digit = if overflow {
                    Some('#')
                } else if after_point {
                    (fraction_at < fraction_end).then(|| fraction.as_bytes()[fraction_at] as char)
                } else if integer_at < padding {
                    if required || first_zero.is_some_and(|first| integer_at >= first) {
                        Some('0')
                    } else if fill {
                        None
                    } else {
                        Some(' ')
                    }
                } else {
                    Some(integer.as_bytes()[integer_at - padding] as char)
                };
                if let Some(digit) = digit {
                    if digit != ' ' && !signed {
                        if negative {
                            result.push('-');
                        } else if !fill {
                            result.push(' ');
                        }
                        signed = true;
                    }
                    result.push(digit);
                }
                if after_point {
                    fraction_at += 1;
                } else {
                    integer_at += 1;
                }
            }
        }
    }
    Ok(result)
}

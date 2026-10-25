// Copyright 2026 AsterSQL.
use super::pg_numeric::{PgNumeric, PgNumericError};
#[test]
fn pg_numeric_test() {
    for (input, expected) in [
        ("12.340", "12.340"),
        ("-0.00", "0.00"),
        ("0012.340", "12.340"),
        ("12.340e2", "1234.0"),
        (".001", "0.001"),
        ("1e3", "1000"),
    ] {
        assert_eq!(input.parse::<PgNumeric>().unwrap().to_string(), expected);
    }
    for value in ["9".repeat(131072), format!("0.{}1", "0".repeat(16382))] {
        assert_eq!(value.parse::<PgNumeric>().unwrap().to_string(), value);
    }
    for value in ["9".repeat(131073), format!("0.{}1", "0".repeat(16383))] {
        assert_eq!(value.parse::<PgNumeric>(), Err(PgNumericError::OutOfRange));
    }
    for value in ["", ".", "1x", "1e", "1e+", "1.2.3"] {
        assert_eq!(
            value.parse::<PgNumeric>(),
            Err(PgNumericError::InvalidSyntax)
        );
    }
}

#[test]
fn pg_numeric_test_expansion_and_zero_safety() {
    for value in [
        "1e131072",
        "1e-16384",
        "1e9223372036854775807",
        "1e-9223372036854775808",
    ] {
        assert_eq!(value.parse::<PgNumeric>(), Err(PgNumericError::OutOfRange));
    }
    assert_eq!("-0e131072".parse::<PgNumeric>().unwrap().to_string(), "0");
    assert_eq!(
        "1e131071".parse::<PgNumeric>().unwrap().to_string().len(),
        131072
    );
}

#[test]
fn pg_numeric_test_modifiers() {
    for (input, p, s, expected) in [
        ("1.25", 3, 1, "1.3"),
        ("-1.25", 3, 1, "-1.3"),
        ("1250", 2, -2, "1300"),
        ("-1250", 2, -2, "-1300"),
        ("0.000995", 2, 4, "0.0010"),
        ("0.0005", 1, 3, "0.001"),
        ("-0.04", 2, 1, "0.0"),
        ("0", 1, -1000, "0"),
        ("1", 1000, 1000, "OVERFLOW"),
        ("0.001", 1, 3, "0.001"),
    ] {
        let result = input.parse::<PgNumeric>().unwrap().apply_modifier(p, s);
        if expected == "OVERFLOW" {
            assert_eq!(result, Err(PgNumericError::OutOfRange));
        } else {
            assert_eq!(result.unwrap().to_string(), expected, "{input} ({p},{s})");
        }
    }
    for value in ["99.95", "-99.95"] {
        assert_eq!(
            value.parse::<PgNumeric>().unwrap().apply_modifier(3, 1),
            Err(PgNumericError::OutOfRange)
        );
    }
    for (p, s) in [(0, 0), (1001, 0), (1, -1001), (1, 1001)] {
        assert_eq!(
            PgNumeric::validate_modifier(p, s),
            Err(PgNumericError::InvalidModifier)
        );
    }
}

#[test]
fn pg_numeric_test_special_values() {
    for (input, expected) in [
        ("NaN", "NaN"),
        (" nan ", "NaN"),
        ("Infinity", "Infinity"),
        ("+INF", "Infinity"),
        ("-infinity", "-Infinity"),
        (" -inf ", "-Infinity"),
    ] {
        let value = input.parse::<PgNumeric>().unwrap();
        assert_eq!(value.to_string(), expected);
        assert_eq!(expected.parse::<PgNumeric>().unwrap(), value);
        assert_ne!(value, "0".parse::<PgNumeric>().unwrap());
        for (p, s) in [(1, 0), (3, 1), (1, -1000), (1, 1000)] {
            let result = value.clone().apply_modifier(p, s);
            if expected == "NaN" {
                assert_eq!(result.unwrap().to_string(), "NaN");
            } else {
                assert_eq!(result, Err(PgNumericError::OutOfRange));
            }
        }
    }
    assert_ne!(
        "Infinity".parse::<PgNumeric>(),
        "-Infinity".parse::<PgNumeric>()
    );
    for input in ["+NaN", "-NaN", "NaNx", "Infinityx", "Inf inity"] {
        assert_eq!(
            input.parse::<PgNumeric>(),
            Err(PgNumericError::InvalidSyntax)
        );
    }
}

#[test]
fn signed_integer_rounding() {
    use super::pg_numeric::PgNumeric;
    for bits in [16, 32, 64] {
        for (text, expected) in [
            ("1.5", 2),
            ("2.5", 3),
            ("-1.5", -2),
            ("-2.5", -3),
            ("0.00000000001", 0),
        ] {
            assert_eq!(
                text.parse::<PgNumeric>().unwrap().to_signed(bits),
                Ok(expected)
            );
        }
        for text in ["NaN", "Infinity", "-Infinity", "1e1000"] {
            assert_eq!(
                text.parse::<PgNumeric>()
                    .unwrap()
                    .to_signed(bits)
                    .unwrap_err()
                    .sqlstate(),
                "22003"
            );
        }
    }
}

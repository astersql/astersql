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

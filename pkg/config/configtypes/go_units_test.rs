// Copyright 2026 AsterSQL.

use super::{ParseGoFloat64, ParseGoSize};

#[test]
fn go_float_hexadecimal_rounding_and_underscore_grammar() {
    for (input, bits) in [
        ("0x1p0", 0x3ff0_0000_0000_0000),
        ("0X_1.8p+1", 0x4008_0000_0000_0000),
        ("-0x0p100000000000000000000", 0x8000_0000_0000_0000),
        ("0x1.00000000000008p0", 0x3ff0_0000_0000_0000),
        (
            "0x1.00000000000008000000000000000000000000000000000001p0",
            0x3ff0_0000_0000_0001,
        ),
        ("0x1.00000000000018p0", 0x3ff0_0000_0000_0002),
        ("0x1p-1074", 1),
        ("0x1p-1075", 0),
        ("0x1.000000000000000000000000000000000000000000001p-1075", 1),
        ("0x1.fffffffffffffp1023", 0x7fef_ffff_ffff_ffff),
        ("1_2.5e+1_0", 125_000_000_000_f64.to_bits()),
        ("0x1p-10000000000000000000", 0),
        ("0e100000000000000000000", 0),
    ] {
        assert_eq!(ParseGoFloat64(input).unwrap().to_bits(), bits, "{input}");
    }
    for invalid in [
        "0x1", "0x_.1p0", "0x1._0p0", "0x1p_0", "0x1p+_1", "1__2", "1_.2", "1._2", "1e_2", "+NaN",
        " 1", "1 ",
    ] {
        assert!(ParseGoFloat64(invalid).is_err(), "{invalid}");
    }
    for overflow in ["1e999", "0x1p1024", "0x1.fffffffffffff8p1023"] {
        assert!(
            ParseGoFloat64(overflow)
                .unwrap_err()
                .contains("value out of range"),
            "{overflow}"
        );
    }
}

#[test]
fn go_units_preserves_decimal_binary_separator_and_conversion_contract() {
    assert_eq!(
        ParseGoSize("1KIBB", false).unwrap_err(),
        "invalid suffix: 'KIBB'"
    );
    for (input, decimal, binary) in [
        ("32.5 kB", 32500, 33280),
        (".3kB", 300, 307),
        ("0x1p4MiB", 16_000_000, 16_777_216),
        ("1_024B", 1024, 1024),
        ("-0 B", 0, 0),
        ("NaN B", i64::MIN, i64::MIN),
        ("Inf B", i64::MIN, i64::MIN),
        ("1000000PB", i64::MIN, i64::MIN),
    ] {
        assert_eq!(ParseGoSize(input, false).unwrap(), decimal, "{input}");
        assert_eq!(ParseGoSize(input, true).unwrap(), binary, "{input}");
    }
    for invalid in [
        "", "-1B", "1  B", " 1", "1B ", "1BB", "1Ki", "1M iB", "1_024_B",
    ] {
        assert!(ParseGoSize(invalid, false).is_err(), "{invalid}");
    }
}

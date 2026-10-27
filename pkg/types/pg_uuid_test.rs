// Copyright 2026 AsterSQL.

use crate::pg_uuid::{PgUuid, PgUuidError};

const COMPACT: &str = "00112233445566778899aabbccddeeff";
const CANONICAL: &str = "00112233-4455-6677-8899-aabbccddeeff";
const BYTES: [u8; 16] = [
    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];

#[test]
fn compact_bytes_and_canonical_output() {
    let uuid: PgUuid = COMPACT.parse().unwrap();
    assert_eq!(uuid.as_bytes(), &BYTES);
    assert_eq!(uuid, PgUuid::from_bytes(BYTES));
    assert_eq!(uuid.to_string(), CANONICAL);
    assert_eq!(uuid.to_string().len(), 36);
    assert_eq!(std::mem::size_of::<PgUuid>(), 16);
}

#[test]
fn every_optional_separator_brace_and_case_combination() {
    // Seven optional boundaries after groups of four digits, never at the end.
    for separators in 0..128 {
        let mut input = String::new();
        for (i, group) in COMPACT.as_bytes().chunks_exact(4).enumerate() {
            if i > 0 && separators & (1 << (i - 1)) != 0 {
                input.push('-');
            }
            input.push_str(std::str::from_utf8(group).unwrap());
        }
        for spelling in [input.clone(), input.to_uppercase()] {
            for spelling in [spelling.clone(), format!("{{{spelling}}}")] {
                let actual = spelling.parse::<PgUuid>().expect(&spelling);
                assert_eq!(actual.as_bytes(), &BYTES, "{spelling}");
                assert_eq!(actual.to_string(), CANONICAL);
                assert_eq!(actual.to_string().parse::<PgUuid>(), Ok(actual));
            }
        }
    }
}

#[test]
fn rejects_invalid_lexemes_with_sqlstate_and_recovers() {
    for input in [
        "",
        "00112233445566778899aabbccddeef",
        "00112233445566778899aabbccddeeff0",
        "00112233445566778899aabbccddeefg",
        "00112233-4455-6677-8899-aabbccddeef",
        "0011--2233445566778899aabbccddeeff",
        "-00112233445566778899aabbccddeeff",
        "00112233445566778899aabbccddeeff-",
        "00-112233445566778899aabbccddeeff",
        "001122-33445566778899aabbccddeeff",
        "{00112233445566778899aabbccddeeff",
        "00112233445566778899aabbccddeeff}",
        "{{00112233445566778899aabbccddeeff}}",
        "urn:uuid:00112233-4455-6677-8899-aabbccddeeff",
        " 00112233445566778899aabbccddeeff",
        "00112233445566778899aabbccddeeff ",
        "\t00112233445566778899aabbccddeeff",
        "00112233445566778899aabbccddeeff\n",
        "{ 00112233445566778899aabbccddeeff}",
        "00112233445566778899aabbccddeeff\0",
        "é0112233445566778899aabbccddeeff",
        "+0112233445566778899aabbccddeeff",
        "００112233445566778899aabbccddeeff",
    ] {
        let err = input.parse::<PgUuid>().expect_err(input);
        assert_eq!(err, PgUuidError);
        assert_eq!(err.sqlstate(), "22P02", "{input:?}");
        assert_eq!(err.to_string(), "invalid input syntax for type uuid");
        assert_eq!(COMPACT.parse::<PgUuid>().unwrap().as_bytes(), &BYTES);
    }
}

#[test]
fn all_byte_values_and_version_variant_bits_round_trip() {
    for value in 0..=255u8 {
        let bytes = [value; 16];
        let uuid = PgUuid::from_bytes(bytes);
        assert_eq!(uuid.to_string().parse::<PgUuid>(), Ok(uuid));
        assert_eq!(uuid.as_bytes(), &bytes);
    }
    for index in 0..16 {
        for value in [1, 128] {
            let mut bytes = [0; 16];
            bytes[index] = value;
            let uuid = PgUuid::from_bytes(bytes);
            assert_eq!(uuid.to_string().parse::<PgUuid>(), Ok(uuid));
            assert_eq!(uuid.as_bytes(), &bytes);
        }
    }
}

#[test]
fn sql_null_is_distinct_from_zero_uuid() {
    let null: Option<PgUuid> = None;
    let zero = PgUuid::from_bytes([0; 16]);
    assert_ne!(null, Some(zero));
    assert_eq!(zero.to_string(), "00000000-0000-0000-0000-000000000000");
    assert_eq!(zero.to_string().parse::<PgUuid>(), Ok(zero));
}

// Copyright 2026 AsterSQL.
use crate::pg_int4range::PgInt4Range;
#[test]
fn pg_int4range_codec() {
    for (source, expected, empty, lower, upper) in [
        ("[4,4)", "empty", true, None, None),
        ("[4,4]", "[4,5)", false, Some(4), Some(5)),
        ("(1,4]", "[2,5)", false, Some(2), Some(5)),
        ("(,4)", "(,4)", false, None, Some(4)),
        ("[1,)", "[1,)", false, Some(1), None),
        ("(,)", "(,)", false, None, None),
        ("empty", "empty", true, None, None),
        ("[\"1\",\"4\"]", "[1,5)", false, Some(1), Some(5)),
        (" [ -2, 4) ", "[-2,4)", false, Some(-2), Some(4)),
        ("[2147483647,2147483647)", "empty", true, None, None),
    ] {
        let r = source.parse::<PgInt4Range>().unwrap();
        assert_eq!(r.to_string(), expected);
        assert_eq!(r.is_empty(), empty);
        assert_eq!(r.lower(), lower);
        assert_eq!(r.upper(), upper);
        assert_eq!(PgInt4Range::from_binary(&r.to_binary()).unwrap(), r);
    }
    for (source, state) in [
        ("[4,1)", "22000"),
        ("[1,2147483647]", "22003"),
        ("(2147483647,)", "22003"),
        ("[x,4)", "22P02"),
        ("[, ]", "22P02"),
        ("[1,2", "22P02"),
        ("[\"\",2)", "22P02"),
    ] {
        assert_eq!(source.parse::<PgInt4Range>().unwrap_err().state, state);
    }
    // Container syntax is checked before invoking either subtype input function.
    assert_eq!(
        "[2147483648,2".parse::<PgInt4Range>().unwrap_err().state,
        "22P02"
    );
    for (bytes, state) in [
        (&b""[..], "08P01"),
        (&b"\x02\0\0\0\x04\0"[..], "08P01"),
        (&b"\x01\0"[..], "22P03"),
    ] {
        assert_eq!(PgInt4Range::from_binary(bytes).unwrap_err().state, state);
    }
    assert_eq!(
        PgInt4Range::from_binary(&[0xff]).unwrap(),
        PgInt4Range::empty()
    );
    assert_eq!(PgInt4Range::OID, 3904);
    assert_eq!(PgInt4Range::SUBTYPE_OID, 23);
}

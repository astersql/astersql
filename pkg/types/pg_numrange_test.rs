// Copyright 2026 AsterSQL.
use crate::pg_numrange::PgNumRange;
#[test]
fn pg08_24_numeric_range_codec() {
    for s in [
        "empty",
        "[1,3)",
        "(1,3]",
        "[1,3]",
        "(1,3)",
        "[4,4]",
        "(,4)",
        "[1,)",
        "(,)",
        "(1.00000000000000000001,1.00000000000000000002]",
        "[-Infinity,Infinity]",
        "[NaN,NaN]",
        "[-2.00,-1.0)",
        "[1.00,1.0]",
        "(-Infinity,)",
        "(,NaN]",
    ] {
        let r = s.parse::<PgNumRange>().unwrap();
        assert_eq!(r.to_string(), s);
        let b = r.to_binary();
        assert_eq!(PgNumRange::from_binary(&b).unwrap(), r);
    }
    for s in ["[4,1)", "[NaN,1)", "[x,1)", "[1,4)garbage"] {
        assert!(s.parse::<PgNumRange>().is_err(), "{s}");
    }
    assert_eq!(
        "[1.0,1.00)".parse::<PgNumRange>().unwrap().to_string(),
        "empty"
    );
}

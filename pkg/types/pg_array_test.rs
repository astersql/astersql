// Copyright 2026 AsterSQL.

use crate::pg_array::{PgArray, PgArrayElement::Int4, PgArrayError as Error};

#[test]
fn pg05_array_value_shape_lower_identity_and_row_major() {
    for (shape, lower) in [(vec![4], vec![1]), (vec![2, 2], vec![0, -2])] {
        let elements = vec![
            Some(Int4(i32::MIN)),
            Some(Int4(2)),
            None,
            Some(Int4(i32::MAX)),
        ];
        let value = PgArray::new(23, -1, shape.clone(), lower.clone(), elements.clone()).unwrap();
        assert_eq!(value.element_oid(), 23);
        assert_eq!(value.element_typmod(), -1);
        assert_eq!(value.shape(), shape);
        assert_eq!(value.lower_bounds(), lower);
        assert_eq!(value.elements(), elements);
        assert!(value.has_nulls());
        assert_eq!(value.clone(), value);
    }
    let typed = PgArray::new(23, 42, vec![1], vec![1], vec![Some(Int4(9))]).unwrap();
    assert_eq!(typed.element_typmod(), 42);
    assert!(!typed.has_nulls());
}

#[test]
fn pg05_array_value_empty_array_null_and_element_null() {
    let sql_null: Option<PgArray> = None;
    let empty = PgArray::new(23, -1, vec![], vec![], vec![]).unwrap();
    let element_null = PgArray::new(23, -1, vec![1], vec![1], vec![None]).unwrap();
    assert!(sql_null.is_none());
    assert_ne!(Some(empty.clone()), sql_null);
    assert_ne!(empty, element_null);
    assert!(!empty.has_nulls());
    assert!(element_null.has_nulls());
    assert_eq!(element_null.elements(), &[None]);
    for (shape, lower) in [(vec![0], vec![-9]), (vec![2, 0, 3], vec![0, -2, 1])] {
        assert_eq!(PgArray::new(23, -1, shape, lower, vec![]).unwrap(), empty);
    }
    assert_eq!(empty.element_oid(), 23);
    assert_eq!(empty.element_typmod(), -1);
}

#[test]
fn pg05_array_value_rejects_inconsistent_shapes_and_recovers() {
    for (shape, lower, elements, error) in [
        (vec![1], vec![], vec![None], Error::DimensionMismatch),
        (vec![], vec![1], vec![], Error::DimensionMismatch),
        (
            vec![2, 2],
            vec![1, 1],
            vec![None; 3],
            Error::ElementCountMismatch,
        ),
        (vec![1], vec![1], vec![None; 2], Error::ElementCountMismatch),
        (vec![], vec![], vec![None], Error::ElementCountMismatch),
        (vec![0], vec![1], vec![None], Error::ElementCountMismatch),
    ] {
        assert_eq!(PgArray::new(23, -1, shape, lower, elements), Err(error));
        assert_eq!(error.sqlstate(), None);
        assert!(PgArray::new(23, -1, vec![1], vec![1], vec![Some(Int4(1))]).is_ok());
    }
}

#[test]
fn pg05_array_value_dimension_size_and_signed_bounds_limits() {
    assert!(PgArray::new(23, -1, vec![1; 6], vec![i32::MIN; 6], vec![Some(Int4(1))]).is_ok());
    assert!(PgArray::new(23, -1, vec![1], vec![i32::MAX - 1], vec![None]).is_ok());
    for (shape, lower, error) in [
        (vec![1; 7], vec![1; 7], Error::TooManyDimensions),
        (vec![-1], vec![1], Error::SizeOverflow),
        (
            vec![(0x3fffffff / std::mem::size_of::<usize>() + 1) as i32],
            vec![0],
            Error::SizeOverflow,
        ),
        (vec![46341, 46341], vec![1, 1], Error::SizeOverflow),
        (vec![i32::MAX; 6], vec![0; 6], Error::SizeOverflow),
        (vec![1], vec![i32::MAX], Error::BoundsOverflow),
        (vec![2], vec![i32::MAX - 1], Error::BoundsOverflow),
        (vec![0, -1], vec![1, 1], Error::SizeOverflow),
        (vec![0, 1], vec![1, i32::MAX], Error::BoundsOverflow),
    ] {
        assert_eq!(PgArray::new(23, -1, shape, lower, vec![]), Err(error));
        assert_eq!(error.sqlstate(), Some("54000"));
    }
}

#[test]
fn pg05_array_value_unopened_families_rejected_even_when_empty_or_null() {
    for oid in [0, 1007, 3802, u32::MAX] {
        for (shape, lower, elements) in [
            (vec![], vec![], vec![]),
            (vec![1], vec![1], vec![None]),
            (vec![1], vec![1], vec![Some(Int4(1))]),
        ] {
            let error = PgArray::new(oid, -1, shape, lower, elements).unwrap_err();
            assert_eq!(error, Error::UnsupportedElement);
            assert_eq!(error.sqlstate(), Some("0A000"));
        }
    }
}

fn text_array(
    input: &[u8],
    delimiter: u8,
) -> Result<PgArray, crate::pg_array::PgArrayTextError<std::convert::Infallible>> {
    PgArray::from_text_with(input, 25, -1, delimiter, |s, _| {
        Ok(crate::pg_array::PgArrayElement::Text(s.into()))
    })
}

#[test]
fn pg05_array_text_input_shape_lower_and_identity() {
    use crate::pg_array::PgArrayElement::Text;
    let a = text_array(b" [0:1][-2:-1] = {{1,2},{3,4}} ", b',').unwrap();
    assert_eq!(a.shape(), [2, 2]);
    assert_eq!(a.lower_bounds(), [0, -2]);
    assert_eq!(a.element_oid(), 25);
    assert_eq!(a.element_typmod(), -1);
    assert_eq!(
        a.elements(),
        [
            Some(Text("1".into())),
            Some(Text("2".into())),
            Some(Text("3".into())),
            Some(Text("4".into()))
        ]
    );
    let mut seen = Vec::new();
    let a = PgArray::from_text_with(b"{one,NULL,two}", 25, 42, b',', |s, typmod| {
        seen.push((s.to_owned(), typmod));
        Ok::<_, std::convert::Infallible>(Text(s.into()))
    })
    .unwrap();
    assert_eq!(a.element_typmod(), 42);
    assert_eq!(seen, [("one".into(), 42), ("two".into(), 42)]);
    assert_eq!(a.elements()[1], None);
}

#[test]
fn pg05_array_text_input_null_quotes_escapes_unicode_and_delimiter() {
    use crate::pg_array::PgArrayElement::Text;
    let a = text_array(
        r#"{"NULL",NULL,null,NuLl,"",N\ULL,"a,b","a\\b","a\"b","中",  x y  ,\ x\ ,"  "}"#
            .as_bytes(),
        b',',
    )
    .unwrap();
    let expected = [
        Some("NULL"),
        None,
        None,
        None,
        Some(""),
        Some("NULL"),
        Some("a,b"),
        Some(r"a\b"),
        Some("a\"b"),
        Some("中"),
        Some("x y"),
        Some(" x "),
        Some("  "),
    ];
    assert_eq!(a.elements(), expected.map(|s| s.map(|s| Text(s.into()))));
    let a = text_array(br#"{a,b;"x;y";\;;null}"#, b';').unwrap();
    assert_eq!(
        a.elements(),
        [
            Some(Text("a,b".into())),
            Some(Text("x;y".into())),
            Some(Text(";".into())),
            None
        ]
    );
    // Bind bytes have only array escaping; backslash-n stays the two characters.
    assert_eq!(
        text_array(br#"{"a\\n"}"#, b',').unwrap().elements(),
        [Some(Text(r"a\n".into()))]
    );
}

#[test]
fn pg05_array_text_input_invalid_container_before_elements_and_recovery() {
    for input in [
        "",
        " ",
        "1",
        "{1,}",
        "{,1}",
        "{1,,2}",
        "{\"x}",
        "{x\\",
        "{\"x\\",
        "{x\"y}",
        "{\"x\"y}",
        "{x{y}",
        "{{1,2},{3}}",
        "{{1},2}",
        "{1,{2}}",
        "[0:2]={1,2}",
        "[0:1][1:1]={1,2}",
        "[1:2]{1,2}",
        "[x:2]={1,2}",
        "{1}junk",
    ] {
        let mut called = false;
        let result = PgArray::from_text_with(input.as_bytes(), 25, -1, b',', |_, _| {
            called = true;
            Ok::<_, std::convert::Infallible>(crate::pg_array::PgArrayElement::Text(
                "unused".into(),
            ))
        });
        assert_eq!(result.unwrap_err().sqlstate(), Some("22P02"), "{input}");
        assert!(
            !called,
            "container must be checked before scalar input: {input}"
        );
        assert!(text_array(b"{ok}", b',').is_ok());
    }
}

#[test]
fn pg05_array_text_input_empty_encoding_limits_and_scalar_failure() {
    use crate::pg_array::{PgArrayElement::Text, PgArrayTextError};
    for input in ["{}", " { } ", "{{},{}}", "\u{000b}{\u{000c}}\u{000b}"] {
        let a = text_array(input.as_bytes(), b',').unwrap();
        assert!(a.shape().is_empty());
        assert!(a.lower_bounds().is_empty());
        assert!(a.elements().is_empty());
        assert_eq!(a.element_oid(), 25);
    }
    for input in [b"{\xff}".as_slice(), b"{\xc3(}", b"{a\0b}"] {
        assert_eq!(
            text_array(input, b',').unwrap_err().sqlstate(),
            Some("22021")
        );
    }
    assert!(text_array(b"{{{{{{x}}}}}}", b',').is_ok());
    for input in ["{{{{{{{x}}}}}}}", "[2147483647:2147483647]={x}"] {
        assert_eq!(
            text_array(input.as_bytes(), b',').unwrap_err().sqlstate(),
            Some("54000")
        );
    }
    for input in ["[-2147483648:2147483646]={x}", "[1][1][1][1][1][1][1]={x}"] {
        assert_eq!(
            text_array(input.as_bytes(), b',').unwrap_err().sqlstate(),
            Some("54000")
        );
    }
    for delimiter in [0, b' ', b'"', b'{', b'\\', 0xff] {
        assert_eq!(
            text_array(b"{}", delimiter).unwrap_err().sqlstate(),
            Some("0A000")
        );
    }
    let mut seen = Vec::new();
    let err = PgArray::from_text_with(b"{ok,bad,never}", 25, -1, b',', |s, _| {
        seen.push(s.to_owned());
        if s == "bad" {
            Err("scalar diagnostic")
        } else {
            Ok(Text(s.into()))
        }
    })
    .unwrap_err();
    assert_eq!(err, PgArrayTextError::Element("scalar diagnostic"));
    assert_eq!(seen, ["ok", "bad"]);
}

#[test]
fn pg05_array_text_input_carrier_identity_and_unopened_paths() {
    use crate::pg_array::{PgArrayElement::Text, PgArrayTextError};
    for (oid, value) in [(23, Text("x".into())), (25, Int4(1))] {
        assert_eq!(
            PgArray::new(oid, -1, vec![1], vec![1], vec![Some(value)]),
            Err(Error::ElementTypeMismatch)
        );
    }
    assert_eq!(
        PgArray::new(25, -1, vec![1], vec![1], vec![Some(Text("x\0y".into()))]),
        Err(Error::InvalidEncoding)
    );
    for oid in [0, 1083, 1007, 3802] {
        let err = PgArray::from_text_with(b"{}", oid, -1, b',', |_, _| {
            Ok::<_, std::convert::Infallible>(Int4(1))
        })
        .unwrap_err();
        assert_eq!(err.sqlstate(), Some("0A000"));
    }
    assert_eq!(
        text_array(b"[1:0]={}", b',').unwrap_err(),
        PgArrayTextError::Bounds
    );
    assert_eq!(
        text_array(b"[1:0]={}", b',').unwrap_err().sqlstate(),
        Some("2202E")
    );
    assert_eq!(text_array(b"[2]={x,y}", b',').unwrap().lower_bounds(), [1]);
    assert_eq!(
        text_array(b"[2147483648:2147483648]={x}", b',')
            .unwrap_err()
            .sqlstate(),
        Some("54000")
    );
    assert_eq!(
        text_array(b"[ 0 : 1 ]={x,y}", b',').unwrap_err().sqlstate(),
        Some("22P02")
    );
}

#[test]
fn pg05_array_text_output_shape_lower_row_major_and_int4() {
    for (input, expected) in [
        (
            "{+001,-0,2147483647,-2147483648,NULL}",
            "{1,0,2147483647,-2147483648,NULL}",
        ),
        ("[0:1][-2:-1]={{1,2},{3,4}}", "[0:1][-2:-1]={{1,2},{3,4}}"),
        ("[1:2][0:1]={{1,2},{3,4}}", "[1:2][0:1]={{1,2},{3,4}}"),
        (
            "[-2147483648:-2147483648]={1}",
            "[-2147483648:-2147483648]={1}",
        ),
        ("[2147483646:2147483646]={1}", "[2147483646:2147483646]={1}"),
        ("{{{{{{1,NULL}}}}}}", "{{{{{{1,NULL}}}}}}"),
    ] {
        let parse = |s: &str| {
            PgArray::from_text_with(s.as_bytes(), 23, 42, b',', |s, _| {
                s.parse::<i32>().map(Int4)
            })
            .unwrap()
        };
        let value = parse(input);
        let output = value.to_text(b',').unwrap();
        assert_eq!(output, expected);
        assert_eq!(parse(&output), value);
        assert_eq!(value.element_oid(), 23);
        assert_eq!(value.element_typmod(), 42);
    }
}

#[test]
fn pg05_array_text_output_quotes_escapes_and_whitespace() {
    use crate::pg_array::PgArrayElement::Text;
    let cases = [
        ("", r#""""#),
        ("NULL", r#""NULL""#),
        ("null", r#""null""#),
        ("NuLl", r#""NuLl""#),
        ("NULLx", "NULLx"),
        ("中", "中"),
        ("a,b", r#""a,b""#),
        ("{x}", r#""{x}""#),
        ("[x]", "[x]"),
        ("a\\b", r#""a\\b""#),
        ("a\"b", r#""a\"b""#),
        (" x ", r#"" x ""#),
        ("a b", r#""a b""#),
        ("\t", "\"\t\""),
        ("\n", "\"\n\""),
        ("\r", "\"\r\""),
        ("\u{000b}", "\"\u{000b}\""),
        ("\u{000c}", "\"\u{000c}\""),
        ("\u{00a0}", "\u{00a0}"),
    ];
    for (text, quoted) in cases {
        let value = PgArray::new(
            25,
            -1,
            vec![2],
            vec![1],
            vec![Some(Text(text.into())), None],
        )
        .unwrap();
        let output = value.to_text(b',').unwrap();
        assert_eq!(output, format!("{{{quoted},NULL}}"), "{text:?}");
        assert_eq!(text_array(output.as_bytes(), b',').unwrap(), value);
    }
}

#[test]
fn pg05_array_text_output_empty_and_two_null_layers() {
    let sql_null: Option<PgArray> = None;
    assert!(sql_null.is_none());
    for oid in [23, 25] {
        let empty = PgArray::new(oid, 42, vec![2, 0], vec![-2, 9], vec![]).unwrap();
        assert_eq!(empty.to_text(b',').unwrap(), "{}");
        assert_eq!(empty.element_oid(), oid);
        assert_eq!(empty.element_typmod(), 42);
        let nulls = PgArray::new(oid, 42, vec![2], vec![0], vec![None, None]).unwrap();
        assert_eq!(nulls.to_text(b',').unwrap(), "[0:1]={NULL,NULL}");
        assert_ne!(Some(empty), sql_null);
    }
    assert_eq!(
        PgArray::new(23, -1, vec![1], vec![1], vec![None])
            .unwrap()
            .to_text(b',')
            .unwrap(),
        "{NULL}"
    );
}

#[test]
fn pg05_array_text_output_delimiter_and_recovery() {
    let integers = PgArray::new(
        23,
        -1,
        vec![2],
        vec![1],
        vec![Some(Int4(-1)), Some(Int4(2))],
    )
    .unwrap();
    let output = integers.to_text(b'-').unwrap();
    assert_eq!(output, r#"{"-1"-2}"#);
    let restored = PgArray::from_text_with(output.as_bytes(), 23, -1, b'-', |s, _| {
        s.parse::<i32>().map(Int4)
    })
    .unwrap();
    assert_eq!(restored, integers);
    let a = text_array(br#"{{a,b;"x;y"};{NULL;"NULL"}}"#, b';').unwrap();
    let output = a.to_text(b';').unwrap();
    assert_eq!(output, r#"{{a,b;"x;y"};{NULL;"NULL"}}"#);
    assert_eq!(text_array(output.as_bytes(), b';').unwrap(), a);
    for delimiter in [0, b' ', b'"', b'{', b'}', b'\\', 0xff] {
        let error = a.to_text(delimiter).unwrap_err();
        assert_eq!(error, Error::UnsupportedElement);
        assert_eq!(error.sqlstate(), Some("0A000"));
        assert_eq!(a.to_text(b';').unwrap(), output);
        assert_eq!(
            text_array(b"{}", b',')
                .unwrap()
                .to_text(delimiter)
                .unwrap_err(),
            error
        );
    }
}

// These network-order fixtures are also sent to the PostgreSQL 18.6 reference.
fn binary_array_words(words: &[i32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_be_bytes()).collect()
}

fn binary_int4(bytes: &[u8]) -> Result<PgArray, crate::pg_array::PgArrayBinaryError<&'static str>> {
    PgArray::from_binary_with(bytes, 23, 42, |bytes, typmod| {
        assert_eq!(typmod, 42);
        if bytes.len() < 4 {
            return Err("08P01");
        }
        let (word, rest) = bytes.split_at(4);
        *bytes = rest;
        Ok(Int4(i32::from_be_bytes(word.try_into().unwrap())))
    })
}

#[test]
fn pg05_array_binary_input_shape_identity_nulls_and_empty() {
    let payload = binary_array_words(&[2, 1, 23, 2, 0, 2, -2, 4, i32::MIN, -1, 4, 2, 4, i32::MAX]);
    let a = binary_int4(&payload).unwrap();
    assert_eq!(a.shape(), &[2, 2]);
    assert_eq!(a.lower_bounds(), &[0, -2]);
    assert_eq!(a.element_oid(), 23);
    assert_eq!(a.element_typmod(), 42);
    assert_eq!(
        a.elements(),
        &[
            Some(Int4(i32::MIN)),
            None,
            Some(Int4(2)),
            Some(Int4(i32::MAX))
        ]
    );
    assert_eq!(
        a.to_text(b',').unwrap(),
        "[0:1][-2:-1]={{-2147483648,NULL},{2,2147483647}}"
    );
    for flags in [0, 1] {
        let a = binary_int4(&binary_array_words(&[1, flags, 23, 1, i32::MIN, -1])).unwrap();
        assert_eq!(a.elements(), &[None]);
        assert_eq!(a.lower_bounds(), &[i32::MIN]);
        assert!(a.has_nulls());
        let a = binary_int4(&binary_array_words(&[1, flags, 23, 1, 1, 4, 9])).unwrap();
        assert!(!a.has_nulls());
    }
    for words in [
        vec![0, 0, 23],
        vec![2, 1, 23, 2, -2, 0, 9],
        vec![0, 0, 10000],
        vec![0, 0, -1],
    ] {
        let empty = binary_int4(&binary_array_words(&words)).unwrap();
        assert!(empty.shape().is_empty());
        assert!(empty.lower_bounds().is_empty());
        assert!(empty.elements().is_empty());
        assert_eq!(empty.element_oid(), 23);
        assert_eq!(empty.element_typmod(), 42);
        let sql_null: Option<PgArray> = None;
        assert_ne!(Some(empty), sql_null);
    }
    let mut words = vec![6, 0, 23];
    for _ in 0..6 {
        words.extend([1, -2]);
    }
    words.extend([4, 8]);
    let a = binary_int4(&binary_array_words(&words)).unwrap();
    assert_eq!(a.shape(), &[1; 6]);
    assert_eq!(a.lower_bounds(), &[-2; 6]);
    assert_eq!(a.elements(), &[Some(Int4(8))]);
}

#[test]
fn pg05_array_binary_input_header_order_and_layout_limits() {
    use crate::pg_array::PgArrayBinaryError as B;
    let cases = [
        (vec![-1], B::InvalidDimensions, "22P03"),
        (vec![7], B::Array(Error::TooManyDimensions), "54000"),
        (vec![0, -1], B::InvalidFlags, "22P03"),
        (vec![0, 2], B::InvalidFlags, "22P03"),
        (vec![0, 0, 25], B::ElementTypeMismatch, "42804"),
        (vec![0, 0, 9999], B::ElementTypeMismatch, "42804"),
        (vec![1, 0, 23, 134217727, 1], B::Truncated, "08P01"),
        (
            vec![1, 0, 23, -1, 1],
            B::Array(Error::SizeOverflow),
            "54000",
        ),
        (
            vec![2, 0, 23, 50000, 1, 50000, 1],
            B::Array(Error::SizeOverflow),
            "54000",
        ),
        (
            vec![1, 0, 23, 134217728, 1],
            B::Array(Error::SizeOverflow),
            "54000",
        ),
        (
            vec![1, 0, 23, 1, i32::MAX],
            B::Array(Error::BoundsOverflow),
            "54000",
        ),
        (
            vec![2, 0, 23, 0, 1, -1, 1],
            B::Array(Error::SizeOverflow),
            "54000",
        ),
        (
            vec![2, 0, 23, 0, 1, 1, i32::MAX],
            B::Array(Error::BoundsOverflow),
            "54000",
        ),
    ];
    for (words, error, state) in cases {
        let actual = binary_int4(&binary_array_words(&words)).unwrap_err();
        assert_eq!(actual, error, "{words:?}");
        assert_eq!(actual.sqlstate(), Some(state));
    }
    let full = binary_array_words(&[1, 0, 23, 1, 1]);
    for n in 0..full.len() {
        let err = binary_int4(&full[..n]).unwrap_err();
        assert_eq!(err, B::Truncated, "{n}");
        assert_eq!(err.sqlstate(), Some("08P01"));
    }
}

#[test]
fn pg05_array_binary_input_element_lengths_consumption_and_recovery() {
    use crate::pg_array::PgArrayBinaryError as B;
    let header = binary_array_words(&[1, 0, 23, 1, 1]);
    for n in 0..4 {
        let mut bytes = header.clone();
        bytes.extend(&4i32.to_be_bytes()[..n]);
        assert_eq!(binary_int4(&bytes), Err(B::Truncated));
    }
    for length in [-2, i32::MIN, 1, 4, i32::MAX] {
        let mut bytes = header.clone();
        bytes.extend(length.to_be_bytes());
        assert_eq!(binary_int4(&bytes), Err(B::InvalidLength));
    }
    for n in 0..4 {
        let mut bytes = header.clone();
        bytes.extend((n as i32).to_be_bytes());
        bytes.extend(vec![0; n]);
        assert_eq!(binary_int4(&bytes), Err(B::Element("08P01")));
    }
    let mut bytes = header;
    bytes.extend(5i32.to_be_bytes());
    bytes.extend([0; 5]);
    assert_eq!(binary_int4(&bytes), Err(B::UnconsumedElement));
    for words in [vec![0, 0, 23], vec![1, 0, 23, 1, 1, 4, 7]] {
        let mut bytes = binary_array_words(&words);
        bytes.push(0);
        let error = binary_int4(&bytes).unwrap_err();
        assert_eq!(error, B::TrailingData);
        assert_eq!(error.sqlstate(), Some("22P03"));
    }
    // Scalar failure takes precedence over a later truncated element; no later callback runs.
    let mut calls = 0;
    let err =
        PgArray::from_binary_with(&binary_array_words(&[1, 0, 23, 2, 1, 0]), 23, -1, |_, _| {
            calls += 1;
            Err("22003")
        })
        .unwrap_err();
    assert_eq!(calls, 1);
    assert_eq!(err, B::Element("22003"));
    assert_eq!(err.sqlstate(), None);
    assert_eq!(B::<()>::InvalidLength.sqlstate(), Some("22P03"));
    assert_eq!(B::<()>::UnconsumedElement.sqlstate(), Some("22P03"));
    assert_eq!(
        binary_int4(&binary_array_words(&[1, 0, 23, 1, 1, 4, 7]))
            .unwrap()
            .elements(),
        &[Some(Int4(7))]
    );
}

#[test]
fn pg05_array_binary_input_text_delegation_and_unopened_families() {
    use crate::pg_array::{PgArrayBinaryError as B, PgArrayElement::Text};
    let mut payload = binary_array_words(&[1, 0, 25, 3, -2, 0, -1, 3]);
    payload.extend("中".as_bytes());
    let mut calls = 0;
    let a = PgArray::from_binary_with(&payload, 25, 17, |bytes, typmod| {
        assert_eq!(typmod, 17);
        calls += 1;
        let s = std::str::from_utf8(bytes).map_err(|_| "22021")?.to_owned();
        *bytes = &[];
        Ok::<_, &'static str>(Text(s))
    })
    .unwrap();
    assert_eq!(calls, 2);
    assert_eq!(a.element_oid(), 25);
    assert_eq!(a.element_typmod(), 17);
    assert_eq!(a.lower_bounds(), &[-2]);
    assert_eq!(
        a.elements(),
        &[Some(Text(String::new())), None, Some(Text("中".into()))]
    );
    for (bytes, expected) in [
        (
            binary_array_words(&[0, 0, 2950]),
            B::Array(Error::UnsupportedElement),
        ),
        (binary_array_words(&[1, 2]), B::InvalidFlags),
        (binary_array_words(&[0, 0, 23]), B::ElementTypeMismatch),
    ] {
        let err = PgArray::from_binary_with(&bytes, 2950, -1, |_, _| -> Result<_, ()> {
            panic!("unopened codec")
        })
        .unwrap_err();
        assert_eq!(err, expected);
    }
    for (words, expected) in [
        (vec![1, 0, 2950, 1, 1], B::Truncated),
        (vec![1, 0, 2950, 1, 1, -2], B::InvalidLength),
        (vec![1, 0, 2950, 1, 1, 4, 9, 0], B::TrailingData),
        (
            vec![1, 0, 2950, 1, 1, -1],
            B::Array(Error::UnsupportedElement),
        ),
        (
            vec![1, 0, 2950, 1, 1, 0],
            B::Array(Error::UnsupportedElement),
        ),
    ] {
        let err = PgArray::from_binary_with(
            &binary_array_words(&words),
            2950,
            -1,
            |_, _| -> Result<_, ()> { panic!("unopened codec") },
        )
        .unwrap_err();
        assert_eq!(err, expected);
    }
    assert_eq!(
        B::<()>::Array(Error::UnsupportedElement).sqlstate(),
        Some("0A000")
    );
    let err =
        PgArray::from_binary_with(&binary_array_words(&[1, 0, 23, 1, 1, 0]), 23, -1, |_, _| {
            Ok::<_, ()>(Text("wrong".into()))
        })
        .unwrap_err();
    assert_eq!(err, B::Array(Error::ElementTypeMismatch));
}

#[test]
fn pg05_array_binary_output_fixed_headers_shape_nulls_and_roundtrip() {
    for (text, words) in [
        ("{}", vec![0, 0, 23]),
        ("{1,NULL}", vec![1, 1, 23, 2, 1, 4, 1, -1]),
        ("{NULL}", vec![1, 1, 23, 1, 1, -1]),
        ("{NULL,NULL}", vec![1, 1, 23, 2, 1, -1, -1]),
        ("{1,2}", vec![1, 0, 23, 2, 1, 4, 1, 4, 2]),
        (
            "[0:1][-2:-1]={{-2147483648,NULL},{2,2147483647}}",
            vec![2, 1, 23, 2, 0, 2, -2, 4, i32::MIN, -1, 4, 2, 4, i32::MAX],
        ),
        (
            "[-2147483648:-2147483648]={1}",
            vec![1, 0, 23, 1, i32::MIN, 4, 1],
        ),
        (
            "[2147483646:2147483646]={1}",
            vec![1, 0, 23, 1, i32::MAX - 1, 4, 1],
        ),
    ] {
        let a = PgArray::from_text_with(text.as_bytes(), 23, 42, b',', |s, _| {
            s.parse::<i32>().map(Int4)
        })
        .unwrap();
        let bytes = a.to_binary().unwrap();
        assert_eq!(bytes, binary_array_words(&words), "{text}");
        let restored = binary_int4(&bytes).unwrap();
        assert_eq!(restored, a);
        assert_eq!(restored.to_text(b',').unwrap(), text);
        assert_eq!(a.element_oid(), 23);
        assert_eq!(a.element_typmod(), 42);
    }
    // Input flags do not determine output flags: actual NULL elements do.
    for (words, expected_flags) in [
        (vec![1, 0, 23, 1, 1, -1], 1),
        (vec![1, 1, 23, 1, 1, 4, 9], 0),
    ] {
        let output = binary_int4(&binary_array_words(&words))
            .unwrap()
            .to_binary()
            .unwrap();
        assert_eq!(&output[4..8], &i32::to_be_bytes(expected_flags));
    }
}

#[test]
fn pg05_array_binary_output_text_bytes_lengths_and_no_alignment() {
    use crate::pg_array::PgArrayElement::Text;
    let text = r#"[0:6]={"NULL",NULL,"","a,b","a\\b","a\"b",中}"#;
    let a = text_array(text.as_bytes(), b',').unwrap();
    let mut expected = binary_array_words(&[1, 1, 25, 7, 0, 4]);
    expected.extend(b"NULL");
    expected.extend(binary_array_words(&[-1, 0, 3]));
    expected.extend(b"a,b");
    expected.extend(3i32.to_be_bytes());
    expected.extend(b"a\\b");
    expected.extend(3i32.to_be_bytes());
    expected.extend(b"a\"b");
    expected.extend(3i32.to_be_bytes());
    expected.extend("中".as_bytes());
    let output = a.to_binary().unwrap();
    assert_eq!(output, expected);
    let restored = PgArray::from_binary_with(&output, 25, -1, |bytes, _| {
        let value = std::str::from_utf8(bytes)?.to_owned();
        *bytes = &[];
        Ok::<_, std::str::Utf8Error>(Text(value))
    })
    .unwrap();
    assert_eq!(restored, a);
    assert_eq!(restored.to_text(b',').unwrap(), text);
}

#[test]
fn pg05_array_binary_output_empty_six_dimensions_and_rejected_values() {
    use crate::pg_array::PgArrayElement::Text;
    for oid in [23, 25] {
        let empty = PgArray::new(oid, 17, vec![2, 0], vec![-2, 9], vec![]).unwrap();
        assert_eq!(
            empty.to_binary().unwrap(),
            binary_array_words(&[0, 0, oid as i32])
        );
        assert_eq!(empty.element_typmod(), 17);
        let sql_null: Option<PgArray> = None;
        assert_ne!(Some(empty), sql_null);
    }
    let a = PgArray::new(23, 42, vec![1; 6], vec![-2; 6], vec![Some(Int4(8))]).unwrap();
    let mut words = vec![6, 0, 23];
    for _ in 0..6 {
        words.extend([1, -2]);
    }
    words.extend([4, 8]);
    assert_eq!(a.to_binary().unwrap(), binary_array_words(&words));
    assert_eq!(binary_int4(&a.to_binary().unwrap()).unwrap(), a);
    // Invalid values cannot reach output through the immutable carrier.
    for oid in [1083, 1007, 2950] {
        let err = PgArray::new(oid, -1, vec![], vec![], vec![]).unwrap_err();
        assert_eq!(err, Error::UnsupportedElement);
        assert_eq!(err.sqlstate(), Some("0A000"));
    }
    assert_eq!(
        PgArray::new(23, -1, vec![1], vec![1], vec![Some(Text("x".into()))]),
        Err(Error::ElementTypeMismatch)
    );
    assert_eq!(
        PgArray::new(25, -1, vec![1], vec![1], vec![Some(Text("x\0y".into()))]),
        Err(Error::InvalidEncoding)
    );
    assert_eq!(
        PgArray::new(23, -1, vec![2], vec![1], vec![None]),
        Err(Error::ElementCountMismatch)
    );
    assert_eq!(a.to_binary().unwrap(), binary_array_words(&words));
}

#[test]
fn pg05_array_cast_int8_carrier_null_and_empty() {
    let value = PgArray::new(20, -1, vec![1], vec![-2], vec![None]).unwrap();
    assert_eq!(value.element_oid(), 20);
    assert_eq!(value.to_text(b',').unwrap(), "[-2:-2]={NULL}");
    assert_eq!(
        value.to_binary().unwrap(),
        [1i32, 1, 20, 1, -2, -1]
            .into_iter()
            .flat_map(i32::to_be_bytes)
            .collect::<Vec<_>>()
    );
}

#[test]
fn pg05_array_cast_int8_exact_width_and_identity() {
    use crate::pg_array::PgArrayElement::Int8;
    let value = PgArray::new(
        20,
        -1,
        vec![2, 2],
        vec![0, -2],
        vec![
            Some(Int8(i64::MIN)),
            None,
            Some(Int8(2147483648)),
            Some(Int8(i64::MAX)),
        ],
    )
    .unwrap();
    assert_eq!(
        value.to_text(b',').unwrap(),
        "[0:1][-2:-1]={{-9223372036854775808,NULL},{2147483648,9223372036854775807}}"
    );
    let mut expected = [2i32, 1, 20, 2, 0, 2, -2, 8]
        .into_iter()
        .flat_map(i32::to_be_bytes)
        .collect::<Vec<_>>();
    expected.extend_from_slice(&i64::MIN.to_be_bytes());
    expected.extend_from_slice(&(-1i32).to_be_bytes());
    for n in [2147483648, i64::MAX] {
        expected.extend_from_slice(&8i32.to_be_bytes());
        expected.extend_from_slice(&n.to_be_bytes());
    }
    assert_eq!(value.to_binary().unwrap(), expected);
    assert_eq!(
        PgArray::new(23, -1, vec![1], vec![1], vec![Some(Int8(7))]),
        Err(Error::ElementTypeMismatch)
    );
    assert_eq!(
        PgArray::new(20, -1, vec![1], vec![1], vec![Some(Int4(7))]),
        Err(Error::ElementTypeMismatch)
    );
}

#[test]
fn pg05_array_bool_canonical_identity_shape_null_and_bytes() {
    use crate::pg_array::PgArrayElement::Bool;
    let array = PgArray::new(
        16,
        -1,
        vec![2, 2],
        vec![0, -2],
        vec![Some(Bool(true)), None, Some(Bool(false)), Some(Bool(true))],
    )
    .unwrap();
    assert_eq!(
        array.to_text(b',').unwrap(),
        "[0:1][-2:-1]={{t,NULL},{f,t}}"
    );
    let mut binary: Vec<u8> = [2i32, 1, 16, 2, 0, 2, -2, 1]
        .into_iter()
        .flat_map(i32::to_be_bytes)
        .collect();
    binary.push(1);
    binary.extend_from_slice(&(-1i32).to_be_bytes());
    for byte in [0, 1] {
        binary.extend_from_slice(&1i32.to_be_bytes());
        binary.push(byte);
    }
    assert_eq!(array.to_binary().unwrap(), binary);
    let decoded = PgArray::from_binary_with(&binary, 16, -1, |payload, _| -> Result<_, ()> {
        let (&byte, rest) = payload.split_first().ok_or(())?;
        *payload = rest;
        Ok(Bool(byte != 0))
    })
    .unwrap();
    assert_eq!(decoded, array);
    assert_eq!(
        PgArray::new(23, -1, vec![1], vec![1], vec![Some(Bool(true))]),
        Err(Error::ElementTypeMismatch)
    );
    assert_eq!(
        PgArray::new(16, -1, vec![1], vec![1], vec![Some(Int4(1))]),
        Err(Error::ElementTypeMismatch)
    );
}

#[test]
fn pg05_array_int2_canonical_shape_typmod_null_and_network_bytes() {
    use crate::pg_array::PgArrayElement::Int2;
    let array = PgArray::new(
        21,
        7,
        vec![2, 2],
        vec![0, -2],
        vec![
            Some(Int2(i16::MIN)),
            None,
            Some(Int2(0)),
            Some(Int2(i16::MAX)),
        ],
    )
    .unwrap();
    assert_eq!(array.element_typmod(), 7);
    assert_eq!(
        array.to_text(b',').unwrap(),
        "[0:1][-2:-1]={{-32768,NULL},{0,32767}}"
    );
    let mut binary: Vec<u8> = [2i32, 1, 21, 2, 0, 2, -2, 2]
        .into_iter()
        .flat_map(i32::to_be_bytes)
        .collect();
    binary.extend_from_slice(&i16::MIN.to_be_bytes());
    binary.extend_from_slice(&(-1i32).to_be_bytes());
    for value in [0i16, i16::MAX] {
        binary.extend_from_slice(&2i32.to_be_bytes());
        binary.extend_from_slice(&value.to_be_bytes());
    }
    assert_eq!(array.to_binary().unwrap(), binary);
    let decoded = PgArray::from_binary_with(&binary, 21, 7, |payload, _| -> Result<_, ()> {
        let bytes = payload.get(..2).ok_or(())?;
        let number = i16::from_be_bytes(bytes.try_into().unwrap());
        *payload = &payload[2..];
        Ok(Int2(number))
    })
    .unwrap();
    assert_eq!(decoded, array);
    assert_eq!(
        PgArray::new(21, -1, vec![1], vec![1], vec![Some(Int4(1))]),
        Err(Error::ElementTypeMismatch)
    );
    assert_eq!(
        PgArray::new(23, -1, vec![1], vec![1], vec![Some(Int2(1))]),
        Err(Error::ElementTypeMismatch)
    );
}

#[test]
fn pg05_array_int8_canonical_shape_typmod_null_and_network_bytes() {
    use crate::pg_array::PgArrayElement::Int8;
    let array = PgArray::new(
        20,
        7,
        vec![2, 2],
        vec![0, -2],
        vec![
            Some(Int8(i64::MIN)),
            None,
            Some(Int8(0)),
            Some(Int8(i64::MAX)),
        ],
    )
    .unwrap();
    assert_eq!(array.element_typmod(), 7);
    assert_eq!(
        array.to_text(b',').unwrap(),
        "[0:1][-2:-1]={{-9223372036854775808,NULL},{0,9223372036854775807}}"
    );
    let mut binary: Vec<u8> = [2i32, 1, 20, 2, 0, 2, -2, 8]
        .into_iter()
        .flat_map(i32::to_be_bytes)
        .collect();
    binary.extend_from_slice(&i64::MIN.to_be_bytes());
    binary.extend_from_slice(&(-1i32).to_be_bytes());
    for value in [0i64, i64::MAX] {
        binary.extend_from_slice(&8i32.to_be_bytes());
        binary.extend_from_slice(&value.to_be_bytes());
    }
    assert_eq!(array.to_binary().unwrap(), binary);
    let decoded = PgArray::from_binary_with(&binary, 20, 7, |payload, _| -> Result<_, ()> {
        let bytes = payload.get(..8).ok_or(())?;
        let number = i64::from_be_bytes(bytes.try_into().unwrap());
        *payload = &payload[8..];
        Ok(Int8(number))
    })
    .unwrap();
    assert_eq!(decoded, array);
    assert_eq!(
        PgArray::new(20, -1, vec![1], vec![1], vec![Some(Int4(1))]),
        Err(Error::ElementTypeMismatch)
    );
    assert_eq!(
        PgArray::new(23, -1, vec![1], vec![1], vec![Some(Int8(1))]),
        Err(Error::ElementTypeMismatch)
    );
}

#[test]
fn pg05_array_bytea_shape_typmod_null_and_raw_bytes() {
    use crate::pg_array::{PgArrayElement::Bytea, bytea_text_input};
    let values = vec![
        Some(Bytea((0..=255).collect())),
        None,
        Some(Bytea(vec![])),
        Some(Bytea(br"\x00".to_vec())),
    ];
    let array = PgArray::new(17, 7, vec![2, 2], vec![0, -2], values.clone()).unwrap();
    assert_eq!(array.element_oid(), 17);
    assert_eq!(array.element_typmod(), 7);
    assert_eq!(array.shape(), [2, 2]);
    assert_eq!(array.lower_bounds(), [0, -2]);
    assert_eq!(array.elements(), values);
    let text = array.to_text(b',').unwrap();
    assert!(text.starts_with(r#"[0:1][-2:-1]={{"\\x000102"#));
    assert!(text.ends_with(r#"ff",NULL},{"\\x","\\x5c783030"}}"#));
    let decoded = PgArray::from_text_with(text.as_bytes(), 17, 7, b',', |s, _| {
        bytea_text_input(s.as_bytes()).map(Bytea)
    })
    .unwrap();
    assert_eq!(decoded, array);
    let binary = array.to_binary().unwrap();
    let mut expected: Vec<u8> = [2i32, 1, 17, 2, 0, 2, -2, 256]
        .into_iter()
        .flat_map(i32::to_be_bytes)
        .collect();
    expected.extend(0..=255);
    expected.extend((-1i32).to_be_bytes());
    expected.extend(0i32.to_be_bytes());
    expected.extend(4i32.to_be_bytes());
    expected.extend(br"\x00");
    assert_eq!(binary, expected);
    let decoded = PgArray::from_binary_with(&binary, 17, 7, |s, _| -> Result<_, ()> {
        let value = Bytea(s.to_vec());
        *s = &[];
        Ok(value)
    })
    .unwrap();
    assert_eq!(decoded, array);
    assert_eq!(
        PgArray::new(17, -1, vec![1], vec![1], vec![Some(Int4(1))]),
        Err(Error::ElementTypeMismatch)
    );
    assert_eq!(
        PgArray::new(25, -1, vec![1], vec![1], vec![Some(Bytea(vec![0]))]),
        Err(Error::ElementTypeMismatch)
    );
    for values in [vec![], vec![None], vec![Some(Bytea(vec![]))]] {
        let a = PgArray::new(17, -1, vec![values.len() as i32], vec![-1], values).unwrap();
        assert_eq!(a.element_oid(), 17);
        assert_eq!(
            PgArray::from_text_with(a.to_text(b',').unwrap().as_bytes(), 17, -1, b',', |s, _| {
                bytea_text_input(s.as_bytes()).map(Bytea)
            })
            .unwrap(),
            a
        );
    }
}

#[test]
fn pg05_array_bytea_extracted_scalar_codec() {
    use crate::pg_array::{bytea_text_input as input, bytea_text_output as output};
    for bytes in [
        vec![],
        vec![0, 255, 195, 40, 92],
        (0..=255).collect::<Vec<u8>>(),
    ] {
        assert_eq!(input(output(&bytes).as_bytes()).unwrap(), bytes);
    }
    assert_eq!(input(br"\xDE ad").unwrap(), [222, 173]);
    assert_eq!(input(br"\000\377\\").unwrap(), [0, 255, 92]);
    for (text, state) in [
        (br"\x0".as_slice(), "22023"),
        (br"\xgg", "22023"),
        (br"\xa b", "22023"),
        (br"\400", "22P02"),
        (br"\08", "22P02"),
        (br"\q", "22P02"),
        (b"\\", "22P02"),
        (b"\0", "22021"),
        (b"\xff", "22021"),
    ] {
        assert_eq!(input(text).unwrap_err().0, state);
    }
}

#[test]
fn pg05_array_numeric_canonical_shape_typmod_null() {
    use super::pg_array::{PgArray, PgArrayElement};
    use super::pg_numeric::PgNumeric;
    let value = PgArray::from_text_with(
        b"[-2:-1]={12.340,NULL}",
        1700,
        ((5 << 16) | 3) + 4,
        b',',
        |s, _| s.parse::<PgNumeric>().map(PgArrayElement::Numeric),
    )
    .unwrap();
    assert_eq!(value.element_typmod(), ((5 << 16) | 3) + 4);
    assert_eq!(value.shape(), [2]);
    assert_eq!(value.lower_bounds(), [-2]);
    assert_eq!(value.to_text(b',').unwrap(), "[-2:-1]={12.340,NULL}");
    let result = PgArray::from_binary_with(
        &value.to_binary().unwrap(),
        1700,
        value.element_typmod(),
        |b, _| {
            let value = PgNumeric::from_binary(b)?;
            *b = &[];
            Ok::<_, super::pg_numeric::PgNumericError>(PgArrayElement::Numeric(value))
        },
    )
    .unwrap();
    assert_eq!(result, value);
}

#[test]
fn pg05_array_float4_raw_bits_shape_typmod_and_null() {
    use crate::pg_array::{PgArray, PgArrayElement, float4_binary_input};
    let bits = [
        0, 0x80000000, 1, 0x007fffff, 0x00800000, 0x7f7fffff, 0x7f800000, 0xff800000, 0x7fc00000,
        0x7fc12345, 0xffc12345, 0x7fa12345, 0xffa12345,
    ];
    let values = bits
        .into_iter()
        .map(|b| Some(PgArrayElement::Float4(b)))
        .chain([None])
        .collect();
    let array = PgArray::new(700, -1, vec![14], vec![-7], values).unwrap();
    assert_eq!(array.element_typmod(), -1);
    let wire = array.to_binary().unwrap();
    assert_eq!(PgArray::float4_from_binary(&wire).unwrap(), array);
    for (i, bits) in bits.into_iter().enumerate() {
        assert_eq!(
            float4_binary_input(&wire[24 + i * 8..28 + i * 8]).unwrap(),
            bits
        );
    }
    assert!(
        array
            .to_text(b',')
            .unwrap()
            .starts_with("[-7:6]={0,-0,1e-45,")
    );
    assert_eq!(float4_binary_input(&[0; 3]).unwrap_err().0, "08P01");
    assert_eq!(float4_binary_input(&[0; 5]).unwrap_err().0, "22P03");
}

#[test]
fn pg05_array_float8_raw_bits_shape_typmod_and_null() {
    use crate::pg_array::{PgArray, PgArrayElement, float8_binary_input};
    let bits = [
        0,
        0x8000000000000000,
        1,
        0x000fffffffffffff,
        0x0010000000000000,
        0x7fefffffffffffff,
        0x7ff0000000000000,
        0xfff0000000000000,
        0x7ff8000000000000,
        0x7ff812345678abcd,
        0xfff812345678abcd,
        0x7ff012345678abcd,
        0xfff012345678abcd,
    ];
    let values = bits
        .into_iter()
        .map(|b| Some(PgArrayElement::Float8(b)))
        .chain([None])
        .collect();
    let array = PgArray::new(701, -1, vec![14], vec![-7], values).unwrap();
    assert_eq!(array.element_typmod(), -1);
    let wire = array.to_binary().unwrap();
    assert_eq!(PgArray::float8_from_binary(&wire).unwrap(), array);
    for (i, bits) in bits.into_iter().enumerate() {
        assert_eq!(
            float8_binary_input(&wire[24 + i * 12..32 + i * 12]).unwrap(),
            bits
        );
    }
    assert!(
        array
            .to_text(b',')
            .unwrap()
            .starts_with("[-7:6]={0,-0,5e-324,")
    );
    assert_eq!(float8_binary_input(&[0; 3]).unwrap_err().0, "08P01");
    assert_eq!(float8_binary_input(&[0; 9]).unwrap_err().0, "22P03");
}

#[test]
fn pg05_array_varchar_canonical_shape_typmod_null_and_binary() {
    use crate::pg_array::{PgArray, PgArrayElement, PgArrayError};
    let value = PgArray::new(
        1043,
        6,
        vec![2, 2],
        vec![0, -2],
        vec![
            Some(PgArrayElement::Text("é😀".into())),
            None,
            Some(PgArrayElement::Text("".into())),
            Some(PgArrayElement::Text("NULL".into())),
        ],
    )
    .unwrap();
    assert_eq!(value.element_oid(), 1043);
    assert_eq!(value.element_typmod(), 6);
    assert_eq!(
        value.to_text(b',').unwrap(),
        "[0:1][-2:-1]={{é😀,NULL},{\"\",\"NULL\"}}"
    );
    let binary = value.to_binary().unwrap();
    let decoded =
        PgArray::from_binary_with(&binary, 1043, 6, |bytes, _| -> Result<PgArrayElement, ()> {
            let value = std::str::from_utf8(bytes).map_err(|_| ())?.to_owned();
            *bytes = &[];
            Ok(PgArrayElement::Text(value))
        })
        .unwrap();
    assert_eq!(value, decoded);
    assert_eq!(
        PgArray::new(
            1043,
            -1,
            vec![1],
            vec![1],
            vec![Some(PgArrayElement::Text("a\0b".into()))]
        ),
        Err(PgArrayError::InvalidEncoding)
    );
}

#[test]
fn pg05_array_bpchar_canonical_shape_typmod_null_and_binary() {
    use crate::pg_array::{PgArray, PgArrayElement, PgArrayError};
    let value = PgArray::new(
        1042,
        6,
        vec![2, 2],
        vec![0, -2],
        vec![
            Some(PgArrayElement::Text("é😀".into())),
            None,
            Some(PgArrayElement::Text("".into())),
            Some(PgArrayElement::Text("NULL".into())),
        ],
    )
    .unwrap();
    assert_eq!(value.element_oid(), 1042);
    assert_eq!(value.element_typmod(), 6);
    assert_eq!(
        value.to_text(b',').unwrap(),
        "[0:1][-2:-1]={{é😀,NULL},{\"\",\"NULL\"}}"
    );
    let binary = value.to_binary().unwrap();
    let decoded =
        PgArray::from_binary_with(&binary, 1042, 6, |bytes, _| -> Result<PgArrayElement, ()> {
            let value = std::str::from_utf8(bytes).map_err(|_| ())?.to_owned();
            *bytes = &[];
            Ok(PgArrayElement::Text(value))
        })
        .unwrap();
    assert_eq!(value, decoded);
    assert_eq!(
        PgArray::new(
            1042,
            -1,
            vec![1],
            vec![1],
            vec![Some(PgArrayElement::Text("a\0b".into()))]
        ),
        Err(PgArrayError::InvalidEncoding)
    );
}

#[test]
fn pg05_array_date_canonical_shape_null_and_binary() {
    use crate::pg_array::{PgArray, PgArrayElement, PgArrayError};
    use crate::pg_temporal::PgDate;
    let date: PgDate = "4714-11-24 BC".parse().unwrap();
    let value = PgArray::new(
        1082,
        -1,
        vec![2, 2],
        vec![0, -2],
        vec![
            Some(PgArrayElement::Date(date)),
            None,
            Some(PgArrayElement::Date("infinity".parse().unwrap())),
            Some(PgArrayElement::Date("5874897-12-31".parse().unwrap())),
        ],
    )
    .unwrap();
    assert_eq!(
        value.to_text(b',').unwrap(),
        "[0:1][-2:-1]={{\"4714-11-24 BC\",NULL},{infinity,5874897-12-31}}"
    );
    let bytes = value.to_binary().unwrap();
    let decoded = PgArray::from_binary_with(&bytes, 1082, -1, |bytes, _| {
        let date = PgDate::from_binary(bytes)?;
        *bytes = &[];
        Ok::<_, crate::pg_temporal::PgTemporalError>(PgArrayElement::Date(date))
    })
    .unwrap();
    assert_eq!(decoded, value);
    assert_eq!(
        PgArray::new(
            1082,
            -1,
            vec![1],
            vec![1],
            vec![Some(PgArrayElement::Text("2000-01-01".into()))]
        ),
        Err(PgArrayError::ElementTypeMismatch)
    );
}

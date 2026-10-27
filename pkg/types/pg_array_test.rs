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
    for oid in [0, 16, 1007, 3802] {
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

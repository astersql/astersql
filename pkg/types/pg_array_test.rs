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
    for oid in [0, 25, 1007, 3802, u32::MAX] {
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

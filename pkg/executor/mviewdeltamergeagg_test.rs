// Copyright 2026 AsterSQL.

use crate::mviewdeltamergeagg::*;
use astersql_types::datum::{Datum, NewIntDatum, NewUintDatum};
fn count(out: usize, delta: usize) -> Mapping {
    Mapping {
        kind: AggKind::Count,
        output_col: out,
        delta_col: delta,
        removed_col: None,
        count_dependency: None,
        nullable: false,
        unsigned: false,
    }
}
struct Recompute(i64);
impl MinMaxRecompute for Recompute {
    fn recompute(&self, _: usize, _: usize) -> Result<Option<Datum>> {
        Ok(Some(NewIntDatum(self.0)))
    }
}
#[test]
fn mview_complete_delta_count_and_sum_merge() {
    let mut rows = vec![vec![
        NewIntDatum(2),
        NewIntDatum(10),
        NewIntDatum(3),
        NewIntDatum(7),
    ]];
    let maps = vec![
        count(2, 0),
        Mapping {
            kind: AggKind::Sum,
            output_col: 3,
            delta_col: 1,
            removed_col: None,
            count_dependency: None,
            nullable: false,
            unsigned: false,
        },
    ];
    let touched = merge_rows(&mut rows, &maps, 2, None).unwrap();
    assert_eq!(rows[0][2].GetInt64(), 5);
    assert_eq!(rows[0][3].GetInt64(), 17);
    assert!(touched[0][2] && touched[0][3]);
}
#[test]
fn mview_complete_delta_unsigned_sum_accepts_signed_delta() {
    let mut rows = vec![vec![NewIntDatum(-2), NewUintDatum(5)]];
    let m = Mapping {
        kind: AggKind::Sum,
        output_col: 1,
        delta_col: 0,
        removed_col: None,
        count_dependency: None,
        nullable: false,
        unsigned: false,
    };
    merge_rows(&mut rows, &[m], 1, None).unwrap();
    assert_eq!(rows[0][1].GetUint64(), 3);
}
#[test]
fn mview_complete_delta_rejects_unsigned_count() {
    let mut m = count(1, 0);
    m.unsigned = true;
    assert!(validate(&[m], 1).unwrap_err().contains("COUNT output"));
}
#[test]
fn mview_complete_delta_rejects_computed_delta_dependency() {
    let m = count(2, 1);
    assert!(validate(&[m], 1).unwrap_err().contains("computed side"));
}
#[test]
fn mview_complete_delta_minmax_fast_path_and_recompute() {
    let mapping = Mapping {
        kind: AggKind::Max,
        output_col: 2,
        delta_col: 0,
        removed_col: Some(1),
        count_dependency: None,
        nullable: false,
        unsigned: false,
    };
    let mut rows = vec![
        vec![NewIntDatum(12), Datum::default(), NewIntDatum(10)],
        vec![NewIntDatum(8), NewIntDatum(12), NewIntDatum(12)],
    ];
    merge_rows(&mut rows, &[mapping], 2, Some(&Recompute(7))).unwrap();
    assert_eq!(rows[0][2].GetInt64(), 12);
    assert_eq!(rows[1][2].GetInt64(), 7);
}
#[test]
fn mview_complete_delta_minmax_requires_recompute_executor() {
    let mapping = Mapping {
        kind: AggKind::Min,
        output_col: 2,
        delta_col: 0,
        removed_col: Some(1),
        count_dependency: None,
        nullable: false,
        unsigned: false,
    };
    let mut rows = vec![vec![Datum::default(), NewIntDatum(3), NewIntDatum(3)]];
    assert!(
        merge_rows(&mut rows, &[mapping], 2, None)
            .unwrap_err()
            .contains("recompute executor")
    );
}
#[test]
fn mview_complete_delta_noop_when_value_unchanged() {
    let mut rows = vec![vec![NewIntDatum(0), NewIntDatum(4)]];
    let touched = merge_rows(&mut rows, &[count(1, 0)], 1, None).unwrap();
    assert!(!touched[0][1]);
}

#[test]
fn mview_delta_merge_count_null_old_is_zero_and_negative_rejected() {
    let mut rows = vec![vec![NewIntDatum(2), Datum::default()]];
    merge_rows(&mut rows, &[count(1, 0)], 1, None).unwrap();
    assert_eq!(rows[0][1].GetInt64(), 2);
    let mut rows = vec![vec![NewIntDatum(-3), NewIntDatum(2)]];
    assert!(merge_rows(&mut rows, &[count(1, 0)], 1, None).is_err());
}
#[test]
fn mview_delta_merge_nullable_sum_resets_when_count_zero() {
    let mut rows = vec![vec![NewIntDatum(-3), NewIntDatum(0), NewIntDatum(3)]];
    let mut mapping = count(2, 0);
    mapping.kind = AggKind::Sum;
    mapping.nullable = true;
    mapping.count_dependency = Some(1);
    // Computed COUNT is on the materialized-view side.
    merge_rows(&mut rows, &[mapping], 1, None).unwrap();
    assert!(rows[0][2].IsNull());
}

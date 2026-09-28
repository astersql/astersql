// Copyright 2026 AsterSQL.

use crate::func_count_distinct::{CountDistinctReal, update_distinct_real};

/// Go's map[float64] treats both IEEE zero encodings as the same key.
#[test]
fn distinct_real_treats_signed_zero_as_one_value_like_go() {
    let mut state = CountDistinctReal::default();

    update_distinct_real(&mut state, [Some(0.0), Some(-0.0)]);

    assert_eq!(state.count(), 1);
}

/// Go float map keys deliberately do not deduplicate NaN because NaN != NaN.
#[test]
fn distinct_real_keeps_each_nan_like_go() {
    let mut state = CountDistinctReal::default();
    let nan = f64::NAN;

    update_distinct_real(&mut state, [Some(nan), Some(nan)]);

    assert_eq!(state.count(), 2);
}

// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::func_cume_dist::CumeDist;
use crate::func_max_min::{BinaryJson, DurationValue, TimeValue};
use crate::func_ntile::Ntile;
use crate::func_percent_rank::PercentRank;
use crate::func_rank::Rank;
use crate::func_sum::Decimal;
use crate::func_value::{FirstValue, LastValue, NthValue, ValueMemory};
use crate::row_number::RowNumber;
use std::fmt::Debug;

fn collect_rank(rows: &[i64], dense: bool) -> Vec<i64> {
    let mut rank = Rank::new(dense);
    rank.update(rows.iter().copied());
    std::iter::from_fn(|| rank.next()).collect()
}

fn collect_cume_dist(rows: &[i64]) -> Vec<f64> {
    let mut function = CumeDist::default();
    function.update(rows.iter().copied());
    std::iter::from_fn(|| function.next()).collect()
}

fn collect_percent_rank(rows: &[i64]) -> Vec<f64> {
    let mut function = PercentRank::default();
    function.update(rows.iter().copied());
    std::iter::from_fn(|| function.next()).collect()
}

fn collect_ntile(n: u64, row_count: u64) -> Vec<u64> {
    let mut function = Ntile::new(Some(n));
    function.update(row_count);
    (0..row_count)
        .map(|_| function.next_value().expect("valid NTILE argument"))
        .collect()
}

fn assert_first_value<T>(first: T, second: T)
where
    T: Clone + Debug + PartialEq + ValueMemory,
{
    let mut function = FirstValue::default();
    function.update(&[Some(first.clone())]);
    function.update(&[Some(second)]);
    assert_eq!(function.result(), Some(Some(&first)));
    assert_eq!(function.result(), Some(Some(&first)));
}

/// Mirrors every case in Go `TestWindowFunctions` against executable Rust
/// window-function state machines instead of checking translated string labels.
#[test]
fn test_window_functions() {
    // CUME_DIST: no ORDER BY makes every row a peer; ordered generated rows are unique.
    assert_eq!(collect_cume_dist(&[0]), vec![1.0]);
    assert_eq!(collect_cume_dist(&[0, 0]), vec![1.0, 1.0]);
    assert_eq!(collect_cume_dist(&[0, 1, 2, 3]), vec![0.25, 0.5, 0.75, 1.0]);

    // DENSE_RANK.
    assert_eq!(collect_rank(&[0, 0], true), vec![1, 1]);
    assert_eq!(collect_rank(&[0, 1, 2, 3], true), vec![1, 2, 3, 4]);

    // FIRST_VALUE's eight Go field-type specializations.
    assert_first_value(0_i64, 1_i64);
    assert_first_value(0_f32, 1_f32);
    assert_first_value(0_f64, 1_f64);
    assert_first_value(Decimal::new(0, 0), Decimal::new(1, 0));
    assert_first_value("0".to_owned(), "1".to_owned());
    assert_first_value(
        TimeValue {
            packed: 365,
            ..Default::default()
        },
        TimeValue {
            packed: 366,
            ..Default::default()
        },
    );
    assert_first_value(DurationValue::default(), DurationValue { nanos: 1, fsp: 0 });
    assert_first_value(
        BinaryJson {
            type_code: 0x09,
            value: 0_i64.to_le_bytes().to_vec(),
        },
        BinaryJson {
            type_code: 0x09,
            value: 1_i64.to_le_bytes().to_vec(),
        },
    );

    // LAST_VALUE and NTH_VALUE consume the generated 0,1,... input rows.
    let mut last = LastValue::default();
    last.update(&[Some(0_i64)]);
    last.update(&[Some(1_i64)]);
    assert_eq!(last.result(), Some(Some(&1)));
    assert_eq!(last.result(), Some(Some(&1)));

    let mut second = NthValue::new(2);
    second.update(&[Some(0_i64), Some(1_i64), Some(2_i64)]);
    assert_eq!(second.result(), Some(Some(&1)));
    assert_eq!(second.result(), Some(Some(&1)));
    assert_eq!(second.result(), Some(Some(&1)));

    let mut fifth = NthValue::new(5);
    fifth.update(&[Some(0_i64), Some(1_i64), Some(2_i64)]);
    assert_eq!(fifth.result(), None);
    assert_eq!(fifth.result(), None);
    assert_eq!(fifth.result(), None);

    assert_eq!(collect_ntile(3, 4), vec![1, 1, 2, 3]);
    assert_eq!(collect_ntile(5, 3), vec![1, 2, 3]);

    // PERCENT_RANK.
    assert_eq!(collect_percent_rank(&[0]), vec![0.0]);
    assert_eq!(collect_percent_rank(&[0, 0, 0]), vec![0.0, 0.0, 0.0]);
    assert_eq!(
        collect_percent_rank(&[0, 1, 2, 3]),
        vec![0.0, 1.0 / 3.0, 2.0 / 3.0, 1.0]
    );

    // RANK.
    assert_eq!(collect_rank(&[0], false), vec![1]);
    assert_eq!(collect_rank(&[0, 0, 0], false), vec![1, 1, 1]);
    assert_eq!(collect_rank(&[0, 1, 2, 3], false), vec![1, 2, 3, 4]);

    let mut row_number = RowNumber::default();
    assert_eq!(
        (0..4).map(|_| row_number.next_value()).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
}

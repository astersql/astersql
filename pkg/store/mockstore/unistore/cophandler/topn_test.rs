// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

use crate::cop_handler::{ByItem, Datum, Expr};
use crate::topn::{SortRow, TopNHeap, TopNSorter};
use std::cmp::Ordering;

fn by_item(descending: bool, enum_unsigned: bool) -> ByItem {
    ByItem {
        expr: Expr::Column(0),
        descending,
        enum_unsigned,
    }
}

fn sort_row(value: Datum) -> SortRow {
    SortRow {
        key: vec![value.clone()],
        data: vec![value],
    }
}

#[test]
fn enum_keys_use_go_unsigned_comparison() {
    let signed = TopNSorter::new(vec![by_item(false, false)]);
    let enum_sorter = TopNSorter::new(vec![by_item(false, true)]);
    let negative = sort_row(Datum::Int(-1));
    let one = sort_row(Datum::Uint(1));

    assert_eq!(signed.compare(&negative, &one), Ordering::Less);
    assert_eq!(enum_sorter.compare(&negative, &one), Ordering::Greater);
}

#[test]
fn bounded_heap_matches_go_for_limits_directions_and_ties() {
    let mut ascending = TopNHeap::new(2, vec![by_item(false, false)]);
    for value in [3, 1, 2, 1] {
        ascending.add_data_row(vec![Datum::Int(value)]).unwrap();
    }
    let rows = ascending.into_sorted_rows().unwrap();
    assert_eq!(rows, vec![vec![Datum::Int(1)], vec![Datum::Int(1)]]);

    let mut descending = TopNHeap::new(2, vec![by_item(true, false)]);
    for value in [1, 3, 2] {
        descending.add_data_row(vec![Datum::Int(value)]).unwrap();
    }
    let rows = descending.into_sorted_rows().unwrap();
    assert_eq!(rows, vec![vec![Datum::Int(3)], vec![Datum::Int(2)]]);

    let mut zero = TopNHeap::new(0, vec![by_item(false, false)]);
    assert!(!zero.add_data_row(vec![Datum::Int(1)]).unwrap());
    assert!(zero.into_sorted_rows().unwrap().is_empty());
}

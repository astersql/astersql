// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

use crate::{KeyRange, KeyRanges, NewKeyRanges};

fn range(start: &str, end: &str) -> KeyRange {
    KeyRange {
        start: start.as_bytes().to_vec(),
        end: end.as_bytes().to_vec(),
    }
}

fn ranges(items: &[(&str, &str)]) -> KeyRanges {
    KeyRanges::new(
        items
            .iter()
            .map(|&(start, end)| range(start, end))
            .collect(),
    )
}

fn assert_ranges(actual: &KeyRanges, expected: &[(&str, &str)]) {
    assert_eq!(actual, &ranges(expected));
    assert_eq!(actual.len(), expected.len());
    for (index, &(start, end)) in expected.iter().enumerate() {
        assert_eq!(actual.ref_at(index), Some(&range(start, end)));
        assert_eq!(actual.at(index), Some(range(start, end)));
    }
}

#[test]
fn key_ranges_access_slice_and_iteration_match_go() {
    let all = NewKeyRanges(vec![range("a", "b"), range("c", "d"), range("e", "f")]);
    assert_eq!(all.ref_at(all.len()), None);
    assert_eq!(all.at(all.len()), None);

    let mut visited = Vec::new();
    all.for_each(|item| visited.push(item.clone()));
    assert_eq!(visited, all.to_ranges());
    for from in 0..=all.len() {
        for to in from..=all.len() {
            assert_eq!(all.slice(from, to).to_ranges(), all.to_ranges()[from..to]);
        }
    }
}

#[test]
fn key_ranges_split_matches_all_go_cases() {
    type SplitCase<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [(&'a str, &'a str)]);
    let cases: &[(&[(&str, &str)], &[SplitCase<'_>])] = &[
        (
            &[("c", "d"), ("e", "g"), ("l", "o")],
            &[
                ("c", &[], &[("c", "d"), ("e", "g"), ("l", "o")]),
                ("d", &[("c", "d")], &[("e", "g"), ("l", "o")]),
                ("f", &[("c", "d"), ("e", "f")], &[("f", "g"), ("l", "o")]),
                ("m", &[("c", "d"), ("e", "g"), ("l", "m")], &[("m", "o")]),
                ("g", &[("c", "d"), ("e", "g")], &[("l", "o")]),
            ],
        ),
        (
            &[("a", "b"), ("c", "d"), ("e", "g"), ("l", "o")],
            &[
                ("a", &[], &[("a", "b"), ("c", "d"), ("e", "g"), ("l", "o")]),
                ("c", &[("a", "b")], &[("c", "d"), ("e", "g"), ("l", "o")]),
                (
                    "m",
                    &[("a", "b"), ("c", "d"), ("e", "g"), ("l", "m")],
                    &[("m", "o")],
                ),
                ("d", &[("a", "b"), ("c", "d")], &[("e", "g"), ("l", "o")]),
                ("o", &[("a", "b"), ("c", "d"), ("e", "g"), ("l", "o")], &[]),
            ],
        ),
        (
            &[("a", "b"), ("c", "d"), ("e", "g"), ("l", "o"), ("q", "t")],
            &[
                (
                    "f",
                    &[("a", "b"), ("c", "d"), ("e", "f")],
                    &[("f", "g"), ("l", "o"), ("q", "t")],
                ),
                (
                    "h",
                    &[("a", "b"), ("c", "d"), ("e", "g")],
                    &[("l", "o"), ("q", "t")],
                ),
                (
                    "r",
                    &[("a", "b"), ("c", "d"), ("e", "g"), ("l", "o"), ("q", "r")],
                    &[("r", "t")],
                ),
                (
                    "p",
                    &[("a", "b"), ("c", "d"), ("e", "g"), ("l", "o")],
                    &[("q", "t")],
                ),
                (
                    "t",
                    &[("a", "b"), ("c", "d"), ("e", "g"), ("l", "o"), ("q", "t")],
                    &[],
                ),
            ],
        ),
    ];

    for (input, splits) in cases {
        let input = ranges(input);
        for &(key, left_expected, right_expected) in *splits {
            let (left, right) = input.split(key.as_bytes());
            assert_ranges(&left, left_expected);
            assert_ranges(&right, right_expected);
        }
    }
}

#[test]
fn key_ranges_unbounded_end_reset_and_pb_conversion_match_go() {
    let unbounded = ranges(&[("a", "")]);
    let (left, right) = unbounded.split(b"m");
    assert_ranges(&left, &[("a", "m")]);
    assert_ranges(&right, &[("m", "")]);

    let mut value = ranges(&[("a", "b")]);
    value.reset(vec![range("c", "d"), range("e", "f")]);
    assert_ranges(&value, &[("c", "d"), ("e", "f")]);
    assert_eq!(value.to_pb_ranges(), value.to_ranges());
}

#[test]
fn key_ranges_string_matches_go_quoted_bytes() {
    let value = KeyRanges::new(vec![
        KeyRange {
            start: b"a\n".to_vec(),
            end: b"b\t".to_vec(),
        },
        KeyRange {
            start: vec![0xff],
            end: "中".as_bytes().to_vec(),
        },
    ]);
    assert_eq!(value.to_string(), "[\"a\\n\", \"b\\t\"][\"\\xff\", \"中\"]");
}

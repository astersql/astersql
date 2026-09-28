// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use crate::find_best_task::{AccessPath, Datum, IndexInfo, Range};
use crate::indexmerge_path::IndexMergeDataSource;
use crate::indexmerge_unfinished_path::{
    buildIntoAccessPath, cmpAlternatives, genUnfinishedPathFromORList, initUnfinishedPathsFromExpr,
    mergeANDItemIntoUnfinishedIndexMergePath, unfinishedAccessPath,
};
use crate::task::Expression;
use std::cmp::Ordering;

fn expression(name: &str, column: usize) -> Expression {
    Expression {
        name: name.into(),
        column: Some(column),
        ..Expression::default()
    }
}

fn index_path(columns: Vec<usize>, unique: bool, multi_valued: bool, rows: f64) -> AccessPath {
    AccessPath {
        index: Some(IndexInfo {
            columns,
            prefix_lengths: Vec::new(),
            unique,
            global: false,
            multi_valued,
            vector: false,
        }),
        count_after_access: rows,
        ..AccessPath::default()
    }
}

#[test]
fn top_level_and_uses_the_same_collection_path_as_go() {
    let candidate = index_path(vec![0, 1], false, false, 100.0);
    let mut ds = IndexMergeDataSource::default();
    ds.source.paths.push(candidate.clone());
    let branch = expression("eq:1", 0);
    let and_item = expression("eq:2", 1);
    let unfinished = genUnfinishedPathFromORList(
        &ds,
        &[branch.clone(), branch.clone()],
        std::slice::from_ref(&candidate),
    )
    .expect("OR branches should initialize");

    let from_and = initUnfinishedPathsFromExpr(&ds, std::slice::from_ref(&candidate), &and_item)
        .expect("AND item should initialize");
    assert!(from_and[0].as_ref().unwrap().initedWithValidRange);

    let merged = mergeANDItemIntoUnfinishedIndexMergePath(Some(unfinished), Some(from_and))
        .expect("Go merges usable filters even into an already valid OR partial path");
    assert!(merged.orBranches.iter().all(|branch| {
        branch[0]
            .as_ref()
            .unwrap()
            .usableFilters
            .iter()
            .any(|condition| condition.name == and_item.name)
    }));
}

#[test]
fn non_mv_or_requires_more_than_one_physical_index() {
    let candidate = index_path(vec![0], false, false, 100.0);
    let mut ds = IndexMergeDataSource::default();
    ds.source.paths.push(candidate.clone());
    let branches = [expression("eq:1", 0), expression("eq:2", 0)];
    let unfinished =
        genUnfinishedPathFromORList(&ds, &branches, std::slice::from_ref(&candidate)).unwrap();
    assert!(buildIntoAccessPath(&ds, unfinished, &branches, 0).is_none());
}

#[test]
fn unique_point_ranges_beat_lower_row_count_scans() {
    let point = AccessPath {
        index: index_path(vec![0], true, false, 0.0).index,
        ranges: vec![Range {
            low: vec![Datum::Int(1)],
            high: vec![Datum::Int(1)],
            ..Range::default()
        }],
        count_after_access: 10.0,
        ..AccessPath::default()
    };
    let scan = AccessPath {
        index: index_path(vec![1], false, false, 0.0).index,
        ranges: vec![Range {
            low: vec![Datum::Int(1)],
            high: vec![Datum::Int(9)],
            ..Range::default()
        }],
        count_after_access: 1.0,
        ..AccessPath::default()
    };
    assert_eq!(
        cmpAlternatives(100.0)(&vec![point], &vec![scan]),
        Ordering::Less
    );
}

#[test]
fn final_path_keeps_top_level_and_filters_but_not_covered_or() {
    let left = index_path(vec![0], false, false, 100.0);
    let right = index_path(vec![1], false, false, 100.0);
    let mut ds = IndexMergeDataSource::default();
    ds.source.paths = vec![left.clone(), right.clone()];
    let or = expression("or:eq:1|eq:2", 0);
    let top_level = expression("eq:9", 2);
    let branches = [expression("eq:1", 0), expression("eq:2", 1)];
    let unfinished = unfinishedAccessPath {
        orBranches: vec![
            vec![
                Some(unfinishedAccessPath {
                    path: Some(left),
                    usableFilters: vec![branches[0].clone()],
                    initedWithValidRange: true,
                    ..unfinishedAccessPath::default()
                }),
                None,
            ],
            vec![
                None,
                Some(unfinishedAccessPath {
                    path: Some(right),
                    usableFilters: vec![branches[1].clone()],
                    initedWithValidRange: true,
                    ..unfinishedAccessPath::default()
                }),
            ],
        ],
        ..unfinishedAccessPath::default()
    };
    let built = buildIntoAccessPath(&ds, unfinished, &[or, top_level.clone()], 0).unwrap();
    assert_eq!(built.table_filters.len(), 1);
    assert_eq!(built.table_filters[0].name, top_level.name);
}

#[test]
fn mv_or_alternative_expands_each_json_value() {
    let candidate = index_path(vec![0], false, true, 100.0);
    let mut ds = IndexMergeDataSource::default();
    ds.source.paths.push(candidate.clone());
    let branches = [
        expression("json-overlaps:[1,2]", 0),
        expression("member-of:3", 0),
    ];
    let unfinished =
        genUnfinishedPathFromORList(&ds, &branches, std::slice::from_ref(&candidate)).unwrap();
    let built = buildIntoAccessPath(&ds, unfinished, &branches, 0).unwrap();
    assert_eq!(built.partial_index_paths.len(), 3);
}

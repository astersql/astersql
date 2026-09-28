// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

use crate::find_best_task::{AccessPath, IndexInfo};
use crate::indexmerge_path::{
    IndexMergeDataSource, IndexMergeHint, SINGLE_VALUE_MV_TP, checkAccessFilter4IdxCol,
    cleanAccessPathForFTS, cleanAccessPathForMVIndexHint, indexMergeContainSpecificIndex,
    isSafeTypeConversion4MVIndexRange,
};
use crate::task::{Expression, FieldType, StoreType, TypeCode};
use std::collections::HashSet;

fn field_type(code: TypeCode, unsigned: bool) -> FieldType {
    FieldType {
        code,
        flen: 0,
        decimal: 0,
        unsigned,
    }
}

fn index(columns: Vec<usize>, multi_valued: bool) -> IndexInfo {
    IndexInfo {
        columns,
        prefix_lengths: Vec::new(),
        unique: false,
        global: false,
        multi_valued,
        vector: false,
    }
}

fn index_path(column: usize, multi_valued: bool) -> AccessPath {
    AccessPath {
        index: Some(index(vec![column], multi_valued)),
        ..AccessPath::default()
    }
}

#[test]
fn singleton_and_empty_mv_arrays_match_go_filter_classification() {
    let singleton = Expression {
        name: "json-overlaps:[7]".into(),
        column: Some(1),
        ..Expression::default()
    };
    assert_eq!(
        checkAccessFilter4IdxCol(&singleton, Some(1)),
        (true, SINGLE_VALUE_MV_TP)
    );

    let empty = Expression {
        name: "json-contains:[]".into(),
        column: Some(1),
        ..Expression::default()
    };
    assert_eq!(checkAccessFilter4IdxCol(&empty, Some(1)), (false, 0));
}

#[test]
fn mv_range_conversion_uses_go_eval_type_families() {
    assert!(isSafeTypeConversion4MVIndexRange(
        &field_type(TypeCode::Int, false),
        &field_type(TypeCode::UInt, true),
    ));
    assert!(isSafeTypeConversion4MVIndexRange(
        &field_type(TypeCode::String, false),
        &field_type(TypeCode::Bytes, false),
    ));
    assert!(!isSafeTypeConversion4MVIndexRange(
        &field_type(TypeCode::Float, false),
        &field_type(TypeCode::Decimal, false),
    ));
}

#[test]
fn specific_index_search_descends_nested_index_merge_paths() {
    let nested = AccessPath {
        partial_index_paths: vec![AccessPath {
            partial_index_paths: vec![index_path(9, true)],
            ..AccessPath::default()
        }],
        ..AccessPath::default()
    };
    assert!(indexMergeContainSpecificIndex(&nested, &HashSet::from([9])));
}

#[test]
fn mv_hint_cleanup_prunes_only_after_finding_a_valid_merge_path() {
    let normal = index_path(1, false);
    let invalid_merge = AccessPath {
        partial_index_paths: vec![index_path(2, true)],
        ..AccessPath::default()
    };
    let mut no_match = IndexMergeDataSource {
        index_merge_hints: vec![IndexMergeHint { indexes: vec![9] }],
        ..IndexMergeDataSource::default()
    };
    no_match.source.paths = vec![normal.clone(), invalid_merge.clone()];
    cleanAccessPathForMVIndexHint(&mut no_match);
    assert_eq!(no_match.source.paths.len(), 2);

    let valid_merge = AccessPath {
        partial_index_paths: vec![index_path(9, true)],
        ..AccessPath::default()
    };
    let mut matched = IndexMergeDataSource {
        index_merge_hints: vec![IndexMergeHint { indexes: vec![9] }],
        ..IndexMergeDataSource::default()
    };
    matched.source.paths = vec![normal, invalid_merge, valid_merge];
    cleanAccessPathForMVIndexHint(&mut matched);
    assert_eq!(matched.source.paths.len(), 1);
    assert!(indexMergeContainSpecificIndex(
        &matched.source.paths[0],
        &HashSet::from([9])
    ));
}

#[test]
fn fts_cleanup_keeps_only_tiflash_and_errors_when_unavailable() {
    let mut ds = IndexMergeDataSource {
        fts_indexes: HashSet::from([7]),
        ..IndexMergeDataSource::default()
    };
    ds.source.paths = vec![
        AccessPath {
            store: Some(StoreType::TiKv),
            ..AccessPath::default()
        },
        AccessPath {
            store: Some(StoreType::TiFlash),
            ..AccessPath::default()
        },
    ];
    cleanAccessPathForFTS(&mut ds).expect("TiFlash path must satisfy FTS");
    assert_eq!(ds.source.paths.len(), 1);
    assert_eq!(ds.source.paths[0].store, Some(StoreType::TiFlash));

    ds.source.paths = vec![AccessPath {
        store: Some(StoreType::TiKv),
        ..AccessPath::default()
    }];
    assert!(cleanAccessPathForFTS(&mut ds).is_err());
}

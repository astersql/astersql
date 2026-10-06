// Copyright 2026 AsterSQL.

use super::find_best_task::{
    AccessPath, DataSource, Datum, IndexInfo, Range, getIndexCandidateForIndexJoin,
};
use super::index_join_path::{
    appendTailTemplateRange, indexJoinPathCmp4UnComparableOnes,
    indexJoinPathCountAfterAccess4Compare, indexJoinPathGetRangeInfoAndMaxOneRow,
    indexJoinPathInfo, indexJoinPathResult, isNDVClose,
};
use super::task::Expression;

#[test]
fn ndv_closeness_matches_go_thresholds() {
    assert!(isNDVClose(1.0, 2.0));
    assert!(isNDVClose(20.0, 40.0));
    assert!(isNDVClose(1_000.0, 1_249.0));
    assert!(!isNDVClose(19.0, 40.0));
    assert!(!isNDVClose(1_000.0, 2_000.0));
    assert!(!isNDVClose(0.0, 1.0));
}

#[test]
fn tail_template_fallback_preserves_the_original_ranges() {
    let original = vec![Range {
        low: vec![Datum::Int(1)],
        high: vec![Datum::Int(1)],
        ..Range::default()
    }];

    let (ranges, fallback) = appendTailTemplateRange(original.clone(), 1);

    assert!(fallback);
    assert_eq!(ranges.len(), original.len());
    assert_eq!(ranges[0].low, original[0].low);
    assert_eq!(ranges[0].high, original[0].high);
}

fn path_result(ndv: f64, used_columns: usize) -> indexJoinPathResult {
    let path = AccessPath::default();
    indexJoinPathResult {
        candidate: getIndexCandidateForIndexJoin(&DataSource::default(), &path, 1, 0.0, false),
        chosenPath: path,
        chosenAccess: Vec::new(),
        chosenRemained: Vec::new(),
        chosenRanges: vec![Range::default()],
        usedColsLen: used_columns,
        eqUsedColsNDV: ndv,
        lastColIsRange: false,
        idxOff2KeyOff: vec![0],
        lastColManager: None,
        mutableRange: None,
    }
}

#[test]
fn incomparable_paths_prioritize_non_close_ndv_before_used_column_count() {
    let best = path_result(100.0, 3);
    let current = path_result(500.0, 1);

    assert!(indexJoinPathCmp4UnComparableOnes(&best, &current));
}

#[test]
fn max_one_row_matches_go_unique_index_contract() {
    let mut result = path_result(10.0, 1);
    result.chosenPath.index = Some(IndexInfo {
        columns: vec![7],
        prefix_lengths: vec![None],
        unique: true,
        global: false,
        multi_valued: false,
        vector: false,
    });
    result.chosenRanges = vec![Range {
        low: vec![Datum::Null],
        high: vec![Datum::Null],
        ..Range::default()
    }];
    let info = indexJoinPathInfo::default();

    assert!(indexJoinPathGetRangeInfoAndMaxOneRow(&info, &result).1);

    result.usedColsLen = 2;
    assert!(!indexJoinPathGetRangeInfoAndMaxOneRow(&info, &result).1);

    result.usedColsLen = 1;
    result.chosenAccess = vec![Expression {
        name: "in:[1,2]".into(),
        ..Expression::default()
    }];
    assert!(!indexJoinPathGetRangeInfoAndMaxOneRow(&info, &result).1);
}

#[test]
fn tail_range_marks_non_equality_and_excludes_its_ndv() {
    use super::index_join_path::{IndexJoinContext, indexJoinPathBuild};
    let path = AccessPath {
        index_columns: vec![7, 8],
        ..Default::default()
    };
    for dynamic in [false, true] {
        let mut info = indexJoinPathInfo {
            innerJoinKeys: vec![7, 9],
            outerJoinKeys: vec![1, 2],
            innerSchema: vec![7, 8, 9],
            columnNDV: [(7, 2.0), (8, 1000.0)].into_iter().collect(),
            ..Default::default()
        };
        let range = Expression {
            name: "gt:10".into(),
            column: Some(8),
            ..Default::default()
        };
        if dynamic {
            info.joinOtherConditions.push(range);
        } else {
            info.innerPushedConditions.push(range);
        }
        let (result, empty) =
            indexJoinPathBuild(&IndexJoinContext::default(), &path, &info, false).unwrap();
        let result = result.expect("join prefix with appended tail range");
        assert!(!empty);
        assert_eq!(result.usedColsLen, 2);
        assert!(result.lastColIsRange);
        assert_eq!(result.eqUsedColsNDV, 2.0);
        assert_eq!(result.lastColManager.is_some(), dynamic);
    }
}

/// A single full-length runtime join key divides CountAfterAccess by its stable NDV.
#[test]
fn index_join_compare_count_uses_single_join_key_ndv() {
    let path = AccessPath {
        index: Some(IndexInfo {
            columns: vec![7, 8],
            prefix_lengths: vec![None, None],
            unique: false,
            global: false,
            multi_valued: false,
            vector: false,
        }),
        count_after_access: 30_300.0,
        ..AccessPath::default()
    };
    let info = indexJoinPathInfo {
        innerTableStats: Some(Default::default()),
        columnNDV: [(7, 303.0)].into_iter().collect(),
        ..Default::default()
    };

    assert_eq!(
        indexJoinPathCountAfterAccess4Compare(&info, &path, &[0, -1], 1),
        (100.0, true)
    );
}

/// Prefix indexes and multiple runtime join keys are not stable enough for strong pruning.
#[test]
fn index_join_compare_count_rejects_unstable_key_shapes() {
    let mut path = AccessPath {
        index: Some(IndexInfo {
            columns: vec![7, 8],
            prefix_lengths: vec![Some(4), None],
            unique: false,
            global: false,
            multi_valued: false,
            vector: false,
        }),
        count_after_access: 30_300.0,
        ..AccessPath::default()
    };
    let info = indexJoinPathInfo {
        innerTableStats: Some(Default::default()),
        columnNDV: [(7, 303.0), (8, 101.0)].into_iter().collect(),
        ..Default::default()
    };
    assert_eq!(
        indexJoinPathCountAfterAccess4Compare(&info, &path, &[0, -1], 1),
        (30_300.0, false)
    );

    path.index.as_mut().unwrap().prefix_lengths[0] = None;
    assert_eq!(
        indexJoinPathCountAfterAccess4Compare(&info, &path, &[0, 1], 2),
        (30_300.0, false)
    );
}

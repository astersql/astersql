// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Index Join 查找路径（lookup path）与 range fallback 单元测试。
//
// 对齐 Go：分析 Index Join 内表查找过滤条件如何映射到索引列、生成 lookup range，
// 以及当 range 过大时逐步丢弃列/谓词的 fallback 行为。

use super::exhaust_physical_plans::{LogicalJoin, getHashJoins};
use super::find_best_task::{AccessPath, Datum, IndexInfo, PhysicalProperty};
use super::index_join_path::{
    IndexJoinContext, indexJoinPathBuild, indexJoinPathInfo, indexJoinPathResult,
};
use super::task::{Expression, JoinType};

#[test]
/// Semi Join 强制左侧 Build 时只保留 useOuterToBuild 候选，与 Go 枚举一致。
fn semi_join_force_left_build_excludes_the_right_build_candidate() {
    let mut join = LogicalJoin {
        join_type: JoinType::Semi,
        hash_join_v2: true,
        ..LogicalJoin::default()
    };
    join.hints.force_left_build = true;

    let (plans, forced) = getHashJoins(&mut join, &PhysicalProperty::default());

    assert!(forced);
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].labels.get("use_outer_to_build"), Some(&1.0));
}

#[test]
/// Hash Join v1 不支持 Semi Join Build/Probe Hint，应告警并退回固定右 Build。
fn semi_join_hash_v1_ignores_build_side_hint_like_go() {
    let mut join = LogicalJoin {
        join_type: JoinType::Semi,
        hash_join_v2: false,
        ..LogicalJoin::default()
    };
    join.hints.force_left_build = true;

    let (plans, forced) = getHashJoins(&mut join, &PhysicalProperty::default());

    assert!(!forced);
    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].labels.get("use_outer_to_build"), Some(&0.0));
    assert_eq!(join.warnings.len(), 1);
}

/// 构造带列引用的简单表达式夹具。
fn expression(name: &str, column: usize) -> Expression {
    Expression {
        name: name.to_owned(),
        column: Some(column),
        ..Default::default()
    }
}

/// 构造带前缀长度信息的索引 AccessPath。
fn access_path(columns: &[usize], prefix_lengths: &[Option<usize>]) -> AccessPath {
    AccessPath {
        index: Some(IndexInfo {
            columns: columns.to_vec(),
            prefix_lengths: prefix_lengths.to_vec(),
            unique: false,
            global: false,
            multi_valued: false,
            vector: false,
        }),
        index_columns: columns.to_vec(),
        ..Default::default()
    }
}

/// 调用 indexJoinPathBuild，返回查找路径结果与是否为空。
fn build_path(
    path: &AccessPath,
    inner_keys: &[usize],
    pushed: Vec<Expression>,
    other: Vec<Expression>,
    range_max_size: usize,
    rebuild_mode: bool,
) -> (Option<indexJoinPathResult>, bool) {
    let mut context = IndexJoinContext::default();
    context.rangeMaxSize = range_max_size;
    let info = indexJoinPathInfo {
        innerJoinKeys: inner_keys.to_vec(),
        outerJoinKeys: (0..inner_keys.len()).map(|offset| offset + 100).collect(),
        innerSchema: path.index_columns.clone(),
        innerPushedConditions: pushed,
        joinOtherConditions: other,
        ..Default::default()
    };
    indexJoinPathBuild(&context, path, &info, rebuild_mode)
        .expect("indexJoinPathBuild should accept the Go lookup-filter fixtures")
}

/// 提取表达式名称列表，便于断言。
fn names(expressions: &[Expression]) -> Vec<&str> {
    expressions.iter().map(|expr| expr.name.as_str()).collect()
}

/// 取出各 chosen range 的 low 边界，便于与 Go 夹具对比。
fn range_lows(result: &indexJoinPathResult) -> Vec<Vec<Datum>> {
    result
        .chosenRanges
        .iter()
        .map(|range| range.low.clone())
        .collect()
}

/// 字符串转 Bytes Datum。
fn bytes(value: &str) -> Datum {
    Datum::Bytes(value.as_bytes().to_vec())
}

/// Go 测试共用的五列索引路径（含两列前缀长度 2）。
fn go_index_path() -> AccessPath {
    access_path(&[0, 1, 2, 3, 4], &[None, None, Some(2), None, Some(2)])
}

#[test]
/// 分析 lookup 过滤：连续/非连续 join key、IN、相关谓词与前缀列映射。
fn test_index_join_analyze_lookup_filters() {
    let path = access_path(&[0, 1, 2, 3, 4], &[None, None, Some(2), None, Some(2)]);

    // Join keys separated by an unmatched index column are not continuous.
    // As in Go, only the leading key can participate in the lookup range.
    let (result, empty) = build_path(&path, &[0, 2], vec![], vec![], 0, false);
    assert!(!empty);
    let result = result.expect("a leading join key should form a lookup range");
    assert_eq!(vec![0, -1, -1, -1, -1], result.idxOff2KeyOff);
    assert_eq!(
        vec![vec![Datum::Null, Datum::Null]],
        range_lows(&result),
        "Go retains the unmatched second index column in the open lookup range"
    );

    // A pushed equality on the first column still cannot bridge a gap before
    // the only join key on the third index column.
    let (result, empty) = build_path(&path, &[2], vec![expression("eq:[1]", 0)], vec![], 0, false);
    assert!(!empty);
    assert!(result.is_none());

    // An equality condition can bridge the leading gap to a following join key.
    let (result, empty) = build_path(&path, &[1], vec![expression("eq:[1]", 0)], vec![], 0, false);
    assert!(!empty);
    let result = result.expect("eq plus join key should form a lookup range");
    assert_eq!(vec![-1, 0, -1, -1, -1], result.idxOff2KeyOff);
    assert_eq!(vec!["eq:[1]"], names(&result.chosenAccess));
    assert_eq!(vec![vec![Datum::Int(1), Datum::Null]], range_lows(&result));

    // Correlated inequalities on the next index column become a dynamic tail
    // manager and are not silently discarded.
    let (result, empty) = build_path(
        &path,
        &[1],
        vec![expression("eq:[1]", 0), expression("gt:[97]", 2)],
        vec![
            expression("gt:outer-column-102", 2),
            expression("lt:concat-outer-column-102", 2),
        ],
        0,
        false,
    );
    assert!(!empty);
    let result = result.expect("correlated lookup filters should form a path");
    let manager = result
        .lastColManager
        .expect("correlated inequalities should install a last-column manager");
    assert_eq!(2, manager.targetColumn);
    assert_eq!(
        vec!["gt:outer-column-102", "lt:concat-outer-column-102"],
        names(&manager.conditions)
    );

    // Prefix-index equality remains an access condition, while a later
    // condition must remain for post-range evaluation.
    let (result, empty) = build_path(
        &path,
        &[1],
        vec![
            expression("eq:[1]", 0),
            expression("in:[97,98,99]", 2),
            expression("eq:[7]", 3),
        ],
        vec![],
        0,
        false,
    );
    assert!(!empty);
    let result = result.expect("prefix-index access should form a lookup path");
    assert_eq!(vec!["eq:[1]", "in:[97,98,99]"], names(&result.chosenAccess));
    assert_eq!(
        vec!["in:[97,98,99]", "eq:[7]"],
        names(&result.chosenRemained)
    );

    // A constant false predicate is the empty-range branch, not a missing path.
    let (result, empty) = build_path(&path, &[1], vec![expression("false", 0)], vec![], 0, false);
    assert!(empty);
    assert!(result.is_none());
}

#[test]
/// 不可下推的 cast 相关过滤应从可下推相关过滤中排除。
fn index_join_correlated_filters_exclude_non_pushdown_casts() {
    let (result, empty) = build_path(
        &go_index_path(),
        &[1],
        vec![expression("eq:[1]", 0)],
        vec![
            expression("gt:outer-column-102", 2),
            expression("lt:cast-plus-outer-column-102", 2),
        ],
        0,
        false,
    );
    assert!(!empty);
    let result = result.expect("continuous a/key/c path");
    assert_eq!(
        names(&result.chosenAccess),
        vec!["eq:[1]", "gt:outer-column-102"]
    );
    assert_eq!(
        names(
            &result
                .lastColManager
                .expect("one pushed comparison")
                .conditions
        ),
        vec!["gt:outer-column-102"]
    );
}

#[test]
/// 前缀 range 截断后仍保留原谓词供回表重检。
fn index_join_prefix_range_keeps_original_predicates_for_recheck() {
    let (result, empty) = build_path(
        &go_index_path(),
        &[1],
        vec![
            expression("eq:[1]", 0),
            expression("gt:[a]", 2),
            expression("lt:[aaaaaa]", 2),
        ],
        vec![],
        0,
        false,
    );
    assert!(!empty);
    let result = result.expect("prefix range");
    assert_eq!(
        names(&result.chosenAccess),
        vec!["eq:[1]", "gt:[a]", "lt:[aaaaaa]"]
    );
    assert_eq!(names(&result.chosenRemained), vec!["gt:[a]", "lt:[aaaaaa]"]);
    assert_eq!(result.chosenRanges.len(), 1);
    assert_eq!(
        result.chosenRanges[0].low,
        vec![Datum::Int(1), Datum::Null, bytes("a")]
    );
    assert_eq!(
        result.chosenRanges[0].high,
        vec![Datum::Int(1), Datum::Null, bytes("aa")]
    );
    assert!(result.chosenRanges[0].low_exclusive);
    assert!(!result.chosenRanges[0].high_exclusive);
}

#[test]
/// 三列 join key 后接 ASCII 前缀列时的 idxOff2KeyOff 映射对齐 Go。
fn index_join_ascii_prefix_after_three_join_keys_has_go_mapping() {
    let (result, empty) = build_path(
        &go_index_path(),
        &[1, 2, 3],
        vec![
            expression("eq:[1]", 0),
            expression("gt:[a]", 4),
            expression("lt:[aaaaaa]", 4),
        ],
        vec![],
        0,
        false,
    );
    assert!(!empty);
    let result = result.expect("four continuous columns and tail prefix");
    assert_eq!(result.idxOff2KeyOff, vec![-1, 0, 1, 2, -1]);
    assert_eq!(result.chosenRanges[0].low.len(), 5);
    assert_eq!(result.chosenRanges[0].low[4], bytes("a"));
    assert_eq!(result.chosenRanges[0].high[4], bytes("aa"));
}

#[test]
/// 两个 IN 谓词应笛卡尔展开为全部 range 组合。
fn index_join_two_in_predicates_generate_all_nine_ranges() {
    let (result, empty) = build_path(
        &go_index_path(),
        &[1],
        vec![expression("in:[1,2,3]", 0), expression("in:[a,b,c]", 2)],
        vec![],
        0,
        false,
    );
    assert!(!empty);
    let result = result.expect("IN cross product");
    assert_eq!(result.chosenRanges.len(), 9);
    assert_eq!(result.idxOff2KeyOff, vec![-1, 0, -1, -1, -1]);
    assert_eq!(
        result.chosenRanges[0].low,
        vec![Datum::Int(1), Datum::Null, bytes("a")]
    );
    assert_eq!(
        result.chosenRanges[8].low,
        vec![Datum::Int(3), Datum::Null, bytes("c")]
    );
    assert_eq!(names(&result.chosenRemained), vec!["in:[a,b,c]"]);
}

#[test]
/// IN range 可继续延伸到相关谓词构成的尾部列。
fn index_join_in_ranges_extend_to_correlated_tail() {
    let (result, empty) = build_path(
        &go_index_path(),
        &[1],
        vec![expression("in:[1,2,3]", 0), expression("in:[a,b,c]", 2)],
        vec![
            expression("gt:outer-column-103", 3),
            expression("lt:plus-outer-column-103", 3),
        ],
        0,
        false,
    );
    assert!(!empty);
    let result = result.expect("IN ranges plus correlated tail");
    assert_eq!(result.chosenRanges.len(), 9);
    assert!(result.chosenRanges.iter().all(|range| range.low.len() == 4));
    assert_eq!(
        names(&result.lastColManager.expect("tail manager").conditions),
        vec!["gt:outer-column-103", "lt:plus-outer-column-103"]
    );
}

#[test]
/// 非等值桥接谓词只打开下一索引列，不跨越更远列。
fn index_join_non_equality_bridges_only_the_next_open_column() {
    let (result, empty) = build_path(
        &go_index_path(),
        &[0, 2],
        vec![expression("gt:[1]", 1)],
        vec![],
        0,
        false,
    );
    assert!(!empty);
    let result = result.expect("leading key plus b inequality");
    assert_eq!(result.idxOff2KeyOff, vec![0, -1, -1, -1, -1]);
    assert_eq!(names(&result.chosenAccess), vec!["gt:[1]"]);
    assert_eq!(result.chosenRanges[0].low, vec![Datum::Null, Datum::Int(1)]);
    assert!(result.chosenRanges[0].low_exclusive);
}

#[test]
/// 多字节前缀按字符数截断，而非字节数。
fn index_join_multibyte_prefix_truncates_by_characters() {
    let (result, empty) = build_path(
        &go_index_path(),
        &[1],
        vec![
            expression("eq:[1]", 0),
            expression("gt:[a]", 2),
            expression("lt:[一二三]", 2),
        ],
        vec![],
        0,
        false,
    );
    assert!(!empty);
    let result = result.expect("multibyte prefix range");
    assert_eq!(result.chosenRanges[0].high[2], bytes("一二"));
    assert_eq!(names(&result.chosenRemained), vec!["gt:[a]", "lt:[一二三]"]);
}

#[test]
/// range 超限时 fallback：逐步缩短 lookup 列并保留可下推过滤。
fn test_range_fallback_for_analyze_lookup_filters() {
    let path = access_path(&[0, 1, 2, 3], &[None, None, None, None]);
    let pushed = vec![expression("in:[1,3]", 1), expression("in:[2,4]", 3)];

    let (result, empty) = build_path(&path, &[0, 2], pushed.clone(), vec![], 0, false);
    assert!(!empty);
    let result = result.expect("unlimited range memory should keep the cartesian ranges");
    assert_eq!(
        vec![
            vec![Datum::Null, Datum::Int(1), Datum::Null, Datum::Int(2)],
            vec![Datum::Null, Datum::Int(1), Datum::Null, Datum::Int(4)],
            vec![Datum::Null, Datum::Int(3), Datum::Null, Datum::Int(2)],
            vec![Datum::Null, Datum::Int(3), Datum::Null, Datum::Int(4)],
        ],
        range_lows(&result)
    );
    assert_eq!(vec![0, -1, 1, -1], result.idxOff2KeyOff);

    // Four four-column point ranges require 256 bytes in the canonical range
    // estimator. One byte less makes Go progressively drop the last IN column,
    // retaining the two three-column lookup prefixes and recording a warning.
    let mut context = IndexJoinContext::default();
    context.rangeMaxSize = 255;
    let info = indexJoinPathInfo {
        innerJoinKeys: vec![0, 2],
        outerJoinKeys: vec![100, 101],
        innerSchema: path.index_columns.clone(),
        innerPushedConditions: pushed.clone(),
        ..Default::default()
    };
    let (result, empty) = indexJoinPathBuild(&context, &path, &info, false)
        .expect("range fallback should retain the Go lookup prefix");
    assert!(!empty);
    let result = result.expect("range fallback should preserve two prefix ranges");
    assert_eq!(
        vec![
            vec![Datum::Null, Datum::Int(1), Datum::Null],
            vec![Datum::Null, Datum::Int(3), Datum::Null],
        ],
        range_lows(&result)
    );
    assert_eq!(vec![0, -1, 1, -1], result.idxOff2KeyOff);
    assert_eq!(vec!["in:[1,3]"], names(&result.chosenAccess));
    assert_eq!(vec!["in:[2,4]"], names(&result.chosenRemained));
    assert!(context.HasRangeFallback());
    assert_eq!(1, context.RangeFallbackWarnings().len());
    context.ResetRangeFallback();
    assert!(!context.HasRangeFallback());
    assert!(context.RangeFallbackWarnings().is_empty());

    // Rebuild mode deliberately ignores the statement range-memory limit.
    let (result, empty) = build_path(&path, &[0, 2], pushed, vec![], 1, true);
    assert!(!empty);
    assert_eq!(
        4,
        result
            .expect("rebuild mode should retain all point ranges")
            .chosenRanges
            .len()
    );
}

#[test]
/// fallback 按优先级逐步丢弃额外 join/IN 列。
fn range_fallback_progressively_drops_extra_join_and_in_columns() {
    let path = go_index_path();
    let pushed = vec![expression("in:[1,3]", 0), expression("in:[aaa,bbb]", 2)];

    let (full, empty) = build_path(&path, &[1, 3], pushed.clone(), vec![], 0, false);
    assert!(!empty);
    let full = full.expect("unlimited build");
    assert_eq!(full.chosenRanges.len(), 4);
    assert!(full.chosenRanges.iter().all(|range| range.low.len() == 4));

    let (fallback, empty) = build_path(&path, &[1, 3], pushed, vec![], 255, false);
    assert!(!empty);
    let fallback = fallback.expect("Go retries after dropping the extra join column");
    assert_eq!(fallback.chosenRanges.len(), 4);
    assert!(
        fallback
            .chosenRanges
            .iter()
            .all(|range| range.low.len() == 3)
    );
    assert_eq!(fallback.idxOff2KeyOff, vec![-1, 0, -1, -1, -1]);

    let (fallback, empty) = build_path(
        &path,
        &[1, 3],
        vec![expression("in:[1,3]", 0), expression("in:[aaa,bbb]", 2)],
        vec![],
        191,
        false,
    );
    assert!(!empty);
    let fallback = fallback.expect("Go next drops the trailing IN range column");
    assert_eq!(fallback.chosenRanges.len(), 2);
    assert!(
        fallback
            .chosenRanges
            .iter()
            .all(|range| range.low.len() == 2)
    );
    assert_eq!(names(&fallback.chosenAccess), vec!["in:[1,3]"]);
    assert_eq!(names(&fallback.chosenRemained), vec!["in:[aaa,bbb]"]);
    assert_eq!(fallback.idxOff2KeyOff, vec![-1, 0, -1, -1, -1]);

    let (fallback, empty) = build_path(
        &path,
        &[1, 3],
        vec![expression("in:[1,3]", 0), expression("in:[aaa,bbb]", 2)],
        vec![],
        63,
        false,
    );
    assert!(!empty);
    assert!(
        fallback.is_none(),
        "after dropping the leading IN, the non-leading join key cannot form a path"
    );
}

#[test]
/// fallback 先丢相关尾部列，再丢 IN 谓词列。
fn range_fallback_drops_correlated_tail_before_in_predicate() {
    let path = go_index_path();
    let pushed = vec![expression("in:[1,3,5]", 1)];
    let other = vec![
        expression("gt:outer-column-102", 2),
        expression("lt:concat-outer-column-102", 2),
    ];
    let (full, empty) = build_path(&path, &[0], pushed.clone(), other.clone(), 0, false);
    assert!(!empty);
    let full = full.expect("unlimited build");
    assert_eq!(full.chosenRanges.len(), 3);
    assert!(full.chosenRanges.iter().all(|range| range.low.len() == 3));

    let (fallback, empty) = build_path(&path, &[0], pushed, other, 143, false);
    assert!(!empty);
    let fallback = fallback.expect("Go drops the correlated tail and keeps IN ranges");
    assert_eq!(fallback.chosenRanges.len(), 3);
    assert!(
        fallback
            .chosenRanges
            .iter()
            .all(|range| range.low.len() == 2)
    );
    assert!(fallback.lastColManager.is_none());

    let (fallback, empty) = build_path(
        &path,
        &[0],
        vec![expression("in:[1,3,5]", 1)],
        vec![
            expression("gt:outer-column-102", 2),
            expression("lt:concat-outer-column-102", 2),
        ],
        95,
        false,
    );
    assert!(!empty);
    let fallback = fallback.expect("Go finally drops IN but retains the leading join range");
    assert_eq!(range_lows(&fallback), vec![vec![Datum::Null]]);
    assert_eq!(fallback.idxOff2KeyOff, vec![0, -1, -1, -1, -1]);
    assert!(fallback.chosenAccess.is_empty());
    assert_eq!(names(&fallback.chosenRemained), vec!["in:[1,3,5]"]);
    assert!(fallback.lastColManager.is_none());
}

#[test]
/// fallback 可丢弃下一列 range，但仍保留前导 IN。
fn range_fallback_drops_next_column_range_but_keeps_leading_in() {
    let path = go_index_path();
    let pushed = vec![
        expression("in:[1,3]", 0),
        expression("gt:[aaa]", 2),
        expression("lt:[bbb]", 2),
    ];
    let (full, empty) = build_path(&path, &[1], pushed.clone(), vec![], 0, false);
    assert!(!empty);
    let full = full.expect("unlimited next-column range");
    assert_eq!(full.chosenRanges.len(), 2);
    assert!(full.chosenRanges.iter().all(|range| range.low.len() == 3));

    let (fallback, empty) = build_path(&path, &[1], pushed, vec![], 95, false);
    assert!(!empty);
    let fallback = fallback.expect("Go drops the next-column interval and keeps the IN prefix");
    assert_eq!(fallback.chosenRanges.len(), 2);
    assert!(
        fallback
            .chosenRanges
            .iter()
            .all(|range| range.low.len() == 2)
    );
    assert_eq!(names(&fallback.chosenAccess), vec!["in:[1,3]"]);
    assert_eq!(
        names(&fallback.chosenRemained),
        vec!["gt:[aaa]", "lt:[bbb]"]
    );
}

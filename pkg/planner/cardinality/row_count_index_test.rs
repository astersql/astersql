// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

use crate::*;
use std::collections::HashMap;
use std::sync::Arc;

struct TestContext {
    vars: variable::SessionVars,
    expr: Arc<exprstatic::ExprContext>,
    ranger: planctx_dependency::rangerctx::RangerContext<'static>,
}

impl Default for TestContext {
    fn default() -> Self {
        let expr = Arc::new(exprstatic::NewExprContext(Vec::new()));
        Self {
            vars: variable::SessionVars::default(),
            ranger: planctx_dependency::rangerctx::RangerContext {
                TypeCtx: (*expression::types::DefaultStmtNoWarningContext).clone(),
                ErrCtx: ranger::errctx::StrictNoWarningContext.clone(),
                ExprCtx: expr.clone(),
                RangeFallbackHandler: None,
                PlanCacheTracker: None,
                OptimizerFixControl: HashMap::new(),
                UseCache: false,
                RegardNULLAsPoint: false,
                OptPrefixIndexSingleScan: false,
            },
            expr,
        }
    }
}

impl CardinalityContext for TestContext {
    fn GetSessionVars(&self) -> &variable::SessionVars {
        &self.vars
    }

    fn GetExprCtx(&self) -> &dyn planctx_dependency::exprctx::ExprContext {
        self.expr.as_ref()
    }

    fn GetRangerCtx(&self) -> &planctx_dependency::rangerctx::RangerContext<'_> {
        &self.ranger
    }
}

#[test]
fn exp_backoff_treats_missing_index_column_mapping_as_empty() {
    let field_type = types::NewFieldType(mysql::TypeLonglong);
    let idx = statistics::Index {
        CMSketch: None,
        TopN: None,
        FMSketch: None,
        Info: Some(statistics::IndexInfo {
            Columns: vec![statistics::IndexColumnInfo::default()],
            ..Default::default()
        }),
        Histogram: statistics::NewHistogram(42, 0, 0, 0, &field_type, 0, 0),
        StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
        PhysicalID: 0,
        StatsVer: statistics::Version2 as i64,
    };
    let coll = statistics::NewHistColl(1, 100, 0, 0, 0);
    let range = ranger::Range {
        LowVal: vec![types::NewIntDatum(1)],
        HighVal: vec![types::NewIntDatum(1)],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Default::default()
    };

    let result = expBackoffEstimation(&TestContext::default(), &idx, &coll, &range, &[])
        .expect("missing mapping should be treated like Go's nil slice");

    assert_eq!(result, (0.0, 0.0, 0.0, false));
}

#[test]
fn go_merge_46_zero_repeat_column_upper_uses_uniform_estimate() {
    let field_type = types::NewFieldType(mysql::TypeLonglong);
    let mut histogram = statistics::NewHistogram(1, 100, 0, 0, &field_type, 2, 0);
    histogram.AppendBucket(&types::NewIntDatum(1), &types::NewIntDatum(50), 100, 0);
    histogram.AppendBucket(&types::NewIntDatum(51), &types::NewIntDatum(100), 200, 5);
    let column = statistics::Column {
        CMSketch: None,
        TopN: None,
        FMSketch: None,
        Info: None,
        Histogram: histogram,
        StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
        PhysicalID: 0,
        StatsVer: statistics::Version2 as i64,
        IsHandle: false,
    };
    let estimate = equalRowCountOnColumn(
        &TestContext::default(),
        &column,
        types::NewIntDatum(50),
        Vec::new(),
        200,
        0,
    )
    .expect("column upper estimate");
    assert_eq!(estimate.Est, 2.0, "zero Repeat uses the uniform average");
    let observed = equalRowCountOnColumn(
        &TestContext::default(),
        &column,
        types::NewIntDatum(100),
        Vec::new(),
        200,
        0,
    )
    .expect("observed upper estimate");
    assert_eq!(observed.Est, 5.0, "positive Repeat remains exact");
}

#[test]
fn go_merge_46_zero_repeat_index_upper_uses_uniform_estimate() {
    let field_type = types::NewFieldType(mysql::TypeBlob);
    let mut histogram = statistics::NewHistogram(1, 100, 0, 0, &field_type, 2, 0);
    histogram.AppendBucket(
        &types::NewBytesDatum(vec![1]),
        &types::NewBytesDatum(vec![50]),
        100,
        0,
    );
    histogram.AppendBucket(
        &types::NewBytesDatum(vec![51]),
        &types::NewBytesDatum(vec![100]),
        200,
        5,
    );
    let index = statistics::Index {
        CMSketch: None,
        TopN: None,
        FMSketch: None,
        Info: Some(statistics::IndexInfo {
            Columns: vec![statistics::IndexColumnInfo::default()],
            ..Default::default()
        }),
        Histogram: histogram,
        StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
        PhysicalID: 0,
        StatsVer: statistics::Version2 as i64,
    };
    let estimate = equalRowCountOnIndex(&TestContext::default(), &index, vec![50], 200, 0);
    assert_eq!(estimate.Est, 2.0, "zero Repeat uses the uniform average");
    let observed = equalRowCountOnIndex(&TestContext::default(), &index, vec![100], 200, 0);
    assert_eq!(observed.Est, 5.0, "positive Repeat remains exact");
}

#[test]
fn appended_handle_selectivity_merges_bounds_damps_and_caps_points() {
    let context = TestContext::default();
    let field_type = types::NewFieldType(mysql::TypeLonglong);
    let mut histogram = statistics::NewHistogram(2, 100, 0, 0, &field_type, 1, 0);
    histogram.AppendBucket(&types::NewIntDatum(1), &types::NewIntDatum(100), 100, 1);
    let mut coll = statistics::NewHistColl(1, 100, 0, 1, 0);
    coll.SetCol(
        2,
        Box::new(statistics::Column {
            CMSketch: None,
            TopN: None,
            FMSketch: None,
            Info: None,
            Histogram: histogram,
            StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
            PhysicalID: 1,
            StatsVer: statistics::Version2 as i64,
            IsHandle: true,
        }),
    );
    let index_col = expression::Column::new(*field_type.clone(), 1, 1, 0);
    let handle_col = expression::Column::new(*field_type, 2, 2, 1);
    let point = ranger::Range {
        LowVal: vec![types::NewIntDatum(5), types::NewIntDatum(7)],
        HighVal: vec![types::NewIntDatum(5), types::NewIntDatum(7)],
        Collators: collate::GetBinaryCollatorSlice(2),
        ..Default::default()
    };
    let result = AdjustRowCountForAppendedHandleColumns(
        &context,
        &coll,
        &[&point],
        &[&index_col, &handle_col],
        1,
        statistics::DefaultRowEst(10.0),
    );
    assert_eq!(result.Est, 1.0);
    assert_eq!(result.MaxEst, 1.0);

    let range = ranger::Range {
        LowVal: vec![types::NewIntDatum(5), types::NewIntDatum(10)],
        HighVal: vec![types::NewIntDatum(5), types::NewIntDatum(20)],
        Collators: collate::GetBinaryCollatorSlice(2),
        ..Default::default()
    };
    let result = AdjustRowCountForAppendedHandleColumns(
        &context,
        &coll,
        &[&range],
        &[&index_col, &handle_col],
        1,
        statistics::DefaultRowEst(10.0),
    );
    assert!(result.Est > 1.0 && result.Est < 10.0, "{result:?}");
    assert_eq!(result.MaxEst, 10.0);
    let single_estimate = result;
    let mut repeated = range.clone();
    repeated.LowVal[0] = types::NewIntDatum(6);
    repeated.HighVal[0] = types::NewIntDatum(6);
    let merged = AdjustRowCountForAppendedHandleColumns(
        &context,
        &coll,
        &[&range, &repeated],
        &[&index_col, &handle_col],
        1,
        statistics::DefaultRowEst(10.0),
    );
    assert_eq!(
        merged, single_estimate,
        "same handle bound is counted once across prefixes"
    );
    let mut exclusive = range.clone();
    exclusive.LowExclude = true;
    let exclusive_count = GetRowCountByColumnRanges(
        &context,
        &coll,
        2,
        &[&ranger::Range {
            LowVal: vec![types::NewIntDatum(10)],
            HighVal: vec![types::NewIntDatum(20)],
            Collators: collate::GetBinaryCollatorSlice(1),
            LowExclude: true,
            ..Default::default()
        }],
        false,
    )
    .unwrap();
    let exclusive_estimate = AdjustRowCountForAppendedHandleColumns(
        &context,
        &coll,
        &[&exclusive],
        &[&index_col, &handle_col],
        1,
        statistics::DefaultRowEst(10.0),
    );
    assert_eq!(
        exclusive_estimate.Est,
        10.0 * (exclusive_count.Est / 100.0).sqrt()
    );
    assert_eq!(
        exclusive_estimate.MinEst,
        10.0 * exclusive_count.Est / 100.0
    );
    let prefix_only = ranger::Range {
        LowVal: vec![types::NewIntDatum(5)],
        HighVal: vec![types::NewIntDatum(5)],
        Collators: collate::GetBinaryCollatorSlice(1),
        ..Default::default()
    };
    let unbound = AdjustRowCountForAppendedHandleColumns(
        &context,
        &coll,
        &[&range, &prefix_only],
        &[&index_col, &handle_col],
        1,
        statistics::DefaultRowEst(10.0),
    );
    assert_eq!(unbound, statistics::DefaultRowEst(10.0));
    let small_prefix = AdjustRowCountForAppendedHandleColumns(
        &context,
        &coll,
        &[&range],
        &[&index_col, &handle_col],
        1,
        statistics::DefaultRowEst(0.5),
    );
    assert_eq!(small_prefix.Est, 0.5, "sub-row prefixes retain their floor");
}

#[test]
fn virtual_column_recursive_index_estimates_propagate_and_retry() {
    let _scenario = fail::FailScenario::setup();
    let field_type = types::NewFieldType(mysql::TypeBlob);
    let low = codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(0)])
        .expect("encode lower bound");
    let high = codec::EncodeKey(codec::time::UTC, Vec::new(), vec![types::NewIntDatum(9)])
        .expect("encode upper bound");
    let make_index = |id: i64, cols: usize, repeat: i64| {
        let mut histogram = statistics::NewHistogram(id, 10, 0, 0, &field_type, 1, 0);
        histogram.AppendBucket(
            &types::NewBytesDatum(low.clone()),
            &types::NewBytesDatum(high.clone()),
            500,
            repeat,
        );
        statistics::Index {
            CMSketch: None,
            TopN: None,
            FMSketch: None,
            Info: Some(statistics::IndexInfo {
                ID: id,
                Columns: vec![statistics::IndexColumnInfo::default(); cols],
                ..Default::default()
            }),
            Histogram: histogram,
            StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
            PhysicalID: 1,
            StatsVer: statistics::Version2 as i64,
        }
    };
    let mut main_index = make_index(1, 2, 50);
    let mut coll = *statistics::NewHistColl(1, 500, 0, 0, 2);
    coll.Indices.insert(11, Box::new(make_index(11, 1, 10)));
    coll.Indices.insert(12, Box::new(make_index(12, 1, 50)));
    coll.Idx2ColUniqueIDs.insert(1, vec![1, 2]);
    coll.ColUniqueID2IdxIDs.insert(1, vec![11, 12]);
    let mut virtual_column =
        expression::Column::new(*types::NewFieldType(mysql::TypeLonglong), 1, 1, 0);
    virtual_column.VirtualExpr = Some(Box::new(expression::NewInt64Const(9)));
    let status_column = expression::Column::new(*types::NewFieldType(mysql::TypeLonglong), 2, 2, 1);
    let range = ranger::Range {
        LowVal: vec![types::NewIntDatum(9), types::NewIntDatum(0)],
        HighVal: vec![types::NewIntDatum(9), types::NewIntDatum(9)],
        Collators: collate::GetBinaryCollatorSlice(2),
        ..Default::default()
    };
    // Both candidates have nonempty statistics for 500 rows. The successful
    // first candidate must stop the search; its repeat count estimates 10 rows.
    let estimate = |coll: &statistics::HistColl| {
        expBackoffEstimation(
            &TestContext::default(),
            &main_index,
            coll,
            &range,
            &[&virtual_column, &status_column],
        )
        .expect("recursive candidate errors must not escape exponential backoff")
    };
    assert_eq!(estimate(&coll), (0.02, 0.02, 0.02, true));

    // The Go regression forces the first candidate to fail and requires the
    // next index's estimate (50 / 500) to survive the recursive call.
    fail::cfg("afterRecursiveIndexEstimation", "return(11)").expect("inject first index error");
    assert_eq!(estimate(&coll), (0.1, 0.1, 0.1, true));

    // No successful candidate must leave foundStats false, preserving the
    // virtual-column fallback to the composite index's own statistics.
    coll.ColUniqueID2IdxIDs.insert(1, vec![11]);
    assert_eq!(estimate(&coll), (0.0, 0.0, 0.0, false));
    fail::remove("afterRecursiveIndexEstimation");

    // MV index entries can outnumber analyzed table rows. With 500 entries
    // versus 250 analyzed rows the recursive denominator is 1000, not 500.
    coll.GetIdxMut(11).unwrap().Info.as_mut().unwrap().MVIndex = true;
    coll.GetIdxMut(12).unwrap().Histogram.Buckets[0].Count = 250;
    coll.ColUniqueID2IdxIDs.insert(1, vec![11]);
    coll.ColUniqueID2IdxIDs.insert(2, vec![12]);
    assert_eq!(
        coll.GetScaledRealtimeAndModifyCnt(coll.GetIdx(11).unwrap())
            .0,
        1000
    );
    // Keep the composite NDV floor below the recursive upper bound.
    main_index.Histogram.NDV = 500;
    let (_, _, upper, found) = expBackoffEstimation(
        &TestContext::default(),
        &main_index,
        &coll,
        &range,
        &[&virtual_column, &status_column],
    )
    .expect("scaled candidate estimation");
    assert!(found);
    assert!((upper - 0.02).abs() < 1e-9, "upper={upper}");
}

#[test]
fn appended_handle_missing_stats_keeps_prefix_and_point_cap() {
    let context = TestContext::default();
    let mut coll = statistics::NewHistColl(1, 100, 0, 0, 0);
    let field_type = types::NewFieldType(mysql::TypeLonglong);
    let index_col = expression::Column::new(*field_type.clone(), 1, 1, 0);
    let handle_col = expression::Column::new(*field_type, 2, 2, 1);
    let point = ranger::Range {
        LowVal: vec![types::NewIntDatum(5), types::NewIntDatum(7)],
        HighVal: vec![types::NewIntDatum(5), types::NewIntDatum(7)],
        Collators: collate::GetBinaryCollatorSlice(2),
        ..Default::default()
    };
    let prefix = statistics::DefaultRowEst(10.0);
    let adjust = |coll: &statistics::HistColl,
                  ranges: &[&ranger::Range],
                  columns: &[&expression::Column]| {
        AdjustRowCountForAppendedHandleColumns(&context, coll, ranges, columns, 1, prefix)
    };
    assert_eq!(adjust(&coll, &[], &[&index_col, &handle_col]), prefix);
    assert_eq!(adjust(&coll, &[&point], &[&index_col]), prefix);
    assert_eq!(
        adjust(&coll, &[&point], &[&index_col, &handle_col]),
        statistics::DefaultRowEst(1.0)
    );
    coll.RealtimeCount = 0;
    assert_eq!(adjust(&coll, &[&point], &[&index_col, &handle_col]), prefix);
}

#[test]
fn combined_merge_optimizer_callbacks_use_merged_histogram_and_topn() {
    let context = TestContext::default();
    let ft = types::NewFieldType(mysql::TypeLonglong);
    let hist = |specs: &[(i64, i64, i64, i64)]| {
        let mut h = statistics::NewHistogram(1, 0, 0, 0, &ft, specs.len(), 0);
        let mut count = 0;
        for &(lo, up, mass, repeat) in specs {
            count += mass;
            h.AppendBucket(
                &types::NewIntDatum(lo),
                &types::NewIntDatum(up),
                count,
                repeat,
            );
        }
        Some(h)
    };
    let encode =
        |v| codec::EncodeKey(chrono_tz::UTC, Vec::new(), vec![types::NewIntDatum(v)]).unwrap();
    for (specs, ndv, n, cap, value, want) in [
        (
            vec![vec![(0, 10, 100, 20)], vec![(10, 20, 100, 1)]],
            50,
            0,
            2,
            10,
            20.0,
        ),
        (
            vec![vec![(1, 100, 100, 1)], vec![(90, 110, 100, 1)]],
            100,
            0,
            2,
            90,
            2.0,
        ),
        (
            vec![{
                let mut s = (0..10)
                    .map(|i| (i * 10 + 1, i * 10 + 10, 100, 1))
                    .collect::<Vec<_>>();
                s.push((200, 200, 1000, 1000));
                s
            }],
            11,
            1,
            10,
            200,
            1000.0,
        ),
        (
            vec![
                vec![(1, 10, 2, 1)],
                vec![(5, 100, 100, 50)],
                vec![(5, 100, 100, 50)],
            ],
            4,
            0,
            2,
            0,
            0.0,
        ),
    ] {
        let hists = specs.iter().map(|s| hist(s)).collect::<Vec<_>>();
        let (top, mut histogram) = statistics::MergePartTopNAndHistToGlobal(
            &stmtctx_dependency::NewStmtCtx(),
            &Default::default(),
            &[],
            &hists,
            n,
            cap,
            false,
        )
        .unwrap();
        histogram.NDV = ndv;
        let total =
            histogram.NotNullCount() + top.as_ref().map_or(0, statistics::TopN::TotalCount) as f64;
        let column = statistics::Column {
            CMSketch: None,
            TopN: top,
            FMSketch: None,
            Info: None,
            Histogram: histogram,
            StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
            PhysicalID: 0,
            StatsVer: 2,
            IsHandle: false,
        };
        if value != 0 {
            assert_eq!(
                equalRowCountOnColumn(
                    &context,
                    &column,
                    types::NewIntDatum(value),
                    encode(value),
                    total as i64,
                    0
                )
                .unwrap()
                .Est,
                want
            );
        }
        let between = |lo, hi| {
            betweenRowCountOnColumn(
                &context,
                &column,
                types::NewIntDatum(lo),
                types::NewIntDatum(hi),
                encode(lo),
                encode(hi),
            )
            .Est
        };
        if value == 10 {
            assert!(between(0, 20) >= 20.0);
        }
        if value == 0 {
            assert!(between(1, 5) >= 0.0);
            assert!(between(1, 100) <= 202.0);
        }
    }
    let index_ft = types::NewFieldType(mysql::TypeBlob);
    let empty = Some(statistics::NewHistogram(1, 6, 0, 0, &index_ft, 0, 0));
    let tops = [vec![1, 2, 3], vec![15], vec![25], vec![35]]
        .into_iter()
        .map(|values| {
            let mut t = statistics::NewTopN(values.len());
            for value in values {
                t.AppendTopN(encode(value), 1);
            }
            t.Sort();
            Some(t)
        })
        .collect::<Vec<_>>();
    let (top, mut histogram) = statistics::MergePartTopNAndHistToGlobal(
        &stmtctx_dependency::NewStmtCtx(),
        &Default::default(),
        &tops,
        &vec![empty; 4],
        100,
        256,
        true,
    )
    .unwrap();
    histogram.NDV = 6;
    let idx = statistics::Index {
        CMSketch: None,
        TopN: top,
        FMSketch: None,
        Info: Some(statistics::IndexInfo {
            Columns: vec![statistics::IndexColumnInfo::default()],
            ..Default::default()
        }),
        Histogram: histogram,
        StatsLoadedStatus: statistics::NewStatsFullLoadStatus(),
        PhysicalID: 0,
        StatsVer: 2,
    };
    let mut encoded_hist = statistics::NewHistogram(1, 10, 0, 0, &index_ft, 4, 0);
    for i in 0..4 {
        encoded_hist.AppendBucket(
            &types::NewBytesDatum(encode(i * 5 + 1)),
            &types::NewBytesDatum(encode(i * 5 + 5)),
            (i + 1) * 25,
            1,
        );
    }
    let tops = [vec![(1, 50), (2, 30)], vec![(1, 40), (3, 20)]]
        .into_iter()
        .map(|entries| {
            let mut top = statistics::NewTopN(2);
            for (value, count) in entries {
                top.AppendTopN(encode(value), count);
            }
            top.Sort();
            Some(top)
        })
        .collect::<Vec<_>>();
    let (top, mut histogram) = statistics::MergePartTopNAndHistToGlobal(
        &stmtctx_dependency::NewStmtCtx(),
        &Default::default(),
        &tops,
        &[Some(encoded_hist.clone()), Some(encoded_hist)],
        2,
        4,
        true,
    )
    .unwrap();
    histogram.NDV = 10;
    let encoded_index = statistics::Index {
        TopN: top,
        Histogram: histogram,
        ..idx.clone()
    };
    assert_eq!(
        equalRowCountOnIndex(&context, &encoded_index, encode(1), 340, 0).Est,
        90.0
    );
    assert!(
        betweenRowCountOnIndex(
            &context,
            &encoded_index,
            types::NewBytesDatum(encode(1)),
            types::NewBytesDatum(encode(200))
        )
        .Est > 0.0
    );
    let range = ranger::Range {
        LowVal: vec![types::NewIntDatum(0)],
        HighVal: vec![types::NewIntDatum(16)],
        Collators: vec![collate::GetBinaryCollator()],
        HighExclude: true,
        ..Default::default()
    };
    assert_eq!(
        getIndexRowCountForStatsV2(&context, &idx, None, &[&range], &[], 6, 0)
            .unwrap()
            .Est,
        4.0
    );
}

// Copyright 2026 AsterSQL.

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
fn go_merge_46_appended_handle_damps_and_caps_full_point() {
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
}

#[test]
fn go_merge_46_virtual_column_recursive_index_error_uses_next_candidate() {
    let _scenario = fail::FailScenario::setup();
    fail::cfg("afterRecursiveIndexEstimation", "return(11)").expect("inject first index error");
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
    let main_index = make_index(1, 2, 50);
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
    let (estimate, _, _, found) = expBackoffEstimation(
        &TestContext::default(),
        &main_index,
        &coll,
        &range,
        &[&virtual_column, &status_column],
    )
    .expect("second index must recover recursive estimation");
    assert!(found);
    assert!((estimate - 0.1).abs() < 1e-9, "estimate={estimate}");
}

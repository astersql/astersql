// Copyright 2026 AsterSQL.

use crate::*;

struct TestContext {
    vars: variable::SessionVars,
}

impl Default for TestContext {
    fn default() -> Self {
        Self {
            vars: variable::SessionVars::default(),
        }
    }
}

impl CardinalityContext for TestContext {
    fn GetSessionVars(&self) -> &variable::SessionVars {
        &self.vars
    }

    fn GetExprCtx(&self) -> &dyn planctx_dependency::exprctx::ExprContext {
        panic!("missing index-column mappings must not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx_dependency::rangerctx::RangerContext<'_> {
        panic!("missing index-column mappings must not build ranges")
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

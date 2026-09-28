// Copyright 2026 AsterSQL.

use crate::*;
use std::sync::Arc;

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
        panic!("stats tracing does not evaluate expressions")
    }

    fn GetRangerCtx(&self) -> &planctx_dependency::rangerctx::RangerContext<'_> {
        panic!("stats tracing does not build ranges")
    }
}

fn column_with_status(status: statistics::StatsLoadedStatus) -> statistics::Column {
    statistics::Column {
        CMSketch: None,
        TopN: None,
        FMSketch: None,
        Info: None,
        Histogram: statistics::NewHistogram(
            7,
            0,
            0,
            0,
            &types::NewFieldType(mysql::TypeLonglong),
            0,
            0,
        ),
        StatsLoadedStatus: status,
        PhysicalID: 42,
        StatsVer: statistics::Version2 as i64,
        IsHandle: false,
    }
}

#[test]
fn missing_analyzed_item_is_recorded_as_uninitialized() {
    let context = TestContext::default();
    let mut existence = statistics::NewColAndIndexExistenceMapWithoutSize();
    existence.InsertCol(7, true);
    existence.InsertIndex(8, true);
    let used = context
        .vars
        .StmtCtx
        .GetUsedStatsInfo(true)
        .expect("used stats container");
    used.RecordUsedInfo(
        42,
        Arc::new(stmtctx::UsedStatsInfoForTable {
            ColAndIdxStatus: Some(stmtctx::cache_value(*existence)),
            ..Default::default()
        }),
    );

    recordUsedItemStatsStatus(&context, UsedStatsItem::Column(None), 42, 7);
    recordUsedItemStatsStatus(&context, UsedStatsItem::Index(None), 42, 8);

    let statuses = context.vars.StmtCtx.UsedStatsLoadStatus();
    assert_eq!(
        statuses.get(&(42, 7, false)).map(String::as_str),
        Some("unInitialized")
    );
    assert_eq!(
        statuses.get(&(42, 8, true)).map(String::as_str),
        Some("unInitialized")
    );
}

#[test]
fn missing_unanalyzed_item_and_non_positive_ids_follow_go_behavior() {
    let context = TestContext::default();

    recordUsedItemStatsStatus(&context, UsedStatsItem::Column(None), 42, -1);
    recordUsedItemStatsStatus(&context, UsedStatsItem::Column(None), 42, 0);
    recordUsedItemStatsStatus(&context, UsedStatsItem::Column(None), 42, 9);
    recordUsedItemStatsStatus(&context, UsedStatsItem::Index(None), 42, 10);

    let statuses = context.vars.StmtCtx.UsedStatsLoadStatus();
    assert_eq!(statuses.len(), 2);
    assert_eq!(
        statuses.get(&(42, 9, false)).map(String::as_str),
        Some("missing")
    );
    assert_eq!(
        statuses.get(&(42, 10, true)).map(String::as_str),
        Some("missing")
    );
}

#[test]
fn full_load_is_ignored_and_partial_status_is_recorded() {
    let context = TestContext::default();
    let full = column_with_status(statistics::NewStatsFullLoadStatus());
    let partial = column_with_status(statistics::NewStatsAllEvictedStatus());

    recordUsedItemStatsStatus(&context, UsedStatsItem::Column(Some(&full)), 42, 7);
    recordUsedItemStatsStatus(&context, UsedStatsItem::Column(Some(&partial)), 42, 8);

    let statuses = context.vars.StmtCtx.UsedStatsLoadStatus();
    assert!(!statuses.contains_key(&(42, 7, false)));
    assert_eq!(
        statuses.get(&(42, 8, false)).map(String::as_str),
        Some("allEvicted")
    );
}

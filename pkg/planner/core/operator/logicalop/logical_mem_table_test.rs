// Copyright 2026 AsterSQL.

use crate::*;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

struct TestPlanContext {
    plan_id: AtomicI32,
    builtin_function_usage: base::BuiltinFunctionUsageCounter,
}

impl base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        panic!("unused")
    }
    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("unused")
    }
    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("unused")
    }
    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        panic!("unused")
    }
    fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
        panic!("unused")
    }
    fn BuiltinFunctionUsageInc(&self, name: &str) {
        self.builtin_function_usage.Inc(name)
    }
}

fn context() -> base::ContextRef {
    Arc::new(TestPlanContext {
        plan_id: AtomicI32::new(0),
        builtin_function_usage: base::BuiltinFunctionUsageCounter::default(),
    })
}

#[derive(Clone, Default)]
struct HintState {
    limit: Option<u64>,
    desc: Option<bool>,
}

#[derive(Clone, Default)]
struct RecordingExtractor(Arc<Mutex<HintState>>);

impl MemTablePredicateExtractor for RecordingExtractor {
    fn CloneBox(&self) -> Box<dyn MemTablePredicateExtractor> {
        Box::new(self.clone())
    }
    fn Extract(
        &mut self,
        _: &Schema,
        _: &NameSlice,
        predicates: Vec<Expression>,
    ) -> Vec<Expression> {
        predicates
    }
    fn SetRowLimitHint(&mut self, limit: u64) {
        self.0.lock().unwrap().limit = Some(limit);
    }
    fn SetDesc(&mut self, desc: bool) {
        self.0.lock().unwrap().desc = Some(desc);
    }
}

fn column(unique_id: i64, id: i64) -> Column {
    let mut column = Column::default();
    column.UniqueID = unique_id;
    column.ID = id;
    column
}

fn mem_table(name: &str, extractor: Option<RecordingExtractor>) -> LogicalMemTable {
    let mut plan = LogicalMemTable {
        Extractor: extractor.map(|extractor| Box::new(extractor) as _),
        TableInfo: model::TableInfo {
            Name: parser_ast::NewCIStr(name),
            ..Default::default()
        },
        ..Default::default()
    }
    .Init(context(), 0);
    plan.SetSchema(expression::NewSchema(vec![column(1, 11), column(2, 12)]));
    plan.Columns = vec![model::ColumnInfo::default(), model::ColumnInfo::default()];
    plan
}

#[test]
fn prune_columns_covers_every_go_whitelisted_memory_table() {
    let names = [
        "statements_summary",
        "statements_summary_history",
        "tidb_statements_stats",
        "cluster_statements_summary",
        "cluster_statements_summary_history",
        "cluster_tidb_statements_stats",
        "slow_query",
        "cluster_slow_query",
        "tidb_trx",
        "cluster_tidb_trx",
        "data_lock_waits",
        "deadlocks",
        "cluster_deadlocks",
        "tables",
    ];
    for name in names {
        let mut plan = mem_table(name, None);
        plan.PruneColumns(&[column(2, 12)]).unwrap();
        assert_eq!(
            plan.Schema().Columns.len(),
            1,
            "{name} must be prunable like Go"
        );
        assert_eq!(plan.Schema().Columns[0].UniqueID, 2, "{name}");
    }
}

#[test]
fn top_n_pushdown_sets_limit_and_slow_log_direction_hints() {
    let extractor = RecordingExtractor::default();
    let state = extractor.0.clone();
    let mut plan = mem_table("slow_query", Some(extractor));
    plan.TableInfo.Columns = vec![model::ColumnInfo {
        ID: 88,
        Name: parser_ast::NewCIStr("time"),
        ..Default::default()
    }];
    let top_n = LogicalTopN {
        ByItems: vec![ByItems {
            Expr: Box::new(column(9, 88)),
            Desc: true,
        }],
        Offset: u64::MAX - 2,
        Count: 10,
        ..Default::default()
    }
    .Init(context(), 0);

    let returned = LogicalPlan::PushDownTopN(&mut plan, Some(Box::new(top_n)))
        .expect("TopN remains above mem table");
    assert!(returned.as_any().is::<LogicalTopN>());
    let hints = state.lock().unwrap().clone();
    assert_eq!(
        hints.limit,
        Some(u64::MAX),
        "offset+count saturates like Go overflow guard"
    );
    assert_eq!(hints.desc, Some(true));
}

#[test]
fn derive_stats_uses_pseudo_table_contract() {
    let mut plan = mem_table("tables", None);
    plan.TableInfo.ID = 42;
    let (stats, reloaded) = plan.DeriveStats(true).unwrap();
    assert!(reloaded);
    assert_eq!(stats.RowCount, statistics::PseudoRowCount as f64);
    assert_eq!(stats.StatsVersion, statistics::PseudoVersion);
    let hist = stats
        .HistColl
        .as_ref()
        .expect("Go stores generated pseudo HistColl");
    let hist = hist
        .downcast_ref::<statistics::HistColl>()
        .expect("HistColl type");
    assert_eq!(hist.PhysicalID, 42);
    assert_eq!(hist.RealtimeCount, statistics::PseudoRowCount);
}

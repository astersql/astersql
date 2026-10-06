// Copyright 2026 AsterSQL.

use crate::statement_ru_reporting::{
    StatementRUComputeUnits, StatementRUEngine, StatementRUOperator,
};
use crate::statement_ru_result::{
    ScanEvidence, StatementRUCalculationSetup, StatementRUCalculator, classify_scan_evidence,
    trim_statement_ru_explain_prefix,
};
use astersql_resourcegroup::ruv2::model::StmtUnits;

struct RestoreGlobalConfig(Option<astersql_config::Config>);

impl Drop for RestoreGlobalConfig {
    fn drop(&mut self) {
        if let Some(config) = self.0.take() {
            astersql_config::store_global_config(config);
        }
    }
}

#[test]
fn statement_ru_finalize_uses_current_config_weights() {
    let _restore = RestoreGlobalConfig(Some(astersql_config::get_global_config().as_ref().clone()));
    astersql_config::update_global(|config| {
        config.ruv2.stmt_weights.cpu_work = 2.0;
        config.ruv2.stmt_weights.scan_byte = 3.0;
    });

    let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup::default());
    calculator.units = StmtUnits {
        cpu_work: 5.0,
        scan_bytes: 7.0,
        ..Default::default()
    };
    assert_eq!(calculator.finalize().unwrap().result.total_ru, 31.0);

    astersql_config::update_global(|config| config.ruv2.stmt_weights.cpu_work = 0.0);
    assert_eq!(calculator.finalize().unwrap().result.total_ru, 21.0);
}

#[test]
fn go_merge_197_scan_evidence_matches_go_validity_contract() {
    assert_eq!(classify_scan_evidence(0, 0, 0), ScanEvidence::Valid(0.0));
    assert_eq!(classify_scan_evidence(9, 0, 0), ScanEvidence::Valid(0.0));
    assert_eq!(classify_scan_evidence(9, 0, 1), ScanEvidence::Invalid);
    assert_eq!(classify_scan_evidence(-1, 1, 1), ScanEvidence::Invalid);
    assert_eq!(classify_scan_evidence(9, -1, 1), ScanEvidence::Invalid);
    assert_eq!(classify_scan_evidence(9, 1, -1), ScanEvidence::Invalid);
    assert_eq!(classify_scan_evidence(0, 1, 10), ScanEvidence::Unavailable);
    assert_eq!(classify_scan_evidence(9, 1, 0), ScanEvidence::Unavailable);
    assert_eq!(classify_scan_evidence(9, 3, 12), ScanEvidence::Valid(36.0));
    assert_eq!(
        classify_scan_evidence(i64::MAX, 1, i64::MAX),
        ScanEvidence::Valid((i64::MAX as f64) * (i64::MAX as f64))
    );
}

#[test]
fn trim_explain_prefix_matches_normalized_go_forms() {
    assert_eq!(
        trim_statement_ru_explain_prefix("explain analyze format = ? select * from t"),
        "select * from t"
    );
    assert_eq!(
        trim_statement_ru_explain_prefix("explain analyze format = ru select * from t"),
        "select * from t"
    );
    assert_eq!(
        trim_statement_ru_explain_prefix("explain analyze format = ? "),
        "explain analyze format = ? "
    );
    assert_eq!(trim_statement_ru_explain_prefix("select 1"), "select 1");
}

#[test]
fn go_merge_197_finalize_applies_tiflash_multiplier_and_freezes_report() {
    let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup {
        frontend_compile_bytes: 2.0,
        full_report: true,
    });
    calculator.units.cpu_work = 3.0;
    calculator.compute[StatementRUEngine::TiFlash as usize] = StatementRUComputeUnits {
        cpu_work: 3.0,
        ..Default::default()
    };
    let report = calculator.report.as_mut().unwrap();
    report.add_operator(
        StatementRUEngine::TiFlash,
        StatementRUOperator::HashAgg,
        StmtUnits {
            cpu_work: 3.0,
            ..Default::default()
        },
    );
    let finalized = calculator.finalize().unwrap();
    assert_eq!(finalized.result.total_ru, 32.0);
    assert_eq!(finalized.engine_ru.tiflash, 30.0);
    assert_eq!(finalized.engine_ru.tidb, 2.0);
    let frozen = finalized.report.unwrap();
    assert_eq!(
        frozen.units[StatementRUEngine::TiDB as usize][StatementRUOperator::Frontend as usize]
            .frontend_compile_bytes,
        2.0
    );
    assert_eq!(
        frozen.units[StatementRUEngine::TiFlash as usize][StatementRUOperator::HashAgg as usize]
            .cpu_work,
        3.0
    );
    assert!(
        !calculator.report.as_ref().unwrap().seen[StatementRUEngine::TiDB as usize]
            [StatementRUOperator::Frontend as usize]
    );
}

#[test]
fn go_merge_197_finalize_rejects_invalid_units() {
    let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup::default());
    calculator.units.cpu_work = f64::NAN;
    assert!(calculator.finalize().is_none());
}

#[test]
fn frontend_compile_bytes_use_normalized_sql_and_ignore_literal_length() {
    use crate::adapter::{StatementKind, StatementNode};
    use crate::statement_ru_result::statement_ru_frontend_compile_bytes;
    let mut node = StatementNode {
        kind: StatementKind::Select,
        original_text: "SELECT 你好".into(),
        text: "fallback".into(),
        secure_text: String::new(),
        prepared_text: None,
    };
    assert_eq!(
        statement_ru_frontend_compile_bytes(&node, true, "original", "normalized"),
        0.0
    );
    assert_eq!(
        statement_ru_frontend_compile_bytes(&node, false, "", "ignored"),
        node.original_text.len() as f64
    );
    assert_eq!(
        statement_ru_frontend_compile_bytes(
            &node,
            false,
            "original",
            "explain analyze format = ru select ?"
        ),
        8.0
    );
    let short_literal = statement_ru_frontend_compile_bytes(
        &node,
        false,
        "select * from t where a = 'aaa'",
        "select * from t where a = ?",
    );
    let long_literal = statement_ru_frontend_compile_bytes(
        &node,
        false,
        "select * from t where a = 'aaaaaaaaaa'",
        "select * from t where a = ?",
    );
    assert_eq!(short_literal, long_literal);
    assert_eq!(short_literal, "select * from t where a = ?".len() as f64);
    node.original_text.clear();
    assert_eq!(
        statement_ru_frontend_compile_bytes(&node, false, "original", ""),
        8.0
    );
    assert_eq!(
        statement_ru_frontend_compile_bytes(&node, false, "", ""),
        8.0
    );
}

#[test]
fn go_merge_197_setup_eligibility() {
    use crate::statement_ru_result::{StatementRUInstallState, new_statement_ru_calculation_setup};
    use astersql_planner_core_base as base;
    use astersql_planner_core_operator_physicalop as physicalop;
    use astersql_planner_planctx as planctx;
    use std::sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    };
    struct Context(
        AtomicI32,
        base::BuiltinFunctionUsageCounter,
        planctx::variable::SessionVars,
    );
    impl base::PlanContext for Context {
        fn alloc_plan_id(&self) -> i32 {
            self.0.fetch_add(1, Ordering::SeqCst) + 1
        }
        fn ignore_explain_id_suffix(&self) -> bool {
            false
        }
        fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
            &self.2
        }
        fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
            unreachable!()
        }
        fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
            unreachable!()
        }
        fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
            unreachable!()
        }
        fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
            unreachable!()
        }
        fn BuiltinFunctionUsageInc(&self, name: &str) {
            self.1.Inc(name)
        }
    }
    let ctx: base::ContextRef = Arc::new(Context(
        AtomicI32::new(0),
        Default::default(),
        Default::default(),
    ));
    let plan = physicalop::PhysicalTableDual::New(ctx.clone(), 1);
    let state = StatementRUInstallState {
        in_select_stmt: true,
        ..Default::default()
    };
    assert_eq!(
        new_statement_ru_calculation_setup(Some(&plan), Some(&state), 17.0),
        Some(StatementRUCalculationSetup {
            frontend_compile_bytes: 17.0,
            full_report: false
        })
    );
    assert!(new_statement_ru_calculation_setup(None, Some(&state), 0.0).is_none());
    assert!(new_statement_ru_calculation_setup(Some(&plan), None, 0.0).is_none());
    assert!(
        new_statement_ru_calculation_setup(
            Some(&plan),
            Some(&StatementRUInstallState::default()),
            0.0
        )
        .is_none()
    );
    for excluded in [
        StatementRUInstallState {
            statement_context_present: false,
            ..state.clone()
        },
        StatementRUInstallState {
            restricted_sql: true,
            ..state.clone()
        },
        StatementRUInstallState {
            cursor_exists: true,
            ..state.clone()
        },
        StatementRUInstallState {
            flat_plan_cached: true,
            ..state.clone()
        },
    ] {
        assert!(new_statement_ru_calculation_setup(Some(&plan), Some(&excluded), 0.0).is_none());
    }
}

#[test]
fn go_merge_197_setup_eligibility_plan_branches_and_owner() {
    use crate::statement_ru_result::*;
    use astersql_planner_core as core;
    use astersql_planner_core_base as base;
    use astersql_planner_core_operator_physicalop as physicalop;
    use astersql_planner_planctx as planctx;
    use std::sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    };
    struct Context(
        AtomicI32,
        base::BuiltinFunctionUsageCounter,
        planctx::variable::SessionVars,
    );
    impl base::PlanContext for Context {
        fn alloc_plan_id(&self) -> i32 {
            self.0.fetch_add(1, Ordering::SeqCst) + 1
        }
        fn ignore_explain_id_suffix(&self) -> bool {
            false
        }
        fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
            &self.2
        }
        fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
            unreachable!()
        }
        fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
            unreachable!()
        }
        fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
            unreachable!()
        }
        fn GetBuildPBCtx(&self) -> &base::BuildPBContext {
            unreachable!()
        }
        fn BuiltinFunctionUsageInc(&self, name: &str) {
            self.1.Inc(name)
        }
    }
    let ctx: base::ContextRef = Arc::new(Context(
        AtomicI32::new(0),
        Default::default(),
        Default::default(),
    ));
    let dual = || {
        Box::new(physicalop::PhysicalTableDual::New(ctx.clone(), 1)) as Box<dyn base::PhysicalPlan>
    };
    let mut replace = physicalop::Insert::New(ctx.clone());
    replace.IsReplace = true;
    let plans: Vec<(Box<dyn base::Plan>, StatementRUPlanKind, &str)> = vec![
        (
            Box::new(physicalop::Insert::New(ctx.clone())),
            StatementRUPlanKind::Write,
            "insert",
        ),
        (Box::new(replace), StatementRUPlanKind::Write, "replace"),
        (
            Box::new(physicalop::Update::New(ctx.clone(), dual())),
            StatementRUPlanKind::Write,
            "update",
        ),
        (
            Box::new(physicalop::Delete::New(ctx.clone(), dual())),
            StatementRUPlanKind::Write,
            "delete",
        ),
        (
            Box::new(core::RuntimeAnalyze::New(
                ctx.clone(),
                core::Analyze::default(),
            )),
            StatementRUPlanKind::Analyze,
            "analyze",
        ),
        (
            Box::new(core::RuntimeSimple::New(
                ctx.clone(),
                astersql_parser_ast::NodeRef::new(Box::new(
                    astersql_parser_ast::CommitStmt::default(),
                )),
            )),
            StatementRUPlanKind::Commit,
            "commit",
        ),
        (
            Box::new(core::RuntimeSimple::New(
                ctx.clone(),
                astersql_parser_ast::NodeRef::new(Box::new(
                    astersql_parser_ast::RollbackStmt::default(),
                )),
            )),
            StatementRUPlanKind::Other,
            "select",
        ),
        (
            Box::new(physicalop::PointGetPlan::New(ctx.clone())),
            StatementRUPlanKind::PointLookup,
            "select",
        ),
        (
            Box::new(physicalop::BatchPointGetPlan::New(ctx.clone())),
            StatementRUPlanKind::PointLookup,
            "select",
        ),
    ];
    let state = StatementRUInstallState::default();
    for (plan, kind, sql_type) in plans {
        let info = classify_statement_ru_plan(plan.as_ref());
        assert_eq!(info.kind, kind);
        assert_eq!(info.sql_type, sql_type);
        let eligible = matches!(
            kind,
            StatementRUPlanKind::Write | StatementRUPlanKind::Analyze | StatementRUPlanKind::Commit
        );
        assert_eq!(
            new_statement_ru_calculation_setup(Some(plan.as_ref()), Some(&state), 4.0).is_some(),
            eligible
        );
        let wrapped = core::RuntimeExecute::New(Arc::from(plan));
        assert_eq!(classify_statement_ru_plan(&wrapped).kind, kind);
    }
    for analyze in [false, true] {
        let explain = core::RuntimeExplain::New(
            ctx.clone(),
            Box::new(physicalop::Insert::New(ctx.clone())),
            "ru".into(),
            analyze,
        );
        let wrapped = core::RuntimeExecute::New(Arc::new(explain));
        let info = classify_statement_ru_plan(&wrapped);
        assert_eq!(
            info.kind,
            if analyze {
                StatementRUPlanKind::Write
            } else {
                StatementRUPlanKind::Other
            }
        );
        assert_eq!(
            new_statement_ru_calculation_setup(Some(&wrapped), Some(&state), 0.0).is_some(),
            analyze
        );
    }
    for analyze in [false, true] {
        let explain = core::RuntimeExplain::New(ctx.clone(), dual(), "ru".into(), analyze);
        let wrapped = core::RuntimeExecute::New(Arc::new(explain));
        let info = classify_statement_ru_plan(&wrapped);
        assert_eq!(
            info.plan.as_any().is::<physicalop::PhysicalTableDual>(),
            analyze
        );
        assert_eq!(info.plan.as_any().is::<core::RuntimeExplain>(), !analyze);
    }
    let plan = dual();
    let read = StatementRUInstallState {
        is_read_only: true,
        ..Default::default()
    };
    let locking = StatementRUInstallState {
        in_select_stmt: true,
        is_read_only: false,
        ..Default::default()
    };
    for full in [false, true] {
        for state in [&read, &locking] {
            let (owner, failure) = install_statement_ru_owner_at_boundary(
                Some(plan.as_ref()),
                Some(state),
                11.0,
                full,
            );
            assert!(!failure);
            let owner = owner.unwrap();
            assert!(!owner.restricted_sql_at_install);
            assert!(!owner.ttl_job_at_install);
            assert!(!owner.cursor_at_install);
            assert_eq!(
                owner.take_terminal_setup(),
                Some(StatementRUCalculationSetup {
                    frontend_compile_bytes: 11.0,
                    full_report: full
                })
            );
        }
        let (owner, failure) =
            install_statement_ru_owner_at_boundary(Some(plan.as_ref()), Some(&state), 0.0, full);
        assert!(owner.is_none());
        assert_eq!(failure, full);
        let (_, failure) = install_statement_ru_owner_at_boundary(None, None, 0.0, full);
        assert!(!failure);
    }
    for restricted in [false, true] {
        for source in ["TTL", "internal"] {
            for job in ["", "job-1"] {
                let ttl = StatementRUInstallState {
                    in_select_stmt: true,
                    restricted_sql: restricted,
                    request_source_type: source.into(),
                    ttl_job_id: job.into(),
                    ..Default::default()
                };
                let expected_ttl = restricted && source == "TTL" && !job.is_empty();
                assert_eq!(is_statement_ru_ttl_job(&ttl), expected_ttl);
                let (owner, failure) = install_statement_ru_owner_at_boundary(
                    Some(plan.as_ref()),
                    Some(&ttl),
                    0.0,
                    true,
                );
                assert!(!failure);
                assert_eq!(owner.is_some(), !restricted || expected_ttl);
                if let Some(owner) = owner {
                    assert_eq!(owner.ttl_job_at_install, expected_ttl);
                    assert_eq!(owner.restricted_sql_at_install, restricted);
                }
            }
        }
    }
    for excluded in [
        StatementRUInstallState {
            cursor_exists: true,
            ..locking.clone()
        },
        StatementRUInstallState {
            flat_plan_cached: true,
            ..locking.clone()
        },
    ] {
        let (owner, failure) =
            install_statement_ru_owner_at_boundary(Some(plan.as_ref()), Some(&excluded), 0.0, true);
        assert!(owner.is_none());
        assert!(failure);
    }
}

#[test]
fn go_merge_195_197_publish_snapshot() {
    use crate::statement_ru_reporting::StatementRUFailureReason;
    use crate::statement_ru_result::{
        StatementRUPublicationSink, publish_statement_ru_finalized_snapshot,
    };
    use std::cell::RefCell;
    #[derive(Default)]
    struct Sink {
        events: RefCell<Vec<String>>,
        panic_metrics: bool,
        panic_consumption: bool,
        panic_calibration: bool,
    }
    impl StatementRUPublicationSink for Sink {
        fn consumption(&self, result: crate::statement_ru_reporting::StatementRUEngineResult) {
            self.events.borrow_mut().push(format!(
                "consumption:{},{},{}",
                result.tikv, result.tidb, result.tiflash
            ));
            assert!(!self.panic_consumption, "reporter panic");
        }
        fn results(&self, snapshot: &crate::statement_ru_result::StatementRUFinalizedSnapshot) {
            self.events
                .borrow_mut()
                .push(format!("results:{}", snapshot.result.total_ru));
            assert!(!self.panic_metrics, "metrics sink panic");
        }
        fn unit(&self, engine: &str, operator: &str, unit: &str, value: f64) {
            self.events
                .borrow_mut()
                .push(format!("{engine}:{operator}:{unit}:{value}"));
        }
        fn statement(&self, status: &str, reason: &str) {
            self.events.borrow_mut().push(format!("{status}:{reason}"));
        }
        fn calibration(
            &self,
            state: crate::statement_ru_result::StatementRUCalibrationState,
            units: StmtUnits,
        ) {
            assert_eq!(
                state,
                crate::statement_ru_result::StatementRUCalibrationState::Incomplete
            );
            assert_eq!(units.cpu_work, 3.0);
            self.events.borrow_mut().push("calibration".into());
            assert!(!self.panic_calibration, "calibration panic");
        }
    }
    let mut calculator = StatementRUCalculator::new(StatementRUCalculationSetup {
        frontend_compile_bytes: 2.0,
        full_report: true,
    });
    calculator.units.cpu_work = 3.0;
    calculator.compute[0].cpu_work = 3.0;
    calculator.report.as_mut().unwrap().add_operator(
        StatementRUEngine::TiDB,
        StatementRUOperator::Projection,
        StmtUnits {
            cpu_work: 3.0,
            ..Default::default()
        },
    );
    let snapshot = calculator.finalize().unwrap();
    let sink = Sink::default();
    publish_statement_ru_finalized_snapshot(&sink, &snapshot);
    assert_eq!(
        *sink.events.borrow(),
        [
            "consumption:0,5,0",
            "results:5",
            "tidb:projection:cpu_work:3",
            "tidb:sql_frontend:frontend_compile_bytes:2",
            "success:incomplete",
            "calibration"
        ]
    );
    let sink = Sink {
        panic_metrics: true,
        ..Default::default()
    };
    publish_statement_ru_finalized_snapshot(&sink, &snapshot);
    assert_eq!(
        *sink.events.borrow(),
        [
            "consumption:0,5,0",
            "results:5",
            "failed:panic",
            "calibration"
        ]
    );
    let sink = Sink {
        panic_consumption: true,
        panic_calibration: true,
        ..Default::default()
    };
    publish_statement_ru_finalized_snapshot(&sink, &snapshot);
    assert_eq!(sink.events.borrow().len(), 6);
    assert!(
        !sink
            .events
            .borrow()
            .iter()
            .any(|event| event == "failed:panic")
    );
    let mut result_only = snapshot.clone();
    result_only.report = None;
    let sink = Sink {
        panic_metrics: true,
        ..Default::default()
    };
    publish_statement_ru_finalized_snapshot(&sink, &result_only);
    assert_eq!(*sink.events.borrow(), ["consumption:0,5,0", "results:5"]);
    crate::statement_ru_result::publish_statement_ru_failure_safely(
        &sink,
        StatementRUFailureReason::Unsupported,
    );
    assert_eq!(
        sink.events.borrow().last().unwrap(),
        "skipped:unsupported_plan"
    );
}

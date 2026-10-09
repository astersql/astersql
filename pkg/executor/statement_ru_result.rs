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

//! Evidence classification for the statement RU v2 calculator.

use astersql_resourcegroup::ruv2::model::{self, StmtResult, StmtUnits, StmtWeights};

use crate::statement_ru_reporting::{
    STATEMENT_RU_TIFLASH_MULTIPLIER, StatementRUComputeUnits, StatementRUEngineResult,
    StatementRUFullReport, statement_ru_engine_result,
};

use astersql_planner_core as plannercore;
use astersql_planner_core_base as base;
use astersql_planner_core_operator_physicalop as physicalop;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatementRUPlanKind {
    Other,
    Write,
    Commit,
    Analyze,
    PointLookup,
}

pub struct StatementRUPlanInfo<'a> {
    pub plan: &'a dyn base::Plan,
    pub kind: StatementRUPlanKind,
    pub sql_type: &'static str,
}

/// Resolve only wrappers whose target is executed, then classify the real plan.
/// A plain EXPLAIN keeps its own plan identity because its target is rendered.
pub fn classify_statement_ru_plan(mut plan: &dyn base::Plan) -> StatementRUPlanInfo<'_> {
    loop {
        if let Some(execute) = plan.as_any().downcast_ref::<plannercore::RuntimeExecute>() {
            plan = execute.Plan.as_ref();
            continue;
        }
        if let Some(explain) = plan.as_any().downcast_ref::<plannercore::RuntimeExplain>() {
            if explain.Analyze {
                plan = explain.TargetPlan.as_ref();
                continue;
            }
        }
        let (kind, sql_type) =
            if let Some(insert) = plan.as_any().downcast_ref::<physicalop::Insert>() {
                (
                    StatementRUPlanKind::Write,
                    if insert.IsReplace {
                        "replace"
                    } else {
                        "insert"
                    },
                )
            } else if plan.as_any().is::<physicalop::Update>() {
                (StatementRUPlanKind::Write, "update")
            } else if plan.as_any().is::<physicalop::Delete>() {
                (StatementRUPlanKind::Write, "delete")
            } else if plan.as_any().is::<plannercore::RuntimeAnalyze>() {
                (StatementRUPlanKind::Analyze, "analyze")
            } else if plan
                .as_any()
                .downcast_ref::<plannercore::RuntimeSimple>()
                .is_some_and(|simple| {
                    simple
                        .Statement
                        .with_node(|node| node.as_any().is::<astersql_parser_ast::CommitStmt>())
                        .unwrap_or(false)
                })
            {
                (StatementRUPlanKind::Commit, "commit")
            } else if plan.as_any().is::<physicalop::PointGetPlan>()
                || plan.as_any().is::<physicalop::BatchPointGetPlan>()
            {
                (StatementRUPlanKind::PointLookup, "select")
            } else {
                (StatementRUPlanKind::Other, "select")
            };
        return StatementRUPlanInfo {
            plan,
            kind,
            sql_type,
        };
    }
}

/// Read the configured statement weights for each finalization. Go keeps the
/// weights in the `ru-v2` config section while RU v2 replaces its legacy model.
pub fn current_statement_ru_weights() -> StmtWeights {
    astersql_config::get_global_config().ruv2.stmt_weights
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatementRUCalibrationState {
    Unknown,
    Complete,
    Incomplete,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StatementRUFinalizedSnapshot {
    pub units: StmtUnits,
    pub result: StmtResult,
    pub engine_ru: StatementRUEngineResult,
    pub report: Option<StatementRUFullReport>,
    pub calibration_state: StatementRUCalibrationState,
    pub sql_type: String,
}

/// Go `statementRUCalculationSetup` is installed for one eligible statement.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatementRUCalculationSetup {
    pub frontend_compile_bytes: f64,
    pub full_report: bool,
}

/// Go `statementRUCalculator` owns only scalar units and optional bounded
/// full-mode report. A plan or runtime-statistics pointer never survives here.
#[derive(Clone, Debug)]
pub struct StatementRUCalculator {
    pub units: StmtUnits,
    pub compute: [StatementRUComputeUnits; 3],
    pub report: Option<StatementRUFullReport>,
}

impl StatementRUCalculator {
    pub fn new(setup: StatementRUCalculationSetup) -> Self {
        Self {
            units: StmtUnits {
                frontend_compile_bytes: setup.frontend_compile_bytes,
                ..StmtUnits::default()
            },
            compute: [StatementRUComputeUnits::default(); 3],
            report: setup.full_report.then(StatementRUFullReport::default),
        }
    }

    /// Calculate with the current config, then apply exactly the Go TiFlash
    /// multiplier and freeze a copy of the full report if requested.
    pub fn finalize(&self) -> Option<StatementRUFinalizedSnapshot> {
        let weights = current_statement_ru_weights();
        let mut result = model::calculate(self.units, weights)?;
        let mut engine_ru = statement_ru_engine_result(self.units, self.compute, weights);
        result.total_ru += engine_ru.tiflash * (STATEMENT_RU_TIFLASH_MULTIPLIER - 1.0);
        engine_ru.tiflash *= STATEMENT_RU_TIFLASH_MULTIPLIER;
        if [
            result.total_ru,
            engine_ru.tidb,
            engine_ru.tikv,
            engine_ru.tiflash,
        ]
        .iter()
        .any(|ru| *ru < 0.0 || !ru.is_finite())
        {
            return None;
        }
        let report = self.report.as_ref().map(|report| {
            let mut frozen = report.clone();
            frozen.add_statement_units(self.units);
            frozen
        });
        Some(StatementRUFinalizedSnapshot {
            units: self.units,
            result,
            engine_ru,
            report,
            calibration_state: StatementRUCalibrationState::Incomplete,
            sql_type: "select".to_owned(),
        })
    }
}

/// Remove the normalized EXPLAIN ANALYZE prefix before charging frontend
/// compile bytes. The Go implementation recognizes only these two forms.
pub fn trim_statement_ru_explain_prefix(normalized_sql: &str) -> &str {
    for prefix in [
        "explain analyze format = ? ",
        "explain analyze format = ru ",
    ] {
        if normalized_sql.len() > prefix.len() && normalized_sql.starts_with(prefix) {
            return &normalized_sql[prefix.len()..];
        }
    }
    normalized_sql
}

/// A value copy of one reader's scan evidence. `Unavailable` means the
/// producer did not provide enough counters; `Invalid` means they contradict
/// each other or yield a non-finite estimate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScanEvidence {
    Invalid,
    Unavailable,
    Valid(f64),
}

/// Estimate scan bytes from total keys and bytes per processed key.
/// Mirrors Go `classifyStatementRUScanEvidence` without retaining runtime
/// statistics or scan-detail pointers after statement finalization.
pub fn classify_scan_evidence(
    total_keys: i64,
    processed_keys: i64,
    processed_bytes: i64,
) -> ScanEvidence {
    if total_keys < 0 || processed_keys < 0 || processed_bytes < 0 {
        return ScanEvidence::Invalid;
    }
    if processed_keys == 0 {
        return if processed_bytes == 0 {
            ScanEvidence::Valid(0.0)
        } else {
            ScanEvidence::Invalid
        };
    }
    if total_keys == 0 || processed_bytes == 0 {
        return ScanEvidence::Unavailable;
    }
    let scan_bytes = processed_bytes as f64 / processed_keys as f64 * total_keys as f64;
    if scan_bytes < 0.0 || !scan_bytes.is_finite() {
        ScanEvidence::Invalid
    } else {
        ScanEvidence::Valid(scan_bytes)
    }
}

/// Go statementRUFrontendCompileBytes: cache hits skip frontend compilation;
/// normalized SQL is used only when a statement-context OriginalSQL is present.
pub fn statement_ru_frontend_compile_bytes(
    node: &crate::adapter::StatementNode,
    found_in_plan_cache: bool,
    original_sql: &str,
    normalized_sql: &str,
) -> f64 {
    if found_in_plan_cache {
        return 0.0;
    }
    let mut sql = node.original_text.as_str();
    if !original_sql.is_empty() {
        let normalized = trim_statement_ru_explain_prefix(normalized_sql);
        if !normalized.is_empty() {
            return normalized.len() as f64;
        }
        if sql.is_empty() {
            sql = original_sql;
        }
    }
    if sql.is_empty() {
        sql = &node.text;
    }
    sql.len() as f64
}

/// Scalar session boundary mirroring Go SessionVars and StmtCtx at install time.
#[derive(Clone, Debug, PartialEq)]
pub struct StatementRUInstallState {
    pub statement_context_present: bool,
    pub is_read_only: bool,
    pub in_select_stmt: bool,
    pub restricted_sql: bool,
    pub request_source_type: String,
    pub ttl_job_id: String,
    pub cursor_exists: bool,
    pub flat_plan_cached: bool,
}

impl Default for StatementRUInstallState {
    fn default() -> Self {
        Self {
            statement_context_present: true,
            is_read_only: false,
            in_select_stmt: false,
            restricted_sql: false,
            request_source_type: String::new(),
            ttl_job_id: String::new(),
            cursor_exists: false,
            flat_plan_cached: false,
        }
    }
}

/// The same three-field TTL identity as Go; an internal source alone is insufficient.
pub fn is_statement_ru_ttl_job(state: &StatementRUInstallState) -> bool {
    state.restricted_sql
        && state.request_source_type == astersql_kv::InternalTxnTTL
        && !state.ttl_job_id.is_empty()
}

pub fn new_statement_ru_calculation_setup(
    plan: Option<&dyn base::Plan>,
    state: Option<&StatementRUInstallState>,
    frontend_compile_bytes: f64,
) -> Option<StatementRUCalculationSetup> {
    let plan = classify_statement_ru_plan(plan?);
    let state = state?;
    let eligible = state.is_read_only
        || state.in_select_stmt
        || matches!(
            plan.kind,
            StatementRUPlanKind::Analyze | StatementRUPlanKind::Write | StatementRUPlanKind::Commit
        );
    if !state.statement_context_present
        || !eligible
        || (state.restricted_sql && !is_statement_ru_ttl_job(state))
        || state.cursor_exists
        || state.flat_plan_cached
    {
        return None;
    }
    Some(StatementRUCalculationSetup {
        frontend_compile_bytes,
        full_report: false,
    })
}

/// Full mode records ineligible user work; restricted work is outside that population.
pub fn install_statement_ru_owner_at_boundary(
    plan: Option<&dyn base::Plan>,
    state: Option<&StatementRUInstallState>,
    frontend_compile_bytes: f64,
    full_report: bool,
) -> (
    Option<std::sync::Arc<crate::statement_ru_plan_walk::StatementRUOwner>>,
    bool,
) {
    let Some(mut setup) = new_statement_ru_calculation_setup(plan, state, frontend_compile_bytes)
    else {
        return (
            None,
            full_report && state.is_some_and(|state| !state.restricted_sql),
        );
    };
    setup.full_report = full_report;
    let state = state.expect("eligible setup has a session");
    (
        Some(std::sync::Arc::new(
            crate::statement_ru_plan_walk::StatementRUOwner::new(
                setup,
                state.restricted_sql,
                is_statement_ru_ttl_job(state),
                state.cursor_exists,
            ),
        )),
        false,
    )
}

pub fn install_statement_ru_owner(stmt: &mut crate::adapter::ExecStmt) {
    let state = stmt.Ctx.StatementRUInstallState(&stmt.StmtNode);
    let full_report = astersql_config::get_global_config().ruv2.report_mode
        == astersql_config::RU_REPORT_MODE_FULL;
    let frontend_compile_bytes =
        if new_statement_ru_calculation_setup(stmt.TypedPlan.as_deref(), state.as_ref(), 0.0)
            .is_some()
        {
            stmt.Ctx.StatementRUFrontendCompileBytes(&stmt.StmtNode)
        } else {
            0.0
        };
    let (owner, ineligible) = install_statement_ru_owner_at_boundary(
        stmt.TypedPlan.as_deref(),
        state.as_ref(),
        frontend_compile_bytes,
        full_report,
    );
    if let Some(owner) = owner {
        stmt.StatementCtx.statement_ru_owner = Some(owner);
    }
    if ineligible {
        stmt.Ctx.StatementRUIneligible();
    }
}

/// External publication boundary. Calculation always uses the frozen real snapshot.
pub trait StatementRUPublicationSink {
    fn consumption(&self, result: StatementRUEngineResult);
    fn results(&self, snapshot: &StatementRUFinalizedSnapshot);
    fn unit(&self, engine: &str, operator: &str, unit: &str, value: f64);
    fn statement(&self, status: &str, reason: &str);
    fn calibration(&self, state: StatementRUCalibrationState, units: StmtUnits);
}

impl StatementRUCalibrationState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Complete => "complete",
            Self::Incomplete => "incomplete",
        }
    }
}

pub fn publish_statement_ru_failure_safely(
    sink: &dyn StatementRUPublicationSink,
    reason: crate::statement_ru_reporting::StatementRUFailureReason,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sink.statement(reason.status(), reason.label())
    }));
}

/// Go protects resource-group, metrics and calibration consumers independently.
pub fn publish_statement_ru_finalized_snapshot(
    sink: &dyn StatementRUPublicationSink,
    snapshot: &StatementRUFinalizedSnapshot,
) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let result = snapshot.engine_ru;
        if result.tidb > 0.0 || result.tikv > 0.0 || result.tiflash > 0.0 {
            sink.consumption(result);
        }
    }));
    let metrics = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        sink.results(snapshot);
        if snapshot.report.is_some() {
            crate::statement_ru_reporting::publish_statement_ru_full_metrics(sink, snapshot);
        }
    }));
    if metrics.is_err() && snapshot.report.is_some() {
        publish_statement_ru_failure_safely(
            sink,
            crate::statement_ru_reporting::StatementRUFailureReason::Panic,
        );
    }
    if snapshot.report.is_some() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sink.calibration(snapshot.calibration_state, snapshot.units)
        }));
    }
}

/// Production sink uses the same session reporter as the existing RU path.
pub struct StatementRUContextSink<'a> {
    pub context: &'a dyn crate::adapter::AdapterRuntime,
    pub ttl_job: bool,
}

impl StatementRUPublicationSink for StatementRUContextSink<'_> {
    fn consumption(&self, result: StatementRUEngineResult) {
        let group = self.context.ResourceGroupName();
        if self.context.RUV2ReporterAvailable() && !group.is_empty() {
            self.context
                .ReportRUV2Consumption(&group, result.tikv, result.tidb, result.tiflash);
        }
    }
    fn results(&self, snapshot: &StatementRUFinalizedSnapshot) {
        let _guard = astersql_metrics::metrics::PACKAGE_INIT_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        astersql_metrics::ru_v2::InitRUV2Metrics();
        if self.ttl_job {
            let counter =
                unsafe { (&*std::ptr::addr_of!(astersql_metrics::ru_v2::RUV2TTLTotal)).clone() };
            counter
                .expect("RU metrics initialized")
                .inc_by(snapshot.result.total_ru);
        }
        astersql_metrics::ru_v2::AddRUV2Results(
            snapshot.engine_ru.tikv,
            snapshot.engine_ru.tidb,
            snapshot.engine_ru.tiflash,
            snapshot.result.total_ru,
            &snapshot.sql_type,
        );
    }
    fn unit(&self, engine: &str, operator: &str, unit: &str, value: f64) {
        let counter = {
            let _guard = astersql_metrics::metrics::PACKAGE_INIT_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            astersql_metrics::ru_v2::InitRUV2Metrics();
            unsafe { (&*std::ptr::addr_of!(astersql_metrics::ru_v2::RUV2Unit)).clone() }
        };
        counter
            .expect("RU metrics initialized")
            .with_label_values(&[engine, operator, unit])
            .inc_by(value);
    }
    fn statement(&self, status: &str, reason: &str) {
        let counter = {
            let _guard = astersql_metrics::metrics::PACKAGE_INIT_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            astersql_metrics::ru_v2::InitRUV2Metrics();
            unsafe { (&*std::ptr::addr_of!(astersql_metrics::ru_v2::RUV2Statements)).clone() }
        };
        counter
            .expect("RU metrics initialized")
            .with_label_values(&[status, reason])
            .inc();
    }
    fn calibration(&self, state: StatementRUCalibrationState, units: StmtUnits) {
        self.context.StatementRUCalibration(state, units);
    }
}

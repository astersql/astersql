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

//! RU v3 statement-local evidence captured at terminal finalization.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use astersql_planner_core::FlatPlanTree;
use astersql_resourcegroup::ruv2::model::StmtUnits;
use astersql_util_execdetails::ruv2_metrics::RUV2Metrics;
use astersql_util_execdetails::ruv2_metrics::tikvutil;

use crate::statement_ru_reporting::{
    StatementRUEngine, StatementRUFailureReason, StatementRUOperator,
};
use crate::statement_ru_result::{StatementRUCalculationSetup, StatementRUCalculator};

/// Go `statementRUFinalOutcome`: first recorded session outcome wins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum StatementRUFinalOutcome {
    Unknown,
    Success,
    Failure,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatementRUOperatorState {
    Unknown,
    Complete,
    Unsupported,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatementRUOperatorResult {
    pub state: StatementRUOperatorState,
    pub output_rows: i64,
}

/// Go `mergeStatementRUOperatorState` gives invalid evidence priority over
/// unsupported occurrences and otherwise requires two complete children.
pub fn merge_statement_ru_operator_state(
    left: StatementRUOperatorState,
    right: StatementRUOperatorState,
) -> StatementRUOperatorState {
    if left == StatementRUOperatorState::Invalid || right == StatementRUOperatorState::Invalid {
        StatementRUOperatorState::Invalid
    } else if left != StatementRUOperatorState::Complete
        || right != StatementRUOperatorState::Complete
    {
        StatementRUOperatorState::Unsupported
    } else {
        StatementRUOperatorState::Complete
    }
}

/// Go `statementRUFailed` and `statementRUTerminalFailure` classify the
/// terminal failure without publishing a result.
pub fn statement_ru_failed(state: StatementRUOperatorState) -> StatementRUFailureReason {
    if state == StatementRUOperatorState::Unsupported {
        StatementRUFailureReason::Unsupported
    } else {
        StatementRUFailureReason::Invalid
    }
}

pub fn statement_ru_terminal_failure(root_eof: bool) -> StatementRUFailureReason {
    if root_eof {
        StatementRUFailureReason::Invalid
    } else {
        StatementRUFailureReason::NotFinished
    }
}

/// A statement-local owner keeps the setup only until its first terminal
/// attempt. The atomics allow the final session outcome and root EOF to be
/// recorded independently, as in the Go owner.
pub struct StatementRUOwner {
    setup: Mutex<Option<StatementRUCalculationSetup>>,
    final_outcome: AtomicU32,
    root_eof: AtomicBool,
    pub restricted_sql_at_install: bool,
    pub ttl_job_at_install: bool,
    pub cursor_at_install: bool,
}

impl StatementRUOwner {
    pub fn new(
        setup: StatementRUCalculationSetup,
        restricted_sql_at_install: bool,
        ttl_job_at_install: bool,
        cursor_at_install: bool,
    ) -> Self {
        Self {
            setup: Mutex::new(Some(setup)),
            final_outcome: AtomicU32::new(StatementRUFinalOutcome::Unknown as u32),
            root_eof: AtomicBool::new(false),
            restricted_sql_at_install,
            ttl_job_at_install,
            cursor_at_install,
        }
    }

    /// Go `RecordStatementRUFinalOutcome` uses CAS so later retries cannot
    /// change the first result. A first failure consumes the owner at once.
    pub fn record_final_outcome(&self, success: bool) -> bool {
        self.record_final_outcome_with_setup(success).0
    }

    /// Return the consumed setup to the publisher on the first failure only.
    pub fn record_final_outcome_with_setup(
        &self,
        success: bool,
    ) -> (bool, Option<StatementRUCalculationSetup>) {
        let outcome = if success {
            StatementRUFinalOutcome::Success
        } else {
            StatementRUFinalOutcome::Failure
        };
        let recorded = self
            .final_outcome
            .compare_exchange(
                StatementRUFinalOutcome::Unknown as u32,
                outcome as u32,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok();
        let setup = if recorded && !success {
            self.take_terminal_setup()
        } else {
            None
        };
        (recorded, setup)
    }

    pub fn final_outcome(&self) -> StatementRUFinalOutcome {
        match self.final_outcome.load(Ordering::Acquire) {
            1 => StatementRUFinalOutcome::Success,
            2 => StatementRUFinalOutcome::Failure,
            _ => StatementRUFinalOutcome::Unknown,
        }
    }

    pub fn record_root_eof(&self) {
        self.root_eof.store(true, Ordering::Release);
    }

    pub fn root_eof(&self) -> bool {
        self.root_eof.load(Ordering::Acquire)
    }

    /// The first terminal caller owns the setup. A second attempt sees None.
    pub fn take_terminal_setup(&self) -> Option<StatementRUCalculationSetup> {
        self.setup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    pub fn abort(&self) {
        self.take_terminal_setup();
    }
}

/// Copy the committed payload; the calculator must not retain live commit details.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StatementRUWriteSnapshot {
    pub keys: i64,
    pub bytes: i64,
}

/// Go `snapshotStatementRUWrites` reads these fields without RUv2 counters.
pub fn snapshot_statement_ru_writes(
    details: Option<&tikvutil::CommitDetails>,
) -> StatementRUWriteSnapshot {
    let Some(details) = details else {
        return StatementRUWriteSnapshot::default();
    };
    StatementRUWriteSnapshot {
        keys: details.WriteKeys as i64,
        bytes: details.WriteSize as i64,
    }
}

/// Go `newStatementRUTerminalCalculator`: root EOF is required, and the
/// statement-wide coprocessor response counter contributes once to NetBytes.
pub fn new_statement_ru_terminal_calculator(
    metrics: Option<&RUV2Metrics>,
    setup: StatementRUCalculationSetup,
    root_eof: bool,
) -> Option<StatementRUCalculator> {
    if !root_eof {
        return None;
    }
    let mut calculator = StatementRUCalculator::new(setup);
    if let Some(metrics) = metrics.filter(|metrics| !metrics.Bypass()) {
        let net_bytes = metrics.TiKVCoprocessorResponseBytes();
        if net_bytes < 0 {
            return None;
        }
        calculator.units.net_bytes = net_bytes as f64;
        if let Some(report) = calculator.report.as_mut() {
            if net_bytes != 0 {
                report.add(
                    StatementRUEngine::TiKV,
                    StatementRUOperator::CopTransport,
                    StmtUnits {
                        net_bytes: net_bytes as f64,
                        ..StmtUnits::default()
                    },
                );
            }
        }
    }
    Some(calculator)
}

/// Go `validateStatementRUFlatTree`: every child must begin exactly after the
/// preceding depth-first subtree, with no duplicate or unreachable entry.
pub fn validate_statement_ru_flat_tree(tree: &FlatPlanTree) -> bool {
    if tree.is_empty() {
        return false;
    }
    matches!(
        validate_statement_ru_flat_subtree(tree, 0, tree.len()),
        Some(next) if next == tree.len()
    )
}

fn validate_statement_ru_flat_subtree(
    tree: &FlatPlanTree,
    operator_index: usize,
    remaining_depth: usize,
) -> Option<usize> {
    let operator = tree.get(operator_index)?;
    if remaining_depth == 0 {
        return None;
    }
    let mut next = operator_index + 1;
    for &child_index in &operator.ChildrenIdx {
        if child_index != next || child_index <= operator_index {
            return None;
        }
        next = validate_statement_ru_flat_subtree(tree, child_index, remaining_depth - 1)?;
    }
    Some(next)
}

/// Go `statementRUSortWork` charges `n * log2(min(n, retained))`, clamping
/// the logarithm argument to two for a one-row TopN.
pub fn statement_ru_sort_work(input_rows: i64, retained_rows: u64) -> f64 {
    if input_rows <= 0 || retained_rows == 0 {
        return 0.0;
    }
    let rows_to_retain = (input_rows as f64).min(retained_rows as f64);
    input_rows as f64 * rows_to_retain.max(2.0).log2()
}

fn add_statement_ru_cpu_work(units: &mut StmtUnits, work: f64) -> bool {
    if work < 0.0 || work.is_nan() || work.is_infinite() {
        return false;
    }
    units.cpu_work += work;
    !units.cpu_work.is_infinite()
}

fn add_statement_ru_scan_bytes(units: &mut StmtUnits, bytes: f64) -> bool {
    if bytes < 0.0 || bytes.is_nan() || bytes.is_infinite() {
        return false;
    }
    units.scan_bytes += bytes;
    !units.scan_bytes.is_infinite()
}

fn add_statement_ru_hash_state_rows(units: &mut StmtUnits, rows: f64) -> bool {
    if rows < 0.0 || rows.is_nan() || rows.is_infinite() {
        return false;
    }
    units.hash_state_rows += rows;
    !units.hash_state_rows.is_infinite()
}

/// Go support matrix: root joins and MPP HashJoin; FullOuter is unsupported.
fn statement_ru_join_contract_for_plan(
    plan: &dyn astersql_planner_core_base::Plan,
) -> Option<(usize, bool)> {
    use astersql_planner_core_operator_physicalop as op;
    let origin = plan.as_any();
    let (base, keys, hash) = if let Some(join) = origin.downcast_ref::<op::PhysicalHashJoin>() {
        (
            &join.BasePhysicalJoin,
            join.EqualConditions.len() + join.NAEqualConditions.len(),
            true,
        )
    } else if let Some(join) = origin.downcast_ref::<op::PhysicalMergeJoin>() {
        (&join.BasePhysicalJoin, join.CompareFuncs.len(), false)
    } else if let Some(join) = op::index_join_base(plan) {
        let keys = if let Some(merge) = origin.downcast_ref::<op::PhysicalIndexMergeJoin>() {
            merge.CompareFuncs.len() + merge.OuterCompareFuncs.len()
        } else if origin.is::<op::PhysicalIndexHashJoin>() || plan.tp(&[]) == "IndexHashJoin" {
            join.OuterHashKeys.len()
        } else if plan.tp(&[]) == "IndexMergeJoin" {
            // A legacy tag cannot supply the two concrete comparison-function lists.
            return None;
        } else {
            join.BasePhysicalJoin.OuterJoinKeys.len()
        };
        (
            &join.BasePhysicalJoin,
            keys + join
                .CompareFilters
                .as_ref()
                .map_or(0, |filters| filters.OpType.len()),
            false,
        )
    } else {
        return None;
    };
    if base.JoinType == astersql_planner_core_base::JoinType::FullOuterJoin {
        return None;
    }
    Some((
        keys + base.LeftConditions.len() + base.RightConditions.len() + base.OtherConditions.len(),
        hash,
    ))
}

/// Missing producers contribute zero, as in Go; invalid published state fails.
fn collect_statement_ru_hash_state_rows(
    stats: Option<&StatementRUPlanEvidence>,
    mpp: bool,
    units: &mut StmtUnits,
) -> StatementRUOperatorState {
    use StatementRUOperatorState::*;
    let rows = if mpp {
        match stats.and_then(|stats| stats.tiflash) {
            Some(stats) if stats.Invalid => return Invalid,
            Some(stats) => stats.HashDistinctEntries as f64 + stats.HashBuildRows as f64,
            None => 0.0,
        }
    } else {
        match stats.and_then(|stats| stats.hash_state_rows) {
            Some(stats) if stats.Invalid() => return Invalid,
            Some(stats) => stats.Rows as f64,
            None => 0.0,
        }
    };
    if add_statement_ru_hash_state_rows(units, rows) {
        Complete
    } else {
        Invalid
    }
}

pub(crate) fn collect_statement_ru_join_units(
    operator: &astersql_planner_core::TypedFlatOperator<'_>,
    children: &[StatementRUOperatorResult],
    output_rows: i64,
    stats: Option<&StatementRUPlanEvidence>,
    mpp: bool,
    calculator: &mut StatementRUCalculator,
) -> StatementRUOperatorState {
    use StatementRUOperatorState::*;
    if (!operator.IsRoot && !mpp)
        || (mpp
            && !operator
                .Origin
                .as_any()
                .is::<astersql_planner_core_operator_physicalop::PhysicalHashJoin>())
    {
        return Unsupported;
    }
    if children.len() != 2 {
        return Invalid;
    }
    let Some((count, hash)) = statement_ru_join_contract_for_plan(operator.Origin) else {
        return Unsupported;
    };
    let mut delta = StmtUnits::default();
    let input = children[0].output_rows as f64 + children[1].output_rows as f64;
    if !add_statement_ru_cpu_work(&mut delta, input * count as f64)
        || !add_statement_ru_join_output_rows(&mut delta, output_rows as f64)
    {
        return Invalid;
    }
    if hash {
        let state = collect_statement_ru_hash_state_rows(stats, mpp, &mut delta);
        if state != Complete {
            return state;
        }
    }
    if merge_statement_ru_unit_delta(calculator, delta) {
        Complete
    } else {
        Invalid
    }
}

pub(crate) fn collect_statement_ru_aggregation_units(
    operator: &astersql_planner_core::TypedFlatOperator<'_>,
    children: &[StatementRUOperatorResult],
    output_rows: i64,
    stats: Option<&StatementRUPlanEvidence>,
    mpp: bool,
    supported_site: bool,
    calculator: &mut StatementRUCalculator,
) -> StatementRUOperatorState {
    use StatementRUOperatorState::*;
    use astersql_planner_core_operator_physicalop as op;
    if !supported_site {
        return Unsupported;
    }
    if children.len() != 1 {
        return Invalid;
    }
    let origin = operator.Origin.as_any();
    let (agg, hash) = if let Some(agg) = origin.downcast_ref::<op::PhysicalHashAgg>() {
        (&agg.BasePhysicalAgg, true)
    } else if let Some(agg) = origin.downcast_ref::<op::PhysicalStreamAgg>() {
        (&agg.BasePhysicalAgg, false)
    } else {
        return Unsupported;
    };
    let mut delta = StmtUnits::default();
    if !add_statement_ru_cpu_work(
        &mut delta,
        children[0].output_rows as f64 * (agg.GroupByItems.len() + agg.AggFuncs.len()) as f64,
    ) {
        return Invalid;
    }
    if hash {
        if operator.IsRoot || mpp {
            let state = collect_statement_ru_hash_state_rows(stats, mpp, &mut delta);
            if state != Complete {
                return state;
            }
        } else if !add_statement_ru_hash_state_rows(&mut delta, output_rows as f64) {
            return Invalid;
        }
    }
    if merge_statement_ru_unit_delta(calculator, delta) {
        Complete
    } else {
        Invalid
    }
}

fn add_statement_ru_join_output_rows(units: &mut StmtUnits, rows: f64) -> bool {
    if rows < 0.0 || rows.is_nan() || rows.is_infinite() {
        return false;
    }
    units.join_output_rows += rows;
    !units.join_output_rows.is_infinite()
}

/// Go `mergeStatementRUUnitDelta` validates an entire occurrence before
/// publishing any of its four mutable units to the statement calculator.
pub fn merge_statement_ru_unit_delta(
    calculator: &mut StatementRUCalculator,
    delta: StmtUnits,
) -> bool {
    let mut merged = calculator.units;
    if !add_statement_ru_cpu_work(&mut merged, delta.cpu_work)
        || !add_statement_ru_scan_bytes(&mut merged, delta.scan_bytes)
        || !add_statement_ru_hash_state_rows(&mut merged, delta.hash_state_rows)
        || !add_statement_ru_join_output_rows(&mut merged, delta.join_output_rows)
    {
        return false;
    }
    calculator.units = merged;
    true
}

/// Terminal copies retain coverage separately from scalar zero values.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatementRUPointSnapshot {
    pub total_keys: i64,
    pub processed_keys: i64,
    pub processed_bytes: i64,
    pub payload_bytes: u64,
    pub valid: bool,
    pub payload_complete: bool,
    pub scan_detail_complete: bool,
}
impl StatementRUPointSnapshot {
    pub fn state(&self) -> StatementRUOperatorState {
        if !self.valid || self.total_keys < 0 || self.processed_keys < 0 || self.processed_bytes < 0
        {
            return StatementRUOperatorState::Invalid;
        }
        if !self.payload_complete {
            return if self.payload_bytes != 0
                || self.total_keys != 0
                || self.processed_keys != 0
                || self.processed_bytes != 0
            {
                StatementRUOperatorState::Invalid
            } else {
                StatementRUOperatorState::Complete
            };
        }
        if !self.scan_detail_complete {
            return StatementRUOperatorState::Unsupported;
        }
        match crate::statement_ru_result::classify_scan_evidence(
            self.total_keys,
            self.processed_keys,
            self.processed_bytes,
        ) {
            crate::statement_ru_result::ScanEvidence::Invalid => StatementRUOperatorState::Invalid,
            _ => StatementRUOperatorState::Complete,
        }
    }
}

#[derive(Clone)]
pub struct StatementRUPlanEvidence {
    pub plan_id: i32,
    pub root_rows: astersql_util_execdetails::execdetails::RootRowsSnapshot,
    pub write_cpu_work: Option<f64>,
    pub analyze_scan_bytes: Option<f64>,
    pub hash_state_rows: Option<astersql_util_execdetails::execdetails::HashStateRowsSnapshot>,
    pub cop_rows: astersql_util_execdetails::execdetails::CopRowsSnapshot,
    pub scan: Option<astersql_util_execdetails::execdetails::util::ScanDetail>,
    pub tiflash: Option<astersql_util_execdetails::execdetails::TiFlashExecutionUnits>,
}
#[derive(Clone, Default)]
pub struct StatementRURuntimeEvidence {
    pub plans: Vec<StatementRUPlanEvidence>,
    pub point: Option<StatementRUPointSnapshot>,
    /// Response snapshots keyed by occurrence plan ID for independent lookups.
    pub points: Vec<(i32, StatementRUPointSnapshot)>,
    pub writes: Option<StatementRUWriteSnapshot>,
    pub tikv_response_bytes: Option<i64>,
    pub frontend_compile_bytes: f64,
}
impl StatementRURuntimeEvidence {
    /// A missing producer is distinct from a valid zero-response provider.
    pub fn point_state(&self) -> StatementRUOperatorState {
        self.point.as_ref().map_or(
            StatementRUOperatorState::Unsupported,
            StatementRUPointSnapshot::state,
        )
    }
    pub fn tikv_response_state(&self) -> StatementRUOperatorState {
        match self.tikv_response_bytes {
            Some(bytes) if bytes < 0 => StatementRUOperatorState::Invalid,
            Some(_) => StatementRUOperatorState::Complete,
            None => StatementRUOperatorState::Unsupported,
        }
    }
}
/// Copy evidence without retaining collectors, RPC responses or commit details.
pub fn snapshot_statement_ru_runtime_evidence(
    collector: Option<&astersql_util_execdetails::execdetails::RuntimeStatsColl>,
    plan_ids: &[i32],
    point: Option<StatementRUPointSnapshot>,
    writes: Option<StatementRUWriteSnapshot>,
    metrics: Option<&RUV2Metrics>,
) -> StatementRURuntimeEvidence {
    let mut seen = std::collections::HashSet::new();
    let plans = collector
        .map(|collector| {
            plan_ids
                .iter()
                .copied()
                .filter(|id| seen.insert(*id))
                .map(|plan_id| {
                    let (units, found) = collector.GetTiFlashExecutionUnits(plan_id);
                    StatementRUPlanEvidence {
                        plan_id,
                        root_rows: collector.GetRootRowsSnapshot(plan_id),
                        write_cpu_work: collector.GetRootWriteCPUWork(plan_id),
                        analyze_scan_bytes: collector.GetAnalyzeScanBytes(plan_id),
                        hash_state_rows: collector.GetRootHashStateRowsSnapshot(plan_id),
                        cop_rows: collector.GetCopRowsSnapshot(plan_id),
                        scan: collector.GetObservedCopScanDetail(plan_id),
                        tiflash: found.then_some(units),
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    StatementRURuntimeEvidence {
        plans,
        point,
        points: Vec::new(),
        writes,
        tikv_response_bytes: metrics
            .filter(|metrics| !metrics.Bypass())
            .map(RUV2Metrics::TiKVCoprocessorResponseBytes),
        frontend_compile_bytes: 0.0,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatementRUForestKind {
    Main,
    CTE,
    ScalarSubQuery,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatementRUExplainOperatorResult {
    pub self_ru: f64,
    pub cum_ru: f64,
}

#[derive(Default)]
pub struct StatementRUExplainResult {
    pub main: Vec<StatementRUExplainOperatorResult>,
    pub ctes: Vec<Vec<StatementRUExplainOperatorResult>>,
    pub scalar_subqueries: Vec<Vec<StatementRUExplainOperatorResult>>,
}

pub fn statement_ru_explain_tree(
    result: Option<&mut StatementRUExplainResult>,
    kind: StatementRUForestKind,
    ordinal: usize,
) -> Option<&mut [StatementRUExplainOperatorResult]> {
    let result = result?;
    match kind {
        StatementRUForestKind::Main => Some(result.main.as_mut_slice()),
        StatementRUForestKind::CTE => result.ctes.get_mut(ordinal).map(Vec::as_mut_slice),
        StatementRUForestKind::ScalarSubQuery => result
            .scalar_subqueries
            .get_mut(ordinal)
            .map(Vec::as_mut_slice),
    }
}

/// Evaluate each independent tree once, in Go forest order. Only a complete
/// forest can escape as a finalized value; partial child charges stay private.
pub fn calculate_statement_ru_forest(
    forest: &astersql_planner_core::TypedFlatPhysicalPlan<'_>,
    evidence: &StatementRURuntimeEvidence,
    setup: StatementRUCalculationSetup,
    root_eof: bool,
) -> Result<crate::statement_ru_result::StatementRUFinalizedSnapshot, StatementRUOperatorState> {
    calculate_statement_ru_forest_with_operators(forest, evidence, setup, root_eof, None)
}

pub fn calculate_statement_ru_forest_with_operators(
    forest: &astersql_planner_core::TypedFlatPhysicalPlan<'_>,
    evidence: &StatementRURuntimeEvidence,
    setup: StatementRUCalculationSetup,
    root_eof: bool,
    mut results: Option<&mut StatementRUExplainResult>,
) -> Result<crate::statement_ru_result::StatementRUFinalizedSnapshot, StatementRUOperatorState> {
    use StatementRUOperatorState::{Complete, Invalid};
    if !root_eof {
        return Err(Invalid);
    }
    // A statement-wide legacy point snapshot cannot be charged to multiple
    // independent point occurrences. Require keyed producer evidence there.
    let point_count = std::iter::once(&forest.Main)
        .chain(forest.CTEs.iter())
        .chain(forest.ScalarSubQueries.iter())
        .flatten()
        .filter(|op| {
            op.Origin
                .as_any()
                .is::<astersql_planner_core_operator_physicalop::PointGetPlan>()
                || op
                    .Origin
                    .as_any()
                    .is::<astersql_planner_core_operator_physicalop::BatchPointGetPlan>()
        })
        .count();
    if point_count > 1 && evidence.point.is_some() && evidence.points.is_empty() {
        return Err(StatementRUOperatorState::Unsupported);
    }
    let mut calculator = StatementRUCalculator::new(setup);
    if let Some(bytes) = evidence.tikv_response_bytes {
        if bytes < 0 {
            return Err(Invalid);
        }
        calculator.units.net_bytes = bytes as f64;
        if bytes != 0 {
            if let Some(report) = calculator.report.as_mut() {
                report.add(
                    StatementRUEngine::TiKV,
                    StatementRUOperator::CopTransport,
                    StmtUnits {
                        net_bytes: bytes as f64,
                        ..Default::default()
                    },
                );
            }
        }
    }
    let Some(root) = forest.Main.first() else {
        return Err(Invalid);
    };
    let plan_info = crate::statement_ru_result::classify_statement_ru_plan(root.Origin);
    use crate::statement_ru_result::StatementRUPlanKind;
    if matches!(
        plan_info.kind,
        StatementRUPlanKind::Write | StatementRUPlanKind::Commit
    ) {
        let writes = evidence.writes.unwrap_or_default();
        calculator.units.write_keys = writes.keys as f64;
        calculator.units.write_bytes = writes.bytes as f64;
    }
    if plan_info.kind == StatementRUPlanKind::Write {
        calculator.units.write_statement = 1.0;
    }
    let root_owned_units = calculator.units;
    for (kind, trees) in [
        (
            StatementRUForestKind::Main,
            std::slice::from_ref(&forest.Main),
        ),
        (StatementRUForestKind::CTE, forest.CTEs.as_slice()),
        (
            StatementRUForestKind::ScalarSubQuery,
            forest.ScalarSubQueries.as_slice(),
        ),
    ] {
        for (ordinal, tree) in trees.iter().enumerate() {
            let operator_results = statement_ru_explain_tree(results.as_deref_mut(), kind, ordinal);
            let result = calculate_statement_ru_plan_with_operators(
                tree,
                0,
                evidence,
                &mut calculator,
                if kind == StatementRUForestKind::Main {
                    root_owned_units
                } else {
                    StmtUnits::default()
                },
                operator_results,
            );
            if result.state != Complete {
                return Err(result.state);
            }
        }
    }
    let mut finalized = calculator.finalize().ok_or(Invalid)?;
    finalized.sql_type = plan_info.sql_type.to_owned();
    Ok(finalized)
}

/// Canonical preorder edges are validated before any occurrence is evaluated.
pub fn validate_statement_ru_typed_flat_tree(
    tree: &[astersql_planner_core::TypedFlatOperator<'_>],
) -> bool {
    fn subtree(
        tree: &[astersql_planner_core::TypedFlatOperator<'_>],
        index: usize,
        depth: usize,
    ) -> Option<usize> {
        let operator = tree.get(index)?;
        if depth == 0 {
            return None;
        }
        let mut next = index + 1;
        for &child in &operator.ChildrenIdx {
            if child != next || child <= index {
                return None;
            }
            next = subtree(tree, child, depth - 1)?;
        }
        Some(next)
    }
    !tree.is_empty() && subtree(tree, 0, tree.len()) == Some(tree.len())
}

pub fn calculate_statement_ru_plan(
    tree: &[astersql_planner_core::TypedFlatOperator<'_>],
    index: usize,
    evidence: &StatementRURuntimeEvidence,
    calculator: &mut StatementRUCalculator,
) -> StatementRUOperatorResult {
    if !validate_statement_ru_typed_flat_tree(tree) {
        return StatementRUOperatorResult {
            state: StatementRUOperatorState::Invalid,
            output_rows: 0,
        };
    }
    calculate_statement_ru_plan_with_operators(
        tree,
        index,
        evidence,
        calculator,
        StmtUnits::default(),
        None,
    )
}

/// Optional occurrence results retain the original preorder indexes even though
/// evaluation is child-first. Statement-wide units belong only to the main root.
pub fn calculate_statement_ru_plan_with_operators(
    tree: &[astersql_planner_core::TypedFlatOperator<'_>],
    index: usize,
    evidence: &StatementRURuntimeEvidence,
    calculator: &mut StatementRUCalculator,
    root_owned_units: StmtUnits,
    results: Option<&mut [StatementRUExplainOperatorResult]>,
) -> StatementRUOperatorResult {
    if !validate_statement_ru_typed_flat_tree(tree)
        || results.as_ref().is_some_and(|r| r.len() != tree.len())
    {
        return StatementRUOperatorResult {
            state: StatementRUOperatorState::Invalid,
            output_rows: 0,
        };
    }
    calculate_statement_ru_plan_child_first(
        tree,
        index,
        evidence,
        calculator,
        tree.len(),
        root_owned_units,
        results,
    )
}

fn calculate_statement_ru_plan_child_first(
    tree: &[astersql_planner_core::TypedFlatOperator<'_>],
    index: usize,
    evidence: &StatementRURuntimeEvidence,
    calculator: &mut StatementRUCalculator,
    depth: usize,
    root_owned_units: StmtUnits,
    mut results: Option<&mut [StatementRUExplainOperatorResult]>,
) -> StatementRUOperatorResult {
    use StatementRUOperatorState::{Complete, Invalid, Unsupported};
    use astersql_planner_core::TypedOperatorLabel as Label;
    use astersql_planner_core_operator_physicalop as op;
    let failed = |state| StatementRUOperatorResult {
        state,
        output_rows: 0,
    };
    let Some(operator) = tree.get(index).filter(|_| depth > 0) else {
        return failed(Invalid);
    };
    let before_subtree = calculator.units;
    let explain_weights = results
        .as_ref()
        .map(|_| crate::statement_ru_result::current_statement_ru_weights());
    let before_subtree_tiflash_ru = explain_weights.map_or(0.0, |weights| {
        crate::statement_ru_reporting::statement_ru_tiflash_ru(
            calculator.compute[StatementRUEngine::TiFlash as usize],
            weights,
        )
    });
    let mut children = Vec::with_capacity(operator.ChildrenIdx.len());
    let mut child_state = Complete;
    for &child in &operator.ChildrenIdx {
        let result = calculate_statement_ru_plan_child_first(
            tree,
            child,
            evidence,
            calculator,
            depth - 1,
            root_owned_units,
            results.as_deref_mut(),
        );
        child_state = merge_statement_ru_operator_state(child_state, result.state);
        children.push(result);
    }
    if child_state != Complete {
        return failed(child_state);
    }
    let stats = evidence
        .plans
        .iter()
        .find(|stats| stats.plan_id == operator.Origin.id());
    let rows = if let Some(stats) = stats {
        if operator.IsRoot {
            if stats.root_rows.Invalid() {
                return failed(Invalid);
            }
            stats.root_rows.Rows
        } else if operator.StoreType == astersql_kv::StoreType::TiFlash
            && operator.ReqType == op::ReadReqType::MPP
        {
            let snapshot = stats.tiflash.as_ref();
            if snapshot.is_some_and(|s| s.Invalid || s.Rows > i64::MAX as u64) {
                return failed(Invalid);
            }
            snapshot.map_or(0, |s| s.Rows as i64)
        } else {
            if stats.cop_rows.Invalid {
                return failed(Invalid);
            }
            stats.cop_rows.Rows
        }
    } else {
        0
    };
    if rows < 0 {
        return failed(Invalid);
    }
    let before = calculator.units;
    let before_operator_tiflash_ru = explain_weights.map_or(0.0, |weights| {
        crate::statement_ru_reporting::statement_ru_tiflash_ru(
            calculator.compute[StatementRUEngine::TiFlash as usize],
            weights,
        )
    });
    let origin = operator.Origin.as_any();
    let category;
    let mpp = statement_ru_operator_runs_at_mpp(operator);
    let supported_site = statement_ru_operator_runs_at_supported_site(operator);
    let input_rows = children.first().map_or(0, |child| child.output_rows);
    let cpu = |calculator: &mut StatementRUCalculator, work| {
        if add_statement_ru_cpu_work(&mut calculator.units, work) {
            Complete
        } else {
            Invalid
        }
    };
    let state = if origin.is::<op::Insert>()
        || origin.is::<op::Update>()
        || origin.is::<op::Delete>()
    {
        category = StatementRUOperator::Write;
        if !operator.IsRoot {
            Unsupported
        } else if let Some(work) = stats.and_then(|stats| stats.write_cpu_work) {
            cpu(calculator, work)
        } else {
            Unsupported
        }
    } else if let Some(plan) = origin.downcast_ref::<astersql_planner_core::RuntimeSimple>() {
        category = StatementRUOperator::Wrapper;
        if operator.IsRoot
            && children.is_empty()
            && plan
                .Statement
                .with_node(|node| node.as_any().is::<astersql_parser_ast::CommitStmt>())
                .unwrap_or(false)
        {
            Complete
        } else {
            Unsupported
        }
    } else if origin.is::<astersql_planner_core::RuntimeAnalyze>() {
        category = StatementRUOperator::Analyze;
        if !operator.IsRoot || !children.is_empty() {
            Unsupported
        } else if let Some(bytes) = stats.and_then(|stats| stats.analyze_scan_bytes) {
            if add_statement_ru_scan_bytes(&mut calculator.units, bytes) {
                Complete
            } else {
                Invalid
            }
        } else {
            Complete
        }
    } else if origin.is::<op::PhysicalHashJoin>()
        || origin.is::<op::PhysicalMergeJoin>()
        || op::index_join_base(operator.Origin).is_some()
    {
        category = if origin.is::<op::PhysicalHashJoin>() {
            StatementRUOperator::HashJoin
        } else if origin.is::<op::PhysicalMergeJoin>() {
            StatementRUOperator::MergeJoin
        } else {
            StatementRUOperator::LookupJoin
        };
        collect_statement_ru_join_units(operator, &children, rows, stats, mpp, calculator)
    } else if origin.is::<op::PhysicalHashAgg>() || origin.is::<op::PhysicalStreamAgg>() {
        category = if origin.is::<op::PhysicalHashAgg>() {
            StatementRUOperator::HashAgg
        } else {
            StatementRUOperator::StreamAgg
        };
        collect_statement_ru_aggregation_units(
            operator,
            &children,
            rows,
            stats,
            mpp,
            supported_site,
            calculator,
        )
    } else if let Some(plan) = origin.downcast_ref::<op::PhysicalProjection>() {
        category = StatementRUOperator::Projection;
        if !supported_site || children.len() != 1 {
            Unsupported
        } else {
            cpu(calculator, input_rows as f64 * plan.Exprs.len() as f64)
        }
    } else if let Some(plan) = origin.downcast_ref::<op::PhysicalSelection>() {
        category = StatementRUOperator::Selection;
        if !supported_site || children.len() != 1 {
            Unsupported
        } else {
            cpu(calculator, input_rows as f64 * plan.Conditions.len() as f64)
        }
    } else if origin.is::<op::PhysicalLimit>() || origin.is::<op::PhysicalMaxOneRow>() {
        category = StatementRUOperator::Limit;
        let site = if origin.is::<op::PhysicalMaxOneRow>() {
            operator.IsRoot
        } else {
            supported_site
        };
        if !site || children.len() != 1 {
            Unsupported
        } else {
            cpu(calculator, input_rows as f64)
        }
    } else if origin.is::<op::PhysicalUnionScan>() {
        category = StatementRUOperator::UnionScan;
        if !operator.IsRoot {
            Unsupported
        } else if children.len() != 1 {
            Invalid
        } else {
            cpu(calculator, input_rows as f64)
        }
    } else if let Some(plan) = origin.downcast_ref::<op::PhysicalWindow>() {
        category = StatementRUOperator::Window;
        if !operator.IsRoot && !mpp {
            Unsupported
        } else if children.len() != 1 {
            Invalid
        } else {
            let mut count =
                plan.WindowFuncDescs.len() + plan.PartitionBy.len() + plan.OrderBy.len();
            if let Some(frame) = &plan.Frame {
                for bound in [&frame.Start, &frame.End].into_iter().flatten() {
                    count += bound.CalcFuncs.len();
                }
            }
            cpu(calculator, input_rows as f64 * count as f64)
        }
    } else if let Some(plan) = origin.downcast_ref::<op::PhysicalSort>() {
        category = StatementRUOperator::Sort;
        if (!operator.IsRoot && !mpp) || children.len() != 1 {
            Unsupported
        } else {
            debug_assert!(
                plan.ByItems
                    .iter()
                    .all(|item| item.Expr.as_scalar_function().is_none()),
                "statement RU expects materialized ordering expressions"
            );
            cpu(
                calculator,
                statement_ru_sort_work(input_rows, input_rows as u64),
            )
        }
    } else if let Some(plan) = origin.downcast_ref::<op::PhysicalTopN>() {
        category = StatementRUOperator::TopN;
        if !supported_site || children.len() != 1 {
            Unsupported
        } else {
            debug_assert!(
                plan.ByItems
                    .iter()
                    .all(|item| item.Expr.as_scalar_function().is_none()),
                "statement RU expects materialized ordering expressions"
            );
            let retained = if operator.IsRoot {
                if plan.Count == 0 {
                    Some(0)
                } else {
                    plan.Offset.checked_add(plan.Count)
                }
            } else if plan.Offset != 0 {
                return failed(Unsupported);
            } else {
                Some(plan.Count)
            };
            match retained {
                Some(retained) => cpu(calculator, statement_ru_sort_work(input_rows, retained)),
                None => Invalid,
            }
        }
    } else if let Some(reader) = origin.downcast_ref::<op::PhysicalTableReader>() {
        category = StatementRUOperator::Reader;
        if operator.IsRoot
            && reader.StoreType == astersql_kv::StoreType::TiFlash
            && reader.ReadReqType == op::ReadReqType::MPP
            && children.len() == 1
        {
            collect_statement_ru_mpp_scan_bytes(tree, operator.ChildrenIdx[0], evidence, calculator)
        } else if !operator.IsRoot
            || reader.StoreType != astersql_kv::StoreType::TiKV
            || reader.ReadReqType != op::ReadReqType::Cop
            || reader.TablePlan.is_none()
            || operator.ChildrenIdx.len() != 1
        {
            Unsupported
        } else {
            collect_statement_ru_reader_scan_bytes(
                tree,
                operator,
                evidence,
                calculator,
                &[reader.TablePlan.as_deref().unwrap() as &dyn astersql_planner_core_base::Plan],
            )
        }
    } else if let Some(reader) = origin.downcast_ref::<op::PhysicalIndexReader>() {
        category = StatementRUOperator::Reader;
        if !operator.IsRoot || reader.IndexPlan.is_none() || operator.ChildrenIdx.len() != 1 {
            Unsupported
        } else {
            collect_statement_ru_reader_scan_bytes(
                tree,
                operator,
                evidence,
                calculator,
                &[reader.IndexPlan.as_deref().unwrap() as &dyn astersql_planner_core_base::Plan],
            )
        }
    } else if let Some(reader) = origin.downcast_ref::<op::PhysicalIndexLookUpReader>() {
        category = StatementRUOperator::LookupReader;
        if !operator.IsRoot
            || reader.IndexLookUpPushDown
            || reader.IndexPlan.is_none()
            || reader.TablePlan.is_none()
            || operator.ChildrenIdx.len() != 2
        {
            Unsupported
        } else {
            let index_child = &tree[operator.ChildrenIdx[0]];
            let table_child = &tree[operator.ChildrenIdx[1]];
            if index_child.Label != Label::BuildSide
                || index_child.IsINLProbeChild
                || table_child.Label != Label::ProbeSide
                || !table_child.IsINLProbeChild
            {
                Unsupported
            } else {
                collect_statement_ru_reader_scan_bytes(
                    tree,
                    operator,
                    evidence,
                    calculator,
                    &[
                        reader.IndexPlan.as_deref().unwrap()
                            as &dyn astersql_planner_core_base::Plan,
                        reader.TablePlan.as_deref().unwrap()
                            as &dyn astersql_planner_core_base::Plan,
                    ],
                )
            }
        }
    } else if let Some(reader) = origin.downcast_ref::<op::PhysicalIndexMergeReader>() {
        category = StatementRUOperator::LookupReader;
        if !operator.IsRoot || reader.TablePlan.is_none() {
            Unsupported
        } else {
            let mut roots: Vec<&dyn astersql_planner_core_base::Plan> = reader
                .PartialPlansRaw
                .iter()
                .map(|p| p.as_ref() as &dyn astersql_planner_core_base::Plan)
                .collect();
            roots.push(reader.TablePlan.as_deref().unwrap() as &dyn astersql_planner_core_base::Plan);
            collect_statement_ru_reader_scan_bytes(tree, operator, evidence, calculator, &roots)
        }
    } else if origin.is::<op::PointGetPlan>() || origin.is::<op::BatchPointGetPlan>() {
        category = StatementRUOperator::PointLookup;
        if !operator.IsRoot || !operator.ChildrenIdx.is_empty() {
            Unsupported
        } else if calculator.units.write_statement != 0.0 {
            Complete
        } else {
            collect_statement_ru_point_lookup_evidence(
                evidence
                    .points
                    .iter()
                    .find(|(id, _)| *id == operator.Origin.id())
                    .map(|(_, point)| point)
                    .or_else(|| {
                        evidence
                            .points
                            .is_empty()
                            .then_some(evidence.point.as_ref())
                            .flatten()
                    }),
                calculator,
            )
        }
    } else if origin.is::<op::PhysicalExchangeSender>()
        || origin.is::<op::PhysicalExchangeReceiver>()
    {
        category = StatementRUOperator::Wrapper;
        if !mpp || children.len() != 1 {
            Unsupported
        } else if origin.is::<op::PhysicalExchangeSender>() {
            collect_statement_ru_mpp_network(stats, calculator)
        } else {
            Complete
        }
    } else if let Some(shuffle) = origin.downcast_ref::<op::PhysicalShuffle>() {
        category = StatementRUOperator::Shuffle;
        if !operator.IsRoot {
            Unsupported
        } else {
            collect_statement_ru_shuffle_units(shuffle, evidence, calculator)
        }
    } else if origin.is::<op::PhysicalTableScan>() || origin.is::<op::PhysicalIndexScan>() {
        category = StatementRUOperator::RangeScan;
        if mpp {
            if origin
                .downcast_ref::<op::PhysicalTableScan>()
                .is_none_or(|scan| !scan.UsedColumnarIndexes.is_empty())
                || !children.is_empty()
            {
                Unsupported
            } else {
                collect_statement_ru_mpp_network(stats, calculator)
            }
        } else if operator.IsRoot
            || !operator.ChildrenIdx.is_empty()
            || operator.StoreType != astersql_kv::StoreType::TiKV
            || operator.ReqType != op::ReadReqType::Cop
        {
            Unsupported
        } else {
            Complete
        }
    } else {
        category = StatementRUOperator::Wrapper;
        // Ownership wrappers retain each occurrence's observed output rows;
        // their producer/child trees own the work and must not be multiplied.
        let supported = if origin.is::<op::PhysicalTableDual>()
            || origin.is::<op::PhysicalMemTable>()
            || origin.is::<op::PhysicalCTE>()
            || origin.is::<op::PhysicalCTETable>()
        {
            operator.IsRoot && operator.ChildrenIdx.is_empty()
        } else if origin.is::<op::PhysicalUnionAll>() || origin.is::<op::PhysicalSequence>() {
            operator.IsRoot && !operator.ChildrenIdx.is_empty()
        } else if origin.is::<astersql_planner_core::ScalarSubqueryEvalCtx>()
            || origin.is::<op::PhysicalLock>()
        {
            operator.IsRoot && operator.ChildrenIdx.len() == 1
        } else if origin.is::<op::PhysicalShuffleReceiverStub>() {
            operator.IsRoot
        } else if origin.is::<op::PhysicalApply>() {
            operator.IsRoot && operator.ChildrenIdx.len() == 2
        } else if origin.is::<op::PhysicalCTEDefinition>() {
            operator.IsRoot
                && (1..=2).contains(&operator.ChildrenIdx.len())
                && tree[operator.ChildrenIdx[0]].Label == Label::SeedPart
                && (operator.ChildrenIdx.len() == 1
                    || tree[operator.ChildrenIdx[1]].Label == Label::RecursivePart)
        } else {
            false
        };
        if supported { Complete } else { Unsupported }
    };
    if state != Complete {
        return failed(state);
    }
    calculator.units.operator_num += 1.0;
    let engine = if mpp {
        StatementRUEngine::TiFlash
    } else if operator.IsRoot {
        StatementRUEngine::TiDB
    } else {
        StatementRUEngine::TiKV
    };
    let delta = calculator.units.sub(before);
    let compute = &mut calculator.compute[engine as usize];
    compute.cpu_work += delta.cpu_work;
    compute.hash_state_rows += delta.hash_state_rows;
    compute.operator_num += 1.0;
    compute.join_output_rows += delta.join_output_rows;
    compute.net_bytes += delta.net_bytes;
    compute.cross_az_net_bytes += delta.cross_az_net_bytes;
    if let Some(report) = calculator.report.as_mut() {
        let mut report_units = delta;
        if origin
            .downcast_ref::<op::PhysicalTableReader>()
            .is_some_and(|reader| reader.StoreType == astersql_kv::StoreType::TiFlash)
        {
            report.add(
                StatementRUEngine::TiFlash,
                StatementRUOperator::Reader,
                StmtUnits {
                    scan_bytes: report_units.scan_bytes,
                    ..Default::default()
                },
            );
            report_units.scan_bytes = 0.0;
        }
        report.add_operator(engine, category, report_units);
    }
    if let Some(results) = results {
        let mut self_units = delta;
        let mut cum_units = calculator.units.sub(before_subtree);
        if index == 0 {
            self_units = self_units.add(root_owned_units);
            cum_units = cum_units.add(root_owned_units);
        }
        let weights = explain_weights.expect("EXPLAIN weights captured with results");
        let Some(self_result) = astersql_resourcegroup::ruv2::model::calculate(self_units, weights)
        else {
            return failed(Invalid);
        };
        let Some(cum_result) = astersql_resourcegroup::ruv2::model::calculate(cum_units, weights)
        else {
            return failed(Invalid);
        };
        let tiflash_ru = crate::statement_ru_reporting::statement_ru_tiflash_ru(
            calculator.compute[StatementRUEngine::TiFlash as usize],
            weights,
        );
        let premium = crate::statement_ru_reporting::STATEMENT_RU_TIFLASH_MULTIPLIER - 1.0;
        results[index] = StatementRUExplainOperatorResult {
            self_ru: self_result.total_ru + (tiflash_ru - before_operator_tiflash_ru) * premium,
            cum_ru: cum_result.total_ru + (tiflash_ru - before_subtree_tiflash_ru) * premium,
        };
    }
    StatementRUOperatorResult {
        state: Complete,
        output_rows: rows,
    }
}

/// Scan ratios belong to request roots at the reader boundary, once per branch.
pub fn collect_statement_ru_reader_scan_bytes(
    tree: &[astersql_planner_core::TypedFlatOperator<'_>],
    operator: &astersql_planner_core::TypedFlatOperator<'_>,
    evidence: &StatementRURuntimeEvidence,
    calculator: &mut StatementRUCalculator,
    roots: &[&dyn astersql_planner_core_base::Plan],
) -> StatementRUOperatorState {
    use crate::statement_ru_result::{ScanEvidence, classify_scan_evidence};
    use StatementRUOperatorState::{Complete, Invalid, Unsupported};
    use astersql_planner_core_operator_physicalop::ReadReqType;
    if roots.len() != operator.ChildrenIdx.len() {
        return Unsupported;
    }
    for (&index, &root) in operator.ChildrenIdx.iter().zip(roots) {
        let Some(child) = tree.get(index) else {
            return Invalid;
        };
        if !std::ptr::eq(child.Origin.as_any(), root.as_any())
            || child.IsRoot
            || child.StoreType != astersql_kv::StoreType::TiKV
            || child.ReqType != ReadReqType::Cop
        {
            return Unsupported;
        }
        let detail = evidence
            .plans
            .iter()
            .find(|stats| stats.plan_id == root.id())
            .and_then(|stats| stats.scan.as_ref());
        let Some(detail) = detail else {
            continue;
        };
        match classify_scan_evidence(
            detail.TotalKeys,
            detail.ProcessedKeys,
            detail.ProcessedKeysSize,
        ) {
            ScanEvidence::Unavailable => {}
            ScanEvidence::Valid(bytes) => {
                if !add_statement_ru_scan_bytes(&mut calculator.units, bytes) {
                    return Invalid;
                }
            }
            ScanEvidence::Invalid => return Invalid,
        }
    }
    Complete
}

pub fn collect_statement_ru_point_lookup_evidence(
    snapshot: Option<&StatementRUPointSnapshot>,
    calculator: &mut StatementRUCalculator,
) -> StatementRUOperatorState {
    use StatementRUOperatorState::{Complete, Invalid};
    let Some(snapshot) = snapshot else {
        return Complete;
    };
    let state = snapshot.state();
    if state != Complete {
        return state;
    }
    if !snapshot.payload_complete {
        return Complete;
    }
    if let crate::statement_ru_result::ScanEvidence::Valid(bytes) =
        crate::statement_ru_result::classify_scan_evidence(
            snapshot.total_keys,
            snapshot.processed_keys,
            snapshot.processed_bytes,
        )
    {
        if !add_statement_ru_scan_bytes(&mut calculator.units, bytes) {
            return Invalid;
        }
    }
    calculator.units.net_bytes += snapshot.payload_bytes as f64;
    Complete
}

/// Go supports root, TiFlash MPP and TiKV cop occurrences, independently of type.
pub fn statement_ru_operator_runs_at_mpp(
    operator: &astersql_planner_core::TypedFlatOperator<'_>,
) -> bool {
    !operator.IsRoot
        && operator.StoreType == astersql_kv::StoreType::TiFlash
        && operator.ReqType == astersql_planner_core_operator_physicalop::ReadReqType::MPP
}
pub fn statement_ru_operator_runs_at_supported_site(
    operator: &astersql_planner_core::TypedFlatOperator<'_>,
) -> bool {
    operator.IsRoot
        || statement_ru_operator_runs_at_mpp(operator)
        || (operator.StoreType == astersql_kv::StoreType::TiKV
            && operator.ReqType == astersql_planner_core_operator_physicalop::ReadReqType::Cop)
}

/// Only sends own network work; receive counters must never be charged again.
fn collect_statement_ru_mpp_network(
    stats: Option<&StatementRUPlanEvidence>,
    calculator: &mut StatementRUCalculator,
) -> StatementRUOperatorState {
    let Some(units) = stats.and_then(|stats| stats.tiflash.as_ref()) else {
        return StatementRUOperatorState::Complete;
    };
    if units.Invalid {
        return StatementRUOperatorState::Invalid;
    }
    calculator.units.net_bytes += units.InnerZoneSendBytes as f64 + units.InterZoneSendBytes as f64;
    calculator.units.cross_az_net_bytes += units.InterZoneSendBytes as f64;
    if calculator.units.net_bytes.is_finite() && calculator.units.cross_az_net_bytes.is_finite() {
        StatementRUOperatorState::Complete
    } else {
        StatementRUOperatorState::Invalid
    }
}

/// Reader owns the sum of actual TableScan bytes, once per MPP occurrence.
fn collect_statement_ru_mpp_scan_bytes(
    tree: &[astersql_planner_core::TypedFlatOperator<'_>],
    index: usize,
    evidence: &StatementRURuntimeEvidence,
    calculator: &mut StatementRUCalculator,
) -> StatementRUOperatorState {
    let operator = &tree[index];
    if !statement_ru_operator_runs_at_mpp(operator) {
        return StatementRUOperatorState::Unsupported;
    }
    if operator
        .Origin
        .as_any()
        .is::<astersql_planner_core_operator_physicalop::PhysicalTableScan>()
    {
        if let Some(units) = evidence
            .plans
            .iter()
            .find(|stats| stats.plan_id == operator.Origin.id())
            .and_then(|stats| stats.tiflash.as_ref())
        {
            if units.Invalid
                || !add_statement_ru_scan_bytes(&mut calculator.units, units.UserReadBytes as f64)
            {
                return StatementRUOperatorState::Invalid;
            }
            calculator.compute[StatementRUEngine::TiFlash as usize].scan_bytes +=
                units.UserReadBytes as f64;
        }
    }
    for &child in &operator.ChildrenIdx {
        let state = collect_statement_ru_mpp_scan_bytes(tree, child, evidence, calculator);
        if state != StatementRUOperatorState::Complete {
            return state;
        }
    }
    StatementRUOperatorState::Complete
}

/// Work = sum(source root rows * (partition keys + 1)); no worker multiplier.
fn collect_statement_ru_shuffle_units(
    shuffle: &astersql_planner_core_operator_physicalop::PhysicalShuffle,
    evidence: &StatementRURuntimeEvidence,
    calculator: &mut StatementRUCalculator,
) -> StatementRUOperatorState {
    if shuffle.DataSources.len() > shuffle.ByItemArrays.len()
        || (shuffle.DataSources.is_empty() && !shuffle.DataSourceExplainIDs.is_empty())
    {
        return StatementRUOperatorState::Unsupported;
    }
    for (source, keys) in shuffle.DataSources.iter().zip(&shuffle.ByItemArrays) {
        let rows = evidence
            .plans
            .iter()
            .find(|stats| stats.plan_id == source.id())
            .map(|stats| &stats.root_rows);
        if rows.is_some_and(|rows| rows.Invalid() || rows.Rows < 0) {
            return StatementRUOperatorState::Invalid;
        }
        if !add_statement_ru_cpu_work(
            &mut calculator.units,
            rows.map_or(0, |rows| rows.Rows) as f64 * (keys.len() as f64 + 1.0),
        ) {
            return StatementRUOperatorState::Invalid;
        }
    }
    StatementRUOperatorState::Complete
}

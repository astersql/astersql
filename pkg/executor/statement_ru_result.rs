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

//! Evidence classification for the statement RU v3 calculator.

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
/// weights in the `ru-v2` config section while RU v3 replaces its legacy model.
pub fn current_statement_ru_weights() -> StmtWeights {
    let configured = &astersql_config::get_global_config().ruv2.stmt_weights;
    StmtWeights {
        cross_az_net_byte: configured.cross_az_net_byte,
        cpu_work: configured.cpu_work,
        scan_byte: configured.scan_byte,
        net_byte: configured.net_byte,
        frontend_compile_byte: configured.frontend_compile_byte,
        hash_state_row: configured.hash_state_row,
        join_output_row: configured.join_output_row,
        write_statement: configured.write_statement,
        operator_num: configured.operator_num,
        write_key: configured.write_key,
        write_byte: configured.write_byte,
    }
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

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
        if recorded && !success {
            self.abort();
        }
        recorded
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
        self.setup.lock().expect("statement RU owner lock").take()
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

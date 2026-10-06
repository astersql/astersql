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

//! RU v2 raw units attributed to TiDB, TiKV, and TiFlash.

use astersql_resourcegroup::ruv2::model::{StmtUnits, StmtWeights};

pub const STATEMENT_RU_TIFLASH_MULTIPLIER: f64 = 10.0;

/// Go `statementRUFailureReason` is a terminal calculation outcome. It does
/// not assert that every remote execution detail was available.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatementRUFailureReason {
    NotFinished,
    Unsupported,
    Invalid,
    StatementError,
    Ineligible,
    Panic,
}

impl StatementRUFailureReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotFinished => "not_finished",
            Self::Unsupported => "unsupported_plan",
            Self::Invalid => "invalid_plan_or_evidence",
            Self::StatementError => "statement_error",
            Self::Ineligible => "ineligible",
            Self::Panic => "panic",
        }
    }

    pub fn status(self) -> &'static str {
        match self {
            Self::Unsupported | Self::Ineligible => "skipped",
            _ => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum StatementRUEngine {
    TiDB,
    TiKV,
    TiFlash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum StatementRUOperator {
    Wrapper,
    Projection,
    Selection,
    Limit,
    Sort,
    TopN,
    Window,
    HashAgg,
    StreamAgg,
    HashJoin,
    MergeJoin,
    LookupJoin,
    Reader,
    LookupReader,
    UnionScan,
    Shuffle,
    RangeScan,
    PointLookup,
    Write,
    Analyze,
    Frontend,
    CopTransport,
    KVWrite,
}

const ENGINE_COUNT: usize = 3;
const OPERATOR_COUNT: usize = 23;

/// Fixed-size full-mode report; it owns numeric units and no plan nodes.
#[derive(Clone, Debug, PartialEq)]
pub struct StatementRUFullReport {
    pub units: [[StmtUnits; OPERATOR_COUNT]; ENGINE_COUNT],
    pub seen: [[bool; OPERATOR_COUNT]; ENGINE_COUNT],
}

impl Default for StatementRUFullReport {
    fn default() -> Self {
        Self {
            units: [[StmtUnits::default(); OPERATOR_COUNT]; ENGINE_COUNT],
            seen: [[false; OPERATOR_COUNT]; ENGINE_COUNT],
        }
    }
}

impl StatementRUFullReport {
    pub fn add(
        &mut self,
        engine: StatementRUEngine,
        operator: StatementRUOperator,
        units: StmtUnits,
    ) {
        let (engine, operator) = (engine as usize, operator as usize);
        self.units[engine][operator] = self.units[engine][operator].add(units);
        self.seen[engine][operator] = true;
    }

    pub fn add_operator(
        &mut self,
        engine: StatementRUEngine,
        operator: StatementRUOperator,
        mut units: StmtUnits,
    ) {
        if engine == StatementRUEngine::TiFlash {
            self.add(engine, operator, units);
            return;
        }
        let remote = StmtUnits {
            scan_bytes: units.scan_bytes,
            net_bytes: units.net_bytes,
            ..StmtUnits::default()
        };
        units.scan_bytes = 0.0;
        units.net_bytes = 0.0;
        self.add(engine, operator, units);
        if remote.scan_bytes != 0.0 || remote.net_bytes != 0.0 {
            self.add(StatementRUEngine::TiKV, operator, remote);
        }
    }

    pub fn add_statement_units(&mut self, units: StmtUnits) {
        self.add(
            StatementRUEngine::TiDB,
            StatementRUOperator::Frontend,
            StmtUnits {
                frontend_compile_bytes: units.frontend_compile_bytes,
                ..StmtUnits::default()
            },
        );
        if units.write_statement != 0.0 {
            self.add(
                StatementRUEngine::TiDB,
                StatementRUOperator::Write,
                StmtUnits {
                    write_statement: units.write_statement,
                    ..StmtUnits::default()
                },
            );
        }
        if units.write_keys != 0.0 || units.write_bytes != 0.0 {
            self.add(
                StatementRUEngine::TiKV,
                StatementRUOperator::KVWrite,
                StmtUnits {
                    write_keys: units.write_keys,
                    write_bytes: units.write_bytes,
                    ..StmtUnits::default()
                },
            );
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatementRUComputeUnits {
    pub cpu_work: f64,
    pub hash_state_rows: f64,
    pub operator_num: f64,
    pub join_output_rows: f64,
    pub scan_bytes: f64,
    pub net_bytes: f64,
    pub cross_az_net_bytes: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatementRUEngineResult {
    pub tidb: f64,
    pub tikv: f64,
    pub tiflash: f64,
}

/// Go `statementRUCalculator.engineResult`, including the distinction between
/// local operator work and remote scan/network work owned by TiKV.
pub fn statement_ru_engine_result(
    units: StmtUnits,
    compute: [StatementRUComputeUnits; 3],
    weights: StmtWeights,
) -> StatementRUEngineResult {
    let [tidb, tikv, tiflash] = compute;
    let tiflash_ru = statement_ru_tiflash_ru(tiflash, weights);
    StatementRUEngineResult {
        tidb: weights.cpu_work * tidb.cpu_work
            + weights.hash_state_row * tidb.hash_state_rows
            + weights.operator_num * tidb.operator_num
            + weights.join_output_row * (units.join_output_rows - tiflash.join_output_rows)
            + weights.frontend_compile_byte * units.frontend_compile_bytes
            + weights.write_statement * units.write_statement,
        tikv: weights.cpu_work * tikv.cpu_work
            + weights.hash_state_row * tikv.hash_state_rows
            + weights.operator_num * tikv.operator_num
            + weights.scan_byte * (units.scan_bytes - tiflash.scan_bytes)
            + weights.net_byte * (units.net_bytes - tiflash.net_bytes)
            + weights.write_key * units.write_keys
            + weights.write_byte * units.write_bytes,
        tiflash: tiflash_ru,
    }
}

/// Raw TiFlash RU shared by engine attribution and per-occurrence EXPLAIN.
pub fn statement_ru_tiflash_ru(tiflash: StatementRUComputeUnits, weights: StmtWeights) -> f64 {
    weights.cpu_work * tiflash.cpu_work
        + weights.hash_state_row * tiflash.hash_state_rows
        + weights.operator_num * tiflash.operator_num
        + weights.join_output_row * tiflash.join_output_rows
        + weights.scan_byte * tiflash.scan_bytes
        + weights.net_byte * tiflash.net_bytes
        + weights.cross_az_net_byte * tiflash.cross_az_net_bytes
}

/// Go full-mode labels are bounded independently of SQL and plan identifiers.
pub fn publish_statement_ru_full_metrics(
    sink: &dyn crate::statement_ru_result::StatementRUPublicationSink,
    snapshot: &crate::statement_ru_result::StatementRUFinalizedSnapshot,
) {
    const ENGINES: [&str; 3] = ["tidb", "tikv", "tiflash"];
    const OPERATORS: [&str; 23] = [
        "wrapper",
        "projection",
        "selection",
        "limit",
        "sort",
        "topn",
        "window",
        "hash_agg",
        "stream_agg",
        "hash_join",
        "merge_join",
        "lookup_join",
        "reader",
        "lookup_reader",
        "union_scan",
        "shuffle",
        "range_scan",
        "point_lookup",
        "write",
        "analyze",
        "sql_frontend",
        "coprocessor",
        "kv_write",
    ];
    let Some(report) = &snapshot.report else {
        return;
    };
    for (engine, operators) in report.units.iter().enumerate() {
        for (operator, units) in operators.iter().enumerate() {
            if !report.seen[engine][operator] {
                continue;
            }
            for (name, value) in [
                ("cpu_work", units.cpu_work),
                ("scan_bytes", units.scan_bytes),
                ("net_bytes", units.net_bytes),
                ("cross_az_net_bytes", units.cross_az_net_bytes),
                ("frontend_compile_bytes", units.frontend_compile_bytes),
                ("hash_state_rows", units.hash_state_rows),
                ("join_output_rows", units.join_output_rows),
                ("write_statement", units.write_statement),
                ("operator_num", units.operator_num),
                ("write_keys", units.write_keys),
                ("write_bytes", units.write_bytes),
            ] {
                if value != 0.0 {
                    sink.unit(ENGINES[engine], OPERATORS[operator], name, value);
                }
            }
        }
    }
    sink.statement("success", snapshot.calibration_state.label());
}

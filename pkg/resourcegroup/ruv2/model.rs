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

//! Raw units and weighting model used to calculate RU v3.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StmtUnits {
    pub write_statement: f64,
    pub operator_num: f64,
    pub write_keys: f64,
    pub write_bytes: f64,
    pub cpu_work: f64,
    pub scan_bytes: f64,
    pub net_bytes: f64,
    pub cross_az_net_bytes: f64,
    pub frontend_compile_bytes: f64,
    pub hash_state_rows: f64,
    pub join_output_rows: f64,
}

impl StmtUnits {
    pub fn valid(self) -> bool {
        self.cross_az_net_bytes <= self.net_bytes
            && valid_values(&[
                self.cross_az_net_bytes,
                self.cpu_work,
                self.scan_bytes,
                self.net_bytes,
                self.frontend_compile_bytes,
                self.hash_state_rows,
                self.join_output_rows,
                self.write_statement,
                self.operator_num,
                self.write_keys,
                self.write_bytes,
            ])
    }

    pub fn add(self, other: Self) -> Self {
        Self {
            cpu_work: self.cpu_work + other.cpu_work,
            scan_bytes: self.scan_bytes + other.scan_bytes,
            net_bytes: self.net_bytes + other.net_bytes,
            cross_az_net_bytes: self.cross_az_net_bytes + other.cross_az_net_bytes,
            frontend_compile_bytes: self.frontend_compile_bytes + other.frontend_compile_bytes,
            hash_state_rows: self.hash_state_rows + other.hash_state_rows,
            join_output_rows: self.join_output_rows + other.join_output_rows,
            write_statement: self.write_statement + other.write_statement,
            operator_num: self.operator_num + other.operator_num,
            write_keys: self.write_keys + other.write_keys,
            write_bytes: self.write_bytes + other.write_bytes,
        }
    }

    pub fn sub(self, other: Self) -> Self {
        Self {
            cpu_work: self.cpu_work - other.cpu_work,
            scan_bytes: self.scan_bytes - other.scan_bytes,
            net_bytes: self.net_bytes - other.net_bytes,
            cross_az_net_bytes: self.cross_az_net_bytes - other.cross_az_net_bytes,
            frontend_compile_bytes: self.frontend_compile_bytes - other.frontend_compile_bytes,
            hash_state_rows: self.hash_state_rows - other.hash_state_rows,
            join_output_rows: self.join_output_rows - other.join_output_rows,
            write_statement: self.write_statement - other.write_statement,
            operator_num: self.operator_num - other.operator_num,
            write_keys: self.write_keys - other.write_keys,
            write_bytes: self.write_bytes - other.write_bytes,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StmtWeights {
    pub cross_az_net_byte: f64,
    pub cpu_work: f64,
    pub scan_byte: f64,
    pub net_byte: f64,
    pub frontend_compile_byte: f64,
    pub hash_state_row: f64,
    pub join_output_row: f64,
    pub write_statement: f64,
    pub operator_num: f64,
    pub write_key: f64,
    pub write_byte: f64,
}

pub fn default_weights() -> StmtWeights {
    StmtWeights {
        cpu_work: 1.0,
        scan_byte: 1.0,
        net_byte: 1.0,
        frontend_compile_byte: 1.0,
        hash_state_row: 1.0,
        join_output_row: 1.0,
        write_statement: 1.0,
        operator_num: 1.0,
        write_key: 1.0,
        write_byte: 1.0,
        ..Default::default()
    }
}

impl StmtWeights {
    pub fn validate(self) -> Result<(), String> {
        for (name, value) in [
            ("cpu-work", self.cpu_work),
            ("scan-byte", self.scan_byte),
            ("net-byte", self.net_byte),
            ("cross-az-net-byte", self.cross_az_net_byte),
            ("frontend-compile-byte", self.frontend_compile_byte),
            ("hash-state-row", self.hash_state_row),
            ("join-output-row", self.join_output_row),
            ("write-statement", self.write_statement),
            ("operator-num", self.operator_num),
            ("write-key", self.write_key),
            ("write-byte", self.write_byte),
        ] {
            if !valid_values(&[value]) {
                return Err(invalid_weight_error(name, value));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DDLWeights {
    pub txn_kv_bytes: f64,
    pub ingest_kv_bytes: f64,
}

pub fn default_ddl_weights() -> DDLWeights {
    DDLWeights {
        txn_kv_bytes: 1.0,
        ingest_kv_bytes: 1.0,
    }
}

impl DDLWeights {
    pub fn validate(self) -> Result<(), String> {
        for (name, value) in [
            ("txn-kv-bytes", self.txn_kv_bytes),
            ("ingest-kv-bytes", self.ingest_kv_bytes),
        ] {
            if !valid_values(&[value]) {
                return Err(invalid_weight_error(name, value));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StmtResult {
    pub total_ru: f64,
}

fn valid_values(values: &[f64]) -> bool {
    values
        .iter()
        .all(|value| *value >= 0.0 && value.is_finite())
}

fn invalid_weight_error(name: &str, value: f64) -> String {
    let rendered = if value == f64::INFINITY {
        "+Inf".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-Inf".to_owned()
    } else {
        value.to_string()
    };
    format!("{name} must be finite and non-negative, got {rendered}")
}

pub fn calculate(units: StmtUnits, weights: StmtWeights) -> Option<StmtResult> {
    if !units.valid() || weights.validate().is_err() {
        return None;
    }
    let total_ru = weights.cpu_work * units.cpu_work
        + weights.scan_byte * units.scan_bytes
        + weights.net_byte * units.net_bytes
        + weights.cross_az_net_byte * units.cross_az_net_bytes
        + weights.frontend_compile_byte * units.frontend_compile_bytes
        + weights.hash_state_row * units.hash_state_rows
        + weights.join_output_row * units.join_output_rows
        + weights.write_statement * units.write_statement
        + weights.operator_num * units.operator_num
        + weights.write_key * units.write_keys
        + weights.write_byte * units.write_bytes;
    valid_values(&[total_ru]).then_some(StmtResult { total_ru })
}

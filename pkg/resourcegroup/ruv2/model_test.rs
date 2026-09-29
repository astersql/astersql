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

use super::model::*;

#[test]
fn go_merge_8_defaults_and_validation() {
    let weights = default_weights();
    assert_eq!(StmtWeights::default().cpu_work, 0.0);
    assert_eq!(
        weights,
        StmtWeights {
            cross_az_net_byte: 0.0,
            cpu_work: 1.0,
            scan_byte: 1.0,
            net_byte: 1.0,
            frontend_compile_byte: 1.0,
            hash_state_row: 1.0,
            join_output_row: 1.0,
            write_statement: 1.0,
            operator_num: 1.0,
            write_key: 1.0,
            write_byte: 1.0
        }
    );
    assert!(weights.validate().is_ok());
    assert_eq!(
        default_ddl_weights(),
        DDLWeights {
            txn_kv_bytes: 1.0,
            ingest_kv_bytes: 1.0
        }
    );
    assert_eq!(DDLWeights::default().txn_kv_bytes, 0.0);
    assert_eq!(
        DDLWeights {
            txn_kv_bytes: -1.0,
            ingest_kv_bytes: 0.0
        }
        .validate()
        .unwrap_err(),
        "txn-kv-bytes must be finite and non-negative, got -1"
    );
    assert!(
        DDLWeights {
            txn_kv_bytes: 0.0,
            ingest_kv_bytes: f64::NAN
        }
        .validate()
        .is_err()
    );
    assert_eq!(
        DDLWeights {
            ingest_kv_bytes: f64::INFINITY,
            ..Default::default()
        }
        .validate()
        .unwrap_err(),
        "ingest-kv-bytes must be finite and non-negative, got +Inf"
    );
    assert!(
        StmtWeights {
            cpu_work: f64::INFINITY,
            ..weights
        }
        .validate()
        .is_err()
    );
}

#[test]
fn go_merge_8_arithmetic_and_calculation() {
    let left = StmtUnits {
        cpu_work: 1.0,
        scan_bytes: 2.0,
        net_bytes: 3.0,
        frontend_compile_bytes: 4.0,
        hash_state_rows: 5.0,
        join_output_rows: 6.0,
        write_statement: 7.0,
        operator_num: 8.0,
        write_keys: 9.0,
        write_bytes: 10.0,
        ..Default::default()
    };
    let right = StmtUnits {
        cpu_work: 10.0,
        scan_bytes: 9.0,
        net_bytes: 8.0,
        frontend_compile_bytes: 7.0,
        hash_state_rows: 6.0,
        join_output_rows: 5.0,
        write_statement: 4.0,
        operator_num: 3.0,
        write_keys: 2.0,
        write_bytes: 1.0,
        ..Default::default()
    };
    assert_eq!(left.add(right).sub(right), left);
    let weights = StmtWeights {
        cpu_work: 2.0,
        scan_byte: 3.0,
        net_byte: 4.0,
        frontend_compile_byte: 5.0,
        hash_state_row: 6.0,
        join_output_row: 7.0,
        write_statement: 8.0,
        operator_num: 9.0,
        write_key: 10.0,
        write_byte: 11.0,
        ..Default::default()
    };
    assert_eq!(
        calculate(left, weights),
        Some(StmtResult { total_ru: 440.0 })
    );
    assert_eq!(
        calculate(
            StmtUnits {
                cpu_work: f64::MAX,
                ..Default::default()
            },
            StmtWeights {
                cpu_work: 2.0,
                ..Default::default()
            }
        ),
        None
    );
}

#[test]
fn go_merge_8_cross_az_and_invalid_units() {
    let units = StmtUnits {
        net_bytes: 150.0,
        cross_az_net_bytes: 50.0,
        ..Default::default()
    };
    assert_eq!(
        calculate(units, default_weights()),
        Some(StmtResult { total_ru: 150.0 })
    );
    assert_eq!(
        calculate(
            units,
            StmtWeights {
                cross_az_net_byte: 2.0,
                ..default_weights()
            }
        ),
        Some(StmtResult { total_ru: 250.0 })
    );
    assert_eq!(units.add(units).sub(units), units);
    assert!(
        !StmtUnits {
            cross_az_net_bytes: 151.0,
            ..units
        }
        .valid()
    );
    assert!(
        !StmtUnits {
            operator_num: -1.0,
            ..Default::default()
        }
        .valid()
    );
    assert!(
        !StmtUnits {
            hash_state_rows: f64::NAN,
            ..Default::default()
        }
        .valid()
    );
    assert_eq!(
        calculate(
            StmtUnits::default(),
            StmtWeights {
                cross_az_net_byte: -1.0,
                ..Default::default()
            }
        ),
        None
    );
}

// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

use std::collections::BTreeSet;

use crate::ttl::{
    ColumnDefinition, ColumnType, DEFAULT_TTL_JOB_INTERVAL, TtlError, TtlInfo, TtlOption, TtlTable,
    apply_model_ttl_change, check_ttl_job_interval, get_ttl_info_in_options, validate_ttl_info,
};

fn ttl_info(enable: bool, job_interval: &str) -> TtlInfo {
    TtlInfo {
        column_name: "test_column".to_string(),
        interval_expression: 5,
        interval_unit: "YEAR".to_string(),
        enable,
        job_interval: job_interval.to_string(),
    }
}

#[test]
fn get_ttl_info_in_options_matches_go_cases() {
    let cases = [
        (vec![], None, None, None),
        (
            vec![TtlOption::Definition {
                column_name: "test_column".to_string(),
                interval_expression: 5,
                interval_unit: "YEAR".to_string(),
            }],
            Some(ttl_info(true, DEFAULT_TTL_JOB_INTERVAL)),
            None,
            None,
        ),
        (
            vec![
                TtlOption::Enable(false),
                TtlOption::Definition {
                    column_name: "test_column".to_string(),
                    interval_expression: 5,
                    interval_unit: "YEAR".to_string(),
                },
            ],
            Some(ttl_info(false, DEFAULT_TTL_JOB_INTERVAL)),
            Some(false),
            None,
        ),
        (
            vec![
                TtlOption::Enable(false),
                TtlOption::Definition {
                    column_name: "test_column".to_string(),
                    interval_expression: 5,
                    interval_unit: "YEAR".to_string(),
                },
                TtlOption::Enable(true),
            ],
            Some(ttl_info(true, DEFAULT_TTL_JOB_INTERVAL)),
            Some(true),
            None,
        ),
        (
            vec![
                TtlOption::Definition {
                    column_name: "test_column".to_string(),
                    interval_expression: 5,
                    interval_unit: "YEAR".to_string(),
                },
                TtlOption::JobInterval("25h".to_string()),
            ],
            Some(ttl_info(true, "25h")),
            None,
            Some("25h".to_string()),
        ),
    ];

    for (options, expected_info, expected_enable, expected_schedule) in cases {
        assert_eq!(
            Ok((expected_info, expected_enable, expected_schedule)),
            get_ttl_info_in_options(&options)
        );
    }
}

#[test]
fn option_aggregation_defers_job_interval_validation_like_go() {
    let invalid_schedule = "not-a-duration".to_string();
    assert_eq!(
        Ok((None, None, Some(invalid_schedule.clone()))),
        get_ttl_info_in_options(&[TtlOption::JobInterval(invalid_schedule)])
    );
}

struct RestoreDeployMode(astersql_config_deploymode::Mode);

impl Drop for RestoreDeployMode {
    fn drop(&mut self) {
        astersql_config_deploymode::Set(self.0).unwrap();
    }
}

#[test]
fn starter_uses_fifteen_minute_default_and_rejects_other_intervals() {
    if !astersql_config_kerneltype::IsNextGen() {
        return;
    }

    let _restore = RestoreDeployMode(astersql_config_deploymode::Get());
    astersql_config_deploymode::Set(astersql_config_deploymode::Starter).unwrap();

    let (info, _, _) = get_ttl_info_in_options(&[TtlOption::Definition {
        column_name: "test_column".to_string(),
        interval_expression: 5,
        interval_unit: "YEAR".to_string(),
    }])
    .unwrap();
    assert_eq!(
        info.unwrap().job_interval,
        astersql_meta_model::StarterDefaultTTLJobInterval
    );
    assert_eq!(
        Ok(()),
        check_ttl_job_interval(astersql_meta_model::StarterDefaultTTLJobInterval)
    );
    assert_eq!(
        Err(TtlError::UnsupportedStarterJobInterval),
        check_ttl_job_interval("1h")
    );
    assert_eq!(
        Err(TtlError::UnsupportedStarterJobInterval),
        get_ttl_info_in_options(&[
            TtlOption::Definition {
                column_name: "test_column".to_string(),
                interval_expression: 5,
                interval_unit: "YEAR".to_string(),
            },
            TtlOption::JobInterval("1h".to_string()),
        ])
    );

    let mut table = astersql_meta_model::TableInfo {
        TTLInfo: Some(astersql_meta_model::TTLInfo {
            JobInterval: astersql_meta_model::StarterDefaultTTLJobInterval.to_string(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let error = apply_model_ttl_change(&mut table, None, None, Some("1h".to_string())).unwrap_err();
    assert!(error.contains("TTL_JOB_INTERVAL other than '15m'"));
    assert_eq!(
        table.TTLInfo.unwrap().JobInterval,
        astersql_meta_model::StarterDefaultTTLJobInterval
    );
}

#[test]
fn common_handle_allows_non_float_primary_key_like_go() {
    let table = TtlTable {
        columns: vec![ColumnDefinition {
            name: "test_column".to_string(),
            column_type: ColumnType::DateTime,
        }],
        ttl: None,
        temporary: false,
        cached: false,
        common_handle: true,
        primary_key_columns: BTreeSet::from(["test_column".to_string()]),
        foreign_key_columns: BTreeSet::new(),
    };

    assert_eq!(Ok(()), validate_ttl_info(&table, &ttl_info(true, "1h")));
}

#[test]
fn common_handle_rejects_float_primary_key_like_go() {
    let table = TtlTable {
        columns: vec![
            ColumnDefinition {
                name: "test_column".to_string(),
                column_type: ColumnType::DateTime,
            },
            ColumnDefinition {
                name: "float_pk".to_string(),
                column_type: ColumnType::Float,
            },
        ],
        ttl: None,
        temporary: false,
        cached: false,
        common_handle: true,
        primary_key_columns: BTreeSet::from(["float_pk".to_string()]),
        foreign_key_columns: BTreeSet::new(),
    };

    assert_eq!(
        Err(crate::ttl::TtlError::UnsupportedPrimaryKey),
        validate_ttl_info(&table, &ttl_info(true, "1h"))
    );
}

#[test]
fn ttl_validation_does_not_reject_unrelated_table_flags() {
    let table = TtlTable {
        columns: vec![ColumnDefinition {
            name: "test_column".to_string(),
            column_type: ColumnType::Timestamp,
        }],
        ttl: None,
        temporary: false,
        cached: true,
        common_handle: false,
        primary_key_columns: BTreeSet::new(),
        foreign_key_columns: BTreeSet::from(["parent_id".to_string()]),
    };

    assert_eq!(Ok(()), validate_ttl_info(&table, &ttl_info(true, "1h")));
}

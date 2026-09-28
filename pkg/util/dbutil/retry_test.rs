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

use crate::{DbError, retry::IsRetryableError};

fn mysql_error(code: u16, message: &str) -> DbError {
    DbError {
        code,
        sql_state: None,
        message: message.into(),
    }
}

#[test]
fn retryable_error_codes_match_go() {
    let retryable = [1213, 9001, 9003, 9004, 8027, 8028, 8005, 8022, 9007, 8245];
    for code in retryable {
        assert!(
            IsRetryableError(&mysql_error(code, "retryable")),
            "error code {code} should be retryable"
        );
    }
}

#[test]
fn conditionally_retryable_and_unknown_codes_remain_non_retryable() {
    // Go deliberately excludes TiKV timeout, table locked, interrupted queries,
    // and unavailable regions because they are conditional or non-retryable.
    for code in [1046, 9002, 8020, 1317, 9005, 1064] {
        assert!(
            !IsRetryableError(&mysql_error(code, "not retryable")),
            "error code {code} should not be retryable"
        );
    }
}

#[test]
fn unknown_error_is_retryable_only_for_legacy_schema_messages() {
    assert!(!IsRetryableError(&mysql_error(1105, "i/o timeout")));
    assert!(IsRetryableError(&mysql_error(
        1105,
        "Information schema is out of date"
    )));
    assert!(IsRetryableError(&mysql_error(
        1105,
        "Information schema is changed"
    )));

    // Go uses strings.Contains, which is substring-based and case-sensitive.
    assert!(IsRetryableError(&mysql_error(
        1105,
        "server: Information schema is changed; retry"
    )));
    assert!(!IsRetryableError(&mysql_error(
        1105,
        "information schema is changed"
    )));
}

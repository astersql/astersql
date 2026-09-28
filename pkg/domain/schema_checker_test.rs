// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

use crate::schema_checker::{
    RelatedSchemaChange, SchemaCheckError, SchemaCheckResult, SchemaChecker, SchemaValidator,
};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Eq)]
struct CheckCall {
    txn_ts: u64,
    schema_version: i64,
    related_table_ids: Vec<i64>,
    check_by_delta: bool,
}

struct Validator {
    results: Mutex<Vec<SchemaCheckResult>>,
    calls: Mutex<Vec<CheckCall>>,
}

impl Validator {
    fn new(results: Vec<SchemaCheckResult>) -> Self {
        Self {
            results: Mutex::new(results.into_iter().rev().collect()),
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl SchemaValidator for Validator {
    fn check(
        &self,
        txn_ts: u64,
        schema_version: i64,
        related_table_ids: &[i64],
        check_by_delta: bool,
    ) -> SchemaCheckResult {
        self.calls.lock().unwrap().push(CheckCall {
            txn_ts,
            schema_version,
            related_table_ids: related_table_ids.to_vec(),
            check_by_delta,
        });
        self.results.lock().unwrap().pop().unwrap()
    }
}

#[test]
fn check_uses_stored_schema_version_and_forwards_validator_arguments() {
    let validator = Arc::new(Validator::new(vec![SchemaCheckResult::Success]));
    let checker = SchemaChecker::new(validator.clone(), 7, vec![1, 2], true);

    assert_eq!(checker.check(100), Ok(()));
    assert_eq!(
        *validator.calls.lock().unwrap(),
        vec![CheckCall {
            txn_ts: 100,
            schema_version: 7,
            related_table_ids: vec![1, 2],
            check_by_delta: true,
        }]
    );
}

#[test]
fn unknown_retries_and_fail_returns_the_related_change() {
    let change = RelatedSchemaChange {
        physical_table_ids: vec![2],
        action_types: vec!["add index".to_owned()],
    };
    let validator = Arc::new(Validator::new(vec![
        SchemaCheckResult::Unknown,
        SchemaCheckResult::Fail(Some(change.clone())),
    ]));
    let checker =
        SchemaChecker::new(validator.clone(), 7, vec![], false).with_retry(Duration::ZERO, 2);

    assert_eq!(
        checker.check_by_schema_version(100, 9),
        Err(SchemaCheckError::InfoSchemaChanged(Some(change)))
    );
    assert_eq!(validator.calls.lock().unwrap().len(), 2);
    assert_eq!(validator.calls.lock().unwrap()[0].schema_version, 9);
}

#[test]
fn exhausted_unknown_retries_sleep_after_every_go_attempt() {
    let validator = Arc::new(Validator::new(vec![SchemaCheckResult::Unknown]));
    let checker = SchemaChecker::new(validator.clone(), 7, vec![], false)
        .with_retry(Duration::from_millis(20), 1);

    let started = Instant::now();
    assert_eq!(checker.check(100), Err(SchemaCheckError::InfoSchemaExpired));
    assert!(
        started.elapsed() >= Duration::from_millis(15),
        "Go sleeps after the final ResultUnknown before returning expired"
    );
    assert_eq!(validator.calls.lock().unwrap().len(), 1);
}

#[test]
fn zero_retry_limit_expires_without_calling_the_validator() {
    let validator = Arc::new(Validator::new(vec![]));
    let checker =
        SchemaChecker::new(validator.clone(), 7, vec![], false).with_retry(Duration::ZERO, 0);

    assert_eq!(checker.check(100), Err(SchemaCheckError::InfoSchemaExpired));
    assert!(validator.calls.lock().unwrap().is_empty());
}

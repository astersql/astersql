// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

//! Rust equivalent of the package-level Go `TestMain` contract.

use std::sync::OnceLock;

use astersql_config::{get_global_config, new_config};
use astersql_testkit_testsetup::SetupForCommonTest;
use astersql_util_topsql_state::{EnableTopSQL, TopSQLEnabled};

static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

fn ensure_test_main_environment() -> Result<(), String> {
    INITIALIZED
        .get_or_init(|| {
            SetupForCommonTest();
            EnableTopSQL();
            // SAFETY: OnceLock makes static metrics initialization process-unique.
            unsafe {
                astersql_metrics::metrics::InitMetrics().map_err(|error| error.to_string())?;
                astersql_metrics::metrics::RegisterMetrics().map_err(|error| error.to_string())?;
            }
            Ok(())
        })
        .clone()
}

fn finish_test_main(status: i32, cleanup: impl FnOnce()) -> i32 {
    cleanup();
    status
}

#[test]
fn test_main_initializes_and_runs_cleanup() {
    ensure_test_main_environment().expect("initialize server tests");
    ensure_test_main_environment().expect("repeat server test initialization");
    assert!(TopSQLEnabled());
    assert_eq!(
        format!("{:#?}", get_global_config()),
        format!("{:#?}", new_config())
    );
    let mut cleaned = false;
    assert_eq!(finish_test_main(23, || cleaned = true), 23);
    assert!(cleaned);
}

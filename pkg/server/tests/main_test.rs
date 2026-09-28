// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

use std::sync::OnceLock;

use astersql_config::{get_global_config, new_config};
use astersql_testkit_testsetup::SetupForCommonTest;
use astersql_util_topsql_state::{EnableTopSQL, TopSQLEnabled};

static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

/// Rust 没有 Go `TestMain`，以进程级一次性初始化复现公共 setup、TopSQL 与 metrics。
pub(crate) fn ensure_test_main_environment() -> Result<(), String> {
    INITIALIZED
        .get_or_init(|| {
            SetupForCommonTest();
            EnableTopSQL();

            // SAFETY: OnceLock 保证本测试进程只初始化并注册一次静态 metrics。
            unsafe {
                astersql_metrics::metrics::InitMetrics().map_err(|error| error.to_string())?;
                astersql_metrics::metrics::RegisterMetrics().map_err(|error| error.to_string())?;
            }
            Ok(())
        })
        .clone()
}

#[test]
fn test_main_applies_go_process_setup_and_preserves_global_config() {
    ensure_test_main_environment().expect("server test environment must initialize");
    ensure_test_main_environment().expect("server test environment must be idempotent");
    assert!(TopSQLEnabled());
    let expected = format!("{:#?}", new_config());
    let actual = format!("{:#?}", get_global_config());
    assert_eq!(
        actual, expected,
        "server global config was changed by package initialization"
    );
}

// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// optimizor 包测试进程级初始化（对应 Go TestMain）。
//
// Rust 可执行公共测试 setup、TopSQL 与 metrics 初始化。

use std::sync::OnceLock;

static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

/// 执行 Go `TestMain` 中存在 Rust 等价物的进程级初始化。
fn initialize_optimizor_test_environment() -> Result<(), String> {
    INITIALIZED
        .get_or_init(|| {
            astersql_testkit_testsetup::SetupForCommonTest();
            astersql_util_topsql_state::EnableTopSQL();

            // Go 仅输出诊断而不中止：保持相同错误路径和副作用。
            let default_config = astersql_config::new_config();
            let global_config = astersql_config::get_global_config();
            let default_snapshot = format!("{default_config:#?}");
            let global_snapshot = format!("{global_config:#?}");
            if default_snapshot != global_snapshot {
                eprintln!(
                    "server: the global config has been changed.\ndefault: {default_snapshot}\nglobal: {global_snapshot}"
                );
            }

            // SAFETY: `INITIALIZED` 保证本测试进程只初始化并注册一次指标。
            unsafe {
                astersql_metrics::metrics::InitMetrics().map_err(|error| error.to_string())?;
                astersql_metrics::metrics::RegisterMetrics().map_err(|error| error.to_string())?;
            }
            Ok(())
        })
        .clone()
}

#[test]
fn optimizor_test_environment_is_initialized_once() {
    initialize_optimizor_test_environment().expect("optimizor test environment must initialize");
    initialize_optimizor_test_environment().expect("repeated initialization must be idempotent");
    assert!(astersql_util_topsql_state::TopSQLEnabled());

    let default_snapshot = format!("{:#?}", astersql_config::new_config());
    let global_snapshot = format!("{:#?}", astersql_config::get_global_config());
    assert_eq!(global_snapshot, default_snapshot);
}

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

// handler 测试进程级初始化，对齐 Go 的 TestMain。
//
// 注册公共测试 setup、启用 TopSQL，并校验全局配置未被污染；
// 指标（metrics）静态量只初始化一次。

use std::sync::OnceLock;

/// 进程级初始化结果缓存。
static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

/// 执行进程级初始化；`OnceLock` 保证并行测试下只注册一次。
///
/// Performs the process-wide setup that Go's TestMain executes before the
/// handler tests. `OnceLock` makes registration safe when tests run in parallel.
pub fn initialize_handler_test_environment() -> Result<(), String> {
    INITIALIZED
        .get_or_init(|| {
            astersql_testkit_testsetup::SetupForCommonTest();
            astersql_util_topsql_state::EnableTopSQL();

            // The global config must still equal a newly-created config after
            // package initialization. Keep the original diagnostic behavior.
            let default_config = astersql_config::new_config();
            let global_config = astersql_config::get_global_config();
            let default_snapshot = format!("{default_config:#?}");
            let global_snapshot = format!("{global_config:#?}");
            if default_snapshot != global_snapshot {
                eprintln!(
                    "server: the global config has been changed.\ndefault: {default_snapshot}\nglobal: {global_snapshot}"
                );
            }

            // SAFETY: metric statics are initialized and registered exactly
            // once in this test process, guarded by `INITIALIZED`.
            unsafe {
                astersql_metrics::metrics::InitMetrics().map_err(|error| error.to_string())?;
                astersql_metrics::metrics::RegisterMetrics().map_err(|error| error.to_string())?;
            }
            Ok(())
        })
        .clone()
}

#[test]
fn handler_test_environment_is_initialized_once() {
    initialize_handler_test_environment().expect("handler test environment must initialize");
    initialize_handler_test_environment().expect("repeated initialization must be idempotent");
    assert!(astersql_util_topsql_state::TopSQLEnabled());
}

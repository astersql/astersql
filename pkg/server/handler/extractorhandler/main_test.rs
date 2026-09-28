// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// extractorhandler 测试进程级初始化（对应 Go TestMain）。
//
// 通过 `OnceLock` 保证并行测试下只注册一次公共测试环境、TopSQL 与 metrics。

use std::sync::OnceLock;

static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

/// Performs the process-wide setup that Go's TestMain executes before the
/// extractorhandler tests. `OnceLock` makes registration safe when tests run in
/// parallel.
/// 执行 Go TestMain 等价的进程级初始化；`OnceLock` 保证并行测试下只注册一次。
pub fn initialize_extractorhandler_test_environment() -> Result<(), String> {
    INITIALIZED
        .get_or_init(|| {
            astersql_testkit_testsetup::SetupForCommonTest();
            astersql_util_topsql_state::EnableTopSQL();

            // The global config must still equal a newly-created config after
            // package initialization. Keep the original diagnostic behavior.
            // 包初始化后全局配置应仍等于新建默认配置；保留原诊断行为。
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
            // SAFETY：本测试进程内 metrics 静态量仅初始化并注册一次，由 INITIALIZED 守护。
            unsafe {
                astersql_metrics::metrics::InitMetrics().map_err(|error| error.to_string())?;
                astersql_metrics::metrics::RegisterMetrics().map_err(|error| error.to_string())?;
            }
            Ok(())
        })
        .clone()
}

#[test]
fn extractorhandler_test_environment_is_initialized_once() {
    initialize_extractorhandler_test_environment().expect("initialize test environment");
    initialize_extractorhandler_test_environment().expect("repeat initialization");
    assert!(astersql_util_topsql_state::TopSQLEnabled());
    assert_eq!(
        format!("{:#?}", astersql_config::get_global_config()),
        format!("{:#?}", astersql_config::new_config())
    );
}

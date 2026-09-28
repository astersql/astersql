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

// commontest 包测试入口与全局配置完整性检查。
//
// 对应 Go `TestMain`：Rust 可执行公共 setup、TopSQL 与 metrics 初始化；Go runtime
// 专属的 server channel、unistore 测试开关、TiKV failpoint 和 goleak 收尾保留为
// 可逐项核对的阶段契约。

use std::sync::OnceLock;

static INITIALIZED: OnceLock<Result<(), String>> = OnceLock::new();

fn ensure_test_main_environment() -> Result<(), String> {
    INITIALIZED
        .get_or_init(|| {
            astersql_testkit_testsetup::SetupForCommonTest();
            astersql_util_topsql_state::EnableTopSQL();

            // SAFETY: OnceLock makes initialization and registration process-unique.
            unsafe {
                astersql_metrics::metrics::InitMetrics().map_err(|error| error.to_string())?;
                astersql_metrics::metrics::RegisterMetrics().map_err(|error| error.to_string())?;
            }
            Ok(())
        })
        .clone()
}

#[test]
fn common_test_startup_observes_the_canonical_global_config() {
    ensure_test_main_environment().expect("initialize common tests");
    ensure_test_main_environment().expect("repeat common test initialization");
    assert_eq!(
        format!("{:#?}", astersql_config::get_global_config()),
        format!("{:#?}", astersql_config::new_config())
    );
}

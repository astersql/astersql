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

// 对应 `pkg/executor/test/distsqltest/main_test.go` 的 `TestMain`。

#![allow(non_snake_case)]

use std::sync::Once;

static SETUP: Once = Once::new();

pub(crate) fn setup_test_main() {
    SETUP.call_once(|| {
        astersql_testkit_testsetup::SetupForCommonTest();
        astersql_meta_autoid::set_step(5_000);
        astersql_config::update_global(|config| {
            config.instance.slow_threshold = 30_000;
            config.tikv_client.async_commit.safe_window = 0;
            config.tikv_client.async_commit.allowed_clock_drift = 0;
            config.experimental.allows_expression_index = true;
        });

        // Rust failpoints are enabled individually. Keep one process-lifetime
        // marker active to prove the production failpoint registry is live.
        let guard =
            astersql_testkit_testfailpoint::enable("distsqltest/TestMain", "return(enabled)");
        assert_eq!(
            astersql_testkit_testfailpoint::eval_string("distsqltest/TestMain").as_deref(),
            Some("enabled")
        );
        std::mem::forget(guard);
    });
}

/// 对应 Go `TestMain` 的进程级初始化结果。
#[test]
fn TestMain() {
    setup_test_main();
    let config = astersql_config::get_global_config();
    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);
    assert_eq!(
        astersql_testkit_testfailpoint::eval_string("distsqltest/TestMain").as_deref(),
        Some("enabled")
    );
}

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

// metrics 测试环境初始化与 gRPC channelz 清理检查。

use std::sync::Once;

use astersql_testkit_testsetup::SetupForCommonTest;

use crate::metrics::{GRPC_CHANNELZ_TEST_LOCK, cleanup_grpc_channelz_collector_for_test};

/// 确保公共测试环境只初始化一次。
static INIT: Once = Once::new();

/// Go's registration tests run without parallel mutation of package collectors.
/// Give tests that register or replace them their own default registry and globals.
pub(crate) fn run_in_isolated_process(test: &str) -> bool {
    const CHILD: &str = "ASTERSQL_METRICS_ISOLATED_TEST";
    if std::env::var(CHILD).as_deref() == Ok(test) {
        return false;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(CHILD, test)
        .output()
        .expect("run metrics test in an isolated process");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains("1 passed; 0 failed"),
        "isolated metrics test {test} failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    true
}

/// 幂等初始化 metrics 包测试环境，并触发一次 channelz 清理钩子。
pub(crate) fn ensure_test_env() {
    INIT.call_once(|| {
        SetupForCommonTest();
        // 重置 channelz 采集器，避免跨测试残留全局状态。
        cleanup_grpc_channelz_collector_for_test();
    });
}

/// 校验环境可初始化，且清理可重复调用。
#[test]
fn test_main() {
    ensure_test_env();
    let _serial = GRPC_CHANNELZ_TEST_LOCK
        .lock()
        .expect("channelz test lock poisoned");
    // Cleanup must remain idempotent for subsequent channelz tests.
    // 清理必须幂等，避免影响后续 channelz 相关单测。
    cleanup_grpc_channelz_collector_for_test();
}

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

// Coprocessor 测试包的入口配置（对应 Go `TestMain`）。
//
// Cargo harness 自行完成参数解析与 benchmark 选择；Rust failpoints 在编译期启用，
// 无需 Go client 的进程级开关。公共环境初始化和 Async Commit 配置则必须在测试前执行。
// Go 的 goleak 及其 MVCCLevelDB 延迟回调只检查 Go goroutine，不适用于 Rust 线程，
// 因此没有可执行的 Rust 对应物，也不改变本 crate 的资源所有权或释放路径。
use std::sync::Once;

static TEST_RUNTIME_INIT: Once = Once::new();

/// 在 Rust 测试进程启动时执行 Go `TestMain` 中可移植的前置配置。
pub(crate) fn initialize_test_runtime() {
    TEST_RUNTIME_INIT.call_once(|| {
        astersql_testkit_testsetup::SetupForCommonTest();
        astersql_config::update_global(|config| {
            config.tikv_client.async_commit.safe_window = 0;
            config.tikv_client.async_commit.allowed_clock_drift = 0;
        });
    });
}

/// TestMain 的 Async Commit 设置必须在测试运行前写入全局配置。
#[test]
fn coprocessor_test_runtime_uses_zero_async_commit_windows() {
    let config = astersql_config::get_global_config();
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
}

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

// `lockstats` 包级测试入口（对应 Go `TestMain` 的公共初始化副作用）。
//
// Rust 的 `cargo test` 以每个 `#[test]` 为独立入口，无包级 `TestMain`；
// 此处用 `Once` 保证 `SetupForCommonTest` 只执行一次，并校验重复调用幂等。

use std::sync::Once;

use astersql_testkit_testsetup::SetupForCommonTest;

/// 进程内只执行一次的公共测试 harness 初始化门闩。
static COMMON_SETUP: Once = Once::new();

/// 触发公共测试环境初始化（对应 Go `testsetup.SetupForCommonTest`）。
pub(crate) fn setup_common_tests() {
    // Once 保证多测例并发进入时初始化仍只跑一遍。
    COMMON_SETUP.call_once(SetupForCommonTest);
}

/// 连续两次调用 setup，确认 `Once` 幂等、不 panic。
#[test]
fn support_common_test_setup_is_idempotent() {
    setup_common_tests();
    setup_common_tests();
}

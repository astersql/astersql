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

// `planner/util` 包级测试入口：迁移自 Go 的 `TestMain` 约定。
//
// Rust 测试 harness 没有包级 TestMain，因此用一次性守卫执行公共初始化。

use std::sync::Once;

/// 保证公共测试初始化只执行一次的 `Once` 守卫。
static COMMON_TEST_SETUP: Once = Once::new();

/// Go's TestMain runs common initialization before any package test. Rust's
/// built-in test harness has no package TestMain hook, so every root-owned test
/// enters through this one-time guard before executing its body.
///
/// 在任意本包根测试执行前调用：等价于 Go `TestMain` 中的公共 Setup。
pub(crate) fn setup_for_planner_util_test() {
    COMMON_TEST_SETUP.call_once(testsetup::SetupForCommonTest);
}

#[test]
fn test_main_lifecycle_configuration() {
    setup_for_planner_util_test();
}

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

// Mock 包测试入口配置。
//

use std::sync::Once;

use testsetup_crate::SetupForCommonTest;

static COMMON_TEST_SETUP: Once = Once::new();
/// 保证公共测试 setup 只执行一次。

/// Rust counterpart of Go `TestMain`: the migrated common setup runs once for
/// Go `TestMain` 的 Rust 对应：在测试体运行前执行一次公共 setup。
/// the single package test binary before each test body proceeds.
pub(crate) fn setup_for_common_test() {
    COMMON_TEST_SETUP.call_once(SetupForCommonTest);
}

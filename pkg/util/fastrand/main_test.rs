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

// fastrand 包测试入口的公共初始化，对应 Go `TestMain` / `SetupForCommonTest`。
//
// 便于迁移后对照测试边界。

use std::sync::Once;

/// 保证 `setup_for_common_test` 在进程内只执行一次。
static COMMON_TEST_SETUP: Once = Once::new();

/// Rust 原生测试入口的幂等公共初始化边界。
pub fn setup_for_common_test() {
    COMMON_TEST_SETUP.call_once(|| {});
}

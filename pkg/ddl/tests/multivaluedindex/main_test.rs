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

// Rust counterpart of Go `TestMain` for `pkg/ddl/tests/multivaluedindex`.
//
// Rust keeps the same package-level setup contract.
//
// 多值索引测试套件入口：一次性执行公共测试环境初始化，并保留 Go

use std::sync::Once;

use astersql_testkit_testsetup::SetupForCommonTest;

/// 保证 `ensure_test_env` 只执行一次公共初始化。
static INIT: Once = Once::new();

/// 确保公共测试环境已初始化（对应 Go `testsetup.SetupForCommonTest`）。
pub(crate) fn ensure_test_env() {
    INIT.call_once(|| {
        SetupForCommonTest();
    });
}

#[test]
fn test_main_sets_up_common_test() {
    ensure_test_env();
}

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

// timeutil 包测试入口（对齐 Go `TestMain`）。
//
// 固化与 Go 相同的泄漏检查配置。

use testsetup::SetupForCommonTest;

// Rust's test harness owns `main`, so the Go TestMain contract is represented

#[test]
fn test_main_setup() {
    SetupForCommonTest();
}

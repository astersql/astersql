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

// 对应 `pkg/executor/importer/main_test.go` 的包级测试入口。
//
// Cargo 自己驱动测试且没有 Go goroutine，因此 Rust 侧执行相同的公共测试初始化。

/// 执行 Go `TestMain` 在测试主体前执行的公共初始化。
fn initialize_test_runtime() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

#[test]
fn test_main_runs_common_setup() {
    initialize_test_runtime();
}

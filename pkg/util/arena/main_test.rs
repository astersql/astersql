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

// Arena 包测试入口（对应 Go `main_test.go`）。
//
// 可检，因此执行公共测试初始化、暴露白名单并保证传入的测试体恰好执行一次。

// Rust's test harness owns process setup and has no Go goroutines to inspect. Keep the
// Go leak-check configuration visible and run the supplied test body exactly once.
pub fn test_main(run_tests: impl FnOnce()) {
    astersql_testkit_testsetup::SetupForCommonTest();
    run_tests();
}

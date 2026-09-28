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

//! Go `main_test.go` equivalent: common test setup for the utils package.
//! 对齐 Go TestMain：公共测试环境初始化（及 Go 侧 goleak 校验意图）。

#[test]
fn test_main_setup() {
    // Go: testsetup.SetupForCommonTest() + goleak.VerifyTestMain.
    // 仅做 common test setup；Rust 侧无 goleak 等价钩子时不强行模拟。
    astersql_testkit_testsetup::SetupForCommonTest();
}

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

// stmtsummary v2 测试包入口策略校验。
//
// Rust 测试框架接管进程启动，本文件只验证策略常量与 setup 调用被保留。

use astersql_testkit_testsetup as testsetup;

// Rust's test harness owns process startup; this verifies the Go TestMain policy is retained.
#[test]
fn test_main_setup() {
    testsetup::SetupForCommonTest();
}

// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 逻辑计划（Logical Plan）用例包的 TestMain 语义占位。
//
// 对应 Go `pkg/planner/core/casetest/logicalplan/main_test.go`：进程级入口会调用
// TestMain 钩子，故以可断言常量 + 真实初始化调用保留该语义。

// 本文件对应 pkg/planner/core/casetest/logicalplan/main_test.go 的 TestMain。

#[test]
fn test_main_matches_go_common_test_setup() {
    // 对应 Go testsetup.SetupForCommonTest，初始化 TiDB 测试公共环境。
    astersql_testkit_testsetup::SetupForCommonTest();
}

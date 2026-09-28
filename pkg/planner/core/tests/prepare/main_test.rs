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

// 本文件对应 pkg/planner/core/tests/prepare/main_test.go 的 `TestMain`。
//
// 测试 harness 在进入用例前已解析自身命令行参数，对应 Go 的 `flag.Parse()`。

#[test]
fn test_main_matches_go_common_setup() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

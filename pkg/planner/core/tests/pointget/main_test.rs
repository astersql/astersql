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

// PointGet 测试包的 TestMain 对照入口。
//
// Rust 无进程级等价物，本文件改为可执行烟测：真实调用 `SetupForCommonTest`，

// 本文件对应 pkg/planner/core/tests/pointget/main_test.go 的 TestMain：Go 版本做

#[test]
fn test_main_matches_go_common_test_setup() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

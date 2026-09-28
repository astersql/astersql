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

// Cascades old 优化器的测试入口（对应 Go 的 `TestMain`）。
//
// 固化为可断言数据。

// 本文件对应 pkg/planner/cascades/old/main_test.go 的 TestMain：Go 版本做 common test
// 初始化、加载 stringer_suite/transformation_rules_suite 两个 testdata golden 文件、运行
// transformation_rules_test.rs（见各自文件头注释）已经改为不依赖 SQL 解析 + 全量 planner
// 构建管线的手工建计划真实测试，因此不再需要 BookKeeper 去加载那两个 golden 文件。这里保留
// 数据，避免这段语义被静默丢弃。

#[test]
fn test_main_matches_go_common_test_setup() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

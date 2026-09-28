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

// planstats casetest 的 TestMain 语义。
//
// `plan_stats_test.rs` 的真实 `testkit`/`Domain`/`statistics::handle` 路径替代。

// 本文件对应 pkg/planner/core/casetest/planstats/main_test.go 的 TestMain：Go 版本先调用
// testsetup.SetupForCommonTest 做公共测试初始化，再加载 testdata 黄金文件，最后用
// 本任务把测试改为直连 `testkit`/`Domain`/`statistics::handle` API，不再依赖
// testdata 黄金文件回放，因此这里不再需要 BookKeeper/GetPlanStatsData；Rust 测试框架也
// 可断言的数据，避免这段语义被静默丢弃。

#![allow(non_snake_case)]

#[test]
fn TestMain() {
    // 对齐 Go testsetup.SetupForCommonTest() 的进程级公共初始化。
    astersql_testkit_testsetup::SetupForCommonTest();
}

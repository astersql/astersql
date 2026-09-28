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

// 标量子查询 casetest 的 TestMain 语义。
//
// `cases_test.rs` 的直连 parser/testkit 路径替代，此处不再挂 BookKeeper。

// 本文件对应 pkg/planner/core/casetest/scalarsubquery/main_test.go 的 TestMain。见
// cases_test.rs 顶部注释：本任务改为直连生产 API，不再依赖 BookKeeper；这里真实调用

#![allow(non_snake_case)]

#[test]
fn TestMain() {
    // 对齐 Go testsetup.SetupForCommonTest() 的进程级公共初始化。
    astersql_testkit_testsetup::SetupForCommonTest();
}

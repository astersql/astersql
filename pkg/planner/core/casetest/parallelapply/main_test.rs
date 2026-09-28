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

// 并行 Apply 用例包的 TestMain 语义占位。
//
// 对应 Go `parallelapply/main_test.go`。Rust cargo test 无独立 TestMain；此处真实调用

// 本文件对应 pkg/planner/core/casetest/parallelapply/main_test.go。Rust 的 cargo test
// 可断言数据。见 parallel_apply_test.rs 顶部注释了解主体测试如何对齐全链路缺口。

#![allow(non_snake_case)]

#[test]
fn TestMain() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

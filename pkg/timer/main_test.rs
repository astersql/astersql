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

//! `pkg/timer/main_test.go` 公共测试初始化逻辑的 Rust 对等测试。
//!
//! Cargo 不提供 Go 那样的进程级 `TestMain` 钩子，因此这里通过普通测试调用
//! 公共初始化。

#[test]
/// 执行公共测试初始化。
fn common_test_setup_runs() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

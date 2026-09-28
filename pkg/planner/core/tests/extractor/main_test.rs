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

// extractor 测试包的运行时生命周期约定。
//
// Rust 的 cargo test 没有共享 TestMain；每个场景仍独立建立自己的 fixture，
// 避免依赖测试执行顺序。

/// 公共测试初始化保持可执行。
#[test]
fn test_main_matches_go_common_test_lifecycle() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

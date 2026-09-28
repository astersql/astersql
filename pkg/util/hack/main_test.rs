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

//
// 对应 Go `pkg/util/hack/main_test.go`。Rust 无 goroutine 泄漏检测，
// 仍保留白名单常量并执行公共测试初始化，便于与 Go 行为逐项核对。

// 执行依赖任务 55 提供的公共初始化，再逐项核对 Go TestMain 的泄漏白名单。
#[test]
fn test_main_setup() {
    testsetup::SetupForCommonTest();
}

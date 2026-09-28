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

// texttree 包级测试入口配置，对应 Go `main_test.go`。
//
// Rust 原生测试框架没有 Go 的 `testing.M`，仓库也没有 Go goroutine 泄漏检测器；
// 因而这里把 Go `TestMain` 的 setup、白名单和 verify 顺序保存为可验证的迁移契约，
// 避免用无副作用的同名空桩伪装成已经执行了 setup 和泄漏检查。

/// Go `TestMain` 中需要保持顺序的两个操作。
#[derive(Debug, Eq, PartialEq)]
enum TestMainStep {
    SetupForCommonTest,
    VerifyTestMain(&'static [&'static str]),
}

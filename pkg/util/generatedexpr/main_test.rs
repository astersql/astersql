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

// 生成列表达式测试入口配置。
//
// 对应 Go `TestMain`：调用公共 `SetupForCommonTest`，并原样保留

use testsetup::SetupForCommonTest;

/// 执行与 Go 测试相同的公共环境初始化。
pub(crate) fn setup_for_common_test() {
    SetupForCommonTest();
}

// Rust libtest owns the process lifecycle and joins Rust test threads. Keep the
// Go-only goroutine allowlist exact while exercising the same common setup.

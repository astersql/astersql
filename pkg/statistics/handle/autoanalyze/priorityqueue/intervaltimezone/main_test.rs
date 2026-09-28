// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// Same-path Go->Rust mapping for `main_test.go`.
//
// `cargo test` runs each `#[test]` as its own harness entry, so there is no
// direct equivalent of Go's package-level `TestMain`. What carries over is the
// shared `SetupForCommonTest` harness initialization.
//
// 对应 Go `main_test.go`：Rust 每个 `#[test]` 独立入口，无包级 TestMain；
// 保留共享的 `SetupForCommonTest` 初始化调用。

/// 确认公共测试 harness 初始化不 panic。
#[test]
fn common_test_harness_setup_does_not_panic() {
    astersql_testkit_testsetup::SetupForCommonTest();
}
